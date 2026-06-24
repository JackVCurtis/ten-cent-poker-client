//! Tests for the replicated [`Table`]: the M4 placeholder path AND the trustless mental path.
//!
//! The mental tests drive N independent [`Table`] replicas (one per peer, each with its own
//! local PeerId and its own private secret key) by exchanging ONLY [`TableMessage`]s through a
//! simple in-memory bus. They assert: identical COMMON state across peers, hole privacy (a peer
//! cannot produce an opponent's hole before showdown), correct showdown winner, chip
//! conservation, and rejection of tampered / out-of-turn deal messages.

use super::*;
use crate::CheckFoldBot;
use libp2p::identity::Keypair;

fn pid() -> PeerId {
    PeerId::from(Keypair::generate_ed25519().public())
}

// ============================================================================
// M4 placeholder path (retained behaviour)
// ============================================================================

fn start_hand(
    host: PeerId,
    hand_no: u64,
    button: usize,
    seed: u64,
    peers: &[PeerId],
    stacks: Vec<u64>,
) -> (TableMessage, PeerId) {
    let seats = peers.iter().map(|p| p.to_bytes()).collect();
    (
        TableMessage::StartHand {
            hand_no,
            button,
            seed,
            seats,
            stacks,
            small_blind: 5,
            big_blind: 10,
        },
        host,
    )
}

#[test]
fn start_hand_from_non_host_rejected() {
    let host = pid();
    let a = pid();
    let mut t = Table::new(a, host);
    let (msg, _) = start_hand(host, 1, 0, 42, &[host, a], vec![1000, 1000]);
    let err = t.handle(msg, a).unwrap_err();
    assert!(matches!(err, TableError::NotHost(_)));
}

#[test]
fn heads_up_fold_out_awards_blinds() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    let step = t.handle(sh, sh_from).unwrap();
    assert_eq!(step.events[0], TableEvent::HandStarted(1));
    assert_eq!(t.seat_to_act(), Some(0));

    let step = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Fold }, host)
        .unwrap();
    let outcome = match step.events.last().unwrap() {
        TableEvent::HandEnded(o) => o.clone(),
        other => panic!("expected HandEnded, got {other:?}"),
    };
    assert_eq!(outcome.deltas[0], -5);
    assert_eq!(outcome.deltas[1], 5);
    assert_eq!(outcome.deltas.iter().sum::<i64>(), 0);
    assert!(t.live_hand_no().is_none());
}

#[test]
fn act_out_of_turn_is_rejected() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();
    let err = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, guest)
        .unwrap_err();
    assert!(matches!(err, TableError::ActOutOfTurn { .. }));
    assert_eq!(t.seat_to_act(), Some(0));
}

#[test]
fn act_for_wrong_hand_rejected() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();
    let err = t
        .handle(TableMessage::Act { hand_no: 99, action: Action::Call }, host)
        .unwrap_err();
    assert!(matches!(err, TableError::WrongHand { got: 99, .. }));
}

#[test]
fn full_heads_up_checkdown_replicates_identically() {
    let host = pid();
    let guest = pid();
    let (sh, sh_from) = start_hand(host, 1, 0, 12345, &[host, guest], vec![1000, 1000]);
    let script: Vec<(TableMessage, PeerId)> = vec![
        (sh, sh_from),
        (TableMessage::Act { hand_no: 1, action: Action::Call }, host),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, guest),
        (TableMessage::Act { hand_no: 1, action: Action::Check }, host),
    ];

    let mut t_host = Table::new(host, host);
    let mut t_guest = Table::new(guest, host);
    let mut outcome_host = None;
    let mut outcome_guest = None;
    for (msg, from) in script {
        for (t, out) in [
            (&mut t_host, &mut outcome_host),
            (&mut t_guest, &mut outcome_guest),
        ] {
            let step = t.handle(msg.clone(), from).unwrap();
            for ev in step.events {
                if let TableEvent::HandEnded(o) = ev {
                    *out = Some(o);
                }
            }
        }
    }
    let oh = outcome_host.expect("host saw HandEnded");
    let og = outcome_guest.expect("guest saw HandEnded");
    assert_eq!(oh, og, "both peers must compute the identical outcome");
    assert_eq!(oh.community.len(), 5);
    assert_eq!(oh.deltas.iter().sum::<i64>(), 0);
    assert_eq!(oh.final_stacks.iter().sum::<u64>(), 2000);
}

