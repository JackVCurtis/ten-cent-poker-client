//! Integration tests for full-hand money flows through the M1 engine driver.
//!
//! Each test plays a complete hand via [`poker_game::play_hand`] over a *deterministic*
//! hand-crafted deck and a scripted action queue, then asserts BOTH:
//!   1. exact per-seat chip deltas (money flowed exactly where it should), and
//!   2. chip conservation (no chips created or destroyed) — `sum(deltas) == 0` and
//!      `sum(final_stacks) == sum(starting_stacks)`.
//!
//! Decks are laid out to match the driver's documented dealing order:
//!   - hole cards: round-robin, two passes, starting at seat `(button + 1) % n`;
//!   - then burn, 3 flop, burn, 1 turn, burn, 1 river.
//! By placing specific cards at the right indices we control exactly who wins showdowns,
//! so the asserted deltas are forced, not incidental.

use poker_game::{play_hand, Action, Card, Rank, ScriptedActions, Suit};

fn c(r: Rank, s: Suit) -> Card {
    Card { rank: r, suit: s }
}

/// Sum of an i64 delta vector — must be zero for every well-formed hand.
fn delta_sum(deltas: &[i64]) -> i64 {
    deltas.iter().sum()
}

// ---------------------------------------------------------------------------
// Scenario 1: Heads-up blind posting; button folds to the big blind.
// ---------------------------------------------------------------------------
#[test]
fn heads_up_button_folds_to_blind() {
    // Heads-up, button = 0. Button posts SB (5) and acts first preflop; seat 1 posts BB
    // (10). Button folds immediately -> seat 1 wins the 15-chip pot uncontested (its own
    // 10 back + the 5 dead small blind). Net: button -5, BB +5.
    let deck = vec![
        // hole (order: seat1, seat0, seat1, seat0) — values irrelevant, no showdown.
        c(Rank::Two, Suit::Clubs),
        c(Rank::Three, Suit::Clubs),
        c(Rank::Four, Suit::Clubs),
        c(Rank::Five, Suit::Clubs),
        // padding so the deck is never "too small" even though we won't reach a board.
        c(Rank::Six, Suit::Clubs),
        c(Rank::Seven, Suit::Clubs),
        c(Rank::Eight, Suit::Clubs),
        c(Rank::Nine, Suit::Clubs),
        c(Rank::Ten, Suit::Clubs),
        c(Rank::Jack, Suit::Clubs),
        c(Rank::Queen, Suit::Clubs),
        c(Rank::King, Suit::Clubs),
    ];
    let mut src = ScriptedActions::new([Action::Fold]);
    let res = play_hand(vec![1000, 1000], 0, 5, 10, &deck, &mut src).unwrap();

    assert_eq!(res.deltas, vec![-5, 5], "button loses SB, BB wins it");
    assert_eq!(res.final_stacks, vec![995, 1005]);
    assert_eq!(delta_sum(&res.deltas), 0, "chips conserved");
    assert_eq!(
        res.final_stacks.iter().sum::<u64>(),
        2000,
        "total chips conserved"
    );
}

// ---------------------------------------------------------------------------
// Scenario 2: Everyone folds to the blind (3-handed).
// ---------------------------------------------------------------------------
#[test]
fn three_handed_everyone_folds_to_big_blind() {
    // n=3, button=0 => SB=1, BB=2, UTG=0. UTG folds, SB folds, BB wins uncontested.
    // BB collects own 10 back + 5 dead SB = +5; SB -5; UTG 0.
    let deck: Vec<Card> = (0..52u8).map(|i| Card::from_index(i).unwrap()).collect();
    let mut src = ScriptedActions::new([Action::Fold, Action::Fold]);
    let res = play_hand(vec![1000, 1000, 1000], 0, 5, 10, &deck, &mut src).unwrap();

    assert_eq!(res.deltas, vec![0, -5, 5]);
    assert_eq!(res.final_stacks, vec![1000, 995, 1005]);
    assert_eq!(delta_sum(&res.deltas), 0);
    assert_eq!(res.final_stacks.iter().sum::<u64>(), 3000);
}

