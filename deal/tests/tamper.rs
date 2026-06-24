//! Adversarial tamper tests for the trustless Barnett–Smart deal (M2).
//!
//! These tests do not check the happy path (that lives in `mental.rs` and in the
//! vendored crate's own tests). Instead they assert that the zero-knowledge proofs
//! actually *bind*: that an attacker who substitutes a forged value, a mismatched
//! ciphertext/output, or a single flipped proof byte is **rejected** by verification.
//!
//! Each test asserts the relevant `verify_*` / `unmask` call returns `Err`. Any case
//! that fails to be rejected is a security bug.
//!
//! We drive the protocol through the same concrete types the production deal uses
//! (`poker_deal::mental`), exercising the underlying `CardProtocol` (the
//! `BarnettSmartProtocol` impl) so we can construct precise tampered inputs.

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::UniformRand;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};

use barnett_smart_card_protocol::error::CardProtocolError;
use barnett_smart_card_protocol::BarnettSmartProtocol;

use proof_essentials::error::CryptoError;
use proof_essentials::utils::permutation::Permutation;
use proof_essentials::utils::rand::sample_vector;

use rand::thread_rng;

use poker_deal::mental::{
    CardProtocol, MaskedCard, MaskedPlaintext, Parameters, ProofShuffle, PublicKey, RevealToken,
    Scalar, SecretKey,
};

const M: usize = 4;
const N: usize = 13;

/// Set up parameters + `num_players` keypairs and the (locally computed) aggregate key.
fn setup(
    num_players: usize,
) -> (
    Parameters,
    Vec<(PublicKey, SecretKey, Scalar)>,
    PublicKey,
) {
    let rng = &mut thread_rng();
    let parameters = CardProtocol::setup(rng, M, N).unwrap();

    let mut players = Vec::with_capacity(num_players);
    let mut aggregate = PublicKey::zero();
    for _ in 0..num_players {
        let (pk, sk) = CardProtocol::player_keygen(rng, &parameters).unwrap();
        let info = Scalar::rand(rng);
        aggregate = (aggregate + pk).into_affine();
        players.push((pk, sk, info));
    }
    (parameters, players, aggregate)
}

/// Assert that flipping *any single byte* of a valid proof's serialization breaks it:
/// for every byte position, the corrupted bytes must either (i) fail to deserialize as a
/// proof, or (ii) deserialize but be rejected by `verify`. Both outcomes are a rejection
/// from an honest verifier's point of view; the security property is that **no** single
/// flipped byte yields an accepted proof. `label` names the proof type for diagnostics.
fn assert_no_single_byte_flip_verifies<P, V>(proof: &P, label: &str, mut verify: V)
where
    P: CanonicalSerialize + CanonicalDeserialize,
    V: FnMut(&P) -> Result<(), CryptoError>,
{
    let mut bytes = Vec::new();
    proof.serialize_compressed(&mut bytes).unwrap();
    assert!(!bytes.is_empty(), "proof serialized to zero bytes");

    let mut accepted_after_deserialize = 0usize;
    for i in 0..bytes.len() {
        for bit in 0..8u8 {
            let mut corrupted = bytes.clone();
            corrupted[i] ^= 1 << bit;
            match P::deserialize_compressed(&corrupted[..]) {
                Err(_) => {
                    // Corrupted bytes are not even a valid proof encoding -> rejected.
                }
                Ok(tampered) => {
                    accepted_after_deserialize += 1;
                    let res = verify(&tampered);
                    assert!(
                        res.is_err(),
                        "SECURITY BUG: {label} proof verified after flipping byte {i} bit \
                         {bit}: {res:?}"
                    );
                }
            }
        }
    }

    // Be sure the test actually exercised the verify path at least once (i.e. there
    // exist single-bit flips that survive deserialization and are then rejected by
    // verify), not only deserialization rejections.
    assert!(
        accepted_after_deserialize > 0,
        "{label}: no single-bit flip survived deserialization to exercise verify()",
    );
}

