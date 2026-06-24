pub mod error;
pub mod homomorphic_encryption;
pub mod utils;
pub mod vector_commitment;
pub mod zkp;

/// Curve type aliases for the port. The original toolbox was generic over an
/// `ark_ec::ProjectiveCurve` and instantiated tests with the Starknet curve.
/// We retarget to Baby Jubjub (`ark-ed-on-bn254`); its base field is the BN254
/// scalar field, which is why it is the chosen curve for the future settlement
/// circuit.
pub mod curve {
    /// Projective group element (an `ark_ec::CurveGroup`).
    pub type Projective = ark_ed_on_bn254::EdwardsProjective;
    /// Affine group element (an `ark_ec::AffineRepr`).
    pub type Affine = ark_ed_on_bn254::EdwardsAffine;
    /// Scalar field of the group (`r`).
    pub type Fr = ark_ed_on_bn254::Fr;
    /// Base field the curve is defined over (= BN254 scalar field).
    pub type Fq = ark_ed_on_bn254::Fq;
}
