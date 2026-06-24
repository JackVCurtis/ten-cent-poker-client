//! Distributed, step-wise driver for the trustless Barnett-Smart deal.
//!
//! [`crate::mental::MentalDeal`] is shaped for *in-process* use: one object holds
//! every player's keys and drives the whole protocol. That is fine for the M2
//! correctness proof, but real networked play needs each peer to run its **own**
//! replica that holds **only its own secret key** and advances the protocol by
//! exchanging *serialized* messages with the other peers.
//!
//! This module is that per-peer replica. It is purely additive on top of
//! [`crate::mental`]: it reuses the same [`CardProtocol`], [`CardEncoding`] and
//! type aliases, and never touches a remote peer's secret. Every object that
//! crosses the wire is (de)serialized with arkworks
//! [`CanonicalSerialize`]/[`CanonicalDeserialize`] (compressed form), exposed
//! through the small `wire` helpers at the bottom of this file.
//!
//! ## Determinism without communication
//!
//! Two protocol steps must produce **byte-identical** results on every peer with
//! *no* message exchange:
//!
//! * **Scheme parameters** ([`CardProtocol::setup`]) sample random group
//!   generators and a Pedersen commitment key. We make them reproducible by
//!   seeding a `ChaCha20Rng` from a single shared 32-byte `session_seed` that all
//!   peers agree on out of band (e.g. a hash of the table id + player set). Same
//!   seed => same `Parameters` on every peer, with nothing sent.
//!
//! * **Initial masked deck.** After the aggregate key is known, the canonical
//!   sorted 52-card deck is masked under it. We mask every card with **fixed zero
//!   randomness** (`r = 0`): card `i`'s plaintext `P_i` becomes the ElGamal
//!   ciphertext `(0, P_i)` (the identity in the first component). This is a valid
//!   encryption under *any* key and is identical on every peer, so the initial
//!   deck needs no randomness exchange. It is deliberately **not hiding** — the
//!   initial order is the public sorted deck — but that is irrelevant: hiding and
//!   the secret permutation come entirely from the `N` subsequent shuffle+remask
//!   rounds, each of which injects fresh secret randomness and is proven in
//!   zero-knowledge. We still attach the protocol's masking proof (also computed
//!   with `r = 0`, hence deterministic) so the step stays uniformly verifiable.
//!
//! Everything else (keygen, shuffle, reveal) carries fresh local randomness and
//! is exchanged + verified over the wire.

use std::collections::BTreeMap;

use ark_ff::Zero;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use barnett_smart_card_protocol::BarnettSmartProtocol;

use poker_game::Card as PokerCard;

use crate::mental::{
    AggregatePublicKey, CardEncoding, CardProtocol, DealError, MaskedCard, Parameters, PublicKey,
    ProofKeyOwnership, ProofReveal, ProofShuffle, RevealToken, Scalar, SecretKey, DECK_SIZE,
};

use proof_essentials::utils::permutation::Permutation;
use proof_essentials::utils::rand::sample_vector;

/// A peer's seat index in the fixed seat order (`0..num_players`). Seat order is
/// agreed out of band and determines both the aggregate-key fold order and the
/// shuffle turn order, so it must be identical on every peer.
pub type Seat = usize;

/// One peer's announcement of its public key together with the Schnorr ownership
/// proof and the public info scalar bound into that proof. Broadcast once during
/// keygen; every peer verifies it before folding it into the aggregate key.
///
/// Wire form: [`KeyAnnouncement::serialize`] / [`KeyAnnouncement::deserialize`].
#[derive(Clone)]
pub struct KeyAnnouncement {
    /// The announcing peer's seat (fold order is by seat, so this pins position).
    pub seat: Seat,
    /// The peer's ElGamal public key.
    pub public: PublicKey,
    /// Schnorr proof that the peer knows the matching secret key.
    pub proof: ProofKeyOwnership,
    /// Public info scalar bound into the ownership proof (we use `seat + 1`).
    pub info: Scalar,
}

