//! Independent, hand-checked Texas Hold'em ranking vectors for `poker_game::hand_eval`.
//!
//! These tests are deliberately written from scratch (own card parser, own expected
//! tiebreak vectors) so they exercise the evaluator independently of the engine's own
//! unit tests. Every vector below was hand-verified against standard poker rules.
//!
//! Conventions being tested (per the M1 engine contract):
//!   * `HandRank` derives `Ord`, weakest -> strongest:
//!       HighCard < Pair < TwoPair < ThreeOfAKind < Straight < Flush
//!                < FullHouse < FourOfAKind < StraightFlush
//!   * `HandValue` compares `(rank, tiebreak)` lexicographically.
//!   * `tiebreak` holds raw rank values (Two=0 .. Ace=12) in descending priority.
//!   * The wheel A-2-3-4-5 is a Five-high straight (high card = Five = 3).
//!   * Suits are unranked.

use std::cmp::Ordering;

use poker_game::hand_eval::{evaluate_best, HandRank, HandValue};
use poker_game::{Card, Rank, Suit};

// ---------------------------------------------------------------------------
// Independent test helpers (not shared with engine source).
// ---------------------------------------------------------------------------

/// Raw rank value as the evaluator encodes it: Two=0 .. Ace=12.
fn rv(r: Rank) -> u8 {
    r as u8
}

fn parse_rank(b: u8) -> Rank {
    match b {
        b'2' => Rank::Two,
        b'3' => Rank::Three,
        b'4' => Rank::Four,
        b'5' => Rank::Five,
        b'6' => Rank::Six,
        b'7' => Rank::Seven,
        b'8' => Rank::Eight,
        b'9' => Rank::Nine,
        b'T' => Rank::Ten,
        b'J' => Rank::Jack,
        b'Q' => Rank::Queen,
        b'K' => Rank::King,
        b'A' => Rank::Ace,
        other => panic!("bad rank byte: {}", other as char),
    }
}

fn parse_suit(b: u8) -> Suit {
    match b {
        b'c' => Suit::Clubs,
        b'd' => Suit::Diamonds,
        b'h' => Suit::Hearts,
        b's' => Suit::Spades,
        other => panic!("bad suit byte: {}", other as char),
    }
}

/// Parse a space-separated hand like "Ah Kd Qc Js Th" into cards.
fn cards(spec: &str) -> Vec<Card> {
    let v: Vec<Card> = spec
        .split_whitespace()
        .map(|t| {
            let b = t.as_bytes();
            assert_eq!(b.len(), 2, "token {t:?} must be rank+suit");
            Card {
                rank: parse_rank(b[0]),
                suit: parse_suit(b[1]),
            }
        })
        .collect();
    // Guard against accidental duplicate cards in a single vector (would be an
    // illegal deal and silently corrupt the expected result).
    for i in 0..v.len() {
        for j in (i + 1)..v.len() {
            assert!(
                v[i] != v[j],
                "duplicate card {:?} in spec {:?}",
                v[i],
                spec
            );
        }
    }
    v
}

fn eval(spec: &str) -> HandValue {
    evaluate_best(&cards(spec)).unwrap_or_else(|| panic!("evaluate_best returned None for {spec:?}"))
}

/// Assert classification (rank) and exact tiebreak vector.
fn assert_hand(spec: &str, rank: HandRank, tiebreak: Vec<u8>) {
    let v = eval(spec);
    assert_eq!(v.rank, rank, "rank mismatch for {spec:?}: got {:?}", v.rank);
    assert_eq!(
        v.tiebreak, tiebreak,
        "tiebreak mismatch for {spec:?}: got {:?}, expected {:?}",
        v.tiebreak, tiebreak
    );
}

/// Assert `a` strictly beats `b` and the reverse comparison holds.
fn assert_beats(a: &str, b: &str) {
    let (va, vb) = (eval(a), eval(b));
    assert_eq!(
        va.cmp(&vb),
        Ordering::Greater,
        "expected {a:?} > {b:?}; got {va:?} vs {vb:?}"
    );
    assert_eq!(
        vb.cmp(&va),
        Ordering::Less,
        "expected {b:?} < {a:?}; got {vb:?} vs {va:?}"
    );
    assert!(va > vb);
    assert!(vb < va);
}

