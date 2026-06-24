//! Focused tests for the PURE replicated [`Table`] state machine (no networking, no async).
//!
//! These exercise the anti-cheat / determinism guarantees that the M4 design rests on:
//!   (a) an `Act` whose authenticated `from` is NOT the seat currently to-act is rejected and
//!       does not mutate table state;
//!   (b) a well-formed turn-by-turn message script for a full heads-up hand drives a fresh
//!       Table to the correct, chip-conserving result;
//!   (c) two independent Table instances fed the same `(message, from)` sequence end in
//!       byte-identical state (replication determinism).
//!
//! All construction uses only the public `poker_protocol` API plus `libp2p` PeerId helpers, so
//! these double as documentation that the pure machine is testable in isolation (no net seam
//! is required to validate the anti-cheat and determinism invariants).

use libp2p::identity::Keypair;
use libp2p::PeerId;
use poker_protocol::table::{TableError, TableEvent};
use poker_protocol::{Action, HandOutcome, Table, TableMessage};

/// A fresh random PeerId.
fn pid() -> PeerId {
    PeerId::from(Keypair::generate_ed25519().public())
}

/// Build a heads-up StartHand authored by `host` for the given seat order.
fn start_hand(
    host: PeerId,
    hand_no: u64,
    button: usize,
    seed: u64,
    seats: &[PeerId],
    stacks: Vec<u64>,
) -> (TableMessage, PeerId) {
    (
        TableMessage::StartHand {
            hand_no,
            button,
            seed,
            seats: seats.iter().map(|p| p.to_bytes()).collect(),
            stacks,
            small_blind: 5,
            big_blind: 10,
        },
        host,
    )
}

/// Pull the terminal `HandEnded` outcome out of a settled step, if present.
fn outcome_of(events: &[TableEvent]) -> Option<HandOutcome> {
    events.iter().find_map(|e| match e {
        TableEvent::HandEnded(o) => Some(o.clone()),
        _ => None,
    })
}

/// (a) An `Act` authored by a peer that does NOT own the seat-to-act is rejected with
/// `ActOutOfTurn`, and the table state is left completely untouched (no seat advance, no
/// betting mutation, hand still live).
#[test]
fn act_from_wrong_author_is_rejected_and_inert() {
    let host = pid();
    let guest = pid();
    // View from the guest. Heads-up, button = seat 0 (host) acts first preflop.
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();

    assert_eq!(t.seat_to_act(), Some(0), "host (seat 0) is to act first");

    // Capture a full snapshot of the observable state before the bad message.
    let to_act_before = t.seat_to_act();
    let contribs_before = t.betting().unwrap().contributions();
    let folded_before = t.betting().unwrap().folded_flags();
    let live_before = t.live_hand_no();
    let community_before = t.community().to_vec();

    // The guest (seat 1) is NOT to act, yet tries to author a Call. Must be rejected.
    let err = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, guest)
        .unwrap_err();
    match err {
        TableError::ActOutOfTurn { from, seat, owner } => {
            assert_eq!(from, guest, "rejected author is the impostor");
            assert_eq!(seat, 0, "seat 0 was the one to act");
            assert_eq!(owner, host, "seat 0 is owned by the host");
        }
        other => panic!("expected ActOutOfTurn, got {other:?}"),
    }

    // State must be byte-for-byte unchanged by the rejected message.
    assert_eq!(t.seat_to_act(), to_act_before);
    assert_eq!(t.betting().unwrap().contributions(), contribs_before);
    assert_eq!(t.betting().unwrap().folded_flags(), folded_before);
    assert_eq!(t.live_hand_no(), live_before);
    assert_eq!(t.community().to_vec(), community_before);
}

/// Extra anti-cheat angle: even the HOST cannot author an `Act` for a seat it does not own.
/// gossipsub StrictSign means the host can relay but never forge a guest's publish; the pure
/// Table enforces the same rule structurally by checking `from == roster[to_act]`.
#[test]
fn host_cannot_impersonate_a_guest_seat() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(host, host);
    // Button = seat 1 (guest), so heads-up the guest (button/SB) acts first preflop.
    let (sh, sh_from) = start_hand(host, 1, 1, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();
    assert_eq!(t.seat_to_act(), Some(1), "guest (seat 1, button) acts first");

    // Host tries to act on the guest's behalf -> rejected, nothing moves.
    let err = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, host)
        .unwrap_err();
    assert!(
        matches!(err, TableError::ActOutOfTurn { owner, .. } if owner == guest),
        "host must not be able to act for the guest's seat, got {err:?}"
    );
    assert_eq!(t.seat_to_act(), Some(1), "still guest's turn after rejection");
}

/// Extra anti-cheat / reliability angle: a *replayed* legitimate Act (correct author, but the
/// seat has already advanced) is harmlessly rejected and does not double-apply.
#[test]
fn replayed_act_is_rejected_after_seat_advances() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(host, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();

    // Host (seat 0) calls legitimately; seat advances to the guest.
    t.handle(TableMessage::Act { hand_no: 1, action: Action::Call }, host)
        .unwrap();
    assert_eq!(t.seat_to_act(), Some(1));
    let contribs_after_first = t.betting().unwrap().contributions();

    // The same host Call arrives again (gossipsub at-least-once). Seat 0 is no longer to act,
    // so it is rejected as out-of-turn and contributions are unchanged.
    let err = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, host)
        .unwrap_err();
    assert!(matches!(err, TableError::ActOutOfTurn { .. }), "got {err:?}");
    assert_eq!(t.seat_to_act(), Some(1), "replay did not advance the seat");
    assert_eq!(
        t.betting().unwrap().contributions(),
        contribs_after_first,
        "replayed Call did not double-contribute"
    );
}