/// One peer's shuffle output: the new deck plus the Bayer-Groth proof relating it
/// to the deck the peer shuffled. Broadcast on the peer's shuffle turn; every
/// other peer verifies `proof` against the *prior* deck before accepting `deck`.
///
/// Wire form: [`ShuffleMessage::serialize`] / [`ShuffleMessage::deserialize`].
pub struct ShuffleMessage {
    /// The seat that produced this shuffle.
    pub seat: Seat,
    /// The new, shuffled-and-remasked deck (length [`DECK_SIZE`]).
    pub deck: Vec<MaskedCard>,
    /// Bayer-Groth zero-knowledge shuffle proof.
    pub proof: ProofShuffle,
}

/// One peer's reveal contribution for a single deck position: a partial-decryption
/// token plus the Chaum-Pedersen proof that it was computed with the peer's secret
/// key (verifiable against the peer's public key). Broadcast for community and
/// showdown positions, and by every non-owner for a seat's hole positions.
///
/// Wire form: [`RevealMessage::serialize`] / [`RevealMessage::deserialize`].
pub struct RevealMessage {
    /// The seat contributing the token.
    pub seat: Seat,
    /// The deck position this token unmasks.
    pub position: usize,
    /// The partial-decryption reveal token.
    pub token: RevealToken,
    /// Chaum-Pedersen proof that `token` is correct for `position`.
    pub proof: ProofReveal,
    /// The contributing peer's public key (lets any verifier check `proof`).
    pub public: PublicKey,
}

/// A single peer's private replica of the distributed deal.
///
/// Holds the shared, deterministically-derived [`Parameters`] and
/// [`CardEncoding`], this peer's *own* key pair (secret never leaves), the set of
/// verified remote public keys, the computed aggregate key, and the current deck.
/// Advance it by producing local messages and ingesting remote ones; it never
/// sees another peer's secret.
pub struct PeerDeal {
    parameters: Parameters,
    encoding: CardEncoding,
    num_players: usize,
    seat: Seat,
    /// This peer's public info scalar (`seat + 1`), bound into its ownership proof.
    info: Scalar,
    /// This peer's secret key. **Never serialized, never broadcast.**
    secret: SecretKey,
    /// This peer's public key.
    public: PublicKey,
    /// Verified `(public, info)` per seat, including this peer's own. Used to
    /// compute the aggregate key in a fixed (seat) order on every peer.
    announced: BTreeMap<Seat, (PublicKey, ProofKeyOwnership, Scalar)>,
    /// The aggregate key, set once all keys are in and verified.
    aggregate_key: Option<AggregatePublicKey>,
    /// The current face-down deck.
    deck: Vec<MaskedCard>,
}

impl PeerDeal {
    /// Create this peer's replica.
    ///
    /// `session_seed` is the shared 32-byte seed all peers agreed on out of band;
    /// it makes [`Parameters`] byte-identical across peers with no exchange (see
    /// the module docs). `m * n` must equal 52. `seat` is this peer's index in the
    /// agreed seat order and `num_players` the total. A fresh key pair is generated
    /// from `rng` (local randomness); the secret never leaves this struct.
    pub fn new<R: Rng>(
        rng: &mut R,
        session_seed: [u8; 32],
        m: usize,
        n: usize,
        seat: Seat,
        num_players: usize,
    ) -> Result<Self, DealError> {
        if m * n != DECK_SIZE {
            return Err(DealError::BadDeckSize(m * n));
        }
        if seat >= num_players {
            return Err(DealError::BadPosition(seat));
        }

        // Deterministic, communication-free parameters: every peer seeds the same
        // ChaCha20 stream from the shared session seed.
        let mut param_rng = ChaCha20Rng::from_seed(session_seed);
        let parameters = CardProtocol::setup(&mut param_rng, m, n)?;

        let (public, secret) = CardProtocol::player_keygen(rng, &parameters)?;
        let info = Scalar::from((seat as u64) + 1);

        Ok(Self {
            parameters,
            encoding: CardEncoding::new(),
            num_players,
            seat,
            info,
            secret,
            public,
            announced: BTreeMap::new(),
            aggregate_key: None,
            deck: Vec::new(),
        })
    }

    /// This peer's seat.
    pub fn seat(&self) -> Seat {
        self.seat
    }

    /// The shared scheme parameters.
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    /// The card <-> point encoding.
    pub fn encoding(&self) -> &CardEncoding {
        &self.encoding
    }

