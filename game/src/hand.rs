//! Full-hand driver: deal cards, run betting per street, reach showdown, return deltas.
//!
//! The driver is pure and deterministic: the deck order and the source of player actions
//! are both inputs. No I/O, no randomness. Standard Hold'em dealing order is used:
//! hole cards are dealt one at a time, starting from the first seat left of the button,
//! for two passes (so each seat gets two cards). A single card is **burned** before the
//! flop, the turn, and the river (standard live convention; documented so deck authors
//! lay out the deck accordingly).
//!
//! Dealing layout for `num_seats = N` from `deck`:
//! - cards `0..2N`: hole cards (round-robin, see above)
//! - burn, then 3 flop cards, burn, 1 turn card, burn, 1 river card.

use crate::betting::{Action, BettingError, BettingState, Street};
use crate::pot::compute_pots;
use crate::showdown::{distribute, PotAward};
use crate::{Card, Chips};
use thiserror::Error;

/// Supplies the next action for a seat facing a decision. Implementors decide actions;
/// the engine only asks. This is how tests inject deterministic play.
pub trait ActionSource {
    /// Return the action seat `seat` takes given the current betting state.
    fn next_action(&mut self, state: &BettingState, seat: usize) -> Action;
}

/// A simple scripted action source: a queue of actions consumed in order. Useful for
/// tests. Panics-free: if it runs out, it folds.
pub struct ScriptedActions {
    queue: std::collections::VecDeque<Action>,
}

impl ScriptedActions {
    pub fn new(actions: impl IntoIterator<Item = Action>) -> Self {
        ScriptedActions {
            queue: actions.into_iter().collect(),
        }
    }
}

impl ActionSource for ScriptedActions {
    fn next_action(&mut self, _state: &BettingState, _seat: usize) -> Action {
        self.queue.pop_front().unwrap_or(Action::Fold)
    }
}