// ---------------------------------------------------------------------------
// Scenario 3: 3-way, two players all-in for different amounts -> main + side pot.
// ---------------------------------------------------------------------------
#[test]
fn three_way_two_all_ins_main_and_side_pot() {
    // Stacks: seat0 = 1000 (button), seat1 = 100 (SB, short), seat2 = 300 (BB, mid).
    // n=3 button=0 => SB=1, BB=2, UTG=0.
    //
    // Action: UTG(0) raises big, SB(1) all-in 100, BB(2) all-in 300, UTG(0) calls 300.
    //   contributions: seat0=300, seat1=100, seat2=300.
    //   Main pot   = 100*3 = 300, eligible {0,1,2}.
    //   Side pot   = 200*2 = 400, eligible {0,2}  (seat1 capped at 100).
    //
    // Here seat0 (button) has the BEST hand and wins BOTH pots; seat1 and seat2 lose. This
    // checks that the short all-in (seat1) is eligible only for the main pot while the side
    // pot is contested and won. (Scenario 3b covers main and side pots going to DIFFERENT
    // winners.)
    //
    // Deck (n=3, button=0): hole order per index ->
    //   s0 = {deck[2], deck[5]}, s1 = {deck[0], deck[3]}, s2 = {deck[1], deck[4]}
    //   burn=deck[6], flop=deck[7,8,9], burn=deck[10], turn=deck[11], burn=deck[12], river=deck[13]
    //
    // Give seat0 pocket aces, board pairs nothing useful for others.
    let deck = vec![
        // index 0: s1 card A
        c(Rank::Seven, Suit::Diamonds),
        // index 1: s2 card A
        c(Rank::Nine, Suit::Diamonds),
        // index 2: s0 card A
        c(Rank::Ace, Suit::Spades),
        // index 3: s1 card B
        c(Rank::Two, Suit::Diamonds),
        // index 4: s2 card B
        c(Rank::Three, Suit::Clubs),
        // index 5: s0 card B
        c(Rank::Ace, Suit::Hearts),
        // index 6: burn
        c(Rank::Four, Suit::Spades),
        // index 7,8,9: flop
        c(Rank::King, Suit::Clubs),
        c(Rank::Queen, Suit::Diamonds),
        c(Rank::Eight, Suit::Hearts),
        // index 10: burn
        c(Rank::Four, Suit::Hearts),
        // index 11: turn
        c(Rank::Five, Suit::Spades),
        // index 12: burn
        c(Rank::Four, Suit::Diamonds),
        // index 13: river
        c(Rank::Six, Suit::Clubs),
    ];
    // Action sequence consumed in turn order as the engine asks:
    //   UTG(0) raise-to 300, SB(1) all-in (100), BB(2) all-in (300), UTG(0) call.
    let mut src = ScriptedActions::new([
        Action::Raise(300),
        Action::AllIn,
        Action::AllIn,
        Action::Call,
    ]);
    let res = play_hand(vec![1000, 100, 300], 0, 5, 10, &deck, &mut src).unwrap();

    // contributions: seat0=300, seat1=100, seat2=300; total pot 700.
    assert_eq!(res.contributions, vec![300, 100, 300]);
    // seat0 (aces) wins both pots (700): net +400. seat1 -100, seat2 -300.
    assert_eq!(res.deltas, vec![400, -100, -300]);
    assert_eq!(res.final_stacks, vec![1400, 0, 0]);
    assert_eq!(delta_sum(&res.deltas), 0);
    assert_eq!(res.final_stacks.iter().sum::<u64>(), 1400);
}

