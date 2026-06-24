//! Trustless card dealing (mental poker).
//!
//! The real engine (M2) is the **Barnett–Smart** protocol over **Baby Jubjub**: threshold
//! ElGamal masking, a **Bayer–Groth** zero-knowledge shuffle argument, and cooperative
//! reveal with **Chaum–Pedersen** proofs — ported from Geometry Research's `mental-poker` /
//! `proof-essentials` and retargeted to `ark-ed-on-bn254` so the settlement circuit (M5)
//! can re-check ciphertexts with native constraints. None of that lives here yet.
//!
//! What *is* here is a deterministic, **insecure** placeholder shuffle. Its sole purpose is
//! to let the game engine, networking, and protocol layers be developed and tested
//! end-to-end before the cryptographic deal is wired in behind the same boundary. It must
//! never be used for a real deal: the order is fully determined by a public seed — there is
//! no hiding, no proof, and no decentralization.

use poker_game::Card;

/// The real trustless deal: Barnett–Smart mental poker over Baby Jubjub. This is the
/// additive M2 path that supersedes [`placeholder_shuffled_deck`] for real play; the
/// placeholder stays intact for layer development and tests.
pub mod mental;

/// Per-peer, step-wise distributed driver for [`mental`]: each peer runs its own
/// replica holding only its secret key and advances the deal by exchanging
/// serialized wire messages. This is the layer networked play drives.
pub mod distributed;

/// A full 52-card deck shuffled deterministically from `seed`.
///
/// **PLACEHOLDER ONLY** (see module docs). Fisher–Yates driven by a splitmix64 stream, so
/// the result is a reproducible permutation we can assert against in tests.
pub fn placeholder_shuffled_deck(seed: u64) -> Vec<Card> {
    let mut deck: Vec<Card> = (0..52u8).map(|i| Card::from_index(i).unwrap()).collect();
    let mut state = seed;
    // Fisher–Yates from the top: for each position i, swap with a random j in 0..=i.
    for i in (1..deck.len()).rev() {
        let j = (next_u64(&mut state) % (i as u64 + 1)) as usize;
        deck.swap(i, j);
    }
    deck
}

/// splitmix64 — a tiny, well-distributed deterministic PRNG step.
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn placeholder_deck_is_a_permutation_of_52() {
        let deck = placeholder_shuffled_deck(42);
        assert_eq!(deck.len(), 52);
        let indices: HashSet<u8> = deck.iter().map(|c| c.to_index()).collect();
        assert_eq!(indices.len(), 52, "shuffle must be a permutation");
    }

    #[test]
    fn placeholder_deck_is_deterministic_and_seed_dependent() {
        assert_eq!(placeholder_shuffled_deck(7), placeholder_shuffled_deck(7));
        assert_ne!(placeholder_shuffled_deck(1), placeholder_shuffled_deck(2));
    }
}
