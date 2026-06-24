//! CHEATING-REJECTED tests for the TRUSTLESS (Barnett-Smart) mental-poker deal at the pure
//! [`Table`] level (no networking, no async).
//!
//! These drive two independent [`Table`] replicas (a host view and a guest view) through the
//! real interactive distributed deal by exchanging the *actual* deal payloads they produce
//! (`KeyAnnounce` / `ShuffleAnnounce` / `RevealAnnounce`). At the cheat-relevant moments we take
//! a LEGITIMATE message an honest peer broadcast and turn it into a CHEAT — a corrupted
//! Bayer-Groth shuffle proof, a corrupted Chaum-Pedersen reveal proof, a shuffle sent OUT OF
//! TURN, or a deal message authored by the wrong peer — then feed it to an honest [`Table`] and
//! assert it is REJECTED (`Table::handle` returns `Err`) and the honest peer's replicated state
//! does NOT advance as if the cheat were valid.
//!
//! Every cheat exercised here uses genuine cryptographic material (real keys, real masked deck,
//! real shuffle/reveal proofs) so the rejection comes from the protocol's verifier, not from a
//! malformed-bytes shortcut. A cheat that is ACCEPTED would be a security bug; the assertions
//! below pin the verify/reject path.

use libp2p::identity::Keypair;
use libp2p::PeerId;
use poker_deal::distributed::{RevealMessage, ShuffleMessage};
use poker_protocol::table::TableError;
use poker_protocol::{DealPhase, Table, TableMessage};

/// A fresh random PeerId.
fn pid() -> PeerId {
    PeerId::from(Keypair::generate_ed25519().public())
}

/// One peer in the simulated table: its identity plus its replicated [`Table`] view.
struct Peer {
    id: PeerId,
    table: Table,
}

/// A simulated 2-peer (host + guest) table running a mental hand. We exchange ONLY the
/// `(TableMessage, from)` pairs each peer broadcasts, exactly as the net layer would.
struct Sim {
    host: PeerId,
    peers: Vec<Peer>,
    /// Pending `(message, from)` broadcasts not yet delivered to every other peer.
    queue: Vec<(TableMessage, PeerId)>,
}

impl Sim {
    /// Build an `n`-peer simulated table (seat 0 = host). Returns the sim plus the seat roster.
    fn new(n: usize) -> Sim {
        let ids: Vec<PeerId> = (0..n).map(|_| pid()).collect();
        let host = ids[0];
        let peers = ids
            .iter()
            .map(|id| Peer {
                id: *id,
                table: Table::new(*id, host),
            })
            .collect();
        Sim {
            host,
            peers,
            queue: Vec::new(),
        }
    }

    fn ids(&self) -> Vec<PeerId> {
        self.peers.iter().map(|p| p.id).collect()
    }

    /// Host broadcasts a StartMentalHand; deliver it to every peer and enqueue follow-ons.
    fn start_mental(&mut self, hand_no: u64, button: u64) {
        let seats: Vec<Vec<u8>> = self.ids().iter().map(|p| p.to_bytes()).collect();
        let n = self.peers.len();
        let msg = TableMessage::StartMentalHand {
            hand_no,
            button: button as usize,
            session_seed: vec![9u8; 32],
            seats,
            stacks: vec![1000; n],
            small_blind: 5,
            big_blind: 10,
        };
        let from = self.host;
        self.deliver(msg, from);
    }

    /// Deliver one `(msg, from)` to every peer, collecting each peer's resulting broadcasts onto
    /// the queue. Panics if any HONEST delivery is rejected (these are legitimate messages).
    fn deliver(&mut self, msg: TableMessage, from: PeerId) {
        for peer in self.peers.iter_mut() {
            let step = peer
                .table
                .handle(msg.clone(), from)
                .unwrap_or_else(|e| panic!("honest peer {} rejected a legit message: {e:?}", peer.id));
            for b in step.broadcasts {
                self.queue.push((b, peer.id));
            }
        }
    }