/// Assert `a` and `b` evaluate to an exact tie.
fn assert_ties(a: &str, b: &str) {
    let (va, vb) = (eval(a), eval(b));
    assert_eq!(
        va.cmp(&vb),
        Ordering::Equal,
        "expected {a:?} == {b:?}; got {va:?} vs {vb:?}"
    );
    assert_eq!(va, vb, "expected {a:?} == {b:?}");
}

// ---------------------------------------------------------------------------
// 1. Classification of each category, with exact hand-checked tiebreaks.
// ---------------------------------------------------------------------------

#[test]
fn classify_high_card() {
    // A K Q J 9 (no straight, no flush, no pair). tiebreak = ranks descending.
    assert_hand(
        "Ah Kd Qc Js 9h",
        HandRank::HighCard,
        vec![rv(Rank::Ace), rv(Rank::King), rv(Rank::Queen), rv(Rank::Jack), rv(Rank::Nine)],
    );
}

#[test]
fn classify_pair() {
    // Pair of nines, kickers A K 7.
    assert_hand(
        "9h 9s Ah Kd 7c",
        HandRank::Pair,
        vec![rv(Rank::Nine), rv(Rank::Ace), rv(Rank::King), rv(Rank::Seven)],
    );
}

#[test]
fn classify_two_pair() {
    // Kings and sevens, kicker A. [hi_pair, lo_pair, kicker].
    assert_hand(
        "Kh Ks 7d 7c Ah",
        HandRank::TwoPair,
        vec![rv(Rank::King), rv(Rank::Seven), rv(Rank::Ace)],
    );
}

#[test]
fn classify_three_of_a_kind() {
    // Trip fives, kickers A K. [trip, k1, k2].
    assert_hand(
        "5h 5s 5d Ah Kc",
        HandRank::ThreeOfAKind,
        vec![rv(Rank::Five), rv(Rank::Ace), rv(Rank::King)],
    );
}

#[test]
fn classify_straight() {
    // 9-high straight 9 8 7 6 5, mixed suits.
    assert_hand("9h 8s 7d 6c 5h", HandRank::Straight, vec![rv(Rank::Nine)]);
}

#[test]
fn classify_flush() {
    // Heart flush A Q 9 5 2; tiebreak = all five ranks descending.
    assert_hand(
        "Ah Qh 9h 5h 2h",
        HandRank::Flush,
        vec![rv(Rank::Ace), rv(Rank::Queen), rv(Rank::Nine), rv(Rank::Five), rv(Rank::Two)],
    );
}

#[test]
fn classify_full_house() {
    // Queens full of fives. [trip, pair].
    assert_hand(
        "Qh Qs Qd 5c 5h",
        HandRank::FullHouse,
        vec![rv(Rank::Queen), rv(Rank::Five)],
    );
}

#[test]
fn classify_four_of_a_kind() {
    // Quad eights, kicker K. [quad, kicker].
    assert_hand(
        "8h 8s 8d 8c Kh",
        HandRank::FourOfAKind,
        vec![rv(Rank::Eight), rv(Rank::King)],
    );
}

#[test]
fn classify_straight_flush() {
    // T-high straight flush in spades, tiebreak = [Ten].
    assert_hand("Ts 9s 8s 7s 6s", HandRank::StraightFlush, vec![rv(Rank::Ten)]);
}

#[test]
fn classify_royal_flush_is_ace_high_straight_flush() {
    // Royal flush is just an Ace-high straight flush; tiebreak = [Ace].
    assert_hand("Ah Kh Qh Jh Th", HandRank::StraightFlush, vec![rv(Rank::Ace)]);
}

// ---------------------------------------------------------------------------
// 2. The wheel (A-2-3-4-5).
// ---------------------------------------------------------------------------

#[test]
fn wheel_is_five_high_straight() {
    // A-2-3-4-5: straight, high card = Five (not Ace).
    assert_hand("Ah 2s 3d 4c 5h", HandRank::Straight, vec![rv(Rank::Five)]);
}

#[test]
fn wheel_straight_flush_is_five_high() {
    assert_hand("As 2s 3s 4s 5s", HandRank::StraightFlush, vec![rv(Rank::Five)]);
}

#[test]
fn wheel_loses_to_six_high_straight() {
    // 6-5-4-3-2 beats A-5-4-3-2.
    assert_beats("6h 5s 4d 3c 2h", "Ah 5s 4d 3c 2c");
}

