//! The async networking node: a libp2p swarm driven on its own tokio task, fronted by a cheap,
//! cloneable [`NodeHandle`].
//!
//! ## Model
//! * Commands flow *in* over an mpsc channel ([`Command`]).
//! * Events flow *out* over an mpsc channel ([`NodeEvent`]).
//! * The swarm itself lives entirely inside [`run_swarm`], polled in a `select!` loop, and is
//!   never touched from another thread. This keeps the (non-`Sync`) swarm single-owner and means
//!   nothing the caller does can block the event loop.
//!
//! ## Layering
//! Everything crossing the boundary is opaque `Vec<u8>`. The node has no idea what poker is.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use libp2p::{
    futures::StreamExt,
    gossipsub::{self, IdentTopic, TopicHash},
    mdns,
    multiaddr::Protocol,
    request_response::{self, OutboundRequestId},
    swarm::SwarmEvent,
    Multiaddr, PeerId, Swarm,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::{
    behaviour::{BehaviourBuildError, PokerBehaviour, PokerBehaviourEvent, TABLE_TOPIC},
    encode_table_uri,
    reachability::{self, Reachability},
    UriError,
};

/// Events surfaced to the application layer. Opaque bytes only.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A table broadcast arrived. `from` is the gossipsub message source (the publishing peer),
    /// which may differ from the peer that relayed it to us.
    Message { from: PeerId, data: Vec<u8> },
    /// A RELIABLE deal payload arrived point-to-point (request-response). `author` is the ORIGINAL
    /// author carried in the envelope — the host fans a guest's payload out to other guests, so the
    /// author differs from `relayed_by` (the transport sender). The application MUST attribute the
    /// payload to `author`, not `relayed_by`; the embedded proof binds it to that author's seat key.
    DealReceived {
        author: PeerId,
        relayed_by: PeerId,
        data: Vec<u8>,
    },
    /// A peer connection was established.
    PeerConnected(PeerId),
    /// A peer connection was fully closed (no remaining connections to that peer).
    PeerDisconnected(PeerId),
    /// A new local listen address became available (informational).
    NewListenAddr(Multiaddr),
    /// The host's shareable table URI was (re)assembled from the best dialable addresses. Emitted
    /// for hosts as addresses are discovered (listen addrs, UPnP external addr, etc.).
    TableUriReady(String),
    /// The best address we can offer remote invitees looks non-routable (CGNAT/private/loopback).
    /// Direct hosting across the internet will not work without a manual port-forward. This is
    /// best-of-{external, listen} reachability, so on a normal home host it can include the LAN
    /// listen address before UPnP has had a chance to confirm a public mapping; it is therefore
    /// deferred until UPnP reports a terminal state (gateway-not-found / non-routable) or a short
    /// grace timer elapses, so it does not fire spuriously on the LAN-only startup transient.
    ReachabilityWarning(Reachability),
    /// An outbound dial failed. Surfaced so a `join()` to an unreachable host does not hang
    /// silently forever — the application must watch for this (and/or apply its own timeout) and
    /// can decide whether to retry or abort. `peer` and `addr` are best-effort context.
    DialFailed {
        peer: Option<PeerId>,
        addr: Option<Multiaddr>,
        error: String,
    },
    /// UPnP-IGD mapped a port and discovered our external address: remote hosting should work
    /// without a manual port-forward. Informational (the address is also folded into the URI).
    UpnpExternalAddr(Multiaddr),
    /// UPnP-IGD is unavailable — no IGD gateway was found, or the gateway's own external address is
    /// non-routable (e.g. CGNAT). Remote hosting will need a public-IP host or a manual port-forward.
    UpnpUnavailable,
}

/// Commands sent to the swarm task. Internal; drive these through [`NodeHandle`].
enum Command {
    /// Publish opaque bytes to the table topic.
    Broadcast {
        data: Vec<u8>,
        ack: oneshot::Sender<Result<(), NetError>>,
    },
    /// Reliably deliver one deal envelope to a specific peer (request-response). The `ack` resolves
    /// `Ok` once that peer's response arrives, or `Err(DealFailed)` on an outbound failure; the
    /// caller owns retry-until-delivered.
    SendDeal {
        peer: PeerId,
        envelope: DealEnvelope,
        ack: oneshot::Sender<Result<(), NetError>>,
    },
    /// Dial an additional address (e.g. a host address from a join URI).
    Dial {
        addr: Multiaddr,
        ack: oneshot::Sender<Result<(), NetError>>,
    },
    /// Fetch the current best table URI, if any addresses are known yet.
    CurrentUri {
        ack: oneshot::Sender<Option<String>>,
    },
    /// Stop the swarm task.
    Shutdown,
}

/// A deal payload plus the PeerId of its ORIGINAL author, carried over request-response. The host
/// relays a guest's payload to other guests, so the transport sender is not the author; carrying the
/// author lets the application attribute the payload correctly (its embedded proof still binds it to
/// that author's seat key, so a lying relay cannot forge one). The net layer treats `data` as opaque.
#[derive(Clone, Debug)]
pub struct DealEnvelope {
    pub author: PeerId,
    pub data: Vec<u8>,
}

