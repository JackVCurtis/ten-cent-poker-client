//! Betting state machine for a single hand of No-Limit Texas Hold'em.
//!
//! # Conventions chosen (documented for test authors)
//!
//! - **Raise semantics: `Raise(to_total)`.** `Action::Raise(n)` means "make my total
//!   committed-this-round equal to `n`". So if the current bet is 20 and I `Raise(60)`, I
//!   am adding `60 - already_in` chips and the new current bet is 60. This is the
//!   "raise-to" convention. `Bet(n)` is the opening wager when the current bet is 0 and
//!   likewise means "set the bet to `n`" (a bet-to / total). `Call` matches the current
//!   bet; `AllIn` pushes the player's entire remaining stack.
//!
//! - **Min-raise.** The minimum legal raise increment equals the size of the last full
//!   bet/raise on this street. Preflop the opening min-raise increment is the big blind.
//!   A `Raise(to)` is legal only if `to - current_bet >= min_raise_increment` *unless* it
//!   is an all-in for the player's whole stack (which may be smaller).
//!
//! - **All-in under-raise does NOT reopen action.** If a player goes all-in for less than
//!   a full raise, the `current_bet` advances to that all-in amount (players who have not
//!   yet matched it still owe the difference to call) but the min-raise "line" is not
//!   moved and players who already acted and faced the prior full bet are NOT given a new
//!   right to re-raise. Enforcement: a genuine full bet/raise calls `reopen_action`, which
//!   clears `acted_since_raise` for everyone but the aggressor; an under-raise does not. A
//!   seat is only allowed to bet/raise when [`BettingState::may_reopen`] holds (i.e. it has
//!   not yet acted since the last full raise). A seat brought back to act purely because an
//!   under-raise advanced `current_bet` may therefore only call or fold. (`last_full_raise_to`
//!   records the level of the last full raise for diagnostics and parity with the design.)
//!
//! - **Button / blinds / first to act.**
//!   - 3+ handed: small blind is the seat left of the button, big blind the next seat,
//!     preflop first-to-act is the seat after the big blind (UTG). Postflop first-to-act
//!     is the first non-folded seat left of the button.
//!   - Heads-up (2 players): the **button posts the small blind** and acts first preflop;
//!     the other player posts the big blind and acts first on every later street.
//!
//! Seats are addressed by index `0..num_seats`. "Left of" means ascending index modulo
//! the table size.

use crate::{amount_to_call, Chips};
use thiserror::Error;

/// The four betting streets of Texas Hold'em.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Street {
    Preflop,
    Flop,
    Turn,
    River,
}

/// A player action submitted to the betting engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Fold,
    Check,
    Call,
    /// Open the betting to a total of `n` chips this round (current bet must be 0).
    Bet(Chips),
    /// Raise so this player's total committed this round becomes `n` ("raise-to").
    Raise(Chips),
    /// Commit the player's entire remaining stack.
    AllIn,
}

/// Errors from validating/applying an [`Action`]. Library logic never panics on these.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum BettingError {
    #[error("no player is currently to act (round or hand is over)")]
    NotInAction,
    #[error("seat {0} is not the player to act")]
    NotYourTurn(usize),
    #[error("cannot check: there is a bet of {current_bet} to call")]
    CannotCheck { current_bet: Chips },
    #[error("cannot call: nothing to call (use Check)")]
    NothingToCall,
    #[error("cannot bet: there is already a bet this round (use Raise)")]
    AlreadyBet,
    #[error("bet/raise of {requested} is below the minimum of {minimum}")]
    BelowMinimum { requested: Chips, minimum: Chips },
    #[error("raise target {target} must exceed current bet {current_bet}")]
    RaiseNotHigher { target: Chips, current_bet: Chips },
    #[error("seat {seat} may not re-raise: action was not reopened (faced only an all-in under-raise)")]
    ActionNotReopened { seat: usize },
    #[error("seat {seat} has only {stack} chips, cannot commit {needed}")]
    InsufficientStack {
        seat: usize,
        stack: Chips,
        needed: Chips,
    },
    #[error("invalid number of seats {0} (must be 2..=9)")]
    InvalidSeatCount(usize),
}

