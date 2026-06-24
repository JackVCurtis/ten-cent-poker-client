//! Make-or-break M4 integration test: REAL in-process `poker_net` nodes (a host plus one or
//! more guests, connected via the host's `tcpoker://` URI) play ONE full hand of Hold'em over
//! the live gossipsub mesh, and every peer must independently arrive at the SAME hand outcome.
//!
//! These tests exercise the whole stack end-to-end: URI assembly, dialing, mesh formation
//! (with the benign `NoSubscribersYet` retry), the lossy-broadcast retransmit/idempotency
//! machinery, the replicated `Table`, and showdown distribution. A deterministic always-
//! check/call strategy (`CallStationBot`) is used so the hand reaches showdown the same way on
//! every peer regardless of who acts in which order.
//!
//! Assertions:
//!   (a) ALL peers connect and complete the hand (no hang, no panic, no disconnect abort);
//!   (b) every peer independently computes the IDENTICAL `HandOutcome` (same winning seat(s),
//!       same per-seat deltas, same final stacks, same board);
//!   (c) chips are conserved (per-hand deltas sum to zero, and final stacks sum to the total
//!       chips put on the table).

use poker_protocol::{
    run_guest_interactive, run_guest_with_config, run_host, run_host_interactive, Action,
    CallStationBot, DriverUpdate, GameReport, HostOptions,
};
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::Mutex;

/// Generous overall budget for a single hand over a freshly-formed loopback mesh. The host runs
/// a 400ms retransmit ticker to recover gossipsub's lossy delivery, so completion is fast once
/// the mesh forms; we keep this budget large to absorb slow mesh formation on loaded CI.
const HAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Generous budget for the host to assemble and surface a `tcpoker://` URI.
const URI_TIMEOUT: Duration = Duration::from_secs(30);

/// Serialize the networked tests in this file. Standing up 2-3 real libp2p nodes per test on a
/// shared loopback runtime is contention-sensitive: running both tests' nodes (5 nodes) at once
/// starves the gossipsub mesh and stalls recovery. A process-wide async lock makes the two tests
/// run one-at-a-time even under the default concurrent `cargo test` harness, without touching the
/// production code. (The lock is `OnceLock`-initialized; `StdMutex` only guards that init.)
///
/// NOTE: the lock alone is NOT sufficient. Two further things make the FULL binary reliable, both
/// in production code (`net`): (1) every node is created with mDNS DISABLED (`enable_mdns: false`
/// below) so the *previous* test's lingering loopback nodes are never auto-discovered/dialed into
/// *this* test's fresh gossipsub mesh — the tests dial explicitly via the URI, so mDNS is pure
/// cross-test interference here; and (2) `NodeHandle::shutdown` now AWAITS real swarm-task
/// termination, so a finished test's sockets are released before the next test starts. Without
/// these, the second networked test in a process intermittently stalled to its 60s timeout.
fn net_test_lock() -> &'static Mutex<()> {
    static LOCK: StdMutex<Option<&'static Mutex<()>>> = StdMutex::new(None);
    let mut guard = LOCK.lock().unwrap();
    if let Some(l) = *guard {
        return l;
    }
    let l: &'static Mutex<()> = Box::leak(Box::new(Mutex::new(())));
    *guard = Some(l);
    l
}

/// Drive a host playing exactly one hand, plumbing its first URI out via a oneshot.
async fn spawn_host(
    opts: HostOptions,
) -> (
    tokio::task::JoinHandle<Result<GameReport, poker_protocol::DriverError>>,
    tokio::sync::oneshot::Receiver<String>,
) {
    let (uri_tx, uri_rx) = tokio::sync::oneshot::channel::<String>();
    let mut uri_tx = Some(uri_tx);
    let host = tokio::spawn(async move {
        run_host(CallStationBot, opts, move |uri| {
            if let Some(tx) = uri_tx.take() {
                let _ = tx.send(uri.to_string());
            }
        })
        .await
    });
    (host, uri_rx)
}

/// Assert a single `HandOutcome` is internally well-formed and conserves chips, given the known
/// total chips put on the table (n_seats * starting_stack).
fn assert_outcome_well_formed(o: &poker_protocol::HandOutcome, total_chips: u64, n_seats: usize) {
    // (c) Chips conserved: deltas net to zero for the hand.
    assert_eq!(
        o.deltas.iter().sum::<i64>(),
        0,
        "hand #{} deltas must sum to zero (chips conserved), got {:?}",
        o.hand_no,
        o.deltas
    );
    // And final stacks still total the chips originally seated.
    assert_eq!(
        o.final_stacks.iter().sum::<u64>(),
        total_chips,
        "hand #{} final stacks must still total {} chips, got {:?}",
        o.hand_no,
        total_chips,
        o.final_stacks
    );
    assert_eq!(o.deltas.len(), n_seats, "one delta per seat");
    assert_eq!(o.final_stacks.len(), n_seats, "one final stack per seat");
    // At least one pot was awarded, and every award names at least one winning seat.
    assert!(
        !o.awards.is_empty(),
        "hand #{} produced no pot awards",
        o.hand_no
    );
    for award in &o.awards {
        assert!(
            !award.winners.is_empty(),
            "hand #{} pot of {} had no winners",
            o.hand_no,
            award.amount
        );
    }
    // The board for a hand that reaches showdown by checking/calling should be the full 5 cards.
    assert_eq!(
        o.community.len(),
        5,
        "hand #{} reached showdown so the board should have 5 community cards, got {}",
        o.hand_no,
        o.community.len()
    );
}