impl DealEnvelope {
    /// Pack into the request frame: `[u16 author-len][author bytes][data]`.
    fn to_wire(&self) -> Vec<u8> {
        let author = self.author.to_bytes();
        let mut out = Vec::with_capacity(2 + author.len() + self.data.len());
        out.extend_from_slice(&(author.len() as u16).to_be_bytes());
        out.extend_from_slice(&author);
        out.extend_from_slice(&self.data);
        out
    }

    /// Parse a request frame produced by [`to_wire`]. Returns `None` on a malformed/truncated frame.
    fn from_wire(bytes: &[u8]) -> Option<DealEnvelope> {
        if bytes.len() < 2 {
            return None;
        }
        let alen = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let rest = &bytes[2..];
        if rest.len() < alen {
            return None;
        }
        let author = PeerId::from_bytes(&rest[..alen]).ok()?;
        Some(DealEnvelope {
            author,
            data: rest[alen..].to_vec(),
        })
    }
}

/// Errors from the networking layer.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("failed to build behaviour: {0}")]
    Behaviour(#[from] BehaviourBuildError),
    #[error("failed to build swarm transport: {0}")]
    Transport(String),
    #[error("failed to subscribe to the table topic: {0}")]
    Subscribe(String),
    #[error("failed to listen on {addr}: {source}")]
    Listen {
        addr: Multiaddr,
        #[source]
        source: libp2p::TransportError<std::io::Error>,
    },
    #[error("failed to publish to the table: {0}")]
    Publish(#[from] gossipsub::PublishError),
    /// Non-fatal: gossipsub accepted the call but no connected peer has signalled a subscription to
    /// the table topic yet (the mesh has not formed, or nobody has joined). The very common case
    /// right after creating a table or right after a join lands — GRAFT/subscription state has not
    /// propagated. Callers should treat this as "retry shortly", NOT as a hard failure: a single
    /// `broadcast()` returning `Ok` never guaranteed delivery, and this is the flip side of that.
    #[error("no peers have subscribed to the table topic yet (retry shortly)")]
    NoSubscribersYet,
    #[error("failed to dial {addr}: {source}")]
    Dial {
        addr: Multiaddr,
        #[source]
        source: libp2p::swarm::DialError,
    },
    #[error("invalid table URI: {0}")]
    Uri(#[from] UriError),
    #[error("reliable deal delivery to {peer} failed: {reason}")]
    DealFailed { peer: PeerId, reason: String },
    #[error("the networking task has shut down")]
    NodeStopped,
}

/// A cheap, cloneable handle to a running node. All methods are async and never block the swarm.
#[derive(Clone)]
pub struct NodeHandle {
    local_peer_id: PeerId,
    commands: mpsc::Sender<Command>,
    /// The spawned swarm task's join handle, shared so [`NodeHandle::shutdown`] can AWAIT the
    /// task's real termination (releasing its listening sockets + mDNS responder) before
    /// returning. Wrapped in `Arc<Mutex<Option<..>>>` so the handle stays cheaply cloneable; the
    /// first `shutdown` takes the join handle (subsequent ones are no-ops). A `std::sync::Mutex`
    /// is fine — it is only ever held briefly to swap the `Option`, never across an `.await`.
    swarm_task: std::sync::Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl NodeHandle {
    /// This node's libp2p [`PeerId`].
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Broadcast opaque bytes to every peer at the table. Resolves once the swarm has accepted the
    /// publish (NOT once peers have received it).
    ///
    /// Right after a table is created, or right after a join lands, gossipsub may not yet have
    /// exchanged subscription state, so this returns `Err(NetError::NoSubscribersYet)` — a benign,
    /// retryable condition, NOT a hard failure. Callers should retry on a short ticker until at
    /// least one peer is meshed; treat only the other `NetError` variants as fatal.
    pub async fn broadcast(&self, data: Vec<u8>) -> Result<(), NetError> {
        let (ack, rx) = oneshot::channel();
        self.commands
            .send(Command::Broadcast { data, ack })
            .await
            .map_err(|_| NetError::NodeStopped)?;
        rx.await.map_err(|_| NetError::NodeStopped)?
    }

    /// Reliably deliver one deal payload to a specific connected peer (request-response). Resolves
    /// `Ok` once the peer's application has received it, `Err(DealFailed)` on an outbound failure
    /// (the caller retries — a single failure is benign, e.g. a transient pre-mesh window). `author`
    /// is the original author the receiver must attribute the payload to (the host passes a guest's
    /// PeerId when fanning out; a peer passes its own when sending its own contribution).
    pub async fn send_deal(
        &self,
        peer: PeerId,
        author: PeerId,
        data: Vec<u8>,
    ) -> Result<(), NetError> {
        let (ack, rx) = oneshot::channel();
        self.commands
            .send(Command::SendDeal {
                peer,
                envelope: DealEnvelope { author, data },
                ack,
            })
            .await
            .map_err(|_| NetError::NodeStopped)?;
        rx.await.map_err(|_| NetError::NodeStopped)?
    }

    /// Dial an additional peer address (each should carry a `/p2p/<PeerId>` suffix).
    pub async fn dial(&self, addr: Multiaddr) -> Result<(), NetError> {
        let (ack, rx) = oneshot::channel();
        self.commands
            .send(Command::Dial { addr, ack })
            .await
            .map_err(|_| NetError::NodeStopped)?;
        rx.await.map_err(|_| NetError::NodeStopped)?
    }

    /// The current best shareable table URI, if any dialable addresses are known yet. For a host
    /// this becomes populated as listen/UPnP addresses are discovered. Prefer subscribing to
    /// [`NodeEvent::TableUriReady`] to be notified as it improves.
    pub async fn current_table_uri(&self) -> Result<Option<String>, NetError> {
        let (ack, rx) = oneshot::channel();
        self.commands
            .send(Command::CurrentUri { ack })
            .await
            .map_err(|_| NetError::NodeStopped)?;
        rx.await.map_err(|_| NetError::NodeStopped)
    }

    /// Ask the swarm task to stop AND wait for it to actually terminate.
    ///
    /// This is more than fire-and-forget: after sending `Shutdown` it AWAITS the swarm task's
    /// `JoinHandle`, so by the time this returns the swarm — and with it every listening socket,
    /// QUIC/TCP port, and the mDNS responder — has been fully dropped. That matters for
    /// back-to-back in-process runs (notably the integration tests): without it, a "stopped" node
    /// could linger on the runtime, keep answering mDNS on loopback, and perturb the next table's
    /// mesh formation. Idempotent: only the first call takes + joins the task; later calls and a
    /// call after the task already exited are no-ops. Best-effort on the command send (the task
    /// may already be gone).
    pub async fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown).await;
        // Take the join handle (if we are the first caller) and await termination.
        let task = self.swarm_task.lock().unwrap().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// A running node: the handle plus the receiver of outbound events. The caller owns the event
/// stream and drains it; dropping it does not stop the swarm (use [`NodeHandle::shutdown`]).
pub struct Node {
    /// Cloneable command handle.
    pub handle: NodeHandle,
    /// Outbound event stream.
    pub events: mpsc::Receiver<NodeEvent>,
}

/// How big the command/event channels are. Generous for a 9-player table.
const CHANNEL_CAP: usize = 256;

/// How long a host waits before warning about non-routable reachability, to give slow UPnP-IGD
/// gateway discovery a chance to confirm a public mapping first (MEDIUM-2).
const REACHABILITY_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

/// Tunables for a node, separate from its identity. Defaults match real LAN play; tests override
/// them (e.g. to disable mDNS so concurrent loopback tables do not auto-discover each other).
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Enable mDNS same-LAN zero-config discovery. `true` for real play (so phones/laptops on the
    /// same Wi-Fi find each other without a URI). `false` for in-process tests, which dial
    /// explicitly via the table URI: there, mDNS only causes unrelated test tables on loopback to
    /// auto-mesh and perturb each other's gossipsub mesh formation.
    pub enable_mdns: bool,
    /// Fixed TCP+UDP listen port, or `None` for an OS-assigned ephemeral port. A remote host behind
    /// NAT sets this to a known port it has manually forwarded on its router, so the same port works
    /// run-to-run. Guests (which only dial outbound) leave it `None`.
    pub listen_port: Option<u16>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        // Real play: mDNS on, ephemeral port.
        NodeConfig {
            enable_mdns: true,
            listen_port: None,
        }
    }
}

/// Start a host node: generate (or use the supplied) identity, listen on QUIC+TCP, subscribe to
/// the table topic, and begin assembling the shareable URI from discovered addresses.
///
/// Pass `keypair = None` to generate a fresh ed25519 identity. Uses the default [`NodeConfig`]
/// (mDNS enabled); use [`host_with_config`] to override.
pub fn host(keypair: Option<libp2p::identity::Keypair>) -> Result<Node, NetError> {
    host_with_config(keypair, NodeConfig::default())
}

/// Like [`host`] but with an explicit [`NodeConfig`] (e.g. mDNS disabled for tests).
pub fn host_with_config(
    keypair: Option<libp2p::identity::Keypair>,
    config: NodeConfig,
) -> Result<Node, NetError> {
    // A host's table topic is scoped to ITS OWN PeerId (see `table_topic`), so two unrelated
    // tables — even on the same host/LAN, auto-meshed by mDNS — never share a gossipsub mesh and
    // cannot deliver each other's traffic. We must know the local PeerId before subscribing, so we
    // resolve the keypair here and pass the explicit host PeerId down.
    let keypair = keypair.unwrap_or_else(libp2p::identity::Keypair::generate_ed25519);
    let host_peer = PeerId::from(keypair.public());
    spawn_node(Some(keypair), /* is_host */ true, &[], host_peer, config)
}

/// Start an invitee node from a `tcpoker://` URI: decode it, listen locally, subscribe to the
/// table topic, and dial the host address(es).
///
/// The dial is outbound and asynchronous. If the host is down or unreachable, the swarm surfaces a
/// [`NodeEvent::DialFailed`] on the event stream rather than failing this call — so the caller MUST
/// watch the event stream (and/or apply its own timeout / `PeerConnected` deadline) to detect an
/// unreachable host; otherwise it will wait indefinitely for a connection that never forms.
pub fn join(uri: &str, keypair: Option<libp2p::identity::Keypair>) -> Result<Node, NetError> {
    join_with_config(uri, keypair, NodeConfig::default())
}

/// Like [`join`] but with an explicit [`NodeConfig`] (e.g. mDNS disabled for tests).
pub fn join_with_config(
    uri: &str,
    keypair: Option<libp2p::identity::Keypair>,
    config: NodeConfig,
) -> Result<Node, NetError> {
    let dial_addrs = crate::decode_table_uri(uri)?;
    // Scope our table topic to the HOST's PeerId (carried in the URI's `/p2p/...`), so we join the
    // mesh of exactly the table we dialed and never the global mesh of every table on the LAN.
    let host_peer = dial_addrs
        .iter()
        .find_map(peer_id_of)
        .ok_or(NetError::Uri(UriError::Malformed))?;
    spawn_node(
        keypair,
        /* is_host */ false,
        &dial_addrs,
        host_peer,
        config,
    )
}

/// The gossipsub topic for the table hosted by `host_peer`. Scoping the topic per host PeerId
/// (rather than one global [`TABLE_TOPIC`]) isolates concurrent tables: two unrelated tables on
/// the same host or LAN — which mDNS will auto-mesh into one swarm — subscribe to DIFFERENT
/// topics, so neither receives the other's `StartHand`/`Act`/`HandComplete` traffic. This closes
/// the cross-table contamination that otherwise wedges concurrent tables (CRITICAL-1).
fn table_topic(host_peer: &PeerId) -> IdentTopic {
    IdentTopic::new(format!("{TABLE_TOPIC}/{host_peer}"))
}

fn spawn_node(
    keypair: Option<libp2p::identity::Keypair>,
    is_host: bool,
    dial_addrs: &[Multiaddr],
    host_peer: PeerId,
    config: NodeConfig,
) -> Result<Node, NetError> {
    let keypair = keypair.unwrap_or_else(libp2p::identity::Keypair::generate_ed25519);
    let local_peer_id = PeerId::from(keypair.public());

    let mut swarm = build_swarm(keypair, config.enable_mdns)?;

    // Subscribe to this table's (host-scoped) topic.
    let topic = table_topic(&host_peer);
    swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .map_err(|e| NetError::Subscribe(e.to_string()))?;

    // Listen on QUIC and TCP. `port = 0` is an OS-assigned ephemeral port (the default); a fixed
    // port lets a NAT'd host forward a stable port for remote play.
    let port = config.listen_port.unwrap_or(0);
    for addr in [
        format!("/ip4/0.0.0.0/udp/{port}/quic-v1")
            .parse::<Multiaddr>()
            .unwrap(),
        format!("/ip4/0.0.0.0/tcp/{port}")
            .parse::<Multiaddr>()
            .unwrap(),
    ] {
        swarm
            .listen_on(addr.clone())
            .map_err(|source| NetError::Listen { addr, source })?;
    }

    // Dial host addresses (join path). add_peer_address helps gossipsub reach them.
    for addr in dial_addrs {
        if let Some(peer) = peer_id_of(addr) {
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
        }
        // Errors here are non-fatal; the loop will keep listening and retry on demand.
        let _ = swarm.dial(addr.clone());
    }

    let (cmd_tx, cmd_rx) = mpsc::channel(CHANNEL_CAP);
    let (evt_tx, evt_rx) = mpsc::channel(CHANNEL_CAP);

    // Retain the swarm task's join handle so `NodeHandle::shutdown` can AWAIT real termination
    // (releasing sockets + the mDNS responder) instead of fire-and-forgetting the Shutdown command.
    let swarm_task = tokio::spawn(run_swarm(
        swarm,
        topic.hash(),
        local_peer_id,
        is_host,
        cmd_rx,
        evt_tx,
    ));

    let handle = NodeHandle {
        local_peer_id,
        commands: cmd_tx,
        swarm_task: std::sync::Arc::new(Mutex::new(Some(swarm_task))),
    };

    Ok(Node {
        handle,
        events: evt_rx,
    })
}

/// Build the swarm: tokio runtime, TCP+noise+yamux AND QUIC, with DNS, and our behaviour.
fn build_swarm(
    keypair: libp2p::identity::Keypair,
    enable_mdns: bool,
) -> Result<Swarm<PokerBehaviour>, NetError> {
    let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|e| NetError::Transport(e.to_string()))?
        .with_quic()
        .with_dns()
        .map_err(|e| NetError::Transport(e.to_string()))?
        .with_behaviour(|kp| {
            PokerBehaviour::with_mdns(kp, enable_mdns)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        })
        .map_err(|e| NetError::Transport(e.to_string()))?
        // Keep idle connections alive: a poker table can sit quiet between actions.
        .with_swarm_config(|c| {
            c.with_idle_connection_timeout(std::time::Duration::from_secs(600))
        })
        .build();
    Ok(swarm)
}

/// Extract the trailing `/p2p/<PeerId>` from a multiaddr, if present.
fn peer_id_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(peer) => Some(peer),
        _ => None,
    })
}

