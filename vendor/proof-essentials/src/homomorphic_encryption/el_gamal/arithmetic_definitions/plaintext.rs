use super::super::Plaintext;

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::Zero;
use ark_std::{rand::Rng, UniformRand};
use std::ops::Mul;

impl<C: CurveGroup> Mul<C::ScalarField> for Plaintext<C> {
    type Output = Self;
    fn mul(self, x: C::ScalarField) -> Self::Output {
        // 0.6: `affine * scalar` yields a projective `Group`; convert back.
        Self(self.0.mul(x).into_affine())
    }
}

impl<C: CurveGroup> std::ops::Add<Plaintext<C>> for Plaintext<C> {
    type Output = Self;

    fn add(self, _rhs: Self) -> Self {
        // 0.6: `affine + affine` yields a projective `Group`; convert back.
        Self((self.0 + _rhs.0).into_affine())
    }
}

impl<C: CurveGroup> UniformRand for Plaintext<C> {
    fn rand<R: Rng + ?Sized>(rng: &mut R) -> Self {
        Self(C::rand(rng).into_affine())
    }
}

impl<C: CurveGroup> Zero for Plaintext<C> {
    fn zero() -> Self {
        Self(C::Affine::zero())
    }

    fn is_zero(&self) -> bool {
        self.0 == C::Affine::zero()
    }
}