#[test]
fn wheel_loses_to_ace_high_straight() {
    // A-K-Q-J-T (Ace high) beats A-5-4-3-2 (wheel, Five high).
    assert_beats("Ah Ks Qd Jc Th", "As 5h 4d 3c 2s");
}

#[test]
fn wheel_straight_flush_loses_to_six_high_straight_flush() {
    assert_beats("6h 5h 4h 3h 2h", "Ah 5h 4h 3h 2h");
}

// ---------------------------------------------------------------------------
// 3. Cross-category orderings, including the classic flush-over-straight.
// ---------------------------------------------------------------------------

#[test]
fn flush_beats_straight() {
    // The headline cross-category check: any flush > any straight.
    // 2-high flush (lowest possible flush) still beats an Ace-high straight.
    assert_beats("7h 5h 4h 3h 2h", "Ah Ks Qd Jc Th");
}

#[test]
fn straight_beats_three_of_a_kind() {
    assert_beats("9h 8s 7d 6c 5h", "Ah As Ad Kc Qh");
}

#[test]
fn three_of_a_kind_beats_two_pair() {
    assert_beats("2h 2s 2d 3c 4h", "Ah As Kd Ks Qh");
}

#[test]
fn two_pair_beats_pair() {
    assert_beats("3h 3s 2d 2c 4h", "Ah As Kd Qc Jh");
}

#[test]
fn pair_beats_high_card() {
    assert_beats("2h 2s 3d 4c 5h", "Ah Ks Qd Jc 9h");
}

#[test]
fn full_house_beats_flush() {
    assert_beats("2h 2s 2d 3c 3h", "Ah Kh Qh Jh 9h");
}

#[test]
fn four_of_a_kind_beats_full_house() {
    assert_beats("2h 2s 2d 2c 3h", "Ah As Ad Kc Kh");
}

#[test]
fn straight_flush_beats_four_of_a_kind() {
    assert_beats("6h 5h 4h 3h 2h", "Ah As Ad Ac Kh");
}

#[test]
fn full_chain_strictly_increasing() {
    // One representative hand per category, ascending. Verify each strictly
    // beats the previous via the same Ord that production uses.
    let chain = [
        "Ah Kd Qc Js 9h", // high card
        "9h 9s Ah Kd 7c", // pair
        "Kh Ks 7d 7c Ah", // two pair
        "5h 5s 5d Ah Kc", // trips
        "9h 8s 7d 6c 5h", // straight
        "Ah Qh 9h 5h 2h", // flush
        "Qh Qs Qd 5c 5h", // full house
        "8h 8s 8d 8c Kh", // quads
        "Ts 9s 8s 7s 6s", // straight flush
    ];
    for w in chain.windows(2) {
        assert_beats(w[1], w[0]);
    }
}

// ---------------------------------------------------------------------------
// 4. Full-house tiebreak (trips first, then pair).
// ---------------------------------------------------------------------------

#[test]
fn full_house_compares_trips_first() {
    // 999 over 22 beats 888 over AA, even though AA > 22 as the pair.
    assert_beats("9h 9s 9d 2c 2h", "8h 8s 8d Ac Ah");
}

#[test]
fn full_house_pair_breaks_equal_trips() {
    // Same trips (KKK); pair AA beats pair 22.
    assert_beats("Kh Ks Kd Ac Ah", "Kc Kh Ks 2c 2h"); // note: distinct cards
}

#[test]
fn full_house_exact_tie_different_suits() {
    // Identical KKK-over-QQ, suits irrelevant.
    assert_ties("Kh Ks Kd Qc Qh", "Kc Ks Kh Qd Qs"); // distinct cards within each
}

// ---------------------------------------------------------------------------
// 5. Kicker tiebreaks across categories.
// ---------------------------------------------------------------------------

#[test]
fn pair_kicker_chain() {
    // Pair of aces: K kicker > Q kicker (first kicker decides).
    assert_beats("Ah As Kd 5c 2h", "Ah As Qd 5c 2h");
    // Equal top kicker, second kicker decides: A-A-K-Q vs A-A-K-J.
    assert_beats("Ah As Kd Qc 2h", "Ah As Kd Jc 2h");
    // Equal top two kickers, third decides: A-A-K-Q-9 vs A-A-K-Q-8.
    assert_beats("Ah As Kd Qc 9h", "Ah As Kd Qc 8h");
}

