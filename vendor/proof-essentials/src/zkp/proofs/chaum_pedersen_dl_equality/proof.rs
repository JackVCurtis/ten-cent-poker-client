use crate::error::CryptoError;
use crate::fs_absorb;

use super::{Parameters, Statement};

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use crate::zkp::fiat_shamir_rng::FiatShamirRng;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::UniformRand;
use digest::Digest;

#[derive(CanonicalDeserialize, CanonicalSerialize)]
pub struct Proof<C>
where
    C: CurveGroup,
{
    pub(crate) a: C,
    pub(crate) b: C,
    pub(crate) r: C::ScalarField,
}

impl<C: CurveGroup> Proof<C> {
    pub fn verify<D: Digest>(
        &self,
        parameters: &Parameters<C>,
        statement: &Statement<C>,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<(), CryptoError> {
        fs_absorb!(
            fs_rng,
            &b"chaum_pedersen"[..],
            parameters.g,
            parameters.h,
            statement.0,
            statement.1
        );
        fs_absorb!(fs_rng, &self.a, &self.b);

        let c = C::ScalarField::rand(fs_rng);

        // g * r ==? a + x*c
        if parameters.g.mul_bigint(self.r.into_bigint())
            != self.a + statement.0.mul_bigint(c.into_bigint())
        {
            return Err(CryptoError::ProofVerificationError(String::from(
                "Chaum-Pedersen",
            )));
        }

        // h * r ==? b + y*c
        if parameters.h.mul_bigint(self.r.into_bigint())
            != self.b + statement.1.mul_bigint(c.into_bigint())
        {
            return Err(CryptoError::ProofVerificationError(String::from(
                "Chaum-Pedersen",
            )));
        }

        Ok(())
    }
}
