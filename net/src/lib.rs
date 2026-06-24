//! Peer-to-peer networking and table discovery.
//!
//! Ten-Cent Poker has no servers and no central directory. A table is found by sharing a
//! single **table URI** out-of-band (email, chat, QR) that encodes everything a peer needs
//! to dial in: the host's dialable `Multiaddr`(s), each ending in `/p2p/<PeerId>`. This
//! crate is unverified async glue built on `libp2p`; only URI encode/decode is pure (and
//! could later move to the verified core).
//!
//! The serverless reach is bounded by NAT physics (see the project plan): a direct dial
//! works for same-LAN, public-IP/VPS hosts, and home hosts whose router speaks UPnP-IGD.
//! The swarm, transports, host bootstrap (UPnP mapping + external-address discovery +
//! CGNAT detection), and host-as-hub message forwarding land in M3; M0 provides the URI
//! codec and the `tcpoker://` scheme.

use libp2p::{identity::Keypair, multiaddr::Protocol, Multiaddr, PeerId};

pub mod behaviour;
pub mod node;
pub mod reachability;

pub use behaviour::{BehaviourBuildError, PokerBehaviour, TABLE_TOPIC};
pub use node::{
    host, host_with_config, join, join_with_config, NetError, Node, NodeConfig, NodeEvent,
    NodeHandle,
};
pub use reachability::{
    classify_ip, classify_multiaddr, is_internet_routable, Reachability,
};

/// URI scheme used to hand a poker table to another player.
pub const SCHEME: &str = "tcpoker://";

/// Errors from parsing a table URI.
#[derive(Debug, thiserror::Error)]
pub enum UriError {
    #[error("table URI is missing the `{SCHEME}` scheme")]
    MissingScheme,
    #[error("table URI payload is not valid base58: {0}")]
    Base58(#[from] bs58::decode::Error),
    #[error("table URI payload is malformed (truncated length-prefixed blob)")]
    Malformed,
    #[error("invalid multiaddr in table URI: {0}")]
    Multiaddr(#[from] libp2p::multiaddr::Error),
}

/// Encode one or more dialable `Multiaddr`s (each should already carry a `/p2p/<PeerId>`
/// suffix) into a single shareable table URI.
///
/// Addresses are packed into a length-prefixed binary blob and base58-encoded, so the URI
/// is one compact, copy-paste-safe token with no `/` or whitespace to be mangled by chat
/// clients — and it can carry both a QUIC and a TCP address for the same host.
pub fn encode_table_uri(addrs: &[Multiaddr]) -> String {
    let mut blob = Vec::new();
    for addr in addrs {
        let bytes = addr.to_vec();
        // u16 big-endian length prefix; multiaddrs are far shorter than 64 KiB.
        blob.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        blob.extend_from_slice(&bytes);
    }
    format!("{SCHEME}{}", bs58::encode(blob).into_string())
}

/// Parse a table URI back into the host's dialable `Multiaddr`(s).
pub fn decode_table_uri(uri: &str) -> Result<Vec<Multiaddr>, UriError> {
    let payload = uri.strip_prefix(SCHEME).ok_or(UriError::MissingScheme)?;
    let blob = bs58::decode(payload).into_vec()?;

    let mut addrs = Vec::new();
    let mut i = 0;
    while i < blob.len() {
        if i + 2 > blob.len() {
            return Err(UriError::Malformed);
        }
        let len = u16::from_be_bytes([blob[i], blob[i + 1]]) as usize;
        i += 2;
        if i + len > blob.len() {
            return Err(UriError::Malformed);
        }
        addrs.push(Multiaddr::try_from(blob[i..i + len].to_vec())?);
        i += len;
    }
    if addrs.is_empty() {
        return Err(UriError::Malformed);
    }
    Ok(addrs)
}

/// Build a fresh, self-contained table URI from a newly generated peer identity.
/// (In the real app the `Multiaddr`s come from the listening swarm + UPnP discovery;
/// here it is a loopback QUIC address so the result is well-formed and round-trippable.)
pub fn sample_table_uri() -> String {
    let keypair = Keypair::generate_ed25519();
    let peer_id = PeerId::from(keypair.public());
    let base: Multiaddr = "/ip4/127.0.0.1/udp/9000/quic-v1"
        .parse()
        .expect("static multiaddr is valid");
    encode_table_uri(&[base.with(Protocol::P2p(peer_id))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_uri_roundtrips() {
        let peer_id = PeerId::from(Keypair::generate_ed25519().public());
        let base: Multiaddr = "/ip4/127.0.0.1/udp/9000/quic-v1".parse().unwrap();
        let full = base.with(Protocol::P2p(peer_id));

        let uri = encode_table_uri(std::slice::from_ref(&full));
        assert!(uri.starts_with(SCHEME));

        assert_eq!(decode_table_uri(&uri).unwrap(), vec![full]);
    }

    #[test]
    fn table_uri_carries_multiple_addrs() {
        let peer_id = PeerId::from(Keypair::generate_ed25519().public());
        let quic = "/ip4/203.0.113.7/udp/9000/quic-v1"
            .parse::<Multiaddr>()
            .unwrap()
            .with(Protocol::P2p(peer_id));
        let tcp = "/ip4/203.0.113.7/tcp/9000"
            .parse::<Multiaddr>()
            .unwrap()
            .with(Protocol::P2p(peer_id));

        let uri = encode_table_uri(&[quic.clone(), tcp.clone()]);
        assert_eq!(decode_table_uri(&uri).unwrap(), vec![quic, tcp]);
    }

    #[test]
    fn decode_rejects_bad_scheme() {
        assert!(matches!(
            decode_table_uri("https://example.com"),
            Err(UriError::MissingScheme)
        ));
    }

    #[test]
    fn sample_uri_decodes() {
        assert_eq!(decode_table_uri(&sample_table_uri()).unwrap().len(), 1);
    }
}