/// Errors from running a full hand.
#[derive(Debug, Error)]
pub enum HandError {
    #[error("betting error: {0}")]
    Betting(#[from] BettingError),
    #[error("deck too small: need {needed} cards, have {have}")]
    DeckTooSmall { needed: usize, have: usize },
}

/// The result of playing one full hand.
#[derive(Clone, Debug)]
pub struct HandResult {
    /// Each seat's two hole cards.
    pub hole: Vec<[Card; 2]>,
    /// The community board actually dealt (0..=5 cards).
    pub community: Vec<Card>,
    /// Per-seat net chip delta for the hand (winnings minus total contributed).
    pub deltas: Vec<i64>,
    /// Per-seat final stacks after the hand.
    pub final_stacks: Vec<Chips>,
    /// Per-pot awards (for display / auditing).
    pub awards: Vec<PotAward>,
    /// Per-seat total contributions to the pot(s).
    pub contributions: Vec<Chips>,
}

/// Deal hole cards round-robin, one at a time, starting from the first seat left of the
/// button, for two passes — the canonical Hold'em layout. Consumes cards `0..2*num_seats`
/// from the front of `deck`.
///
/// This is the single source of truth for the hole-card layout: the local [`play_hand`]
/// driver and the networked replicated `Table` both call it so every peer deals identically.
pub fn deal_hole(deck: &[Card], num_seats: usize, button: usize) -> Result<Vec<[Card; 2]>, HandError> {
    let n = num_seats;
    let needed_hole = 2 * n;
    if deck.len() < needed_hole {
        return Err(HandError::DeckTooSmall {
            needed: needed_hole,
            have: deck.len(),
        });
    }
    let mut idx = 0usize;
    let mut hole: Vec<[Option<Card>; 2]> = vec![[None, None]; n];
    for round in 0..2 {
        let mut seat = (button + 1) % n;
        for _ in 0..n {
            hole[seat][round] = Some(deck[idx]);
            idx += 1;
            seat = (seat + 1) % n;
        }
    }
    Ok(hole
        .iter()
        .map(|h| [h[0].unwrap(), h[1].unwrap()])
        .collect())
}

/// The deck index immediately after the hole cards (where community dealing begins).
pub fn hole_card_count(num_seats: usize) -> usize {
    2 * num_seats
}

/// Deal the community cards for one street from `deck`, advancing `*idx` past the burn and
/// the board cards, appending to `community`. Mirrors the live-poker burn convention used
/// by [`play_hand`]; the networked `Table` reuses it so boards match across peers.
pub fn deal_community_street(
    street: Street,
    deck: &[Card],
    idx: &mut usize,
    community: &mut Vec<Card>,
) -> Result<(), HandError> {
    deal_street(street, deck, idx, community)
}

/// Play a complete hand and return the result.
///
/// - `stacks`: starting stacks per seat (length 2..=9).
/// - `button`: dealer button seat index.
/// - `small_blind` / `big_blind`: blind sizes.
/// - `deck`: pre-arranged deck order (the driver consumes from the front).
/// - `actions`: source of player decisions.
pub fn play_hand(
    stacks: Vec<Chips>,
    button: usize,
    small_blind: Chips,
    big_blind: Chips,
    deck: &[Card],
    actions: &mut dyn ActionSource,
) -> Result<HandResult, HandError> {
    let n = stacks.len();
    let starting = stacks.clone();
    let mut state = BettingState::new(stacks, button, small_blind, big_blind)?;

    // Deal hole cards round-robin starting left of button (shared dealing layout).
    let hole_cards: Vec<[Card; 2]> = deal_hole(deck, n, button)?;
    let mut idx = hole_card_count(n);

    let mut community: Vec<Card> = Vec::with_capacity(5);

    // Run betting street by street, dealing community cards as we advance.
    run_betting_round(&mut state, actions);

    loop {
        if state.hand_over_by_folds() {
            break;
        }
        if state.betting_complete_for_hand() {
            // Run out remaining community cards with no more betting.
            deal_remaining_community(&state, deck, &mut idx, &mut community)?;
            break;
        }
        // Advance to next street; deal its community card(s).
        match state.next_street() {
            None => break, // river betting done
            Some(street) => {
                deal_street(street, deck, &mut idx, &mut community)?;
                run_betting_round(&mut state, actions);
            }
        }
    }

    // Compute pots and distribute.
    let contributions = state.contributions();
    let folded = state.folded_flags();
    let pots = compute_pots(&contributions, &folded);

    // Build hole option list: None for folded seats so they cannot win.
    let hole_for_showdown: Vec<Option<[Card; 2]>> = (0..n)
        .map(|i| {
            if folded[i] {
                None
            } else {
                Some(hole_cards[i])
            }
        })
        .collect();

    let (winnings, awards) = distribute(&pots, &hole_for_showdown, &community, state.button, n);

    // Final stacks = leftover stack (uncommitted) + winnings. Note total_committed left
    // the stack already, so leftover stack is starting - total_committed.
    let mut final_stacks = vec![0u64; n];
    let mut deltas = vec![0i64; n];
    for i in 0..n {
        let leftover = starting[i] - contributions[i];
        final_stacks[i] = leftover + winnings[i];
        deltas[i] = final_stacks[i] as i64 - starting[i] as i64;
    }

    Ok(HandResult {
        hole: hole_cards,
        community,
        deltas,
        final_stacks,
        awards,
        contributions,
    })
}

/// Drive one betting round to completion by repeatedly asking the action source.
fn run_betting_round(state: &mut BettingState, actions: &mut dyn ActionSource) {
    let mut guard = 0;
    while let Some(seat) = state.to_act() {
        let action = actions.next_action(state, seat);
        // If the action is illegal, fall back to a safe legal action so the engine never
        // deadlocks: check if possible else fold. (Driver must not panic.)
        let result = state.apply(seat, action);
        if result.is_err() {
            let fallback = if state.to_call(seat) == 0 {
                Action::Check
            } else {
                Action::Fold
            };
            let _ = state.apply(seat, fallback);
        }
        guard += 1;
        if guard > 10_000 {
            break; // safety valve; should never trigger
        }
    }
}

fn deal_street(
    street: Street,
    deck: &[Card],
    idx: &mut usize,
    community: &mut Vec<Card>,
) -> Result<(), HandError> {
    match street {
        Street::Flop => {
            burn(deck, idx)?;
            for _ in 0..3 {
                community.push(take(deck, idx)?);
            }
        }
        Street::Turn | Street::River => {
            burn(deck, idx)?;
            community.push(take(deck, idx)?);
        }
        Street::Preflop => {}
    }
    Ok(())
}

/// Deal whatever community cards remain (flop/turn/river) without betting.
fn deal_remaining_community(
    state: &BettingState,
    deck: &[Card],
    idx: &mut usize,
    community: &mut Vec<Card>,
) -> Result<(), HandError> {
    // Determine which streets remain based on current street and board size.
    let mut street = state.street;
    while community.len() < 5 {
        let next = match street {
            Street::Preflop => Street::Flop,
            Street::Flop => Street::Turn,
            Street::Turn => Street::River,
            Street::River => break,
        };
        deal_street(next, deck, idx, community)?;
        street = next;
    }
    Ok(())
}

fn burn(deck: &[Card], idx: &mut usize) -> Result<(), HandError> {
    take(deck, idx).map(|_| ())
}

fn take(deck: &[Card], idx: &mut usize) -> Result<Card, HandError> {
    if *idx >= deck.len() {
        return Err(HandError::DeckTooSmall {
            needed: *idx + 1,
            have: deck.len(),
        });
    }
    let c = deck[*idx];
    *idx += 1;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, Rank, Suit};