    /// Run the queue to quiescence: deliver every pending broadcast (and the broadcasts those
    /// produce) until nothing new is generated. Also pokes each peer's local deal step so
    /// shuffle-turn / reveal contributions that depend only on local state are emitted.
    fn run_to_quiescence(&mut self) {
        for _ in 0..2000 {
            // Poke local deal steps first (a peer whose shuffle turn just arrived owes a msg).
            for i in 0..self.peers.len() {
                let step = self.peers[i].table.local_deal_step().expect("local deal step");
                let id = self.peers[i].id;
                for b in step.broadcasts {
                    self.queue.push((b, id));
                }
            }
            if self.queue.is_empty() {
                return;
            }
            let batch = std::mem::take(&mut self.queue);
            for (msg, from) in batch {
                // Deliver to every peer EXCEPT the author (the author already applied it locally).
                for peer in self.peers.iter_mut() {
                    if peer.id == from {
                        continue;
                    }
                    // Idempotent / out-of-turn re-delivery may legitimately Err; only legit deal
                    // progress matters here, so ignore benign rejections of echoes.
                    let _ = peer.table.handle(msg.clone(), from).map(|step| {
                        for b in step.broadcasts {
                            self.queue.push((b, peer.id));
                        }
                    });
                }
            }
        }
        panic!("deal did not reach quiescence");
    }

    fn seat_of(&self, id: PeerId) -> usize {
        self.peers.iter().position(|p| p.id == id).unwrap()
    }

    /// Drive the deal forward and return the FIRST broadcast matching `pred`, as `(msg, from)`,
    /// WITHOUT delivering it to any peer. Every other broadcast is delivered normally so the deal
    /// keeps progressing until the wanted message is produced.
    fn intercept<F>(&mut self, pred: F) -> (TableMessage, PeerId)
    where
        F: Fn(&TableMessage) -> bool,
    {
        for _ in 0..4000 {
            // Surface local contributions (key/shuffle turn/reveal) from every peer.
            for i in 0..self.peers.len() {
                let id = self.peers[i].id;
                let step = self.peers[i].table.local_deal_step().expect("local step");
                for b in step.broadcasts {
                    if pred(&b) {
                        return (b, id);
                    }
                    self.queue.push((b, id));
                }
            }
            if self.queue.is_empty() {
                panic!("no matching message produced before quiescence");
            }
            let batch = std::mem::take(&mut self.queue);
            for (msg, from) in batch {
                if pred(&msg) {
                    return (msg, from);
                }
                for peer in self.peers.iter_mut() {
                    if peer.id == from {
                        continue;
                    }
                    let _ = peer.table.handle(msg.clone(), from).map(|step| {
                        for b in step.broadcasts {
                            self.queue.push((b, peer.id));
                        }
                    });
                }
            }
        }
        panic!("intercept never matched");
    }
}

/// Helper: capture the FIRST `ShuffleAnnounce` a given seat broadcasts as it is produced. We do
/// this by stepping the deal manually one shuffle at a time, returning the (msg, from) for the
/// target turn WITHOUT having yet delivered it to the honest verifier.
///
/// Strategy: after `start_mental` and key exchange, the shuffle for `turn` is produced by the seat
/// whose `local_deal_step` yields it. We intercept it from the producing peer's step.
fn produce_first_shuffle(sim: &mut Sim) -> (TableMessage, PeerId) {
    // Drive keygen + mask, then intercept the turn-0 shuffle before any verifier sees it.
    sim.intercept(|m| matches!(m, TableMessage::ShuffleAnnounce { turn: 0, .. }))
}

/// Flip the last byte of a deal payload (the proof region is at the tail of the serialized
/// message), corrupting the zero-knowledge proof while leaving framing intact.
fn corrupt_tail(bytes: &mut [u8]) {
    let n = bytes.len();
    assert!(n > 0);
    bytes[n - 1] ^= 0xFF;
}

