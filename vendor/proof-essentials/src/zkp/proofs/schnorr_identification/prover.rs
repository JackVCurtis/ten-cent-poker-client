use crate::error::CryptoError;
use crate::fs_absorb;

use super::{proof::Proof, Parameters, Statement, Witness};

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use crate::zkp::fiat_shamir_rng::FiatShamirRng;
use ark_std::rand::Rng;
use ark_std::UniformRand;
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
        pp: &Parameters<C>,
        statement: &Statement<C>,
        witness: &Witness<C>,
        fs_rng: &mut FiatShamirRng<D>,
    ) -> Result<Proof<C>, CryptoError> {
        let random = C::ScalarField::rand(rng);

        let random_commit = pp.mul_bigint(random.into_bigint());

        fs_absorb!(
            fs_rng,
            &b"schnorr_identity"[..],
            pp,
            statement,
            &random_commit
        );

        let c = C::ScalarField::rand(fs_rng);

        let opening = random - c * witness;

        Ok(Proof {
            random_commit,
            opening,
        })
    }
}