#[test]
fn two_pair_tiebreak_chain() {
    // Higher top pair wins regardless of second pair / kicker:
    // AA + 22 beats KK + QQ.
    assert_beats("Ah As 2d 2c 5h", "Kh Ks Qd Qc Ah");
    // Equal top pair (AA), higher second pair wins: AA+KK > AA+QQ.
    assert_beats("Ah As Kd Kc 2h", "Ah As Qd Qc Kh");
    // Equal both pairs (AA+KK), kicker decides: kicker Q > kicker J.
    assert_beats("Ah As Kd Kc Qh", "Ah As Kd Kc Jh");
}

#[test]
fn trips_kicker_chain() {
    // Equal trips (777), kickers decide: A-K > A-Q.
    assert_beats("7h 7s 7d Ac Kh", "7h 7s 7d Ac Qh");
    // Equal trips and first kicker (A), second kicker: K > Q.
    // (uses the same A kicker; second kicker breaks)
    assert_beats("7h 7s 7d Ac Kh", "7c 7d 7s Ah Qd");
}

#[test]
fn quads_kicker_breaks_tie() {
    // Equal quads (QQQQ), kicker decides: A > K.
    assert_beats("Qh Qs Qd Qc Ah", "Qh Qs Qd Qc Kh");
}

#[test]
fn high_card_kicker_chain() {
    // A-K-Q-J-9 beats A-K-Q-J-8 (last card decides).
    assert_beats("Ah Kd Qc Js 9h", "Ah Kd Qc Js 8h");
    // A-K-Q-J-9 beats A-K-Q-T-9 (fourth card decides).
    assert_beats("Ah Kd Qc Js 9h", "Ah Kd Qc Ts 9h");
}

#[test]
fn flush_high_card_chain() {
    // Ace-high flush beats King-high flush.
    assert_beats("Ah Qh 9h 5h 2h", "Kh Qh 9h 5h 2h");
    // Equal down to the last card: flush ...3 beats flush ...2.
    assert_beats("Ah Qh 9h 5h 3h", "Ah Qh 9h 5h 2h");
}

#[test]
fn straight_high_card_decides() {
    // Ace-high straight beats King-high straight.
    assert_beats("Ah Ks Qd Jc Th", "Kh Qs Jd Tc 9h");
}

#[test]
fn straight_flush_high_card_decides() {
    // King-high straight flush beats Queen-high straight flush.
    assert_beats("Kh Qh Jh Th 9h", "Qs Js Ts 9s 8s");
}

// ---------------------------------------------------------------------------
// 6. 7-card best-5 selection (and 6-card).
// ---------------------------------------------------------------------------

#[test]
fn seven_card_picks_flush_over_pair() {
    // Five hearts present plus a pair of nines off-suit. Best 5 = the flush.
    let v = evaluate_best(&cards("Ah Kh Qh 2h 3h 9s 9d")).unwrap();
    assert_eq!(v.rank, HandRank::Flush);
    assert_eq!(
        v.tiebreak,
        vec![rv(Rank::Ace), rv(Rank::King), rv(Rank::Queen), rv(Rank::Three), rv(Rank::Two)],
    );
}

#[test]
fn seven_card_picks_straight_over_lesser_pairs() {
    // 9-8-7-6-5 straight available; ignore the trailing 5d/2c pair noise.
    let v = evaluate_best(&cards("9h 8s 7d 6c 5h 5d 2c")).unwrap();
    assert_eq!(v.rank, HandRank::Straight);
    assert_eq!(v.tiebreak, vec![rv(Rank::Nine)]);
}

#[test]
fn seven_card_picks_best_full_house() {
    // Two trips present (AAA and KKK); best full house is AAA over KK.
    let v = evaluate_best(&cards("Ah As Ad Kh Ks Kd 2c")).unwrap();
    assert_eq!(v.rank, HandRank::FullHouse);
    assert_eq!(v.tiebreak, vec![rv(Rank::Ace), rv(Rank::King)]);
}

#[test]
fn seven_card_picks_quads_with_best_kicker() {
    // Quad sevens + spare A, K, 3 -> best kicker is the Ace.
    let v = evaluate_best(&cards("7h 7s 7d 7c Ah Kd 3c")).unwrap();
    assert_eq!(v.rank, HandRank::FourOfAKind);
    assert_eq!(v.tiebreak, vec![rv(Rank::Seven), rv(Rank::Ace)]);
}

