//! The composed libp2p [`NetworkBehaviour`] for a poker table.
//!
//! Layering note: this behaviour only ever moves **opaque `Vec<u8>`** over gossipsub. It knows
//! nothing about poker rules, envelopes, or app-level signatures — those belong to
//! `poker-protocol` (M4). gossipsub messages *are* signed by the local libp2p node key (so peers
//! can't forge each other's `source` on the wire), but that is purely a transport property; the
//! application must still verify its own signatures on top.

use std::time::Duration;

use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{
    gossipsub, identify, mdns, ping, request_response,
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour},
    upnp, PeerId, StreamProtocol,
};

/// The single gossipsub topic every player at a table subscribes to. All table traffic is
/// broadcast here.
///
/// Host-as-hub relaying (client A -> host -> client B, who are not directly connected in the star
/// topology) is load-bearing and rests on TWO things, not on `flood_publish` alone:
///   1. The node adds every connected peer as a gossipsub *explicit peer*
///      (`add_explicit_peer`, in `node.rs`). gossipsub's `forward_msg` forwards a *received*
///      message to explicit peers + mesh peers, so the host re-forwards A's message to B even
///      before a mesh forms. `flood_publish` only governs messages the host *originates*, NOT ones
///      it forwards — so it alone would not relay client traffic.
///   2. Content-addressed message ids (see `message_id_fn`) + a long `duplicate_cache_time` so the
///      host's re-forward dedups instead of looping.
/// Explicit peers are never removed; at <=9 players the monotonic growth across reconnects is
/// negligible.
pub const TABLE_TOPIC: &str = "tcpoker/table/v1";

/// Identify protocol version advertised to peers.
const IDENTIFY_PROTOCOL: &str = "/tcpoker/id/1.0.0";

/// Request-response protocol for RELIABLE, point-to-point delivery of the trustless-deal payloads
/// (key/shuffle/reveal). gossipsub is best-effort and lossy; the mental-poker deal rounds need
/// every-peer delivery, so they ride this instead (the host fans a guest's payload out to each
/// other guest, retrying until delivered). See `poker_protocol::driver`.
pub const DEAL_PROTOCOL: &str = "/tcpoker/deal/1.0.0";

/// Hard cap on a single deal frame. The largest payload is a Bayer–Groth shuffle proof; 1 MiB is
/// comfortably above it and bounds a malicious/corrupt length prefix.
const MAX_DEAL_FRAME: usize = 1 << 20;

/// A minimal length-prefixed `Vec<u8>` codec for the deal request-response protocol. The payload is
/// already a serialized envelope (author + `TableMessage` bytes); we move it verbatim, so there is
/// nothing to encode beyond a `[u32 big-endian length][bytes]` frame. The response is an empty ack
/// (a zero-length frame): its arrival means the peer received the request.
#[derive(Clone, Default)]
pub struct DealCodec;

#[async_trait]
impl request_response::Codec for DealCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> std::io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io).await
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> std::io::Result<Vec<u8>>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_frame(io).await
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        req: Vec<u8>,
    ) -> std::io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &req).await
    }

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        res: Vec<u8>,
    ) -> std::io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_frame(io, &res).await
    }
}

async fn read_frame<T: AsyncRead + Unpin + Send>(io: &mut T) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    io.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_DEAL_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "deal frame exceeds maximum size",
        ));
    }
    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame<T: AsyncWrite + Unpin + Send>(io: &mut T, data: &[u8]) -> std::io::Result<()> {
    if data.len() > MAX_DEAL_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "deal frame exceeds maximum size",
        ));
    }
    io.write_all(&(data.len() as u32).to_be_bytes()).await?;
    io.write_all(data).await?;
    io.flush().await?;
    Ok(())
}

/// The combined behaviour. The derive macro generates `PokerBehaviourEvent` (one variant per
/// field) and a `From` impl for each sub-behaviour's event.
#[derive(NetworkBehaviour)]
pub struct PokerBehaviour {
    /// Opaque-byte table broadcast.
    pub gossipsub: gossipsub::Behaviour,
    /// Peer metadata + observed-address exchange. The observed address surfaces as a
    /// `NewExternalAddrCandidate`; `node.rs` confirms it (via `add_external_address`) when it is
    /// internet-routable, so a host with a manual static port-forward but no UPnP-IGD can still
    /// learn and publish its public address.
    pub identify: identify::Behaviour,
    /// Liveness.
    pub ping: ping::Behaviour,
    /// Zero-config same-LAN discovery. Toggleable: real LAN play keeps it ON, but in-process
    /// tests turn it OFF so unrelated test tables on loopback do not auto-mesh and perturb each
    /// other (the tests dial explicitly via the table URI, so mDNS is pure interference there).
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    /// UPnP-IGD port mapping AND external-address discovery (no STUN).
    pub upnp: upnp::tokio::Behaviour,
    /// Reliable point-to-point delivery for trustless-deal payloads (see [`DEAL_PROTOCOL`]).
    pub deal: request_response::Behaviour<DealCodec>,
}

