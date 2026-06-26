//! PURE projection from the live [`crate::gui_state::GuiState`] snapshot (filled by the p2p driver)
//! into the egui-free render [`crate::freeplay::model::Table`]. This is the seam that lets the
//! polished free-play UI render REAL networked state without the model knowing anything about
//! async/runtime/egui types.
//!
//! Two responsibilities:
//! 1. Card formatting — [`card_token`] / [`cards_tokens`] emit the LOWERCASE-suit tokens
//!    [`crate::freeplay::cards::parse`] consumes (`"As Kd 7c"`, `T` for ten). NOTE: this is
//!    deliberately NOT [`crate::gui_state::card_str`], which emits unicode glyphs (`A♠`) the card
//!    parser cannot read.
//! 2. Roster rotation — [`project`] rotates the seating so the LOCAL player (`gui.my_seat`) lands at
//!    `seats[0]`, the model's "You" position (see [`crate::freeplay::model::Table::hero_stack`] /
//!    [`crate::freeplay::model::Table::acting_name`]). The dealer/acting indices rotate with it.

use crate::freeplay::model::{Game, Seat, Table, TIMER_TOTAL_MS};
use crate::gui_state::GuiState;
use poker_game::{Rank, Suit};

/// Format one [`poker_game::Card`] as a parser-ready token: rank char(s) (`T` for ten) followed by a
/// LOWERCASE suit letter `s`/`h`/`d`/`c` — e.g. `As`, `Kd`, `Tc`, `2h`.
pub fn card_token(c: &poker_game::Card) -> String {
    let rank = match c.rank {
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
    let suit = match c.suit {
        Suit::Spades => 's',
        Suit::Hearts => 'h',
        Suit::Diamonds => 'd',
        Suit::Clubs => 'c',
    };
    format!("{rank}{suit}")
}

/// Format a slice of cards as a single space-separated token string (`"As Kd 7c"`); an empty slice
/// yields an empty string. Round-trips through [`crate::freeplay::cards::parse`].
pub fn cards_tokens(cards: &[poker_game::Card]) -> String {
    cards.iter().map(card_token).collect::<Vec<_>>().join(" ")
}

/// Project the live snapshot into a single render [`Table`] with the given tile `id`. The roster is
/// rotated so `gui.my_seat` becomes `seats[0]`; `dealer`/`act` are rotated to match; board and hero
/// cards are formatted via [`cards_tokens`]; pot/to_call/your_turn and the legal-action bounds are
/// copied from the snapshot; `invite_uri`/`reachability` pass through.
pub fn project(gui: &GuiState, id: u64) -> Table {
    let n = gui.seats.len();
    // Rotate so the LOCAL player (gui.my_seat) lands at seats[0] — the model's "You" position.
    let offset = gui.my_seat.unwrap_or(0);
    // Map an original seat index into the rotated (model) index. Guarded against an empty roster.
    let rotate = |orig: usize| -> usize {
        if n == 0 {
            0
        } else {
            (orig + n - offset % n) % n
        }
    };

    let mut seats = Vec::with_capacity(n);
    for i in 0..n {
        let sv = &gui.seats[(offset + i) % n];
        // An empty label marks an open seat in the snapshot; everything else is a seated player.
        if sv.label.is_empty() {
            seats.push(Seat::Empty);
        } else {
            seats.push(Seat::Filled {
                name: sv.label.clone(),
                stack: sv.stack,
                folded: sv.folded,
            });
        }
    }

    let dealer = gui.button.map(rotate).unwrap_or(0);
    let act = gui
        .seats
        .iter()
        .position(|s| s.is_to_act)
        .map(rotate)
        .unwrap_or(0);

    let hero = gui.my_hole.map(|h| cards_tokens(&h)).unwrap_or_default();

    Table {
        id,
        name: "Free Table".to_string(),
        game: Game::Holdem,
        // Placeholder display: the snapshot does not carry the live blinds, so the app overwrites
        // this with the host's configured blinds (`FreePlayApp::sync_conn`) to match the engine.
        blinds: "20 / 40".to_string(),
        pot: gui.pot,
        board: cards_tokens(&gui.board),
        hero,
        seats,
        dealer,
        act,
        // Match `ui.rs`: it is only your turn once the deal is ready. In a trustless hand the betting
        // state can exist while the mental deal is still completing (`gui.dealing`), so suppress the
        // action bar / turn timer until cards are actually dealt.
        your_turn: gui.is_my_turn && !gui.dealing,
        to_call: gui.legal.call_amount,
        time_left: TIMER_TOTAL_MS,
        invite_uri: gui.table_uri.clone(),
        reachability: gui.reachability_warning.clone(),
        can_check: gui.legal.can_check,
        can_call: gui.legal.can_call,
        can_bet: gui.legal.can_bet,
        can_raise: gui.legal.can_raise,
        can_all_in: gui.legal.can_all_in,
        min_bet: gui.legal.min_bet,
        min_raise_to: gui.legal.min_raise_to,
        max_to: gui.legal.max_to,
        // The sizing target is owned by the app and written in after projection (clamped into the
        // active legal range); start it at the minimum legal bet/raise as a sane default.
        bet_to: if gui.legal.can_bet {
            gui.legal.min_bet
        } else {
            gui.legal.min_raise_to
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freeplay::cards;
    use crate::freeplay::model::Seat;
    use crate::gui_state::{LegalActions, SeatView};
    use poker_game::{Card, Rank, Suit};

    fn card(rank: Rank, suit: Suit) -> Card {
        Card { rank, suit }
    }

    fn seat(label: &str, stack: u64, folded: bool) -> SeatView {
        SeatView {
            seat: 0,
            label: label.to_string(),
            stack,
            committed: 0,
            folded,
            all_in: false,
            is_button: false,
            is_me: false,
            is_to_act: false,
        }
    }

    #[test]
    fn card_token_formats_rank_and_lowercase_suit() {
        assert_eq!(card_token(&card(Rank::Ace, Suit::Spades)), "As");
        assert_eq!(card_token(&card(Rank::King, Suit::Diamonds)), "Kd");
        assert_eq!(card_token(&card(Rank::Ten, Suit::Clubs)), "Tc");
        assert_eq!(card_token(&card(Rank::Two, Suit::Hearts)), "2h");
        assert_eq!(card_token(&card(Rank::Queen, Suit::Hearts)), "Qh");
        assert_eq!(card_token(&card(Rank::Jack, Suit::Clubs)), "Jc");
    }

    #[test]
    fn cards_tokens_empty_is_blank() {
        assert_eq!(cards_tokens(&[]), "");
    }

    #[test]
    fn cards_tokens_round_trips_through_cards_parse() {
        let hand = [
            card(Rank::Ace, Suit::Spades),
            card(Rank::King, Suit::Diamonds),
            card(Rank::Ten, Suit::Clubs),
            card(Rank::Two, Suit::Hearts),
        ];
        let tokens = cards_tokens(&hand);
        assert_eq!(tokens, "As Kd Tc 2h");

        // The token string the model carries must be readable by the card renderer's parser.
        let parsed = cards::parse(&tokens);
        assert_eq!(parsed.len(), hand.len());
        let ranks: Vec<&str> = parsed.iter().map(|c| c.rank.as_str()).collect();
        assert_eq!(ranks, vec!["A", "K", "10", "2"]);
        let reds: Vec<bool> = parsed.iter().map(|c| c.red).collect();
        assert_eq!(reds, vec![false, true, false, true]);
    }

    /// Build a 4-seat live snapshot (local seat = 1, button = 2, seat-3 to act, seat-0 open) and
    /// assert the projection rotates the hero to `seats[0]`, maps Filled/Empty correctly, rotates
    /// dealer/act, formats cards, and copies pot/to_call/your_turn + the legal bounds and invite.
    #[test]
    fn project_rotates_hero_to_seat0_and_maps_fields() {
        let mut seats = vec![
            seat("", 0, false),       // orig 0 — open (empty label)
            seat("you", 1500, false), // orig 1 — local seat
            seat("bob", 900, false),  // orig 2 — button
            seat("carol", 700, true), // orig 3 — to act, folded
        ];
        seats[1].is_me = true;
        seats[2].is_button = true;
        seats[3].is_to_act = true;

        let gui = GuiState {
            table_uri: Some("tcpoker://invite".to_string()),
            reachability_warning: Some("private address".to_string()),
            pot: 320,
            board: vec![
                card(Rank::Ace, Suit::Spades),
                card(Rank::King, Suit::Diamonds),
                card(Rank::Seven, Suit::Clubs),
            ],
            my_seat: Some(1),
            my_hole: Some([
                card(Rank::Seven, Suit::Diamonds),
                card(Rank::Seven, Suit::Clubs),
            ]),
            button: Some(2),
            seats,
            is_my_turn: true,
            legal: LegalActions {
                can_check: false,
                can_call: true,
                call_amount: 40,
                call_is_all_in: false,
                can_bet: false,
                can_raise: true,
                can_all_in: true,
                min_bet: 10,
                min_raise_to: 80,
                max_to: 1500,
            },
            ..GuiState::default()
        };

        let t = project(&gui, 7);

        assert_eq!(t.id, 7);

        // Rotation: model seat i == original seat (my_seat + i) % n.
        assert_eq!(t.seats.len(), 4);
        match &t.seats[0] {
            Seat::Filled {
                name,
                stack,
                folded,
            } => {
                assert_eq!(name, "you");
                assert_eq!(*stack, 1500);
                assert!(!folded);
            }
            Seat::Empty => panic!("hero seat must be Filled"),
        }
        match &t.seats[1] {
            Seat::Filled { name, stack, .. } => {
                assert_eq!(name, "bob");
                assert_eq!(*stack, 900);
            }
            Seat::Empty => panic!("seat 1 must be Filled (bob)"),
        }
        match &t.seats[2] {
            Seat::Filled { name, folded, .. } => {
                assert_eq!(name, "carol");
                assert!(*folded, "carol folded");
            }
            Seat::Empty => panic!("seat 2 must be Filled (carol)"),
        }
        assert!(t.seats[3].is_empty(), "the open seat maps to Seat::Empty");

        // Hero stack reads seats[0].
        assert_eq!(t.hero_stack(), 1500);

        // Dealer/act rotate with the roster: button orig 2 -> model 1; to-act orig 3 -> model 2.
        assert_eq!(t.dealer, 1);
        assert_eq!(t.act, 2);

        // Card tokens are parser-ready (lowercase suits).
        assert_eq!(t.board, "As Kd 7c");
        assert_eq!(t.hero, "7d 7c");

        // Scalar passthrough.
        assert_eq!(t.pot, 320);
        assert_eq!(t.to_call, 40);
        assert!(t.your_turn);

        // Legal-action bounds mirror the snapshot.
        assert!(!t.can_check);
        assert!(t.can_call);
        assert!(!t.can_bet);
        assert!(t.can_raise);
        assert!(t.can_all_in);
        assert_eq!(t.min_bet, 10);
        assert_eq!(t.min_raise_to, 80);
        assert_eq!(t.max_to, 1500);

        // Invite + reachability pass through.
        assert_eq!(t.invite_uri.as_deref(), Some("tcpoker://invite"));
        assert_eq!(t.reachability.as_deref(), Some("private address"));
    }
}
