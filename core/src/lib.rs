// Ten-Cent Poker — verified synchronous core.
//
// Everything inside `verus! { ... }` is machine-checked by Verus: the proofs are
// discharged by the Z3 SMT solver at verification time and erased at runtime.
// Async networking, wallet RPC, and the GUI live in the sibling cargo crates and
// are deliberately *out* of scope for verification (per project requirements).
//
// Verify (from the repo root):   ./tools/verus core/src/lib.rs
// Expected:                      verification results:: verified: N errors: 0
//
// Negative test (confirms a spec is load-bearing): delete the `forall` line in
// `winner_by_stack`'s loop `invariant` and re-run — verification must then FAIL.

use vstd::prelude::*;

verus! {

/// Chips a player must put in to call: the gap between the table's current bet and
/// what the player has already committed this round.
///
/// Verified: the result is exactly the difference, and adding it back reaches the
/// current bet (no chips created or destroyed).
pub fn amount_to_call(current_bet: u64, already_in: u64) -> (r: u64)
    requires
        already_in <= current_bet,
    ensures
        r == current_bet - already_in,
        already_in + r == current_bet,
{
    current_bet - already_in
}

/// Index of the seat holding the largest stack (the chip leader; first seat wins ties).
///
/// Verified: the returned index is in bounds and no seat has a strictly larger stack.
/// The loop `invariant` carrying the `forall` is load-bearing — it is what lets Verus
/// conclude the postcondition once the loop has scanned every seat.
pub fn winner_by_stack(stacks: &Vec<u64>) -> (idx: usize)
    requires
        stacks.len() > 0,
    ensures
        idx < stacks.len(),
        forall|j: int| 0 <= j < stacks.len() ==> stacks@[j] <= stacks@[idx as int],
{
    let mut best: usize = 0;
    let mut i: usize = 1;
    while i < stacks.len()
        invariant
            0 <= best < stacks.len(),
            1 <= i <= stacks.len(),
            forall|j: int| 0 <= j < i ==> stacks@[j] <= stacks@[best as int],
        decreases stacks.len() - i,
    {
        if stacks[i] > stacks[best] {
            best = i;
        }
        i = i + 1;
    }
    best
}

/// Spec-level sum of the first `n` per-player contributions. This is the ground
/// truth the executable `total_pot` below is proved equal to: it is what "the pot"
/// *means*, defined recursively with no possibility of arithmetic overflow or loss.
pub open spec fn sum_contribs(contribs: Seq<u64>, n: int) -> int
    decreases n,
{
    if n <= 0 {
        0
    } else {
        sum_contribs(contribs, n - 1) + contribs[n - 1] as int
    }
}

/// Fold every player's per-round contribution into the single total pot.
///
/// Verified — chip conservation: the returned pot equals the spec sum of *all*
/// contributions (`ensures pot as int == sum_contribs(.., len)`), so not a single
/// chip is created or destroyed by the fold. The `requires` bounds the running
/// total so the `+=` cannot overflow u64; without it the addition would be
/// rejected, which is itself a (compile-time) conservation guarantee.
///
/// The loop `invariant` carrying `pot as int == sum_contribs(contribs@, i as int)`
/// is LOAD-BEARING: it ties the executable running total to the recursive spec at
/// every step, and it is the only fact that lets Verus discharge the postcondition
/// when the loop exits with `i == len`.
///
/// Negative test (confirms the invariant is load-bearing): delete the line
/// `pot as int == sum_contribs(contribs@, i as int),` from the loop `invariant`
/// and re-run ./verify.sh — Verus can no longer relate `pot` to the spec sum and
/// verification FAILS on the postcondition. Likewise, deleting the running-total
/// bound invariant reintroduces a possible-overflow error on `pot = pot + ...`.
pub fn total_pot(contribs: &Vec<u64>) -> (pot: u64)
    requires
        sum_contribs(contribs@, contribs.len() as int) <= u64::MAX,
    ensures
        pot as int == sum_contribs(contribs@, contribs.len() as int),
{
    let mut pot: u64 = 0;
    let mut i: usize = 0;
    proof {
        // The full-vector sum is non-negative, so the empty prefix (0) is <= it:
        // establishes the running-total bound invariant on loop entry.
        sum_contribs_monotone(contribs@, 0, contribs.len() as int);
    }
    while i < contribs.len()
        invariant
            0 <= i <= contribs.len(),
            // Load-bearing: the running total is exactly the spec sum so far.
            pot as int == sum_contribs(contribs@, i as int),
            // Running-total bound: keeps the next `+=` overflow-free. `sum_contribs`
            // is monotone, so the prefix sum never exceeds the full-vector sum, and
            // the full-vector sum is bounded by u64::MAX (carried from `requires`).
            sum_contribs(contribs@, contribs.len() as int) <= u64::MAX,
            sum_contribs(contribs@, i as int) <= sum_contribs(contribs@, contribs.len() as int),
        decreases contribs.len() - i,
    {
        proof {
            // Unfold one step of the spec sum: sum(i+1) == sum(i) + contribs[i].
            assert(sum_contribs(contribs@, i + 1) == sum_contribs(contribs@, i as int) + contribs@[i as int] as int);
            // Monotonicity gives sum(i+1) <= sum(len) <= u64::MAX, which (combined
            // with pot as int == sum(i)) proves `pot + contribs[i]` cannot overflow.
            sum_contribs_monotone(contribs@, i + 1, contribs.len() as int);
            assert(pot as int + contribs@[i as int] as int == sum_contribs(contribs@, i + 1));
            assert(pot as int + contribs@[i as int] as int <= u64::MAX);
        }
        pot = pot + contribs[i];
        i = i + 1;
    }
    pot
}

/// `sum_contribs` is monotone in its prefix length: extending the prefix can only
/// add (non-negative u64) chips, never remove them. Used by `total_pot` to show the
/// running total stays within the full-vector sum (hence within `u64::MAX`).
proof fn sum_contribs_monotone(contribs: Seq<u64>, lo: int, hi: int)
    requires
        0 <= lo <= hi,
    ensures
        sum_contribs(contribs, lo) <= sum_contribs(contribs, hi),
    decreases hi - lo,
{
    if lo < hi {
        sum_contribs_monotone(contribs, lo, hi - 1);
        // sum_contribs(contribs, hi) == sum_contribs(contribs, hi - 1) + contribs[hi-1] (>= 0)
    }
}

} // verus!
