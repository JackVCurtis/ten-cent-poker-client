//! One seat rendered on the felt rim. A filled seat is a pill (avatar circle with initial, name,
//! chip stack) optionally with a small white `D` dealer chip; the player to act (when it is NOT your
//! turn) gets an accent border + soft glow; folded players are dimmed; an empty seat renders as a
//! dashed `Open` pill. Seats are absolutely positioned by [`super::felt`], which passes the centre
//! point; this fn paints the pill centred on that point.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2, Ui,
};

use crate::freeplay::cards;
use crate::freeplay::model::Seat;
use crate::freeplay::theme::{self, rad, Palette, Weight};

/// Per-seat presentation flags resolved by the felt before placing each seat.
#[derive(Clone, Copy, Default)]
pub struct SeatStyle {
    /// This seat is the one to act (accent rim + glow). Mutually exclusive with `your_turn` tiles.
    pub acting: bool,
    /// This seat has folded (render dimmed).
    pub folded: bool,
    /// This seat holds the dealer button.
    pub dealer: bool,
    /// Seat index, used to pick the avatar palette color.
    pub index: usize,
}

// Pill metrics from the prototype: `padding:5px 8px;gap:6px` with an 18px avatar; the name/stack
// stack vertically with no extra gap. `border-radius:9px` matches the input/chip token.
const AVATAR_D: f32 = 18.0;
const DEALER_D: f32 = 14.0;
const PAD: Vec2 = Vec2::new(8.0, 5.0);
const GAP: f32 = 6.0;
/// Name column ellipsis cap (prototype `max-width:64px`).
const NAME_MAX_W: f32 = 64.0;

/// Paint one seat pill centred on `center`. `glow_pulse` is the eased opacity (0..=1) for the
/// to-act glow ring (pass `1.0` at rest / for non-acting seats).
pub fn render(ui: &mut Ui, center: Pos2, seat: &Seat, style: SeatStyle, glow_pulse: f32) {
    match seat {
        Seat::Empty => render_open(ui, center),
        Seat::Filled { name, stack, folded } => {
            // The model carries `folded`; the felt may also pass it via `style.folded`. Treat either
            // as folded so dimming is robust regardless of which path the caller resolved.
            let folded = *folded || style.folded;
            render_filled(ui, center, name, *stack, &style, folded, glow_pulse);
        }
    }
}

/// A dashed "Open" pill: dashed hairline border + muted `Open` label, centred on `center`.
fn render_open(ui: &mut Ui, center: Pos2) {
    let font = theme::ui_font(10.0, Weight::Medium);
    let galley = ui.painter().layout_no_wrap("Open".to_string(), font.clone(), Palette::TEXT_MUTED_DIM);
    // Prototype open-seat padding is `5px 9px`.
    let pad = Vec2::new(9.0, 5.0);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_center_size(center, size);
    if !ui.is_rect_visible(rect) {
        return;
    }
    // A barely-there fill (`rgba(255,255,255,0.01)`) over the felt, then the dashed border.
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(rad::INPUT),
        Color32::from_rgba_premultiplied(3, 3, 3, 3),
    );
    paint_dashed_border(ui, rect, rad::INPUT as f32, Palette::BORDER_DASH);
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, "Open", font, Palette::TEXT_MUTED_DIM);
}

