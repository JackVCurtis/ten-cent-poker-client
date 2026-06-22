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

} // verus!