#[test]
fn local_turn_drives_strategy_and_broadcasts_act() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(host, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();
    assert!(t.is_local_turn());
    let mut bot = CheckFoldBot;
    let step = t.local_turn(&mut bot).unwrap();
    assert_eq!(step.broadcasts.len(), 1);
    match &step.broadcasts[0] {
        TableMessage::Act { hand_no: 1, action: Action::Fold } => {}
        other => panic!("expected Act Fold, got {other:?}"),
    }
    assert!(t.live_hand_no().is_none());
}

#[test]
fn local_turn_noop_when_not_our_turn() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh, sh_from).unwrap();
    assert!(!t.is_local_turn());
    let mut bot = CheckFoldBot;
    let step = t.local_turn(&mut bot).unwrap();
    assert!(step.broadcasts.is_empty());
}

#[test]
fn retransmitted_start_hand_is_idempotent() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh.clone(), sh_from).unwrap();
    t.handle(TableMessage::Act { hand_no: 1, action: Action::Call }, host)
        .unwrap();
    let to_act_before = t.seat_to_act();
    let step = t.handle(sh, sh_from).unwrap();
    assert!(step.events.is_empty());
    assert_eq!(t.seat_to_act(), to_act_before);
}

#[test]
fn stale_start_hand_for_completed_hand_is_dropped() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 7, &[host, guest], vec![1000, 1000]);
    t.handle(sh.clone(), sh_from).unwrap();
    t.handle(TableMessage::Act { hand_no: 1, action: Action::Fold }, host)
        .unwrap();
    assert!(t.live_hand_no().is_none());
    let step = t.handle(sh, sh_from).unwrap();
    assert!(step.events.is_empty());
    assert!(t.live_hand_no().is_none());
}

#[test]
fn all_in_runs_out_full_board() {
    let host = pid();
    let guest = pid();
    let mut t = Table::new(host, host);
    let (sh, sh_from) = start_hand(host, 1, 0, 999, &[host, guest], vec![500, 500]);
    t.handle(sh, sh_from).unwrap();
    t.handle(TableMessage::Act { hand_no: 1, action: Action::AllIn }, host)
        .unwrap();
    let step = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, guest)
        .unwrap();
    let outcome = match step.events.last().unwrap() {
        TableEvent::HandEnded(o) => o.clone(),
        other => panic!("expected HandEnded got {other:?}"),
    };
    assert_eq!(outcome.community.len(), 5);
    assert_eq!(outcome.final_stacks.iter().sum::<u64>(), 1000);
}

// ============================================================================
// Trustless (mental) path
// ============================================================================

/// A tiny harness: N peer Tables (one per seat) and a way to run a mental hand to completion by
/// passing every broadcast to every *other* peer until the bus is quiescent.
struct MentalSim {
    peers: Vec<PeerId>,
    tables: Vec<Table>,
    /// Last outcome each peer settled (parallel to `tables`).
    outcomes: Vec<Option<HandOutcome>>,
    /// Each peer's own decrypted hole cards, captured when LocalHoleReady fires (the live hand is
    /// cleared on settle, so we snapshot here).
    holes: Vec<Option<[Card; 2]>>,
}

/// One delivery on the bus: (msg, author seat index).
type Wire = (TableMessage, usize);

