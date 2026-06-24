//! The PURE, deterministic, replicated table state machine.
//!
//! Every peer at the table runs an identical [`Table`]. It contains NO networking and NO
//! async: it is driven entirely by [`TableMessage`]s tagged with their authenticated author
//! ([`Table::handle`]) plus a local "is it my turn?" poke ([`Table::local_turn`]). Given the
//! same ordered sequence of `(message, from)` pairs, every peer reaches byte-identical COMMON
//! state — the same winners, the same per-seat deltas, the same board — which is what makes the
//! table trustless to relay.
//!
//! # Two deal modes
//! * **Placeholder** ([`TableMessage::StartHand`]). The HOST chooses a `seed` that drives
//!   [`poker_deal::placeholder_shuffled_deck`]; every peer derives the SAME deck and deals the
//!   SAME hole/community cards. INSECURE (anyone who sees the seed sees every card) — retained
//!   for fast tests / demos behind a flag.
//! * **Trustless / mental** ([`TableMessage::StartMentalHand`]). There is NO public deck.
//!   The cards are dealt by the interactive Barnett–Smart distributed protocol
//!   ([`poker_deal::distributed`], orchestrated by [`crate::mental::MentalDeal`]): keygen ->
//!   deterministic mask -> seat-order shuffle -> threshold (l-of-l) reveal. The HOST still fixes
//!   the betting context (button / blinds / seats / stacks) via `StartMentalHand`; everything
//!   that hides the cards comes from each peer's secret key + shuffle randomness, never the host.
//!
//! # Common vs local state
//! For a mental hand the [`Table`] holds two kinds of state:
//! * **COMMON / replicated** — identical on every peer: the aggregate key, the masked +
//!   shuffled deck, the count of accepted shuffles, the decrypted community board, the betting
//!   state, the pots, and the final outcome. All of it is derived from *verified* broadcast
//!   messages, so every peer computes the same bytes.
//! * **LOCAL / private** — this peer's secret key (inside its [`crate::mental::MentalDeal`]'s
//!   [`poker_deal::distributed::PeerDeal`], never serialized) and its own decrypted hole cards.
//!   No other peer can derive them.
//!
//! # Round sequencing (mental)
//! `Keygen -> deterministic mask -> shuffle (seat order) -> hole reveal -> preflop betting ->
//! flop reveal -> flop betting -> turn reveal -> turn betting -> river reveal -> river betting
//! -> showdown reveal -> distribute`. The [`Table`] drives the deal forward after every message
//! it applies: it collects KeyAnnounce until the aggregate forms, masks deterministically,
//! enforces shuffle TURN ORDER, runs the hole reveal, gates preflop betting until the LOCAL peer
//! has its hole cards, triggers a community reveal at each street boundary (gating betting on
//! that street until the board is decrypted), and triggers showdown reveals for the non-folded
//! seats before distributing. Every proof is verified; anything out of turn / from the wrong
//! author / with a bad proof is rejected (never applied), preserving M4 anti-cheat.
//!
//! # Anti-cheat
//! [`Table::handle`] checks `from == roster[to_act]` for every [`TableMessage::Act`], and for
//! every deal message checks the authenticated publisher's seat against the declared seat and
//! the protocol's verifier. The host cannot forge this because gossipsub `StrictSign`
//! authenticates the publisher. [`TableMessage::StartHand`] / [`TableMessage::StartMentalHand`]
//! are only accepted from the configured host PeerId.

use crate::mental::{DealEffect, DealPhase, MentalDeal};
use crate::{Action, RevealRound, TableMessage};
use libp2p::PeerId;
use poker_deal::placeholder_shuffled_deck;
use poker_game::{
    compute_pots, deal_community_street, deal_hole, distribute, hole_card_count, BettingState,
    Card, Chips, PotAward, Street,
};
use thiserror::Error;