/// A seated-player pill: avatar circle + name/stack column + optional dealer chip.
fn render_filled(
    ui: &mut Ui,
    center: Pos2,
    name: &str,
    stack: u64,
    style: &SeatStyle,
    folded: bool,
    glow_pulse: f32,
) {
    let acting = style.acting && !folded;

    // --- measure the pill so it can be centred on `center` ---
    let name_font = theme::ui_font(10.0, Weight::SemiBold);
    let stack_font = theme::mono_font(9.0, Weight::Medium);
    let stack_str = format_chips(stack);

    // Name column width is the wider of name/stack, capped at NAME_MAX_W (name ellipsizes).
    let name_galley = ui.painter().layout_no_wrap(name.to_string(), name_font.clone(), Palette::TEXT_PRIMARY_DIM);
    let stack_galley = ui.painter().layout_no_wrap(stack_str.clone(), stack_font.clone(), Palette::ACCENT_STACK);
    let text_col_w = name_galley.size().x.min(NAME_MAX_W).max(stack_galley.size().x.min(NAME_MAX_W));
    let text_col_h = name_galley.size().y + stack_galley.size().y;

    let mut content_w = AVATAR_D + GAP + text_col_w;
    if style.dealer {
        content_w += GAP + DEALER_D;
    }
    let content_h = AVATAR_D.max(text_col_h).max(if style.dealer { DEALER_D } else { 0.0 });
    let size = Vec2::new(content_w, content_h) + PAD * 2.0;
    let rect = Rect::from_center_size(center, size);
    if !ui.is_rect_visible(rect) {
        return;
    }

    // Folded seats render at 0.4 opacity (prototype `opacity:0.4`). We fold that factor into every
    // colour rather than compositing a layer, which egui's immediate painter cannot do cheaply.
    let alpha = if folded { 0.4 } else { 1.0 };

    // --- to-act glow + rim (drawn under the pill fill) ---
    if acting {
        // Soft outer glow approximating `box-shadow:0 0 16px -3px rgba(47,214,160,0.4)`. egui can't
        // blur, so layer a few expanding accent strokes; `glow_pulse` eases the whole ring.
        let glow_a = (110.0 * glow_pulse.clamp(0.0, 1.0)) as u8;
        let glow = Color32::from_rgba_premultiplied(
            Palette::ACCENT.r(),
            Palette::ACCENT.g(),
            Palette::ACCENT.b(),
            glow_a,
        );
        theme::soft_ring(ui, rect, rad::INPUT, glow, 4);
    }

    // --- pill frame: seat-pill surface + hairline (or accent rim when acting) ---
    let fill = dim(Palette::SEAT_PILL, alpha);
    let stroke = if acting {
        // Prototype `border:1px solid rgba(47,214,160,0.55)`.
        Stroke::new(1.0, Color32::from_rgba_premultiplied(0x1a, 0x76, 0x59, 140))
    } else {
        theme::hairline(dim(Palette::BORDER_07, alpha))
    };
    ui.painter()
        .rect(rect, CornerRadius::same(rad::INPUT), fill, stroke, StrokeKind::Inside);

    // --- inner content laid out left→right ---
    let inner_left = rect.min.x + PAD.x;
    let cy = rect.center().y;

    // Avatar circle: accent fill when acting (near-black initial), else the per-seat palette colour.
    let avatar_center = Pos2::new(inner_left + AVATAR_D / 2.0, cy);
    let (avatar_fill, initial_color) = if acting {
        (Palette::ACCENT, Palette::ON_ACCENT)
    } else {
        (cards::avatar_color(style.index), Color32::from_rgb(0xcd, 0xd2, 0xda))
    };
    ui.painter().circle_filled(avatar_center, AVATAR_D / 2.0, dim(avatar_fill, alpha));
    let initial = name.chars().next().unwrap_or('?').to_ascii_uppercase();
    ui.painter().text(
        avatar_center,
        Align2::CENTER_CENTER,
        initial,
        theme::ui_font(9.0, Weight::Bold),
        dim(initial_color, alpha),
    );

    // Name + stack column, vertically centred against the avatar.
    let text_left = inner_left + AVATAR_D + GAP;
    let col_top = cy - text_col_h / 2.0;
    paint_clipped(
        ui,
        Pos2::new(text_left, col_top),
        text_col_w,
        name,
        name_font,
        dim(Palette::TEXT_PRIMARY_DIM, alpha),
    );
    ui.painter().text(
        Pos2::new(text_left, col_top + name_galley.size().y),
        Align2::LEFT_TOP,
        stack_str,
        stack_font,
        dim(Palette::ACCENT_STACK, alpha),
    );

    // Dealer chip: a small white `D` disc at the far right.
    if style.dealer {
        let chip_center = Pos2::new(rect.max.x - PAD.x - DEALER_D / 2.0, cy);
        ui.painter().circle_filled(chip_center, DEALER_D / 2.0, dim(Palette::TEXT_PRIMARY_DIM, alpha));
        ui.painter().text(
            chip_center,
            Align2::CENTER_CENTER,
            "D",
            theme::ui_font(8.0, Weight::Bold),
            dim(Palette::APP_BG_GRID, alpha),
        );
    }
}

