//! Async glue: bind a running [`poker_net::Node`] to the pure replicated [`Table`].
//!
//! The driver owns the [`poker_net::Node`] (its [`NodeHandle`] + event stream). It:
//! - decodes inbound [`NodeEvent::Message`] bytes to [`TableMessage`] (with the
//!   cryptographically authenticated `from`) and feeds them to the [`Table`];
//! - broadcasts the table's outbound messages, retrying on
//!   [`NetError::NoSubscribersYet`](poker_net::NetError) with a short ticker until the mesh forms;
//! - on the local turn, asks the injected [`Strategy`] and broadcasts the resulting `Act`;
//! - reacts to peer connect/disconnect: the HOST collects the lobby and emits
//!   [`TableMessage::StartHand`] once enough peers are seated; a mid-hand disconnect aborts the
//!   live hand cleanly (documented MVP rule).
//!
//! Two entry points: [`run_host`] (creates a table, prints/returns the URI, plays N hands) and
//! [`run_guest`] (joins a URI and plays). Both return a [`GameReport`] summarizing the hands.
//!
//! # Determinism / replication
//! The host is the only peer that emits [`TableMessage::StartHand`] and advances the button, so
//! every peer's [`Table`] is fed an identical authoritative message stream and computes
//! identical outcomes. Betting actions are each peer's own broadcast, applied identically
//! everywhere (see [`crate::table`]).
//!
//! # Disconnect rule (MVP)
//! If a seated peer disconnects mid-hand, the host stops the table: the in-flight hand is
//! abandoned (no winner is computed for it) and `run_host` returns the hands completed so far.
//! A production client would instead fold the absent seat / sit them out; that is future work.

use crate::bot::{OneShot, Strategy};
use crate::table::{HandOutcome, Step, Table, TableError, TableEvent};
use crate::{Action, TableMessage};
use libp2p::PeerId;
use poker_net::{NetError, Node, NodeEvent, NodeHandle};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

/// How often to retry a broadcast that returned [`NetError::NoSubscribersYet`].
const BROADCAST_RETRY: Duration = Duration::from_millis(150);
/// How often to re-broadcast the message(s) a peer is waiting to be acted on, to recover from
/// gossipsub's lossy, best-effort delivery. Retransmits are idempotent (see `play_one_hand`).
const RETRANSMIT: Duration = Duration::from_millis(400);
/// How long the host waits for the first guest before giving up.
const LOBBY_TIMEOUT: Duration = Duration::from_secs(120);

/// A decision source for the local seat: a deterministic bot (acts synchronously the moment it is
/// the local turn) or a human (the chosen [`Action`] arrives asynchronously on a channel, applied
/// via [`OneShot`] in the interactive drivers' `select!` arm). Boxing the bot keeps the strategy
/// generic out of the loop bodies so both drive sources share one code path.
enum Decider {
    Bot(Box<dyn Strategy>),
    Human(mpsc::Receiver<Action>),
}

impl Decider {
    /// The bot strategy, if this is a bot decider (the loop acts immediately on the local turn).
    /// `None` for a human, whose action instead arrives on the [`Decider::next_action`] arm.
    fn bot(&mut self) -> Option<&mut dyn Strategy> {
        match self {
            Decider::Bot(s) => Some(s.as_mut()),
            Decider::Human(_) => None,
        }
    }

    /// Await the next human action. For a bot this never resolves (the bot acts inline instead), so
    /// the `select!` arm that polls it is effectively disabled.
    async fn next_action(&mut self) -> Option<Action> {
        match self {
            Decider::Bot(_) => std::future::pending().await,
            Decider::Human(rx) => rx.recv().await,
        }
    }
}

/// Updates an interactive front-end (the egui app) receives as a game progresses. The bot drivers
/// pass a no-op observer. Deliberately UI-agnostic: the `protocol` crate has no GUI dependency.
pub enum DriverUpdate<'a> {
    /// The shareable `tcpoker://` table URI is ready (host only).
    Uri(String),
    /// The host's best dialable address looks non-routable; remote peers may not connect (host only).
    Reachability(String),
    /// Live replicated table state changed — re-render from it.
    State(&'a Table),
    /// A hand just completed: the public outcome (board, deltas, final stacks) plus this peer's own
    /// hole cards for that hand.
    HandResult {
        outcome: HandOutcome,
        local_hole: Option<[poker_game::Card; 2]>,
    },
    /// The game has ended (the host finished its hands, or the table aborted).
    Ended,
}

