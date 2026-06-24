//! Peer-to-peer poker protocol: the transport-agnostic wire messages and the replicated
//! state machine that sequences a Texas Hold'em hand across the table.
//!
//! This crate is deliberately independent of *how* bytes move: [`TableMessage`] defines
//! *what* peers say. The host-as-hub relays gossipsub messages but, because gossipsub runs
//! in `StrictSign` mode, the `from` PeerId surfaced by the net layer is the
//! cryptographically authenticated original publisher — a malicious host can stall or drop
//! a message but cannot forge or alter another peer's. The replicated [`table::Table`]
//! leans on that for anti-cheat: a betting [`Action`] is only applied if its author owns the
//! seat that is actually to act.
//!
//! Layering:
//! - [`table`] — the PURE, deterministic, unit-testable replicated state machine. No async,
//!   no networking. Every peer runs it and reaches identical state from identical messages.
//! - [`driver`] — async glue binding a [`poker_net::Node`] to a [`table::Table`].
//! - [`bot`] — trivial deterministic strategies for headless / bot play.

use serde::{Deserialize, Serialize};

pub mod bot;
pub mod driver;
pub mod mental;
pub mod table;

pub use bot::{CallStationBot, CheckFoldBot, Strategy};
pub use driver::{
    run_guest, run_guest_with_config, run_host, DriverError, GameReport, HandReport, HostOptions,
};
pub use mental::{DealEffect, DealPhase};
pub use table::{HandOutcome, Step, Table, TableError, TableEvent};
// Re-export the card type used in public report/outcome structs so CLI/UI callers can format
// hole cards and the board without depending on `poker-game` directly.
pub use poker_game::Card;

/// A player's betting action during a hand. Amounts are in chips (see [`poker_game::Chips`]).
///
/// Mirrors [`poker_game::Action`] but lives here so the wire format is owned by the protocol
/// crate; [`Action::to_game`] / [`Action::from_game`] convert between the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Fold,
    Check,
    Call,
    Bet(u64),
    Raise(u64),
    AllIn,
}

impl Action {
    /// Convert to the engine's action type.
    pub fn to_game(self) -> poker_game::Action {
        match self {
            Action::Fold => poker_game::Action::Fold,
            Action::Check => poker_game::Action::Check,
            Action::Call => poker_game::Action::Call,
            Action::Bet(n) => poker_game::Action::Bet(n),
            Action::Raise(n) => poker_game::Action::Raise(n),
            Action::AllIn => poker_game::Action::AllIn,
        }
    }

    /// Convert from the engine's action type.
    pub fn from_game(a: poker_game::Action) -> Action {
        match a {
            poker_game::Action::Fold => Action::Fold,
            poker_game::Action::Check => Action::Check,
            poker_game::Action::Call => Action::Call,
            poker_game::Action::Bet(n) => Action::Bet(n),
            poker_game::Action::Raise(n) => Action::Raise(n),
            poker_game::Action::AllIn => Action::AllIn,
        }
    }
}

/// Which reveal phase a batch of [`TableMessage::RevealAnnounce`] tokens belongs to.
///
/// Reveal rounds happen at fixed points in the hand and target fixed deck positions, so a peer
/// can collect the correct l-of-l token set per round. A reveal for the wrong round (or wrong
/// positions for the live betting street) is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RevealRound {
    /// Hole-card reveal: the announcing seat contributes its token for EVERY OTHER seat's two
    /// hole positions (never its own), so each owner — and only that owner — can combine the
    /// `N-1` received tokens with its own withheld token to decrypt its hand locally.
    Hole,
    /// Community reveal for a street's board positions; ALL seats contribute, everyone decrypts.
    Flop,
    Turn,
    River,
    /// Showdown: each non-folded seat contributes its token for its OWN hole positions so every
    /// peer can decrypt the contested hands and compute the winner.
    Showdown,
}