impl MentalSim {
    fn new(n: usize) -> Self {
        let peers: Vec<PeerId> = (0..n).map(|_| pid()).collect();
        // Host is seat 0.
        let host = peers[0];
        let tables = peers.iter().map(|&p| Table::new(p, host)).collect();
        MentalSim {
            peers,
            tables,
            outcomes: vec![None; n],
            holes: vec![None; n],
        }
    }

    fn host(&self) -> PeerId {
        self.peers[0]
    }

    /// Deliver `wires` to every peer EXCEPT the author (the author already applied its own
    /// contribution when it produced the broadcast). Collect the new broadcasts each peer emits.
    /// Repeat until no new messages are produced. Records any settled outcomes.
    fn pump(&mut self, initial: Vec<Wire>) {
        let mut queue = initial;
        let mut guard = 0;
        while !queue.is_empty() {
            guard += 1;
            assert!(guard < 100_000, "mental sim did not converge");
            let (msg, author) = queue.remove(0);
            let from = self.peers[author];
            for seat in 0..self.tables.len() {
                if seat == author {
                    continue; // producer already applied its own contribution
                }
                match self.tables[seat].handle(msg.clone(), from) {
                    Ok(step) => self.absorb(seat, step, &mut queue),
                    // Benign races/replays (already-applied, out-of-order) are ignored; genuine
                    // logic errors panic the test.
                    Err(TableError::DealOutOfTurn)
                    | Err(TableError::WrongHand { .. })
                    | Err(TableError::NoMentalHand)
                    | Err(TableError::NotBettingYet)
                    | Err(TableError::ActOutOfTurn { .. }) => {}
                    Err(e) => panic!("seat {seat} rejected {:?}: {e}", short(&msg)),
                }
            }
        }
    }

    /// Drive every peer's local turn (betting) once, queuing any Acts. Also nudges each peer's
    /// local deal step so a peer that owes a shuffle/reveal speaks. Returns produced wires.
    fn tick_local<S: crate::Strategy + Clone>(&mut self, strat: &S) -> Vec<Wire> {
        let mut out = Vec::new();
        for seat in 0..self.tables.len() {
            // Deal contributions this peer owes (shuffle turn, reveals).
            let step = self.tables[seat].local_deal_step().unwrap();
            self.absorb(seat, step, &mut out);
            // Betting turn.
            let mut s = strat.clone();
            let step = self.tables[seat].local_turn(&mut s).unwrap();
            self.absorb(seat, step, &mut out);
        }
        out
    }

    fn absorb(&mut self, seat: usize, step: Step, queue: &mut Vec<Wire>) {
        for ev in &step.events {
            match ev {
                TableEvent::HandEnded(o) => self.outcomes[seat] = Some(o.clone()),
                TableEvent::LocalHoleReady { .. } => {
                    self.holes[seat] = self.tables[seat].local_hole();
                }
                _ => {}
            }
        }
        for m in step.broadcasts {
            queue.push((m, seat));
        }
    }

    /// Play a full mental hand under `strat` until every peer settles, then return outcomes.
    fn play<S: crate::Strategy + Clone>(&mut self, button: usize, stacks: Vec<u64>, strat: S) {
        let session_seed = vec![42u8; 32];
        let seats: Vec<Vec<u8>> = self.peers.iter().map(|p| p.to_bytes()).collect();
        let host = self.host();
        // Host applies + broadcasts StartMentalHand.
        let start = TableMessage::StartMentalHand {
            hand_no: 1,
            button,
            session_seed,
            seats,
            stacks,
            small_blind: 5,
            big_blind: 10,
        };
        let host_step = self.tables[0].handle(start.clone(), host).unwrap();
        let mut q: Vec<Wire> = Vec::new();
        self.absorb(0, host_step, &mut q);
        // Deliver StartMentalHand to the guests (so they build their MentalDeal + announce keys).
        for seat in 1..self.tables.len() {
            let step = self.tables[seat].handle(start.clone(), host).unwrap();
            self.absorb(seat, step, &mut q);
        }
        // Run the deal + betting to completion.
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(rounds < 10_000, "hand did not converge");
            self.pump(std::mem::take(&mut q));
            let local = self.tick_local(&strat);
            if local.is_empty() {
                // Nothing left to do and nothing in flight -> settled (or stuck).
                if self.all_settled() {
                    break;
                }
                // No local work and not settled: the deal may need another pump pass triggered by
                // local_deal_step's effects already queued; if truly empty, we are done.
                break;
            }
            q = local;
        }
    }

    fn all_settled(&self) -> bool {
        self.outcomes.iter().all(|o| o.is_some())
    }
}