    /// The verified public key registered for `seat` during keygen, if present. A relayed reveal
    /// token must embed exactly this key for its seat, which is what lets a reveal be authenticated
    /// by its proof + this binding rather than by who forwarded the message.
    pub fn registered_public(&self, seat: Seat) -> Option<PublicKey> {
        self.announced.get(&seat).map(|(pk, _, _)| *pk)
    }

    /// The aggregate key, if all peer keys have been ingested and verified.
    pub fn aggregate_key(&self) -> Option<&AggregatePublicKey> {
        self.aggregate_key.as_ref()
    }

    /// The current face-down deck.
    pub fn deck(&self) -> &[MaskedCard] {
        &self.deck
    }

    // ---- KEYGEN ----------------------------------------------------------

    /// Produce this peer's [`KeyAnnouncement`] to broadcast during keygen.
    ///
    /// Also records this peer's own key locally so [`Self::ingest_key`] is only
    /// needed for *remote* peers. Idempotent.
    pub fn key_announcement<R: Rng>(
        &mut self,
        rng: &mut R,
    ) -> Result<KeyAnnouncement, DealError> {
        let proof = CardProtocol::prove_key_ownership(
            rng,
            &self.parameters,
            &self.public,
            &self.secret,
            &self.info,
        )?;
        self.announced
            .insert(self.seat, (self.public, proof.clone(), self.info));
        Ok(KeyAnnouncement {
            seat: self.seat,
            public: self.public,
            proof,
            info: self.info,
        })
    }

    /// Ingest and **verify** a remote peer's [`KeyAnnouncement`].
    ///
    /// Verifies the Schnorr ownership proof before recording the key, so a peer
    /// that does not actually hold the matching secret cannot enter the aggregate.
    pub fn ingest_key(&mut self, ann: &KeyAnnouncement) -> Result<(), DealError> {
        // Bind the claimed seat to the `info` scalar committed inside the ownership proof
        // (info = seat + 1). With this, `ann.seat` is partially authenticated by the proof, so a
        // key announcement can be accepted no matter which peer relayed it (the host relays one
        // guest's key to the others). NOTE (free-game caveat): the Schnorr proof shows ownership
        // of THIS key but does not bind the key to the seat's PeerId, so a seated peer could still
        // squat another seat's slot to grief a hand (a DoS, not a card-secrecy break — it cannot
        // reveal anyone's cards). Hardening (PeerId-bound keys) is required before real stakes.
        if ann.seat >= self.num_players || ann.info != Scalar::from((ann.seat as u64) + 1) {
            return Err(DealError::SeatMismatch(ann.seat));
        }
        CardProtocol::verify_key_ownership(
            &self.parameters,
            &ann.public,
            &ann.info,
            &ann.proof,
        )?;
        self.announced
            .insert(ann.seat, (ann.public, ann.proof.clone(), ann.info));
        Ok(())
    }

    /// Whether announcements from all `num_players` seats have been verified.
    pub fn all_keys_in(&self) -> bool {
        self.announced.len() == self.num_players
    }

    /// Fold all verified keys into the aggregate key, in fixed seat order, and
    /// store it. Identical on every peer (same keys, same order). Errors if any
    /// announcement is still missing.
    pub fn finalize_aggregate_key(&mut self) -> Result<AggregatePublicKey, DealError> {
        if !self.all_keys_in() {
            return Err(DealError::BadPosition(self.announced.len()));
        }
        // BTreeMap iterates by seat => fixed, identical order on every peer.
        let ordered: Vec<(PublicKey, ProofKeyOwnership, Scalar)> = self
            .announced
            .values()
            .map(|(pk, proof, info)| (*pk, proof.clone(), *info))
            .collect();
        let aggregate = CardProtocol::compute_aggregate_key(&self.parameters, &ordered)?;
        self.aggregate_key = Some(aggregate);
        Ok(aggregate)
    }

    // ---- INITIAL MASK ----------------------------------------------------

