//! 5-card hand evaluation and 7→best-5 selection.
//!
//! [`evaluate_best`] takes 5..=7 cards and returns the strongest 5-card [`HandValue`].
//! `HandValue: Ord` so two evaluated hands compare correctly (higher = stronger), with
//! full kicker tiebreaks baked in. For 6 or 7 cards we take the max over every 5-card
//! subset — correctness over cleverness.
//!
//! Ranks inside tiebreak vectors are the raw `Rank as u8` values (Two=0 .. Ace=12), in
//! descending order of importance, so lexicographic `Vec` comparison gives the right
//! answer. The wheel (A-2-3-4-5) is handled as a Five-high straight: the Ace counts low
//! and the straight's high card is the Five.

use crate::{Card, Rank};

/// The category of a 5-card poker hand, ordered weakest to strongest.
///
/// `Ord` is derived, so `HighCard < Pair < ... < StraightFlush`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandRank {
    HighCard = 0,
    Pair,
    TwoPair,
    ThreeOfAKind,
    Straight,
    Flush,
    FullHouse,
    FourOfAKind,
    StraightFlush,
}

/// A fully-evaluated 5-card hand: its category plus ordered tiebreak data.
///
/// Comparison is `(rank, tiebreak)` lexicographically. `tiebreak` holds the relevant
/// rank values (as `u8`, Ace=12) in descending priority so equal-category hands break
/// correctly (e.g. for a full house: `[trip_rank, pair_rank]`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandValue {
    pub rank: HandRank,
    pub tiebreak: Vec<u8>,
}

impl HandValue {
    fn new(rank: HandRank, tiebreak: Vec<u8>) -> Self {
        HandValue { rank, tiebreak }
    }
}

/// Evaluate the best 5-card hand from 5..=7 cards.
///
/// Returns `None` if fewer than 5 cards are supplied. Duplicate cards are not rejected
/// here (the deal layer guarantees uniqueness); evaluation is purely combinatorial.
pub fn evaluate_best(cards: &[Card]) -> Option<HandValue> {
    let n = cards.len();
    if n < 5 {
        return None;
    }
    let mut best: Option<HandValue> = None;
    // Iterate over all 5-card combinations via nested index loops.
    for combo in combinations5(n) {
        let five = [
            cards[combo[0]],
            cards[combo[1]],
            cards[combo[2]],
            cards[combo[3]],
            cards[combo[4]],
        ];
        let v = evaluate_five(&five);
        match &best {
            Some(b) if *b >= v => {}
            _ => best = Some(v),
        }
    }
    best
}

/// All 5-element index subsets of `0..n` (n in 5..=7). Small and allocation-light.
fn combinations5(n: usize) -> Vec<[usize; 5]> {
    let mut out = Vec::new();
    for a in 0..n {
        for b in (a + 1)..n {
            for c in (b + 1)..n {
                for d in (c + 1)..n {
                    for e in (d + 1)..n {
                        out.push([a, b, c, d, e]);
                    }
                }
            }
        }
    }
    out
}

