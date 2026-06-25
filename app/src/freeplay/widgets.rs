//! Small shared widgets used across screens and components, all styled via [`theme::Palette`]:
//! segmented-control pills, the seat stepper, badges/chips (game / FREE / count / "need you"),
//! pulsing status dots, and the two CTA buttons (full-width accent + neutral slate). These wrap the
//! [`theme`] painter helpers so the host rail, toolbar, tiles, and modal all share one consistent
//! look.
//!
//! Render fns take `ui: &mut egui::Ui` and return a click [`egui::Response`] (or a small response
//! struct) so the parent can react to clicks/selection.

use eframe::egui::{self, Color32, Rect, Sense, Ui, Vec2};

use crate::freeplay::theme::{self, rad, Palette, Weight};

/// A segmented-control pill: rounded surface (or accent tint when `selected`) with a centered label.
/// Selected = accent tint bg + accent text + accent border; unselected = `#16181e` surface +
/// secondary `#9aa0ab` text. `mono` picks JetBrains-Mono (chip/blind pills) vs the proportional UI
/// font (game pills). Returns the response so the caller can branch on `.clicked()`.
pub fn segmented_pill(ui: &mut Ui, label: &str, selected: bool, mono: bool) -> egui::Response {
    let desired = Vec2::new(ui.available_width().min(160.0).max(48.0), 34.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    paint_segmented(ui, rect, label, selected, mono, resp.hovered());
    resp
}

/// Paint a segmented pill into an explicit `rect` (for callers that lay pills out by hand, e.g. an
/// equal-flex row). `hovered` lifts the unselected border slightly. Returns nothing — the caller
/// already owns the response that produced `rect`.
pub fn paint_segmented(ui: &Ui, rect: Rect, label: &str, selected: bool, mono: bool, hovered: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    theme::pill_frame(ui, rect, rad::INPUT, Palette::SURFACE, selected);
    if hovered && !selected {
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(rad::INPUT),
            theme::hairline(Palette::BORDER_10),
            egui::StrokeKind::Inside,
        );
    }
    let color = if selected { Palette::ACCENT_TEXT } else { Palette::TEXT_SECONDARY };
    let font = if mono {
        theme::mono_font(12.0, Weight::SemiBold)
    } else {
        theme::ui_font(12.0, Weight::SemiBold)
    };
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, color);
}

/// Lay out a row of segmented pills that share the available width equally (the host rail's game /
/// blinds / visibility rows). Returns the index of any pill clicked this frame, else `None`.
/// `selected` highlights the active option.
pub fn segmented_row(ui: &mut Ui, labels: &[&str], selected: usize, mono: bool) -> Option<usize> {
    let gap = 8.0;
    let n = labels.len().max(1);
    let total_w = ui.available_width();
    let pill_w = ((total_w - gap * (n as f32 - 1.0)) / n as f32).max(40.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(gap, 0.0);
        for (i, label) in labels.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(pill_w, 34.0), Sense::click());
            paint_segmented(ui, rect, label, i == selected, mono, resp.hovered());
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Outcome of the seat stepper: which side (if any) the user clicked this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepperClick {
    None,
    Down,
    Up,
}

/// A `−  value  +` stepper row inside a surface pill. The arrows are click targets; the centered
/// value is mono. `min`/`max` only gate the arrows visually (greyed at the bound) — the caller still
/// applies the actual clamp. Returns which arrow was clicked.
pub fn stepper(ui: &mut Ui, value: usize) -> StepperClick {
    // Back-compat entry point with no bounds: arrows always look active.
    stepper_bounded(ui, value, usize::MIN, usize::MAX)
}

/// Bounds-aware stepper: greys the `−`/`+` arrow when `value` is at `min`/`max`. Clicking a greyed
/// arrow still returns its [`StepperClick`]; the caller's clamp makes it a no-op.
pub fn stepper_bounded(ui: &mut Ui, value: usize, min: usize, max: usize) -> StepperClick {
    let h = 34.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(120.0), h), Sense::hover());
    let mut click = StepperClick::None;
    if !ui.is_rect_visible(rect) {
        return click;
    }
    theme::fill_rect(ui, rect, rad::INPUT, Palette::SURFACE, theme::hairline(Palette::BORDER_07));

    // Carve out square arrow hit-zones at each end; the value sits centered between them.
    let arrow_w = h;
    let down_rect = Rect::from_min_size(rect.min, Vec2::new(arrow_w, h));
    let up_rect = Rect::from_min_size(egui::pos2(rect.max.x - arrow_w, rect.min.y), Vec2::new(arrow_w, h));

    let down_enabled = value > min;
    let up_enabled = value < max;

    let down_resp = ui.interact(down_rect, ui.id().with(("stepper_down", value)), Sense::click());
    let up_resp = ui.interact(up_rect, ui.id().with(("stepper_up", value)), Sense::click());
    if down_resp.clicked() {
        click = StepperClick::Down;
    }
    if up_resp.clicked() {
        click = StepperClick::Up;
    }

    let painter = ui.painter();
    let arrow_color = |enabled: bool, hovered: bool| {
        if !enabled {
            Palette::TEXT_MUTED_DIM
        } else if hovered {
            Palette::TEXT_PRIMARY
        } else {
            Palette::TEXT_SECONDARY
        }
    };
    painter.text(
        down_rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{2212}", // − minus sign
        theme::ui_font(16.0, Weight::Medium),
        arrow_color(down_enabled, down_resp.hovered()),
    );
    painter.text(
        up_rect.center(),
        egui::Align2::CENTER_CENTER,
        "+",
        theme::ui_font(16.0, Weight::Medium),
        arrow_color(up_enabled, up_resp.hovered()),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        value.to_string(),
        theme::mono_font(15.0, Weight::SemiBold),
        Palette::TEXT_PRIMARY,
    );
    click
}