/// Tracks the addresses we've learned so we can (re)build the host URI.
struct AddrBook {
    local_peer_id: PeerId,
    /// Local listen addresses (`0.0.0.0` is expanded to concrete LAN IPs by the swarm).
    listen: Vec<Multiaddr>,
    /// UPnP / externally-confirmed addresses (the gold standard for a remote-dialable host).
    external: Vec<Multiaddr>,
}

impl AddrBook {
    fn new(local_peer_id: PeerId) -> Self {
        Self {
            local_peer_id,
            listen: Vec::new(),
            external: Vec::new(),
        }
    }

    fn add_listen(&mut self, addr: Multiaddr) {
        if !self.listen.contains(&addr) {
            self.listen.push(addr);
        }
    }

    fn add_external(&mut self, addr: Multiaddr) {
        if !self.external.contains(&addr) {
            self.external.push(addr);
        }
    }

    /// Strip any existing `/p2p` and re-append our own — gives a clean dialable address.
    fn with_self_p2p(&self, addr: &Multiaddr) -> Multiaddr {
        let mut a: Multiaddr = addr
            .iter()
            .filter(|p| !matches!(p, Protocol::P2p(_)))
            .collect();
        a.push(Protocol::P2p(self.local_peer_id));
        a
    }

    /// Is this a QUIC multiaddr?
    fn is_quic(addr: &Multiaddr) -> bool {
        addr.iter().any(|p| matches!(p, Protocol::QuicV1))
    }

