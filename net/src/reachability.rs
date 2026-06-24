//! Pure, dependency-light classification of how reachable an address is.
//!
//! Ten-Cent Poker is *bounded-serverless*: only the host needs to be dialable, and there is no
//! STUN/relay to paper over NAT. So when we assemble the host's shareable URI we want to know,
//! for each candidate address, whether a remote invitee on the open internet could actually reach
//! it. This module answers that purely from the IP, with no I/O, so it is trivially unit-testable.
//!
//! The interesting case is **CGNAT** (carrier-grade NAT, RFC 6598 `100.64.0.0/10`): an address in
//! that range looks "external" to UPnP-IGD but is *not* globally routable, so direct hosting will
//! silently fail. We surface it as its own class so the host can warn the user.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use libp2p::{multiaddr::Protocol, Multiaddr};

/// How reachable an IP address is from the public internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    /// A globally-routable public address. A remote invitee can dial this directly.
    PubliclyRoutable,
    /// RFC 1918 (IPv4 private) or RFC 4193 (IPv6 unique-local) LAN address. Reachable only from
    /// the same network; fine for same-LAN play, useless across the internet.
    PrivateLan,
    /// RFC 6598 `100.64.0.0/10` shared address space — the hallmark of carrier-grade NAT. Looks
    /// external but is **not** routable; direct hosting needs a manual port-forward / different
    /// network.
    CgnatLikely,
    /// Loopback (`127.0.0.0/8` / `::1`). Only reachable from the same machine.
    Loopback,
}

impl Reachability {
    /// Whether a remote peer on the open internet could plausibly dial an address of this class.
    pub fn is_internet_routable(self) -> bool {
        matches!(self, Reachability::PubliclyRoutable)
    }
}

/// Classify a raw [`IpAddr`].
pub fn classify_ip(ip: IpAddr) -> Reachability {
    match ip {
        IpAddr::V4(v4) => classify_ipv4(v4),
        IpAddr::V6(v6) => classify_ipv6(v6),
    }
}

fn classify_ipv4(ip: Ipv4Addr) -> Reachability {
    let o = ip.octets();
    if ip.is_loopback() {
        Reachability::Loopback
    } else if o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000 {
        // 100.64.0.0/10 (RFC 6598) — second octet 64..=127.
        Reachability::CgnatLikely
    } else if ip.is_private() || ip.is_link_local() {
        // 10/8, 172.16/12, 192.168/16 (RFC 1918) and 169.254/16 link-local (RFC 3927).
        Reachability::PrivateLan
    } else if o[0] == 0 || ip.is_broadcast() || ip.is_unspecified() {
        // Non-routable specials we should never advertise as a dialable host address.
        // (Note: RFC 5737 documentation ranges like 203.0.113.0/24 are intentionally treated as
        // routable — they are the canonical "stand-in public IP" and a real deployment never sees
        // them; classifying them as private would only mask test intent.)
        Reachability::PrivateLan
    } else {
        Reachability::PubliclyRoutable
    }
}

fn classify_ipv6(ip: Ipv6Addr) -> Reachability {
    if ip.is_loopback() {
        return Reachability::Loopback;
    }
    // Map IPv4-mapped/compatible addresses (::ffff:a.b.c.d) onto their v4 classification.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_ipv4(v4);
    }
    let seg = ip.segments();
    if (seg[0] & 0xfe00) == 0xfc00 {
        // fc00::/7 unique-local (RFC 4193) — the IPv6 analogue of RFC 1918.
        Reachability::PrivateLan
    } else if (seg[0] & 0xffc0) == 0xfe80 {
        // fe80::/10 link-local.
        Reachability::PrivateLan
    } else if ip.is_unspecified() {
        Reachability::PrivateLan
    } else {
        Reachability::PubliclyRoutable
    }
}

/// Extract the IP component of a [`Multiaddr`] and classify it. Returns `None` for multiaddrs that
/// carry no IP (e.g. a pure DNS or memory address) — those are not something we can classify here.
pub fn classify_multiaddr(addr: &Multiaddr) -> Option<Reachability> {
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(classify_ipv4(ip)),
        Protocol::Ip6(ip) => Some(classify_ipv6(ip)),
        _ => None,
    })
}

