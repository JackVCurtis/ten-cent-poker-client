use crate::error::CryptoError;
use crate::fs_absorb;

use super::proof::Proof;
use super::{Parameters, Statement, Witness};

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use crate::zkp::fiat_shamir_rng::FiatShamirRng;
use ark_std::{rand::Rng, UniformRand};
use digest::Digest;

use std::marker::PhantomData;

pub struct Prover<C>
where
    C: CurveGroup,
{
    phantom: PhantomData<C>,
}

impl<C> Prover<C>
where
    C: CurveGroup,
{
    pub fn create_proof<R: Rng, D: Digest>(
        rng: &mut R,
        parameters: &Parameters<C>,
        statement: &Statement<C>,
        witness: &Witness<C>,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<Proof<C>, CryptoError> {
        fs_absorb!(
            fs_rng,
            &b"chaum_pedersen"[..],
            parameters.g,
            parameters.h,
            statement.0,
            statement.1
        );

        let omega = C::ScalarField::rand(rng);
        let a = parameters.g.mul_bigint(omega.into_bigint());
        let b = parameters.h.mul_bigint(omega.into_bigint());

        fs_absorb!(fs_rng, &a, &b);

        let c = C::ScalarField::rand(fs_rng);

        let r = omega + c * *witness;

        Ok(Proof { a, b, r })
    }
}
