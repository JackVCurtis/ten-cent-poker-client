//! The replicated runtime for a TRUSTLESS (Barnett–Smart) mental-poker hand.
//!
//! This is the deal-orchestration half of the live hand: it owns one
//! [`poker_deal::distributed::PeerDeal`] (this peer's private replica — its secret key never
//! leaves) plus the COMMON, replicated bookkeeping that drives the interactive deal across
//! peers: how many keys have been collected, how many shuffles applied, which positions have
//! been revealed, and the decrypted public cards. [`crate::table::Table`] wraps it together
//! with the (unchanged M4) [`poker_game::BettingState`] so betting interleaves with the deal.
//!
//! # Replication
//! Every peer runs an identical [`MentalDeal`] fed the identical ordered stream of deal
//! messages. The bytes a peer *produces* locally (keys, shuffles, tokens) carry fresh
//! randomness and differ per peer, but everything that becomes COMMON state is derived from
//! *verified* messages, so the aggregate key, the post-shuffle deck, and every revealed card
//! are byte-identical on all peers. A peer's own hole cards are LOCAL: it decrypts them with
//! its withheld token and no one else can (see [`poker_deal::distributed`]).
//!
//! # Phase order
//! `Keygen -> Mask (deterministic, no exchange) -> Shuffle (seat order) -> Hole reveal ->
//! [betting/community reveals interleaved by the Table] -> Showdown reveal`. Each inbound
//! message is verified (proof + author + turn) before it mutates state; anything that fails is
//! rejected so M4-style anti-cheat extends to the deal.

use std::collections::{BTreeMap, BTreeSet};

use poker_deal::distributed::{KeyAnnouncement, PeerDeal, RevealMessage, ShuffleMessage};
use poker_deal::mental::{PublicKey, RevealToken};
use poker_game::Card;

use crate::table::TableError;
use crate::RevealRound;

/// Deck-position layout for the mental deal, mirroring [`poker_game::deal_hole`] /
/// [`poker_game::deal_community_street`] so the trustless deal and the placeholder/local
/// dealer agree on which deck index is which card.
///
/// Hole cards occupy positions `0..2n`, dealt round-robin (two passes) starting at the seat
/// left of the button — identical to [`poker_game::deal_hole`]. The board then follows the
/// burn convention: `flop = burn(2n), [2n+1, 2n+2, 2n+3]`, `turn = burn(2n+4), [2n+5]`,
/// `river = burn(2n+6), [2n+7]`.
pub struct DeckLayout {
    /// `hole[seat] == [pos0, pos1]`: the two deck positions for `seat`'s hole cards.
    hole: Vec<[usize; 2]>,
}

impl DeckLayout {
    pub fn new(num_seats: usize, button: usize) -> Self {
        // Mirror deal_hole's round-robin exactly.
        let mut hole = vec![[0usize; 2]; num_seats];
        let mut idx = 0usize;
        for round in 0..2 {
            let mut seat = (button + 1) % num_seats;
            for _ in 0..num_seats {
                hole[seat][round] = idx;
                idx += 1;
                seat = (seat + 1) % num_seats;
            }
        }
        DeckLayout { hole }
    }

    /// The two deck positions for `seat`'s hole cards.
    pub fn hole(&self, seat: usize) -> [usize; 2] {
        self.hole[seat]
    }

    /// All hole positions for all seats, ascending.
    pub fn all_hole(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.hole.iter().flatten().copied().collect();
        v.sort_unstable();
        v
    }

    /// The board (non-burn) deck positions for one street, given seat count `n`.
    pub fn community(&self, street: RevealRound, n: usize) -> Vec<usize> {
        let base = 2 * n;
        match street {
            // flop: burn at base, take base+1..=base+3
            RevealRound::Flop => vec![base + 1, base + 2, base + 3],
            // turn: burn at base+4, take base+5
            RevealRound::Turn => vec![base + 5],
            // river: burn at base+6, take base+7
            RevealRound::River => vec![base + 7],
            _ => Vec::new(),
        }
    }
}

/// Phase of the interactive deal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DealPhase {
    /// Collecting key announcements; aggregate not yet formed.
    Keygen,
    /// Aggregate key formed, initial deck masked; running the seat-order shuffle.
    Shuffle,
    /// Deck shuffled; running the hole-card reveal (and thereafter community/showdown reveals
    /// driven by the betting Table). Once `Ready`, betting may proceed.
    Ready,
}

