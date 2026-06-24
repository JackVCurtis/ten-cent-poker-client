#[cfg(test)]
mod test {
    use crate::discrete_log_cards;
    use crate::error::CardProtocolError;
    use crate::BarnettSmartProtocol;

    use ark_ec::CurveGroup;
    use ark_ff::UniformRand;
    use ark_serialize::CanonicalSerialize;
    use ark_std::rand::Rng;
    use proof_essentials::error::CryptoError;
    use proof_essentials::utils::permutation::Permutation;
    use proof_essentials::utils::rand::sample_vector;
    use rand::thread_rng;
    use std::iter::Iterator;

    // Choose elliptic curve setting
    type Curve = proof_essentials::curve::Projective;
    type Scalar = proof_essentials::curve::Fr;

    // Instantiate concrete type for our card protocol
    type CardProtocol = discrete_log_cards::DLCards<Curve>;
    type CardParameters = discrete_log_cards::Parameters<Curve>;
    type PublicKey = discrete_log_cards::PublicKey<Curve>;
    type SecretKey = discrete_log_cards::PlayerSecretKey<Curve>;

    type Card = discrete_log_cards::Card<Curve>;
    type MaskedCard = discrete_log_cards::MaskedCard<Curve>;
    type RevealToken = discrete_log_cards::RevealToken<Curve>;

    /// Setup `n` players. We use a Scalar to represent player public information
    fn setup_players<R: Rng>(
        rng: &mut R,
        parameters: &CardParameters,
        num_of_players: usize,
    ) -> (Vec<(PublicKey, SecretKey, Scalar)>, PublicKey) {
        let mut players: Vec<(PublicKey, SecretKey, Scalar)> = Vec::with_capacity(num_of_players);
        let mut expected_shared_key = PublicKey::zero();

        for i in 0..num_of_players {
            let (pk, sk) = CardProtocol::player_keygen(rng, &parameters).unwrap();
            let player_info = Scalar::rand(rng);
            players.push((pk, sk, player_info));
            expected_shared_key = (expected_shared_key + players[i].0).into_affine()
        }

        (players, expected_shared_key)
    }

    #[test]
    fn generate_and_verify_key() {
        let rng = &mut thread_rng();
        let m = 4;
        let n = 13;

        let parameters = CardProtocol::setup(rng, m, n).unwrap();

        let (pk, sk) = CardProtocol::player_keygen(rng, &parameters).unwrap();
        let player_name = b"Alice";

        let p1_keyproof =
            CardProtocol::prove_key_ownership(rng, &parameters, &pk, &sk, &player_name).unwrap();

        assert_eq!(
            Ok(()),
            CardProtocol::verify_key_ownership(&parameters, &pk, &player_name, &p1_keyproof)
        );

        let other_key = Scalar::rand(rng);
        let wrong_proof =
            CardProtocol::prove_key_ownership(rng, &parameters, &pk, &other_key, &player_name)
                .unwrap();

        assert_eq!(
            CardProtocol::verify_key_ownership(&parameters, &pk, &player_name, &wrong_proof),
            Err(CryptoError::ProofVerificationError(String::from(
                "Schnorr Identification"
            )))
        )
    }

