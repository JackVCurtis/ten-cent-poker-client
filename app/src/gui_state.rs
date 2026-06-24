//! The shared, owned snapshot the egui front-end renders, plus the builder that fills it from the
//! replicated [`poker_protocol::Table`]. The driver task (on the tokio runtime) writes this under a
//! mutex via the observer; the eframe UI thread reads a copy each frame. Keeping the snapshot owned
//! (no borrows of `Table`) is what lets the two threads share it through a plain `std::sync::Mutex`.

use poker_game::{BettingState, Card, Chips, Rank, Street, Suit};
use poker_protocol::{DealPhase, HandOutcome, Table};

/// Which side of the table this client is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Guest,
}

/// Top-level screen the app shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Lobby,
    Table,
}

/// Connection / game lifecycle, as far as the UI is concerned.
#[derive(Clone, PartialEq, Eq)]
pub enum Conn {
    Idle,
    /// Guest is dialing the host, or host is starting up.
    Connecting,
    /// Host is waiting for enough players to join.
    Waiting,
    /// A hand is in progress.
    Playing,
    /// The host finished its hand run (clean end).
    GameOver,
    /// The table aborted (a player left, a net/decode error, …).
    Error(String),
}

impl Default for Conn {
    fn default() -> Self {
        Conn::Idle
    }
}

/// One seat's renderable state.
#[derive(Clone)]
pub struct SeatView {
    pub seat: usize,
    pub label: String,
    pub stack: Chips,
    /// Chips committed on the current street (shown "in front" of the seat).
    pub committed: Chips,
    pub folded: bool,
    pub all_in: bool,
    pub is_button: bool,
    pub is_me: bool,
    pub is_to_act: bool,
}

/// Precomputed legal actions for the local seat's action bar. Advisory only — the betting engine is
/// authoritative, so an illegal submission is simply ignored and the bar stays live for a retry.
#[derive(Clone, Default)]
pub struct LegalActions {
    pub can_check: bool,
    pub can_call: bool,
    pub call_amount: Chips,
    /// A call that would put the seat all-in (mapped to `Action::AllIn`).
    pub call_is_all_in: bool,
    /// No outstanding bet to call — opening a bet is allowed.
    pub can_bet: bool,
    /// There is an outstanding bet — raising is allowed.
    pub can_raise: bool,
    pub can_all_in: bool,
    /// Minimum legal opening bet (= big blind, clamped to stack).
    pub min_bet: Chips,
    /// Minimum legal raise-to total.
    pub min_raise_to: Chips,
    /// Maximum bet/raise-to (the seat's all-in size).
    pub max_to: Chips,
}

/// The result of one completed hand, for the results panel.
#[derive(Clone)]
pub struct ResultView {
    pub hand_no: u64,
    pub board: Vec<Card>,
    pub my_hole: Option<[Card; 2]>,
    pub deltas: Vec<i64>,
    pub final_stacks: Vec<u64>,
}

impl ResultView {
    pub fn from_outcome(o: &HandOutcome, my_hole: Option<[Card; 2]>) -> Self {
        ResultView {
            hand_no: o.hand_no,
            board: o.community.clone(),
            my_hole,
            deltas: o.deltas.clone(),
            final_stacks: o.final_stacks.clone(),
        }
    }
}

/// The whole renderable snapshot.
#[derive(Default, Clone)]
pub struct GuiState {
    pub role: Option<Role>,
    pub conn: Conn,
    pub my_peer_short: String,
    /// Host's shareable `tcpoker://` URI (to copy and send to guests).
    pub table_uri: Option<String>,
    pub reachability_warning: Option<String>,

    pub hand_no: Option<u64>,
    pub is_mental: bool,
    pub deal_phase: Option<DealPhase>,
    /// True while the trustless deal is running and betting cannot proceed yet.
    pub dealing: bool,
    pub street: Option<Street>,
    pub button: Option<usize>,
    pub pot: Chips,
    pub board: Vec<Card>,
    pub my_seat: Option<usize>,
    pub my_hole: Option<[Card; 2]>,
    pub seats: Vec<SeatView>,
    pub is_my_turn: bool,
    pub legal: LegalActions,

    /// Most recent completed hand (results panel).
    pub last_result: Option<ResultView>,
}