/// Two REAL nodes (host + one guest) over loopback play one full hand and must agree.
///
/// The trustless mental-poker deal rounds ride libp2p request-response (reliable, point-to-point)
/// with receiver-side deferral of out-of-phase payloads, so the historical intermittent showdown
/// stall (~15-30% of hands) is fixed. These two tests now run as part of the default suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_real_peers_play_one_hand_and_agree() {
    let _serial = net_test_lock().lock().await;
    let starting_stack = 1000u64;
    let opts = HostOptions {
        hands: 1,
        starting_stack,
        small_blind: 5,
        big_blind: 10,
        min_players: 2,
        keypair: None,
        // mDNS OFF in tests: this node must not auto-discover a prior test's lingering loopback
        // node and dial it into our fresh mesh. We dial explicitly via the URI below.
        enable_mdns: false,
        // TRUSTLESS: drive the real Barnett–Smart distributed deal over the wire.
        mental: true,
        listen_port: None,
    };

    let (host, uri_rx) = spawn_host(opts).await;

    let uri = tokio::time::timeout(URI_TIMEOUT, uri_rx)
        .await
        .expect("host should surface a tcpoker:// URI within the timeout (never produced a URI)")
        .expect("uri oneshot channel should not drop");

    // Always-call strategy drives the hand deterministically to showdown. mDNS OFF (tests).
    let guest =
        tokio::spawn(async move { run_guest_with_config(&uri, CallStationBot, None, false).await });

    // (a) Both peers must finish the hand within the budget (no hang).
    let host_report = tokio::time::timeout(HAND_TIMEOUT, host)
        .await
        .expect("host did not finish the hand in time (hung)")
        .expect("host task panicked")
        .expect("host run_host returned an error");
    let guest_report = tokio::time::timeout(HAND_TIMEOUT, guest)
        .await
        .expect("guest did not finish the hand in time (hung)")
        .expect("guest task panicked")
        .expect("guest run_guest returned an error");

    // Both peers played exactly one hand.
    assert_eq!(host_report.hands.len(), 1, "host should play exactly 1 hand");
    assert_eq!(
        guest_report.hands.len(),
        1,
        "guest should play exactly 1 hand"
    );

    let n_seats = 2;
    let total_chips = starting_stack * n_seats as u64;
    let host_outcome = &host_report.hands[0].outcome;
    let guest_outcome = &guest_report.hands[0].outcome;

    // (b) The two peers must agree byte-for-byte on the hand outcome (same winners, same
    //     deltas, same final stacks, same board). HandOutcome is PartialEq/Eq for exactly this.
    assert_eq!(
        host_outcome, guest_outcome,
        "host and guest disagree on the hand outcome\n host: {:?}\nguest: {:?}",
        host_outcome, guest_outcome
    );

    // (c) Well-formedness + chip conservation, checked on each independently-computed outcome.
    assert_outcome_well_formed(host_outcome, total_chips, n_seats);
    assert_outcome_well_formed(guest_outcome, total_chips, n_seats);
}