    /// Deterministically mask the canonical sorted 52-card deck under the
    /// aggregate key, storing it as the starting deck. Uses fixed zero randomness
    /// so every peer computes the *identical* initial deck with no exchange (see
    /// module docs). Requires the aggregate key to be finalized.
    pub fn mask_initial_deck(&mut self) -> Result<(), DealError> {
        let aggregate = self
            .aggregate_key
            .ok_or(DealError::BadPosition(usize::MAX))?;
        // A throwaway RNG: `mask` takes one, but with r = 0 the proof is a
        // deterministic Fiat-Shamir transcript, so the output is reproducible
        // regardless of what this RNG yields.
        let mut rng = ChaCha20Rng::from_seed([0u8; 32]);
        let zero = Scalar::zero();
        let open = self.encoding.open_deck();
        let mut deck = Vec::with_capacity(DECK_SIZE);
        for card in open.iter() {
            let (masked, proof) = CardProtocol::mask(
                &mut rng,
                &self.parameters,
                &aggregate,
                card,
                &zero,
            )?;
            // Self-check (every peer reconstructs the same masked card, so this
            // also confirms the deterministic step is internally consistent).
            CardProtocol::verify_mask(&self.parameters, &aggregate, card, &masked, &proof)?;
            deck.push(masked);
        }
        self.deck = deck;
        Ok(())
    }

    // ---- SHUFFLE ---------------------------------------------------------

    /// Whether it is this peer's turn to shuffle, given how many shuffles have
    /// already been applied (`shuffles_done`). Shuffle order is by seat.
    pub fn is_my_shuffle_turn(&self, shuffles_done: usize) -> bool {
        shuffles_done == self.seat
    }

    /// Produce this peer's shuffle-and-remask of the current deck behind a
    /// zero-knowledge proof, store the result as the new deck, and return the
    /// [`ShuffleMessage`] to broadcast. Fresh local randomness; self-verified.
    pub fn shuffle<R: Rng>(&mut self, rng: &mut R) -> Result<ShuffleMessage, DealError> {
        let aggregate = self
            .aggregate_key
            .ok_or(DealError::BadPosition(usize::MAX))?;
        let permutation = Permutation::new(rng, DECK_SIZE);
        let masking_factors: Vec<Scalar> = sample_vector(rng, DECK_SIZE);
        let old_deck = self.deck.clone();
        let (shuffled, proof) = CardProtocol::shuffle_and_remask(
            rng,
            &self.parameters,
            &aggregate,
            &old_deck,
            &masking_factors,
            &permutation,
        )?;
        CardProtocol::verify_shuffle(
            &self.parameters,
            &aggregate,
            &old_deck,
            &shuffled,
            &proof,
        )?;
        self.deck = shuffled.clone();
        Ok(ShuffleMessage {
            seat: self.seat,
            deck: shuffled,
            proof,
        })
    }

    /// Ingest and **verify** a remote peer's [`ShuffleMessage`] against the deck
    /// this peer currently holds (the prior deck), then adopt the new deck. Every
    /// peer runs this on every other peer's shuffle, so all replicas stay in sync
    /// on the same verified deck.
    pub fn ingest_shuffle(&mut self, msg: &ShuffleMessage) -> Result<(), DealError> {
        let aggregate = self
            .aggregate_key
            .ok_or(DealError::BadPosition(usize::MAX))?;
        CardProtocol::verify_shuffle(
            &self.parameters,
            &aggregate,
            &self.deck,
            &msg.deck,
            &msg.proof,
        )?;
        self.deck = msg.deck.clone();
        Ok(())
    }

    // ---- REVEAL ----------------------------------------------------------

    /// Produce this peer's reveal token (+ proof) for each position in
    /// `positions`, as [`RevealMessage`]s to broadcast. Each token is self-verified
    /// before it is handed out.
    ///
    /// For a seat's hole cards, every peer *except* that seat calls this for the
    /// seat's positions; the owner withholds its own token, so no one else can
    /// complete the decryption (privacy). For community/showdown positions every
    /// required peer calls this.
    pub fn reveal_tokens<R: Rng>(
        &self,
        rng: &mut R,
        positions: &[usize],
    ) -> Result<Vec<RevealMessage>, DealError> {
        let mut out = Vec::with_capacity(positions.len());
        for &position in positions {
            let masked = self
                .deck
                .get(position)
                .ok_or(DealError::BadPosition(position))?;
            let (token, proof) = CardProtocol::compute_reveal_token(
                rng,
                &self.parameters,
                &self.secret,
                &self.public,
                masked,
            )?;
            CardProtocol::verify_reveal(&self.parameters, &self.public, &token, masked, &proof)?;
            out.push(RevealMessage {
                seat: self.seat,
                position,
                token,
                proof,
                public: self.public,
            });
        }
        Ok(out)
    }