/// Per-seat state for one hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    /// Chips remaining in front of the player (not yet committed).
    pub stack: Chips,
    /// Chips committed *this betting round*.
    pub committed: Chips,
    /// Total chips committed across the whole hand (feeds pot computation).
    pub total_committed: Chips,
    pub folded: bool,
    pub all_in: bool,
}

impl Seat {
    fn new(stack: Chips) -> Self {
        Seat {
            stack,
            committed: 0,
            total_committed: 0,
            folded: false,
            all_in: false,
        }
    }

    /// Active = still able to make decisions (not folded, not all-in).
    fn active(&self) -> bool {
        !self.folded && !self.all_in
    }
}

/// The betting state machine for a single hand.
#[derive(Clone, Debug)]
pub struct BettingState {
    pub seats: Vec<Seat>,
    pub button: usize,
    pub small_blind: Chips,
    pub big_blind: Chips,
    pub street: Street,
    /// Highest total committed-this-round by any seat (the amount to match).
    pub current_bet: Chips,
    /// The minimum legal raise *increment* for the next raise.
    pub min_raise: Chips,
    /// The committed-this-round level of the last *full* bet/raise. Used to decide
    /// whether an all-in under-raise reopens action. Players whose `last_acted_at` equals
    /// this value have already faced the standing full bet.
    last_full_raise_to: Chips,
    /// Index of the seat to act, or `None` if the round/hand is closed.
    to_act: Option<usize>,
    /// For each seat, whether they have acted since the last full raise (and thus do not
    /// owe another decision unless reopened).
    acted_since_raise: Vec<bool>,
}

impl BettingState {
    /// Start a new hand: deduct blinds, set the first player to act. Does not deal cards
    /// (the driver handles cards). `stacks` length is the seat count (2..=9).
    pub fn new(
        stacks: Vec<Chips>,
        button: usize,
        small_blind: Chips,
        big_blind: Chips,
    ) -> Result<Self, BettingError> {
        let n = stacks.len();
        if !(2..=9).contains(&n) {
            return Err(BettingError::InvalidSeatCount(n));
        }
        let seats: Vec<Seat> = stacks.into_iter().map(Seat::new).collect();
        let mut s = BettingState {
            seats,
            button: button % n,
            small_blind,
            big_blind,
            street: Street::Preflop,
            current_bet: 0,
            min_raise: big_blind,
            last_full_raise_to: 0,
            to_act: None,
            acted_since_raise: vec![false; n],
        };
        s.post_blinds();
        Ok(s)
    }

    fn num_seats(&self) -> usize {
        self.seats.len()
    }

    /// Next seat index clockwise (ascending mod n).
    fn next_seat(&self, from: usize) -> usize {
        (from + 1) % self.num_seats()
    }

    /// First seat at/after `from` (inclusive of `from` only if `include_from`) that is
    /// active (can act). Returns `None` if no such seat.
    fn next_active(&self, from: usize, include_from: bool) -> Option<usize> {
        let n = self.num_seats();
        let start = if include_from { from } else { self.next_seat(from) };
        let mut i = start;
        for _ in 0..n {
            if self.seats[i].active() {
                return Some(i);
            }
            i = self.next_seat(i);
        }
        None
    }

    /// Post the blinds and set preflop first-to-act.
    fn post_blinds(&mut self) {
        let n = self.num_seats();
        let heads_up = n == 2;
        let (sb_seat, bb_seat) = if heads_up {
            // Button posts SB, other posts BB.
            (self.button, self.next_seat(self.button))
        } else {
            let sb = self.next_seat(self.button);
            let bb = self.next_seat(sb);
            (sb, bb)
        };

        self.post_blind(sb_seat, self.small_blind);
        self.post_blind(bb_seat, self.big_blind);

        self.current_bet = self.big_blind;
        self.min_raise = self.big_blind;
        self.last_full_raise_to = self.big_blind;

        // First to act preflop: seat after BB (UTG); heads-up that is the button (SB).
        let first = if heads_up {
            sb_seat
        } else {
            self.next_seat(bb_seat)
        };
        self.to_act = self.next_active(first, true);
        // Blind posters have not "acted" voluntarily; they get an option.
        self.acted_since_raise = vec![false; n];
    }