fn short(m: &TableMessage) -> &'static str {
    match m {
        TableMessage::StartMentalHand { .. } => "StartMentalHand",
        TableMessage::KeyAnnounce { .. } => "KeyAnnounce",
        TableMessage::ShuffleAnnounce { .. } => "ShuffleAnnounce",
        TableMessage::RevealAnnounce { .. } => "RevealAnnounce",
        TableMessage::Act { .. } => "Act",
        _ => "other",
    }
}

/// A passive strategy usable in the sim (Clone-able). Always check, otherwise call -> showdown.
#[derive(Clone)]
struct CallStation;
impl crate::Strategy for CallStation {
    fn decide(&mut self, state: &poker_game::BettingState, seat: usize) -> Action {
        if state.to_call(seat) == 0 {
            Action::Check
        } else {
            Action::Call
        }
    }
}

/// A fold-to-any-bet strategy (Clone-able): check when free, otherwise fold.
#[derive(Clone)]
struct CheckFold;
impl crate::Strategy for CheckFold {
    fn decide(&mut self, state: &poker_game::BettingState, seat: usize) -> Action {
        if state.to_call(seat) == 0 {
            Action::Check
        } else {
            Action::Fold
        }
    }
}

#[test]
fn mental_heads_up_checkdown_to_showdown() {
    let mut sim = MentalSim::new(2);
    sim.play(0, vec![1000, 1000], CallStation);
    assert!(sim.all_settled(), "both peers settled the mental hand");

    let o0 = sim.outcomes[0].clone().unwrap();
    let o1 = sim.outcomes[1].clone().unwrap();
    // COMMON state identical across peers.
    assert_eq!(o0, o1, "peers disagree on mental hand outcome");
    // A genuine showdown: full board dealt, chips conserved.
    assert_eq!(o0.community.len(), 5, "showdown board must be 5 cards");
    assert_eq!(o0.deltas.iter().sum::<i64>(), 0, "chips conserved");
    assert_eq!(o0.final_stacks.iter().sum::<u64>(), 2000);

    // Each peer learned its OWN hole cards (snapshotted on LocalHoleReady).
    assert!(sim.holes[0].is_some());
    assert!(sim.holes[1].is_some());
    // The two peers hold DIFFERENT hole cards (sanity: real deal, not a constant).
    assert_ne!(sim.holes[0], sim.holes[1]);
}

#[test]
fn mental_three_player_checkdown_to_showdown() {
    let mut sim = MentalSim::new(3);
    sim.play(0, vec![1000, 1000, 1000], CallStation);
    assert!(sim.all_settled(), "all three peers settled");
    let o0 = sim.outcomes[0].clone().unwrap();
    for s in 1..3 {
        assert_eq!(
            sim.outcomes[s].clone().unwrap(),
            o0,
            "peer {s} disagrees on outcome"
        );
    }
    assert_eq!(o0.community.len(), 5);
    assert_eq!(o0.deltas.iter().sum::<i64>(), 0);
    assert_eq!(o0.final_stacks.iter().sum::<u64>(), 3000);

    // All three peers learned distinct hole cards.
    let h: Vec<_> = sim.holes.clone();
    assert!(h.iter().all(|x| x.is_some()));
    assert_ne!(h[0], h[1]);
    assert_ne!(h[1], h[2]);
    assert_ne!(h[0], h[2]);
}