    fn ordered_deck() -> Vec<Card> {
        (0..52u8).map(|i| Card::from_index(i).unwrap()).collect()
    }

    fn c(r: Rank, s: Suit) -> Card {
        Card { rank: r, suit: s }
    }

    #[test]
    fn fold_around_gives_pot_to_last_standing() {
        // 3 handed, everyone folds to BB.
        let mut actions = ScriptedActions::new([Action::Fold, Action::Fold]);
        let res = play_hand(
            vec![1000, 1000, 1000],
            0,
            5,
            10,
            &ordered_deck(),
            &mut actions,
        )
        .unwrap();
        // UTG(0) and SB(1) fold; BB(2) wins SB+BB = 15 net.
        // Seat 2 collects the pot of 15 (own 10 back + 5 from SB).
        assert_eq!(res.deltas[0], 0);
        assert_eq!(res.deltas[1], -5);
        assert_eq!(res.deltas[2], 5);
        // Chips conserved.
        assert_eq!(res.deltas.iter().sum::<i64>(), 0);
    }

    #[test]
    fn heads_up_checkdown_to_showdown_conserves_chips() {
        // Heads up. Button calls, BB checks, then check it all down.
        let actions = vec![
            Action::Call,  // preflop button (SB)
            Action::Check, // preflop BB option
            Action::Check, // flop BB
            Action::Check, // flop button
            Action::Check, // turn BB
            Action::Check, // turn button
            Action::Check, // river BB
            Action::Check, // river button
        ];
        let mut src = ScriptedActions::new(actions);
        let res = play_hand(vec![1000, 1000], 0, 5, 10, &ordered_deck(), &mut src).unwrap();
        assert_eq!(res.community.len(), 5);
        assert_eq!(res.deltas.iter().sum::<i64>(), 0);
        // Total final chips conserved.
        assert_eq!(res.final_stacks.iter().sum::<u64>(), 2000);
    }

    #[test]
    fn all_in_runs_out_board() {
        // Heads up, button shoves all-in, BB calls. Board runs out, chips conserved.
        let actions = vec![Action::AllIn, Action::Call];
        let mut src = ScriptedActions::new(actions);
        let res = play_hand(vec![500, 500], 0, 5, 10, &ordered_deck(), &mut src).unwrap();
        assert_eq!(res.community.len(), 5);
        assert_eq!(res.final_stacks.iter().sum::<u64>(), 1000);
        // One player has everything (or split).
        let total: u64 = res.final_stacks.iter().sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn deck_too_small_errors() {
        let short: Vec<Card> = vec![c(Rank::Ace, Suit::Hearts); 3];
        let mut src = ScriptedActions::new([]);
        let r = play_hand(vec![100, 100], 0, 5, 10, &short, &mut src);
        assert!(matches!(r, Err(HandError::DeckTooSmall { .. })));
    }

    #[test]
    fn chips_conserved_with_side_pot() {
        // 3 players, short stack all-in creating a side pot.
        // Stacks: 0=1000 (button), 1=50 (SB short), 2=1000 (BB).
        // Preflop: UTG is seat0? n=3 button=0 => SB=1, BB=2, UTG=0.
        let actions = vec![
            Action::Call,  // UTG(0) calls 10
            Action::AllIn, // SB(1) all-in 50
            Action::Call,  // BB(2) calls
            Action::Call,  // UTG(0) calls the raise
            // flop/turn/river: remaining active players check down
            Action::Check,
            Action::Check,
            Action::Check,
            Action::Check,
            Action::Check,
            Action::Check,
        ];
        let mut src = ScriptedActions::new(actions);
        let res = play_hand(vec![1000, 50, 1000], 0, 5, 10, &ordered_deck(), &mut src).unwrap();
        assert_eq!(res.final_stacks.iter().sum::<u64>(), 2050);
        assert_eq!(res.deltas.iter().sum::<i64>(), 0);
    }
}
