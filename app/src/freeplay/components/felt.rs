//! The oval felt area of a tile: the radial-gradient backing region, the oval table surface (faint
//! accent rim), the centred community board + `POT <amount>` (or `PRE-FLOP` / `· · ·`), and the
//! opponent seats positioned around the rim. Seats 1..N are placed elliptically and delegated to
//! [`super::seat`]; seat 0 ("You") is rendered in the bottom strip, not on the felt.
//!
//! egui has no radial/linear gradient, so the README's `radial-gradient(120% 130% at 50% 32%, …)`
//! backdrop and the oval's `linear-gradient(180deg,#141a1e,#0e1216)` surface are approximated with a
//! couple of layered flat ellipse/rect fills (a darker base + a lighter inner pass toward the 50%/32%
//! highlight, and a darker lower ellipse over the oval to fake the top→bottom falloff).

use eframe::egui::{self, pos2, Align2, Color32, Pos2, Rect, Stroke, Ui, Vec2};

use crate::freeplay::cards;
use crate::freeplay::components::seat::{self, SeatStyle};
use crate::freeplay::model::Table;
use crate::freeplay::theme::{self, Palette, Weight};

/// Felt oval size as a fraction of the felt area, per the README (`width:90%; height:84%`).
const OVAL_W_FRAC: f32 = 0.90;
const OVAL_H_FRAC: f32 = 0.84;

/// Seat-ring geometry from the prototype's `seatPos(i,N)`:
/// `th = π/2 + i·2π/N`, position `(cx + a·cosθ, cy + b·sinθ)` then divided into percentages by the
/// prototype's `4.6` (x) and `2.5` (y) screen-fit divisors. Pre-dividing by 100 leaves a 0..1
/// fraction of the oval box. Seat 0 lands at the bottom centre and is drawn in the strip, not here.
const RING_A: f32 = 200.0;
const RING_B: f32 = 104.0;
const RING_CX: f32 = 230.0;
const RING_CY: f32 = 125.0;
const RING_DIV_X: f32 = 4.6;
const RING_DIV_Y: f32 = 2.5;

/// Paint the felt into `rect` for `table`. `glow_pulse` is the eased opacity for any to-act seat
/// glow (forwarded to [`super::seat::render`]).
pub fn render(ui: &mut Ui, rect: Rect, table: &Table, glow_pulse: f32) {
    paint_backdrop(ui, rect);

    // The oval surface, centred in the felt area at 90% × 84%.
    let oval = Rect::from_center_size(
        rect.center(),
        Vec2::new(rect.width() * OVAL_W_FRAC, rect.height() * OVAL_H_FRAC),
    );
    paint_oval(ui, oval);

    paint_center(ui, oval, table);
    paint_seats(ui, oval, table, glow_pulse);
}

/// Approximate the felt-area radial gradient `radial-gradient(120% 130% at 50% 32%, #11161a 0%,
/// #0a0c0f 75%)`: fill the whole area with the dark outer stop, then lay a lighter ellipse centred
/// near the 50%/32% highlight so the middle reads brighter than the rim.
fn paint_backdrop(ui: &Ui, rect: Rect) {
    // Clip everything to the felt area: the highlight ellipse below is larger than `rect`, and without
    // a clip it bleeds up over the tile header (and even the toolbar) as a stray glow.
    let painter = ui.painter().with_clip_rect(rect);
    // Outer (75%) stop `#0a0c0f`; the felt area has square corners (the tile clips the rounding).
    let outer = Color32::from_rgb(0x0a, 0x0c, 0x0f);
    painter.rect_filled(rect, egui::CornerRadius::ZERO, outer);
    // Inner highlight at (50%, 32%), brightening toward `FELT_AREA` (#11161a). A single soft ellipse
    // is the closest flat stand-in for the radial centre.
    let hl_center = pos2(rect.center().x, rect.min.y + rect.height() * 0.32);
    painter.add(egui::Shape::ellipse_filled(
        hl_center,
        Vec2::new(rect.width() * 0.6, rect.height() * 0.65),
        Palette::FELT_AREA,
    ));
}

/// Paint the oval table surface: the `#141a1e→#0e1216` vertical gradient (approximated by a base
/// `FELT_TOP` ellipse with a darker `FELT_BOTTOM` ellipse nudged downward) plus the faint accent rim.
fn paint_oval(ui: &Ui, oval: Rect) {
    let painter = ui.painter();
    let radius = oval.size() / 2.0;
    let center = oval.center();

    // Base (top-stop) fill.
    painter.add(egui::Shape::ellipse_filled(center, radius, Palette::FELT_TOP));
    // Lower half darkened toward the bottom stop: a slightly smaller ellipse shifted down, faking the
    // linear top→bottom gradient that egui can't express directly.
    let lower_center = pos2(center.x, center.y + radius.y * 0.18);
    painter.add(egui::Shape::ellipse_filled(
        lower_center,
        Vec2::new(radius.x * 0.96, radius.y * 0.82),
        Palette::FELT_BOTTOM,
    ));
    // Faint accent rim (`rgba(47,214,160,0.18)`).
    painter.add(egui::Shape::ellipse_stroke(
        center,
        radius,
        Stroke::new(1.0, Palette::ACCENT_BORDER_FAINT),
    ));
}