/// (a) A shuffle proof that does NOT correspond to the claimed output deck is rejected.
///
/// Two flavours:
///   - the output deck is replaced wholesale by an unrelated deck (the proof is for a
///     different output);
///   - a single card of the genuine shuffled output is swapped for a different
///     ciphertext (the proof no longer matches the claimed output).
#[test]
fn shuffle_proof_rejected_for_wrong_output_deck() {
    let rng = &mut thread_rng();
    let (parameters, _players, aggregate) = setup(3);

    let deck: Vec<MaskedCard> = sample_vector(rng, M * N);
    let permutation = Permutation::new(rng, M * N);
    let masking_factors: Vec<Scalar> = sample_vector(rng, M * N);

    let (shuffled, proof) = CardProtocol::shuffle_and_remask(
        rng,
        &parameters,
        &aggregate,
        &deck,
        &masking_factors,
        &permutation,
    )
    .unwrap();

    // Sanity: the genuine shuffle verifies.
    assert_eq!(
        Ok(()),
        CardProtocol::verify_shuffle(&parameters, &aggregate, &deck, &shuffled, &proof)
    );

    // Tamper 1: an entirely unrelated output deck.
    let bogus_output: Vec<MaskedCard> = sample_vector(rng, M * N);
    let res =
        CardProtocol::verify_shuffle(&parameters, &aggregate, &deck, &bogus_output, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: shuffle proof accepted an unrelated output deck: {res:?}"
    );

    // Tamper 2: swap exactly one card in the genuine output for a different ciphertext.
    let mut one_card_swapped = shuffled.clone();
    let foreign: MaskedCard = MaskedCard::rand(rng);
    // Make sure we actually changed it.
    assert_ne!(one_card_swapped[0], foreign);
    one_card_swapped[0] = foreign;
    let res = CardProtocol::verify_shuffle(
        &parameters,
        &aggregate,
        &deck,
        &one_card_swapped,
        &proof,
    );
    assert!(
        res.is_err(),
        "SECURITY BUG: shuffle proof accepted an output deck with one swapped card: {res:?}"
    );
}

/// (a') A shuffle proof is also bound to its *input* deck: verifying against a different
/// original deck must fail.
#[test]
fn shuffle_proof_rejected_for_wrong_input_deck() {
    let rng = &mut thread_rng();
    let (parameters, _players, aggregate) = setup(3);

    let deck: Vec<MaskedCard> = sample_vector(rng, M * N);
    let permutation = Permutation::new(rng, M * N);
    let masking_factors: Vec<Scalar> = sample_vector(rng, M * N);

    let (shuffled, proof) = CardProtocol::shuffle_and_remask(
        rng,
        &parameters,
        &aggregate,
        &deck,
        &masking_factors,
        &permutation,
    )
    .unwrap();

    let wrong_input: Vec<MaskedCard> = sample_vector(rng, M * N);
    let res =
        CardProtocol::verify_shuffle(&parameters, &aggregate, &wrong_input, &shuffled, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: shuffle proof accepted a wrong input deck: {res:?}"
    );
}

/// (b) A forged / incorrect reveal token (wrong partial decryption) is rejected by its
/// Chaum–Pedersen check.
///
/// We compute a genuine masked card, then for one player substitute a reveal token that
/// is NOT `sk * c0` (we use a random point, and separately a token derived from the
/// wrong secret key). The standalone `verify_reveal` must reject it, and `unmask`
/// (which verifies every token internally) must error rather than silently decrypt.
#[test]
fn forged_reveal_token_rejected_by_chaum_pedersen() {
    let rng = &mut thread_rng();
    let (parameters, players, aggregate) = setup(4);

    let card = MaskedPlaintext::rand(rng);
    let alpha = Scalar::rand(rng);
    let (masked, _mask_proof) =
        CardProtocol::mask(rng, &parameters, &aggregate, &card, &alpha).unwrap();

    // Genuine reveal tokens + proofs for every player.
    let mut decryption_key = players
        .iter()
        .map(|(pk, sk, _)| {
            let (token, proof) =
                CardProtocol::compute_reveal_token(rng, &parameters, sk, pk, &masked).unwrap();
            // sanity: genuine token verifies
            assert_eq!(
                Ok(()),
                CardProtocol::verify_reveal(&parameters, pk, &token, &masked, &proof)
            );
            (token, proof, *pk)
        })
        .collect::<Vec<_>>();

    // Forge player 0's token: keep their (now stale) proof + public key, but replace
    // the token with a random group element (a wrong partial decryption).
    let forged_token = RevealToken::rand(rng);
    assert_ne!(decryption_key[0].0, forged_token);

    // Standalone Chaum–Pedersen check must reject the forged token under the real proof.
    let standalone = CardProtocol::verify_reveal(
        &parameters,
        &decryption_key[0].2,
        &forged_token,
        &masked,
        &decryption_key[0].1,
    );
    assert!(
        standalone.is_err(),
        "SECURITY BUG: verify_reveal accepted a forged reveal token: {standalone:?}"
    );

    decryption_key[0].0 = forged_token;
    let res = CardProtocol::unmask(&parameters, &decryption_key, &masked);
    assert_eq!(
        res,
        Err(CardProtocolError::ProofVerificationError(
            CryptoError::ProofVerificationError(String::from("Chaum-Pedersen"))
        )),
        "SECURITY BUG: unmask accepted a forged reveal token instead of rejecting it"
    );
}