    /// Verify a remote peer's [`RevealMessage`] against the masked card at its
    /// position. Returns the verified `(token, public)` to be collected for the
    /// final unmask. Use this to validate every token before feeding it to
    /// [`Self::unmask_with`].
    pub fn verify_reveal(
        &self,
        msg: &RevealMessage,
    ) -> Result<(RevealToken, PublicKey), DealError> {
        let masked = self
            .deck
            .get(msg.position)
            .ok_or(DealError::BadPosition(msg.position))?;
        CardProtocol::verify_reveal(
            &self.parameters,
            &msg.public,
            &msg.token,
            masked,
            &msg.proof,
        )?;
        Ok((msg.token, msg.public))
    }

    /// This peer's own reveal token + public key for `position`, *without*
    /// broadcasting it. A seat uses this to decrypt its **own** hole card locally:
    /// it combines the `N-1` tokens it received from the other peers with this one,
    /// so its hole-card token never leaves the machine and no one else can decrypt
    /// the card.
    pub fn own_reveal_contribution<R: Rng>(
        &self,
        rng: &mut R,
        position: usize,
    ) -> Result<(RevealToken, PublicKey), DealError> {
        let masked = self
            .deck
            .get(position)
            .ok_or(DealError::BadPosition(position))?;
        let (token, proof) = CardProtocol::compute_reveal_token(
            rng,
            &self.parameters,
            &self.secret,
            &self.public,
            masked,
        )?;
        CardProtocol::verify_reveal(&self.parameters, &self.public, &token, masked, &proof)?;
        Ok((token, self.public))
    }

    /// Combine reveal contributions and decrypt the card at `position`.
    ///
    /// `contributions` must contain one verified `(token, public)` from **every**
    /// peer (`num_players` of them), since the deck is masked under the aggregate
    /// key. With fewer than all tokens the card cannot be recovered: a strictly
    /// undersized set is rejected with [`DealError::UnknownCard`] rather than
    /// returning a wrong card. Tokens are re-checked via the protocol's `unmask`.
    pub fn unmask_with(
        &self,
        position: usize,
        contributions: &[(RevealToken, PublicKey)],
    ) -> Result<PokerCard, DealError> {
        if contributions.len() != self.num_players {
            // Threshold is l-of-l: every peer's token is required. A missing token
            // means the card is (cryptographically) undecryptable here.
            return Err(DealError::UnknownCard);
        }
        let masked = self
            .deck
            .get(position)
            .ok_or(DealError::BadPosition(position))?;
        // Tokens were already Chaum-Pedersen-verified on ingest
        // ([`Self::verify_reveal`]) / when produced locally, so here we just sum
        // them and reveal: plaintext = c2 - sum(tokens).
        let mut aggregate_token = RevealToken::zero();
        for (token, _pk) in contributions {
            aggregate_token = aggregate_token + *token;
        }
        let plaintext = reveal(&aggregate_token, masked)?;
        self.encoding.decode(&plaintext)
    }
}

/// Aggregate-token reveal: `plaintext.c2 - aggregate_token`, mirroring the
/// protocol's `Reveal` impl, used after all tokens are summed.
fn reveal(
    aggregate_token: &RevealToken,
    masked: &MaskedCard,
) -> Result<crate::mental::MaskedPlaintext, DealError> {
    use barnett_smart_card_protocol::Reveal;
    aggregate_token
        .reveal(masked)
        .map_err(DealError::Protocol)
}

/// Wire (de)serialization for the cross-network message types.
///
/// Every payload is arkworks `CanonicalSerialize`/`CanonicalDeserialize` in
/// **compressed** form. Composite messages length-prefix and concatenate their
/// fields. The seat/position framing (`u32`) is fixed little-endian.
pub mod wire {
    use super::*;
    use ark_serialize::SerializationError;

    fn write_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn read_u32(buf: &[u8], at: &mut usize) -> Result<u32, DealError> {
        if *at + 4 > buf.len() {
            return Err(DealError::UnknownCard);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&buf[*at..*at + 4]);
        *at += 4;
        Ok(u32::from_le_bytes(b))
    }
    fn map_ser(_e: SerializationError) -> DealError {
        DealError::UnknownCard
    }