// ---------------------------------------------------------------------------
// Scenario 3b: side pot won by a DIFFERENT player than the main pot.
// ---------------------------------------------------------------------------
#[test]
fn three_way_side_pot_won_by_non_short_player() {
    // Same shape as scenario 3, but rig hands so the SHORT all-in player (eligible only for
    // the main pot) actually wins the MAIN pot, while a deeper player wins the SIDE pot.
    // This is the canonical "side pot goes to a different winner" case.
    //
    // Stacks: seat0 = 1000 (button), seat1 = 100 (SB short), seat2 = 300 (BB mid).
    //   contributions seat0=300, seat1=100, seat2=300.
    //   Main pot = 300, eligible {0,1,2}; Side pot = 400, eligible {0,2}.
    //
    // Rig: seat1 (short) has the nuts and wins the MAIN pot. Among {0,2} (side pot), seat2
    // beats seat0. Net result:
    //   seat1: wins 300 main, contributed 100 -> +200
    //   seat2: wins 400 side, contributed 300 -> +100
    //   seat0: wins nothing, contributed 300 -> -300
    //
    // hole: s0={deck[2],deck[5]}, s1={deck[0],deck[3]}, s2={deck[1],deck[4]}
    let deck = vec![
        // index 0: s1 card A  -> give seat1 a card toward quads/straight flush nut
        c(Rank::Ace, Suit::Clubs),
        // index 1: s2 card A
        c(Rank::King, Suit::Clubs),
        // index 2: s0 card A
        c(Rank::Seven, Suit::Hearts),
        // index 3: s1 card B
        c(Rank::Ace, Suit::Diamonds),
        // index 4: s2 card B
        c(Rank::King, Suit::Diamonds),
        // index 5: s0 card B
        c(Rank::Two, Suit::Hearts),
        // index 6: burn
        c(Rank::Three, Suit::Spades),
        // index 7,8,9: flop
        c(Rank::Ace, Suit::Spades),
        c(Rank::King, Suit::Spades),
        c(Rank::Nine, Suit::Hearts),
        // index 10: burn
        c(Rank::Four, Suit::Hearts),
        // index 11: turn
        c(Rank::Queen, Suit::Diamonds),
        // index 12: burn
        c(Rank::Five, Suit::Diamonds),
        // index 13: river
        c(Rank::Eight, Suit::Clubs),
    ];
    let mut src = ScriptedActions::new([
        Action::Raise(300),
        Action::AllIn,
        Action::AllIn,
        Action::Call,
    ]);
    let res = play_hand(vec![1000, 100, 300], 0, 5, 10, &deck, &mut src).unwrap();

    assert_eq!(res.contributions, vec![300, 100, 300]);
    assert_eq!(res.deltas, vec![-300, 200, 100]);
    assert_eq!(res.final_stacks, vec![700, 300, 400]);
    assert_eq!(delta_sum(&res.deltas), 0);
    assert_eq!(res.final_stacks.iter().sum::<u64>(), 1400);
}

// ---------------------------------------------------------------------------
// Scenario 4: split pot with an odd chip.
// ---------------------------------------------------------------------------
#[test]
fn split_pot_with_odd_chip() {
    // A two-way tie split over an ODD pot, so exactly one chip cannot divide evenly. The odd
    // chip must go to the first tied winner left of the button (ascending from button+1).
    //
    // A heads-up matched pot is always even (both put in the same amount), so to force an odd
    // two-way pot we use DEAD MONEY from a third player who folds after posting an odd blind.
    // n=3, button=0 (SB=1, BB=2, UTG=0), blinds 5/10:
    //   - UTG(0) calls 10, SB(1) folds (its 5 stays dead), BB(2) checks its option, and the
    //     hand is checked down. Live to showdown: seats 0 and 2; seat 1 forfeited 5.
    //   - contributions: seat0=10, seat1=5, seat2=10  ->  pot = 25 (odd).
    //   - Board is a 5-6-7-8-9 straight; seats 0 and 2 both hold low blanks (2/3) that do not
    //     extend it, so they PLAY THE BOARD and tie.
    //   - share = 25/2 = 12 each, remainder 1. Odd-chip order starts at button+1 = seat 1
    //     (not a winner) then seat 2 (winner) -> seat 2 gets the extra chip.
    //
    // hole (n=3,button=0): s0={deck[2],deck[5]}, s1={deck[0],deck[3]}, s2={deck[1],deck[4]}
    // hole (n=3,button=0): s0={deck[2],deck[5]}, s1={deck[0],deck[3]}, s2={deck[1],deck[4]}
    // board straight 5-6-7-8-9; give s0 and s2 low/irrelevant cards that don't extend it.
    let deck = vec![
        // index 0: s1 card A (folder, irrelevant)
        c(Rank::King, Suit::Clubs),
        // index 1: s2 card A
        c(Rank::Two, Suit::Clubs),
        // index 2: s0 card A
        c(Rank::Two, Suit::Diamonds),
        // index 3: s1 card B
        c(Rank::King, Suit::Diamonds),
        // index 4: s2 card B
        c(Rank::Three, Suit::Diamonds),
        // index 5: s0 card B
        c(Rank::Three, Suit::Hearts),
        // index 6: burn
        c(Rank::Jack, Suit::Spades),
        // index 7,8,9: flop  (straight 5-6-7 ... need 5..9 on the board)
        c(Rank::Five, Suit::Clubs),
        c(Rank::Six, Suit::Diamonds),
        c(Rank::Seven, Suit::Hearts),
        // index 10: burn
        c(Rank::Jack, Suit::Hearts),
        // index 11: turn
        c(Rank::Eight, Suit::Spades),
        // index 12: burn
        c(Rank::Jack, Suit::Diamonds),
        // index 13: river
        c(Rank::Nine, Suit::Clubs),
    ];
    // Preflop: UTG(0) call 10, SB(1) fold, BB(2) check option. Then check the hand down.
    // Postflop first-to-act is the first active seat left of the button (seat 1 folded, so
    // seat 2), then seat 0. We must supply explicit checks: an exhausted action queue
    // FOLDS, and folding is legal when facing no bet, which would end the hand early.
    let mut src = ScriptedActions::new([
        Action::Call,  // preflop UTG(0)
        Action::Fold,  // preflop SB(1) folds (its 5 stays dead in the pot)
        Action::Check, // preflop BB(2) option
        Action::Check, // flop seat 2
        Action::Check, // flop seat 0
        Action::Check, // turn seat 2
        Action::Check, // turn seat 0
        Action::Check, // river seat 2
        Action::Check, // river seat 0
    ]);
    let res = play_hand(vec![1000, 1000, 1000], 0, 5, 10, &deck, &mut src).unwrap();

    assert_eq!(res.contributions, vec![10, 5, 10]);
    // Pot 25 split between seats 0 and 2: 12 each + 1 odd chip to seat 2 (first winner left
    // of button 0). seat0: +12-10 = +2; seat2: +13-10 = +3; seat1: -5.
    assert_eq!(res.deltas, vec![2, -5, 3]);
    assert_eq!(res.final_stacks, vec![1002, 995, 1003]);
    assert_eq!(delta_sum(&res.deltas), 0);
    assert_eq!(res.final_stacks.iter().sum::<u64>(), 3000);
}

