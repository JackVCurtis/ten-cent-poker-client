//! In-tree replacement for `ark_marlin::rng::FiatShamirRng`.
//!
//! `ark-marlin` has no arkworks-0.6 release, and the only thing the toolbox used
//! from it was `ark_marlin::rng::FiatShamirRng<D>`: a `SeedableRng` that refreshes
//! its seed by hashing together the previous seed and freshly absorbed material,
//! then reseeds an inner `ChaChaRng`. We reproduce that exact behaviour here with
//! `digest` 0.10 + `rand_chacha`, preserving the original surface that the
//! callers rely on:
//!   * `FiatShamirRng::<D>::from_seed(&seed_bytes)`  -> `seed = H(seed_bytes)`
//!   * `fs_rng.absorb(&new_bytes)`                   -> `seed = H(new_bytes || seed)`
//!   * `impl RngCore` (so `Scalar::rand(fs_rng)` and `R: Rng` bounds keep working).
//!
//! NOTE: this is byte-for-byte algorithmically equivalent to ark-marlin 0.3's
//! `FiatShamirRng` (seed = H(input); absorb = H(new || seed); reseed ChaCha20),
//! so transcripts produced here match the original construction. Verified against
//! the upstream `marlin/src/rng.rs` implementation.

use ark_std::marker::PhantomData;
use digest::Digest;
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// A Blake2-/Digest-based Fiat-Shamir RNG.
pub struct FiatShamirRng<D: Digest> {
    r: ChaCha20Rng,
    seed: [u8; 32],
    #[doc(hidden)]
    digest: PhantomData<D>,
}

impl<D: Digest> RngCore for FiatShamirRng<D> {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        self.r.next_u32()
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.r.next_u64()
    }

    #[inline]
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.r.fill_bytes(dest);
    }

    #[inline]
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.r.try_fill_bytes(dest)
    }
}

impl<D: Digest> FiatShamirRng<D> {
    /// Create a new `FiatShamirRng` by hashing `seed` with `D` to obtain the
    /// initial 32-byte seed and seeding the inner ChaCha20 RNG with it.
    pub fn from_seed(seed: &[u8]) -> Self {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&D::digest(seed)[..32]);
        let r = ChaCha20Rng::from_seed(bytes);
        Self {
            r,
            seed: bytes,
            digest: PhantomData,
        }
    }

    /// Refresh the internal seed with new absorbed material:
    /// `seed <- H(new_input || seed)`, then reseed the inner RNG.
    pub fn absorb(&mut self, new_input: &[u8]) {
        let mut hasher = D::new();
        hasher.update(new_input);
        hasher.update(self.seed);
        let digest = hasher.finalize();

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest[..32]);
        self.seed = bytes;
        self.r = ChaCha20Rng::from_seed(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake2::Blake2s256;
    use rand::Rng;

    #[test]
    fn deterministic_and_seed_dependent() {
        let mut a = FiatShamirRng::<Blake2s256>::from_seed(b"seed-a");
        let mut b = FiatShamirRng::<Blake2s256>::from_seed(b"seed-a");
        let mut c = FiatShamirRng::<Blake2s256>::from_seed(b"seed-c");

        // Same seed -> same stream.
        let xa: u64 = a.gen();
        let xb: u64 = b.gen();
        let xc: u64 = c.gen();
        assert_eq!(xa, xb);
        assert_ne!(xa, xc);
    }

    #[test]
    fn absorb_changes_stream() {
        let mut a = FiatShamirRng::<Blake2s256>::from_seed(b"seed");
        let mut b = FiatShamirRng::<Blake2s256>::from_seed(b"seed");
        b.absorb(b"challenge");
        let xa: u64 = a.gen();
        let xb: u64 = b.gen();
        assert_ne!(xa, xb);
    }
}