/// Errors from applying a message to the [`Table`]. Library logic never panics on these.
#[derive(Debug, Error)]
pub enum TableError {
    #[error("only the host may send StartHand (got it from {0})")]
    NotHost(PeerId),
    #[error("StartHand seat list does not match stacks (seats {seats}, stacks {stacks})")]
    SeatStackMismatch { seats: usize, stacks: usize },
    #[error("StartHand seat bytes are not a valid PeerId at index {0}")]
    BadSeatPeerId(usize),
    #[error("StartHand needs 2..=9 seats, got {0}")]
    BadSeatCount(usize),
    #[error("Act for hand {got} but the live hand is {expected:?}")]
    WrongHand { got: u64, expected: Option<u64> },
    #[error("no hand is in progress")]
    NoHandInProgress,
    #[error("Act from {from} but seat {seat} ({owner:?}) is to act")]
    ActOutOfTurn {
        from: PeerId,
        seat: usize,
        owner: PeerId,
    },
    #[error("betting engine rejected the action: {0}")]
    Betting(#[from] poker_game::BettingError),
    #[error("deck exhausted dealing community cards: {0}")]
    Deal(#[from] poker_game::HandError),
    // --- trustless (mental-poker) deal errors -------------------------------
    #[error("mental-poker deal layer error: {0}")]
    Deal2(#[from] poker_deal::mental::DealError),
    #[error("deal message arrived out of turn / wrong phase")]
    DealOutOfTurn,
    #[error("deal message author does not match its declared seat")]
    DealAuthor,
    #[error("a deal message arrived but no mental hand is in progress")]
    NoMentalHand,
    #[error("a betting Act arrived but the live hand is not in the betting phase yet")]
    NotBettingYet,
    #[error("StartMentalHand session_seed must be 32 bytes, got {0}")]
    BadSessionSeed(usize),
}

/// A completed hand's outcome, computed identically by every peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandOutcome {
    pub hand_no: u64,
    pub button: usize,
    /// Final community board actually dealt (0..=5 cards).
    pub community: Vec<Card>,
    /// Per-seat net chip delta for the hand (winnings minus contributions).
    pub deltas: Vec<i64>,
    /// Per-seat stacks after the hand (the next hand starts from these).
    pub final_stacks: Vec<Chips>,
    /// Per-pot awards (for display / auditing).
    pub awards: Vec<PotAward>,
}

/// Side-effect notifications emitted by [`Table::handle`] / [`Table::local_turn`] so the
/// async driver can react without re-deriving table state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableEvent {
    /// A new hand started; `hand_no` is live.
    HandStarted(u64),
    /// A street's board cards were dealt / revealed.
    StreetDealt { hand_no: u64, street: Street },
    /// The mental deal advanced to a new phase (keygen -> shuffle -> ready). Diagnostics.
    DealPhase { hand_no: u64, phase: DealPhase },
    /// The LOCAL peer has decrypted its own hole cards for the live mental hand.
    LocalHoleReady { hand_no: u64 },
    /// The hand ended; carries the outcome.
    HandEnded(HandOutcome),
}

/// What [`Table::handle`] / [`Table::local_turn`] return: messages THIS peer should now
/// broadcast, plus local events the driver may act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Messages this peer should broadcast to the table (betting `Act`s AND deal payloads:
    /// [`TableMessage::KeyAnnounce`] / [`TableMessage::ShuffleAnnounce`] /
    /// [`TableMessage::RevealAnnounce`]).
    pub broadcasts: Vec<TableMessage>,
    /// The deal contributions the LOCAL peer produced this step (already turned into the
    /// matching `broadcasts` above). Exposed so a driver/UI can observe deal progress; the
    /// driver does NOT need to act on these (the broadcasts are already populated).
    pub deal_effects: Vec<DealEffect>,
    /// Local notifications (hand started/ended, streets dealt, deal phase, hole ready).
    pub events: Vec<TableEvent>,
}

impl Step {
    fn new() -> Self {
        Step::default()
    }
}

/// How a live hand's cards are dealt.
enum Deal {
    /// The INSECURE placeholder: a fully-known deck derived from the host `seed`.
    Placeholder {
        seed: u64,
        deck: Vec<Card>,
        /// Deck index of the next undealt community card.
        deck_idx: usize,
        /// All seats' hole cards (known to everyone — placeholder has no privacy).
        hole: Vec<[Card; 2]>,
    },
    /// The trustless Barnett–Smart deal. Holds this peer's private replica + COMMON deal state.
    Mental(Box<MentalDeal>),
}

/// The live hand's replicated runtime state.
struct LiveHand {
    hand_no: u64,
    button: usize,
    deal: Deal,
    community: Vec<Card>,
    betting: BettingState,
    /// Starting stacks at the top of the hand (for delta computation).
    starting: Vec<Chips>,
    /// Mental hands defer settlement until the required reveals (run-out board + non-folded
    /// showdown holes) are all in. Once betting is complete the Table sets this; each pump
    /// retries the settle until the reveals are available. Always `false` for placeholder hands.
    pending_settle: bool,
}

impl LiveHand {
    fn is_mental(&self) -> bool {
        matches!(self.deal, Deal::Mental(_))
    }
    fn mental(&self) -> Option<&MentalDeal> {
        match &self.deal {
            Deal::Mental(m) => Some(m),
            _ => None,
        }
    }
    fn mental_mut(&mut self) -> Option<&mut MentalDeal> {
        match &mut self.deal {
            Deal::Mental(m) => Some(m),
            _ => None,
        }
    }
}

/// The replicated table. Construct with [`Table::new`]; feed it messages with
/// [`Table::handle`] and poke it for local turns with [`Table::local_turn`].
pub struct Table {
    /// This peer's own PeerId (so it knows which seat it controls).
    local: PeerId,
    /// The host's PeerId; only the host may send [`TableMessage::StartHand`].
    host: PeerId,
    /// Current roster in seat order (seat `i` -> PeerId). Set per-hand from `StartHand`.
    roster: Vec<PeerId>,
    /// The live hand, if one is in progress.
    live: Option<LiveHand>,
    /// The highest hand number this peer has already settled. Used to drop retransmitted
    /// `StartHand`s for a hand that already completed.
    last_completed: Option<u64>,
}