/// Whether a [`Multiaddr`] is plausibly dialable by a remote internet peer. A multiaddr with no IP
/// component (e.g. `/dns4/...`) is treated as routable, since DNS names generally resolve to public
/// addresses and we cannot say otherwise without resolving.
pub fn is_internet_routable(addr: &Multiaddr) -> bool {
    match classify_multiaddr(addr) {
        Some(r) => r.is_internet_routable(),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ma(s: &str) -> Multiaddr {
        s.parse().expect("valid multiaddr")
    }

    #[test]
    fn public_ipv4_is_routable() {
        assert_eq!(
            classify_ip("203.0.113.7".parse().unwrap()),
            Reachability::PubliclyRoutable
        );
        assert_eq!(
            classify_ip("8.8.8.8".parse().unwrap()),
            Reachability::PubliclyRoutable
        );
    }

    #[test]
    fn rfc1918_is_private() {
        for s in ["10.0.0.5", "172.16.4.1", "172.31.255.1", "192.168.1.10"] {
            assert_eq!(
                classify_ip(s.parse().unwrap()),
                Reachability::PrivateLan,
                "{s}"
            );
        }
    }

    #[test]
    fn link_local_v4_is_private() {
        assert_eq!(
            classify_ip("169.254.10.10".parse().unwrap()),
            Reachability::PrivateLan
        );
    }

    #[test]
    fn cgnat_range_detected() {
        // 100.64.0.0/10 = 100.64.x.x .. 100.127.x.x
        for s in ["100.64.0.1", "100.100.50.50", "100.127.255.254"] {
            assert_eq!(
                classify_ip(s.parse().unwrap()),
                Reachability::CgnatLikely,
                "{s}"
            );
        }
        // Just outside the /10 must NOT be CGNAT.
        assert_eq!(
            classify_ip("100.63.255.255".parse().unwrap()),
            Reachability::PubliclyRoutable
        );
        assert_eq!(
            classify_ip("100.128.0.0".parse().unwrap()),
            Reachability::PubliclyRoutable
        );
    }

    #[test]
    fn loopback_detected() {
        assert_eq!(
            classify_ip("127.0.0.1".parse().unwrap()),
            Reachability::Loopback
        );
        assert_eq!(classify_ip("::1".parse().unwrap()), Reachability::Loopback);
    }

    #[test]
    fn ipv6_unique_local_is_private() {
        assert_eq!(
            classify_ip("fd00::1".parse().unwrap()),
            Reachability::PrivateLan
        );
        assert_eq!(
            classify_ip("fe80::1".parse().unwrap()),
            Reachability::PrivateLan
        );
    }

    #[test]
    fn ipv6_public_is_routable() {
        assert_eq!(
            classify_ip("2606:4700:4700::1111".parse().unwrap()),
            Reachability::PubliclyRoutable
        );
    }

    #[test]
    fn multiaddr_classification_pulls_ip() {
        assert_eq!(
            classify_multiaddr(&ma("/ip4/203.0.113.7/udp/9000/quic-v1")),
            Some(Reachability::PubliclyRoutable)
        );
        assert_eq!(
            classify_multiaddr(&ma("/ip4/192.168.1.5/tcp/9000")),
            Some(Reachability::PrivateLan)
        );
        assert_eq!(
            classify_multiaddr(&ma("/ip4/100.70.0.1/tcp/9000")),
            Some(Reachability::CgnatLikely)
        );
        // No IP component.
        assert_eq!(classify_multiaddr(&ma("/dns4/example.com/tcp/9000")), None);
    }

    #[test]
    fn internet_routable_helper() {
        assert!(is_internet_routable(&ma("/ip4/8.8.8.8/tcp/9000")));
        assert!(!is_internet_routable(&ma("/ip4/192.168.0.2/tcp/9000")));
        assert!(!is_internet_routable(&ma("/ip4/127.0.0.1/tcp/9000")));
        assert!(!is_internet_routable(&ma("/ip4/100.80.0.1/tcp/9000")));
        // DNS addrs are assumed routable.
        assert!(is_internet_routable(&ma("/dns4/example.com/tcp/9000")));
    }
}