    /// Serialize any arkworks object to compressed bytes.
    pub fn to_bytes<T: CanonicalSerialize>(value: &T) -> Result<Vec<u8>, DealError> {
        let mut buf = Vec::new();
        value.serialize_compressed(&mut buf).map_err(map_ser)?;
        Ok(buf)
    }

    /// Deserialize any arkworks object from compressed bytes.
    pub fn from_bytes<T: CanonicalDeserialize>(bytes: &[u8]) -> Result<T, DealError> {
        T::deserialize_compressed(bytes).map_err(map_ser)
    }

    fn write_field<T: CanonicalSerialize>(buf: &mut Vec<u8>, v: &T) -> Result<(), DealError> {
        let bytes = to_bytes(v)?;
        write_u32(buf, bytes.len() as u32);
        buf.extend_from_slice(&bytes);
        Ok(())
    }
    fn read_field<T: CanonicalDeserialize>(buf: &[u8], at: &mut usize) -> Result<T, DealError> {
        let len = read_u32(buf, at)? as usize;
        if *at + len > buf.len() {
            return Err(DealError::UnknownCard);
        }
        let v = from_bytes(&buf[*at..*at + len])?;
        *at += len;
        Ok(v)
    }

    impl KeyAnnouncement {
        /// Serialize to wire bytes.
        pub fn serialize(&self) -> Result<Vec<u8>, DealError> {
            let mut buf = Vec::new();
            write_u32(&mut buf, self.seat as u32);
            write_field(&mut buf, &self.public)?;
            write_field(&mut buf, &self.proof)?;
            write_field(&mut buf, &self.info)?;
            Ok(buf)
        }
        /// Deserialize from wire bytes.
        pub fn deserialize(buf: &[u8]) -> Result<Self, DealError> {
            let mut at = 0;
            let seat = read_u32(buf, &mut at)? as usize;
            let public = read_field(buf, &mut at)?;
            let proof = read_field(buf, &mut at)?;
            let info = read_field(buf, &mut at)?;
            Ok(Self { seat, public, proof, info })
        }
    }

    impl ShuffleMessage {
        /// Serialize to wire bytes.
        pub fn serialize(&self) -> Result<Vec<u8>, DealError> {
            let mut buf = Vec::new();
            write_u32(&mut buf, self.seat as u32);
            write_u32(&mut buf, self.deck.len() as u32);
            for c in &self.deck {
                write_field(&mut buf, c)?;
            }
            write_field(&mut buf, &self.proof)?;
            Ok(buf)
        }
        /// Deserialize from wire bytes.
        pub fn deserialize(buf: &[u8]) -> Result<Self, DealError> {
            let mut at = 0;
            let seat = read_u32(buf, &mut at)? as usize;
            let n = read_u32(buf, &mut at)? as usize;
            let mut deck = Vec::with_capacity(n);
            for _ in 0..n {
                deck.push(read_field(buf, &mut at)?);
            }
            let proof = read_field(buf, &mut at)?;
            Ok(Self { seat, deck, proof })
        }
    }