/// Paint `text` at `top_left` clipped to `max_w` so an over-long name is cut off (egui has no
/// built-in ellipsis on a bare painter call; a hard clip is the lightest faithful stand-in).
fn paint_clipped(ui: &Ui, top_left: Pos2, max_w: f32, text: &str, font: FontId, color: Color32) {
    let clip = Rect::from_min_size(top_left, Vec2::new(max_w, font.size + 4.0));
    let p = ui.painter().with_clip_rect(clip);
    p.text(top_left, Align2::LEFT_TOP, text, font, color);
}

/// Format a chip count with thousands separators (`8560` -> `8,560`) to match the prototype's
/// `toLocaleString` stack readout. Chips only — no currency symbol in free play.
fn format_chips(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Multiply `color`'s alpha by `factor` (used to dim folded seats to 0.4). Premultiplied colours
/// scale all channels by the same factor, preserving the visible hue at lower opacity.
fn dim(color: Color32, factor: f32) -> Color32 {
    if factor >= 1.0 {
        return color;
    }
    let f = factor.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (color.r() as f32 * f) as u8,
        (color.g() as f32 * f) as u8,
        (color.b() as f32 * f) as u8,
        (color.a() as f32 * f) as u8,
    )
}

/// Approximate a dashed rounded-rect border. egui has no native dashed stroke for rounded rects, so
/// we dash the four straight edges (inset by the corner radius) and stroke short solid arcs at the
/// corners — close enough for the small "Open" pill. Documented approximation of the prototype's
/// `border:1px dashed`.
fn paint_dashed_border(ui: &Ui, rect: Rect, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let (dash, gap) = (3.0, 3.0);
    let painter = ui.painter();

    // Straight edges, each inset by `r` so dashes don't overrun the rounded corners.
    let top = [Pos2::new(rect.min.x + r, rect.min.y), Pos2::new(rect.max.x - r, rect.min.y)];
    let bottom = [Pos2::new(rect.min.x + r, rect.max.y), Pos2::new(rect.max.x - r, rect.max.y)];
    let left = [Pos2::new(rect.min.x, rect.min.y + r), Pos2::new(rect.min.x, rect.max.y - r)];
    let right = [Pos2::new(rect.max.x, rect.min.y + r), Pos2::new(rect.max.x, rect.max.y - r)];
    for seg in [top, bottom, left, right] {
        painter.add(egui::Shape::dashed_line(&seg, stroke, dash, gap));
    }

    // Solid quarter-arc corners (small enough that dashing them adds no fidelity).
    let corners = [
        (Pos2::new(rect.min.x + r, rect.min.y + r), std::f32::consts::PI, 1.5 * std::f32::consts::PI),
        (Pos2::new(rect.max.x - r, rect.min.y + r), 1.5 * std::f32::consts::PI, 2.0 * std::f32::consts::PI),
        (Pos2::new(rect.max.x - r, rect.max.y - r), 0.0, 0.5 * std::f32::consts::PI),
        (Pos2::new(rect.min.x + r, rect.max.y - r), 0.5 * std::f32::consts::PI, std::f32::consts::PI),
    ];
    for (c, a0, a1) in corners {
        let n = 4;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * (i as f32 / n as f32);
                Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        painter.add(egui::Shape::line(pts, stroke));
    }
}