    #[test]
    fn aggregate_keys() {
        let rng = &mut thread_rng();
        let m = 4;
        let n = 13;

        let num_of_players = 10;

        let parameters = CardProtocol::setup(rng, m, n).unwrap();

        let (players, expected_shared_key) = setup_players(rng, &parameters, num_of_players);

        let proofs = players
            .iter()
            .map(|player| {
                CardProtocol::prove_key_ownership(rng, &parameters, &player.0, &player.1, &player.2)
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let key_proof_info = players
            .iter()
            .zip(proofs.iter())
            .map(|(player, &proof)| (player.0, proof.clone(), player.2))
            .collect::<Vec<(PublicKey, _, _)>>();

        let test_aggregate =
            CardProtocol::compute_aggregate_key(&parameters, &key_proof_info).unwrap();

        assert_eq!(test_aggregate, expected_shared_key);

        let mut bad_key_proof_pairs = key_proof_info;
        bad_key_proof_pairs[0].0 = PublicKey::zero();

        let test_fail_aggregate =
            CardProtocol::compute_aggregate_key(&parameters, &bad_key_proof_pairs);

        assert_eq!(
            test_fail_aggregate,
            Err(CardProtocolError::ProofVerificationError(
                CryptoError::ProofVerificationError(String::from("Schnorr Identification"))
            ))
        )
    }

    #[test]
    fn test_unmask() {
        let rng = &mut thread_rng();
        let m = 4;
        let n = 13;

        let num_of_players = 10;

        let parameters = CardProtocol::setup(rng, m, n).unwrap();

        let (players, expected_shared_key) = setup_players(rng, &parameters, num_of_players);

        let card = Card::rand(rng);
        let alpha = Scalar::rand(rng);
        let (masked, _) =
            CardProtocol::mask(rng, &parameters, &expected_shared_key, &card, &alpha).unwrap();

        let decryption_key = players
            .iter()
            .map(|player| {
                let (token, proof) = CardProtocol::compute_reveal_token(
                    rng,
                    &parameters,
                    &player.1,
                    &player.0,
                    &masked,
                )
                .unwrap();

                (token, proof, player.0)
            })
            .collect::<Vec<_>>();

        let unmasked = CardProtocol::unmask(&parameters, &decryption_key, &masked).unwrap();

        assert_eq!(card, unmasked);

        let mut bad_decryption_key = decryption_key;
        bad_decryption_key[0].0 = RevealToken::rand(rng);

        let failed_decryption = CardProtocol::unmask(&parameters, &bad_decryption_key, &masked);

        assert_eq!(
            failed_decryption,
            Err(CardProtocolError::ProofVerificationError(
                CryptoError::ProofVerificationError(String::from("Chaum-Pedersen"))
            ))
        )
    }

    #[test]
    fn test_shuffle() {
        let rng = &mut thread_rng();
        let m = 4;
        let n = 13;

        let num_of_players = 10;

        let parameters = CardProtocol::setup(rng, m, n).unwrap();

        let (_, aggregate_key) = setup_players(rng, &parameters, num_of_players);

        let deck: Vec<MaskedCard> = sample_vector(rng, m * n);

        let permutation = Permutation::new(rng, m * n);
        let masking_factors: Vec<Scalar> = sample_vector(rng, m * n);

        let (shuffled_deck, shuffle_proof) = CardProtocol::shuffle_and_remask(
            rng,
            &parameters,
            &aggregate_key,
            &deck,
            &masking_factors,
            &permutation,
        )
        .unwrap();

        assert_eq!(
            Ok(()),
            CardProtocol::verify_shuffle(
                &parameters,
                &aggregate_key,
                &deck,
                &shuffled_deck,
                &shuffle_proof
            )
        );

        let wrong_output: Vec<MaskedCard> = sample_vector(rng, m * n);

        assert_eq!(
            CardProtocol::verify_shuffle(
                &parameters,
                &aggregate_key,
                &deck,
                &wrong_output,
                &shuffle_proof
            ),
            Err(CryptoError::ProofVerificationError(String::from(
                "Hadamard Product (5.1)"
            )))
        )
    }

    /// Full trustless N-player deal, end to end:
    ///   1. every player generates a key and a proof of key ownership;
    ///   2. all keys are verified and aggregated into the shared key;
    ///   3. an open deck is masked (each masking accompanied by a verified proof);
    ///   4. every player shuffles-and-remasks the deck behind a verified shuffle
    ///      proof, so no single player knows the final permutation;
    ///   5. for each card every player issues a verified reveal token and the
    ///      deck is cooperatively unmasked;
    ///   6. the recovered cards are exactly the original deck (as a multiset),
    ///      proving the deal is correct without any trusted party.
    #[test]
    fn full_nplayer_deal_round_trip() {
        let rng = &mut thread_rng();
        let m = 2;
        let n = 26;
        let deck_size = m * n;
        let num_of_players = 4;

        let parameters = CardProtocol::setup(rng, m, n).unwrap();

        // 1 + 2: per-player keygen, ownership proofs, verified aggregation.
        let players = setup_players(rng, &parameters, num_of_players).0;

        let key_proof_info = players
            .iter()
            .map(|(pk, sk, info)| {
                let proof =
                    CardProtocol::prove_key_ownership(rng, &parameters, pk, sk, info).unwrap();
                (*pk, proof, *info)
            })
            .collect::<Vec<_>>();

        let aggregate_key =
            CardProtocol::compute_aggregate_key(&parameters, &key_proof_info).unwrap();

        // 3: build an open deck of distinct cards and mask each one.
        let original_deck: Vec<Card> = sample_vector(rng, deck_size);

        let mut deck: Vec<MaskedCard> = Vec::with_capacity(deck_size);
        for card in original_deck.iter() {
            let alpha = Scalar::rand(rng);
            let (masked, proof) =
                CardProtocol::mask(rng, &parameters, &aggregate_key, card, &alpha).unwrap();
            assert_eq!(
                Ok(()),
                CardProtocol::verify_mask(&parameters, &aggregate_key, card, &masked, &proof)
            );
            deck.push(masked);
        }

        // 4: each player shuffles-and-remasks the whole deck behind a proof.
        for _ in 0..num_of_players {
            let permutation = Permutation::new(rng, deck_size);
            let masking_factors: Vec<Scalar> = sample_vector(rng, deck_size);

            let (shuffled, proof) = CardProtocol::shuffle_and_remask(
                rng,
                &parameters,
                &aggregate_key,
                &deck,
                &masking_factors,
                &permutation,
            )
            .unwrap();

            assert_eq!(
                Ok(()),
                CardProtocol::verify_shuffle(
                    &parameters,
                    &aggregate_key,
                    &deck,
                    &shuffled,
                    &proof
                )
            );

            deck = shuffled;
        }

        // 5: cooperatively reveal every card (all players contribute a verified
        // reveal token) and unmask.
        let mut revealed_deck: Vec<Card> = Vec::with_capacity(deck_size);
        for masked in deck.iter() {
            let decryption_key = players
                .iter()
                .map(|(pk, sk, _)| {
                    let (token, proof) =
                        CardProtocol::compute_reveal_token(rng, &parameters, sk, pk, masked)
                            .unwrap();
                    assert_eq!(
                        Ok(()),
                        CardProtocol::verify_reveal(&parameters, pk, &token, masked, &proof)
                    );
                    (token, proof, *pk)
                })
                .collect::<Vec<_>>();

            let card = CardProtocol::unmask(&parameters, &decryption_key, masked).unwrap();
            revealed_deck.push(card);
        }

        // 6: the revealed cards are exactly the original deck as a multiset.
        let sort_key = |cards: &Vec<Card>| {
            let mut bytes = cards
                .iter()
                .map(|c| {
                    let mut b = Vec::new();
                    c.serialize_compressed(&mut b).unwrap();
                    b
                })
                .collect::<Vec<_>>();
            bytes.sort();
            bytes
        };

        assert_eq!(revealed_deck.len(), deck_size);
        assert_eq!(sort_key(&revealed_deck), sort_key(&original_deck));
    }
}
