#[cfg(test)]
mod test {

    use crate::error::CryptoError;
    use crate::zkp::fiat_shamir_rng::FiatShamirRng;
    use crate::zkp::proofs::chaum_pedersen_dl_equality;
    use crate::zkp::proofs::chaum_pedersen_dl_equality::DLEquality;
    use crate::zkp::ArgumentOfKnowledge;
    use ark_ec::{AffineRepr, CurveGroup};
    use ark_ff::PrimeField;
    use ark_std::{rand::thread_rng, UniformRand};
    use blake2::Blake2s256;
    use rand::{prelude::ThreadRng, Rng};

    // Baby Jubjub via ark-ed-on-bn254.
    type AffinePoint = crate::curve::Affine;
    type Curve = crate::curve::Projective;
    type Scalar = crate::curve::Fr;
    type Parameters<'a> = chaum_pedersen_dl_equality::Parameters<'a, Curve>;
    type FS = FiatShamirRng<Blake2s256>;

    fn setup<R: Rng>(rng: &mut R) -> (AffinePoint, AffinePoint) {
        (
            Curve::rand(rng).into_affine(),
            Curve::rand(rng).into_affine(),
        )
    }

    fn test_template() -> (ThreadRng, AffinePoint, AffinePoint, Scalar) {
        let mut rng = thread_rng();
        let (g, h) = setup(&mut rng);
        let secret = Scalar::rand(&mut rng);

        (rng, g, h, secret)
    }

    #[test]
    fn test_honest_prover() {
        let (mut rng, g, h, secret) = test_template();

        let point_a = g.mul_bigint(secret.into_bigint()).into_affine();
        let point_b = h.mul_bigint(secret.into_bigint()).into_affine();

        let crs = Parameters::new(&g, &h);
        let statement = chaum_pedersen_dl_equality::Statement::<Curve>::new(&point_a, &point_b);
        let witness = &secret;

        let mut fs_rng = FS::from_seed(b"Initialised with some input");
        let proof =
            DLEquality::<Curve>::prove(&mut rng, &crs, &statement, &witness, &mut fs_rng).unwrap();

        let mut fs_rng = FS::from_seed(b"Initialised with some input");
        assert_eq!(
            DLEquality::<Curve>::verify(&crs, &statement, &proof, &mut fs_rng),
            Ok(())
        );

        assert_ne! {point_a, point_b};
    }

    #[test]
    fn test_malicious_prover() {
        let (mut rng, g, h, secret) = test_template();

        let point_a = g.mul_bigint(secret.into_bigint()).into_affine();
        let point_b = h.mul_bigint(secret.into_bigint()).into_affine();

        let another_scalar = Scalar::rand(&mut rng);

        let crs = Parameters::new(&g, &h);
        let statement = chaum_pedersen_dl_equality::Statement::<Curve>::new(&point_a, &point_b);

        let wrong_witness = &another_scalar;

        let mut fs_rng = FS::from_seed(b"Initialised with some input");
        let invalid_proof =
            DLEquality::<Curve>::prove(&mut rng, &crs, &statement, &wrong_witness, &mut fs_rng)
                .unwrap();

        let mut fs_rng = FS::from_seed(b"Initialised with some input");
        assert_eq!(
            DLEquality::<Curve>::verify(&crs, &statement, &invalid_proof, &mut fs_rng),
            Err(CryptoError::ProofVerificationError(String::from(
                "Chaum-Pedersen"
            )))
        );
    }
}