/// What the LOCAL peer must contribute to the deal right now. The driver turns these into
/// broadcasts; they are recomputed from replicated state, so re-emitting one is harmless
/// (idempotent — peers reject duplicates / out-of-turn messages).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DealEffect {
    /// Broadcast this peer's [`crate::TableMessage::KeyAnnounce`].
    AnnounceKey,
    /// It is this peer's shuffle turn (`turn`); broadcast a [`crate::TableMessage::ShuffleAnnounce`].
    Shuffle { turn: usize },
    /// Broadcast this peer's hole-reveal tokens (for every OTHER seat's hole positions).
    RevealHole,
    /// Broadcast this peer's reveal tokens for a community street's board positions.
    RevealCommunity { round: RevealRound },
    /// Broadcast this peer's reveal tokens for its OWN hole positions at showdown.
    RevealShowdown,
}

/// Collected, already-verified reveal tokens keyed by deck position. l-of-l: a position is
/// decryptable once it holds one token from every seat.
type TokenSet = BTreeMap<usize, Vec<(RevealToken, PublicKey)>>;

/// The replicated mental-deal runtime for one hand.
pub struct MentalDeal {
    /// This peer's private deal replica (secret key never leaves it).
    peer: PeerDeal,
    num_players: usize,
    seat: usize,
    layout: DeckLayout,
    phase: DealPhase,
    /// Seats whose key announcement has been ingested + verified (own included once produced).
    keys_in: BTreeSet<usize>,
    /// Number of verified shuffles applied so far (also the next shuffle turn).
    shuffles_done: usize,
    /// Verified hole tokens received from OTHER seats, per hole position.
    hole_tokens: TokenSet,
    /// This peer's decrypted hole cards, once both are known. LOCAL/private.
    local_hole: Option<[Card; 2]>,
    /// Verified community tokens, per board position.
    community_tokens: TokenSet,
    /// Decrypted community cards in board order (COMMON).
    community: Vec<Card>,
    /// Which community streets have been fully revealed.
    community_done: BTreeSet<RevealRound>,
    /// Verified showdown tokens, per hole position (all seats including owners contribute).
    showdown_tokens: TokenSet,
    /// Decrypted showdown hole cards per seat (COMMON, only for revealed seats).
    showdown_hole: BTreeMap<usize, [Card; 2]>,
    /// Whether the local peer has already emitted each effect this hand (avoid spamming the
    /// driver; the driver still retransmits on its ticker).
    announced_key: bool,
    announced_hole: bool,
    announced_shuffle_for: BTreeSet<usize>,
    announced_community: BTreeSet<RevealRound>,
    announced_showdown: bool,
    /// One-shot event bookkeeping for the Table's `pump`: the last phase surfaced as an event,
    /// whether the local-hole-ready event has fired, and which street-dealt events have fired.
    emitted_phase: Option<DealPhase>,
    emitted_hole_ready: bool,
    emitted_streets: BTreeSet<RevealRound>,
}

impl MentalDeal {
    /// Build this peer's runtime for a hand. `session_seed` (32 bytes) yields deterministic
    /// scheme parameters on every peer. `seat` is this peer's seat in `seats`.
    pub fn new(
        session_seed: [u8; 32],
        seat: usize,
        num_players: usize,
        button: usize,
    ) -> Result<Self, TableError> {
        let mut rng = rand::thread_rng();
        // Full 52-card deck: m * n = 4 * 13.
        let peer = PeerDeal::new(&mut rng, session_seed, 4, 13, seat, num_players)
            .map_err(TableError::Deal2)?;
        Ok(MentalDeal {
            peer,
            num_players,
            seat,
            layout: DeckLayout::new(num_players, button),
            phase: DealPhase::Keygen,
            keys_in: BTreeSet::new(),
            shuffles_done: 0,
            hole_tokens: BTreeMap::new(),
            local_hole: None,
            community_tokens: BTreeMap::new(),
            community: Vec::new(),
            community_done: BTreeSet::new(),
            showdown_tokens: BTreeMap::new(),
            showdown_hole: BTreeMap::new(),
            announced_key: false,
            announced_hole: false,
            announced_shuffle_for: BTreeSet::new(),
            announced_community: BTreeSet::new(),
            announced_showdown: false,
            emitted_phase: None,
            emitted_hole_ready: false,
            emitted_streets: BTreeSet::new(),
        })
    }