    /// Post a (possibly partial, if short-stacked) blind.
    fn post_blind(&mut self, seat: usize, amount: Chips) {
        let pay = amount.min(self.seats[seat].stack);
        self.seats[seat].stack -= pay;
        self.seats[seat].committed += pay;
        self.seats[seat].total_committed += pay;
        if self.seats[seat].stack == 0 {
            self.seats[seat].all_in = true;
        }
    }

    /// The seat currently to act, if any.
    pub fn to_act(&self) -> Option<usize> {
        self.to_act
    }

    /// Number of seats that have not folded.
    pub fn players_remaining(&self) -> usize {
        self.seats.iter().filter(|s| !s.folded).count()
    }

    /// True if the hand has ended because all but one player folded.
    pub fn hand_over_by_folds(&self) -> bool {
        self.players_remaining() <= 1
    }

    /// Chips seat `i` must add to call the current bet (0 if already matched).
    pub fn to_call(&self, seat: usize) -> Chips {
        amount_to_call(self.current_bet, self.seats[seat].committed)
    }

    /// Validate and apply `action` for `seat`. On success, advances `to_act`.
    pub fn apply(&mut self, seat: usize, action: Action) -> Result<(), BettingError> {
        let actor = self.to_act.ok_or(BettingError::NotInAction)?;
        if actor != seat {
            return Err(BettingError::NotYourTurn(seat));
        }

        match action {
            Action::Fold => self.do_fold(seat),
            Action::Check => self.do_check(seat)?,
            Action::Call => self.do_call(seat)?,
            Action::Bet(n) => self.do_bet(seat, n)?,
            Action::Raise(n) => self.do_raise(seat, n)?,
            Action::AllIn => self.do_all_in(seat)?,
        }

        self.advance(seat);
        Ok(())
    }

    fn do_fold(&mut self, seat: usize) {
        self.seats[seat].folded = true;
        self.acted_since_raise[seat] = true;
    }

    fn do_check(&mut self, seat: usize) -> Result<(), BettingError> {
        if self.to_call(seat) > 0 {
            return Err(BettingError::CannotCheck {
                current_bet: self.current_bet,
            });
        }
        self.acted_since_raise[seat] = true;
        Ok(())
    }

    fn do_call(&mut self, seat: usize) -> Result<(), BettingError> {
        let need = self.to_call(seat);
        if need == 0 {
            return Err(BettingError::NothingToCall);
        }
        // Calling for less than `need` is an all-in call (capped at stack).
        let pay = need.min(self.seats[seat].stack);
        self.commit(seat, pay);
        self.acted_since_raise[seat] = true;
        Ok(())
    }

    fn do_bet(&mut self, seat: usize, total: Chips) -> Result<(), BettingError> {
        if self.current_bet != 0 {
            return Err(BettingError::AlreadyBet);
        }
        // Opening bet must be >= big blind (the min bet), unless it is an all-in shove
        // for the whole stack that happens to be smaller.
        let stack = self.seats[seat].stack;
        let is_full_stack = total >= stack;
        if total > stack {
            return Err(BettingError::InsufficientStack {
                seat,
                stack,
                needed: total,
            });
        }
        if total < self.big_blind && !is_full_stack {
            return Err(BettingError::BelowMinimum {
                requested: total,
                minimum: self.big_blind,
            });
        }
        self.commit(seat, total);
        self.current_bet = self.seats[seat].committed;
        // A full bet (>= bb) sets the raise line; an all-in short bet does not.
        if self.seats[seat].committed >= self.big_blind {
            self.min_raise = self.seats[seat].committed;
            self.last_full_raise_to = self.seats[seat].committed;
            self.reopen_action(seat);
        }
        self.acted_since_raise[seat] = true;
        Ok(())
    }

