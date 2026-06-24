use crate::error::CryptoError;
use crate::zkp::fiat_shamir_rng::FiatShamirRng;
use ark_std::rand::Rng;
use digest::Digest;

// FOUNDATION (ported to arkworks 0.6 + Baby Jubjub):
pub mod absorb;
pub mod fiat_shamir_rng;
pub mod transcript;

// PORTED (M2 phase 2): sigma-protocol proofs.
pub mod proofs;

// Bayer-Groth product sub-arguments. Individual submodules that are not yet
// ported (still on the arkworks 0.3 API / ark-marlin FiatShamirRng) are
// cfg-gated inside `arguments/mod.rs`; un-gate each as it is ported.
pub mod arguments;

/// Common interface for a Sigma-protocol / Bayer-Groth-style argument.
///
/// The `FiatShamirRng` type parameter was `ark_marlin::rng::FiatShamirRng<D>`
/// in the original; it is now the in-tree Blake2-based replacement in
/// [`crate::zkp::fiat_shamir_rng`] (same `from_seed` / `absorb` / `Rng` surface).
pub trait ArgumentOfKnowledge {
    type CommonReferenceString;
    type Statement;
    type Witness;
    type Proof;

    fn prove<R: Rng, D: Digest>(
        rng: &mut R,
        common_reference_string: &Self::CommonReferenceString,
        statement: &Self::Statement,
        witness: &Self::Witness,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<Self::Proof, CryptoError>;

    fn verify<D: Digest>(
        common_reference_string: &Self::CommonReferenceString,
        statement: &Self::Statement,
        proof: &Self::Proof,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<(), CryptoError>;
}