    /// One-shot: returns `true` exactly once per distinct phase transition, so the Table emits a
    /// single [`crate::table::TableEvent::DealPhase`] per phase change.
    pub fn take_phase_changed(&mut self, phase: DealPhase) -> bool {
        if self.emitted_phase == Some(phase) {
            false
        } else {
            self.emitted_phase = Some(phase);
            true
        }
    }

    /// One-shot: returns `true` the first time the local hole cards are ready (used to emit a
    /// single [`crate::table::TableEvent::LocalHoleReady`]). Caller should only invoke it when
    /// the local hole is in fact known.
    pub fn take_hole_ready_once(&mut self) -> bool {
        if self.emitted_hole_ready {
            false
        } else {
            self.emitted_hole_ready = true;
            true
        }
    }

    /// One-shot: returns `true` the first time a community `round` has been fully revealed (used
    /// to emit a single [`crate::table::TableEvent::StreetDealt`] per street). Returns `false`
    /// if the round is not yet revealed or has already been announced.
    pub fn take_street_announced(&mut self, round: RevealRound) -> bool {
        if !self.community_done.contains(&round) {
            return false;
        }
        if self.emitted_streets.contains(&round) {
            return false;
        }
        self.emitted_streets.insert(round);
        true
    }

    pub fn phase(&self) -> DealPhase {
        self.phase
    }

    /// Is the deal far enough along that betting may proceed (deck shuffled and this peer's
    /// own hole cards decrypted)?
    pub fn ready_for_betting(&self) -> bool {
        self.phase == DealPhase::Ready && self.local_hole.is_some()
    }

    /// This peer's decrypted hole cards, if known (LOCAL/private).
    pub fn local_hole(&self) -> Option<[Card; 2]> {
        self.local_hole
    }

    /// The revealed community board (COMMON).
    pub fn community(&self) -> &[Card] {
        &self.community
    }

    /// A seat's hole cards revealed at showdown, if available (COMMON).
    pub fn showdown_hole(&self, seat: usize) -> Option<[Card; 2]> {
        self.showdown_hole.get(&seat).copied()
    }

    // ---- LOCAL CONTRIBUTIONS (effects -> messages) -----------------------

    /// Produce this peer's key-announcement payload (and record it locally). Idempotent.
    pub fn make_key_announcement(&mut self) -> Result<Vec<u8>, TableError> {
        let mut rng = rand::thread_rng();
        let ann = self.peer.key_announcement(&mut rng).map_err(TableError::Deal2)?;
        self.keys_in.insert(self.seat);
        self.maybe_finalize_keys()?;
        self.announced_key = true;
        ann.serialize().map_err(TableError::Deal2)
    }

    /// Produce this peer's shuffle payload for its turn (applies it to the local deck).
    pub fn make_shuffle(&mut self) -> Result<(usize, Vec<u8>), TableError> {
        if !self.peer.is_my_shuffle_turn(self.shuffles_done) {
            return Err(TableError::DealOutOfTurn);
        }
        let mut rng = rand::thread_rng();
        let msg = self.peer.shuffle(&mut rng).map_err(TableError::Deal2)?;
        let turn = self.shuffles_done;
        self.shuffles_done += 1;
        self.announced_shuffle_for.insert(turn);
        self.maybe_ready();
        let bytes = msg.serialize().map_err(TableError::Deal2)?;
        Ok((turn, bytes))
    }

    /// Produce this peer's hole-reveal tokens: one batch covering EVERY OTHER seat's two hole
    /// positions (never this peer's own — that token stays local for privacy).
    pub fn make_hole_reveal(&mut self) -> Result<Vec<Vec<u8>>, TableError> {
        let positions: Vec<usize> = (0..self.num_players)
            .filter(|s| *s != self.seat)
            .flat_map(|s| self.layout.hole(s))
            .collect();
        let out = self.reveal_for(&positions)?;
        // Self-collect our own tokens for these (other seats') positions so that — at showdown —
        // this peer has its OWN contribution toward every non-self hole, and can decrypt every
        // contesting seat's hand for the COMMON outcome without relying on its broadcast being
        // echoed back to it. (We never produce a token for our OWN hole here: privacy.)
        self.self_collect(RevealRound::Hole, &positions)?;
        self.announced_hole = true;
        Ok(out)
    }

