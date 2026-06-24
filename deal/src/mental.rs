//! Real trustless mental-poker deal (M2).
//!
//! This is the cryptographic deal that replaces [`crate::placeholder_shuffled_deck`]
//! for production play. It wires the workspace to the vendored, arkworks-0.6 /
//! Baby-Jubjub port of Geometry Research's **Barnett–Smart** card protocol
//! (`barnett-smart-card-protocol` + `proof-essentials`):
//!
//! * **threshold ElGamal masking** so the open deck is hidden behind the players'
//!   aggregate key (no single player can decrypt),
//! * a **Bayer–Groth zero-knowledge shuffle argument** run once per player, so the
//!   final permutation is known to nobody and every shuffle is publicly verifiable,
//! * **Chaum–Pedersen** reveal proofs for cooperative, verifiable unmasking.
//!
//! The placeholder shuffle in the crate root stays available unchanged; this module
//! is a purely additive new path, sharing the same `poker_game::Card` boundary.
//!
//! ## Card <-> point encoding
//!
//! The protocol deals *group elements*, not poker cards. We fix an injective map from
//! the 52 card indices (`poker_game::Card::to_index`, `0..52`) to 52 distinct Baby
//! Jubjub points: index `i` maps to `(i + 1) * G`, where `G` is the curve's standard
//! prime-order generator. This is injective (the 52 scalars `1..=52` are distinct and
//! far below the group order) and the reverse lookup is a precomputed table, so a
//! revealed point recovers its card in O(1). See [`CardEncoding`].

use std::collections::HashMap;

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use ark_std::rand::Rng;

use barnett_smart_card_protocol::discrete_log_cards;
use barnett_smart_card_protocol::BarnettSmartProtocol;

use proof_essentials::homomorphic_encryption::el_gamal::Plaintext;
use proof_essentials::utils::permutation::Permutation;
use proof_essentials::utils::rand::sample_vector;

use poker_game::Card as PokerCard;

/// Baby Jubjub projective group (`ark_ed_on_bn254::EdwardsProjective`).
pub type Curve = proof_essentials::curve::Projective;
/// Scalar field of [`Curve`].
pub type Scalar = proof_essentials::curve::Fr;
/// Affine point of [`Curve`].
pub type Point = proof_essentials::curve::Affine;

/// Concrete discrete-log instantiation of the Barnett–Smart protocol over Baby Jubjub.
pub type CardProtocol = discrete_log_cards::DLCards<Curve>;

/// Scheme parameters (deck size = `m * n`). Built via [`MentalDeal::setup`].
pub type Parameters = discrete_log_cards::Parameters<Curve>;
/// A player's public key (also the type of the aggregate key).
pub type PublicKey = discrete_log_cards::PublicKey<Curve>;
/// A player's secret key.
pub type SecretKey = discrete_log_cards::PlayerSecretKey<Curve>;
/// The aggregate (shared) public key all players mask under.
pub type AggregatePublicKey = discrete_log_cards::PublicKey<Curve>;

/// An open card as a group element (ElGamal plaintext).
pub type MaskedPlaintext = discrete_log_cards::Card<Curve>;
/// A masked (face-down) card: an ElGamal ciphertext.
pub type MaskedCard = discrete_log_cards::MaskedCard<Curve>;
/// A per-player reveal token contributing to cooperative unmasking.
pub type RevealToken = discrete_log_cards::RevealToken<Curve>;

/// Proof types (associated types on [`BarnettSmartProtocol`]).
pub type ProofKeyOwnership = <CardProtocol as BarnettSmartProtocol>::ZKProofKeyOwnership;
/// Proof that a deck was masked correctly.
pub type ProofMasking = <CardProtocol as BarnettSmartProtocol>::ZKProofMasking;
/// Proof that a shuffle-and-remask was performed correctly.
pub type ProofShuffle = <CardProtocol as BarnettSmartProtocol>::ZKProofShuffle;
/// Proof that a reveal token was computed correctly.
pub type ProofReveal = <CardProtocol as BarnettSmartProtocol>::ZKProofReveal;

