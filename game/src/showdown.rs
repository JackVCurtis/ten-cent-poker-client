//! Showdown evaluation and pot distribution.
//!
//! For each pot, the best hand among its eligible, non-folded players wins. Ties split the
//! pot evenly. Odd chips that cannot divide evenly are awarded deterministically: **odd
//! chips go to the winners in seat order starting from the first seat left of the button**
//! (ascending index modulo table size). This matches standard live-poker high-hand odd-
//! chip rules closely enough to be deterministic and is fully documented for callers.
//!
//! Chip conservation: the sum of all per-seat payouts equals the sum of all pot amounts.

use crate::hand_eval::{evaluate_best, HandValue};
use crate::pot::Pot;
use crate::{Card, Chips};

/// Winners of a single pot and the amount each receives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PotAward {
    /// Seat indices that share this pot.
    pub winners: Vec<usize>,
    /// Total amount of the pot distributed (== `Pot::amount`).
    pub amount: Chips,
    /// Per-seat payout for this pot, indexed by seat (len == num_seats).
    pub payouts: Vec<Chips>,
}

/// Distribute every pot among eligible players and return per-seat total winnings.
///
/// - `pots`: layered pots from [`crate::pot::compute_pots`].
/// - `hole`: each seat's two hole cards (`None` if the seat is not contesting / folded;
///   such seats simply never appear as eligible).
/// - `community`: the shared board (0..=5 cards).
/// - `button`: dealer button index, for odd-chip ordering.
/// - `num_seats`: table size.
///
/// Returns `(per_seat_winnings, awards)` where `per_seat_winnings[i]` is total chips seat
/// `i` won across all pots. Conserves chips exactly.
pub fn distribute(
    pots: &[Pot],
    hole: &[Option<[Card; 2]>],
    community: &[Card],
    button: usize,
    num_seats: usize,
) -> (Vec<Chips>, Vec<PotAward>) {
    let mut winnings = vec![0u64; num_seats];
    let mut awards = Vec::with_capacity(pots.len());

    for pot in pots {
        let award = distribute_one(pot, hole, community, button, num_seats);
        // Chip conservation per layer: every chip in the pot is paid out. With non-empty
        // eligible sets (guaranteed by `compute_pots`) and at least one evaluable hand this
        // always holds; the assertion guards against a silent drop slipping back in.
        debug_assert_eq!(
            award.payouts.iter().sum::<Chips>(),
            pot.amount,
            "pot of {} not fully distributed (eligible {:?})",
            pot.amount,
            pot.eligible
        );
        for (i, &p) in award.payouts.iter().enumerate() {
            winnings[i] += p;
        }
        awards.push(award);
    }

    (winnings, awards)
}

fn distribute_one(
    pot: &Pot,
    hole: &[Option<[Card; 2]>],
    community: &[Card],
    button: usize,
    num_seats: usize,
) -> PotAward {
    // Single eligible player (e.g. everyone else folded) wins uncontested without needing
    // a full board to evaluate.
    if pot.eligible.len() == 1 {
        let mut payouts = vec![0u64; num_seats];
        payouts[pot.eligible[0]] = pot.amount;
        return PotAward {
            winners: pot.eligible.clone(),
            amount: pot.amount,
            payouts,
        };
    }

    // Evaluate each eligible seat's best hand.
    let mut best: Option<HandValue> = None;
    let mut winners: Vec<usize> = Vec::new();

    for &seat in &pot.eligible {
        let cards = match hole.get(seat).and_then(|h| *h) {
            Some(h) => {
                let mut v: Vec<Card> = community.to_vec();
                v.push(h[0]);
                v.push(h[1]);
                v
            }
            None => continue,
        };
        let value = match evaluate_best(&cards) {
            Some(v) => v,
            None => continue,
        };
        match &best {
            None => {
                best = Some(value);
                winners = vec![seat];
            }
            Some(b) if value > *b => {
                best = Some(value);
                winners = vec![seat];
            }
            Some(b) if value == *b => winners.push(seat),
            _ => {}
        }
    }

    let mut payouts = vec![0u64; num_seats];
    if winners.is_empty() {
        // No eligible hand (shouldn't happen in a well-formed hand); nothing distributed.
        return PotAward {
            winners,
            amount: pot.amount,
            payouts,
        };
    }

    let share = pot.amount / winners.len() as u64;
    let mut remainder = pot.amount % winners.len() as u64;
    for &w in &winners {
        payouts[w] = share;
    }
    // Distribute odd chips one at a time, starting from the first winner left of button.
    if remainder > 0 {
        let order = odd_chip_order(&winners, button, num_seats);
        for w in order {
            if remainder == 0 {
                break;
            }
            payouts[w] += 1;
            remainder -= 1;
        }
    }

    PotAward {
        winners,
        amount: pot.amount,
        payouts,
    }
}