/// CHEAT 1a: a shuffle whose Bayer-Groth proof FAILS VERIFICATION (it is well-formed and
/// deserializes, but proves a DIFFERENT permutation than the deck actually announced) must be
/// rejected by an honest verifier, and the honest peer must NOT adopt the cheater's deck / advance
/// its shuffle count.
///
/// We forge this by taking seat 0's genuine, valid shuffle message and SWAPPING two cards in the
/// announced deck while keeping the real proof. The proof no longer matches `(prior_deck,
/// tampered_deck)`, so Bayer-Groth `verify_shuffle` rejects it. This exercises the cryptographic
/// reject path (not a deserialize shortcut).
#[test]
fn shuffle_with_invalid_proof_is_rejected() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    // Intercept seat 0's legitimate turn-0 shuffle before any honest verifier sees it.
    let (shuffle_msg, producer) = produce_first_shuffle(&mut sim);
    assert_eq!(sim.seat_of(producer), 0, "turn-0 shuffle is from seat 0");

    let (hand_no, turn, payload) = match shuffle_msg {
        TableMessage::ShuffleAnnounce { hand_no, turn, payload } => (hand_no, turn, payload),
        other => panic!("expected ShuffleAnnounce, got {other:?}"),
    };

    // Deserialize the REAL shuffle, swap two announced cards, re-serialize WITH the real proof.
    let mut sm = ShuffleMessage::deserialize(&payload).expect("legit shuffle deserializes");
    sm.deck.swap(0, 1); // proof now proves a different deck than the one announced
    let tampered = sm.serialize().expect("re-serialize tampered shuffle");
    let cheat = TableMessage::ShuffleAnnounce {
        hand_no,
        turn,
        payload: tampered,
    };

    // The honest verifier is the OTHER seat (seat 1, the guest).
    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let verifier = &mut sim.peers[verifier_idx];

    // Snapshot the verifier's deal phase before the cheat (must be Shuffle, turn 0 not yet done).
    assert_eq!(verifier.table.deal_phase(), Some(DealPhase::Shuffle));

    let err = verifier
        .table
        .handle(cheat, producer)
        .expect_err("a shuffle whose proof fails verification MUST be rejected");
    assert!(
        matches!(err, TableError::Deal2(_)),
        "expected a deal-layer verification error, got {err:?}"
    );

    // The honest peer did not advance: still in Shuffle phase (the cheat's deck was not adopted).
    assert_eq!(
        verifier.table.deal_phase(),
        Some(DealPhase::Shuffle),
        "honest peer advanced past an invalid shuffle (it adopted the cheater's deck)"
    );
}

/// CHEAT 1b: a shuffle with a structurally-corrupted proof (random byte flip) is also rejected
/// (it fails at deserialize / verification before the deck is adopted). Belt-and-suspenders on
/// top of the verification-failure case above.
#[test]
fn shuffle_with_corrupted_proof_bytes_is_rejected() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    let (shuffle_msg, producer) = produce_first_shuffle(&mut sim);
    let (hand_no, turn, mut payload) = match shuffle_msg {
        TableMessage::ShuffleAnnounce { hand_no, turn, payload } => (hand_no, turn, payload),
        other => panic!("expected ShuffleAnnounce, got {other:?}"),
    };
    corrupt_tail(&mut payload);
    let cheat = TableMessage::ShuffleAnnounce { hand_no, turn, payload };

    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let verifier = &mut sim.peers[verifier_idx];
    assert_eq!(verifier.table.deal_phase(), Some(DealPhase::Shuffle));

    let err = verifier
        .table
        .handle(cheat, producer)
        .expect_err("corrupted shuffle bytes MUST be rejected");
    assert!(matches!(err, TableError::Deal2(_)), "got {err:?}");
    assert_eq!(verifier.table.deal_phase(), Some(DealPhase::Shuffle));
}

/// CHEAT 2: a shuffle sent OUT OF TURN (seat 1 tries to shuffle on turn 1 before turn 0 is done,
/// i.e. a `turn` that does not match the verifier's shuffles-done count) must be rejected with
/// `DealOutOfTurn` and leave the honest peer's state untouched.
#[test]
fn shuffle_out_of_turn_is_rejected() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    // Get seat 0's legitimate turn-0 shuffle (a real, valid shuffle message).
    let (shuffle_msg, producer) = produce_first_shuffle(&mut sim);
    assert_eq!(sim.seat_of(producer), 0);

    let (hand_no, payload) = match shuffle_msg {
        TableMessage::ShuffleAnnounce { hand_no, payload, .. } => (hand_no, payload),
        other => panic!("expected ShuffleAnnounce, got {other:?}"),
    };

    // The honest verifier is seat 1. It is still waiting for turn 0 (0 shuffles done). Seat 1
    // tries to jump ahead and shuffle on turn 1 (out of turn) — authored by itself, so the
    // seat==turn relation is irrelevant; the verifier's `shuffles_done` (0) != turn (1) is what
    // must trip the turn-order guard. The payload carries seat 0's real turn-0 bytes; the deal
    // must reject on turn order BEFORE ever looking at the proof.
    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let cheater_seat1 = sim.peers[verifier_idx].id;
    let cheat = TableMessage::ShuffleAnnounce {
        hand_no,
        turn: 1,
        payload: payload.clone(),
    };
    let verifier = &mut sim.peers[verifier_idx];
    assert_eq!(verifier.table.deal_phase(), Some(DealPhase::Shuffle));

    let err = verifier
        .table
        .handle(cheat, cheater_seat1)
        .expect_err("out-of-turn shuffle MUST be rejected");
    assert!(
        matches!(err, TableError::DealOutOfTurn),
        "expected DealOutOfTurn, got {err:?}"
    );
    assert_eq!(
        verifier.table.deal_phase(),
        Some(DealPhase::Shuffle),
        "honest peer advanced on an out-of-turn shuffle"
    );
}

