//! Zero-knowledge cryptography for decentralized card dealing (mental poker) and the
//! on-chain settlement proof, built on the `arkworks` ecosystem over **BN254**.
//!
//! BN254 (alt_bn128) is chosen because the EVM exposes it through the pairing precompiles
//! (0x06/0x07/0x08), so the Groth16 settlement proof verifies cheaply on-chain — the same
//! curve the contract's `Verifier.sol` will target.
//!
//! This module is a scaffold: it pins the proving stack and proves it builds end-to-end.
//! The Barnett–Smart shuffle protocol and the settlement circuit land on top of these types.

use ark_bn254::{Fr, G1Projective};
use ark_ec::PrimeGroup;
use ark_ff::PrimeField;

/// Smoke check that the BN254 group/scalar stack is wired up: returns `g · x`, the scalar
/// multiple of the BN254 G1 generator. (A building block for ElGamal card masking and
/// Pedersen commitments — not a commitment scheme on its own.)
pub fn g1_scalar_mul(x: u64) -> G1Projective {
    G1Projective::generator() * Fr::from(x)
}

/// The bit-size of the BN254 scalar field — a trivial fact used only to force the
/// `ark-ff` field machinery to compile and link.
pub fn scalar_field_bits() -> usize {
    Fr::MODULUS_BIT_SIZE as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_scalar_mul_is_consistent() {
        // g·2 == g + g
        let g = G1Projective::generator();
        let two_g = g1_scalar_mul(2);
        assert_eq!(two_g, g + g);
    }

    #[test]
    fn bn254_scalar_field_is_254_bits() {
        assert_eq!(scalar_field_bits(), 254);
    }
}
