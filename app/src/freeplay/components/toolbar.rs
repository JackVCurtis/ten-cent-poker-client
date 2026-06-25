//! The 54px grid toolbar: the `Tables` title, an `N active` count chip, a pulsing `N need you`
//! accent chip when one or more tables await action, a muted "chips in play" readout, plus the
//! right-side keyboard legend (`F fold · C call · R raise`), a `Next table` button (Space hint), and
//! the `+ New table` accent button that opens the host slide-over.

use eframe::egui::{self, Align, Color32, Layout, Rect, Sense, Ui, Vec2};

use crate::freeplay::model::AppState;
use crate::freeplay::theme::{self, rad, size, Palette, Weight};

/// What the user clicked in the toolbar this frame; the grid screen reacts.
#[derive(Clone, Copy, Default)]
pub struct ToolbarResponse {
    /// `Next table` (or its Space shortcut surrogate) was clicked.
    pub next_table: bool,
    /// `+ New table` was clicked — open the host slide-over (`AppState::host_open = true`).
    pub new_table: bool,
}

/// Render the toolbar across the full width at [`size::TOOLBAR_H`]. `pulse` is the current eased
/// opacity (0..=1) for the "need you" dot, supplied by the animation driver.
///
/// `state` is borrowed immutably: the toolbar only *reads* the table counts and returns which control
/// fired so the grid screen can apply the matching [`AppState`] transition (open the slide-over /
/// focus the next your-turn table). All amounts shown are CHIPS (free play has no `$`).
pub fn render(ui: &mut Ui, state: &AppState, pulse: f32) -> ToolbarResponse {
    let mut resp = ToolbarResponse::default();

    // Reserve the full toolbar band; paint its `#0c0d11` fill + bottom hairline, then lay the two
    // clusters out inside the horizontal content padding (20px in the prototype).
    let band = ui.available_rect_before_wrap();
    let band = Rect::from_min_size(band.min, Vec2::new(band.width(), size::TOOLBAR_H));
    ui.painter().rect_filled(band, egui::CornerRadius::ZERO, Palette::TOOLBAR);
    ui.painter().hline(
        band.x_range(),
        band.max.y - 0.5,
        theme::hairline(Palette::BORDER_05),
    );

    let inner = band.shrink2(Vec2::new(20.0, 0.0));
    let mut content = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(
        Layout::left_to_right(Align::Center),
    ));
    content.spacing_mut().item_spacing = Vec2::new(12.0, 0.0);

    // Left cluster: title, active chip, optional "need you" chip, muted chips-in-play readout.
    content.label(
        egui::RichText::new("Tables")
            .font(theme::ui_font(16.0, Weight::Bold))
            .color(Palette::TEXT_PRIMARY),
    );
    widgets_count_chip(&mut content, &format!("{} active", state.active_count()));
    let need = state.need_count();
    if need > 0 {
        crate::freeplay::widgets::need_chip(&mut content, &need_label(need), pulse);
    }
    // Tighten the gap before the readout to mirror the prototype's `margin-left:2px`.
    content.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
    content.label(
        egui::RichText::new(format!("· {} chips in play", chips_in_play(state)))
            .font(theme::mono_font(11.0, Weight::Regular))
            .color(Palette::TEXT_MUTED_DIM),
    );

    // Right cluster: laid out right-to-left so the accent CTA pins to the far edge, then the
    // `Next table` button, then the keyboard legend (which thus ends up visually leftmost).
    content.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(14.0, 0.0);
        if new_table_button(ui).clicked() {
            resp.new_table = true;
        }
        if next_table_button(ui).clicked() {
            resp.next_table = true;
        }
        keyboard_legend(ui);
    });

    // Advance the parent cursor past the band we painted into.
    ui.allocate_rect(band, Sense::hover());
    resp
}

/// The need-action label: `1 needs you` (singular) / `N need you` (plural), per the prototype.
fn need_label(n: usize) -> String {
    if n == 1 {
        "1 needs you".to_string()
    } else {
        format!("{n} need you")
    }
}

/// Total chips you hold across every table, grouped with thousands separators (`12,560`).
fn chips_in_play(state: &AppState) -> String {
    group_thousands(state.chips_in_play())
}