/// Order winners for odd-chip distribution: ascending seat index starting from the seat
/// immediately left of the button (button+1), wrapping around.
fn odd_chip_order(winners: &[usize], button: usize, num_seats: usize) -> Vec<usize> {
    let mut ordered: Vec<usize> = winners.to_vec();
    let start = (button + 1) % num_seats;
    ordered.sort_by_key(|&seat| (seat + num_seats - start) % num_seats);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rank, Suit};

    fn c(r: Rank, s: Suit) -> Card {
        Card { rank: r, suit: s }
    }

    #[test]
    fn single_winner_takes_pot() {
        let community = vec![
            c(Rank::Two, Suit::Clubs),
            c(Rank::Seven, Suit::Diamonds),
            c(Rank::Nine, Suit::Hearts),
            c(Rank::Jack, Suit::Spades),
            c(Rank::King, Suit::Clubs),
        ];
        let hole = vec![
            Some([c(Rank::Ace, Suit::Hearts), c(Rank::Ace, Suit::Spades)]),
            Some([c(Rank::King, Suit::Hearts), c(Rank::Queen, Suit::Spades)]),
        ];
        let pots = vec![Pot {
            amount: 100,
            eligible: vec![0, 1],
        }];
        let (w, _) = distribute(&pots, &hole, &community, 0, 2);
        assert_eq!(w, vec![100, 0]);
    }

    #[test]
    fn split_pot_even() {
        // Both play the board (same straight on board).
        let community = vec![
            c(Rank::Five, Suit::Clubs),
            c(Rank::Six, Suit::Diamonds),
            c(Rank::Seven, Suit::Hearts),
            c(Rank::Eight, Suit::Spades),
            c(Rank::Nine, Suit::Clubs),
        ];
        let hole = vec![
            Some([c(Rank::Two, Suit::Hearts), c(Rank::Three, Suit::Spades)]),
            Some([c(Rank::Two, Suit::Clubs), c(Rank::Three, Suit::Diamonds)]),
        ];
        let pots = vec![Pot {
            amount: 100,
            eligible: vec![0, 1],
        }];
        let (w, _) = distribute(&pots, &hole, &community, 0, 2);
        assert_eq!(w, vec![50, 50]);
    }

    #[test]
    fn odd_chip_goes_left_of_button() {
        // Split pot of 101: odd chip to first winner left of button.
        let community = vec![
            c(Rank::Five, Suit::Clubs),
            c(Rank::Six, Suit::Diamonds),
            c(Rank::Seven, Suit::Hearts),
            c(Rank::Eight, Suit::Spades),
            c(Rank::Nine, Suit::Clubs),
        ];
        let hole = vec![
            Some([c(Rank::Two, Suit::Hearts), c(Rank::Three, Suit::Spades)]),
            Some([c(Rank::Two, Suit::Clubs), c(Rank::Three, Suit::Diamonds)]),
        ];
        let pots = vec![Pot {
            amount: 101,
            eligible: vec![0, 1],
        }];
        // Button = 0, so seat 1 is left of button -> gets the odd chip.
        let (w, _) = distribute(&pots, &hole, &community, 0, 2);
        assert_eq!(w, vec![50, 51]);
        // Button = 1 -> seat 0 left of button gets odd chip.
        let (w2, _) = distribute(&pots, &hole, &community, 1, 2);
        assert_eq!(w2, vec![51, 50]);
    }

    #[test]
    fn conserves_chips_across_side_pots() {
        let community = vec![
            c(Rank::Two, Suit::Clubs),
            c(Rank::Seven, Suit::Diamonds),
            c(Rank::Nine, Suit::Hearts),
            c(Rank::Jack, Suit::Spades),
            c(Rank::King, Suit::Clubs),
        ];
        let hole = vec![
            Some([c(Rank::Ace, Suit::Hearts), c(Rank::Ace, Suit::Spades)]),
            Some([c(Rank::King, Suit::Hearts), c(Rank::Queen, Suit::Spades)]),
            Some([c(Rank::Three, Suit::Hearts), c(Rank::Four, Suit::Spades)]),
        ];
        let pots = vec![
            Pot {
                amount: 150,
                eligible: vec![0, 1, 2],
            },
            Pot {
                amount: 100,
                eligible: vec![1, 2],
            },
        ];
        let (w, _) = distribute(&pots, &hole, &community, 0, 3);
        // Seat 0 wins main (aces). Seat 1 wins side (KQ pair of kings > seat2).
        assert_eq!(w[0], 150);
        assert_eq!(w[1], 100);
        assert_eq!(w[2], 0);
        assert_eq!(w.iter().sum::<u64>(), 250);
    }
}