impl Table {
    /// Create a fresh table view for this peer. `host` is the authoritative host PeerId.
    pub fn new(local: PeerId, host: PeerId) -> Self {
        Table {
            local,
            host,
            roster: Vec::new(),
            live: None,
            last_completed: None,
        }
    }

    /// This peer's PeerId.
    pub fn local_peer(&self) -> PeerId {
        self.local
    }

    /// The host PeerId.
    pub fn host_peer(&self) -> PeerId {
        self.host
    }

    /// True if this peer is the host.
    pub fn is_host(&self) -> bool {
        self.local == self.host
    }

    /// The seat index controlled by this peer in the live hand, if any.
    pub fn local_seat(&self) -> Option<usize> {
        self.roster.iter().position(|p| *p == self.local)
    }

    /// The current roster (seat order).
    pub fn roster(&self) -> &[PeerId] {
        &self.roster
    }

    /// The live hand number, if a hand is in progress.
    pub fn live_hand_no(&self) -> Option<u64> {
        self.live.as_ref().map(|h| h.hand_no)
    }

    /// Read-only view of the live betting state, if any.
    pub fn betting(&self) -> Option<&BettingState> {
        self.live.as_ref().map(|h| &h.betting)
    }

    /// The current community board for the live hand.
    pub fn community(&self) -> &[Card] {
        self.live
            .as_ref()
            .map(|h| h.community.as_slice())
            .unwrap_or(&[])
    }

    /// This peer's hole cards for the live hand, if it is seated AND they are known. For a
    /// placeholder hand these are known immediately; for a mental hand they are known only once
    /// the hole reveal completes and the local peer decrypts them locally (LOCAL/private).
    pub fn local_hole(&self) -> Option<[Card; 2]> {
        let live = self.live.as_ref()?;
        match &live.deal {
            Deal::Placeholder { hole, .. } => {
                let seat = self.local_seat()?;
                hole.get(seat).copied()
            }
            Deal::Mental(m) => m.local_hole(),
        }
    }

    /// True if the live hand is a trustless (mental) hand.
    pub fn is_mental_hand(&self) -> bool {
        self.live.as_ref().map(|h| h.is_mental()).unwrap_or(false)
    }

    /// The mental deal phase for the live hand, if it is a mental hand.
    pub fn deal_phase(&self) -> Option<DealPhase> {
        self.live.as_ref().and_then(|h| h.mental()).map(|m| m.phase())
    }

    /// Diagnostics: per-seat showdown token report for the live mental hand, if any.
    pub fn debug_deal_missing(&self) -> Option<String> {
        self.live
            .as_ref()
            .and_then(|h| h.mental())
            .map(|m| m.debug_missing())
    }

    /// The seat currently to act in the live hand, if any AND betting is currently open. For a
    /// mental hand this is `None` while the deal/reveal for the current street is still pending.
    pub fn seat_to_act(&self) -> Option<usize> {
        let live = self.live.as_ref()?;
        if !self.betting_open(live) {
            return None;
        }
        live.betting.to_act()
    }

    /// True when it is the LOCAL peer's turn to act in the live hand.
    pub fn is_local_turn(&self) -> bool {
        match (self.seat_to_act(), self.local_seat()) {
            (Some(a), Some(s)) => a == s,
            _ => false,
        }
    }

    /// Whether betting may currently proceed for `live`. For placeholder hands betting is always
    /// open. For mental hands, betting on the current street is gated until: the deal is `Ready`,
    /// the LOCAL peer has decrypted its own hole cards, and the current street's community board
    /// has been revealed.
    fn betting_open(&self, live: &LiveHand) -> bool {
        match &live.deal {
            Deal::Placeholder { .. } => true,
            Deal::Mental(m) => {
                if !m.ready_for_betting() {
                    return false;
                }
                community_revealed_for_street(m, live.betting.street)
            }
        }
    }

    // ====================================================================
    // Message handling
    // ====================================================================

