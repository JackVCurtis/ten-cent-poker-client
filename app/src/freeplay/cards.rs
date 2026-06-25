//! Playing-card parsing and rendering. Parses the model's space-separated card-token strings
//! (`"As Kd 7c"`, `T` = ten, suit letter `s/h/d/c`) into renderable cards, and paints a card face
//! (rounded `#ecedf0` rect, rank glyph + Unicode suit, red `#d6443c` for h/d, near-black `#15171b`
//! for s/c).
//!
//! Both the community board and the hero hole cards use these; hero cards render larger (`big`).
//! A face-down card paints a muted patterned back instead of a rank/suit.

use eframe::egui::{self, Color32, Rect, Sense, Ui, Vec2};

use crate::freeplay::theme::{self, rad, Palette};

/// A parsed card: display rank (`"10"` for ten), suit glyph, and whether it is a red suit.
#[derive(Clone)]
pub struct Card {
    /// Display rank string (`A K Q J 10 9 …`).
    pub rank: String,
    /// Unicode suit glyph (`♠ ♥ ♦ ♣`).
    pub suit: char,
    /// True for hearts/diamonds (rendered in [`Palette::SUIT_RED`]).
    pub red: bool,
}

impl Card {
    /// The face color for this card's rank+suit text (red for h/d, near-black for s/c).
    pub fn ink(&self) -> Color32 {
        if self.red {
            Palette::SUIT_RED
        } else {
            Palette::SUIT_BLACK
        }
    }
}

/// Parse a space-separated card string into renderable [`Card`]s. An empty/blank string yields an
/// empty vec (pre-flop). Tokens that are too short to carry a suit letter are skipped.
pub fn parse(s: &str) -> Vec<Card> {
    s.split_whitespace()
        .filter_map(|t| {
            let suit_ch = t.chars().last()?;
            let rank_raw: String = t[..t.len().saturating_sub(1)].to_string();
            if rank_raw.is_empty() {
                return None;
            }
            let (suit, red) = match suit_ch {
                's' => ('\u{2660}', false), // ♠
                'h' => ('\u{2665}', true),  // ♥
                'd' => ('\u{2666}', true),  // ♦
                'c' => ('\u{2663}', false), // ♣
                _ => return None,
            };
            let rank = if rank_raw == "T" { "10".to_string() } else { rank_raw };
            Some(Card { rank, suit, red })
        })
        .collect()
}

/// Card face size (px): board cards `(27, 36)`, hero cards `(30, 40)` per the prototype.
pub fn card_size(big: bool) -> Vec2 {
    if big {
        Vec2::new(30.0, 40.0)
    } else {
        Vec2::new(27.0, 36.0)
    }
}

/// Inter-card gap (px) for a row: hero `4`, board `5` per the prototype.
pub fn card_gap(big: bool) -> f32 {
    if big {
        4.0
    } else {
        5.0
    }
}

/// Paint one card into an explicit `rect`. When `faceup`, draws the rank+suit on the card face;
/// otherwise paints a muted card back. Pure painter call — allocates nothing, so callers can place
/// cards at arbitrary felt coordinates.
pub fn draw_card(painter: &egui::Painter, c: &Card, rect: Rect, faceup: bool) {
    let radius = egui::CornerRadius::same(rad::CARD);
    if faceup {
        painter.rect_filled(rect, radius, Palette::CARD_FACE);
        let big = rect.height() >= 38.0;
        let glyph_px = if big { 14.0 } else { 12.0 };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}{}", c.rank, c.suit),
            egui::FontId::proportional(glyph_px),
            c.ink(),
        );
    } else {
        // Face-down: a dim surface back with a faint hairline, no rank/suit.
        painter.rect(
            rect,
            radius,
            Palette::SEAT_PILL,
            theme::hairline(Palette::BORDER_10),
            egui::StrokeKind::Inside,
        );
    }
}

/// Allocate and paint one face-up card in the current layout flow. Returns the response for the
/// allocated rect. (Thin wrapper over [`draw_card`] for the common in-flow case.)
pub fn card(ui: &mut Ui, c: &Card, big: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(card_size(big), Sense::hover());
    if ui.is_rect_visible(rect) {
        draw_card(ui.painter(), c, rect, true);
    }
    resp
}

/// Paint a horizontal row of cards in the current layout flow with the prototype's small gaps.
/// Used for both the community board (`big = false`) and the larger hero hole cards (`big = true`).
pub fn card_row(ui: &mut Ui, cards: &[Card], big: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(card_gap(big), 0.0);
        for c in cards {
            card(ui, c, big);
        }
    });
}

/// Total footprint (px) of a row of `n` cards laid out by [`card_row`] / [`board_row_rects`], so a
/// caller can center the row inside the felt before painting.
pub fn row_size(n: usize, big: bool) -> Vec2 {
    if n == 0 {
        return Vec2::ZERO;
    }
    let size = card_size(big);
    let w = size.x * n as f32 + card_gap(big) * (n.saturating_sub(1)) as f32;
    Vec2::new(w, size.y)
}

/// Lay out `n` cards as a horizontal row centered on `center`, returning each card's rect (in order).
/// Callers paint with [`draw_card`]; works for the community board and the hero hand alike.
pub fn row_rects(center: egui::Pos2, n: usize, big: bool) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let size = card_size(big);
    let gap = card_gap(big);
    let total = row_size(n, big);
    let mut x = center.x - total.x / 2.0;
    let top = center.y - size.y / 2.0;
    let mut rects = Vec::with_capacity(n);
    for _ in 0..n {
        let rect = Rect::from_min_size(egui::pos2(x, top), size);
        rects.push(rect);
        x += size.x + gap;
    }
    rects
}

/// Convenience: paint a community board row (small cards) centered on `center`.
pub fn paint_board_row(painter: &egui::Painter, cards: &[Card], center: egui::Pos2) {
    for (rect, c) in row_rects(center, cards.len(), false).into_iter().zip(cards) {
        draw_card(painter, c, rect, true);
    }
}

/// Convenience: paint the hero hole cards (larger) centered on `center`.
pub fn paint_hero_row(painter: &egui::Painter, cards: &[Card], center: egui::Pos2) {
    for (rect, c) in row_rects(center, cards.len(), true).into_iter().zip(cards) {
        draw_card(painter, c, rect, true);
    }
}

/// A muted avatar-fill palette (per-seat) used by seats and previews when no accent applies.
pub const AVATAR_PALETTE: [Color32; 6] = [
    Color32::from_rgb(0x2c, 0x4a, 0x5e),
    Color32::from_rgb(0x4a, 0x3c, 0x5e),
    Color32::from_rgb(0x5e, 0x4a, 0x2c),
    Color32::from_rgb(0x2c, 0x5e, 0x44),
    Color32::from_rgb(0x5e, 0x2c, 0x3c),
    Color32::from_rgb(0x3c, 0x4a, 0x5e),
];

/// The avatar fill for seat index `i` (cycles through [`AVATAR_PALETTE`]).
pub fn avatar_color(i: usize) -> Color32 {
    AVATAR_PALETTE[i % AVATAR_PALETTE.len()]
}