impl PokerBehaviour {
    /// Construct the full behaviour stack from the local node keypair, with mDNS ENABLED (the
    /// default for real LAN play).
    pub fn new(keypair: &libp2p::identity::Keypair) -> Result<Self, BehaviourBuildError> {
        Self::with_mdns(keypair, true)
    }

    /// Construct the behaviour stack, choosing whether mDNS same-LAN discovery is enabled.
    /// Real play passes `true`; in-process tests pass `false` so concurrent loopback tables do
    /// not auto-discover and mesh into each other.
    pub fn with_mdns(
        keypair: &libp2p::identity::Keypair,
        enable_mdns: bool,
    ) -> Result<Self, BehaviourBuildError> {
        let local_peer_id = PeerId::from(keypair.public());

        // gossipsub tuned for a *small* group (2..=9) in a star around the host. We deliberately
        // favour delivery over bandwidth-efficiency: flood-publish so a node fans its OWN messages
        // out to all known subscribers regardless of mesh membership, a permissive mesh, and
        // content-addressed message ids so duplicates dedup. NOTE: flood_publish only affects
        // self-originated messages; client<->client relay through the host relies on the explicit-
        // peer set (see TABLE_TOPIC doc), not on flood_publish.
        let message_id_fn = |message: &gossipsub::Message| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            message.data.hash(&mut hasher);
            // Include source so two players sending identical bytes aren't collapsed.
            if let Some(src) = message.source {
                src.hash(&mut hasher);
            }
            message.sequence_number.hash(&mut hasher);
            gossipsub::MessageId::from(hasher.finish().to_be_bytes().to_vec())
        };

        let gossip_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1))
            // Sign every message with our node key: peers can't spoof the wire-level `source`.
            // This is NOT the app-level signature (that is M4).
            .validation_mode(gossipsub::ValidationMode::Strict)
            .message_id_fn(message_id_fn)
            // Fan out to every known subscriber, not just mesh peers — critical for the star
            // topology where the host is the only common peer.
            .flood_publish(true)
            // Small-group mesh: allow a fully-connected handful of peers.
            .mesh_n_low(1)
            .mesh_n(4)
            .mesh_n_high(12)
            .mesh_outbound_min(0)
            // Keep dedup memory long enough to swallow the host's re-broadcast of our own message.
            .duplicate_cache_time(Duration::from_secs(60))
            .build()
            .map_err(BehaviourBuildError::GossipsubConfig)?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossip_config,
        )
        .map_err(BehaviourBuildError::Gossipsub)?;

        let identify = identify::Behaviour::new(identify::Config::new(
            IDENTIFY_PROTOCOL.to_string(),
            keypair.public(),
        ));

        let ping = ping::Behaviour::new(ping::Config::new());

        let mdns: Toggle<mdns::tokio::Behaviour> = if enable_mdns {
            Some(
                mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
                    .map_err(BehaviourBuildError::Mdns)?,
            )
            .into()
        } else {
            None.into()
        };

        let upnp = upnp::tokio::Behaviour::default();

        // Reliable deal delivery. A 10s request timeout is generous versus the driver's ~400ms
        // retransmit cadence (the driver owns retry-until-delivered), so a slow handshake completes
        // rather than churning; the concurrency cap covers a few in-flight frames per peer at a
        // small table.
        let deal = request_response::Behaviour::with_codec(
            DealCodec,
            std::iter::once((
                StreamProtocol::new(DEAL_PROTOCOL),
                request_response::ProtocolSupport::Full,
            )),
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(10))
                .with_max_concurrent_streams(64),
        );

        Ok(Self {
            gossipsub,
            identify,
            ping,
            mdns,
            upnp,
            deal,
        })
    }
}

/// Errors constructing the [`PokerBehaviour`] stack.
#[derive(Debug, thiserror::Error)]
pub enum BehaviourBuildError {
    #[error("invalid gossipsub config: {0}")]
    GossipsubConfig(#[source] gossipsub::ConfigBuilderError),
    #[error("failed to build gossipsub behaviour: {0}")]
    Gossipsub(&'static str),
    #[error("failed to build mdns behaviour: {0}")]
    Mdns(#[source] std::io::Error),
}