    /// Apply an inbound [`TableMessage`] authored by `from`, mutating the table
    /// deterministically and returning the messages this peer should now broadcast plus any
    /// local events.
    pub fn handle(&mut self, msg: TableMessage, from: PeerId) -> Result<Step, TableError> {
        match msg {
            TableMessage::JoinTable { .. } => Ok(Step::new()),
            TableMessage::DealPayload { .. } => Ok(Step::new()),
            TableMessage::HandComplete { .. } => Ok(Step::new()),
            TableMessage::StartHand {
                hand_no,
                button,
                seed,
                seats,
                stacks,
                small_blind,
                big_blind,
            } => self.apply_start_hand(
                from, hand_no, button, seed, seats, stacks, small_blind, big_blind,
            ),
            TableMessage::StartMentalHand {
                hand_no,
                button,
                session_seed,
                seats,
                stacks,
                small_blind,
                big_blind,
            } => self.apply_start_mental_hand(
                from,
                hand_no,
                button,
                session_seed,
                seats,
                stacks,
                small_blind,
                big_blind,
            ),
            TableMessage::Act { hand_no, action } => self.apply_act(from, hand_no, action),
            TableMessage::KeyAnnounce {
                hand_no,
                seat,
                payload,
            } => self.apply_key_announce(from, hand_no, seat, &payload),
            TableMessage::ShuffleAnnounce {
                hand_no,
                turn,
                payload,
            } => self.apply_shuffle_announce(from, hand_no, turn, &payload),
            TableMessage::RevealAnnounce {
                hand_no,
                seat,
                round,
                tokens,
            } => self.apply_reveal_announce(from, hand_no, seat, round, &tokens),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_start_hand(
        &mut self,
        from: PeerId,
        hand_no: u64,
        button: usize,
        seed: u64,
        seats: Vec<Vec<u8>>,
        stacks: Vec<u64>,
        small_blind: u64,
        big_blind: u64,
    ) -> Result<Step, TableError> {
        if from != self.host {
            return Err(TableError::NotHost(from));
        }
        // Idempotent re-delivery of the live placeholder hand (same seed).
        if let Some(live) = self.live.as_ref() {
            if live.hand_no == hand_no {
                if let Deal::Placeholder { seed: s, .. } = &live.deal {
                    if *s == seed {
                        return Ok(Step::new());
                    }
                }
            }
        }
        if let Some(done) = self.last_completed {
            if hand_no <= done {
                return Ok(Step::new());
            }
        }
        let (roster, stacks) = self.validate_start(seats, stacks)?;
        let n = roster.len();

        let deck = placeholder_shuffled_deck(seed);
        let hole = deal_hole(&deck, n, button)?;
        let deck_idx = hole_card_count(n);
        let betting = BettingState::new(stacks.clone(), button, small_blind, big_blind)?;

        self.roster = roster;
        self.live = Some(LiveHand {
            hand_no,
            button,
            deal: Deal::Placeholder {
                seed,
                deck,
                deck_idx,
                hole,
            },
            community: Vec::with_capacity(5),
            betting,
            starting: stacks,
            pending_settle: false,
        });

        let mut step = Step::new();
        step.events.push(TableEvent::HandStarted(hand_no));
        Ok(step)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_start_mental_hand(
        &mut self,
        from: PeerId,
        hand_no: u64,
        button: usize,
        session_seed: Vec<u8>,
        seats: Vec<Vec<u8>>,
        stacks: Vec<u64>,
        small_blind: u64,
        big_blind: u64,
    ) -> Result<Step, TableError> {
        if from != self.host {
            return Err(TableError::NotHost(from));
        }
        // Idempotent re-delivery: the same mental hand is already live.
        if let Some(live) = self.live.as_ref() {
            if live.hand_no == hand_no && live.is_mental() {
                return Ok(Step::new());
            }
        }
        if let Some(done) = self.last_completed {
            if hand_no <= done {
                return Ok(Step::new());
            }
        }
        if session_seed.len() != 32 {
            return Err(TableError::BadSessionSeed(session_seed.len()));
        }
        let (roster, stacks) = self.validate_start(seats, stacks)?;
        let n = roster.len();
        let seat = roster.iter().position(|p| *p == self.local);

        let mut session = [0u8; 32];
        session.copy_from_slice(&session_seed);
        // If this peer is not seated it still tracks COMMON state, but it cannot run the deal
        // (it has no seat / key). MVP: only seated peers participate in a mental hand.
        let seat = seat.ok_or(TableError::NoMentalHand)?;

        let mental = MentalDeal::new(session, seat, n, button)?;
        let betting = BettingState::new(stacks.clone(), button, small_blind, big_blind)?;

        self.roster = roster;
        self.live = Some(LiveHand {
            hand_no,
            button,
            deal: Deal::Mental(Box::new(mental)),
            community: Vec::with_capacity(5),
            betting,
            starting: stacks,
            pending_settle: false,
        });

        let mut step = Step::new();
        step.events.push(TableEvent::HandStarted(hand_no));
        // Kick off the deal: the local peer announces its key (and produces any follow-on
        // effects already unblocked).
        self.pump_mental(&mut step)?;
        Ok(step)
    }

    /// Shared validation for both StartHand variants: roster from seat bytes + stack length.
    fn validate_start(
        &self,
        seats: Vec<Vec<u8>>,
        stacks: Vec<u64>,
    ) -> Result<(Vec<PeerId>, Vec<u64>), TableError> {
        if seats.len() != stacks.len() {
            return Err(TableError::SeatStackMismatch {
                seats: seats.len(),
                stacks: stacks.len(),
            });
        }
        let n = seats.len();
        if !(2..=9).contains(&n) {
            return Err(TableError::BadSeatCount(n));
        }
        let mut roster = Vec::with_capacity(n);
        for (i, bytes) in seats.iter().enumerate() {
            let pid = PeerId::from_bytes(bytes).map_err(|_| TableError::BadSeatPeerId(i))?;
            roster.push(pid);
        }
        Ok((roster, stacks))
    }

    fn apply_act(
        &mut self,
        from: PeerId,
        hand_no: u64,
        action: Action,
    ) -> Result<Step, TableError> {
        // Validate phase + turn before mutating.
        {
            let live = self.live.as_ref().ok_or(TableError::NoHandInProgress)?;
            if hand_no != live.hand_no {
                return Err(TableError::WrongHand {
                    got: hand_no,
                    expected: Some(live.hand_no),
                });
            }
            if !self.betting_open(live) {
                // Mental hand whose deal/reveal for this street is still pending.
                return Err(TableError::NotBettingYet);
            }
        }
        let live = self.live.as_mut().ok_or(TableError::NoHandInProgress)?;
        let seat = live.betting.to_act().ok_or(TableError::NoHandInProgress)?;
        let owner = self.roster[seat];
        // ANTI-CHEAT: the authenticated publisher must own the seat that is to act.
        if from != owner {
            return Err(TableError::ActOutOfTurn { from, seat, owner });
        }
        live.betting.apply(seat, action.to_game())?;

        let mut step = Step::new();
        self.advance_streets_and_settle(&mut step)?;
        Ok(step)
    }

    // ====================================================================
    // Deal message handling (mental)
    // ====================================================================

    fn deal_seat_of(&self, from: PeerId) -> Option<usize> {
        self.roster.iter().position(|p| *p == from)
    }

    fn check_mental_msg(&self, from: PeerId, hand_no: u64, seat: usize) -> Result<(), TableError> {
        let live = self.live.as_ref().ok_or(TableError::NoMentalHand)?;
        if !live.is_mental() {
            return Err(TableError::NoMentalHand);
        }
        if hand_no != live.hand_no {
            return Err(TableError::WrongHand {
                got: hand_no,
                expected: Some(live.hand_no),
            });
        }
        // ANTI-CHEAT: the authenticated publisher must own the seat it claims in the payload.
        // (The ingest step additionally binds each contribution to its seat's registered key via
        // the proofs, but this gossipsub-`from` check is what prevents a seated peer from squatting
        // another seat during keygen, so it is retained.)
        match self.deal_seat_of(from) {
            Some(s) if s == seat => Ok(()),
            _ => Err(TableError::DealAuthor),
        }
    }

    fn apply_key_announce(
        &mut self,
        from: PeerId,
        hand_no: u64,
        seat: usize,
        payload: &[u8],
    ) -> Result<Step, TableError> {
        self.check_mental_msg(from, hand_no, seat)?;
        let live = self.live.as_mut().ok_or(TableError::NoMentalHand)?;
        live.mental_mut()
            .ok_or(TableError::NoMentalHand)?
            .ingest_key(seat, payload)?;
        let mut step = Step::new();
        self.pump_mental(&mut step)?;
        Ok(step)
    }

    fn apply_shuffle_announce(
        &mut self,
        from: PeerId,
        hand_no: u64,
        turn: usize,
        payload: &[u8],
    ) -> Result<Step, TableError> {
        // The shuffle's seat IS its turn (seat-order shuffle), so author check uses `turn`.
        self.check_mental_msg(from, hand_no, turn)?;
        let live = self.live.as_mut().ok_or(TableError::NoMentalHand)?;
        live.mental_mut()
            .ok_or(TableError::NoMentalHand)?
            .ingest_shuffle(turn, turn, payload)?;
        let mut step = Step::new();
        self.pump_mental(&mut step)?;
        Ok(step)
    }

    fn apply_reveal_announce(
        &mut self,
        from: PeerId,
        hand_no: u64,
        seat: usize,
        round: RevealRound,
        tokens: &[Vec<u8>],
    ) -> Result<Step, TableError> {
        self.check_mental_msg(from, hand_no, seat)?;
        let live = self.live.as_mut().ok_or(TableError::NoMentalHand)?;
        live.mental_mut()
            .ok_or(TableError::NoMentalHand)?
            .ingest_reveal(seat, round, tokens)?;
        let mut step = Step::new();
        self.pump_mental(&mut step)?;
        Ok(step)
    }

    // ====================================================================
    // Mental deal driver: emit local contributions + advance phase.
    // ====================================================================

    /// Advance the mental deal as far as the current COMMON state allows: emit any pending local
    /// contribution (key / shuffle / reveal) as a broadcast, surface phase + hole-ready events,
    /// trigger street-boundary community reveals, drive showdown reveals, and retry a deferred
    /// settle. Called after every applied message; idempotent (re-emitting a contribution the
    /// local peer already announced is suppressed, and peers reject duplicates anyway).
    fn pump_mental(&mut self, step: &mut Step) -> Result<(), TableError> {
        if self.live.as_ref().map(|h| h.is_mental()) != Some(true) {
            return Ok(());
        }

        // Loop: producing one contribution can immediately unblock the next (e.g. our key
        // completes the aggregate, which immediately makes it our shuffle turn).
        loop {
            let before = (step.broadcasts.len(), step.events.len());

            self.emit_phase_events(step);
            self.emit_pending_deal_effect(step)?;
            self.drive_community_and_betting(step)?;
            self.drive_showdown_and_settle(step)?;

            // Stop once a pass produced nothing new.
            if (step.broadcasts.len(), step.events.len()) == before {
                break;
            }
            if self.live.is_none() {
                break; // settled
            }
        }
        Ok(())
    }

    /// Surface phase / hole-ready events once each (deduplicated by tracking last-seen in the
    /// MentalDeal-derived booleans we read here vs. what we have already emitted is the driver's
    /// concern; we emit on transition by comparing to recorded flags on the live hand).
    fn emit_phase_events(&mut self, step: &mut Step) {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return,
        };
        let hand_no = live.hand_no;
        let (phase, hole_ready) = match live.mental() {
            Some(m) => (m.phase(), m.local_hole().is_some()),
            None => return,
        };
        // Track emitted phase/hole via the betting-independent fields on MentalDeal is awkward;
        // instead we use the Step contents are recomputed each pump. To avoid duplicate events
        // we record the last emitted state on the MentalDeal itself.
        if let Some(m) = live.mental_mut() {
            if m.take_phase_changed(phase) {
                step.events.push(TableEvent::DealPhase { hand_no, phase });
            }
            if hole_ready && m.take_hole_ready_once() {
                step.events.push(TableEvent::LocalHoleReady { hand_no });
            }
        }
    }

    /// Emit the local peer's currently-pending keygen/shuffle/hole-reveal contribution, if any.
    fn emit_pending_deal_effect(&mut self, step: &mut Step) -> Result<(), TableError> {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return Ok(()),
        };
        let hand_no = live.hand_no;
        let seat = match live.mental().map(|m| m.seat()) {
            Some(s) => s,
            None => return Ok(()),
        };
        let effect = match live.mental().and_then(|m| m.pending_effect()) {
            Some(e) => e,
            None => return Ok(()),
        };
        let m = live.mental_mut().unwrap();
        match effect {
            DealEffect::AnnounceKey => {
                let payload = m.make_key_announcement()?;
                step.broadcasts.push(TableMessage::KeyAnnounce {
                    hand_no,
                    seat,
                    payload,
                });
                step.deal_effects.push(DealEffect::AnnounceKey);
            }
            DealEffect::Shuffle { .. } => {
                let (turn, payload) = m.make_shuffle()?;
                step.broadcasts.push(TableMessage::ShuffleAnnounce {
                    hand_no,
                    turn,
                    payload,
                });
                step.deal_effects.push(DealEffect::Shuffle { turn });
            }
            DealEffect::RevealHole => {
                let tokens = m.make_hole_reveal()?;
                step.broadcasts.push(TableMessage::RevealAnnounce {
                    hand_no,
                    seat,
                    round: RevealRound::Hole,
                    tokens,
                });
                step.deal_effects.push(DealEffect::RevealHole);
            }
            // Community / showdown reveals are driven by the street/settle logic below, not here.
            DealEffect::RevealCommunity { .. } | DealEffect::RevealShowdown => {}
        }
        Ok(())
    }

    /// For a mental hand: if the current betting street needs its community board revealed and
    /// this peer has not yet contributed its tokens for it, broadcast them. (Every seat
    /// contributes for community cards — they are public.)
    fn drive_community_and_betting(&mut self, step: &mut Step) -> Result<(), TableError> {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return Ok(()),
        };
        if live.pending_settle {
            return Ok(()); // run-out handled by the settle path
        }
        let hand_no = live.hand_no;
        let seat = match live.mental().map(|m| m.seat()) {
            Some(s) => s,
            None => return Ok(()),
        };
        let m = match live.mental() {
            Some(m) => m,
            None => return Ok(()),
        };
        if m.phase() != DealPhase::Ready {
            return Ok(());
        }
        // Which community street (if any) does the CURRENT betting street need revealed?
        let round = match street_community_round(live.betting.street) {
            Some(r) => r,
            None => {
                // Preflop: no community needed; sync revealed community into the board.
                self.sync_community(step);
                return Ok(());
            }
        };
        // If not yet revealed and this peer still owes its tokens, broadcast them.
        let needs = m.needs_community_reveal(round);
        if needs {
            let m = live.mental_mut().unwrap();
            let tokens = m.make_community_reveal(round)?;
            step.broadcasts.push(TableMessage::RevealAnnounce {
                hand_no,
                seat,
                round,
                tokens,
            });
            step.deal_effects.push(DealEffect::RevealCommunity { round });
        }
        self.sync_community(step);
        Ok(())
    }