impl GuiState {
    /// Rebuild the live-hand portion of the snapshot from the replicated table.
    pub fn update_from_table(&mut self, t: &Table) {
        self.my_seat = t.local_seat();
        self.hand_no = t.live_hand_no();
        self.is_mental = t.is_mental_hand();
        self.deal_phase = t.deal_phase();
        self.board = t.community().to_vec();
        self.my_hole = t.local_hole();
        let roster = t.roster();

        match t.betting() {
            Some(b) => {
                self.street = Some(b.street);
                self.button = Some(b.button);
                self.pot = b.contributions().iter().copied().sum();
                self.is_my_turn = t.is_local_turn();
                let to_act = b.to_act();
                self.seats = b
                    .seats
                    .iter()
                    .enumerate()
                    .map(|(i, s)| SeatView {
                        seat: i,
                        label: seat_label(roster.get(i).map(|p| p.to_string()), i, self.my_seat),
                        stack: s.stack,
                        committed: s.committed,
                        folded: s.folded,
                        all_in: s.all_in,
                        is_button: i == b.button,
                        is_me: Some(i) == self.my_seat,
                        is_to_act: Some(i) == to_act,
                    })
                    .collect();
                self.legal = match (self.is_my_turn, self.my_seat) {
                    (true, Some(seat)) => compute_legal(b, seat),
                    _ => LegalActions::default(),
                };
                // A mental hand is still "dealing" until the deck is Ready and our hole decrypts.
                self.dealing = t.is_mental_hand()
                    && (t.deal_phase() != Some(DealPhase::Ready) || t.local_hole().is_none());
            }
            None => {
                // No live betting state yet (deal in progress, or between hands).
                self.is_my_turn = false;
                self.legal = LegalActions::default();
                self.dealing = t.is_mental_hand() && t.live_hand_no().is_some();
                self.seats = roster
                    .iter()
                    .enumerate()
                    .map(|(i, _)| SeatView {
                        seat: i,
                        label: seat_label(roster.get(i).map(|p| p.to_string()), i, self.my_seat),
                        stack: 0,
                        committed: 0,
                        folded: false,
                        all_in: false,
                        is_button: false,
                        is_me: Some(i) == self.my_seat,
                        is_to_act: false,
                    })
                    .collect();
            }
        }
    }
}

/// Compute the local seat's legal actions from the live betting state (advisory — see
/// [`LegalActions`]).
fn compute_legal(b: &BettingState, seat: usize) -> LegalActions {
    let to_call = b.to_call(seat);
    let stack = b.seats[seat].stack;
    let committed = b.seats[seat].committed;
    let can_check = to_call == 0;
    let can_call = to_call > 0 && stack > 0;
    let call_is_all_in = can_call && to_call >= stack;
    // Open a bet only when nothing is owed; raise only when there is a bet and we can exceed a call.
    let can_bet = b.current_bet == 0 && stack > 0;
    let can_raise = b.current_bet > 0 && stack > to_call;
    LegalActions {
        can_check,
        can_call,
        call_amount: to_call,
        call_is_all_in,
        can_bet,
        can_raise,
        can_all_in: stack > 0,
        min_bet: b.big_blind.min(stack),
        min_raise_to: (b.current_bet + b.min_raise).min(stack + committed),
        max_to: stack + committed,
    }
}

/// A short, human-ish label for a seat: "you" for the local seat, "host" for seat 0, otherwise a
/// truncated PeerId (`peer` is the already-formatted PeerId string, so this module needs no libp2p
/// dependency).
fn seat_label(peer: Option<String>, i: usize, me: Option<usize>) -> String {
    if Some(i) == me {
        return "you".to_string();
    }
    if i == 0 {
        return "host".to_string();
    }
    match peer {
        Some(s) => format!("seat {i} (…{})", &s[s.len().saturating_sub(6)..]),
        None => format!("seat {i}"),
    }
}

/// Render a card compactly, e.g. `A♠`, `T♥`, `2♣`.
pub fn card_str(c: &Card) -> String {
    let r = match c.rank {
        Rank::Two => "2",
        Rank::Three => "3",
        Rank::Four => "4",
        Rank::Five => "5",
        Rank::Six => "6",
        Rank::Seven => "7",
        Rank::Eight => "8",
        Rank::Nine => "9",
        Rank::Ten => "T",
        Rank::Jack => "J",
        Rank::Queen => "Q",
        Rank::King => "K",
        Rank::Ace => "A",
    };
    let s = match c.suit {
        Suit::Clubs => "♣",
        Suit::Diamonds => "♦",
        Suit::Hearts => "♥",
        Suit::Spades => "♠",
    };
    format!("{r}{s}")
}