#[test]
fn seven_card_picks_straight_flush_over_higher_flush() {
    // Cards: 6h5h4h3h2h (steel wheel SF) plus Ah Kh would be a higher flush,
    // but a straight flush outranks a flush. Use 9h8h7h6h5h plus Ah Kh:
    // best is the 9-high straight flush, not the Ace-high flush.
    let v = evaluate_best(&cards("9h 8h 7h 6h 5h Ah Kh")).unwrap();
    assert_eq!(v.rank, HandRank::StraightFlush);
    assert_eq!(v.tiebreak, vec![rv(Rank::Nine)]);
}

#[test]
fn seven_card_wheel_straight_from_scattered_cards() {
    // A,2,3,4,5 present among 7; best straight is the Five-high wheel.
    let v = evaluate_best(&cards("Ah 2d 3c 4s 5h Kd Qc")).unwrap();
    assert_eq!(v.rank, HandRank::Straight);
    assert_eq!(v.tiebreak, vec![rv(Rank::Five)]);
}

#[test]
fn six_card_input_supported() {
    // Quads available among 6 cards.
    let v = evaluate_best(&cards("Ah As Ad Ac Kh Kd")).unwrap();
    assert_eq!(v.rank, HandRank::FourOfAKind);
    assert_eq!(v.tiebreak, vec![rv(Rank::Ace), rv(Rank::King)]);
}

#[test]
fn seven_card_board_plays_two_pair_with_best_kicker() {
    // KK + 77 with kickers A, Q, 4 among 7 -> two pair K/7, kicker A.
    let v = evaluate_best(&cards("Kh Ks 7d 7c Ah Qd 4s")).unwrap();
    assert_eq!(v.rank, HandRank::TwoPair);
    assert_eq!(
        v.tiebreak,
        vec![rv(Rank::King), rv(Rank::Seven), rv(Rank::Ace)],
    );
}

// ---------------------------------------------------------------------------
// 7. Fewer than 5 cards -> None.
// ---------------------------------------------------------------------------

#[test]
fn fewer_than_five_cards_is_none() {
    assert!(evaluate_best(&cards("Ah Ks Qd Jc")).is_none());
    assert!(evaluate_best(&cards("Ah Ks")).is_none());
    assert!(evaluate_best(&[]).is_none());
}

// ---------------------------------------------------------------------------
// 8. Exact ties (suits unranked) and Ord consistency.
// ---------------------------------------------------------------------------

#[test]
fn identical_royal_flushes_tie() {
    assert_ties("Ah Kh Qh Jh Th", "As Ks Qs Js Ts");
}

#[test]
fn same_two_pair_same_kicker_ties() {
    // AA + KK + Q kicker, different suit assignments -> exact tie.
    assert_ties("Ah As Kd Kc Qh", "Ad Ac Kh Ks Qd");
}

#[test]
fn same_straight_different_suits_ties() {
    // T-high straight, different suit composition (one not a flush) -> tie.
    assert_ties("Th 9s 8d 7c 6h", "Td 9h 8c 7s 6d");
}

#[test]
fn seven_card_equal_best_five_ties() {
    // Two 7-card sets that both make the identical best-5 (board-plays straight),
    // with different (irrelevant) hole cards -> exact tie.
    let a = evaluate_best(&cards("Th 9h 8h 7s 6c 2d 3d")).unwrap();
    let b = evaluate_best(&cards("Ts 9s 8c 7d 6h 2c 4c")).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.cmp(&b), Ordering::Equal);
    assert_eq!(a.rank, HandRank::Straight);
    assert_eq!(a.tiebreak, vec![rv(Rank::Ten)]);
}

#[test]
fn ord_is_total_and_consistent() {
    // Sanity: a < b and b < c implies a < c across categories, using cmp.
    let a = eval("Ah Kd Qc Js 9h"); // high card
    let b = eval("9h 8s 7d 6c 5h"); // straight
    let c = eval("Ts 9s 8s 7s 6s"); // straight flush
    assert_eq!(a.cmp(&b), Ordering::Less);
    assert_eq!(b.cmp(&c), Ordering::Less);
    assert_eq!(a.cmp(&c), Ordering::Less);
    // Reflexive equality.
    assert_eq!(a.cmp(&a.clone()), Ordering::Equal);
}