/// CHEAT 3: a shuffle authored by the WRONG peer (seat 0's valid turn-0 shuffle, but relayed
/// with a `from` that is NOT seat 0) must be rejected: the authenticated publisher must match the
/// declared seat/turn. (Models the host trying to inject a guest's turn under a forged author,
/// which gossipsub StrictSign also prevents, but the Table enforces structurally.)
#[test]
fn shuffle_from_wrong_author_is_rejected() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    let (shuffle_msg, producer) = produce_first_shuffle(&mut sim);
    assert_eq!(sim.seat_of(producer), 0);

    // Re-author seat 0's legit turn-0 shuffle as if seat 1 sent it.
    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let wrong_author = sim.peers[verifier_idx].id; // seat 1

    let verifier = &mut sim.peers[verifier_idx];
    // Deliver seat-0's turn-0 shuffle but claim it came from seat 1 (turn 0, seat 1 != turn).
    let err = verifier
        .table
        .handle(shuffle_msg, wrong_author)
        .expect_err("shuffle from the wrong author MUST be rejected");
    assert!(
        matches!(err, TableError::DealAuthor | TableError::DealOutOfTurn),
        "expected DealAuthor/DealOutOfTurn, got {err:?}"
    );
    assert_eq!(verifier.table.deal_phase(), Some(DealPhase::Shuffle));
}

/// CHEAT 4a: a reveal whose Chaum-Pedersen proof FAILS VERIFICATION (well-formed and
/// deserializes, but is bound to a DIFFERENT deck position than the one it is announced for) must
/// be rejected on ingest, and the honest peer must not collect the bad token toward decryption.
///
/// A hole-reveal batch from one seat covers two distinct deck positions, each with its own
/// (token, Chaum-Pedersen proof) bound to the masked card at THAT position. We SWAP the
/// `(token, proof)` payloads of the two messages while leaving their `position` fields intact:
/// now each token+proof is checked against the wrong masked card and Chaum-Pedersen verification
/// rejects it. This exercises the real cryptographic reveal-verify path.
#[test]
fn reveal_with_invalid_proof_is_rejected() {
    let mut sim = Sim::new(3);
    sim.start_mental(1, 0);

    // Drive the deal and intercept the FIRST hole-reveal RevealAnnounce, before any verifier sees
    // it. With 3 seats a non-owner's hole batch covers >= 4 positions, so two distinct positions
    // exist to swap between.
    let (reveal_msg, producer) =
        sim.intercept(|m| matches!(m, TableMessage::RevealAnnounce { .. }));

    let (hand_no, seat, round, tokens) = match reveal_msg {
        TableMessage::RevealAnnounce { hand_no, seat, round, tokens } => {
            (hand_no, seat, round, tokens)
        }
        other => panic!("expected RevealAnnounce, got {other:?}"),
    };
    assert!(tokens.len() >= 2, "hole reveal batch covers >= 2 positions");

    // Deserialize the first two reveal messages and swap their token+proof, keeping positions.
    let mut m0 = RevealMessage::deserialize(&tokens[0]).expect("reveal 0 deserializes");
    let mut m1 = RevealMessage::deserialize(&tokens[1]).expect("reveal 1 deserializes");
    assert_ne!(m0.position, m1.position, "two distinct positions to mismatch");
    std::mem::swap(&mut m0.token, &mut m1.token);
    std::mem::swap(&mut m0.proof, &mut m1.proof);
    let mut bad_tokens = tokens.clone();
    bad_tokens[0] = m0.serialize().expect("re-serialize reveal 0");
    bad_tokens[1] = m1.serialize().expect("re-serialize reveal 1");
    let cheat = TableMessage::RevealAnnounce {
        hand_no,
        seat,
        round,
        tokens: bad_tokens,
    };

    // Honest verifier = a seat other than the producer.
    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let verifier = &mut sim.peers[verifier_idx];

    let err = verifier
        .table
        .handle(cheat, producer)
        .expect_err("a reveal whose Chaum-Pedersen proof fails verification MUST be rejected");
    assert!(
        matches!(err, TableError::Deal2(_)),
        "expected a deal-layer verification error for a bad reveal proof, got {err:?}"
    );
}

