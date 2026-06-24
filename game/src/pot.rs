//! Main pot and side-pot computation from per-player total contributions.
//!
//! Standard layered all-in algorithm: repeatedly peel off the smallest non-zero
//! remaining contribution as a "level", form a pot of `level * (#contributors at that
//! level)`, with every player who still has chips at that level eligible. Folded
//! players still contribute chips to the pots (their dead money) but are not eligible to
//! win, so eligibility is tracked separately from contribution.
//!
//! Invariant: `pots.iter().map(|p| p.amount).sum() == contributions.iter().sum()`.
//!
//! A pot layer is never emitted with an empty eligible set: if every contributor at some
//! level has folded, those chips are dead money with no contestant. Rather than silently
//! drop them (a chip-conservation violation if a downstream caller can't award them), they
//! are folded into the nearest *preceding* contested pot. If there is no preceding contested
//! pot (i.e. every contributor folded), the chips are refunded to their contributors.

use crate::Chips;

/// One pot layer: an amount and the set of seat indices eligible to win it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pot {
    pub amount: Chips,
    /// Seat indices eligible to contest this pot (not folded, contributed at this level).
    pub eligible: Vec<usize>,
}

/// Compute main + side pots from each seat's *total* contribution this hand.
///
/// `contributions[i]` is seat `i`'s total chips in the pot. `folded[i]` marks seats that
/// folded — they still feed the pots but cannot win them. Returns pots ordered main-pot
/// first. Seats with zero contribution are ignored. Total pot chips always equal the sum
/// of contributions.
pub fn compute_pots(contributions: &[Chips], folded: &[bool]) -> Vec<Pot> {
    assert_eq!(contributions.len(), folded.len());
    let n = contributions.len();
    let mut remaining: Vec<Chips> = contributions.to_vec();
    let mut pots: Vec<Layer> = Vec::new();

    loop {
        // Smallest non-zero remaining contribution defines the next level.
        let level = remaining
            .iter()
            .filter(|&&c| c > 0)
            .copied()
            .min();
        let level = match level {
            Some(l) => l,
            None => break,
        };

        let mut amount: Chips = 0;
        let mut eligible: Vec<usize> = Vec::new();
        let mut contributors: Vec<usize> = Vec::new();
        for i in 0..n {
            if remaining[i] >= level && remaining[i] > 0 {
                amount += level;
                remaining[i] -= level;
                contributors.push(i);
                if !folded[i] {
                    eligible.push(i);
                }
            }
        }
        pots.push(Layer {
            amount,
            eligible,
            contributors,
        });
    }

    // A layer whose contributors all folded has no eligible contestant. Such dead money is
    // folded into the nearest preceding contested pot, or refunded to its contributors if no
    // contested pot precedes it. This guarantees no `Pot` is ever emitted with an empty
    // eligible set, so every chip can be awarded by the showdown.
    let pots = absorb_dead_layers(pots);

    // Merge adjacent pots with identical eligible sets (e.g. consecutive levels where no
    // one dropped out) to keep the layering minimal and tidy.
    merge_equal_eligible(pots)
}

/// An intermediate pot layer that also tracks who contributed (for dead-money refunds).
struct Layer {
    amount: Chips,
    eligible: Vec<usize>,
    contributors: Vec<usize>,
}

/// Resolve layers with an empty eligible set (all contributors folded). Their chips are
/// merged into the nearest preceding contested layer; if none exists, refunded evenly to
/// the dead layer's own contributors (which keeps total chips conserved). Returns plain
/// `Pot`s, all with non-empty eligible sets.
fn absorb_dead_layers(layers: Vec<Layer>) -> Vec<Pot> {
    let mut out: Vec<Pot> = Vec::new();
    // Pending refunds, keyed by seat, when a dead layer has no preceding contested pot.
    let mut refunds: Vec<(usize, Chips)> = Vec::new();
    for layer in layers {
        if layer.eligible.is_empty() {
            if let Some(last) = out.last_mut() {
                // Roll the dead chips into the most recent contested pot.
                last.amount += layer.amount;
            } else if !layer.contributors.is_empty() {
                // No contested pot yet: refund to the contributors (an uncalled all-fold).
                let k = layer.contributors.len() as Chips;
                let share = layer.amount / k;
                let mut rem = layer.amount % k;
                for &c in &layer.contributors {
                    let mut give = share;
                    if rem > 0 {
                        give += 1;
                        rem -= 1;
                    }
                    refunds.push((c, give));
                }
            }
            continue;
        }
        out.push(Pot {
            amount: layer.amount,
            eligible: layer.eligible,
        });
    }
    // Apply any refunds by giving each contributor a single-eligible pot (it is their own
    // money returned uncontested). This keeps the `Pot` contract intact and conserves chips.
    for (seat, amount) in refunds {
        if amount > 0 {
            out.push(Pot {
                amount,
                eligible: vec![seat],
            });
        }
    }
    out
}