/// Evaluate exactly 5 cards.
fn evaluate_five(cards: &[Card; 5]) -> HandValue {
    // Rank values descending.
    let mut ranks: Vec<u8> = cards.iter().map(|c| c.rank as u8).collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));

    let is_flush = cards.iter().all(|c| c.suit == cards[0].suit);
    let straight_high = straight_high_card(&ranks);

    // Count occurrences per rank value (0..=12).
    let mut counts = [0u8; 13];
    for &r in &ranks {
        counts[r as usize] += 1;
    }
    // Groups: (count, rank_value), sorted by count desc then rank desc.
    let mut groups: Vec<(u8, u8)> = (0..13u8)
        .filter(|&r| counts[r as usize] > 0)
        .map(|r| (counts[r as usize], r))
        .collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));

    let counts_pattern: Vec<u8> = groups.iter().map(|g| g.0).collect();

    match (is_flush, straight_high) {
        (true, Some(high)) => return HandValue::new(HandRank::StraightFlush, vec![high]),
        _ => {}
    }

    if counts_pattern.first() == Some(&4) {
        let quad = groups[0].1;
        let kicker = groups[1].1;
        return HandValue::new(HandRank::FourOfAKind, vec![quad, kicker]);
    }
    if counts_pattern.first() == Some(&3) && counts_pattern.get(1) == Some(&2) {
        return HandValue::new(HandRank::FullHouse, vec![groups[0].1, groups[1].1]);
    }
    if is_flush {
        return HandValue::new(HandRank::Flush, ranks);
    }
    if let Some(high) = straight_high {
        return HandValue::new(HandRank::Straight, vec![high]);
    }
    if counts_pattern.first() == Some(&3) {
        let trip = groups[0].1;
        let kickers: Vec<u8> = groups[1..].iter().map(|g| g.1).collect();
        let mut tb = vec![trip];
        tb.extend(kickers);
        return HandValue::new(HandRank::ThreeOfAKind, tb);
    }
    if counts_pattern.first() == Some(&2) && counts_pattern.get(1) == Some(&2) {
        let hi_pair = groups[0].1;
        let lo_pair = groups[1].1;
        let kicker = groups[2].1;
        return HandValue::new(HandRank::TwoPair, vec![hi_pair, lo_pair, kicker]);
    }
    if counts_pattern.first() == Some(&2) {
        let pair = groups[0].1;
        let kickers: Vec<u8> = groups[1..].iter().map(|g| g.1).collect();
        let mut tb = vec![pair];
        tb.extend(kickers);
        return HandValue::new(HandRank::Pair, tb);
    }
    HandValue::new(HandRank::HighCard, ranks)
}