/// The standard 52-card deck size.
pub const DECK_SIZE: usize = 52;

/// Errors from the mental-poker deal layer.
#[derive(Debug, thiserror::Error)]
pub enum DealError {
    /// A protocol-level operation (setup/mask/shuffle/...) failed.
    #[error("card protocol error: {0}")]
    Protocol(#[from] barnett_smart_card_protocol::error::CardProtocolError),
    /// A zero-knowledge proof failed to verify.
    #[error("proof verification failed: {0}")]
    Verification(#[from] proof_essentials::error::CryptoError),
    /// A revealed group element did not correspond to any of the 52 cards.
    #[error("revealed point does not decode to a valid card")]
    UnknownCard,
    /// `m * n` did not equal the 52-card deck size.
    #[error("deck dimensions m*n = {0} must equal 52")]
    BadDeckSize(usize),
    /// A position index was out of range for the deck.
    #[error("position {0} is out of range for a 52-card deck")]
    BadPosition(usize),
    /// A contribution's claimed seat did not match its cryptographic binding (key-ownership
    /// `info` scalar, or a reveal token's registered seat key). Authenticates the seat of a
    /// relayed payload independently of who forwarded it.
    #[error("deal contribution seat {0} does not match its cryptographic binding")]
    SeatMismatch(usize),
}

/// Injective map between the 52 poker cards and 52 distinct Baby Jubjub points.
///
/// Forward: card index `i` -> `(i + 1) * G`. Reverse: a precomputed table from the
/// compressed point bytes back to the card index, used when a revealed plaintext must
/// be decoded to a [`PokerCard`].
#[derive(Clone)]
pub struct CardEncoding {
    /// `points[i]` is the plaintext for card index `i`.
    points: Vec<MaskedPlaintext>,
    /// Reverse lookup: compressed point bytes -> card index.
    lookup: HashMap<Vec<u8>, u8>,
}

impl CardEncoding {
    /// Build the canonical 52-card encoding.
    pub fn new() -> Self {
        use ark_serialize::CanonicalSerialize;

        let base = Point::generator();
        let mut points = Vec::with_capacity(DECK_SIZE);
        let mut lookup = HashMap::with_capacity(DECK_SIZE);

        for i in 0..DECK_SIZE as u8 {
            // scalar = i + 1, so distinct nonzero small scalars -> distinct points.
            let scalar = Scalar::from((i as u64) + 1);
            let point = base.mul_bigint(scalar.into_bigint()).into_affine();
            let plaintext: MaskedPlaintext = Plaintext(point);

            let mut bytes = Vec::new();
            plaintext
                .serialize_compressed(&mut bytes)
                .expect("serialize Baby Jubjub point");
            lookup.insert(bytes, i);
            points.push(plaintext);
        }

        Self { points, lookup }
    }

    /// Encode a poker card as its group element.
    pub fn encode(&self, card: PokerCard) -> MaskedPlaintext {
        self.points[card.to_index() as usize]
    }

    /// Encode a card index (`0..52`) as its group element.
    pub fn encode_index(&self, index: u8) -> MaskedPlaintext {
        self.points[index as usize]
    }

    /// Decode a revealed group element back to a poker card.
    pub fn decode(&self, plaintext: &MaskedPlaintext) -> Result<PokerCard, DealError> {
        use ark_serialize::CanonicalSerialize;
        let mut bytes = Vec::new();
        plaintext
            .serialize_compressed(&mut bytes)
            .map_err(|_| DealError::UnknownCard)?;
        let index = *self.lookup.get(&bytes).ok_or(DealError::UnknownCard)?;
        PokerCard::from_index(index).ok_or(DealError::UnknownCard)
    }

    /// The full ordered deck of 52 plaintexts (card index order).
    pub fn open_deck(&self) -> Vec<MaskedPlaintext> {
        self.points.clone()
    }
}

impl Default for CardEncoding {
    fn default() -> Self {
        Self::new()
    }
}

/// A player's key material plus public info, used to build the aggregate key.
#[derive(Clone)]
pub struct PlayerKeys {
    /// Public key, shared with all players.
    pub public: PublicKey,
    /// Secret key, never shared.
    pub secret: SecretKey,
    /// Public info bound into the key-ownership proof (e.g. an id/name scalar).
    pub info: Scalar,
}

/// A reveal contribution from one player for one masked card: token + proof + the
/// player's public key (so any verifier can check the Chaum–Pedersen proof).
pub type RevealContribution = (RevealToken, ProofReveal, PublicKey);

/// A trustless N-player Texas Hold'em deal over Baby Jubjub.
///
/// Holds the scheme parameters, the aggregate key, the card encoding, and the current
/// (masked, shuffled) deck. Construct with [`MentalDeal::setup`], then:
/// 1. [`MentalDeal::player_keygen`] per player + [`MentalDeal::aggregate_keys`],
/// 2. [`MentalDeal::mask_initial_deck`] to face down the sorted 52-card deck,
/// 3. [`MentalDeal::shuffle`] once per player (each output verified by all),
/// 4. [`MentalDeal::reveal_token`] / [`MentalDeal::unmask_position`] to open cards.
pub struct MentalDeal {
    parameters: Parameters,
    aggregate_key: AggregatePublicKey,
    encoding: CardEncoding,
    /// The current face-down deck (masked + shuffled so far).
    deck: Vec<MaskedCard>,
}

impl MentalDeal {
    /// Set up the protocol for a 52-card deck. `m * n` must equal 52
    /// (the Bayer–Groth shuffle factors the deck into an `m x n` matrix; the commit
    /// key is sized to `n`). Common choices: `m = 4, n = 13` or `m = 2, n = 26`.
    pub fn setup<R: Rng>(rng: &mut R, m: usize, n: usize) -> Result<Self, DealError> {
        if m * n != DECK_SIZE {
            return Err(DealError::BadDeckSize(m * n));
        }
        let parameters = CardProtocol::setup(rng, m, n)?;
        Ok(Self {
            parameters,
            aggregate_key: AggregatePublicKey::default(),
            encoding: CardEncoding::new(),
            deck: Vec::new(),
        })
    }

    /// The scheme parameters.
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    /// The card <-> point encoding.
    pub fn encoding(&self) -> &CardEncoding {
        &self.encoding
    }

    /// The aggregate (shared) key. Meaningful only after [`Self::aggregate_keys`].
    pub fn aggregate_key(&self) -> &AggregatePublicKey {
        &self.aggregate_key
    }

    /// The current face-down deck (after masking and any shuffles applied so far).
    pub fn deck(&self) -> &[MaskedCard] {
        &self.deck
    }

    /// Generate a fresh key pair plus a key-ownership proof for one player.
    ///
    /// `info` is any public identifier (a scalar) bound into the proof. Returns the
    /// player's keys and the ownership proof; share `(public, proof, info)` with the
    /// other players so they can verify and aggregate.
    pub fn player_keygen<R: Rng>(
        &self,
        rng: &mut R,
        info: Scalar,
    ) -> Result<(PlayerKeys, ProofKeyOwnership), DealError> {
        let (public, secret) = CardProtocol::player_keygen(rng, &self.parameters)?;
        let proof =
            CardProtocol::prove_key_ownership(rng, &self.parameters, &public, &secret, &info)?;
        Ok((PlayerKeys { public, secret, info }, proof))
    }

    /// Verify every player's key-ownership proof and combine the keys into the shared
    /// aggregate key, which is stored on the deal for masking. `key_proof_info` is the
    /// list of `(public_key, ownership_proof, info)` gathered from all players.
    pub fn aggregate_keys(
        &mut self,
        key_proof_info: &[(PublicKey, ProofKeyOwnership, Scalar)],
    ) -> Result<AggregatePublicKey, DealError> {
        // `compute_aggregate_key` verifies every ownership proof internally.
        let owned: Vec<(PublicKey, ProofKeyOwnership, Scalar)> = key_proof_info.to_vec();
        let aggregate = CardProtocol::compute_aggregate_key(&self.parameters, &owned)?;
        self.aggregate_key = aggregate;
        Ok(aggregate)
    }

    /// Mask the canonical sorted 52-card deck under the aggregate key, producing the
    /// initial face-down deck. Every masking is accompanied by a Chaum–Pedersen proof
    /// that is verified before the card is accepted. Stores the deck on the deal.
    ///
    /// Returns the per-card masking proofs so they can be broadcast/checked by peers.
    pub fn mask_initial_deck<R: Rng>(
        &mut self,
        rng: &mut R,
    ) -> Result<Vec<ProofMasking>, DealError> {
        use ark_ff::UniformRand;

        let open = self.encoding.open_deck();
        let mut deck = Vec::with_capacity(DECK_SIZE);
        let mut proofs = Vec::with_capacity(DECK_SIZE);

        for card in open.iter() {
            let alpha = Scalar::rand(rng);
            let (masked, proof) =
                CardProtocol::mask(rng, &self.parameters, &self.aggregate_key, card, &alpha)?;
            CardProtocol::verify_mask(
                &self.parameters,
                &self.aggregate_key,
                card,
                &masked,
                &proof,
            )?;
            deck.push(masked);
            proofs.push(proof);
        }

        self.deck = deck;
        Ok(proofs)
    }

    /// Perform one player's shuffle-and-remask of the current deck behind a
    /// zero-knowledge shuffle proof. A fresh random permutation and masking factors
    /// are sampled internally. The new deck and proof are returned; the new deck is
    /// also stored on the deal so the next player shuffles on top of it.
    ///
    /// The caller is responsible for broadcasting the `(old_deck, new_deck, proof)` so
    /// every other player can call [`Self::verify_shuffle`]. We verify the proof here
    /// too, so a buggy shuffle is caught immediately.
    pub fn shuffle<R: Rng>(
        &mut self,
        rng: &mut R,
    ) -> Result<(Vec<MaskedCard>, ProofShuffle), DealError> {
        let permutation = Permutation::new(rng, DECK_SIZE);
        let masking_factors: Vec<Scalar> = sample_vector(rng, DECK_SIZE);

        let old_deck = self.deck.clone();
        let (shuffled, proof) = CardProtocol::shuffle_and_remask(
            rng,
            &self.parameters,
            &self.aggregate_key,
            &old_deck,
            &masking_factors,
            &permutation,
        )?;

        // Self-check: the shuffle proof must verify against the deck we shuffled.
        CardProtocol::verify_shuffle(
            &self.parameters,
            &self.aggregate_key,
            &old_deck,
            &shuffled,
            &proof,
        )?;

        self.deck = shuffled.clone();
        Ok((shuffled, proof))
    }

    /// Verify a shuffle proof relating `original_deck` to `shuffled_deck`. Every player
    /// runs this on every other player's shuffle.
    pub fn verify_shuffle(
        &self,
        original_deck: &[MaskedCard],
        shuffled_deck: &[MaskedCard],
        proof: &ProofShuffle,
    ) -> Result<(), DealError> {
        CardProtocol::verify_shuffle(
            &self.parameters,
            &self.aggregate_key,
            &original_deck.to_vec(),
            &shuffled_deck.to_vec(),
            proof,
        )?;
        Ok(())
    }

    /// Compute one player's reveal contribution for the masked card at `position`,
    /// together with a Chaum–Pedersen proof that any peer can verify.
    pub fn reveal_token<R: Rng>(
        &self,
        rng: &mut R,
        keys: &PlayerKeys,
        position: usize,
    ) -> Result<RevealContribution, DealError> {
        let masked = self.deck.get(position).ok_or(DealError::BadPosition(position))?;
        let (token, proof) = CardProtocol::compute_reveal_token(
            rng,
            &self.parameters,
            &keys.secret,
            &keys.public,
            masked,
        )?;
        CardProtocol::verify_reveal(&self.parameters, &keys.public, &token, masked, &proof)?;
        Ok((token, proof, keys.public))
    }

    /// Cooperatively unmask the card at `position` given a reveal contribution from
    /// each player. Every contribution's proof is verified (inside `unmask`); the
    /// resulting plaintext is decoded to a [`PokerCard`].
    ///
    /// For a hole card, only its owner needs the result; for community cards everyone
    /// does — either way every player must contribute a token, since the deck is masked
    /// under the *aggregate* key.
    pub fn unmask_position(
        &self,
        position: usize,
        contributions: &Vec<RevealContribution>,
    ) -> Result<PokerCard, DealError> {
        let masked = self.deck.get(position).ok_or(DealError::BadPosition(position))?;
        let plaintext = CardProtocol::unmask(&self.parameters, contributions, masked)?;
        self.encoding.decode(&plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use std::collections::HashSet;

    #[test]
    fn card_point_map_roundtrips_all_52() {
        let enc = CardEncoding::new();
        let mut points = HashSet::new();
        for i in 0..52u8 {
            let card = PokerCard::from_index(i).unwrap();
            let pt = enc.encode(card);
            // distinct points
            assert!(points.insert(pt.0), "duplicate point for card index {i}");
            // round-trip
            let back = enc.decode(&pt).unwrap();
            assert_eq!(back, card, "decode(encode(card)) != card for index {i}");
            assert_eq!(back.to_index(), i);
        }
        assert_eq!(points.len(), 52);
    }

    #[test]
    fn decode_rejects_non_card_point() {
        use ark_ff::UniformRand;
        let enc = CardEncoding::new();
        let rng = &mut thread_rng();
        // A random plaintext is overwhelmingly unlikely to be one of the 52 cards.
        let bogus: MaskedPlaintext = Plaintext(Curve::rand(rng).into_affine());
        assert!(matches!(enc.decode(&bogus), Err(DealError::UnknownCard)));
    }

    /// Full multi-player trustless deal:
    ///  - per-player keygen + verified aggregate key,
    ///  - mask the sorted 52-card deck (each mask proof verified),
    ///  - every player shuffles-and-remasks behind a verified proof,
    ///  - cooperatively reveal every position with verified reveal proofs,
    ///  - the 52 revealed cards form a permutation of the full deck.
    #[test]
    fn full_multiplayer_deal_reveals_a_permutation_of_52() {
        let rng = &mut thread_rng();
        let num_players = 4;

        let mut deal = MentalDeal::setup(rng, 4, 13).unwrap();

        // 1. keygen + aggregate.
        let mut players = Vec::new();
        let mut key_proof_info = Vec::new();
        for p in 0..num_players {
            let info = Scalar::from((p as u64) + 1);
            let (keys, proof) = deal.player_keygen(rng, info).unwrap();
            key_proof_info.push((keys.public, proof, keys.info));
            players.push(keys);
        }
        let _aggregate = deal.aggregate_keys(&key_proof_info).unwrap();

        // 2. mask the initial sorted deck.
        deal.mask_initial_deck(rng).unwrap();
        assert_eq!(deal.deck().len(), 52);

        // 3. each player shuffles; every other player verifies.
        for _ in 0..num_players {
            let before = deal.deck().to_vec();
            let (after, proof) = deal.shuffle(rng).unwrap();
            // independent verification (as every peer would do)
            deal.verify_shuffle(&before, &after, &proof).unwrap();
        }

        // 4. cooperatively reveal every position.
        let mut revealed = Vec::with_capacity(52);
        for pos in 0..52usize {
            let contributions: Vec<RevealContribution> = players
                .iter()
                .map(|keys| deal.reveal_token(rng, keys, pos).unwrap())
                .collect();
            let card = deal.unmask_position(pos, &contributions).unwrap();
            revealed.push(card);
        }

        // 5. the revealed deck is a permutation of all 52 cards.
        assert_eq!(revealed.len(), 52);
        let indices: HashSet<u8> = revealed.iter().map(|c| c.to_index()).collect();
        assert_eq!(indices.len(), 52, "revealed deck must be a permutation of 52");
    }

    #[test]
    fn bad_deck_size_is_rejected() {
        let rng = &mut thread_rng();
        assert!(matches!(
            MentalDeal::setup(rng, 5, 13),
            Err(DealError::BadDeckSize(65))
        ));
    }
}