/// Three REAL nodes (host + two guests) over loopback play one full hand and must ALL agree.
/// The driver API supports this directly: `min_players: 3` makes the host wait for both guests
/// before dealing, and each guest joins the same URI.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn three_real_peers_play_one_hand_and_agree() {
    let _serial = net_test_lock().lock().await;
    let starting_stack = 1000u64;
    let opts = HostOptions {
        hands: 1,
        starting_stack,
        small_blind: 5,
        big_blind: 10,
        min_players: 3,
        keypair: None,
        // mDNS OFF in tests (see the 2-peer test): no cross-test loopback auto-discovery.
        enable_mdns: false,
        // TRUSTLESS: three real peers run the distributed deal over the wire.
        mental: true,
        listen_port: None,
    };

    let (host, uri_rx) = spawn_host(opts).await;

    let uri = tokio::time::timeout(URI_TIMEOUT, uri_rx)
        .await
        .expect("host should surface a tcpoker:// URI within the timeout (never produced a URI)")
        .expect("uri oneshot channel should not drop");

    let uri_a = uri.clone();
    let guest_a = tokio::spawn(async move {
        run_guest_with_config(&uri_a, CallStationBot, None, false).await
    });
    let guest_b =
        tokio::spawn(async move { run_guest_with_config(&uri, CallStationBot, None, false).await });

    // (a) All three peers must finish the hand within the budget (no hang).
    let host_report = tokio::time::timeout(HAND_TIMEOUT, host)
        .await
        .expect("host did not finish the hand in time (hung)")
        .expect("host task panicked")
        .expect("host run_host returned an error");
    let guest_a_report = tokio::time::timeout(HAND_TIMEOUT, guest_a)
        .await
        .expect("guest A did not finish the hand in time (hung)")
        .expect("guest A task panicked")
        .expect("guest A run_guest returned an error");
    let guest_b_report = tokio::time::timeout(HAND_TIMEOUT, guest_b)
        .await
        .expect("guest B did not finish the hand in time (hung)")
        .expect("guest B task panicked")
        .expect("guest B run_guest returned an error");

    assert_eq!(host_report.hands.len(), 1, "host should play exactly 1 hand");
    assert_eq!(
        guest_a_report.hands.len(),
        1,
        "guest A should play exactly 1 hand"
    );
    assert_eq!(
        guest_b_report.hands.len(),
        1,
        "guest B should play exactly 1 hand"
    );

    let n_seats = 3;
    let total_chips = starting_stack * n_seats as u64;
    let host_outcome = &host_report.hands[0].outcome;
    let guest_a_outcome = &guest_a_report.hands[0].outcome;
    let guest_b_outcome = &guest_b_report.hands[0].outcome;

    // (b) All three peers must agree on the identical outcome.
    assert_eq!(
        host_outcome, guest_a_outcome,
        "host and guest A disagree on the hand outcome\n host: {:?}\n  gA: {:?}",
        host_outcome, guest_a_outcome
    );
    assert_eq!(
        host_outcome, guest_b_outcome,
        "host and guest B disagree on the hand outcome\n host: {:?}\n  gB: {:?}",
        host_outcome, guest_b_outcome
    );

    // (c) Well-formedness + chip conservation on each independently-computed outcome.
    assert_outcome_well_formed(host_outcome, total_chips, n_seats);
    assert_outcome_well_formed(guest_a_outcome, total_chips, n_seats);
    assert_outcome_well_formed(guest_b_outcome, total_chips, n_seats);
}

/// Observer stand-in for a human: when it is our turn, send a check (if free) or a call. Used by
/// the interactive test to drive a hand through the GUI's action channel without a real UI.
fn auto_call(tx: &tokio::sync::mpsc::Sender<Action>, u: DriverUpdate) {
    if let DriverUpdate::State(t) = u {
        if t.is_local_turn() {
            if let (Some(seat), Some(b)) = (t.local_seat(), t.betting()) {
                let action = if b.to_call(seat) == 0 {
                    Action::Check
                } else {
                    Action::Call
                };
                let _ = tx.try_send(action);
            }
        }
    }
}

/// Two REAL nodes play one hand driven through the INTERACTIVE drivers (the GUI's code path): each
/// seat's action is supplied asynchronously on a channel rather than by an inline bot. An observer
/// auto-supplies check/call on the local turn, standing in for a human clicking — proving the
/// human-action bridge (`OneShot` + the `next_action` select arm) drives a full trustless hand to
/// agreement without stalling, and that the bot path's reliability fix carries over to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interactive_two_peers_play_one_hand_and_agree() {
    let _serial = net_test_lock().lock().await;
    let starting_stack = 1000u64;
    let opts = HostOptions {
        hands: 1,
        starting_stack,
        small_blind: 5,
        big_blind: 10,
        min_players: 2,
        keypair: None,
        enable_mdns: false,
        mental: true,
        listen_port: None,
    };

    let (host_tx, host_rx) = tokio::sync::mpsc::channel::<Action>(1);
    let (uri_tx, uri_rx) = tokio::sync::oneshot::channel::<String>();
    let host = tokio::spawn(async move {
        let mut uri_tx = Some(uri_tx);
        let mut obs = move |u: DriverUpdate| auto_call(&host_tx, u);
        run_host_interactive(
            host_rx,
            opts,
            move |uri| {
                if let Some(tx) = uri_tx.take() {
                    let _ = tx.send(uri.to_string());
                }
            },
            &mut obs,
        )
        .await
    });

    let uri = tokio::time::timeout(URI_TIMEOUT, uri_rx)
        .await
        .expect("host should surface a tcpoker:// URI within the timeout")
        .expect("uri oneshot channel should not drop");

    let (guest_tx, guest_rx) = tokio::sync::mpsc::channel::<Action>(1);
    let guest = tokio::spawn(async move {
        let mut obs = move |u: DriverUpdate| auto_call(&guest_tx, u);
        run_guest_interactive(&uri, guest_rx, None, false, &mut obs).await
    });

    let host_report = tokio::time::timeout(HAND_TIMEOUT, host)
        .await
        .expect("interactive host did not finish in time (hung)")
        .expect("host task panicked")
        .expect("host returned an error");
    let guest_report = tokio::time::timeout(HAND_TIMEOUT, guest)
        .await
        .expect("interactive guest did not finish in time (hung)")
        .expect("guest task panicked")
        .expect("guest returned an error");

    assert_eq!(host_report.hands.len(), 1, "host should play exactly 1 hand");
    assert_eq!(guest_report.hands.len(), 1, "guest should play exactly 1 hand");
    assert_eq!(
        host_report.hands[0].outcome, guest_report.hands[0].outcome,
        "interactive host and guest disagree on the hand outcome\n host: {:?}\nguest: {:?}",
        host_report.hands[0].outcome, guest_report.hands[0].outcome
    );
}