    /// Whether `seat` is permitted to reopen the betting (bet/raise) right now, as opposed
    /// to only being allowed to call/fold. Action is closed to a seat that has already acted
    /// since the last *full* bet/raise: it is back to act solely because an all-in under-
    /// raise advanced `current_bet` (so it owes chips to call), which by rule does not grant
    /// a fresh re-raise right. See module docs and `last_full_raise_to`.
    fn may_reopen(&self, seat: usize) -> bool {
        // A seat may reopen only if it has not yet acted against the standing full raise.
        // Equivalently, it has not been committed up to the last full-raise line by its own
        // prior action. `acted_since_raise` is cleared by `reopen_action` whenever a genuine
        // full raise occurs, so a seat with `acted_since_raise == true` is here only to
        // answer an under-raise and must not re-raise.
        !self.acted_since_raise[seat]
    }

    fn do_raise(&mut self, seat: usize, target_total: Chips) -> Result<(), BettingError> {
        if self.current_bet == 0 {
            // No bet to raise; treat as a bet path error to be explicit.
            return Err(BettingError::NothingToCall);
        }
        if !self.may_reopen(seat) {
            return Err(BettingError::ActionNotReopened { seat });
        }
        if target_total <= self.current_bet {
            return Err(BettingError::RaiseNotHigher {
                target: target_total,
                current_bet: self.current_bet,
            });
        }
        let already = self.seats[seat].committed;
        let needed = target_total - already;
        let stack = self.seats[seat].stack;
        if needed > stack {
            return Err(BettingError::InsufficientStack {
                seat,
                stack,
                needed,
            });
        }
        let increment = target_total - self.current_bet;
        let is_full_stack = needed == stack;
        let min_increment = self.min_raise;
        if increment < min_increment && !is_full_stack {
            return Err(BettingError::BelowMinimum {
                requested: increment,
                minimum: min_increment,
            });
        }
        self.commit(seat, needed);
        let new_total = self.seats[seat].committed;
        let prev_bet = self.current_bet;
        self.current_bet = new_total;
        if increment >= min_increment {
            // Full raise: advance the raise line and reopen action.
            self.min_raise = increment;
            self.last_full_raise_to = new_total;
            self.reopen_action(seat);
        }
        // else: all-in under-raise; current_bet advanced but no reopen.
        let _ = prev_bet;
        self.acted_since_raise[seat] = true;
        Ok(())
    }

    fn do_all_in(&mut self, seat: usize) -> Result<(), BettingError> {
        let stack = self.seats[seat].stack;
        if stack == 0 {
            return Err(BettingError::InsufficientStack {
                seat,
                stack,
                needed: 1,
            });
        }
        let already = self.seats[seat].committed;
        let new_total = already + stack;
        let prev_bet = self.current_bet;
        self.commit(seat, stack); // empties stack, sets all_in

        if new_total > prev_bet {
            // This all-in raises (or opens) the bet.
            let increment = new_total - prev_bet;
            let opening = prev_bet == 0;
            let min_increment = self.min_raise;
            self.current_bet = new_total;
            // A seat that may not reopen (it is here only to answer an under-raise) can still
            // shove its remaining chips, but doing so must not grant a fresh re-raise to
            // anyone: the raise line stays put and no one is reopened.
            if increment >= min_increment && self.may_reopen(seat) {
                self.min_raise = increment;
                self.last_full_raise_to = new_total;
                self.reopen_action(seat);
            } else if opening {
                // Opening all-in below a full bet still "opens" but does not set a full
                // raise line beyond its own amount; still everyone must respond.
                self.reopen_action(seat);
            }
            // else under-raise: no reopen.
        }
        // If new_total <= prev_bet it is an all-in call for less; nothing reopens.
        self.acted_since_raise[seat] = true;
        Ok(())
    }

    /// Move chips from stack to committed; mark all-in if stack emptied.
    fn commit(&mut self, seat: usize, amount: Chips) {
        let s = &mut self.seats[seat];
        s.stack -= amount;
        s.committed += amount;
        s.total_committed += amount;
        if s.stack == 0 {
            s.all_in = true;
        }
    }