// ---------------------------------------------------------------------------
// Badges / chips
// ---------------------------------------------------------------------------

/// A small rounded badge (e.g. `NLH`, `FREE`, `FOCUS`). `fill`/`text`/`border` let callers express
/// the muted FREE badge, the accent FOCUS tag, and the neutral game badge with one primitive.
pub fn badge(ui: &mut Ui, label: &str, fill: Color32, text: Color32, border: Color32, mono: bool) {
    let font = if mono {
        theme::mono_font(9.5, Weight::SemiBold)
    } else {
        theme::ui_font(9.0, Weight::SemiBold)
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), text);
    let pad = Vec2::new(6.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    if ui.is_rect_visible(rect) {
        theme::fill_rect(ui, rect, rad::BADGE, fill, theme::hairline(border));
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, text);
    }
}

/// The game badge (`NLH` / `PLO` / `STUD`): neutral surface + secondary text + hairline, mono.
pub fn game_badge(ui: &mut Ui, label: &str) {
    badge(ui, label, Palette::SURFACE, Palette::TEXT_SECONDARY, Palette::BORDER_07, true);
}

/// The free-table stake badge: a MUTED `FREE` (not accent) — `#6e737d` on `#16181e`. Proportional.
pub fn free_badge(ui: &mut Ui) {
    badge(ui, "FREE", Palette::SURFACE, Palette::TEXT_MUTED, Palette::BORDER_07, false);
}

/// The accent `FOCUS` tag shown on the keyboard-focused tile: accent text on accent tint.
pub fn focus_badge(ui: &mut Ui) {
    badge(ui, "FOCUS", Palette::ACCENT_TINT, Palette::ACCENT_TEXT, Palette::ACCENT_BORDER, false);
}

/// A neutral count chip, e.g. `4 active`: surface fill, secondary text, hairline, mono number. The
/// whole label is passed in so callers control pluralization.
pub fn count_chip(ui: &mut Ui, label: &str) {
    let font = theme::mono_font(11.0, Weight::Medium);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Palette::TEXT_SECONDARY);
    let pad = Vec2::new(9.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    if ui.is_rect_visible(rect) {
        theme::fill_rect(ui, rect, rad::INPUT, Palette::SURFACE, theme::hairline(Palette::BORDER_07));
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, Palette::TEXT_SECONDARY);
    }
}