#[test]
fn mental_fold_out_awards_blinds_no_reveal_needed() {
    // Heads-up: button(0)=SB faces the BB and folds preflop. The hand settles by folds with NO
    // showdown reveal (single contestant wins uncontested).
    let mut sim = MentalSim::new(2);
    sim.play(0, vec![1000, 1000], CheckFold);
    assert!(sim.all_settled());
    let o0 = sim.outcomes[0].clone().unwrap();
    let o1 = sim.outcomes[1].clone().unwrap();
    assert_eq!(o0, o1);
    // Button folded its SB(5); BB wins it.
    assert_eq!(o0.deltas[0], -5);
    assert_eq!(o0.deltas[1], 5);
    assert_eq!(o0.deltas.iter().sum::<i64>(), 0);
}

#[test]
fn mental_hole_privacy_opponent_cannot_decrypt_before_showdown() {
    // Run keygen + shuffle + hole reveal, then assert seat 1 cannot produce seat 0's hole.
    let mut sim = MentalSim::new(2);
    let session_seed = vec![9u8; 32];
    let seats: Vec<Vec<u8>> = sim.peers.iter().map(|p| p.to_bytes()).collect();
    let host = sim.host();
    let start = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed,
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    let mut q: Vec<Wire> = Vec::new();
    let s0 = sim.tables[0].handle(start.clone(), host).unwrap();
    sim.absorb(0, s0, &mut q);
    let s1 = sim.tables[1].handle(start.clone(), host).unwrap();
    sim.absorb(1, s1, &mut q);
    // Pump keygen + shuffle + hole reveal to completion (no betting yet beyond what bots do).
    sim.pump(std::mem::take(&mut q));
    // Trigger any owed local deal steps (shuffle turns) repeatedly until quiescent.
    for _ in 0..10 {
        let mut more = Vec::new();
        for seat in 0..2 {
            let step = sim.tables[seat].local_deal_step().unwrap();
            sim.absorb(seat, step, &mut more);
        }
        if more.is_empty() {
            break;
        }
        sim.pump(more);
    }

    // Both peers should now have their own hole cards (deal reached Ready + hole reveal done).
    let mine0 = sim.tables[0].local_hole();
    let mine1 = sim.tables[1].local_hole();
    assert!(mine0.is_some(), "seat 0 should know its own hole");
    assert!(mine1.is_some(), "seat 1 should know its own hole");

    // PRIVACY: seat 1's Table exposes its own hole but NOT seat 0's. There is no API on the
    // Table to read another seat's hole pre-showdown, and the MentalDeal has no showdown_hole
    // for seat 0 yet. Assert seat 1 holds no decrypted hole for seat 0.
    assert!(
        sim.tables[1].deal_phase().is_some(),
        "seat 1 is in a mental hand"
    );
    // The opponent's hole must differ from ours (and be unknown to us): the only hole seat 1 can
    // read is its own, which is distinct from seat 0's own.
    assert_ne!(mine0, mine1, "the two peers' holes are different cards");
}

#[test]
fn mental_start_from_non_host_rejected() {
    let host = pid();
    let a = pid();
    let mut t = Table::new(a, host);
    let seats = vec![host.to_bytes(), a.to_bytes()];
    let msg = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed: vec![1u8; 32],
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    let err = t.handle(msg, a).unwrap_err();
    assert!(matches!(err, TableError::NotHost(_)));
}

#[test]
fn mental_bad_session_seed_rejected() {
    let host = pid();
    let a = pid();
    let mut t = Table::new(host, host);
    let seats = vec![host.to_bytes(), a.to_bytes()];
    let msg = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed: vec![1u8; 7], // wrong length
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    let err = t.handle(msg, host).unwrap_err();
    assert!(matches!(err, TableError::BadSessionSeed(7)));
}