/// Errors from the async driver.
#[derive(Debug, Error)]
pub enum DriverError {
    #[error("net error: {0}")]
    Net(#[from] NetError),
    #[error("table error: {0}")]
    Table(#[from] TableError),
    #[error("message decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("the node event stream closed unexpectedly")]
    NodeClosed,
    #[error("timed out waiting for peers to join the lobby")]
    LobbyTimeout,
    #[error("a seated peer disconnected mid-hand; table aborted")]
    PeerLeftMidHand,
    #[error("could not obtain a table URI from the host node")]
    NoUri,
}

/// Per-hand summary for reporting / tests.
#[derive(Clone, Debug)]
pub struct HandReport {
    pub outcome: HandOutcome,
    /// This peer's OWN two hole cards for the hand, if it was seated and they were decrypted
    /// (always known for a placeholder hand; for a trustless hand, decrypted LOCALLY once the
    /// hole reveal completes — no other peer can derive them). `None` if this peer was not dealt
    /// in. Surfaced so a CLI/UI can show the player its private hand.
    pub local_hole: Option<[poker_game::Card; 2]>,
}

/// Summary of a completed game (a run of hands).
#[derive(Clone, Debug)]
pub struct GameReport {
    /// The table URI (host only; `None` for guests).
    pub uri: Option<String>,
    /// Seat order (PeerIds) used for the game.
    pub seats: Vec<PeerId>,
    /// One entry per completed hand, in order.
    pub hands: Vec<HandReport>,
}

/// Host configuration.
#[derive(Clone, Debug)]
pub struct HostOptions {
    /// How many hands to play before stopping (demo bound).
    pub hands: u64,
    /// Starting stack for every seat.
    pub starting_stack: u64,
    pub small_blind: u64,
    pub big_blind: u64,
    /// Minimum seated players (including the host) before the first hand starts.
    pub min_players: usize,
    /// Optional fixed identity keypair (None = generate).
    pub keypair: Option<libp2p::identity::Keypair>,
    /// Enable mDNS same-LAN discovery on the host node. `true` (default) for real play, so peers
    /// on the same Wi-Fi find the table without a URI. In-process integration tests set this
    /// `false`: they dial explicitly via the URI, so mDNS would only let unrelated test tables on
    /// loopback auto-discover and mesh into each other, perturbing mesh formation. See
    /// [`poker_net::NodeConfig`].
    pub enable_mdns: bool,
    /// Run the TRUSTLESS Barnett–Smart mental-poker deal (`true`, default) instead of the
    /// INSECURE public-seed placeholder (`false`). When `true` the host emits
    /// [`TableMessage::StartMentalHand`] with a fresh random 32-byte `session_seed` per hand and
    /// the driver runs the distributed keygen / shuffle / threshold-reveal protocol over the
    /// wire so no peer (the host included) can see another peer's hole cards. The placeholder
    /// path is retained only for fast/legacy demos and tests.
    pub mental: bool,
    /// Fixed listen port for the host node, or `None` for an OS-assigned ephemeral port. Set this to
    /// a port forwarded on the router for remote play behind NAT. See [`poker_net::NodeConfig`].
    pub listen_port: Option<u16>,
}

impl Default for HostOptions {
    fn default() -> Self {
        HostOptions {
            hands: 3,
            starting_stack: 1000,
            small_blind: 5,
            big_blind: 10,
            min_players: 2,
            keypair: None,
            // Real LAN play: mDNS on so same-network peers can discover the table.
            enable_mdns: true,
            // Trustless by default: real networked play must hide cards.
            mental: true,
            // Ephemeral port by default; set for a NAT'd host that has forwarded a fixed port.
            listen_port: None,
        }
    }
}

/// Generate a fresh random 32-byte session seed for a trustless hand. The seed only fixes the
/// PUBLIC scheme parameters / card encoding (identical on every peer); it reveals NO card —
/// hiding comes from each peer's secret key + shuffle randomness. A fresh seed per hand keeps
/// the deterministic parameters from being correlated across hands.
fn fresh_session_seed() -> Vec<u8> {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    seed.to_vec()
}

/// Derive a per-hand deck seed deterministically from the hand number.
///
/// SECURITY: this is an INSECURE PLACEHOLDER. A public, predictable seed means every peer
/// (and any observer) can reconstruct the whole deck, so there is no card hiding at all. It
/// exists only so demo runs are reproducible while the layers are wired together. The M2
/// trustless Barnett–Smart deal replaces seed-based dealing behind the `StartHand` seam.
pub fn demo_seed(hand_no: u64) -> u64 {
    // Spread the bits a little so consecutive hands look different to the placeholder shuffle.
    hand_no
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03)
}

/// Key identifying one deal payload slot (seat, message-kind, reveal-round), matching
/// [`deal_relay_key`]. A peer queues at most one outstanding payload per slot.
type DealKey = (usize, u8, u8);

/// One deal payload awaiting reliable delivery to a set of peers.
struct DealOut {
    /// The ORIGINAL author, carried so the receiver attributes the payload correctly (the host
    /// relays a guest's payload, so the transport sender is not the author).
    author: PeerId,
    /// Serialized [`TableMessage`] deal payload.
    bytes: Vec<u8>,
    /// Peers that have NOT yet acknowledged receipt.
    targets: BTreeSet<PeerId>,
}

/// Reliable, retrying delivery of trustless-deal payloads over libp2p request-response. Sends run
/// as detached tasks (so a slow or dead peer never blocks the driver loop) and report their result
/// on the channel [`DealSender::new`] returns; [`DealSender::ack`] applies a result and
/// [`DealSender::pump`] (re)spawns a send for every (payload, target) pair not currently in flight.
///
/// This replaces gossipsub's lossy, best-effort delivery for the deal rounds (key / shuffle /
/// reveal), which require reliable every-peer delivery. Transport loss is handled here (retry until
/// acked); the RECEIVER handles deal-phase ordering by deferring a payload that arrives before it
/// can be applied (see [`apply_deal_payload`] / [`replay_deferred`]).
struct DealSender {
    pending: BTreeMap<DealKey, DealOut>,
    in_flight: BTreeSet<(DealKey, PeerId)>,
    tx: mpsc::Sender<(DealKey, PeerId, bool)>,
}

impl DealSender {
    fn new() -> (Self, mpsc::Receiver<(DealKey, PeerId, bool)>) {
        let (tx, rx) = mpsc::channel(256);
        (
            DealSender {
                pending: BTreeMap::new(),
                in_flight: BTreeSet::new(),
                tx,
            },
            rx,
        )
    }

    /// Queue `msg` (a deal payload authored by `author`) for reliable delivery to each of `targets`.
    /// No-op for a non-deal message or an empty target set. Idempotent per slot: a payload already
    /// queued for a slot is kept (deal payloads are immutable per slot).
    fn enqueue(
        &mut self,
        msg: &TableMessage,
        author: PeerId,
        targets: impl IntoIterator<Item = PeerId>,
    ) {
        let key = match deal_relay_key(msg) {
            Some(k) => k,
            None => return,
        };
        let targets: BTreeSet<PeerId> = targets.into_iter().collect();
        if targets.is_empty() {
            return;
        }
        let bytes = match msg.encode() {
            Ok(b) => b,
            Err(_) => return,
        };
        self.pending.entry(key).or_insert(DealOut {
            author,
            bytes,
            targets,
        });
    }

    /// Spawn a reliable send for every (payload, target) pair not already in flight. Cheap to call
    /// repeatedly (after each step and on the retransmit ticker): already-acked pairs are gone and
    /// in-flight pairs are skipped.
    fn pump(&mut self, handle: &NodeHandle) {
        let mut to_send: Vec<(DealKey, PeerId, PeerId, Vec<u8>)> = Vec::new();
        for (key, out) in &self.pending {
            for target in &out.targets {
                if !self.in_flight.contains(&(*key, *target)) {
                    to_send.push((*key, *target, out.author, out.bytes.clone()));
                }
            }
        }
        for (key, target, author, bytes) in to_send {
            self.in_flight.insert((key, target));
            let handle = handle.clone();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let ok = handle.send_deal(target, author, bytes).await.is_ok();
                let _ = tx.send((key, target, ok)).await;
            });
        }
    }

    /// Apply a send result: clear its in-flight mark and, on success, drop that target (and the
    /// whole payload once every target has acked). A failure leaves the pair pending for the next
    /// [`pump`](Self::pump).
    fn ack(&mut self, key: DealKey, target: PeerId, ok: bool) {
        self.in_flight.remove(&(key, target));
        if ok {
            if let Some(out) = self.pending.get_mut(&key) {
                out.targets.remove(&target);
                if out.targets.is_empty() {
                    self.pending.remove(&key);
                }
            }
        }
    }

    /// Drop all queued payloads (e.g. at a new hand). In-flight tasks may still complete; their late
    /// results are ignored harmlessly.
    fn clear(&mut self) {
        self.pending.clear();
        self.in_flight.clear();
    }
}

/// Broadcast the NON-deal messages of a step over gossipsub, and queue its deal payloads (authored
/// by the local peer `me`) for RELIABLE delivery to `targets` (the host's guests, or a guest's
/// host). Replaces the old all-gossipsub broadcast for any step that can carry a deal payload.
async fn send_step(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    step: Step,
) -> Result<(), DriverError> {
    for msg in step.broadcasts {
        if is_deal_payload(&msg) {
            sender.enqueue(&msg, me, targets.iter().copied());
        } else {
            broadcast_retry(handle, &msg).await?;
        }
    }
    Ok(())
}

/// A deal error worth DEFERRING for replay: the payload arrived before this peer's deal reached the
/// phase that can apply it (a reveal before `Ready`, a shuffle before its turn, or any deal payload
/// before the mental hand started locally). It WILL become applicable as this peer's deal advances,
/// so we stash it and retry. This is the core of the reliability fix: reliable transport delivers
/// each payload once, and deferral ensures an early arrival is applied later instead of dropped.
/// Permanent rejects (bad proof, wrong author, duplicate) are NOT deferred.
fn is_deferrable_deal(e: &TableError) -> bool {
    matches!(e, TableError::DealOutOfTurn | TableError::NoMentalHand)
}