    /// Produce this peer's reveal tokens for a community street's board positions.
    ///
    /// Also collects this peer's OWN tokens into the community token set, so the producer reaches
    /// the l-of-l threshold from the `N-1` it later ingests from others without relying on its
    /// own broadcast being echoed back to it.
    pub fn make_community_reveal(&mut self, round: RevealRound) -> Result<Vec<Vec<u8>>, TableError> {
        let positions = self.layout.community(round, self.num_players);
        let out = self.reveal_for(&positions)?;
        self.self_collect(round, &positions)?;
        self.announced_community.insert(round);
        Ok(out)
    }

    /// Produce this peer's reveal tokens for its OWN hole positions at showdown.
    ///
    /// Also collects this peer's own tokens into the showdown token set (see
    /// [`Self::make_community_reveal`]).
    pub fn make_showdown_reveal(&mut self) -> Result<Vec<Vec<u8>>, TableError> {
        let positions: Vec<usize> = self.layout.hole(self.seat).to_vec();
        let out = self.reveal_for(&positions)?;
        self.self_collect(RevealRound::Showdown, &positions)?;
        self.announced_showdown = true;
        Ok(out)
    }

    /// Collect this peer's OWN reveal tokens for `positions` into the `round`'s token set, so the
    /// producer counts toward the l-of-l threshold (community + showdown only; hole-card owners
    /// withhold their own token and decrypt via [`Self::try_decrypt_local_hole`]).
    fn self_collect(&mut self, round: RevealRound, positions: &[usize]) -> Result<(), TableError> {
        let mut rng = rand::thread_rng();
        for &pos in positions {
            let (tok, pk) = self
                .peer
                .own_reveal_contribution(&mut rng, pos)
                .map_err(TableError::Deal2)?;
            self.collect(round, self.seat, pos, tok, pk)?;
        }
        Ok(())
    }

    fn reveal_for(&self, positions: &[usize]) -> Result<Vec<Vec<u8>>, TableError> {
        let mut rng = rand::thread_rng();
        let msgs = self
            .peer
            .reveal_tokens(&mut rng, positions)
            .map_err(TableError::Deal2)?;
        msgs.iter()
            .map(|m| m.serialize().map_err(TableError::Deal2))
            .collect()
    }

    // ---- INGEST (verify remote messages) ---------------------------------

    /// Ingest a remote key announcement (verifies the Schnorr proof). `from_seat` is the
    /// authenticated author's seat; it must match the payload's seat.
    pub fn ingest_key(&mut self, from_seat: usize, payload: &[u8]) -> Result<(), TableError> {
        let ann = KeyAnnouncement::deserialize(payload).map_err(TableError::Deal2)?;
        if ann.seat != from_seat {
            return Err(TableError::DealAuthor);
        }
        if from_seat == self.seat {
            return Ok(()); // our own key is recorded locally
        }
        self.peer.ingest_key(&ann).map_err(TableError::Deal2)?;
        self.keys_in.insert(from_seat);
        self.maybe_finalize_keys()?;
        Ok(())
    }

    /// Ingest a remote shuffle (verifies the Bayer–Groth proof against the prior deck, in turn
    /// order). `from_seat` is the authenticated author.
    pub fn ingest_shuffle(
        &mut self,
        from_seat: usize,
        turn: usize,
        payload: &[u8],
    ) -> Result<(), TableError> {
        if self.phase != DealPhase::Shuffle {
            return Err(TableError::DealOutOfTurn);
        }
        // Enforce turn order: turn must be the next shuffle, and authored by that seat.
        if turn != self.shuffles_done || from_seat != turn {
            return Err(TableError::DealOutOfTurn);
        }
        if from_seat == self.seat {
            // Our own shuffle is applied when we produce it; ignore the echo.
            return Ok(());
        }
        let msg = ShuffleMessage::deserialize(payload).map_err(TableError::Deal2)?;
        if msg.seat != from_seat {
            return Err(TableError::DealAuthor);
        }
        self.peer.ingest_shuffle(&msg).map_err(TableError::Deal2)?;
        self.shuffles_done += 1;
        self.maybe_ready();
        Ok(())
    }