/// Format a chip count with comma thousands separators (`8560` → `8,560`).
fn group_thousands(n: u64) -> String {
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

/// The neutral `N active` count chip (delegates to the shared widget; named locally so the call site
/// reads as a left-cluster step alongside the other toolbar pieces).
fn widgets_count_chip(ui: &mut Ui, label: &str) {
    crate::freeplay::widgets::count_chip(ui, label);
}

/// The muted keyboard legend `F fold · C call · R raise`: mono key glyphs in `#7a7f88`, prose in the
/// dimmest muted text. Laid out left-to-right inside the right cluster's reserved slot so the glyphs
/// keep their order regardless of the surrounding right-to-left flow.
fn keyboard_legend(ui: &mut Ui) {
    let key_font = theme::mono_font(11.0, Weight::Medium);
    let prose_font = theme::ui_font(11.0, Weight::Regular);
    let key_color = Color32::from_rgb(0x7a, 0x7f, 0x88);
    let prose_color = Color32::from_rgb(0x56, 0x5a, 0x62);

    // Size the legend to its content so right-to-left flow reserves exactly the needed width.
    let parts: [(&str, &egui::FontId, Color32); 6] = [
        ("F", &key_font, key_color),
        (" fold · ", &prose_font, prose_color),
        ("C", &key_font, key_color),
        (" call · ", &prose_font, prose_color),
        ("R", &key_font, key_color),
        (" raise", &prose_font, prose_color),
    ];
    let mut width = 0.0;
    let mut height = 0.0f32;
    for (text, font, color) in parts.iter() {
        let g = ui.painter().layout_no_wrap(text.to_string(), (*font).clone(), *color);
        width += g.size().x;
        height = height.max(g.size().y);
    }
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let mut x = rect.min.x;
    for (text, font, color) in parts.iter() {
        let g = painter.layout_no_wrap(text.to_string(), (*font).clone(), *color);
        painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g.clone(), *color);
        x += g.size().x;
    }
}

/// The `Next table` button: a neutral surface pill with a secondary label and a small mono `Space`
/// shortcut tag. Returns the click response.
fn next_table_button(ui: &mut Ui) -> egui::Response {
    let label = "Next table";
    let label_font = theme::ui_font(12.0, Weight::Medium);
    let tag = "Space";
    let tag_font = theme::mono_font(10.0, Weight::Medium);

    let pad = Vec2::new(12.0, 7.0);
    let inner_gap = 7.0;
    let tag_pad = Vec2::new(6.0, 2.0);

    let label_g = ui.painter().layout_no_wrap(label.to_string(), label_font.clone(), Palette::TEXT_PRIMARY);
    let tag_g = ui.painter().layout_no_wrap(tag.to_string(), tag_font.clone(), Color32::from_rgb(0x7a, 0x7f, 0x88));
    let tag_size = tag_g.size() + tag_pad * 2.0;

    let content_w = label_g.size().x + inner_gap + tag_size.x;
    let content_h = label_g.size().y.max(tag_size.y);
    let size = Vec2::new(content_w + pad.x * 2.0, content_h + pad.y * 2.0);

    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    // Surface fill + hairline; the border lifts slightly on hover (no shadow/gradient in egui).
    let stroke = if resp.hovered() {
        theme::hairline(Palette::BORDER_10)
    } else {
        theme::hairline(Palette::BORDER_07)
    };
    theme::fill_rect(ui, rect, rad::INPUT, Palette::SURFACE, stroke);

    let painter = ui.painter();
    let label_pos = egui::pos2(rect.min.x + pad.x, rect.center().y);
    let label_color = if resp.hovered() { Palette::TEXT_PRIMARY } else { Palette::TEXT_PRIMARY_DIM };
    painter.text(label_pos, egui::Align2::LEFT_CENTER, label, label_font, label_color);

    // The `Space` tag: a faint translucent-white chip with mono text.
    let tag_min = egui::pos2(label_pos.x + label_g.size().x + inner_gap, rect.center().y - tag_size.y / 2.0);
    let tag_rect = Rect::from_min_size(tag_min, tag_size);
    theme::fill_rect(ui, tag_rect, rad::BADGE, Palette::BORDER_05, egui::Stroke::NONE);
    painter.text(
        tag_rect.center(),
        egui::Align2::CENTER_CENTER,
        tag,
        tag_font,
        Color32::from_rgb(0x7a, 0x7f, 0x88),
    );
    resp
}

/// The `+ New table` accent (emerald) button with near-black on-accent text. Sized to its label so it
/// sits naturally in the right cluster. Returns the click response.
fn new_table_button(ui: &mut Ui) -> egui::Response {
    let label = "+ New table";
    let font = theme::ui_font(13.0, Weight::SemiBold);
    let pad = Vec2::new(14.0, 8.0);

    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Palette::ON_ACCENT);
    let size = galley.size() + pad * 2.0;

    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    // Slight lighten on hover to signal interactivity (egui has no gradient/shadow).
    let fill = if resp.hovered() {
        Color32::from_rgb(0x3a, 0xe0, 0xac)
    } else {
        Palette::ACCENT
    };
    theme::fill_rect(ui, rect, rad::INPUT, fill, egui::Stroke::NONE);
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, Palette::ON_ACCENT);
    resp
}