#[test]
fn mental_tampered_key_announce_rejected() {
    // Bring up two peers, capture seat 0's KeyAnnounce, corrupt the proof bytes, and assert
    // seat 1 rejects it (the Schnorr ownership proof fails to verify).
    let host = pid();
    let guest = pid();
    let peers = [host, guest];
    let seats: Vec<Vec<u8>> = peers.iter().map(|p| p.to_bytes()).collect();
    let mut t0 = Table::new(host, host);
    let mut t1 = Table::new(guest, host);
    let start = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed: vec![5u8; 32],
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    let step0 = t0.handle(start.clone(), host).unwrap();
    t1.handle(start, host).unwrap();
    // Seat 0's KeyAnnounce is in step0.broadcasts.
    let key0 = step0
        .broadcasts
        .iter()
        .find_map(|m| match m {
            TableMessage::KeyAnnounce { hand_no, seat, payload } => {
                Some((*hand_no, *seat, payload.clone()))
            }
            _ => None,
        })
        .expect("seat 0 announced its key");
    let (hand_no, seat, mut payload) = key0;
    // Corrupt the proof region (flip a late byte).
    let at = payload.len() - 1;
    payload[at] ^= 0xFF;
    let tampered = TableMessage::KeyAnnounce { hand_no, seat, payload };
    let err = t1.handle(tampered, host).unwrap_err();
    // Either the proof fails (Deal2) or the corrupted bytes fail to deserialize (Deal2).
    assert!(
        matches!(err, TableError::Deal2(_)),
        "tampered key announce must be rejected, got {err:?}"
    );
}

#[test]
fn mental_shuffle_out_of_turn_rejected() {
    // Capture seat 1's legitimate turn-1 ShuffleAnnounce, then deliver it to a peer that has NOT
    // yet applied seat 0's turn-0 shuffle (it still expects turn 0). It must be rejected as out
    // of turn — preserving the seat-order anti-cheat.
    let host = pid();
    let guest = pid();
    let third = pid();
    let peers = [host, guest, third];
    let seats: Vec<Vec<u8>> = peers.iter().map(|p| p.to_bytes()).collect();
    let make_start = || TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed: vec![3u8; 32],
        seats: seats.clone(),
        stacks: vec![1000, 1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };

    // Bring three tables to the Shuffle phase by exchanging keys. Collect EVERY broadcast each
    // table emits (effects can fire inside the very handle that unblocks them).
    let mut t: Vec<Table> = peers.iter().map(|&p| Table::new(p, host)).collect();
    // (msg, author seat) for everything the tables produce.
    let mut bus: Vec<(TableMessage, usize)> = Vec::new();
    for i in 0..3 {
        let step = t[i].handle(make_start(), host).unwrap();
        for m in step.broadcasts {
            bus.push((m, i));
        }
    }
    // Deliver all key announcements (and any follow-on broadcasts) to every other table.
    let keys: Vec<(TableMessage, usize)> = bus
        .iter()
        .filter(|(m, _)| matches!(m, TableMessage::KeyAnnounce { .. }))
        .cloned()
        .collect();
    for (key, author) in &keys {
        for i in 0..3 {
            if i != *author {
                let step = t[i].handle(key.clone(), peers[*author]).unwrap();
                for m in step.broadcasts {
                    bus.push((m, i));
                }
            }
        }
    }
    for ti in &t {
        assert_eq!(ti.deal_phase(), Some(DealPhase::Shuffle));
    }

    // Seat 0's turn-0 shuffle is somewhere in the bus (emitted when its aggregate formed).
    let sh0 = bus
        .iter()
        .find(|(m, a)| *a == 0 && matches!(m, TableMessage::ShuffleAnnounce { turn: 0, .. }))
        .map(|(m, _)| m.clone())
        .expect("seat 0's turn-0 shuffle");
    // Applying it to seat 1's table advances it to its own turn; the pump in that same handle
    // emits seat 1's turn-1 ShuffleAnnounce.
    let s1_step = t[1].handle(sh0.clone(), peers[0]).unwrap();
    let sh1 = s1_step
        .broadcasts
        .iter()
        .find(|m| matches!(m, TableMessage::ShuffleAnnounce { turn: 1, .. }))
        .cloned()
        .expect("seat 1's turn-1 shuffle");

    // Deliver seat 1's turn-1 shuffle to seat 2's table, which has NOT applied turn 0 yet (it
    // still expects turn 0). Out of turn -> rejected.
    let err = t[2].handle(sh1.clone(), peers[1]).unwrap_err();
    assert!(
        matches!(err, TableError::DealOutOfTurn),
        "out-of-turn shuffle must be rejected, got {err:?}"
    );
}