/// A table-level protocol message exchanged over the gossipsub table topic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableMessage {
    /// Lobby: a peer requests a seat at the table (client -> host).
    JoinTable { display_name: String },

    /// Authoritative hand start (HOST -> all). Defines the entire deterministic context for
    /// the hand so every peer builds an identical [`poker_game::BettingState`] and deck:
    /// - `hand_no`: monotonically increasing hand counter.
    /// - `button`: dealer button seat index for this hand.
    /// - `seed`: drives [`poker_deal::placeholder_shuffled_deck`].
    /// - `seats`: PeerId bytes in seat order (seat `i` == `PeerId::from_bytes(&seats[i])`).
    /// - `stacks`: starting stack per seat, parallel to `seats`.
    /// - `small_blind` / `big_blind`: blind sizes.
    ///
    /// SECURITY: the host choosing `seed` is an INSECURE PLACEHOLDER. Anyone who knows the
    /// seed knows the whole deck, so the host (and anyone observing the message) can see every
    /// hole card. This exists only to wire the game/net/protocol layers end-to-end before the
    /// M2 trustless Barnett–Smart deal replaces the seed+placeholder shuffle behind this seam.
    StartHand {
        hand_no: u64,
        button: usize,
        seed: u64,
        seats: Vec<Vec<u8>>,
        stacks: Vec<u64>,
        small_blind: u64,
        big_blind: u64,
    },

    /// A betting action taken by the seat currently to act (that seat -> all). Validity is
    /// checked by every peer against the authenticated publisher (see [`table::Table::handle`]).
    Act { hand_no: u64, action: Action },

    /// Authoritative start of a TRUSTLESS (Barnett–Smart mental-poker) hand (HOST -> all).
    ///
    /// Like [`TableMessage::StartHand`] it fixes the deterministic betting context (button,
    /// blinds, seats, stacks), but instead of a deck `seed` it carries a `session_seed`: a
    /// shared 32-byte seed (agreed out of band / chosen by the host per hand) that every peer
    /// feeds to [`poker_deal::distributed::PeerDeal::new`] to derive byte-identical scheme
    /// [`poker_deal::distributed`]`::Parameters` with NO exchange. There is NO public deck: the
    /// cards are dealt by the interactive distributed protocol (keygen -> mask -> shuffle ->
    /// threshold reveal) carried by the [`KeyAnnounce`](Self::KeyAnnounce),
    /// [`ShuffleAnnounce`](Self::ShuffleAnnounce) and [`RevealAnnounce`](Self::RevealAnnounce)
    /// messages below. Knowing `session_seed` reveals only the public scheme parameters, never
    /// any card — hiding comes from each peer's secret key + shuffle randomness.
    StartMentalHand {
        hand_no: u64,
        button: usize,
        /// Shared seed for deterministic scheme parameters (32 bytes).
        session_seed: Vec<u8>,
        seats: Vec<Vec<u8>>,
        stacks: Vec<u64>,
        small_blind: u64,
        big_blind: u64,
    },

    /// KEYGEN (any seat -> all): this seat's serialized
    /// [`poker_deal::distributed::KeyAnnouncement`] (public key + Schnorr ownership proof).
    /// Every peer verifies the proof and folds the key into the aggregate key in seat order.
    KeyAnnounce {
        hand_no: u64,
        seat: usize,
        payload: Vec<u8>,
    },

    /// SHUFFLE (the seat whose turn it is -> all): this seat's serialized
    /// [`poker_deal::distributed::ShuffleMessage`] (new deck + Bayer–Groth proof). Accepted
    /// only when `turn` matches the number of shuffles already applied AND `seat == turn`; the
    /// proof is verified against the prior deck before the new deck is adopted.
    ShuffleAnnounce {
        hand_no: u64,
        /// Shuffle turn index (0..num_players); must equal `seat` and the shuffles-done count.
        turn: usize,
        payload: Vec<u8>,
    },

    /// REVEAL (any seat -> all): a batch of this seat's serialized
    /// [`poker_deal::distributed::RevealMessage`]s (partial-decryption token +
    /// Chaum–Pedersen proof per deck position) for one reveal round. `round` identifies which
    /// reveal phase the tokens belong to (hole / a community street / showdown) so peers can
    /// collect the right l-of-l token set. Each token is verified against the masked card at
    /// its position before being collected.
    RevealAnnounce {
        hand_no: u64,
        seat: usize,
        round: RevealRound,
        /// One serialized `RevealMessage` per element.
        tokens: Vec<Vec<u8>>,
    },

    /// A non-host peer announces it has finished applying hand `hand_no` (its replicated table
    /// settled). The host waits for one `HandComplete` from every seated guest before dealing
    /// the next hand, so the table self-paces to the slowest peer and a fast host never resets
    /// a hand a guest has not yet settled. Carries the peer's computed final stacks so the host
    /// can detect (and, in future, reconcile) any divergence; in M4 it is advisory.
    HandComplete { hand_no: u64, final_stacks: Vec<u64> },

    /// Opaque deal-protocol payload (masking / shuffle / reveal blobs); structure defined in
    /// M2 once the Barnett–Smart types exist. Unused by the M4 replicated state machine.
    DealPayload { hand_no: u64, bytes: Vec<u8> },
}

impl TableMessage {
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<TableMessage, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// A signed envelope wrapping a serialized [`TableMessage`]. RETAINED from M0 for callers
/// that want an explicit author+signature framing. The M4 replicated path does NOT use it:
/// gossipsub's `StrictSign` already authenticates the publisher (`NodeEvent::Message.from`),
/// so the driver passes raw [`TableMessage`] bytes and trusts the net-layer `from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// libp2p `PeerId` bytes of the author.
    pub from: Vec<u8>,
    /// Encoded [`TableMessage`] (see [`TableMessage::encode`]).
    pub payload: Vec<u8>,
    /// Ed25519 signature by the author over `payload`.
    pub signature: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_message_roundtrips() {
        let msg = TableMessage::Act {
            hand_no: 3,
            action: Action::Raise(50),
        };
        let bytes = msg.encode().unwrap();
        assert_eq!(TableMessage::decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn start_hand_roundtrips() {
        let msg = TableMessage::StartHand {
            hand_no: 1,
            button: 0,
            seed: 42,
            seats: vec![vec![1, 2, 3], vec![4, 5, 6]],
            stacks: vec![1000, 1000],
            small_blind: 5,
            big_blind: 10,
        };
        let bytes = msg.encode().unwrap();
        assert_eq!(TableMessage::decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn envelope_roundtrips() {
        let env = Envelope {
            from: vec![1, 2, 3],
            payload: TableMessage::JoinTable {
                display_name: "alice".into(),
            }
            .encode()
            .unwrap(),
            signature: vec![7, 7, 7],
        };
        let json = serde_json::to_vec(&env).unwrap();
        assert_eq!(serde_json::from_slice::<Envelope>(&json).unwrap(), env);
    }

    #[test]
    fn action_converts_both_ways() {
        for a in [
            Action::Fold,
            Action::Check,
            Action::Call,
            Action::Bet(20),
            Action::Raise(40),
            Action::AllIn,
        ] {
            assert_eq!(Action::from_game(a.to_game()), a);
        }
    }
}