/// (b) A well-formed turn-by-turn script for a full heads-up hand (check/call down to showdown)
/// drives a fresh Table to a correct, terminal, chip-conserving result.
#[test]
fn full_heads_up_script_reaches_correct_result() {
    let host = pid();
    let guest = pid();
    // Button = seat 0 (host) => host is SB and acts first preflop; guest (BB, seat 1) acts
    // first on every postflop street.
    let mut t = Table::new(host, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 12345, &[host, guest], vec![1000, 1000]);
    let start_step = t.handle(sh, sh_from).unwrap();
    assert_eq!(
        start_step.events.first(),
        Some(&TableEvent::HandStarted(1)),
        "StartHand emits HandStarted"
    );

    // Author each Act with the correct seat owner.
    let script: Vec<(Action, PeerId)> = vec![
        (Action::Call, host),   // SB completes preflop
        (Action::Check, guest), // BB checks option -> flop
        (Action::Check, guest), // flop: BB first
        (Action::Check, host),  // flop: button
        (Action::Check, guest), // turn: BB first
        (Action::Check, host),  // turn: button
        (Action::Check, guest), // river: BB first
        (Action::Check, host),  // river: button -> showdown
    ];

    let mut outcome = None;
    for (action, from) in script {
        let step = t
            .handle(TableMessage::Act { hand_no: 1, action }, from)
            .unwrap_or_else(|e| panic!("legitimate Act {action:?} from {from} rejected: {e:?}"));
        if let Some(o) = outcome_of(&step.events) {
            outcome = Some(o);
        }
    }

    let o = outcome.expect("a full check/call-down must end the hand at showdown");
    assert!(t.live_hand_no().is_none(), "hand cleared after settlement");
    assert_eq!(o.hand_no, 1);
    assert_eq!(o.button, 0);
    assert_eq!(o.community.len(), 5, "full board dealt at showdown");
    // Both players limped for the BB (10) only: stacks are conserved and bounded.
    assert_eq!(o.deltas.iter().sum::<i64>(), 0, "chips conserved over the hand");
    assert_eq!(o.final_stacks.iter().sum::<u64>(), 2000, "total chips preserved");
    assert_eq!(o.final_stacks.len(), 2);
    assert!(!o.awards.is_empty(), "showdown produced at least one pot award");
    // No one folded, so neither seat lost more than the limped BB.
    for d in &o.deltas {
        assert!(d.abs() <= 10, "limped pot delta within one BB, got {d}");
    }
}

/// (c) Two independent Tables (one per peer's perspective) fed the SAME ordered
/// `(message, from)` sequence end in byte-identical settled state — the replication-determinism
/// guarantee the trustless relay depends on. We compare the full `HandOutcome` (which derives
/// `PartialEq`/`Eq` precisely so peers can diff for divergence) plus the post-settle accessors.
#[test]
fn two_tables_same_script_reach_identical_state() {
    let host = pid();
    let guest = pid();

    // A mixed script (a preflop raise + call, then a check-down) so determinism is exercised on
    // a non-trivial betting line, not just a flat checkdown.
    let (sh, sh_from) = start_hand(host, 1, 0, 0xC0FFEE, &[host, guest], vec![1000, 1000]);
    let script: Vec<(TableMessage, PeerId)> = vec![
        (sh, sh_from),
        (TableMessage::Act { hand_no: 1, action: Action::Raise(30) }, host), // SB raises
        (TableMessage::Act { hand_no: 1, action: Action::Call }, guest),     // BB calls -> flop
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),    // flop BB
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),     // flop button
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),    // turn BB
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),     // turn button
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),    // river BB
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),     // river button
    ];

    let mut t_host = Table::new(host, host);
    let mut t_guest = Table::new(guest, host);
    let mut outcome_host = None;
    let mut outcome_guest = None;

    for (msg, from) in &script {
        let sh_step = t_host.handle(msg.clone(), *from).unwrap();
        if let Some(o) = outcome_of(&sh_step.events) {
            outcome_host = Some(o);
        }
        let sg_step = t_guest.handle(msg.clone(), *from).unwrap();
        if let Some(o) = outcome_of(&sg_step.events) {
            outcome_guest = Some(o);
        }
    }

    let oh = outcome_host.expect("host table settled the hand");
    let og = outcome_guest.expect("guest table settled the hand");

    // The crux: byte-identical outcomes across independent peer views.
    assert_eq!(oh, og, "two peers must compute the identical HandOutcome");
    assert_eq!(oh.community.len(), 5);
    assert_eq!(oh.deltas.iter().sum::<i64>(), 0);
    assert_eq!(oh.final_stacks.iter().sum::<u64>(), 2000);

    // Post-settle accessors must also agree (both cleared the live hand, same roster, same
    // last-completed hand reflected by no live hand + identical outcome).
    assert_eq!(t_host.live_hand_no(), None);
    assert_eq!(t_guest.live_hand_no(), None);
    assert_eq!(t_host.roster(), t_guest.roster());
    assert_eq!(t_host.community(), t_guest.community());
    assert_eq!(t_host.seat_to_act(), t_guest.seat_to_act());

    // Determinism must also be stable to a SHUFFLE-equivalent of delivery that preserves causal
    // order: a fresh third table fed the very same sequence reproduces the same outcome.
    let mut t_replay = Table::new(host, host);
    let mut outcome_replay = None;
    for (msg, from) in &script {
        let step = t_replay.handle(msg.clone(), *from).unwrap();
        if let Some(o) = outcome_of(&step.events) {
            outcome_replay = Some(o);
        }
    }
    assert_eq!(
        outcome_replay.expect("replay settled"),
        oh,
        "re-running the identical script reproduces the identical outcome"
    );
}