/// The pulsing accent "N need you" chip for the toolbar: accent-tint pill with a pulsing dot +
/// accent label. `pulse` is the current eased opacity (`0.0..=1.0`) for the dot; pass `1.0` at rest.
pub fn need_chip(ui: &mut Ui, label: &str, pulse: f32) {
    let font = theme::ui_font(11.5, Weight::SemiBold);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Palette::ACCENT_TEXT);
    let dot_d = 6.0;
    let inner_gap = 6.0;
    let pad = Vec2::new(10.0, 5.0);
    let size = Vec2::new(
        galley.size().x + dot_d + inner_gap + pad.x * 2.0,
        galley.size().y.max(dot_d) + pad.y * 2.0,
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    theme::fill_rect(ui, rect, rad::INPUT, Palette::ACCENT_TINT, theme::hairline(Palette::ACCENT_BORDER));
    let dot_center = egui::pos2(rect.min.x + pad.x + dot_d / 2.0, rect.center().y);
    let a = (255.0 * pulse.clamp(0.0, 1.0)) as u8;
    let dot_c = Color32::from_rgba_premultiplied(Palette::ACCENT.r(), Palette::ACCENT.g(), Palette::ACCENT.b(), a);
    theme::dot(ui, dot_center, dot_d, dot_c);
    ui.painter().text(
        egui::pos2(dot_center.x + dot_d / 2.0 + inner_gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        Palette::ACCENT_TEXT,
    );
}

/// A small status dot (need-action / waiting). `pulse` is the current eased opacity in `0.0..=1.0`
/// supplied by the animation driver; at rest pass `1.0`.
pub fn status_dot(ui: &mut Ui, color: Color32, diameter: f32, pulse: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    if ui.is_rect_visible(rect) {
        let a = (color.a() as f32 * pulse.clamp(0.0, 1.0)) as u8;
        let c = Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), a);
        theme::dot(ui, rect.center(), diameter, c);
    }
}

/// Eased pulse opacity in `0.4..=1.0` from a free-running animation clock (ms), matching the
/// prototype's `tcpulse` (opacity 1 → 0.4 → 1 over ~1.2s). Feed [`crate::freeplay::model::AppState`]'s
/// `clock_ms`.
pub fn pulse_opacity(clock_ms: f32) -> f32 {
    let period = 1_200.0;
    let phase = (clock_ms % period) / period; // 0..1
    // Triangle wave 1 → 0.4 → 1.
    let tri = 1.0 - (phase - 0.5).abs() * 2.0; // 0 at ends, 1 at middle
    0.4 + 0.6 * (1.0 - tri)
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// A full-width primary accent (emerald) button with near-black on-accent text — the host "Create
/// free table" / "Next table" CTA. `height` lets the host footer use a taller button. Returns the
/// click response.
pub fn primary_button(ui: &mut Ui, label: &str, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if ui.is_rect_visible(rect) {
        // Slight darken on hover to signal interactivity (egui has no gradient/shadow).
        let fill = if resp.hovered() {
            Color32::from_rgb(0x3a, 0xe0, 0xac)
        } else {
            Palette::ACCENT
        };
        theme::fill_rect(ui, rect, rad::PANEL, fill, egui::Stroke::NONE);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::ui_font(13.5, Weight::SemiBold),
            Palette::ON_ACCENT,
        );
    }
    resp
}

/// A neutral slate button (`#262a31` + hairline, primary text) — the calm "Leave table" / "Done"
/// confirm CTA. Full-width to `height`. Returns the click response.
pub fn slate_button(ui: &mut Ui, label: &str, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() {
            Color32::from_rgb(0x2e, 0x33, 0x3b)
        } else {
            Palette::SLATE_BTN
        };
        theme::fill_rect(ui, rect, rad::PANEL, fill, theme::hairline(Palette::BORDER_07));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::ui_font(13.0, Weight::SemiBold),
            Palette::TEXT_PRIMARY,
        );
    }
    resp
}

/// A secondary/ghost button (transparent-ish neutral surface + hairline, secondary text) — e.g.
/// "Stay at table". Full-width to `height`. Returns the click response.
pub fn neutral_button(ui: &mut Ui, label: &str, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { Palette::SURFACE } else { Palette::NEUTRAL_BTN };
        theme::fill_rect(ui, rect, rad::PANEL, fill, theme::hairline(Palette::BORDER_07));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::ui_font(13.0, Weight::Medium),
            Palette::TEXT_SECONDARY,
        );
    }
    resp
}