    /// Build the best shareable URI: prefer UPnP-confirmed external addresses, otherwise
    /// non-loopback LAN addresses. Carry BOTH a QUIC and a TCP address when available.
    /// Returns `None` if we have nothing dialable yet.
    fn best_uri(&self) -> Option<String> {
        // Candidate pool: external first (routable), then non-loopback listen addresses.
        let routable_external: Vec<&Multiaddr> = self
            .external
            .iter()
            .filter(|a| reachability::is_internet_routable(a))
            .collect();

        let lan_listen: Vec<&Multiaddr> = self
            .listen
            .iter()
            .filter(|a| {
                matches!(
                    reachability::classify_multiaddr(a),
                    Some(Reachability::PubliclyRoutable) | Some(Reachability::PrivateLan)
                )
            })
            .collect();

        // Choose source pool: external if any are routable, otherwise LAN listen addresses.
        let pool: Vec<&Multiaddr> = if !routable_external.is_empty() {
            // Combine routable external + LAN (so same-LAN invitees still work too).
            routable_external.into_iter().chain(lan_listen).collect()
        } else {
            // No routable external; fall back to LAN, but also include any non-loopback external
            // (e.g. a CGNAT addr) so at least the URI is non-empty.
            lan_listen
                .into_iter()
                .chain(self.external.iter().filter(|a| {
                    !matches!(
                        reachability::classify_multiaddr(a),
                        Some(Reachability::Loopback)
                    )
                }))
                .collect()
        };

        if pool.is_empty() {
            return None;
        }

        // Pick one QUIC and one TCP address, dedup'd, each with our /p2p suffix.
        let mut chosen: Vec<Multiaddr> = Vec::new();
        let mut have_quic = false;
        let mut have_tcp = false;
        for a in &pool {
            let is_quic = Self::is_quic(a);
            if is_quic && !have_quic {
                chosen.push(self.with_self_p2p(a));
                have_quic = true;
            } else if !is_quic && !have_tcp {
                chosen.push(self.with_self_p2p(a));
                have_tcp = true;
            }
            if have_quic && have_tcp {
                break;
            }
        }
        // If we only found one transport family, still emit what we have.
        if chosen.is_empty() {
            chosen.push(self.with_self_p2p(pool[0]));
        }
        // Dedup (an external addr might equal a listen addr).
        let mut seen = HashSet::new();
        chosen.retain(|a| seen.insert(a.clone()));

        Some(encode_table_uri(&chosen))
    }

