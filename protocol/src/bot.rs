//! Strategies: pluggable decision sources for a seat the local peer controls.
//!
//! A [`Strategy`] is asked for an [`Action`](crate::Action) whenever it is the local seat's
//! turn. It is given a read-only view of the live [`poker_game::BettingState`] and the seat
//! index. Implementations MUST be deterministic if the table is to stay replicated only when
//! the same seat is driven by the same strategy on the owning peer — but note that ONLY the
//! owning peer asks its strategy; the resulting [`Action`](crate::Action) is broadcast and
//! every other peer just applies it. So a strategy may be as fancy / random / interactive as
//! it likes without breaking determinism, because its output is shared as a message.

use crate::Action;
use poker_game::BettingState;

/// A decision source for a locally-controlled seat.
pub trait Strategy: Send {
    /// Choose an action for `seat` given the current betting `state`. The seat IS the seat to
    /// act (`state.to_act() == Some(seat)`), so a legal action exists.
    fn decide(&mut self, state: &BettingState, seat: usize) -> Action;
}

/// The simplest sane strategy: check when free, otherwise fold. Calls nothing, never bets.
/// Deterministic and dependency-free — ideal for headless demos and integration tests where
/// you want hands to terminate quickly and predictably.
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckFoldBot;

impl Strategy for CheckFoldBot {
    fn decide(&mut self, state: &BettingState, seat: usize) -> Action {
        if state.to_call(seat) == 0 {
            Action::Check
        } else {
            Action::Fold
        }
    }
}

/// A one-shot strategy that yields a single pre-chosen [`Action`] exactly once. The interactive
/// driver constructs one from a human's chosen action and feeds it through the same
/// [`Table::local_turn`](crate::table::Table::local_turn) path the bots use, so a human seat needs
/// no special-casing in the replicated state machine. Only ever consumed on a confirmed local turn.
pub struct OneShot(Option<Action>);

impl OneShot {
    pub fn new(action: Action) -> Self {
        OneShot(Some(action))
    }
}

impl Strategy for OneShot {
    fn decide(&mut self, _state: &BettingState, _seat: usize) -> Action {
        self.0
            .take()
            .expect("OneShot::decide called more than once")
    }
}

/// A passive strategy that always stays in the cheapest way: check when free, otherwise call.
/// Drives hands to showdown (good for exercising the full street/showdown path) while never
/// putting in a raise. Deterministic.
#[derive(Clone, Copy, Debug, Default)]
pub struct CallStationBot;

impl Strategy for CallStationBot {
    fn decide(&mut self, state: &BettingState, seat: usize) -> Action {
        if state.to_call(seat) == 0 {
            Action::Check
        } else {
            Action::Call
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_game::BettingState;

    #[test]
    fn check_fold_checks_when_free() {
        // Heads-up, button(0) posts SB, faces a 5 call preflop -> folds.
        let mut s = BettingState::new(vec![1000, 1000], 0, 5, 10).unwrap();
        let mut bot = CheckFoldBot;
        assert_eq!(bot.decide(&s, 0), Action::Fold);
        // After button calls, BB(1) is free to check.
        s.apply(0, poker_game::Action::Call).unwrap();
        assert_eq!(bot.decide(&s, 1), Action::Check);
    }

    #[test]
    fn call_station_calls_when_facing_bet() {
        let s = BettingState::new(vec![1000, 1000], 0, 5, 10).unwrap();
        let mut bot = CallStationBot;
        assert_eq!(bot.decide(&s, 0), Action::Call);
    }
}
