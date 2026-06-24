use crate::error::CryptoError;
use crate::vector_commitment::HomomorphicCommitmentScheme;

use ark_ec::CurveGroup;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::marker::PhantomData;
use rand::Rng;

pub mod arithmetic_definitions;
mod tests;

pub struct PedersenCommitment<C: CurveGroup> {
    _curve: PhantomData<C>,
}

#[derive(Clone, CanonicalSerialize, CanonicalDeserialize, Debug)]
pub struct CommitKey<C: CurveGroup> {
    g: Vec<C::Affine>,
    h: C::Affine,
}

impl<C: CurveGroup> CommitKey<C> {
    pub fn new(g: Vec<C::Affine>, h: C::Affine) -> Self {
        Self { g, h }
    }
}

// NOTE (0.3 -> 0.6): the manual `ToBytes` impls for `CommitKey` / `Commitment`
// are dropped; `ToBytes` no longer exists in arkworks 0.6. Absorption uses the
// derived `CanonicalSerialize`.

#[derive(Clone, Copy, Debug, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct Commitment<C: CurveGroup>(pub C::Affine);

impl<C: CurveGroup> HomomorphicCommitmentScheme<C::ScalarField> for PedersenCommitment<C> {
    type CommitKey = CommitKey<C>;
    type Commitment = Commitment<C>;

    fn setup<R: Rng>(public_randomess: &mut R, len: usize) -> CommitKey<C> {
        let mut g = Vec::with_capacity(len);
        for _ in 0..len {
            g.push(C::rand(public_randomess).into_affine());
        }
        let h = C::rand(public_randomess).into_affine();
        CommitKey::<C> { g, h }
    }

    fn commit(
        commit_key: &CommitKey<C>,
        x: &Vec<C::ScalarField>,
        r: C::ScalarField,
    ) -> Result<Self::Commitment, CryptoError> {
        if x.len() > commit_key.g.len() {
            return Err(CryptoError::CommitmentLengthError(
                String::from("Pedersen"),
                x.len(),
                commit_key.g.len(),
            ));
        }

        // 0.6 `VariableBaseMSM::msm` takes scalar *field elements* directly (no
        // `into_repr()` to bigints, as in 0.3's `multi_scalar_mul`).
        //
        // 0.3's `multi_scalar_mul` silently zipped to the shorter of bases/scalars,
        // so a short commitment (x shorter than the commit key) was fine. 0.6's
        // `msm` instead *errors* on a length mismatch, so we slice the generator
        // bases down to `x.len()` to preserve the original short-commitment
        // semantics (the unused g_i are effectively multiplied by 0).
        let scalars = [&[r], x.as_slice()].concat();
        let bases = [&[commit_key.h], &commit_key.g[..x.len()]].concat();

        let msm = C::msm(&bases, &scalars).map_err(|min_len| {
            CryptoError::CommitmentLengthError(String::from("Pedersen-msm"), scalars.len(), min_len)
        })?;

        Ok(Commitment(msm.into_affine()))
    }
}