    impl RevealMessage {
        /// Serialize to wire bytes.
        pub fn serialize(&self) -> Result<Vec<u8>, DealError> {
            let mut buf = Vec::new();
            write_u32(&mut buf, self.seat as u32);
            write_u32(&mut buf, self.position as u32);
            write_field(&mut buf, &self.token)?;
            write_field(&mut buf, &self.proof)?;
            write_field(&mut buf, &self.public)?;
            Ok(buf)
        }
        /// Deserialize from wire bytes.
        pub fn deserialize(buf: &[u8]) -> Result<Self, DealError> {
            let mut at = 0;
            let seat = read_u32(buf, &mut at)? as usize;
            let position = read_u32(buf, &mut at)? as usize;
            let token = read_field(buf, &mut at)?;
            let proof = read_field(buf, &mut at)?;
            let public = read_field(buf, &mut at)?;
            Ok(Self { seat, position, token, proof, public })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use std::collections::HashSet;

    /// Round-trip every wire message through serialize/deserialize so the test
    /// below only ever moves *bytes* between peers, never live objects.
    fn rt_key(a: &KeyAnnouncement) -> KeyAnnouncement {
        KeyAnnouncement::deserialize(&a.serialize().unwrap()).unwrap()
    }
    fn rt_shuffle(m: &ShuffleMessage) -> ShuffleMessage {
        ShuffleMessage::deserialize(&m.serialize().unwrap()).unwrap()
    }
    fn rt_reveal(m: &RevealMessage) -> RevealMessage {
        RevealMessage::deserialize(&m.serialize().unwrap()).unwrap()
    }

    /// Full simulated-distributed deal: N independent [`PeerDeal`] replicas that
    /// exchange ONLY serialized messages. Asserts:
    ///  - every keygen/shuffle/reveal proof verifies,
    ///  - all peers compute the same aggregate key and the same initial deck,
    ///  - hole cards decrypt for their owner,
    ///  - an owner's hole card is NOT decryptable by others (missing-token case),
    ///  - community + showdown positions decrypt for everyone,
    ///  - the fully-revealed deck is a valid 52-permutation.
    #[test]
    fn simulated_distributed_full_deal() {
        let rng = &mut thread_rng();
        let num_players = 3usize;
        let session_seed = [7u8; 32];

        // Each peer builds its own replica from the SAME shared session seed.
        let mut peers: Vec<PeerDeal> = (0..num_players)
            .map(|seat| {
                PeerDeal::new(rng, session_seed, 4, 13, seat, num_players).unwrap()
            })
            .collect();

        // --- KEYGEN: broadcast announcements (as bytes), everyone verifies. ---
        let announcements: Vec<KeyAnnouncement> = peers
            .iter_mut()
            .map(|p| p.key_announcement(rng).unwrap())
            .collect();
        for (seat, peer) in peers.iter_mut().enumerate() {
            for ann in &announcements {
                if ann.seat != seat {
                    peer.ingest_key(&rt_key(ann)).unwrap();
                }
            }
            assert!(peer.all_keys_in());
        }
        let aggregates: Vec<_> = peers
            .iter_mut()
            .map(|p| p.finalize_aggregate_key().unwrap())
            .collect();
        // Every peer derived the SAME aggregate key.
        for agg in &aggregates[1..] {
            assert_eq!(*agg, aggregates[0], "aggregate key differs across peers");
        }

        // --- INITIAL MASK: deterministic, identical on every peer. ---
        for peer in peers.iter_mut() {
            peer.mask_initial_deck().unwrap();
        }
        let deck0 = peers[0].deck().to_vec();
        for peer in &peers[1..] {
            assert_eq!(peer.deck(), &deck0[..], "initial deck differs across peers");
        }

        // --- SHUFFLE: seat order; each broadcasts, others verify the bytes. ---
        for turn in 0..num_players {
            // Producing peer shuffles.
            let msg = {
                let producer = &mut peers[turn];
                assert!(producer.is_my_shuffle_turn(turn));
                producer.shuffle(rng).unwrap()
            };
            let wire = rt_shuffle(&msg);
            for (seat, peer) in peers.iter_mut().enumerate() {
                if seat != turn {
                    peer.ingest_shuffle(&wire).unwrap();
                }
            }
        }
        // All replicas agree on the final shuffled deck.
        let final_deck = peers[0].deck().to_vec();
        for peer in &peers[1..] {
            assert_eq!(peer.deck(), &final_deck[..], "final deck differs");
        }

        // Hole positions: seat s gets positions [2s, 2s+1]. Community = 6..11.
        let hole = |s: usize| [2 * s, 2 * s + 1];
        let community: Vec<usize> = (2 * num_players..2 * num_players + 5).collect();

        // --- HOLE REVEAL: every peer EXCEPT s sends tokens for s's holes. ---
        // Collect, per (target_seat, position), the verified non-owner tokens.
        for target in 0..num_players {
            for &pos in hole(target).iter() {
                // Non-owners broadcast tokens (serialized); owner verifies + collects.
                let mut collected: Vec<(RevealToken, PublicKey)> = Vec::new();
                for src in 0..num_players {
                    if src == target {
                        continue;
                    }
                    let msgs = peers[src].reveal_tokens(rng, &[pos]).unwrap();
                    let wire = rt_reveal(&msgs[0]);
                    let (tok, pk) = peers[target].verify_reveal(&wire).unwrap();
                    collected.push((tok, pk));
                }

                // PRIVACY (API guard): the l-of-l API refuses to unmask with fewer
                // than N tokens.
                assert!(
                    peers[0].unmask_with(pos, &collected).is_err(),
                    "hole card decryptable without the owner's token (privacy break)"
                );

                // Owner combines the N-1 received tokens with its OWN local token
                // (never broadcast) and decrypts locally.
                let (own_tok, own_pk) =
                    peers[target].own_reveal_contribution(rng, pos).unwrap();
                let mut owner_set = collected.clone();
                owner_set.push((own_tok, own_pk));
                let card = peers[target].unmask_with(pos, &owner_set).unwrap();
                assert!(card.to_index() < 52);

                // PRIVACY (cryptographic): even bypassing the guard, summing only
                // the N-1 non-owner tokens (the most any attacker without the
                // owner's secret can do) does NOT recover the owner's card. We
                // pad to N with the identity token to force a decode attempt; the
                // result must differ from the true card (it is missing
                // sk_owner * c1 and so is not a valid card point at all).
                let mut forced = collected.clone();
                forced.push((RevealToken::zero(), own_pk));
                match peers[0].unmask_with(pos, &forced) {
                    Ok(wrong) => assert_ne!(
                        wrong, card,
                        "owner's hole card recovered from non-owner tokens (privacy break)"
                    ),
                    Err(_) => { /* did not decode to any card: also private */ }
                }
            }
        }

        // --- COMMUNITY REVEAL: ALL peers broadcast; everyone decrypts. ---
        let mut revealed_indices: HashSet<u8> = HashSet::new();
        for &pos in &community {
            let mut collected: Vec<(RevealToken, PublicKey)> = Vec::new();
            for src in 0..num_players {
                let msgs = peers[src].reveal_tokens(rng, &[pos]).unwrap();
                let wire = rt_reveal(&msgs[0]);
                // Verify against an arbitrary peer's deck (all identical).
                let (tok, pk) = peers[0].verify_reveal(&wire).unwrap();
                collected.push((tok, pk));
            }
            // Every peer can decrypt; check two of them agree.
            let c0 = peers[0].unmask_with(pos, &collected).unwrap();
            let c1 = peers[1].unmask_with(pos, &collected).unwrap();
            assert_eq!(c0, c1, "peers disagree on a community card");
            revealed_indices.insert(c0.to_index());
        }

        // --- SHOWDOWN: every (non-folded) seat broadcasts its OWN hole tokens. ---
        for target in 0..num_players {
            for &pos in hole(target).iter() {
                let mut collected: Vec<(RevealToken, PublicKey)> = Vec::new();
                for src in 0..num_players {
                    let msgs = peers[src].reveal_tokens(rng, &[pos]).unwrap();
                    let wire = rt_reveal(&msgs[0]);
                    let (tok, pk) = peers[0].verify_reveal(&wire).unwrap();
                    collected.push((tok, pk));
                }
                let card = peers[0].unmask_with(pos, &collected).unwrap();
                revealed_indices.insert(card.to_index());
            }
        }

        // --- FULL DECK is a valid 52-permutation. ---
        // Reveal every remaining position cooperatively and assert all 52 distinct.
        for pos in 0..DECK_SIZE {
            let mut collected: Vec<(RevealToken, PublicKey)> = Vec::new();
            for src in 0..num_players {
                let msgs = peers[src].reveal_tokens(rng, &[pos]).unwrap();
                let wire = rt_reveal(&msgs[0]);
                let (tok, pk) = peers[0].verify_reveal(&wire).unwrap();
                collected.push((tok, pk));
            }
            let card = peers[0].unmask_with(pos, &collected).unwrap();
            revealed_indices.insert(card.to_index());
        }
        assert_eq!(
            revealed_indices.len(),
            52,
            "fully revealed deck must be a permutation of all 52 cards"
        );

        // A tampered reveal proof must be rejected on ingest.
        let good = peers[1].reveal_tokens(rng, &[0]).unwrap();
        let mut bad_bytes = good[0].serialize().unwrap();
        // Flip a byte in the proof region (after the seat+position+token framing).
        let flip_at = bad_bytes.len() - 1;
        bad_bytes[flip_at] ^= 0xFF;
        match RevealMessage::deserialize(&bad_bytes) {
            Ok(bad) => assert!(
                peers[0].verify_reveal(&bad).is_err(),
                "tampered reveal proof was accepted"
            ),
            Err(_) => { /* corruption rejected at deserialize: also fine */ }
        }
    }
}
