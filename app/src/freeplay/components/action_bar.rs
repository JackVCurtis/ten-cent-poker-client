//! The tile bottom strip: your hole cards (larger) + `You` + your chip stack on the left. When it is
//! your turn: a thin timer bar across the top (accent → amber → red as it depletes) and the action
//! buttons `Fold` / `Check`-or-`Call <amount>` / `Raise`. When it is not your turn: a muted
//! `Waiting on <name>` line with a pulsing dot.
//!
//! Painted by hand (not in-flow widgets) so the strip can sit at an explicit `rect` inside the tile,
//! mirroring the prototype's `padding:11px 14px` row. The buttons are square-cornered surface pills
//! laid out right-aligned; the returned `Act` lets the tile apply the state transition.

use eframe::egui::{pos2, Align2, Color32, Rect, Sense, Stroke, Ui, Vec2};

use crate::freeplay::cards;
use crate::freeplay::model::{Act, Table};
use crate::freeplay::theme::{self, rad, Palette, Weight};

/// Timer-bar track height (px) — the prototype's `height:3px` rail across the top of the strip.
const TIMER_H: f32 = 3.0;
/// Horizontal / vertical inner padding of the content row (prototype `padding:11px 14px`).
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 11.0;
/// Gap between the action buttons (prototype `gap:6px`).
const BTN_GAP: f32 = 6.0;
/// Gap between the hero cards and the `You`/stack column (prototype `gap:9px`).
const HERO_GAP: f32 = 9.0;

/// Render the bottom strip into `rect` for `table`. `pulse` is the eased opacity for the
/// waiting/pulsing dot. Returns `Some(act)` when the player clicked an action button this frame.
pub fn render(ui: &mut Ui, rect: Rect, table: &Table, pulse: f32) -> Option<Act> {
    // Strip backing (`#0c0e11`) with the top hairline (`rgba(255,255,255,0.05)`) the prototype draws
    // as a `border-top` on the strip container.
    ui.painter().rect_filled(rect, 0.0, Palette::STRIP_BG);
    ui.painter().hline(
        rect.x_range(),
        rect.min.y,
        theme::hairline(Palette::BORDER_05),
    );

    // The your-turn timer rail consumes the top `TIMER_H` px; the content row fills the rest.
    let content_top = if table.your_turn {
        paint_timer(ui, rect, table);
        rect.min.y + TIMER_H
    } else {
        rect.min.y
    };
    let content = Rect::from_min_max(pos2(rect.min.x + PAD_X, content_top + PAD_Y), pos2(rect.max.x - PAD_X, rect.max.y - PAD_Y));

    paint_hero(ui, content, table);

    if table.your_turn {
        paint_actions(ui, content, table)
    } else {
        paint_waiting(ui, content, table, pulse);
        None
    }
}

/// Paint the depleting timer rail at the top of `rect`: a faint full-width track with a fill whose
/// width tracks `time_left/18000` and whose color steps accent (>40%) → amber (18–40%) → red (<18%),
/// matching the prototype's `pct>40?accent:pct>18?amber:red`.
fn paint_timer(ui: &Ui, rect: Rect, table: &Table) {
    let painter = ui.painter();
    let track = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TIMER_H));
    painter.rect_filled(track, 0.0, Palette::BORDER_05);

    let frac = table.timer_frac();
    let color = if frac > 0.40 {
        Palette::ACCENT
    } else if frac > 0.18 {
        Palette::TIMER_AMBER
    } else {
        Palette::TIMER_RED
    };
    if frac > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, TIMER_H));
        painter.rect_filled(fill, 0.0, color);
    }
}

/// Paint the left cluster: the larger hero hole cards followed by the `You` label over the hero
/// stack (mono, desaturated accent), vertically centred in the content row.
fn paint_hero(ui: &Ui, content: Rect, table: &Table) {
    let hero = cards::parse(&table.hero);
    let cy = content.center().y;
    let mut x = content.min.x;

    // Hero cards (big), laid out left-to-right with the prototype's 4px gap.
    if !hero.is_empty() {
        let size = cards::card_size(true);
        let gap = cards::card_gap(true);
        let top = cy - size.y / 2.0;
        for c in &hero {
            let card_rect = Rect::from_min_size(pos2(x, top), size);
            cards::draw_card(ui.painter(), c, card_rect, true);
            x += size.x + gap;
        }
        x += HERO_GAP - gap;
    }

    // `You` (accent text, 11px/600) stacked over the stack count (mono 10px/500, desaturated accent).
    let stack = group_thousands(table.hero_stack());
    ui.painter().text(
        pos2(x, cy - 6.0),
        Align2::LEFT_CENTER,
        "You",
        theme::ui_font(11.0, Weight::SemiBold),
        Palette::ACCENT_TEXT,
    );
    ui.painter().text(
        pos2(x, cy + 7.0),
        Align2::LEFT_CENTER,
        stack,
        theme::mono_font(10.0, Weight::Medium),
        Palette::ACCENT_STACK,
    );
}