/// Apply one inbound deal payload to the table. On success returns the resulting [`Step`] and, when
/// `relay_targets` is `Some`, queues the payload for reliable relay to those peers (the host fanning
/// a guest's payload out to the other guests). A payload that cannot be applied YET is pushed onto
/// `deferred` for replay; a permanently-rejected one is dropped.
fn apply_deal_payload(
    sender: &mut DealSender,
    table: &mut Table,
    deferred: &mut Vec<(PeerId, TableMessage)>,
    relay_targets: Option<Vec<PeerId>>,
    author: PeerId,
    msg: TableMessage,
) -> Result<Option<Step>, DriverError> {
    match table.handle(msg.clone(), author) {
        Ok(step) => {
            if let Some(targets) = relay_targets {
                sender.enqueue(&msg, author, targets);
            }
            Ok(Some(step))
        }
        Err(e) if is_deferrable_deal(&e) => {
            deferred.push((author, msg));
            Ok(None)
        }
        Err(e) if is_ignorable(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Re-attempt every deferred deal payload now that this peer's deal may have advanced. Applied
/// payloads are processed (their own outbound payloads queued via `send_step`, relay queued when
/// `relay` is set), and any settle outcome captured; ones that still cannot apply are kept for the
/// next attempt. Loops until a pass makes no progress.
#[allow(clippy::too_many_arguments)]
async fn replay_deferred(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    last_outcome: &mut Option<HandOutcome>,
    deferred: &mut Vec<(PeerId, TableMessage)>,
    relay: bool,
) -> Result<(), DriverError> {
    loop {
        if deferred.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(deferred);
        let before = batch.len();
        for (author, msg) in batch {
            let relay_targets = if relay {
                Some(targets.iter().copied().filter(|t| *t != author).collect())
            } else {
                None
            };
            if let Some(step) =
                apply_deal_payload(sender, table, deferred, relay_targets, author, msg)?
            {
                capture_outcome(&step, last_outcome);
                send_step(handle, sender, me, targets, step).await?;
            }
        }
        // `deferred` now holds the payloads that still could not apply (re-pushed above). If none
        // applied this pass, applying again won't help until more state arrives.
        if deferred.len() >= before {
            return Ok(());
        }
    }
}

/// Run as the HOST: create the table, wait for `min_players`, play `opts.hands` hands with
/// `strategy` driving the host's own seat, then shut down. Returns a [`GameReport`].
///
/// On success the URI is printed via the supplied `on_uri` callback as soon as it is known
/// (so a CLI can display it). Pass a no-op closure to ignore.
pub async fn run_host<S, F>(strategy: S, opts: HostOptions, on_uri: F) -> Result<GameReport, DriverError>
where
    S: Strategy + 'static,
    F: FnMut(&str),
{
    run_host_generic(Decider::Bot(Box::new(strategy)), opts, on_uri, &mut |_| {}).await
}

/// Run as the HOST with a HUMAN at seat 0: the seat's actions arrive on `action_rx` (the GUI sends
/// them when the player clicks) and live state changes are pushed to `observer`. Otherwise
/// identical to [`run_host`].
pub async fn run_host_interactive<F>(
    action_rx: mpsc::Receiver<Action>,
    opts: HostOptions,
    on_uri: F,
    observer: &mut (dyn FnMut(DriverUpdate) + Send),
) -> Result<GameReport, DriverError>
where
    F: FnMut(&str),
{
    run_host_generic(Decider::Human(action_rx), opts, on_uri, observer).await
}

async fn run_host_generic<F>(
    mut decider: Decider,
    opts: HostOptions,
    mut on_uri: F,
    observer: &mut (dyn FnMut(DriverUpdate) + Send),
) -> Result<GameReport, DriverError>
where
    F: FnMut(&str),
{
    let node = poker_net::host_with_config(
        opts.keypair.clone(),
        poker_net::NodeConfig {
            enable_mdns: opts.enable_mdns,
            listen_port: opts.listen_port,
        },
    )?;
    let Node { handle, mut events } = node;
    let me = handle.local_peer_id();

    // Lobby: collect peers (host is seat 0). We seat ONLY on an authenticated `JoinTable`
    // (MEDIUM-1 / CRITICAL-1 hardening), never on a raw transport `PeerConnected`:
    //  * Seating on a `JoinTable` proves the peer's gossipsub publish path to us works (its
    //    message actually arrived), so when we then broadcast `StartHand` the peer is far more
    //    likely to be meshed and receive it — we are not relying solely on the retransmit ticker
    //    to paper over a peer that connected at the transport level but never meshed.
    //  * A foreign node (e.g. one auto-discovered via mDNS from an unrelated table on the same
    //    host/LAN) that merely connects but never sends a `JoinTable` for THIS table is not
    //    seated, so unrelated tables cannot pollute each other's roster.
    // Seating in `JoinTable` arrival order gives the documented "guests seated in join order".
    let mut roster: Vec<PeerId> = vec![me];
    let mut display: BTreeMap<PeerId, String> = BTreeMap::new();
    display.insert(me, "host".into());
    let mut uri: Option<String> = None;

    // Wait until we have min_players seated (host + guests that have announced via JoinTable).
    let deadline = tokio::time::Instant::now() + LOBBY_TIMEOUT;
    while roster.len() < opts.min_players {
        let ev = tokio::time::timeout_at(deadline, events.recv())
            .await
            .map_err(|_| DriverError::LobbyTimeout)?
            .ok_or(DriverError::NodeClosed)?;
        match ev {
            NodeEvent::TableUriReady(u) => {
                on_uri(&u);
                observer(DriverUpdate::Uri(u.clone()));
                uri = Some(u);
            }
            NodeEvent::ReachabilityWarning(r) => {
                eprintln!("reachability warning: {r:?} — remote peers may not be able to dial in");
                observer(DriverUpdate::Reachability(format!(
                    "{r:?} — remote peers may not be able to dial in"
                )));
            }
            NodeEvent::PeerDisconnected(p) => {
                roster.retain(|x| *x != p);
            }
            NodeEvent::Message { from, data } => {
                if let Ok(TableMessage::JoinTable { display_name }) = TableMessage::decode(&data) {
                    display.insert(from, display_name);
                    if !roster.contains(&from) {
                        roster.push(from);
                    }
                }
            }
            _ => {}
        }
    }

    // Make sure we have a URI to return (best-effort).
    if uri.is_none() {
        uri = handle.current_table_uri().await.ok().flatten();
    }

    // Play the configured number of hands.
    let mut table = Table::new(me, me);
    let mut stacks = vec![opts.starting_stack; roster.len()];
    let mut button = 0usize;
    let mut report = GameReport {
        uri: uri.clone(),
        seats: roster.clone(),
        hands: Vec::new(),
    };

    for hand_no in 1..=opts.hands {
        // Drop any seats that have left (host stays). MVP: require all original seats present.
        if roster.len() < opts.min_players {
            break;
        }
        let seats: Vec<Vec<u8>> = roster.iter().map(|p| p.to_bytes()).collect();
        let start = if opts.mental {
            // TRUSTLESS: a fresh per-hand session seed fixes only the public scheme parameters.
            TableMessage::StartMentalHand {
                hand_no,
                button,
                session_seed: fresh_session_seed(),
                seats,
                stacks: stacks.clone(),
                small_blind: opts.small_blind,
                big_blind: opts.big_blind,
            }
        } else {
            // INSECURE placeholder path (retained for fast demos / tests).
            TableMessage::StartHand {
                hand_no,
                button,
                seed: demo_seed(hand_no),
                seats,
                stacks: stacks.clone(),
                small_blind: opts.small_blind,
                big_blind: opts.big_blind,
            }
        };
        // Host applies its own StartHand to get the start step (for a mental hand it also carries
        // the host's own KeyAnnounce). `play_one_hand` broadcasts the start over gossipsub and
        // routes the step's deal payloads over the reliable channel.
        let start_step = table.handle(start.clone(), me)?;

        // The set of guests whose HandComplete we must collect before dealing the next hand.
        let guests: Vec<PeerId> = roster.iter().copied().filter(|p| *p != me).collect();
        let (outcome, local_hole) = play_one_hand(
            &handle, &mut events, &mut table, &mut decider, hand_no, me, &guests, &start,
            start_step, observer,
        )
        .await?;

        // Advance stacks + button for the next hand.
        observer(DriverUpdate::HandResult {
            outcome: outcome.clone(),
            local_hole,
        });
        stacks = outcome.final_stacks.clone();
        button = (button + 1) % roster.len();
        report.hands.push(HandReport {
            outcome,
            local_hole,
        });
    }

    handle.shutdown().await;
    observer(DriverUpdate::Ended);
    Ok(report)
}

/// Run as a GUEST: join `uri`, wait to be seated by the host's first `StartHand`, then play
/// every hand the host deals with `strategy` driving this peer's seat. Returns a
/// [`GameReport`] once the host stops dealing (event stream closes).
pub async fn run_guest<S>(
    uri: &str,
    strategy: S,
    keypair: Option<libp2p::identity::Keypair>,
) -> Result<GameReport, DriverError>
where
    S: Strategy + 'static,
{
    // Real LAN play: mDNS on.
    run_guest_with_config(uri, strategy, keypair, true).await
}

/// Like [`run_guest`] but with explicit control over mDNS. Real play passes `enable_mdns = true`;
/// in-process integration tests pass `false` so concurrent loopback tables (which dial explicitly
/// via the URI) do not auto-discover and mesh into each other. See [`poker_net::NodeConfig`].
pub async fn run_guest_with_config<S>(
    uri: &str,
    strategy: S,
    keypair: Option<libp2p::identity::Keypair>,
    enable_mdns: bool,
) -> Result<GameReport, DriverError>
where
    S: Strategy + 'static,
{
    run_guest_generic(
        uri,
        Decider::Bot(Box::new(strategy)),
        keypair,
        enable_mdns,
        &mut |_| {},
    )
    .await
}

/// Run as a GUEST with a HUMAN at this seat: the seat's actions arrive on `action_rx` (the GUI sends
/// them) and live state changes are pushed to `observer`. Otherwise identical to [`run_guest`].
pub async fn run_guest_interactive(
    uri: &str,
    action_rx: mpsc::Receiver<Action>,
    keypair: Option<libp2p::identity::Keypair>,
    enable_mdns: bool,
    observer: &mut (dyn FnMut(DriverUpdate) + Send),
) -> Result<GameReport, DriverError> {
    run_guest_generic(uri, Decider::Human(action_rx), keypair, enable_mdns, observer).await
}

async fn run_guest_generic(
    uri: &str,
    mut decider: Decider,
    keypair: Option<libp2p::identity::Keypair>,
    enable_mdns: bool,
    observer: &mut (dyn FnMut(DriverUpdate) + Send),
) -> Result<GameReport, DriverError> {
    // HIGH-1 anti-cheat: the legitimate host is the peer the URI told us to dial — its PeerId is
    // carried in the `/p2p/<PeerId>` suffix of the URI's multiaddr(s). We pin our host identity to
    // that peer and reject a `StartHand` from anyone else, so a foreign node on the mesh (e.g. an
    // mDNS-discovered node from an unrelated table) cannot impersonate our host by racing in the
    // first StartHand. If the URI somehow lacks a PeerId we fall back to trust-on-first-StartHand.
    let expected_host = host_peer_id_from_uri(uri);

    let node = poker_net::join_with_config(
        uri,
        keypair,
        poker_net::NodeConfig {
            enable_mdns,
            // Guests only dial outbound, so their listen port is irrelevant — leave it ephemeral.
            listen_port: None,
        },
    )?;
    let Node { handle, mut events } = node;
    let me = handle.local_peer_id();

    // Announce ourselves to the lobby once the mesh forms.
    let join_msg = TableMessage::JoinTable {
        display_name: format!("guest-{}", me),
    };
    // Retry the announcement until the host's mesh accepts it.
    broadcast_retry(&handle, &join_msg).await?;

    // We learn the host PeerId from the URI (preferred) or the first StartHand author.
    let mut table: Option<Table> = None;
    let mut host_peer: Option<PeerId> = None;
    // Peers this guest's OWN deal payloads go to: just the host, who fans them out. Empty until
    // seated (no deal payloads are produced before then).
    let mut host_targets: Vec<PeerId> = Vec::new();
    // Per-hand outbound state we retransmit on the ticker to survive gossipsub's lossy delivery:
    // our most recent own Act and our completion ack. Both are idempotent on the receiving peers.
    // We keep resending our last Act for the CURRENT hand even after our own table has settled,
    // because we may have authored the hand's CLOSING action and no one else resends it; we drop
    // it once a NEW hand's StartHand arrives. (Deal payloads now use the reliable channel below.)
    let mut last_act: Option<TableMessage> = None;
    let mut last_ack: Option<TableMessage> = None;
    // Reliable delivery of this guest's OWN deal contributions to the host (who fans them out).
    let (mut sender, mut deal_rx) = DealSender::new();
    // Inbound deal payloads (relayed by the host) received before this guest could apply them —
    // including any that arrive before the StartHand seats us — replayed as the deal advances.
    let mut deferred: Vec<(PeerId, TableMessage)> = Vec::new();
    // This guest's OWN decrypted hole cards for the current hand, snapshotted before settle
    // clears the live hand. Cleared on each new hand. Attached to the HandReport on settle.
    let mut current_hole: Option<[poker_game::Card; 2]> = None;
    let mut current_hand: Option<u64> = None;
    let mut seated = false;
    let mut report = GameReport {
        uri: None,
        seats: Vec::new(),
        hands: Vec::new(),
    };
    // How many completed hands we've already pushed to `observer` (so each fires once).
    let mut emitted_hands = 0usize;

    let mut ticker = tokio::time::interval(RETRANSMIT);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // Snapshot our own hole as soon as it is decrypted, before settle clears the live hand.
        if current_hole.is_none() {
            if let Some(t) = table.as_ref() {
                if let Some(h) = t.local_hole() {
                    current_hole = Some(h);
                }
            }
        }
        // Surface any newly-completed hand(s) to the observer (each exactly once).
        while emitted_hands < report.hands.len() {
            let h = &report.hands[emitted_hands];
            observer(DriverUpdate::HandResult {
                outcome: h.outcome.clone(),
                local_hole: h.local_hole,
            });
            emitted_hands += 1;
        }
        tokio::select! {
            _ = ticker.tick() => {
                if !seated {
                    // Keep announcing until the host seats us (first StartHand arrives).
                    broadcast_retry(&handle, &join_msg).await?;
                } else {
                    if std::env::var("POKER_TRACE").is_ok() {
                        if let Some(t) = table.as_ref() {
                            eprintln!(
                                "[trace guest seat={:?} hand={:?}] phase={:?} hole={} board={} \
                                 to_act={:?} last_act={} last_ack={} report_hands={} deferred={}\n    missing={:?}",
                                t.local_seat(), t.live_hand_no(), t.deal_phase(),
                                t.local_hole().is_some(), t.community().len(),
                                t.seat_to_act(),
                                last_act.is_some(), last_ack.is_some(), report.hands.len(), deferred.len(),
                                t.debug_deal_missing(),
                            );
                        } else {
                            eprintln!("[trace guest NO-TABLE seated={seated} last_ack={}]", last_ack.is_some());
                        }
                    }
                    // Re-broadcast our last Act / completion ack over gossipsub (idempotent).
                    if let Some(a) = &last_act { broadcast_retry(&handle, a).await?; }
                    if let Some(a) = &last_ack { broadcast_retry(&handle, a).await?; }
                    if let Some(t) = table.as_mut() {
                        // Replay any deferred inbound deal payloads, pump our own owed deal
                        // contributions (a ticker-path settle can decrypt the final showdown hole
                        // and produce our HandComplete ack — capture it so it keeps retransmitting),
                        // and re-spawn any not-yet-acked deal send to the host.
                        guest_replay_deferred(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack, &mut deferred).await?;
                        pump_local_deal_guest(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack).await?;
                        sender.pump(&handle);
                    }
                }
            }
            Some((key, target, ok)) = deal_rx.recv() => {
                sender.ack(key, target, ok);
            }
            maybe_action = decider.next_action() => {
                // HUMAN guest's action arrived from the GUI (never fires for a bot). Apply it via a
                // one-shot strategy through the same path the bot uses, if it is still our turn.
                if let Some(action) = maybe_action {
                    if let Some(t) = table.as_mut() {
                        if t.is_local_turn() {
                            let mut once = OneShot::new(action);
                            if let Some(a) = act_if_local_turn(&handle, &mut sender, me, &host_targets, t, &mut once, &mut report, &mut last_ack).await? {
                                last_act = Some(a);
                            }
                            pump_local_deal_guest(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack).await?;
                            sender.pump(&handle);
                            observer(DriverUpdate::State(t));
                        }
                    }
                }
            }
            ev = events.recv() => {
                let ev = match ev {
                    Some(e) => e,
                    None => break, // node stopped: host finished or we shut down.
                };
                match ev {
                    NodeEvent::DealReceived { author, data, .. } => {
                        // A deal payload relayed by the host. It may arrive before the StartHand
                        // that seats us; defer it until we have a table, then it is replayed.
                        let msg = match TableMessage::decode(&data) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        match table.as_mut() {
                            None => deferred.push((author, msg)),
                            Some(t) => {
                                if let Some(step) =
                                    apply_deal_payload(&mut sender, t, &mut deferred, None, author, msg)?
                                {
                                    // Re-snapshot the hole AFTER applying: ingesting this token may
                                    // have decrypted it, and a settle in this step needs it.
                                    if current_hole.is_none() {
                                        if let Some(h) = t.local_hole() {
                                            current_hole = Some(h);
                                        }
                                    }
                                    if let Some(ack) = finish_step(&handle, &mut sender, me, &host_targets, &mut report, step, current_hole).await? {
                                        last_ack = Some(ack);
                                    }
                                }
                                guest_replay_deferred(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack, &mut deferred).await?;
                                pump_local_deal_guest(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack).await?;
                                if let Some(a) = guest_bot_act(&handle, &mut sender, me, &host_targets, t, &mut decider, &mut report, &mut last_ack).await? {
                                    last_act = Some(a);
                                }
                                observer(DriverUpdate::State(t));
                                sender.pump(&handle);
                            }
                        }
                    }
                    NodeEvent::Message { from, data } => {
                        // gossipsub carries only non-deal messages now: Start*, Act, HandComplete,
                        // JoinTable. Deal payloads arrive via `DealReceived`.
                        let msg = match TableMessage::decode(&data) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        // Acks are host-only coordination; a guest ignores inbound ones.
                        if let TableMessage::HandComplete { .. } = msg {
                            continue;
                        }
                        // Anti-cheat (HIGH-1): a StartHand / StartMentalHand is only legitimate
                        // from the host the URI told us to dial. Ignore a start from any other
                        // peer so a foreign node cannot hijack our table or latch us to a wrong host.
                        let is_start = start_hand_no(&msg).is_some();
                        if is_start {
                            if let Some(expected) = expected_host {
                                if from != expected {
                                    continue;
                                }
                            }
                        }
                        // A new start opens a new hand: drop the previous hand's stale Act, clear
                        // outbound deal sends, and drop deferred deal payloads from earlier hands.
                        if let Some(hn) = start_hand_no(&msg) {
                            if current_hand != Some(hn) {
                                current_hand = Some(hn);
                                last_act = None;
                                current_hole = None;
                                sender.clear();
                                deferred.retain(|(_, m)| deal_hand_no(m).is_none_or(|h| h >= hn));
                            }
                        }
                        // The first start we accept defines the host identity and seats us.
                        if table.is_none() {
                            if is_start {
                                host_peer = Some(from);
                                host_targets = vec![from];
                                seated = true;
                                let mut t = Table::new(me, from);
                                let step = t.handle(msg.clone(), from)?;
                                report.seats = t.roster().to_vec();
                                last_ack = finish_step(&handle, &mut sender, me, &host_targets, &mut report, step, current_hole).await?.or(last_ack);
                                // Replay deal payloads that arrived before we were seated, pump our
                                // owed contributions, and act if it is already our turn.
                                guest_replay_deferred(&handle, &mut sender, me, &host_targets, &mut t, &mut report, &mut current_hole, &mut last_ack, &mut deferred).await?;
                                pump_local_deal_guest(&handle, &mut sender, me, &host_targets, &mut t, &mut report, &mut current_hole, &mut last_ack).await?;
                                last_act = guest_bot_act(&handle, &mut sender, me, &host_targets, &mut t, &mut decider, &mut report, &mut last_ack).await?.or(last_act);
                                sender.pump(&handle);
                                observer(DriverUpdate::State(&t));
                                table = Some(t);
                            }
                            continue;
                        }
                        let t = table.as_mut().unwrap();
                        match t.handle(msg, from) {
                            Ok(step) => {
                                // Snapshot our hole before settle (HandEnded) clears the live hand.
                                if current_hole.is_none() {
                                    if let Some(h) = t.local_hole() {
                                        current_hole = Some(h);
                                    }
                                }
                                // We deliberately do NOT clear `last_act` on settle: we may have
                                // authored the closing Act and must keep re-sending it until the
                                // next start, so a peer that missed it can still settle.
                                if let Some(ack) = finish_step(&handle, &mut sender, me, &host_targets, &mut report, step, current_hole).await? {
                                    last_ack = Some(ack);
                                }
                                // Applying this message may have advanced the deal: replay deferred
                                // payloads, pump owed contributions (which can settle the hand), and
                                // act if it is now our turn.
                                guest_replay_deferred(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack, &mut deferred).await?;
                                pump_local_deal_guest(&handle, &mut sender, me, &host_targets, t, &mut report, &mut current_hole, &mut last_ack).await?;
                                if let Some(a) = guest_bot_act(&handle, &mut sender, me, &host_targets, t, &mut decider, &mut report, &mut last_ack).await? {
                                    last_act = Some(a);
                                }
                                observer(DriverUpdate::State(t));
                                sender.pump(&handle);
                            }
                            Err(e) if is_ignorable(&e) => { /* reject / stale / replay: ignore */ }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    NodeEvent::PeerDisconnected(p) => {
                        // MVP exit + disconnect rule: when the host disconnects (it finished its
                        // hand run and shut down, or vanished mid-hand), the guest stops. Any
                        // in-flight hand is simply abandoned (no outcome recorded for it).
                        if host_peer == Some(p) {
                            break;
                        }
                    }
                    NodeEvent::DialFailed { peer, addr, error } => {
                        // ADVISORY, not fatal. A URI carries several addresses (e.g. QUIC AND TCP)
                        // and libp2p surfaces one `DialFailed` per address that fails — including
                        // transient handshake races — even when ANOTHER address (or a retry) will
                        // succeed and the mesh forms moments later. Aborting on the first failure
                        // killed otherwise-healthy joins (the two-terminal demo hit exactly this:
                        // a transient TCP handshake error aborted the guest while QUIC was fine).
                        // We log and keep waiting; the JoinTable retransmit on the ticker keeps
                        // trying to mesh. A genuinely unreachable host simply never seats us — the
                        // caller (CLI/test) bounds that with its own timeout.
                        eprintln!(
                            "dial attempt failed (peer={peer:?}, addr={addr:?}): {error} — \
                             retrying other addresses"
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    handle.shutdown().await;
    observer(DriverUpdate::Ended);
    Ok(report)
}

/// Extract the host's [`PeerId`] from a `tcpoker://` URI by decoding its multiaddr(s) and
/// reading the trailing `/p2p/<PeerId>`. Returns `None` if the URI cannot be decoded or carries
/// no peer id (in which case the guest falls back to trust-on-first-StartHand).
fn host_peer_id_from_uri(uri: &str) -> Option<PeerId> {
    let addrs = poker_net::decode_table_uri(uri).ok()?;
    addrs.iter().find_map(|a| {
        a.iter().find_map(|p| match p {
            libp2p::multiaddr::Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
    })
}

/// Drive one full hand to completion on an already-started table (host side). Pumps the event
/// stream, applies inbound `Act`s and reliably-delivered deal payloads (relaying each verified deal
/// payload to the other guests), broadcasts the host's own action when it is its turn until the
/// live hand ends, then waits for a `HandComplete` ack from every guest in `guests` before
/// returning. The ack barrier makes the table self-pacing: the host never deals hand N+1 until
/// every guest has settled hand N, so a fast host can never reset a hand a guest has not finished.
#[allow(clippy::too_many_arguments)]
async fn play_one_hand(
    handle: &NodeHandle,
    events: &mut mpsc::Receiver<NodeEvent>,
    table: &mut Table,
    decider: &mut Decider,
    hand_no: u64,
    me: PeerId,
    guests: &[PeerId],
    start: &TableMessage,
    start_step: Step,
    observer: &mut (dyn FnMut(DriverUpdate) + Send),
) -> Result<(HandOutcome, Option<[poker_game::Card; 2]>), DriverError> {
    let mut last_outcome: Option<HandOutcome> = None;
    // Snapshot this host's OWN hole cards while the hand is live (the live hand — and thus the
    // hole — is cleared on settle, so we capture it before then). For a trustless hand this is
    // decrypted locally once the hole reveal completes; no other peer can derive it.
    let mut local_hole: Option<[poker_game::Card; 2]> = None;
    let mut acked: std::collections::HashSet<PeerId> = std::collections::HashSet::new();
    // The most recent Act that advanced the HOST'S table, regardless of who authored it. The host
    // is the relay hub: clients A and B are not directly connected, so B's Act reaches A only via
    // the host. A peer stops resending its OWN Act once its table settles, so the closing Act of
    // the hand is no longer retransmitted by anyone once its author settles. By re-broadcasting the
    // last Act it applied (any author) until every guest has acked, the host reliably re-relays the
    // closing action to any peer that missed it. Replayed Acts are rejected harmlessly everywhere.
    let mut last_applied_act: Option<TableMessage> = None;
    // Inbound deal payloads received before this peer's deal could apply them, replayed as the deal
    // advances. This is the reliability fix: the deal channel delivers each payload reliably ONCE,
    // and deferral ensures an early arrival is applied later instead of dropped (see `replay_deferred`).
    let mut deferred: Vec<(PeerId, TableMessage)> = Vec::new();
    // Reliable deal delivery: the host fans every verified deal payload out to all its guests.
    let (mut sender, mut deal_rx) = DealSender::new();

    // Broadcast the StartMentalHand/StartHand itself over gossipsub, and route the start step's deal
    // payloads (for a mental hand, the host's own KeyAnnounce) reliably to the guests.
    broadcast_retry(handle, start).await?;
    capture_outcome(&start_step, &mut last_outcome);
    send_step(handle, &mut sender, me, guests, start_step).await?;

    // If the host itself is first to act, a BOT acts immediately; a human waits (its action arrives
    // on the `next_action` arm). Pump any owed deal contribution first (e.g. the host's shuffle turn
    // if it is seat 0).
    pump_local_deal(handle, &mut sender, me, guests, table, &mut last_outcome).await?;
    host_bot_act(handle, &mut sender, me, guests, table, decider, &mut last_applied_act, &mut last_outcome).await?;
    sender.pump(handle);
    observer(DriverUpdate::State(table));

    // Retransmit ticker: gossipsub is lossy, so we periodically re-broadcast what we are
    // waiting to be acted on. StartHand is idempotent (see `Table::apply_start_hand`) and a
    // replayed Act is rejected harmlessly, so retransmits never corrupt the replicated state.
    let mut ticker = tokio::time::interval(RETRANSMIT);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // Snapshot our own hole as soon as it is decrypted, before settle clears the live hand.
        if local_hole.is_none() {
            if let Some(h) = table.local_hole() {
                local_hole = Some(h);
            }
        }

        // Done only once the host's table has settled AND every guest has acked this hand.
        if last_outcome.is_some() && guests.iter().all(|g| acked.contains(g)) {
            let o = last_outcome.ok_or(DriverError::NodeClosed)?;
            return Ok((o, local_hole));
        }

        tokio::select! {
            _ = ticker.tick() => {
                if std::env::var("POKER_TRACE").is_ok() {
                    eprintln!(
                        "[trace HOST hand={hand_no}] phase={:?} board={} to_act={:?} \
                         have_outcome={} acked={}/{}",
                        table.deal_phase(), table.community().len(),
                        table.seat_to_act(), last_outcome.is_some(), acked.len(), guests.len(),
                    );
                }
                // Re-announce so a guest that missed StartHand catches up, and re-relay the last
                // Act applied to our table (any author) so a peer that missed the closing action
                // — which its author no longer resends after settling — still settles and acks.
                broadcast_retry(handle, start).await?;
                if let Some(a) = &last_applied_act {
                    broadcast_retry(handle, a).await?;
                }
                // Pump any deal contribution this peer still owes, replay any deferred inbound deal
                // payloads now that the deal may have advanced, and re-spawn any deal send that has
                // not yet been acked (transport-level retry).
                pump_local_deal(handle, &mut sender, me, guests, table, &mut last_outcome).await?;
                replay_deferred(handle, &mut sender, me, guests, table, &mut last_outcome, &mut deferred, true).await?;
                sender.pump(handle);
            }
            Some((key, target, ok)) = deal_rx.recv() => {
                sender.ack(key, target, ok);
            }
            maybe_action = decider.next_action() => {
                // HUMAN host's action arrived from the GUI (never fires for a bot). Apply it via a
                // one-shot strategy through the same path the bot uses, if it is still our turn.
                if let Some(action) = maybe_action {
                    if table.is_local_turn() {
                        let mut once = OneShot::new(action);
                        let step = table.local_turn(&mut once)?;
                        remember_last_act(&step, &mut last_applied_act);
                        capture_outcome(&step, &mut last_outcome);
                        send_step(handle, &mut sender, me, guests, step).await?;
                        pump_local_deal(handle, &mut sender, me, guests, table, &mut last_outcome).await?;
                        sender.pump(handle);
                        observer(DriverUpdate::State(table));
                    }
                }
            }
            ev = events.recv() => {
                let ev = ev.ok_or(DriverError::NodeClosed)?;
                match ev {
                    NodeEvent::DealReceived { author, data, .. } => {
                        // A reliably-delivered deal payload from a guest. Apply + verify it, then
                        // RELAY it to every OTHER guest (the author already has it; guests are not
                        // directly connected). Only a payload we successfully verified is relayed,
                        // so a cheater's bad proof is never amplified.
                        let msg = match TableMessage::decode(&data) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        let relay: Vec<PeerId> =
                            guests.iter().copied().filter(|g| *g != author).collect();
                        if let Some(step) = apply_deal_payload(
                            &mut sender, table, &mut deferred, Some(relay), author, msg,
                        )? {
                            capture_outcome(&step, &mut last_outcome);
                            send_step(handle, &mut sender, me, guests, step).await?;
                        }
                        // Applying may have advanced the deal and unblocked deferred payloads, owed
                        // local contributions, and/or our betting turn (a bot acts; a human waits).
                        replay_deferred(handle, &mut sender, me, guests, table, &mut last_outcome, &mut deferred, true).await?;
                        pump_local_deal(handle, &mut sender, me, guests, table, &mut last_outcome).await?;
                        host_bot_act(handle, &mut sender, me, guests, table, decider, &mut last_applied_act, &mut last_outcome).await?;
                        sender.pump(handle);
                        observer(DriverUpdate::State(table));
                    }
                    NodeEvent::Message { from, data } => {
                        // gossipsub carries only non-deal messages now: Act, Start*, HandComplete,
                        // JoinTable. Deal payloads arrive via `DealReceived`.
                        let msg = match TableMessage::decode(&data) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        if let TableMessage::HandComplete { hand_no: hn, .. } = &msg {
                            if *hn == hand_no {
                                acked.insert(from);
                            }
                            continue;
                        }
                        let is_act = matches!(msg, TableMessage::Act { .. });
                        match table.handle(msg.clone(), from) {
                            Ok(step) => {
                                // A guest's Act just advanced our table: remember it so the host
                                // re-relays it on the ticker to any peer that missed it.
                                if is_act {
                                    last_applied_act = Some(msg.clone());
                                }
                                capture_outcome(&step, &mut last_outcome);
                                send_step(handle, &mut sender, me, guests, step).await?;
                            }
                            Err(e) if is_ignorable(&e) => continue,
                            Err(e) => return Err(e.into()),
                        }
                        // Applying this Act may have opened a street (the host owes a community
                        // reveal), unblocked deferred payloads, and/or made it our betting turn
                        // (a bot acts immediately; a human waits for the GUI to send an action).
                        replay_deferred(handle, &mut sender, me, guests, table, &mut last_outcome, &mut deferred, true).await?;
                        pump_local_deal(handle, &mut sender, me, guests, table, &mut last_outcome).await?;
                        host_bot_act(handle, &mut sender, me, guests, table, decider, &mut last_applied_act, &mut last_outcome).await?;
                        sender.pump(handle);
                        observer(DriverUpdate::State(table));
                    }
                    NodeEvent::PeerDisconnected(p) => {
                        if guests.contains(&p) && !acked.contains(&p) {
                            return Err(DriverError::PeerLeftMidHand);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// HOST: if the local seat is driven by a BOT and it is its turn, ask the bot and send the Act.
/// For a human seat this is a no-op — the action arrives asynchronously on the `next_action` arm.
async fn host_bot_act(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    guests: &[PeerId],
    table: &mut Table,
    decider: &mut Decider,
    last_applied_act: &mut Option<TableMessage>,
    last_outcome: &mut Option<HandOutcome>,
) -> Result<(), DriverError> {
    if let Some(bot) = decider.bot() {
        let step = table.local_turn(bot)?;
        remember_last_act(&step, last_applied_act);
        capture_outcome(&step, last_outcome);
        send_step(handle, sender, me, guests, step).await?;
    }
    Ok(())
}

/// True for the trustless deal payload messages the HOST must RELAY between guests. Guests are
/// not directly connected: a deal contribution from guest A reaches guest B only because the
/// host re-broadcasts it. (`StartMentalHand` is the host's own message; `Act` is relayed by the
/// dedicated `last_applied_act` path; `HandComplete` is host-only coordination.)
fn is_deal_payload(msg: &TableMessage) -> bool {
    matches!(
        msg,
        TableMessage::KeyAnnounce { .. }
            | TableMessage::ShuffleAnnounce { .. }
            | TableMessage::RevealAnnounce { .. }
    )
}

/// The hand number a START message opens, for either the placeholder ([`TableMessage::StartHand`])
/// or the trustless ([`TableMessage::StartMentalHand`]) variant. `None` for any other message.
fn start_hand_no(msg: &TableMessage) -> Option<u64> {
    match msg {
        TableMessage::StartHand { hand_no, .. } | TableMessage::StartMentalHand { hand_no, .. } => {
            Some(*hand_no)
        }
        _ => None,
    }
}

/// The hand a deal payload belongs to, for dropping a deferred payload from an earlier hand.
/// `None` for any non-deal message.
fn deal_hand_no(msg: &TableMessage) -> Option<u64> {
    match msg {
        TableMessage::KeyAnnounce { hand_no, .. }
        | TableMessage::ShuffleAnnounce { hand_no, .. }
        | TableMessage::RevealAnnounce { hand_no, .. } => Some(*hand_no),
        _ => None,
    }
}

/// A stable key for a relayed deal payload so the host re-relays one copy per logical slot
/// (seat + message kind + reveal round), replacing a superseded payload rather than accumulating
/// duplicates. Returns `None` for non-deal messages.
fn deal_relay_key(msg: &TableMessage) -> Option<(usize, u8, u8)> {
    match msg {
        TableMessage::KeyAnnounce { seat, .. } => Some((*seat, 0, 0)),
        TableMessage::ShuffleAnnounce { turn, .. } => Some((*turn, 1, 0)),
        TableMessage::RevealAnnounce { seat, round, .. } => Some((*seat, 2, *round as u8)),
        _ => None,
    }
}

/// Drive the LOCAL peer's owed mental-deal contributions to quiescence: repeatedly poke
/// [`Table::local_deal_step`] and broadcast whatever it produces (this peer's KeyAnnounce on
/// hand start, its ShuffleAnnounce on its shuffle turn, its reveal-token batches) until no new
/// contribution is produced. Empty for placeholder hands. Captures any settle outcome.
///
/// This is what makes the deal progress WITHOUT an inbound trigger: it is this peer's turn to
/// shuffle, or it owes reveal tokens, and nothing else will speak for it. Producing one
/// contribution can immediately unblock the next (the Table loops internally too), but we also
/// loop here so a contribution that only becomes owed after the Table re-pumps is still emitted.
async fn pump_local_deal(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    last_outcome: &mut Option<HandOutcome>,
) -> Result<(), DriverError> {
    loop {
        let step = table.local_deal_step()?;
        if step.broadcasts.is_empty() && step.events.is_empty() {
            return Ok(());
        }
        capture_outcome(&step, last_outcome);
        send_step(handle, sender, me, targets, step).await?;
        if last_outcome.is_some() && !table.is_mental_hand() {
            // Settled (the live hand cleared); nothing more to pump.
            return Ok(());
        }
    }
}

/// Like [`pump_local_deal`] but for the GUEST: a settle here ends the hand, so each step goes
/// through [`finish_step`] to record the [`HandReport`] and broadcast (and capture for resend) the
/// `HandComplete` ack. A contested showdown frequently settles on this pump — when
/// `local_deal_step` decrypts the final hole — so capturing the ack here releases the host barrier.
#[allow(clippy::too_many_arguments)]
async fn pump_local_deal_guest(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    report: &mut GameReport,
    current_hole: &mut Option<[poker_game::Card; 2]>,
    last_ack: &mut Option<TableMessage>,
) -> Result<(), DriverError> {
    loop {
        let step = table.local_deal_step()?;
        if step.broadcasts.is_empty() && step.events.is_empty() {
            return Ok(());
        }
        if current_hole.is_none() {
            if let Some(h) = table.local_hole() {
                *current_hole = Some(h);
            }
        }
        if let Some(ack) =
            finish_step(handle, sender, me, targets, report, step, *current_hole).await?
        {
            *last_ack = Some(ack);
        }
    }
}

/// Re-attempt the GUEST's deferred inbound deal payloads as its deal advances, finishing each
/// applied step (a replayed reveal can settle the hand). Loops until a pass makes no progress.
#[allow(clippy::too_many_arguments)]
async fn guest_replay_deferred(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    report: &mut GameReport,
    current_hole: &mut Option<[poker_game::Card; 2]>,
    last_ack: &mut Option<TableMessage>,
    deferred: &mut Vec<(PeerId, TableMessage)>,
) -> Result<(), DriverError> {
    loop {
        if deferred.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(deferred);
        let before = batch.len();
        for (author, msg) in batch {
            if let Some(step) = apply_deal_payload(sender, table, deferred, None, author, msg)? {
                if current_hole.is_none() {
                    if let Some(h) = table.local_hole() {
                        *current_hole = Some(h);
                    }
                }
                if let Some(ack) =
                    finish_step(handle, sender, me, targets, report, step, *current_hole).await?
                {
                    *last_ack = Some(ack);
                }
            }
        }
        if deferred.len() >= before {
            return Ok(());
        }
    }
}

/// Record the last `Act` a [`Step`] broadcast so the host can retransmit it on the ticker.
fn remember_last_act(step: &Step, slot: &mut Option<TableMessage>) {
    for m in &step.broadcasts {
        if matches!(m, TableMessage::Act { .. }) {
            *slot = Some(m.clone());
        }
    }
}

/// If it is the local seat's turn, ask the strategy and send the resulting Act. Returns the `Act`
/// message (so the caller can retransmit it on loss), if any. A closing Act can settle the hand,
/// producing this peer's own showdown/run-out reveals in the same step; [`finish_step`] routes
/// those (deal payloads reliably) and captures the `HandComplete` ack.
#[allow(clippy::too_many_arguments)]
async fn act_if_local_turn(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    strategy: &mut dyn Strategy,
    report: &mut GameReport,
    last_ack: &mut Option<TableMessage>,
) -> Result<Option<TableMessage>, DriverError> {
    // Snapshot the hole before local_turn — a closing Act settles the hand and clears it.
    let local_hole = table.local_hole();
    let step = table.local_turn(strategy)?;
    let act = step
        .broadcasts
        .iter()
        .rev()
        .find(|m| matches!(m, TableMessage::Act { .. }))
        .cloned();
    if let Some(ack) = finish_step(handle, sender, me, targets, report, step, local_hole).await? {
        *last_ack = Some(ack);
    }
    Ok(act)
}

/// GUEST: if the local seat is driven by a BOT, act when it is its turn (via [`act_if_local_turn`]);
/// for a human seat this is a no-op — the action arrives on the `next_action` arm instead.
#[allow(clippy::too_many_arguments)]
async fn guest_bot_act(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    table: &mut Table,
    decider: &mut Decider,
    report: &mut GameReport,
    last_ack: &mut Option<TableMessage>,
) -> Result<Option<TableMessage>, DriverError> {
    if let Some(bot) = decider.bot() {
        act_if_local_turn(handle, sender, me, targets, table, bot, report, last_ack).await
    } else {
        Ok(None)
    }
}

/// Process a GUEST [`Step`]: record any completed hand into `report` and broadcast (over gossipsub)
/// a [`TableMessage::HandComplete`] ack for it, then route the step's messages — deal payloads
/// reliably to `targets`, everything else over gossipsub. Returns the last ack (for retransmission).
async fn finish_step(
    handle: &NodeHandle,
    sender: &mut DealSender,
    me: PeerId,
    targets: &[PeerId],
    report: &mut GameReport,
    step: Step,
    local_hole: Option<[poker_game::Card; 2]>,
) -> Result<Option<TableMessage>, DriverError> {
    let mut ack = None;
    for ev in &step.events {
        if let TableEvent::HandEnded(o) = ev {
            report.hands.push(HandReport {
                outcome: o.clone(),
                local_hole,
            });
            let a = TableMessage::HandComplete {
                hand_no: o.hand_no,
                final_stacks: o.final_stacks.clone(),
            };
            broadcast_retry(handle, &a).await?;
            ack = Some(a);
        }
    }
    send_step(handle, sender, me, targets, step).await?;
    Ok(ack)
}

/// Broadcast a single message, retrying on the benign `NoSubscribersYet` until the mesh forms.
async fn broadcast_retry(handle: &NodeHandle, msg: &TableMessage) -> Result<(), DriverError> {
    let bytes = msg.encode()?;
    loop {
        match handle.broadcast(bytes.clone()).await {
            Ok(()) => return Ok(()),
            Err(NetError::NoSubscribersYet) => {
                tokio::time::sleep(BROADCAST_RETRY).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn capture_outcome(step: &Step, out: &mut Option<HandOutcome>) {
    for ev in &step.events {
        if let TableEvent::HandEnded(o) = ev {
            *out = Some(o.clone());
        }
    }
}

/// True for table errors an honest peer must SURVIVE rather than abort on: rejected cheat
/// attempts (out-of-turn / non-host StartHand), and benign races/replays from gossipsub's
/// at-least-once, possibly-reordered delivery (an Act for a hand that already settled or has
/// not started on this peer yet, or a duplicate Act the engine rejects as not-your-turn).
/// Genuine logic faults (deck exhaustion, seat/stack mismatch) are NOT ignored.
fn is_ignorable(e: &TableError) -> bool {
    matches!(
        e,
        TableError::ActOutOfTurn { .. }
            | TableError::NotHost(_)
            | TableError::WrongHand { .. }
            | TableError::NoHandInProgress
            | TableError::Betting(_)
            // Trustless-deal benign races/replays from gossipsub's at-least-once, possibly
            // reordered delivery: a deal payload that arrives out of phase/turn, before this
            // peer has started the mental hand, or a duplicate the verifier rejects. None of
            // these are honest-peer faults; the deal pump + retransmit recover the live path.
            | TableError::DealOutOfTurn
            | TableError::DealAuthor
            | TableError::NoMentalHand
            | TableError::NotBettingYet
            // A deal payload whose embedded proof FAILS verification is a rejected cheat (or a
            // corrupted/duplicated wire frame): drop it and keep playing, exactly as M4 survives
            // a rejected out-of-turn Act. An honest peer never aborts on a peer's bad proof.
            | TableError::Deal2(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_seed_is_deterministic_and_distinct() {
        assert_eq!(demo_seed(1), demo_seed(1));
        assert_ne!(demo_seed(1), demo_seed(2));
    }

    #[test]
    fn host_options_default_is_heads_up_friendly() {
        let o = HostOptions::default();
        assert_eq!(o.min_players, 2);
        assert!(o.big_blind > o.small_blind);
    }
}