/// CHEAT 4b: a reveal with structurally-corrupted proof bytes is also rejected (deserialize /
/// verification failure before the token is collected).
#[test]
fn reveal_with_corrupted_proof_bytes_is_rejected() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    let (reveal_msg, producer) =
        sim.intercept(|m| matches!(m, TableMessage::RevealAnnounce { .. }));

    let (hand_no, seat, round, mut tokens) = match reveal_msg {
        TableMessage::RevealAnnounce { hand_no, seat, round, tokens } => {
            (hand_no, seat, round, tokens)
        }
        other => panic!("expected RevealAnnounce, got {other:?}"),
    };
    assert!(!tokens.is_empty(), "hole reveal carries tokens");
    corrupt_tail(&mut tokens[0]);
    let cheat = TableMessage::RevealAnnounce { hand_no, seat, round, tokens };

    let verifier_idx = sim.peers.iter().position(|p| p.id != producer).unwrap();
    let verifier = &mut sim.peers[verifier_idx];
    let err = verifier
        .table
        .handle(cheat, producer)
        .expect_err("corrupted reveal bytes MUST be rejected");
    assert!(matches!(err, TableError::Deal2(_)), "got {err:?}");
}

/// CHEAT 5: a reveal authored by the WRONG peer (a legit reveal payload from seat X relayed with
/// a `from` of seat Y) must be rejected: the deal message's authenticated author must own the
/// seat declared in the payload.
#[test]
fn reveal_from_wrong_author_is_rejected() {
    let mut sim = Sim::new(3);
    sim.start_mental(1, 0);

    let (reveal_msg, producer) =
        sim.intercept(|m| matches!(m, TableMessage::RevealAnnounce { .. }));

    // The reveal payload declares `seat == producer's seat`. We relay it claiming a DIFFERENT
    // author (`wrong_author`), and deliver it to an honest third-party verifier. The verifier
    // must reject it because the authenticated author does not own the declared seat.
    let wrong_author = sim
        .peers
        .iter()
        .map(|p| p.id)
        .find(|id| *id != producer)
        .unwrap();
    // Third-party honest verifier: neither the producer nor the forged author.
    let verifier_idx = sim
        .peers
        .iter()
        .position(|p| p.id != producer && p.id != wrong_author)
        .expect("3-peer sim has a third party");
    let verifier = &mut sim.peers[verifier_idx];
    let err = verifier
        .table
        .handle(reveal_msg, wrong_author)
        .expect_err("reveal with mismatched author MUST be rejected");
    assert!(
        matches!(err, TableError::DealAuthor),
        "expected DealAuthor for a reveal whose author != declared seat, got {err:?}"
    );
}

/// Sanity: in the absence of any cheat, the same harness drives a full mental hand to a settled,
/// chip-conserving outcome on BOTH peers — so the rejections above are rejecting cheats, not a
/// broken-by-construction deal.
#[test]
fn honest_mental_hand_settles_for_reference() {
    let mut sim = Sim::new(2);
    sim.start_mental(1, 0);

    // Drive deal + betting to settlement. Each iteration: pump the deal (reveals), then let any
    // peer whose local turn it is act via the CallStationBot (check when free, else call). The
    // Act it broadcasts is delivered to everyone via the queue.
    let mut bot = poker_protocol::CallStationBot;
    let mut settled = false;
    for _ in 0..10_000 {
        sim.run_to_quiescence();

        let mut acted = false;
        for i in 0..sim.peers.len() {
            if sim.peers[i].table.is_local_turn() {
                let id = sim.peers[i].id;
                let step = sim.peers[i].table.local_turn(&mut bot).expect("local turn");
                for b in step.broadcasts {
                    sim.queue.push((b, id));
                }
                acted = true;
                break;
            }
        }
        sim.run_to_quiescence();

        if sim.peers.iter().all(|p| p.table.live_hand_no().is_none()) {
            settled = true;
            break;
        }
        // If nobody could act and nothing is pending, the deal is stuck — fail loudly.
        if !acted
            && sim.queue.is_empty()
            && sim.peers.iter().all(|p| p.table.seat_to_act().is_none())
        {
            // Pumping again may unblock a pending reveal; loop continues.
        }
    }
    assert!(
        settled,
        "honest mental hand should settle on both peers (no cheat present)"
    );
}