    /// The worst-case reachability of our external/listen addresses, for warning the user.
    /// Returns the *best* class we can offer; if that is not routable, the caller warns.
    fn best_reachability(&self) -> Option<Reachability> {
        self.external
            .iter()
            .chain(self.listen.iter())
            .filter_map(reachability::classify_multiaddr)
            .min_by_key(|r| match r {
                Reachability::PubliclyRoutable => 0,
                Reachability::PrivateLan => 1,
                Reachability::CgnatLikely => 2,
                Reachability::Loopback => 3,
            })
    }
}

/// Emit an event to the application WITHOUT ever blocking the swarm loop.
///
/// HIGH-2: the previous code did `events.send(..).await` from inside the `select!` arm that polls
/// the swarm. If the application drained `events` slowly (or stopped), that await blocked the whole
/// loop — no swarm polling, no command servicing, ping/keepalive stalls, peers eventually drop —
/// directly contradicting the "nothing the caller does can block the event loop" contract.
///
/// Events are advisory; the swarm's own state is the source of truth. So we `try_send`: if the
/// 256-slot channel is momentarily full we drop the event rather than stall the entire node. A
/// disconnected receiver simply means the app dropped its event stream; we keep running (it can
/// still be shut down via the command channel). Callers MUST still drain `events` promptly to avoid
/// dropped notifications, but a slow/stopped consumer can no longer wedge the swarm.
fn emit(events: &mpsc::Sender<NodeEvent>, ev: NodeEvent) {
    use mpsc::error::TrySendError;
    match events.try_send(ev) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => { /* advisory event dropped under backpressure */ }
        Err(TrySendError::Closed(_)) => { /* app dropped the event stream; keep the swarm alive */ }
    }
}

