use super::{Parameters, Statement};
use crate::error::CryptoError;
use crate::fs_absorb;
use crate::zkp::fiat_shamir_rng::FiatShamirRng;

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::UniformRand;
use digest::Digest;

#[derive(Copy, Clone, CanonicalDeserialize, CanonicalSerialize, Debug, PartialEq, Eq)]
pub struct Proof<C>
where
    C: CurveGroup,
{
    pub(crate) random_commit: C,
    pub(crate) opening: C::ScalarField,
}

impl<C: CurveGroup> Proof<C> {
    pub fn verify<D: Digest>(
        &self,
        pp: &Parameters<C>,
        statement: &Statement<C>,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<(), CryptoError> {
        fs_absorb!(
            fs_rng,
            &b"schnorr_identity"[..],
            pp,
            statement,
            &self.random_commit
        );

        let c = C::ScalarField::rand(fs_rng);

        // 0.3 `affine.mul(scalar.into_repr())` -> 0.6 `affine.mul_bigint(scalar.into_bigint())`;
        // both yield a projective `C`, so the comparison stays in the group.
        if pp.mul_bigint(self.opening.into_bigint()) + statement.mul_bigint(c.into_bigint())
            != self.random_commit
        {
            return Err(CryptoError::ProofVerificationError(String::from(
                "Schnorr Identification",
            )));
        }

        Ok(())
    }
}