/// (b') A reveal token computed with the *wrong secret key* (a different player's key, or
/// a freshly sampled key) does not match the claimed public key and is rejected.
#[test]
fn reveal_token_from_wrong_secret_key_rejected() {
    let rng = &mut thread_rng();
    let (parameters, players, aggregate) = setup(2);

    let card = MaskedPlaintext::rand(rng);
    let alpha = Scalar::rand(rng);
    let (masked, _) = CardProtocol::mask(rng, &parameters, &aggregate, &card, &alpha).unwrap();

    // Player 0 advertises pk0 but computes the token+proof with a foreign secret key.
    let pk0 = players[0].0;
    let wrong_sk: SecretKey = Scalar::rand(rng);
    let (token, proof) =
        CardProtocol::compute_reveal_token(rng, &parameters, &wrong_sk, &pk0, &masked).unwrap();

    // The proof is internally consistent for `wrong_sk`, but it is checked against the
    // advertised public key `pk0`, so verification must fail.
    let res = CardProtocol::verify_reveal(&parameters, &pk0, &token, &masked, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: reveal token from the wrong secret key verified against pk0: {res:?}"
    );
}

/// (c) A masking proof with a mismatched ciphertext is rejected.
///
/// The masking proof binds `(card, masked_card)` together (it is a Chaum–Pedersen proof
/// that `masked = mask(card, r)` under the shared key). We verify the genuine proof
/// against (i) a different masked ciphertext and (ii) a different underlying card; both
/// must be rejected.
#[test]
fn masking_proof_rejected_for_mismatched_ciphertext() {
    let rng = &mut thread_rng();
    let (parameters, _players, aggregate) = setup(3);

    let card = MaskedPlaintext::rand(rng);
    let alpha = Scalar::rand(rng);
    let (masked, proof) =
        CardProtocol::mask(rng, &parameters, &aggregate, &card, &alpha).unwrap();

    // Sanity: genuine masking verifies.
    assert_eq!(
        Ok(()),
        CardProtocol::verify_mask(&parameters, &aggregate, &card, &masked, &proof)
    );

    // (i) Mismatched ciphertext: an independent masking of the same card under a fresh r.
    let beta = Scalar::rand(rng);
    let (other_masked, _) =
        CardProtocol::mask(rng, &parameters, &aggregate, &card, &beta).unwrap();
    assert_ne!(masked, other_masked);
    let res = CardProtocol::verify_mask(&parameters, &aggregate, &card, &other_masked, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: masking proof accepted a mismatched ciphertext: {res:?}"
    );

    // (ii) Mismatched plaintext: verify the genuine ciphertext against a *different*
    // underlying card. A correct proof must bind the opened card, so this is rejected.
    let other_card = MaskedPlaintext::rand(rng);
    assert_ne!(card, other_card);
    let res = CardProtocol::verify_mask(&parameters, &aggregate, &other_card, &masked, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: masking proof accepted a different underlying card: {res:?}"
    );

    // (iii) Hand-built bogus ciphertext (one component perturbed) is also rejected.
    let mut tampered = masked;
    tampered.0 = (tampered.0 + parameters_generator_point(&parameters)).into_affine();
    assert_ne!(tampered, masked);
    let res = CardProtocol::verify_mask(&parameters, &aggregate, &card, &tampered, &proof);
    assert!(
        res.is_err(),
        "SECURITY BUG: masking proof accepted a hand-tampered ciphertext: {res:?}"
    );
}

