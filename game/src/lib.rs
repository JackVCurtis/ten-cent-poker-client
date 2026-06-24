//! Pure, deterministic Texas Hold'em engine.
//!
//! This crate carries the synchronous game logic — cards, betting math, pot/side-pot
//! distribution, hand evaluation, showdown. It does no I/O and pulls in no async or
//! crypto machinery. Load-bearing arithmetic mirrors the Verus-verified `core/` crate
//! verbatim, so the proofs in `core/` cover the code that actually runs here.
//!
//! M0 seeds the crate with the 52-card model and the first mirrored arithmetic helper.
//! The full betting state machine, side-pot logic, and 7→best-5 hand evaluator land in M1.

use serde::{Deserialize, Serialize};

pub mod betting;
pub mod hand;
pub mod hand_eval;
pub mod pot;
pub mod showdown;

pub use betting::{Action, BettingError, BettingState, Seat, Street};
pub use hand::{
    deal_community_street, deal_hole, hole_card_count, play_hand, ActionSource, HandError,
    HandResult, ScriptedActions,
};
pub use hand_eval::{evaluate_best, HandRank, HandValue};
pub use pot::{compute_pots, Pot};
pub use showdown::{distribute, PotAward};

/// A card suit. The ordinal is used only for the canonical card index; it carries no
/// hand-strength meaning (poker suits are unranked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Suit {
    Clubs = 0,
    Diamonds = 1,
    Hearts = 2,
    Spades = 3,
}

impl Suit {
    pub const ALL: [Suit; 4] = [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades];
}

/// A card rank, Two (low) through Ace (high). `Ord` reflects poker rank ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Rank {
    Two = 0,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Jack,
    Queen,
    King,
    Ace,
}

impl Rank {
    pub const ALL: [Rank; 13] = [
        Rank::Two,
        Rank::Three,
        Rank::Four,
        Rank::Five,
        Rank::Six,
        Rank::Seven,
        Rank::Eight,
        Rank::Nine,
        Rank::Ten,
        Rank::Jack,
        Rank::Queen,
        Rank::King,
        Rank::Ace,
    ];
}

/// A playing card from the standard 52-card deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Card {
    pub rank: Rank,
    pub suit: Suit,
}

impl Card {
    /// Map a card to a canonical index in `0..52`. This index is the bridge to the
    /// mental-poker layer (M2), which encodes each card as a distinct curve point keyed
    /// by it.
    pub fn to_index(self) -> u8 {
        (self.rank as u8) * 4 + (self.suit as u8)
    }

    /// Inverse of [`Card::to_index`]. Returns `None` for `index >= 52`.
    pub fn from_index(index: u8) -> Option<Card> {
        if index >= 52 {
            return None;
        }
        Some(Card {
            rank: Rank::ALL[(index / 4) as usize],
            suit: Suit::ALL[(index % 4) as usize],
        })
    }
}

/// Chips are counted in the smallest indivisible unit (no fractional chips).
pub type Chips = u64;

/// Chips a player must add to call: the gap between the table's current bet and what the
/// player has already committed this round.
///
/// Mirrors the Verus-verified `amount_to_call` in `core/src/lib.rs` (chips conserved:
/// `already_in + result == current_bet`). Callers must ensure `already_in <= current_bet`.
pub fn amount_to_call(current_bet: Chips, already_in: Chips) -> Chips {
    current_bet - already_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn card_index_roundtrips_all_52() {
        let mut seen = HashSet::new();
        for i in 0..52u8 {
            let c = Card::from_index(i).unwrap();
            assert_eq!(c.to_index(), i);
            assert!(seen.insert(i), "duplicate index {i}");
        }
        assert_eq!(seen.len(), 52);
        assert!(Card::from_index(52).is_none());
    }

    #[test]
    fn amount_to_call_conserves_chips() {
        assert_eq!(amount_to_call(100, 40), 60);
        let (current, already) = (250, 75);
        assert_eq!(already + amount_to_call(current, already), current);
    }
}