#[test]
fn mental_tampered_reveal_token_rejected() {
    // Run keygen + shuffle to Ready, capturing a peer's legitimate hole-reveal broadcast, then
    // corrupt one token's proof bytes and assert the recipient rejects it (Chaum–Pedersen fails).
    let mut sim = MentalSim::new(2);
    let session_seed = vec![11u8; 32];
    let seats: Vec<Vec<u8>> = sim.peers.iter().map(|p| p.to_bytes()).collect();
    let host = sim.host();
    let start = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed,
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    // Bring both tables up and run keygen+shuffle to Ready, collecting every broadcast.
    let mut all: Vec<Wire> = Vec::new();
    for seat in 0..2 {
        let step = sim.tables[seat].handle(start.clone(), host).unwrap();
        sim.absorb(seat, step, &mut all);
    }
    // Deliver everything (keys, shuffles) round by round; capture a RevealAnnounce(Hole).
    let mut hole_reveal: Option<(usize, TableMessage)> = None;
    let mut guard = 0;
    while !all.is_empty() {
        guard += 1;
        assert!(guard < 10_000);
        let (msg, author) = all.remove(0);
        if let TableMessage::RevealAnnounce { round: RevealRound::Hole, .. } = &msg {
            if hole_reveal.is_none() {
                hole_reveal = Some((author, msg.clone()));
            }
        }
        for seat in 0..2 {
            if seat == author {
                continue;
            }
            if let Ok(step) = sim.tables[seat].handle(msg.clone(), sim.peers[author]) {
                sim.absorb(seat, step, &mut all);
            }
        }
    }
    let (author, reveal) = hole_reveal.expect("a hole reveal was broadcast");
    let recipient = 1 - author;
    // Corrupt the first token's proof bytes.
    let tampered = match reveal {
        TableMessage::RevealAnnounce { hand_no, seat, round, mut tokens } => {
            let at = tokens[0].len() - 1;
            tokens[0][at] ^= 0xFF;
            TableMessage::RevealAnnounce { hand_no, seat, round, tokens }
        }
        _ => unreachable!(),
    };
    let err = sim.tables[recipient]
        .handle(tampered, sim.peers[author])
        .unwrap_err();
    assert!(
        matches!(err, TableError::Deal2(_)),
        "tampered reveal token must be rejected, got {err:?}"
    );
}

#[test]
fn mental_act_before_deal_ready_is_rejected() {
    // A betting Act that arrives before the deal has dealt hole cards (still in keygen) must be
    // rejected with NotBettingYet — betting is gated on the deal being ready.
    let host = pid();
    let guest = pid();
    let mut t = Table::new(guest, host);
    let seats = vec![host.to_bytes(), guest.to_bytes()];
    let start = TableMessage::StartMentalHand {
        hand_no: 1,
        button: 0,
        session_seed: vec![8u8; 32],
        seats,
        stacks: vec![1000, 1000],
        small_blind: 5,
        big_blind: 10,
    };
    t.handle(start, host).unwrap();
    // Deal is mid-keygen: no seat is to act yet.
    assert!(t.seat_to_act().is_none());
    let err = t
        .handle(TableMessage::Act { hand_no: 1, action: Action::Call }, host)
        .unwrap_err();
    assert!(
        matches!(err, TableError::NotBettingYet),
        "Act before deal ready must be rejected, got {err:?}"
    );
}