// ---------------------------------------------------------------------------
// Scenario 5: a hand checked down to showdown (heads-up), known winner.
// ---------------------------------------------------------------------------
#[test]
fn heads_up_checked_down_to_showdown() {
    // Heads-up, button=0. Button (SB) limps (calls to 10), BB checks option, then both
    // check every street to showdown. Pot = 20. Rig the board+holes so seat 0 wins
    // outright (a flush only seat 0 makes).
    //
    // hole (n=2,button=0): s0={deck[1],deck[3]}, s1={deck[0],deck[2]}
    // burn=deck[4], flop=deck[5,6,7], burn=deck[8], turn=deck[9], burn=deck[10], river=deck[11]
    //
    // Give seat0 two hearts; put three hearts on the board so only seat0 makes a flush.
    let deck = vec![
        // index 0: s1 card A
        c(Rank::King, Suit::Clubs),
        // index 1: s0 card A
        c(Rank::Ace, Suit::Hearts),
        // index 2: s1 card B
        c(Rank::Queen, Suit::Clubs),
        // index 3: s0 card B
        c(Rank::Jack, Suit::Hearts),
        // index 4: burn
        c(Rank::Two, Suit::Spades),
        // index 5,6,7: flop (two hearts + a blank)
        c(Rank::Three, Suit::Hearts),
        c(Rank::Seven, Suit::Hearts),
        c(Rank::Nine, Suit::Clubs),
        // index 8: burn
        c(Rank::Two, Suit::Diamonds),
        // index 9: turn (third heart -> seat0 flush)
        c(Rank::Five, Suit::Hearts),
        // index 10: burn
        c(Rank::Three, Suit::Diamonds),
        // index 11: river (blank)
        c(Rank::Four, Suit::Clubs),
    ];
    // Button(0) call, BB(1) check; flop BB check, button check; turn BB check, button check;
    // river BB check, button check.
    let mut src = ScriptedActions::new([
        Action::Call,
        Action::Check,
        Action::Check,
        Action::Check,
        Action::Check,
        Action::Check,
        Action::Check,
        Action::Check,
    ]);
    let res = play_hand(vec![1000, 1000], 0, 5, 10, &deck, &mut src).unwrap();

    assert_eq!(res.community.len(), 5, "checked down to a full board");
    assert_eq!(res.contributions, vec![10, 10]);
    // seat0 makes a heart flush, wins the 20-chip pot: +10. seat1: -10.
    assert_eq!(res.deltas, vec![10, -10]);
    assert_eq!(res.final_stacks, vec![1010, 990]);
    assert_eq!(delta_sum(&res.deltas), 0);
    assert_eq!(res.final_stacks.iter().sum::<u64>(), 2000);
}