    /// Ingest a batch of remote reveal tokens (each Chaum–Pedersen proof verified against the
    /// masked card at its position). Routes them to the correct collection by `round`, then
    /// tries to complete any now-decryptable card.
    pub fn ingest_reveal(
        &mut self,
        from_seat: usize,
        round: RevealRound,
        tokens: &[Vec<u8>],
    ) -> Result<(), TableError> {
        if self.phase != DealPhase::Ready {
            return Err(TableError::DealOutOfTurn);
        }
        for raw in tokens {
            let msg = RevealMessage::deserialize(raw).map_err(TableError::Deal2)?;
            // Authenticate the contributor by its REGISTERED seat key, not by who forwarded the
            // message: the token's embedded key must equal the key this seat proved ownership of at
            // keygen, and the Chaum-Pedersen proof (checked in `verify_reveal`) is over that key.
            // This is what makes a host-relayed reveal trustworthy (guests are not directly
            // connected, so the host must forward one guest's tokens to another) while still
            // rejecting a forged-seat token: a peer cannot produce a valid proof for a key whose
            // secret it does not hold.
            let registered = self
                .peer
                .registered_public(msg.seat)
                .ok_or(TableError::DealAuthor)?;
            if msg.public != registered {
                return Err(TableError::DealAuthor);
            }
            // Dedup BEFORE the (expensive) proof verification: the host floods every accepted deal
            // payload on its retransmit ticker, so without this each peer would re-verify the same
            // Chaum-Pedersen proofs every tick — CPU thrash that widens the timing window and was
            // observed to worsen the showdown stall. If this seat's token for this position+round is
            // already collected, skip it cheaply (the retransmit has done its job).
            if self.have_token(round, registered, msg.position) {
                continue;
            }
            // Reject a hole-reveal token an author sent for its OWN hole position: owners must
            // withhold those (privacy). Showdown is where owners reveal their own holes.
            let (tok, pk) = self.peer.verify_reveal(&msg).map_err(TableError::Deal2)?;
            self.collect(round, msg.seat, msg.position, tok, pk)?;
        }
        let _ = from_seat;
        Ok(())
    }

    fn collect(
        &mut self,
        round: RevealRound,
        from_seat: usize,
        position: usize,
        tok: RevealToken,
        pk: PublicKey,
    ) -> Result<(), TableError> {
        match round {
            RevealRound::Hole => {
                // An owner must NOT broadcast a token for its own hole position.
                let owner = self.hole_owner(position);
                if owner == Some(from_seat) {
                    return Err(TableError::DealAuthor);
                }
                push_unique(&mut self.hole_tokens, position, from_seat, tok, pk);
                self.try_decrypt_local_hole()?;
            }
            RevealRound::Flop | RevealRound::Turn | RevealRound::River => {
                push_unique(&mut self.community_tokens, position, from_seat, tok, pk);
                self.try_decrypt_community(round)?;
            }
            RevealRound::Showdown => {
                push_unique(&mut self.showdown_tokens, position, from_seat, tok, pk);
                self.try_decrypt_showdown(position)?;
            }
        }
        Ok(())
    }

    /// Whether a token for `position` from the seat whose key is `pk` is already collected in
    /// `round`'s set (dedup by public key, matching [`push_unique`]). Lets ingest skip re-verifying
    /// a retransmitted/relayed duplicate before doing expensive proof verification.
    fn have_token(&self, round: RevealRound, pk: PublicKey, position: usize) -> bool {
        let set = match round {
            RevealRound::Hole => &self.hole_tokens,
            RevealRound::Flop | RevealRound::Turn | RevealRound::River => &self.community_tokens,
            RevealRound::Showdown => &self.showdown_tokens,
        };
        set.get(&position)
            .map(|v| v.iter().any(|(_, p)| *p == pk))
            .unwrap_or(false)
    }

    // ---- PHASE TRANSITIONS / DECRYPTION ----------------------------------

