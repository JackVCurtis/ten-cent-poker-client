use super::super::Ciphertext;
use ark_ec::{AffineRepr, CurveGroup};
use ark_std::{UniformRand, Zero};
use rand::Rng;
use std::ops::Mul;

impl<C: CurveGroup> std::ops::Add<Ciphertext<C>> for Ciphertext<C> {
    type Output = Self;

    fn add(self, _rhs: Self) -> Self {
        // 0.6: `affine + affine` yields a projective `Group`; convert back.
        Self(
            (self.0 + _rhs.0).into_affine(),
            (self.1 + _rhs.1).into_affine(),
        )
    }
}

impl<C: CurveGroup> Mul<C::ScalarField> for Ciphertext<C> {
    type Output = Self;
    fn mul(self, x: C::ScalarField) -> Self::Output {
        // 0.6: `affine * scalar` yields a projective `Group`; convert back.
        Self(self.0.mul(x).into_affine(), self.1.mul(x).into_affine())
    }
}

impl<C: CurveGroup> std::iter::Sum for Ciphertext<C> {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self(C::Affine::zero(), C::Affine::zero()), |a, b| a + b)
    }
}

impl<C: CurveGroup> Zero for Ciphertext<C> {
    fn zero() -> Self {
        Self(C::Affine::zero(), C::Affine::zero())
    }

    fn is_zero(&self) -> bool {
        *self == Self(C::Affine::zero(), C::Affine::zero())
    }
}

impl<C: CurveGroup> UniformRand for Ciphertext<C> {
    fn rand<R: Rng + ?Sized>(rng: &mut R) -> Self {
        let c0 = C::rand(rng).into_affine();
        let c1 = C::rand(rng).into_affine();

        Self(c0, c1)
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::curve::Projective;
    use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
    use ark_std::UniformRand;
    use rand::thread_rng;

    #[test]
    fn serialize_unserialize_test() {
        let mut rng = thread_rng();
        let c0 = Projective::rand(&mut rng).into_affine();
        let c1 = Projective::rand(&mut rng).into_affine();

        let cipher = Ciphertext::<Projective>(c0, c1);

        let mut serialized = vec![0; cipher.compressed_size()];
        cipher.serialize_compressed(&mut serialized[..]).unwrap();

        let deserialized = Ciphertext::<Projective>::deserialize_compressed(&serialized[..]).unwrap();
        assert_eq!(cipher, deserialized);
    }
}