/// Paint the centred community board (or empty-board label) and the `POT <amount>` line.
fn paint_center(ui: &Ui, oval: Rect, table: &Table) {
    let center = oval.center();
    let board = cards::parse(&table.board);

    // The board sits just above centre; the POT line sits just below, matching the prototype's
    // column with an 8px gap.
    if board.is_empty() {
        // `PRE-FLOP` (no pot yet) or `· · ·` (pot building but no board) per `boardEmptyLabel`.
        let label = if table.pot > 0 { "\u{b7} \u{b7} \u{b7}" } else { "PRE-FLOP" };
        ui.painter().text(
            pos2(center.x, center.y - 10.0),
            Align2::CENTER_CENTER,
            label,
            theme::mono_font(10.0, Weight::Medium),
            Palette::TEXT_MUTED_DIM,
        );
    } else {
        // Board cards (small) centred above the pot line.
        cards::paint_board_row(ui.painter(), &board, pos2(center.x, center.y - 14.0));
    }

    // POT row: muted `POT` label + chip amount (mono), centred under the board.
    paint_pot(ui, pos2(center.x, center.y + 16.0), table.pot);
}

/// Paint the `POT <amount>` row centred on `center`. Amount is in chips with a thousands separator;
/// `0` renders as `—` (the README's no-pot marker).
fn paint_pot(ui: &Ui, center: Pos2, pot: u64) {
    let painter = ui.painter();
    let label_font = theme::ui_font(9.0, Weight::Regular);
    let amount = if pot > 0 { group_thousands(pot) } else { "\u{2014}".to_string() };
    let amount_font = theme::mono_font(13.0, Weight::SemiBold);

    // Lay the `POT` label and the amount side by side with a 6px gap, centred as a unit.
    let label_w = painter
        .layout_no_wrap("POT".to_string(), label_font.clone(), Palette::TEXT_MUTED_DIM)
        .size()
        .x;
    let amount_w = painter
        .layout_no_wrap(amount.clone(), amount_font.clone(), Palette::TEXT_PRIMARY)
        .size()
        .x;
    let gap = 6.0;
    let total = label_w + gap + amount_w;
    let mut x = center.x - total / 2.0;
    painter.text(pos2(x, center.y), Align2::LEFT_CENTER, "POT", label_font, Palette::TEXT_MUTED_DIM);
    x += label_w + gap;
    painter.text(pos2(x, center.y), Align2::LEFT_CENTER, amount, amount_font, Palette::TEXT_PRIMARY);
}

/// Place opponent seats (indices 1..N) around the oval rim and delegate each to [`seat::render`].
/// Seat 0 ("You") is intentionally skipped — it lives in the tile's bottom strip.
fn paint_seats(ui: &mut Ui, oval: Rect, table: &Table, glow_pulse: f32) {
    let n = table.seats.len();
    for i in 1..n {
        let seat_data = &table.seats[i];
        let center = seat_pos(oval, i, n);
        let acting = !table.your_turn && i == table.act && !seat_data.is_folded();
        let style = SeatStyle {
            acting,
            folded: seat_data.is_folded(),
            dealer: i == table.dealer,
            index: i,
        };
        seat::render(ui, center, seat_data, style, glow_pulse);
    }
}

/// The centre point of seat `i` of `n` on the oval rim (prototype `seatPos`). Returns a screen `Pos2`
/// inside `oval`.
fn seat_pos(oval: Rect, i: usize, n: usize) -> Pos2 {
    let th = std::f32::consts::FRAC_PI_2 + (i as f32) * 2.0 * std::f32::consts::PI / (n as f32);
    // Prototype percentages (0..100) → 0..1 fraction of the oval box.
    let fx = (RING_CX + RING_A * th.cos()) / RING_DIV_X / 100.0;
    let fy = (RING_CY + RING_B * th.sin()) / RING_DIV_Y / 100.0;
    pos2(oval.min.x + fx * oval.width(), oval.min.y + fy * oval.height())
}

/// Format a chip count with comma thousands separators (`1700` → `"1,700"`), matching the prototype's
/// `Number(n).toLocaleString()`.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = bytes.len();
    for (idx, b) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