fn merge_equal_eligible(pots: Vec<Pot>) -> Vec<Pot> {
    let mut merged: Vec<Pot> = Vec::new();
    for pot in pots {
        if let Some(last) = merged.last_mut() {
            if last.eligible == pot.eligible {
                last.amount += pot.amount;
                continue;
            }
        }
        merged.push(pot);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(pots: &[Pot]) -> Chips {
        pots.iter().map(|p| p.amount).sum()
    }

    #[test]
    fn single_pot_no_allin() {
        let pots = compute_pots(&[100, 100, 100], &[false, false, false]);
        assert_eq!(pots.len(), 1);
        assert_eq!(pots[0].amount, 300);
        assert_eq!(pots[0].eligible, vec![0, 1, 2]);
    }

    #[test]
    fn one_short_allin_makes_side_pot() {
        // Seat 0 all-in for 50, seats 1&2 put in 100.
        let pots = compute_pots(&[50, 100, 100], &[false, false, false]);
        assert_eq!(total(&pots), 250);
        // Main pot: 50*3 = 150, all eligible.
        assert_eq!(pots[0].amount, 150);
        assert_eq!(pots[0].eligible, vec![0, 1, 2]);
        // Side pot: 50*2 = 100, only seats 1&2.
        assert_eq!(pots[1].amount, 100);
        assert_eq!(pots[1].eligible, vec![1, 2]);
    }

    #[test]
    fn folded_player_contributes_but_not_eligible() {
        // Seat 1 folded after putting in 30; others matched 100.
        let pots = compute_pots(&[100, 30, 100], &[false, true, false]);
        assert_eq!(total(&pots), 230);
        // Level 30: all three contribute (90), eligible 0,2. Level 70: seats 0,2 add 140,
        // eligible 0,2. Both layers share eligible set {0,2} and merge into one pot.
        assert_eq!(pots.len(), 1);
        assert_eq!(pots[0].amount, 230);
        assert_eq!(pots[0].eligible, vec![0, 2]);
    }

    #[test]
    fn multiple_allins_layered() {
        // 0: 25, 1: 50, 2: 100, 3: 100
        let pots = compute_pots(&[25, 50, 100, 100], &[false, false, false, false]);
        assert_eq!(total(&pots), 275);
        // Level 25: 4*25=100 eligible all.
        assert_eq!(pots[0].amount, 100);
        assert_eq!(pots[0].eligible, vec![0, 1, 2, 3]);
        // Level 25 (to 50): 3*25=75 eligible 1,2,3.
        assert_eq!(pots[1].amount, 75);
        assert_eq!(pots[1].eligible, vec![1, 2, 3]);
        // Level 50 (to 100): 2*50=100 eligible 2,3.
        assert_eq!(pots[2].amount, 100);
        assert_eq!(pots[2].eligible, vec![2, 3]);
    }

    #[test]
    fn zero_contributors_ignored() {
        let pots = compute_pots(&[0, 100, 100, 0], &[false, false, false, false]);
        assert_eq!(total(&pots), 200);
        assert_eq!(pots.len(), 1);
        assert_eq!(pots[0].eligible, vec![1, 2]);
    }

    #[test]
    fn empty_returns_no_pots() {
        let pots = compute_pots(&[0, 0], &[false, false]);
        assert!(pots.is_empty());
    }

    #[test]
    fn dead_top_layer_never_drops_chips() {
        // Seat 0 puts in 100, seat 1 folded after putting in 50. The 50-chip layer above
        // seat 1's contribution is contributed solely by seat 0, who is still in — that is
        // fine. But the contested layer's only non-folded contributor is seat 0, so it is a
        // single-eligible pot. Total must equal 150 and no chip may vanish.
        let pots = compute_pots(&[100, 50], &[false, true]);
        assert_eq!(total(&pots), 150);
        for p in &pots {
            assert!(!p.eligible.is_empty(), "no pot may have empty eligible set");
        }
    }

    #[test]
    fn all_contributors_folded_layer_refunds_not_drops() {
        // Both contributors at the top level folded (seat 1 deeper). The 50 chips seat 1 put
        // in above seat 0's level have no eligible winner (seat 1 folded). They must be
        // refunded / rolled forward, never dropped: total stays 150.
        let pots = compute_pots(&[100, 50], &[true, false]);
        assert_eq!(total(&pots), 150);
        for p in &pots {
            assert!(!p.eligible.is_empty(), "no pot may have empty eligible set");
        }
    }

    #[test]
    fn everyone_folded_refunds_all_chips() {
        // Pathological: every contributor folded. Chips must be refunded, total conserved,
        // and no empty-eligible pot emitted.
        let pots = compute_pots(&[100, 50], &[true, true]);
        assert_eq!(total(&pots), 150);
        for p in &pots {
            assert!(!p.eligible.is_empty(), "no pot may have empty eligible set");
        }
    }
}