/// Paint the right-aligned action buttons (`Fold` / `Check`-or-`Call <amount>` / `Raise`) and return
/// the chosen `Act`, if any. Buttons are measured from their labels then placed right-to-left so the
/// row hugs the strip's right edge like the prototype's `justify-content:flex-end`.
fn paint_actions(ui: &mut Ui, content: Rect, table: &Table) -> Option<Act> {
    let call_label = if table.to_call > 0 {
        format!("Call {}", group_thousands(table.to_call))
    } else {
        "Check".to_string()
    };

    // (label, text color, accent-filled) for each button, in left-to-right visual order.
    let specs: [(String, Color32, bool); 3] = [
        ("Fold".to_string(), Palette::FOLD_TEXT, false),
        (call_label, Color32::from_rgb(0xd4, 0xd8, 0xde), false),
        ("Raise".to_string(), Palette::ON_ACCENT, true),
    ];
    let acts = [Act::Fold, Act::Call, Act::Raise];

    // Measure each button (label width + the prototype's horizontal padding) and total the row so it
    // can be right-aligned. Raise uses a hair more side padding (`8px 14px` vs `8px 12px`).
    let font = theme::ui_font(12.0, Weight::SemiBold);
    let h = 30.0;
    let widths: Vec<f32> = specs
        .iter()
        .map(|(label, _, accent)| {
            let lw = ui
                .painter()
                .layout_no_wrap(label.clone(), font.clone(), Palette::TEXT_PRIMARY)
                .size()
                .x;
            let pad = if *accent { 14.0 } else { 12.0 };
            lw + pad * 2.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + BTN_GAP * (specs.len() as f32 - 1.0);

    let mut x = content.max.x - total;
    let top = content.center().y - h / 2.0;
    let mut chosen = None;
    for (i, ((label, text, accent), w)) in specs.iter().zip(widths.iter()).enumerate() {
        let btn = Rect::from_min_size(pos2(x, top), Vec2::new(*w, h));
        let resp = ui.interact(btn, ui.id().with(("act_btn", table.id, i)), Sense::click());

        if *accent {
            // Raise — accent (emerald) fill, near-black label; brightens slightly on hover.
            let fill = if resp.hovered() {
                Color32::from_rgb(0x3a, 0xe0, 0xac)
            } else {
                Palette::ACCENT
            };
            theme::fill_rect(ui, btn, rad::INPUT, fill, Stroke::NONE);
        } else {
            // Fold / Check-Call — neutral surface (`#1a1c21`) + hairline; lifts to surface on hover.
            let fill = if resp.hovered() { Palette::SURFACE } else { Palette::NEUTRAL_BTN };
            theme::fill_rect(ui, btn, rad::INPUT, fill, theme::hairline(Palette::BORDER_07));
        }
        ui.painter()
            .text(btn.center(), Align2::CENTER_CENTER, label, font.clone(), *text);

        if resp.clicked() {
            chosen = Some(acts[i]);
        }
        x += w + BTN_GAP;
    }
    chosen
}

/// Paint the not-your-turn line: a muted pulsing dot (`#5b606b`) followed by the `Waiting on <name>`
/// copy (or `Waiting for players` when no one is seated to act), right-aligned in the content row.
fn paint_waiting(ui: &mut Ui, content: Rect, table: &Table, pulse: f32) {
    let label = match table.acting_name() {
        Some(name) => format!("Waiting on {name}"),
        None => "Waiting for players".to_string(),
    };
    let font = theme::ui_font(11.0, Weight::Regular);
    let cy = content.center().y;

    let dot_d = 6.0;
    let gap = 8.0;
    let text_w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), Palette::TEXT_MUTED)
        .size()
        .x;
    let total = dot_d + gap + text_w;
    let mut x = content.max.x - total;

    // Pulsing status dot — same alpha easing as `widgets::status_dot`, applied here so the dot can be
    // hand-placed at the start of the right-aligned waiting line. `pulse` comes from the shared
    // animation clock (see `widgets::pulse_opacity`).
    let dot_c = Palette::TEXT_MUTED_DIM;
    let a = (255.0 * pulse.clamp(0.0, 1.0)) as u8;
    theme::dot(
        ui,
        pos2(x + dot_d / 2.0, cy),
        dot_d,
        Color32::from_rgba_premultiplied(dot_c.r(), dot_c.g(), dot_c.b(), a),
    );
    x += dot_d + gap;
    ui.painter()
        .text(pos2(x, cy), Align2::LEFT_CENTER, label, font, Palette::TEXT_MUTED);
}

/// Format a chip count with comma thousands separators (`1700` → `"1,700"`), matching the prototype's
/// free-play `Number(n).toLocaleString()`. (Local copy — the felt module's grouping helper is private.)
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, b) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