/// If the 5 descending rank values form a straight, return its high-card rank value.
/// Handles the wheel A-2-3-4-5 as a Five-high straight (returns `Rank::Five as u8`).
fn straight_high_card(ranks_desc: &[u8]) -> Option<u8> {
    // Need 5 distinct ranks.
    let mut distinct = ranks_desc.to_vec();
    distinct.dedup();
    if distinct.len() != 5 {
        return None;
    }
    // Normal straight: consecutive descending.
    if distinct.windows(2).all(|w| w[0] == w[1] + 1) {
        return Some(distinct[0]);
    }
    // Wheel: A,5,4,3,2 -> ranks 12,3,2,1,0.
    if distinct == [Rank::Ace as u8, Rank::Five as u8, Rank::Four as u8, Rank::Three as u8, Rank::Two as u8]
    {
        return Some(Rank::Five as u8);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Suit;

    fn card(r: Rank, s: Suit) -> Card {
        Card { rank: r, suit: s }
    }

    /// Parse like "Ah Kd Qc Js Th" using rank+suit chars.
    fn hand(spec: &str) -> Vec<Card> {
        spec.split_whitespace()
            .map(|t| {
                let bytes = t.as_bytes();
                let r = match bytes[0] {
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
                    _ => panic!("bad rank"),
                };
                let s = match bytes[1] {
                    b'c' => Suit::Clubs,
                    b'd' => Suit::Diamonds,
                    b'h' => Suit::Hearts,
                    b's' => Suit::Spades,
                    _ => panic!("bad suit"),
                };
                card(r, s)
            })
            .collect()
    }

    fn eval(spec: &str) -> HandValue {
        evaluate_best(&hand(spec)).unwrap()
    }

    #[test]
    fn rank_ordering() {
        assert!(HandRank::HighCard < HandRank::Pair);
        assert!(HandRank::Straight < HandRank::Flush);
        assert!(HandRank::FullHouse < HandRank::FourOfAKind);
        assert!(HandRank::FourOfAKind < HandRank::StraightFlush);
    }

    #[test]
    fn detects_each_category() {
        assert_eq!(eval("Ah Kh Qh Jh Th").rank, HandRank::StraightFlush);
        assert_eq!(eval("Ah As Ad Ac Kh").rank, HandRank::FourOfAKind);
        assert_eq!(eval("Ah As Ad Kc Kh").rank, HandRank::FullHouse);
        assert_eq!(eval("Ah 9h 7h 5h 2h").rank, HandRank::Flush);
        assert_eq!(eval("9h 8s 7d 6c 5h").rank, HandRank::Straight);
        assert_eq!(eval("Ah As Ad Qc Kh").rank, HandRank::ThreeOfAKind);
        assert_eq!(eval("Ah As Kd Kc Qh").rank, HandRank::TwoPair);
        assert_eq!(eval("Ah As Kd Qc Jh").rank, HandRank::Pair);
        assert_eq!(eval("Ah Kd Qc Js 9h").rank, HandRank::HighCard);
    }

    #[test]
    fn wheel_is_five_high_straight() {
        let v = eval("Ah 2s 3d 4c 5h");
        assert_eq!(v.rank, HandRank::Straight);
        assert_eq!(v.tiebreak, vec![Rank::Five as u8]);
        // Wheel loses to six-high straight.
        let six = eval("2h 3s 4d 5c 6h");
        assert!(six > v);
    }

    #[test]
    fn wheel_straight_flush() {
        let v = eval("Ah 2h 3h 4h 5h");
        assert_eq!(v.rank, HandRank::StraightFlush);
        assert_eq!(v.tiebreak, vec![Rank::Five as u8]);
    }

    #[test]
    fn ace_high_straight_beats_king_high() {
        assert!(eval("Ah Ks Qd Jc Th") > eval("Kh Qs Jd Tc 9h"));
    }

    #[test]
    fn kicker_breaks_pairs() {
        // Pair of aces, K kicker vs pair of aces, Q kicker.
        assert!(eval("Ah As Kd 5c 2h") > eval("Ah As Qd 5c 2h"));
    }

    #[test]
    fn two_pair_tiebreaks() {
        // Aces and twos vs kings and queens: aces-up wins.
        assert!(eval("Ah As 2d 2c 5h") > eval("Kh Ks Qd Qc Ah"));
        // Same two pair, kicker decides.
        assert!(eval("Ah As Kd Kc Qh") > eval("Ah As Kd Kc Jh"));
    }

    #[test]
    fn full_house_compares_on_trips_first() {
        // 999-22 vs 888-AA: nines full beats eights full.
        assert!(eval("9h 9s 9d 2c 2h") > eval("8h 8s 8d Ac Ah"));
    }

    #[test]
    fn flush_compares_high_cards() {
        assert!(eval("Ah Qh 9h 5h 2h") > eval("Kh Qh 9h 5h 2h"));
    }

    #[test]
    fn seven_card_best_five() {
        // Board + hole giving a flush among 7.
        let v = evaluate_best(&hand("Ah Kh Qh 2h 3h 9s 4d")).unwrap();
        assert_eq!(v.rank, HandRank::Flush);
        // 7-card straight where best 5 is the straight, not a lesser pair.
        let s = evaluate_best(&hand("9h 8s 7d 6c 5h 5d 2c")).unwrap();
        assert_eq!(s.rank, HandRank::Straight);
    }

    #[test]
    fn six_card_input_supported() {
        let v = evaluate_best(&hand("Ah As Ad Ac Kh Kd")).unwrap();
        assert_eq!(v.rank, HandRank::FourOfAKind);
    }

    #[test]
    fn too_few_cards_is_none() {
        assert!(evaluate_best(&hand("Ah Ks Qd Jc")).is_none());
    }

    #[test]
    fn equal_hands_compare_equal() {
        assert_eq!(eval("Ah Kh Qh Jh Th"), eval("As Ks Qs Js Ts"));
    }
}