    fn maybe_finalize_keys(&mut self) -> Result<(), TableError> {
        if self.phase == DealPhase::Keygen
            && self.keys_in.len() == self.num_players
            && self.peer.all_keys_in()
        {
            let _agg = self.peer.finalize_aggregate_key().map_err(TableError::Deal2)?;
            self.peer.mask_initial_deck().map_err(TableError::Deal2)?;
            self.phase = DealPhase::Shuffle;
        }
        Ok(())
    }

    fn maybe_ready(&mut self) {
        if self.phase == DealPhase::Shuffle && self.shuffles_done == self.num_players {
            self.phase = DealPhase::Ready;
        }
    }

    /// Which seat owns deck `position` as a hole card, if any.
    fn hole_owner(&self, position: usize) -> Option<usize> {
        (0..self.num_players).find(|&s| self.layout.hole(s).contains(&position))
    }

    /// Try to decrypt this peer's own hole cards: needs the `N-1` other-seat tokens for each of
    /// its hole positions plus its own withheld token (computed locally, never broadcast).
    fn try_decrypt_local_hole(&mut self) -> Result<(), TableError> {
        if self.local_hole.is_some() {
            return Ok(());
        }
        let mine = self.layout.hole(self.seat);
        let mut cards = [None, None];
        let mut rng = rand::thread_rng();
        for (i, &pos) in mine.iter().enumerate() {
            let received = self.hole_tokens.get(&pos).cloned().unwrap_or_default();
            if received.len() != self.num_players - 1 {
                return Ok(()); // not all non-owner tokens in yet
            }
            // Add our own withheld token to reach the l-of-l threshold, decrypt LOCALLY.
            let (own_tok, own_pk) = self
                .peer
                .own_reveal_contribution(&mut rng, pos)
                .map_err(TableError::Deal2)?;
            let mut all = received;
            all.push((own_tok, own_pk));
            let card = self.peer.unmask_with(pos, &all).map_err(TableError::Deal2)?;
            cards[i] = Some(card);
        }
        if let (Some(a), Some(b)) = (cards[0], cards[1]) {
            self.local_hole = Some([a, b]);
        }
        Ok(())
    }

    fn try_decrypt_community(&mut self, round: RevealRound) -> Result<(), TableError> {
        if self.community_done.contains(&round) {
            return Ok(());
        }
        let positions = self.layout.community(round, self.num_players);
        // Need all N tokens for every board position of the street.
        for &pos in &positions {
            let toks = self.community_tokens.get(&pos);
            if toks.map(|t| t.len()).unwrap_or(0) != self.num_players {
                return Ok(());
            }
        }
        // Decrypt in board order, append to the community board.
        for &pos in &positions {
            let toks = self.community_tokens.get(&pos).cloned().unwrap_or_default();
            let card = self.peer.unmask_with(pos, &toks).map_err(TableError::Deal2)?;
            self.community.push(card);
        }
        self.community_done.insert(round);
        Ok(())
    }

    /// Try to decrypt a seat's hole pair at showdown. The l-of-l token set for an owner's hole
    /// position is assembled from BOTH reveal rounds: the `N-1` non-owner tokens broadcast during
    /// the HOLE round, plus the owner's own token broadcast in the SHOWDOWN round (the owner
    /// withheld it earlier for privacy). We merge the two sets, de-duplicated by public key, and
    /// decrypt once a position has all `N` distinct tokens.
    fn try_decrypt_showdown(&mut self, position: usize) -> Result<(), TableError> {
        let owner = match self.hole_owner(position) {
            Some(s) => s,
            None => return Ok(()),
        };
        if self.showdown_hole.contains_key(&owner) {
            return Ok(());
        }
        let [p0, p1] = self.layout.hole(owner);
        let mut cards: [Option<Card>; 2] = [None, None];
        for (i, &pos) in [p0, p1].iter().enumerate() {
            let merged = self.merged_showdown_tokens(pos);
            if merged.len() != self.num_players {
                return Ok(()); // not all tokens in for this position yet
            }
            let card = self.peer.unmask_with(pos, &merged).map_err(TableError::Deal2)?;
            cards[i] = Some(card);
        }
        if let (Some(a), Some(b)) = (cards[0], cards[1]) {
            self.showdown_hole.insert(owner, [a, b]);
        }
        Ok(())
    }