    /// After a full bet/raise, everyone except the raiser owes a fresh decision.
    fn reopen_action(&mut self, raiser: usize) {
        for (i, acted) in self.acted_since_raise.iter_mut().enumerate() {
            *acted = i == raiser;
        }
    }

    /// Advance to the next player to act, or close the round.
    fn advance(&mut self, last: usize) {
        if self.hand_over_by_folds() {
            self.to_act = None;
            return;
        }
        // If only one (or zero) seat can still act and everyone has matched, the round is
        // effectively closed — but we still need any remaining active player who owes a
        // decision.
        let n = self.num_seats();
        let mut i = self.next_seat(last);
        for _ in 0..n {
            let s = &self.seats[i];
            // A seat must act if it is active AND either it hasn't acted since the last
            // full raise, or it still owes chips to call (e.g. it faced an all-in under-
            // raise that advanced current_bet without reopening — it must still match it).
            if s.active() && (!self.acted_since_raise[i] || self.to_call(i) > 0) {
                self.to_act = Some(i);
                return;
            }
            i = self.next_seat(i);
        }
        // No one left who owes a decision: round closes.
        self.to_act = None;
    }

    /// True if the current betting round is closed (no one left to act).
    pub fn round_closed(&self) -> bool {
        self.to_act.is_none()
    }

    /// Advance to the next street, resetting per-round state. Returns the new street, or
    /// `None` if already on the river (no further betting). Caller deals community cards.
    pub fn next_street(&mut self) -> Option<Street> {
        let next = match self.street {
            Street::Preflop => Street::Flop,
            Street::Flop => Street::Turn,
            Street::Turn => Street::River,
            Street::River => return None,
        };
        self.street = next;
        self.current_bet = 0;
        self.min_raise = self.big_blind;
        self.last_full_raise_to = 0;
        for s in self.seats.iter_mut() {
            s.committed = 0;
        }
        self.acted_since_raise = vec![false; self.num_seats()];
        // First to act postflop: first active seat left of the button.
        self.to_act = self.next_active(self.button, false);
        Some(next)
    }

    /// Per-seat total contributions across the hand (for pot computation).
    pub fn contributions(&self) -> Vec<Chips> {
        self.seats.iter().map(|s| s.total_committed).collect()
    }

    /// Per-seat folded flags (for pot eligibility).
    pub fn folded_flags(&self) -> Vec<bool> {
        self.seats.iter().map(|s| s.folded).collect()
    }

