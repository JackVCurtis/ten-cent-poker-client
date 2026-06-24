//! Focused unit tests for the CGNAT / reachability classifier (`poker_net::classify_ip` &
//! friends). These exercise only the pure, public API and must not touch `net/src`.
//!
//! Coverage required by M3:
//!   - public IPv4 (203.0.113.7)            => PubliclyRoutable
//!   - RFC 1918 (10/8, 192.168/16, 172.16-31/12) => PrivateLan
//!   - RFC 6598 100.64.0.0/10               => CgnatLikely (incl. boundaries)
//!   - 127.0.0.1                            => Loopback

use std::net::IpAddr;

use poker_net::{classify_ip, classify_multiaddr, Reachability};

fn ip(s: &str) -> IpAddr {
    s.parse().expect("valid IP literal")
}

#[test]
fn public_ipv4_is_publicly_routable() {
    assert_eq!(classify_ip(ip("203.0.113.7")), Reachability::PubliclyRoutable);
    assert!(classify_ip(ip("203.0.113.7")).is_internet_routable());
}

#[test]
fn rfc1918_ten_is_private_lan() {
    // 10.0.0.0/8
    for s in ["10.0.0.1", "10.255.255.254", "10.1.2.3"] {
        assert_eq!(classify_ip(ip(s)), Reachability::PrivateLan, "{s}");
        assert!(!classify_ip(ip(s)).is_internet_routable(), "{s}");
    }
}

#[test]
fn rfc1918_192_168_is_private_lan() {
    // 192.168.0.0/16
    for s in ["192.168.0.1", "192.168.1.10", "192.168.255.254"] {
        assert_eq!(classify_ip(ip(s)), Reachability::PrivateLan, "{s}");
    }
}

#[test]
fn rfc1918_172_16_31_is_private_lan() {
    // 172.16.0.0/12 == 172.16.x .. 172.31.x
    for s in ["172.16.0.1", "172.20.5.5", "172.31.255.254"] {
        assert_eq!(classify_ip(ip(s)), Reachability::PrivateLan, "{s}");
    }
    // Just outside the /12 must NOT be classified as private LAN.
    assert_ne!(classify_ip(ip("172.15.0.1")), Reachability::PrivateLan);
    assert_ne!(classify_ip(ip("172.32.0.1")), Reachability::PrivateLan);
}

#[test]
fn rfc6598_cgnat_typical_address() {
    // 100.64.0.0/10 — typical CGNAT-assigned address.
    assert_eq!(classify_ip(ip("100.64.1.2")), Reachability::CgnatLikely);
    assert_eq!(classify_ip(ip("100.100.50.50")), Reachability::CgnatLikely);
    // Not routable across the open internet.
    assert!(!classify_ip(ip("100.64.1.2")).is_internet_routable());
}

#[test]
fn rfc6598_cgnat_boundaries() {
    // Inclusive lower boundary of 100.64.0.0/10.
    assert_eq!(classify_ip(ip("100.64.0.0")), Reachability::CgnatLikely);
    // Inclusive upper boundary: 100.127.255.255 is the last address inside /10.
    assert_eq!(
        classify_ip(ip("100.127.255.255")),
        Reachability::CgnatLikely,
        "100.127.255.255 must be inside the /10"
    );

    // Just outside the lower edge: 100.63.255.255 is NOT CGNAT.
    assert_ne!(classify_ip(ip("100.63.255.255")), Reachability::CgnatLikely);
    // Just outside the upper edge: 100.128.0.0 is NOT CGNAT (it is public space).
    assert_ne!(
        classify_ip(ip("100.128.0.0")),
        Reachability::CgnatLikely,
        "100.128.0.0 must be outside the /10"
    );
    assert_eq!(
        classify_ip(ip("100.128.0.0")),
        Reachability::PubliclyRoutable
    );
}

#[test]
fn loopback_v4_detected() {
    assert_eq!(classify_ip(ip("127.0.0.1")), Reachability::Loopback);
    assert!(!classify_ip(ip("127.0.0.1")).is_internet_routable());
}

#[test]
fn multiaddr_classifier_agrees_with_ip_classifier() {
    let cases = [
        ("/ip4/203.0.113.7/udp/9000/quic-v1", Reachability::PubliclyRoutable),
        ("/ip4/10.0.0.1/tcp/9000", Reachability::PrivateLan),
        ("/ip4/192.168.1.10/tcp/9000", Reachability::PrivateLan),
        ("/ip4/172.16.0.1/tcp/9000", Reachability::PrivateLan),
        ("/ip4/100.64.1.2/tcp/9000", Reachability::CgnatLikely),
        ("/ip4/100.127.255.255/tcp/9000", Reachability::CgnatLikely),
        ("/ip4/127.0.0.1/tcp/9000", Reachability::Loopback),
    ];
    for (ma, expected) in cases {
        let addr: libp2p::Multiaddr = ma.parse().expect("valid multiaddr");
        assert_eq!(classify_multiaddr(&addr), Some(expected), "{ma}");
    }
}