/// Helper: a non-zero point we can add to perturb a ciphertext component. We use the
/// curve generator (always a valid, non-identity point).
fn parameters_generator_point(_pp: &Parameters) -> poker_deal::mental::Point {
    poker_deal::mental::Point::generator()
}

/// (d) Tampering a single byte of an otherwise-valid proof makes verification fail.
///
/// Covers each proof type whose verifier we expose: masking (Chaum–Pedersen), reveal
/// (Chaum–Pedersen), and shuffle (Bayer–Groth). A flipped byte either fails to
/// deserialize (rejection) or, once deserialized, fails the verify check.
#[test]
fn one_flipped_byte_breaks_masking_proof() {
    let rng = &mut thread_rng();
    let (parameters, _players, aggregate) = setup(3);

    let card = MaskedPlaintext::rand(rng);
    let alpha = Scalar::rand(rng);
    let (masked, proof) =
        CardProtocol::mask(rng, &parameters, &aggregate, &card, &alpha).unwrap();
    assert_eq!(
        Ok(()),
        CardProtocol::verify_mask(&parameters, &aggregate, &card, &masked, &proof)
    );

    assert_no_single_byte_flip_verifies(&proof, "masking", |p| {
        CardProtocol::verify_mask(&parameters, &aggregate, &card, &masked, p)
    });
}

#[test]
fn one_flipped_byte_breaks_reveal_proof() {
    let rng = &mut thread_rng();
    let (parameters, players, aggregate) = setup(2);

    let card = MaskedPlaintext::rand(rng);
    let alpha = Scalar::rand(rng);
    let (masked, _) = CardProtocol::mask(rng, &parameters, &aggregate, &card, &alpha).unwrap();

    let (pk, sk, _) = &players[0];
    let (token, proof) =
        CardProtocol::compute_reveal_token(rng, &parameters, sk, pk, &masked).unwrap();
    assert_eq!(
        Ok(()),
        CardProtocol::verify_reveal(&parameters, pk, &token, &masked, &proof)
    );

    assert_no_single_byte_flip_verifies(&proof, "reveal", |p| {
        CardProtocol::verify_reveal(&parameters, pk, &token, &masked, p)
    });
}

#[test]
fn one_flipped_byte_breaks_shuffle_proof() {
    let rng = &mut thread_rng();
    let (parameters, _players, aggregate) = setup(2);

    let deck: Vec<MaskedCard> = sample_vector(rng, M * N);
    let permutation = Permutation::new(rng, M * N);
    let masking_factors: Vec<Scalar> = sample_vector(rng, M * N);

    let (shuffled, proof) = CardProtocol::shuffle_and_remask(
        rng,
        &parameters,
        &aggregate,
        &deck,
        &masking_factors,
        &permutation,
    )
    .unwrap();
    assert_eq!(
        Ok(()),
        CardProtocol::verify_shuffle(&parameters, &aggregate, &deck, &shuffled, &proof)
    );

    // The Bayer–Groth shuffle proof is large and its verifier is expensive, so we don't
    // sweep every byte (as we do for the small Chaum–Pedersen proofs). We probe a set of
    // single-byte flips spread across the encoding; every one must be rejected, either by
    // failing to deserialize or by failing verification. At least one flip must survive
    // deserialization so the verify path is actually exercised.
    let mut bytes = Vec::new();
    proof.serialize_compressed(&mut bytes).unwrap();
    assert!(!bytes.is_empty());

    let mut exercised_verify = false;
    let probes: Vec<usize> = (0..16).map(|k| (k * bytes.len()) / 16).collect();
    for &i in &probes {
        let mut corrupted = bytes.clone();
        corrupted[i] ^= 0x01;
        match ProofShuffle::deserialize_compressed(&corrupted[..]) {
            Err(_) => { /* not a valid proof encoding -> rejected */ }
            Ok(tampered) => {
                exercised_verify = true;
                let res = CardProtocol::verify_shuffle(
                    &parameters,
                    &aggregate,
                    &deck,
                    &shuffled,
                    &tampered,
                );
                assert!(
                    res.is_err(),
                    "SECURITY BUG: shuffle proof verified after flipping byte {i}: {res:?}"
                );
            }
        }
    }
    assert!(
        exercised_verify,
        "shuffle: no probed single-byte flip survived deserialization to exercise verify()"
    );
}