    /// True when no further betting is possible this hand (at most one active player and
    /// all bets matched, or hand over by folds). Used by the driver to skip remaining
    /// streets and run them out.
    pub fn betting_complete_for_hand(&self) -> bool {
        if self.hand_over_by_folds() {
            return true;
        }
        // If <=1 player is still active (the rest all-in or folded) and the round is
        // closed, no more betting can occur.
        let active = self.seats.iter().filter(|s| s.active()).count();
        active <= 1 && self.round_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(stacks: &[Chips], button: usize) -> BettingState {
        BettingState::new(stacks.to_vec(), button, 5, 10).unwrap()
    }

    #[test]
    fn invalid_seat_count_rejected() {
        assert!(matches!(
            BettingState::new(vec![100], 0, 5, 10),
            Err(BettingError::InvalidSeatCount(1))
        ));
        assert!(BettingState::new(vec![100; 10], 0, 5, 10).is_err());
    }

    #[test]
    fn three_handed_blinds_and_first_to_act() {
        let s = state(&[1000, 1000, 1000], 0);
        // SB = seat 1, BB = seat 2, UTG = seat 0.
        assert_eq!(s.seats[1].committed, 5);
        assert_eq!(s.seats[2].committed, 10);
        assert_eq!(s.current_bet, 10);
        assert_eq!(s.to_act(), Some(0));
    }

    #[test]
    fn heads_up_button_is_sb_and_acts_first_preflop() {
        let s = state(&[1000, 1000], 0);
        // Button (0) posts SB, seat 1 posts BB. Button acts first preflop.
        assert_eq!(s.seats[0].committed, 5);
        assert_eq!(s.seats[1].committed, 10);
        assert_eq!(s.to_act(), Some(0));
    }

    #[test]
    fn heads_up_bb_acts_first_postflop() {
        let mut s = state(&[1000, 1000], 0);
        // Preflop: button calls, BB checks.
        s.apply(0, Action::Call).unwrap();
        s.apply(1, Action::Check).unwrap();
        assert!(s.round_closed());
        s.next_street();
        // Postflop first to act is non-button (seat 1).
        assert_eq!(s.to_act(), Some(1));
    }

    #[test]
    fn fold_to_one_ends_hand() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Fold).unwrap();
        s.apply(1, Action::Fold).unwrap();
        assert!(s.hand_over_by_folds());
        assert!(s.round_closed());
    }

    #[test]
    fn check_when_facing_bet_errors() {
        let mut s = state(&[1000, 1000, 1000], 0);
        assert!(matches!(
            s.apply(0, Action::Check),
            Err(BettingError::CannotCheck { .. })
        ));
    }

    #[test]
    fn bb_gets_option_to_check() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Call).unwrap(); // UTG calls 10
        s.apply(1, Action::Call).unwrap(); // SB calls (adds 5)
        // BB to act with option.
        assert_eq!(s.to_act(), Some(2));
        s.apply(2, Action::Check).unwrap();
        assert!(s.round_closed());
        assert_eq!(s.current_bet, 10);
    }

    #[test]
    fn raise_to_total_semantics_and_min_raise() {
        let mut s = state(&[1000, 1000, 1000], 0);
        // UTG raises to 30 (increment 20 >= bb 10). Min raise now 20.
        s.apply(0, Action::Raise(30)).unwrap();
        assert_eq!(s.current_bet, 30);
        assert_eq!(s.seats[0].committed, 30);
        assert_eq!(s.min_raise, 20);
        // SB tries to raise to 40: increment 10 < min 20 -> error.
        assert!(matches!(
            s.apply(1, Action::Raise(40)),
            Err(BettingError::BelowMinimum { .. })
        ));
        // SB raises to 50 (increment 20 ok).
        s.apply(1, Action::Raise(50)).unwrap();
        assert_eq!(s.current_bet, 50);
    }

    #[test]
    fn raise_must_be_higher_than_current_bet() {
        let mut s = state(&[1000, 1000, 1000], 0);
        assert!(matches!(
            s.apply(0, Action::Raise(10)),
            Err(BettingError::RaiseNotHigher { .. })
        ));
    }

    #[test]
    fn call_matches_and_closes_round() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Raise(30)).unwrap();
        s.apply(1, Action::Fold).unwrap();
        s.apply(2, Action::Call).unwrap();
        assert!(s.round_closed());
        assert_eq!(s.seats[2].committed, 30);
    }

    #[test]
    fn all_in_under_raise_advances_bet_but_not_raise_line() {
        // 4 seats. UTG (3) raises to 40, min_raise becomes 30. Short seat 0 (55 total)
        // all-ins to 55: a 15 increment, under the 30 min. current_bet advances to 55 but
        // the min-raise line is unchanged.
        let mut s = BettingState::new(vec![55, 1000, 1000, 1000], 0, 5, 10).unwrap();
        s.apply(3, Action::Raise(40)).unwrap();
        s.apply(0, Action::AllIn).unwrap();
        assert_eq!(s.current_bet, 55);
        assert_eq!(s.min_raise, 30); // unchanged by the under-raise
    }

    #[test]
    fn all_in_under_raise_does_not_let_original_raiser_reraise() {
        // 4 seats. UTG (seat 3) raises to 40 (min_raise=30). Seat 0 (short, 55 total)
        // all-ins to 55 — a 15 increment, under the 30 min — which must NOT reopen the
        // original raiser. SB and BB still owe a call; once they call/fold, the round
        // closes WITHOUT giving seat 3 another action.
        let mut s = BettingState::new(vec![55, 1000, 1000, 1000], 0, 5, 10).unwrap();
        assert_eq!(s.to_act(), Some(3));
        s.apply(3, Action::Raise(40)).unwrap();
        s.apply(0, Action::AllIn).unwrap(); // to 55, under-raise
        assert_eq!(s.current_bet, 55);
        // SB (1) calls 55, BB (2) calls 55.
        s.apply(1, Action::Call).unwrap();
        s.apply(2, Action::Call).unwrap();
        // Back to seat 3: it already faced the standing 55 only as an under-raise over its
        // own 40; it still owes 15 to call, so it DOES get to act (call), but cannot have
        // been forced into a re-raise reopening. It must act to match 55.
        assert_eq!(s.to_act(), Some(3));
        s.apply(3, Action::Call).unwrap();
        assert!(s.round_closed());
    }

    #[test]
    fn all_in_under_raise_forbids_original_raiser_from_reraising() {
        // Regression for the reopening bug: an all-in under-raise must not give a player who
        // already faced the prior full bet a fresh right to re-raise.
        // 4 seats, button 0 => SB 1, BB 2, UTG 3. Seat 0 is the short stack.
        let mut s = BettingState::new(vec![55, 1000, 1000, 1000], 0, 5, 10).unwrap();
        // Seat 3 (UTG) raises to 40: full raise, min_raise = 30.
        s.apply(3, Action::Raise(40)).unwrap();
        assert_eq!(s.min_raise, 30);
        // Seat 0 all-ins to 55: +15 over 40 is an under-raise; current_bet advances, no reopen.
        s.apply(0, Action::AllIn).unwrap();
        assert_eq!(s.current_bet, 55);
        assert_eq!(s.min_raise, 30);
        // SB and BB call 55.
        s.apply(1, Action::Call).unwrap();
        s.apply(2, Action::Call).unwrap();
        // Back to seat 3, which owes 15 to call. It must NOT be allowed to re-raise.
        assert_eq!(s.to_act(), Some(3));
        assert!(matches!(
            s.apply(3, Action::Raise(200)),
            Err(BettingError::ActionNotReopened { seat: 3 })
        ));
        // An all-in shove from seat 3 must also not reopen anyone; it may only call.
        // Calling closes the round.
        s.apply(3, Action::Call).unwrap();
        assert!(s.round_closed());
    }

    #[test]
    fn full_all_in_raise_reopens() {
        let mut s = BettingState::new(vec![1000, 1000, 1000], 0, 5, 10).unwrap();
        s.apply(0, Action::Raise(30)).unwrap();
        // SB all-in shoves its whole stack (1000) — a full raise, which reopens action.
        s.apply(1, Action::AllIn).unwrap();
        assert_eq!(s.current_bet, 1000);
        assert!(s.min_raise >= 970);
        // Action reopens to BB then back to UTG.
        assert!(s.to_act().is_some());
    }

    #[test]
    fn contributions_track_total() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Call).unwrap(); // 10
        s.apply(1, Action::Call).unwrap(); // +5 = 10
        s.apply(2, Action::Check).unwrap();
        let c = s.contributions();
        assert_eq!(c, vec![10, 10, 10]);
    }

    #[test]
    fn next_street_resets_round_state() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Call).unwrap();
        s.apply(1, Action::Call).unwrap();
        s.apply(2, Action::Check).unwrap();
        s.next_street();
        assert_eq!(s.street, Street::Flop);
        assert_eq!(s.current_bet, 0);
        for seat in &s.seats {
            assert_eq!(seat.committed, 0);
        }
        // First to act postflop = SB (seat 1) left of button.
        assert_eq!(s.to_act(), Some(1));
    }

    #[test]
    fn postflop_check_around_closes() {
        let mut s = state(&[1000, 1000, 1000], 0);
        s.apply(0, Action::Call).unwrap();
        s.apply(1, Action::Call).unwrap();
        s.apply(2, Action::Check).unwrap();
        s.next_street();
        s.apply(1, Action::Check).unwrap();
        s.apply(2, Action::Check).unwrap();
        s.apply(0, Action::Check).unwrap();
        assert!(s.round_closed());
    }
}