    /// Pull any newly-revealed community cards from the MentalDeal into the Table's board,
    /// emitting a StreetDealt event for each completed street.
    fn sync_community(&mut self, step: &mut Step) {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return,
        };
        let hand_no = live.hand_no;
        let revealed: Vec<Card> = match live.mental() {
            Some(m) => m.community().to_vec(),
            None => return,
        };
        while live.community.len() < revealed.len() {
            let card = revealed[live.community.len()];
            live.community.push(card);
        }
        // Emit StreetDealt for streets that just completed (flop=3, turn=4, river=5).
        if let Some(m) = live.mental_mut() {
            for (round, count) in [
                (RevealRound::Flop, 3usize),
                (RevealRound::Turn, 4),
                (RevealRound::River, 5),
            ] {
                if revealed.len() >= count && m.take_street_announced(round) {
                    let street = match round {
                        RevealRound::Flop => Street::Flop,
                        RevealRound::Turn => Street::Turn,
                        RevealRound::River => Street::River,
                        _ => continue,
                    };
                    step.events
                        .push(TableEvent::StreetDealt { hand_no, street });
                }
            }
        }
    }

    /// For a mental hand whose betting is complete: run out remaining community streets via
    /// reveal, collect non-folded seats' showdown holes, and settle once everything is in.
    fn drive_showdown_and_settle(&mut self, step: &mut Step) -> Result<(), TableError> {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return Ok(()),
        };
        if !live.is_mental() {
            return Ok(());
        }
        if !live.pending_settle {
            return Ok(());
        }
        if live.mental().map(|m| m.phase()) != Some(DealPhase::Ready) {
            return Ok(());
        }
        let hand_no = live.hand_no;
        let seat = live.mental().unwrap().seat();
        let folded = live.betting.folded_flags();
        let n = folded.len();
        let active = folded.iter().filter(|f| !**f).count();

        // Fold-out: a single contestant wins uncontested — no reveals needed.
        if active <= 1 {
            self.settle(step)?;
            return Ok(());
        }

        // Contested showdown: need the full 5-card board run out + every non-folded seat's holes.
        // 1) Run out remaining community streets via reveal.
        for round in [RevealRound::Flop, RevealRound::Turn, RevealRound::River] {
            let m = self.live.as_ref().unwrap().mental().unwrap();
            if m.community_revealed(round) {
                continue;
            }
            if m.needs_community_reveal(round) {
                let live = self.live.as_mut().unwrap();
                let m = live.mental_mut().unwrap();
                let tokens = m.make_community_reveal(round)?;
                step.broadcasts.push(TableMessage::RevealAnnounce {
                    hand_no,
                    seat,
                    round,
                    tokens,
                });
                step.deal_effects.push(DealEffect::RevealCommunity { round });
            }
        }
        self.sync_community(step);

        // 2) This (non-folded) peer broadcasts its OWN showdown holes.
        if !folded[seat] {
            let live = self.live.as_mut().unwrap();
            let m = live.mental_mut().unwrap();
            if m.needs_showdown_reveal() {
                let tokens = m.make_showdown_reveal()?;
                step.broadcasts.push(TableMessage::RevealAnnounce {
                    hand_no,
                    seat,
                    round: RevealRound::Showdown,
                    tokens,
                });
                step.deal_effects.push(DealEffect::RevealShowdown);
            }
        }

        // 3) If the board is complete and every non-folded seat's holes are decrypted, settle.
        let live = self.live.as_ref().unwrap();
        let board_ok = live.community.len() == 5;
        let m = live.mental().unwrap();
        let holes_ok = (0..n)
            .filter(|i| !folded[*i])
            .all(|i| m.showdown_hole(i).is_some());
        if board_ok && holes_ok {
            self.settle(step)?;
        }
        Ok(())
    }

    // ====================================================================
    // Betting -> street advance / settle (shared)
    // ====================================================================

    /// After an action, advance closed streets and settle the hand when betting is over.
    fn advance_streets_and_settle(&mut self, step: &mut Step) -> Result<(), TableError> {
        loop {
            let live = match self.live.as_mut() {
                Some(l) => l,
                None => return Ok(()),
            };
            let is_mental = live.is_mental();
            if live.betting.hand_over_by_folds() {
                break;
            }
            if !live.betting.round_closed() {
                return Ok(()); // still mid-street
            }
            if live.betting.betting_complete_for_hand() {
                break; // all but one all-in: settle (runs out)
            }
            // Round closed with live betting still possible. Advance to the next street.
            match live.betting.next_street() {
                None => break, // river betting done -> settle
                Some(street) => {
                    if is_mental {
                        // Community for the new street is revealed asynchronously; the deal
                        // pump (called below) triggers it and gates betting until it lands.
                        // We do NOT deal locally and we do NOT loop further: betting on the
                        // new street waits for its reveal.
                        self.pump_mental(step)?;
                        return Ok(());
                    } else if let Deal::Placeholder { deck, deck_idx, .. } = &mut live.deal {
                        deal_community_street(street, deck, deck_idx, &mut live.community)?;
                        let hand_no = live.hand_no;
                        step.events.push(TableEvent::StreetDealt { hand_no, street });
                    }
                }
            }
        }
        // Betting is over. Placeholder settles immediately; mental defers to the reveal pump.
        let live = self.live.as_mut().unwrap();
        if live.is_mental() {
            live.pending_settle = true;
            self.pump_mental(step)?;
            Ok(())
        } else {
            self.settle(step)
        }
    }

    /// Run out any remaining board, compute the showdown, record the outcome, and clear the
    /// live hand. For placeholder hands the board is dealt locally; for mental hands the caller
    /// (the reveal pump) only invokes this once the board + required holes are revealed.
    fn settle(&mut self, step: &mut Step) -> Result<(), TableError> {
        let live = match self.live.as_mut() {
            Some(l) => l,
            None => return Ok(()),
        };

        // Run out the remaining community for a PLACEHOLDER showdown (mental hands already have
        // their board revealed by the pump before settle is called).
        if !live.betting.hand_over_by_folds() {
            if let Deal::Placeholder { deck, deck_idx, .. } = &mut live.deal {
                let mut street = live.betting.street;
                while live.community.len() < 5 {
                    let next = match street {
                        Street::Preflop => Street::Flop,
                        Street::Flop => Street::Turn,
                        Street::Turn => Street::River,
                        Street::River => break,
                    };
                    deal_community_street(next, deck, deck_idx, &mut live.community)?;
                    step.events.push(TableEvent::StreetDealt {
                        hand_no: live.hand_no,
                        street: next,
                    });
                    street = next;
                }
            }
        }

        let n = live.starting.len();
        let contributions = live.betting.contributions();
        let folded = live.betting.folded_flags();
        let pots = compute_pots(&contributions, &folded);

        // Hole cards for showdown: a folded seat mucks (None). For a placeholder hand every
        // seat's holes are known; for a mental hand a contesting seat's holes come from the
        // showdown reveal (the LOCAL seat's are its own decrypted holes).
        let hole_for_showdown: Vec<Option<[Card; 2]>> = match &live.deal {
            Deal::Placeholder { hole, .. } => (0..n)
                .map(|i| if folded[i] { None } else { Some(hole[i]) })
                .collect(),
            Deal::Mental(m) => (0..n)
                .map(|i| {
                    if folded[i] {
                        None
                    } else if i == m.seat() {
                        m.local_hole().or_else(|| m.showdown_hole(i))
                    } else {
                        m.showdown_hole(i)
                    }
                })
                .collect(),
        };

        let (winnings, awards) =
            distribute(&pots, &hole_for_showdown, &live.community, live.button, n);

        let mut final_stacks = vec![0u64; n];
        let mut deltas = vec![0i64; n];
        for i in 0..n {
            let leftover = live.starting[i] - contributions[i];
            final_stacks[i] = leftover + winnings[i];
            deltas[i] = final_stacks[i] as i64 - live.starting[i] as i64;
        }

        let outcome = HandOutcome {
            hand_no: live.hand_no,
            button: live.button,
            community: live.community.clone(),
            deltas,
            final_stacks,
            awards,
        };
        let completed = outcome.hand_no;
        self.live = None;
        self.last_completed = Some(match self.last_completed {
            Some(prev) => prev.max(completed),
            None => completed,
        });
        step.events.push(TableEvent::HandEnded(outcome));
        Ok(())
    }

    // ====================================================================
    // Local turn
    // ====================================================================

    /// If it is the LOCAL seat's turn to act, ask `strategy` for an action, apply it locally,
    /// and return the [`TableMessage::Act`] to broadcast (plus any resulting events). Returns an
    /// empty [`Step`] if it is not the local turn (including while a mental deal/reveal is still
    /// pending for the current street).
    pub fn local_turn(&mut self, strategy: &mut dyn crate::Strategy) -> Result<Step, TableError> {
        if !self.is_local_turn() {
            return Ok(Step::new());
        }
        let (hand_no, seat, action) = {
            let live = self.live.as_ref().ok_or(TableError::NoHandInProgress)?;
            let seat = live.betting.to_act().ok_or(TableError::NoHandInProgress)?;
            let action = strategy.decide(&live.betting, seat);
            (live.hand_no, seat, action)
        };

        let live = self.live.as_mut().ok_or(TableError::NoHandInProgress)?;
        live.betting.apply(seat, action.to_game())?;

        let mut step = Step::new();
        step.broadcasts.push(TableMessage::Act { hand_no, action });
        self.advance_streets_and_settle(&mut step)?;
        Ok(step)
    }

    /// Poke a mental hand to emit any local deal contribution it owes right now (key / shuffle /
    /// reveal) WITHOUT a betting turn. The driver calls this when it has no inbound message to
    /// process but the deal may still need this peer to speak (e.g. it is this peer's shuffle
    /// turn). Returns the broadcasts + events; empty for a placeholder hand or when nothing is
    /// owed.
    pub fn local_deal_step(&mut self) -> Result<Step, TableError> {
        let mut step = Step::new();
        self.pump_mental(&mut step)?;
        Ok(step)
    }

    /// The seed of the live PLACEHOLDER hand (diagnostics / display); `None` for mental hands.
    pub fn live_seed(&self) -> Option<u64> {
        self.live.as_ref().and_then(|h| match &h.deal {
            Deal::Placeholder { seed, .. } => Some(*seed),
            Deal::Mental(_) => None,
        })
    }
}

/// The community reveal round a betting street needs revealed before play (preflop needs none).
fn street_community_round(street: Street) -> Option<RevealRound> {
    match street {
        Street::Preflop => None,
        Street::Flop => Some(RevealRound::Flop),
        Street::Turn => Some(RevealRound::Turn),
        Street::River => Some(RevealRound::River),
    }
}

/// Whether the mental deal has revealed the community board needed to act on `street`.
fn community_revealed_for_street(m: &MentalDeal, street: Street) -> bool {
    match street_community_round(street) {
        None => true,
        Some(round) => m.community_revealed(round),
    }
}

#[cfg(test)]
mod tests;