    /// The combined, de-duplicated (by public key) token set for `position` across the hole and
    /// showdown rounds — the full l-of-l set needed to decrypt the owner's hole card.
    fn merged_showdown_tokens(&self, position: usize) -> Vec<(RevealToken, PublicKey)> {
        let mut out: Vec<(RevealToken, PublicKey)> = Vec::new();
        for src in [&self.hole_tokens, &self.showdown_tokens] {
            if let Some(toks) = src.get(&position) {
                for (tok, pk) in toks {
                    if !out.iter().any(|(_, p)| p == pk) {
                        out.push((*tok, *pk));
                    }
                }
            }
        }
        out
    }

    // ---- EFFECTS: what must THIS peer broadcast now ----------------------

    /// Compute the deal effect (if any) the local peer should act on for the CURRENT phase.
    /// The driver calls this after every state change. Effects are recomputed from replicated
    /// state, so resending one is harmless.
    pub fn pending_effect(&self) -> Option<DealEffect> {
        match self.phase {
            DealPhase::Keygen => (!self.announced_key).then_some(DealEffect::AnnounceKey),
            DealPhase::Shuffle => {
                let turn = self.shuffles_done;
                if self.peer.is_my_shuffle_turn(turn) && !self.announced_shuffle_for.contains(&turn)
                {
                    Some(DealEffect::Shuffle { turn })
                } else {
                    None
                }
            }
            DealPhase::Ready => {
                if !self.announced_hole {
                    Some(DealEffect::RevealHole)
                } else {
                    None
                }
            }
        }
    }

    /// Whether this peer still owes its key announcement.
    pub fn needs_key_announce(&self) -> bool {
        self.phase == DealPhase::Keygen && !self.announced_key
    }

    /// Whether the local peer still owes a community reveal for `round`.
    pub fn needs_community_reveal(&self, round: RevealRound) -> bool {
        self.phase == DealPhase::Ready && !self.announced_community.contains(&round)
    }

    /// Whether the local peer still owes its showdown reveal.
    pub fn needs_showdown_reveal(&self) -> bool {
        self.phase == DealPhase::Ready && !self.announced_showdown
    }

    /// Whether a community street has been fully decrypted.
    pub fn community_revealed(&self, round: RevealRound) -> bool {
        self.community_done.contains(&round)
    }

    /// Test/inspection: this peer's seat.
    pub fn seat(&self) -> usize {
        self.seat
    }

    /// Test/inspection: a debug rng handle is unnecessary; expose the deal for the Table.
    pub fn peer(&self) -> &PeerDeal {
        &self.peer
    }

    /// Diagnostics: per-seat showdown hole token counts (hole-round + showdown-round = merged),
    /// and whether each seat's showdown hole has decrypted. A seat needs `merged == num_players`.
    pub fn debug_missing(&self) -> String {
        let mut s = format!(
            "phase={:?} lhole={} cdone={} ann_sd={}",
            self.phase,
            self.local_hole.is_some(),
            self.community_done.len(),
            self.announced_showdown
        );
        for seat in 0..self.num_players {
            let dec = self.showdown_hole.contains_key(&seat);
            let parts: Vec<String> = self
                .layout
                .hole(seat)
                .iter()
                .map(|&p| {
                    format!(
                        "p{p}:h{}+s{}=m{}",
                        self.hole_tokens.get(&p).map(|v| v.len()).unwrap_or(0),
                        self.showdown_tokens.get(&p).map(|v| v.len()).unwrap_or(0),
                        self.merged_showdown_tokens(p).len()
                    )
                })
                .collect();
            s.push_str(&format!(" s{seat}(dec={dec} {})", parts.join(",")));
        }
        s
    }
}

/// Push a (token, public) for `position` from `from_seat`, de-duplicating by seat so a peer
/// cannot inflate the token count by resending. (Tokens are keyed implicitly by insertion;
/// we also track seat membership to keep the l-of-l count honest.)
fn push_unique(
    set: &mut TokenSet,
    position: usize,
    _from_seat: usize,
    tok: RevealToken,
    pk: PublicKey,
) {
    let entry = set.entry(position).or_default();
    // De-dup by public key: the same seat's token must not be counted twice.
    if entry.iter().any(|(_, p)| *p == pk) {
        return;
    }
    entry.push((tok, pk));
}
