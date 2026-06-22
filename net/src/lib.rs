//! Peer-to-peer networking and table discovery.
//!
//! Ten-Cent Poker has no servers and no central directory. A table is found by sharing a
//! single **table URI** out-of-band (email, chat, QR) that encodes everything a peer needs
//! to dial in: a transport `Multiaddr` ending in `/p2p/<PeerId>`. This crate is unverified
//! async glue built on `libp2p`; only URI encode/decode is pure (and could later move to
//! the verified core).

use libp2p::{identity::Keypair, multiaddr::Protocol, Multiaddr, PeerId};

/// URI scheme used to hand a poker table to another player.
pub const SCHEME: &str = "tencentpoker:";

/// Encode a dialable `Multiaddr` (which should already carry a `/p2p/<PeerId>` suffix)
/// into a shareable table URI.
pub fn encode_table_uri(addr: &Multiaddr) -> String {
    format!("{SCHEME}{addr}")
}

/// Parse a table URI back into a `Multiaddr`.
pub fn decode_table_uri(uri: &str) -> Result<Multiaddr, Box<dyn std::error::Error>> {
    let rest = uri
        .strip_prefix(SCHEME)
        .ok_or("table URI is missing the `tencentpoker:` scheme")?;
    Ok(rest.parse::<Multiaddr>()?)
}

/// Build a fresh, self-contained table URI from a newly generated peer identity.
/// (In the real app the `Multiaddr` comes from the listening swarm; here it is a
/// loopback QUIC address so the result is well-formed and round-trippable.)
pub fn sample_table_uri() -> String {
    let keypair = Keypair::generate_ed25519();
    let peer_id = PeerId::from(keypair.public());
    let base: Multiaddr = "/ip4/127.0.0.1/udp/9000/quic-v1"
        .parse()
        .expect("static multiaddr is valid");
    encode_table_uri(&base.with(Protocol::P2p(peer_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_uri_roundtrips() {
        let keypair = Keypair::generate_ed25519();
        let peer_id = PeerId::from(keypair.public());
        let base: Multiaddr = "/ip4/127.0.0.1/udp/9000/quic-v1".parse().unwrap();
        let full = base.with(Protocol::P2p(peer_id));

        let uri = encode_table_uri(&full);
        assert!(uri.starts_with(SCHEME));

        let decoded = decode_table_uri(&uri).unwrap();
        assert_eq!(decoded, full);
    }

    #[test]
    fn decode_rejects_bad_scheme() {
        assert!(decode_table_uri("https://example.com").is_err());
    }
}