/// The swarm task. Owns the swarm; pumps commands in and events out. Never blocks.
async fn run_swarm(
    mut swarm: Swarm<PokerBehaviour>,
    topic: TopicHash,
    local_peer_id: PeerId,
    is_host: bool,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<NodeEvent>,
) {
    let mut book = AddrBook::new(local_peer_id);
    let mut last_uri: Option<String> = None;
    let mut warned = false;
    // Outstanding reliable-deal sends, keyed by the request id, so an inbound response (or outbound
    // failure) can resolve the caller's `send_deal` future.
    let mut pending_deal_acks: HashMap<OutboundRequestId, oneshot::Sender<Result<(), NetError>>> =
        HashMap::new();

    // MEDIUM-2: a grace timer for the reachability warning. UPnP-IGD gateway discovery is slow, so
    // we do not warn the instant we see a LAN-only listen address. We wait either for UPnP to report
    // a terminal "no help" state (handled in the behaviour event path) or for this timer to elapse,
    // whichever comes first — by then `best_reachability()` reflects the real terminal state.
    let reachability_grace = tokio::time::sleep(REACHABILITY_GRACE);
    tokio::pin!(reachability_grace);
    let mut grace_armed = is_host;

    loop {
        tokio::select! {
            // Reachability grace timer elapsed: now it is fair to warn if we still have no routable
            // address. Only relevant for a host that hasn't already warned.
            _ = &mut reachability_grace, if grace_armed && !warned => {
                grace_armed = false;
                maybe_warn(&book, &mut warned, &events);
            }
            cmd = commands.recv() => {
                match cmd {
                    Some(Command::Broadcast { data, ack }) => {
                        let res = match swarm.behaviour_mut().gossipsub.publish(topic.clone(), data) {
                            Ok(_) => Ok(()),
                            // Distinguish the benign "mesh not formed yet" cases from real failures
                            // so callers can retry instead of aborting. `Duplicate` means we already
                            // published these exact bytes (our own re-broadcast) — also benign.
                            Err(gossipsub::PublishError::NoPeersSubscribedToTopic)
                            | Err(gossipsub::PublishError::AllQueuesFull(_)) => {
                                Err(NetError::NoSubscribersYet)
                            }
                            Err(gossipsub::PublishError::Duplicate) => Ok(()),
                            Err(e) => Err(NetError::Publish(e)),
                        };
                        let _ = ack.send(res);
                    }
                    Some(Command::SendDeal { peer, envelope, ack }) => {
                        let id = swarm
                            .behaviour_mut()
                            .deal
                            .send_request(&peer, envelope.to_wire());
                        pending_deal_acks.insert(id, ack);
                    }
                    Some(Command::Dial { addr, ack }) => {
                        if let Some(peer) = peer_id_of(&addr) {
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                        }
                        let res = swarm
                            .dial(addr.clone())
                            .map_err(|source| NetError::Dial { addr, source });
                        let _ = ack.send(res);
                    }
                    Some(Command::CurrentUri { ack }) => {
                        let _ = ack.send(last_uri.clone());
                    }
                    Some(Command::Shutdown) | None => break,
                }
            }
            event = swarm.select_next_some() => {
                handle_swarm_event(
                    event,
                    &mut swarm,
                    is_host,
                    &mut book,
                    &mut last_uri,
                    &mut warned,
                    &mut pending_deal_acks,
                    &events,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_swarm_event(
    event: SwarmEvent<PokerBehaviourEvent>,
    swarm: &mut Swarm<PokerBehaviour>,
    is_host: bool,
    book: &mut AddrBook,
    last_uri: &mut Option<String>,
    warned: &mut bool,
    pending_deal_acks: &mut HashMap<OutboundRequestId, oneshot::Sender<Result<(), NetError>>>,
    events: &mpsc::Sender<NodeEvent>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            emit(events, NodeEvent::NewListenAddr(address.clone()));
            book.add_listen(address);
            if is_host {
                maybe_emit_uri(book, last_uri, events);
            }
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            book.add_external(address);
            if is_host {
                maybe_emit_uri(book, last_uri, events);
            }
        }
        SwarmEvent::NewExternalAddrCandidate { address } => {
            // identify reports the address peers observe us at. We do NOT blindly trust it as
            // confirmed (a peer could lie, and without AutoNAT we can't prove routability), but a
            // host behind a manual static port-forward has no UPnP and no other way to learn its
            // public IP. Confirming candidates lets such a host publish a dialable URI. Only
            // confirm internet-routable candidates so we never advertise a peer's view of our LAN.
            if is_host && reachability::is_internet_routable(&address) {
                swarm.add_external_address(address.clone());
                book.add_external(address);
                maybe_emit_uri(book, last_uri, events);
            }
        }
        SwarmEvent::OutgoingConnectionError {
            peer_id, error, ..
        } => {
            // HIGH-1: surface dial failures so a `join()` to an unreachable host does not hang
            // silently forever. Advisory: the caller decides whether to retry or abort.
            emit(
                events,
                NodeEvent::DialFailed {
                    peer: peer_id,
                    addr: None,
                    error: error.to_string(),
                },
            );
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            // Make sure gossipsub considers this peer for the mesh.
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
            emit(events, NodeEvent::PeerConnected(peer_id));
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established,
            ..
        } => {
            if num_established == 0 {
                emit(events, NodeEvent::PeerDisconnected(peer_id));
            }
        }
        SwarmEvent::Behaviour(ev) => {
            handle_behaviour_event(
                ev,
                swarm,
                is_host,
                book,
                last_uri,
                warned,
                pending_deal_acks,
                events,
            );
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_behaviour_event(
    ev: PokerBehaviourEvent,
    swarm: &mut Swarm<PokerBehaviour>,
    is_host: bool,
    book: &mut AddrBook,
    last_uri: &mut Option<String>,
    warned: &mut bool,
    pending_deal_acks: &mut HashMap<OutboundRequestId, oneshot::Sender<Result<(), NetError>>>,
    events: &mpsc::Sender<NodeEvent>,
) {
    match ev {
        PokerBehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. }) => {
            // Source is the original publisher (signed); fall back to nothing if anonymous.
            // `source` is always `Some` here: ValidationMode::Strict + Signed authenticity rejects
            // unsigned inbound before it becomes a behaviour event and always stamps the author.
            // The `if let` is intentional defensive code, not a real message-loss path.
            if let Some(from) = message.source {
                emit(
                    events,
                    NodeEvent::Message {
                        from,
                        data: message.data,
                    },
                );
            }
        }
        PokerBehaviourEvent::Deal(request_response::Event::Message { peer, message, .. }) => {
            match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // Reliable inbound deal frame. Ack ONLY if we successfully hand the payload to
                    // the application: send the event first and respond on success. If the event
                    // channel is momentarily full we DROP the request without acking, so the sender
                    // times out and retries — a backpressure drop can never silently lose a deal
                    // frame (closing the gap that ack-on-receipt would otherwise open).
                    if let Some(env) = DealEnvelope::from_wire(&request) {
                        let ev = NodeEvent::DealReceived {
                            author: env.author,
                            relayed_by: peer,
                            data: env.data,
                        };
                        if events.try_send(ev).is_ok() {
                            let _ = swarm.behaviour_mut().deal.send_response(channel, Vec::new());
                        }
                    }
                    // A malformed frame is dropped without acking; the sender will retry/expire.
                }
                request_response::Message::Response { request_id, .. } => {
                    if let Some(tx) = pending_deal_acks.remove(&request_id) {
                        let _ = tx.send(Ok(()));
                    }
                }
            }
        }
        PokerBehaviourEvent::Deal(request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        }) => {
            if let Some(tx) = pending_deal_acks.remove(&request_id) {
                let _ = tx.send(Err(NetError::DealFailed {
                    peer,
                    reason: error.to_string(),
                }));
            }
        }
        // InboundFailure / ResponseSent: nothing to do (the sender's outbound side drives retries).
        PokerBehaviourEvent::Deal(_) => {}
        PokerBehaviourEvent::Mdns(mdns::Event::Discovered(peers)) => {
            // Same-LAN auto-discovery: add and dial so gossipsub can mesh with them.
            for (peer, addr) in peers {
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                let _ = swarm.dial(addr);
            }
        }
        PokerBehaviourEvent::Upnp(upnp_ev) => match upnp_ev {
            libp2p::upnp::Event::NewExternalAddr(addr) => {
                emit(events, NodeEvent::UpnpExternalAddr(addr.clone()));
                book.add_external(addr);
                if is_host {
                    maybe_emit_uri(book, last_uri, events);
                }
            }
            libp2p::upnp::Event::GatewayNotFound | libp2p::upnp::Event::NonRoutableGateway => {
                // Terminal "no UPnP help" signal: the URI falls back to LAN addresses, so it is now
                // fair to warn (MEDIUM-2) without waiting for the grace timer. `maybe_warn` is
                // one-shot, so this races harmlessly with the grace timer — whichever fires first.
                emit(events, NodeEvent::UpnpUnavailable);
                if is_host {
                    maybe_warn(book, warned, events);
                }
            }
            libp2p::upnp::Event::ExpiredExternalAddr(addr) => {
                book.external.retain(|a| a != &addr);
            }
        },
        _ => {}
    }
}

/// Recompute the URI and emit a `TableUriReady` event if it changed. Reachability warnings are NOT
/// raised here anymore (see [`maybe_warn`] / MEDIUM-2): emitting them on every address discovery
/// fired on the LAN-only startup transient before UPnP had a chance to confirm a public mapping.
fn maybe_emit_uri(book: &AddrBook, last_uri: &mut Option<String>, events: &mpsc::Sender<NodeEvent>) {
    if let Some(uri) = book.best_uri() {
        if last_uri.as_deref() != Some(uri.as_str()) {
            *last_uri = Some(uri.clone());
            emit(events, NodeEvent::TableUriReady(uri));
        }
    }
}

/// Emit a one-shot reachability warning if our best address is not internet-routable.
///
/// MEDIUM-2: this is now driven by a *terminal* signal — UPnP reporting `GatewayNotFound` /
/// `NonRoutableGateway`, or a grace timer elapsing — rather than by the first `NewListenAddr`. On a
/// normal home host the first non-loopback listen addr is the LAN IP (`PrivateLan`), so firing on
/// it told the user "hosting won't work" even when UPnP was about to succeed. By the time we reach
/// here UPnP has had its say, so `best_reachability()` reflects the true terminal state.
fn maybe_warn(book: &AddrBook, warned: &mut bool, events: &mpsc::Sender<NodeEvent>) {
    if *warned {
        return;
    }
    if let Some(best) = book.best_reachability() {
        if !best.is_internet_routable() {
            *warned = true;
            emit(events, NodeEvent::ReachabilityWarning(best));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    fn pid() -> PeerId {
        PeerId::from(Keypair::generate_ed25519().public())
    }

    #[test]
    fn uri_built_from_routable_external_addrs() {
        let peer = pid();
        let mut book = AddrBook::new(peer);
        book.add_listen("/ip4/127.0.0.1/udp/4001/quic-v1".parse().unwrap());
        book.add_listen("/ip4/192.168.1.5/tcp/4001".parse().unwrap());
        book.add_external("/ip4/203.0.113.7/udp/4001/quic-v1".parse().unwrap());
        book.add_external("/ip4/203.0.113.7/tcp/4001".parse().unwrap());

        let uri = book.best_uri().expect("should produce a URI");
        let addrs = crate::decode_table_uri(&uri).unwrap();
        // Should carry both a QUIC and a TCP routable address, each with our /p2p.
        assert!(addrs.iter().any(|a| AddrBook::is_quic(a)));
        assert!(addrs.iter().any(|a| !AddrBook::is_quic(a)));
        for a in &addrs {
            assert_eq!(peer_id_of(a), Some(peer));
            // No loopback in the published URI.
            assert_ne!(
                reachability::classify_multiaddr(a),
                Some(Reachability::Loopback)
            );
        }
    }

    #[test]
    fn uri_falls_back_to_lan_when_no_external() {
        let peer = pid();
        let mut book = AddrBook::new(peer);
        book.add_listen("/ip4/127.0.0.1/tcp/4001".parse().unwrap());
        book.add_listen("/ip4/192.168.1.5/tcp/4001".parse().unwrap());
        book.add_listen("/ip4/192.168.1.5/udp/4001/quic-v1".parse().unwrap());

        let uri = book.best_uri().expect("LAN fallback URI");
        let addrs = crate::decode_table_uri(&uri).unwrap();
        assert!(!addrs.is_empty());
        for a in &addrs {
            assert_eq!(
                reachability::classify_multiaddr(a),
                Some(Reachability::PrivateLan),
                "should only publish LAN addrs: {a}"
            );
            assert_eq!(peer_id_of(a), Some(peer));
        }
    }

    #[test]
    fn no_uri_from_loopback_only() {
        let mut book = AddrBook::new(pid());
        book.add_listen("/ip4/127.0.0.1/tcp/4001".parse().unwrap());
        book.add_listen("/ip4/127.0.0.1/udp/4001/quic-v1".parse().unwrap());
        assert!(book.best_uri().is_none());
    }

    #[test]
    fn reachability_warning_on_cgnat_only() {
        let mut book = AddrBook::new(pid());
        book.add_external("/ip4/100.70.0.1/udp/4001/quic-v1".parse().unwrap());
        assert_eq!(book.best_reachability(), Some(Reachability::CgnatLikely));
        assert!(!book.best_reachability().unwrap().is_internet_routable());
    }

    #[test]
    fn deal_envelope_wire_roundtrips() {
        let author = pid();
        let env = DealEnvelope {
            author,
            data: vec![1, 2, 3, 4, 5],
        };
        let wire = env.to_wire();
        let back = DealEnvelope::from_wire(&wire).expect("should parse");
        assert_eq!(back.author, author);
        assert_eq!(back.data, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn deal_envelope_wire_roundtrips_empty_data() {
        // An empty payload (the closest analogue to the empty-ack response) must round-trip.
        let author = pid();
        let env = DealEnvelope {
            author,
            data: Vec::new(),
        };
        let back = DealEnvelope::from_wire(&env.to_wire()).expect("should parse");
        assert_eq!(back.author, author);
        assert!(back.data.is_empty());
    }

    #[test]
    fn deal_envelope_rejects_truncated_frames() {
        assert!(DealEnvelope::from_wire(&[]).is_none());
        assert!(DealEnvelope::from_wire(&[0]).is_none());
        // Claims a 9-byte author but only 3 bytes follow.
        assert!(DealEnvelope::from_wire(&[0, 9, 1, 2, 3]).is_none());
    }
}
