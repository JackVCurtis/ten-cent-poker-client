//! Fiat-Shamir absorption helper.
//!
//! Replaces the arkworks-0.3 `fs_rng.absorb(&to_bytes![a, b, c]?)` pattern that
//! the proofs/arguments layer relied on. `ToBytes` / the `to_bytes!` macro no
//! longer exist in arkworks 0.6, so absorption now goes through
//! `CanonicalSerialize::serialize_compressed`.
//!
//! Usage (drop-in for `fs_rng.absorb(&to_bytes![a, b, c]?)`):
//! ```ignore
//! use crate::fs_absorb;
//! fs_absorb!(fs_rng, &a, &b, &c);
//! ```
//! Each argument must implement [`ark_serialize::CanonicalSerialize`]. Byte
//! slices / `&[u8]` literals (domain-separation tags like `b"chaum_pedersen"`)
//! implement it too, so they can be mixed in exactly as before.
//!
//! For a single value you can also call [`serialize_to_bytes`] directly to get
//! the compressed byte vector.

use ark_serialize::CanonicalSerialize;

/// Serialize a single `CanonicalSerialize` value to its compressed byte vector.
///
/// This is the per-value primitive behind [`fs_absorb!`]; the macro concatenates
/// the output of this for each argument and feeds the result to `fs_rng.absorb`.
#[inline]
pub fn serialize_to_bytes<T: CanonicalSerialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.compressed_size());
    value
        .serialize_compressed(&mut bytes)
        .expect("serializing to a Vec<u8> is infallible");
    bytes
}

/// Absorb one or more `CanonicalSerialize` values into a Fiat-Shamir RNG.
///
/// Drop-in replacement for the old `fs_rng.absorb(&to_bytes![a, b, c]?)`:
/// `fs_absorb!(fs_rng, &a, &b, &c)`. Each value is `serialize_compressed`-ed and
/// the concatenated bytes are absorbed in a single `absorb` call, matching the
/// original (which built one contiguous byte buffer via `to_bytes!`).
#[macro_export]
macro_rules! fs_absorb {
    ($fs_rng:expr, $($value:expr),+ $(,)?) => {{
        let mut __fs_bytes: Vec<u8> = Vec::new();
        $(
            __fs_bytes.extend_from_slice(
                &$crate::zkp::absorb::serialize_to_bytes($value),
            );
        )+
        $fs_rng.absorb(&__fs_bytes);
    }};
}
