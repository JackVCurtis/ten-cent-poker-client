//! The host-a-table config rail (left column of the host screen, and the body of the slide-over).
//! Top to bottom: the `Build your table` heading (standalone only), optional Quick-start presets,
//! the Game segmented pills, the Seats stepper + Visibility segmented control, the Stakes cards
//! (locked to `Free play` here — no buy-in field, no fee row), the Blinds chips, and the full-width
//! `Create free table` accent CTA in the footer.
//!
//! Mutates `state.host` directly as the user changes controls; returns whether the CTA was clicked.
//!
//! Crypto is out of scope: the `ETH buy-in` stake card is painted DISABLED (so the locked `Free play`
//! choice reads clearly) but never selectable, and because Free play is selected the buy-in field and
//! the fee row are omitted entirely. All blinds are CHIPS.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Ui, Vec2};

use crate::freeplay::model::{AppState, Game, HostConfig, Visibility, SEATS_MAX, SEATS_MIN};
use crate::freeplay::theme::{self, rad, Palette, Weight};
use crate::freeplay::widgets::{self, StepperClick};

/// What the rail produced this frame.
#[derive(Clone, Copy, Default)]
pub struct HostRailResponse {
    /// `Create free table` was clicked — append a new free table and (if in the slide-over) close it.
    pub create: bool,
}

/// A free-table quick-start template: a one-tap preset that fills several `HostConfig` fields at once
/// (the prototype's `applyPreset`, recast as free-play — chips, no buy-in). `label` shows on the chip.
struct Preset {
    label: &'static str,
    game: Game,
    seats: usize,
    blinds: &'static str,
}

/// Free-play quick-start templates (chips-only analogues of the prototype's staked presets).
const PRESETS: &[Preset] = &[
    Preset { label: "NLH · 6-max", game: Game::Holdem, seats: 6, blinds: "20 / 40" },
    Preset { label: "PLO · 6-max", game: Game::Omaha, seats: 6, blinds: "10 / 20" },
    Preset { label: "NLH · 9-max", game: Game::Holdem, seats: 9, blinds: "40 / 80" },
    Preset { label: "Stud · 8-max", game: Game::Stud, seats: 8, blinds: "10 / 20" },
];

/// Free-play blind levels, in chips (the prototype's staked `.10/.25 … 2/5` recast as chip strings).
const BLIND_LEVELS: &[&str] = &["10 / 20", "20 / 40", "40 / 80", "100 / 200", "200 / 500"];

/// Render the config rail editing `state.host`. `compact` trims the heading/presets for the 380px
/// slide-over variant (vs the 392px standalone rail). Returns the CTA click.
pub fn render(ui: &mut Ui, state: &mut AppState, compact: bool) -> HostRailResponse {
    let mut resp = HostRailResponse::default();
    let host = &mut state.host;

    // Vertical rhythm between sections roughly matches the prototype's 20–24px section margins; the
    // label→control gap is the prototype's 10px.
    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

    // --- heading (standalone only) ---
    if !compact {
        ui.label(
            egui::RichText::new("Build your table")
                .font(theme::ui_font(20.0, Weight::Bold))
                .color(Palette::TEXT_PRIMARY),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("It comes together live on the right")
                .font(theme::ui_font(12.5, Weight::Regular))
                .color(Palette::TEXT_MUTED),
        );
        ui.add_space(12.0);
    }

    // --- quick start presets (omitted in the compact slide-over to save vertical room) ---
    if !compact {
        section_label(ui, "Quick start");
        ui.add_space(2.0);
        preset_chips(ui, host);
        ui.add_space(16.0);
    }

    // --- game ---
    section_label(ui, "Game");
    ui.add_space(2.0);
    let games = [Game::Holdem, Game::Omaha, Game::Stud];
    let labels = ["Hold'em", "Omaha", "Stud"];
    let selected = games.iter().position(|g| *g == host.game).unwrap_or(0);
    if let Some(i) = widgets::segmented_row(ui, &labels, selected, false) {
        host.game = games[i];
    }
    ui.add_space(14.0);

    // --- seats stepper + visibility, side by side (prototype's 1fr/1fr grid) ---
    seats_and_visibility(ui, host);
    ui.add_space(14.0);

    // --- stakes (Free play LOCKED; ETH buy-in shown disabled and ignored) ---
    section_label(ui, "Stakes");
    ui.add_space(2.0);
    stakes_cards(ui);
    ui.add_space(14.0);
    // No buy-in field and no fee row: Free play is selected, so both are hidden by design.

    // --- blinds (expressed in chips) ---
    // Split across two rows so the wider mono labels (`100 / 200`, `200 / 500`) aren't squeezed/clipped
    // into a single 5-up row in the narrow rail.
    section_label(ui, "Blinds");
    ui.add_space(2.0);
    let sel = BLIND_LEVELS.iter().position(|b| *b == host.blinds);
    let (row1, row2) = BLIND_LEVELS.split_at(3);
    if let Some(i) = widgets::segmented_row(ui, row1, sel.filter(|&s| s < 3).unwrap_or(usize::MAX), true) {
        host.blinds = row1[i].to_string();
    }
    ui.add_space(8.0);
    let sel2 = sel.filter(|&s| s >= 3).map(|s| s - 3).unwrap_or(usize::MAX);
    if let Some(i) = widgets::segmented_row(ui, row2, sel2, true) {
        host.blinds = row2[i].to_string();
    }
    ui.add_space(16.0);

    // --- footer CTA (no fee row above it, no signing) ---
    if widgets::primary_button(ui, "Create free table", 46.0).clicked() {
        resp.create = true;
    }

    resp
}

/// A section label: uppercase 11px/600, letter-spaced, muted `#6e737d` — the prototype's section
/// captions. egui has no letter-spacing, so the cap text alone carries the look.
fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::ui_font(11.0, Weight::SemiBold))
            .color(Palette::TEXT_MUTED),
    );
}

/// The wrapping row of quick-start preset chips. Each chip is a neutral surface pill with `#c6cad1`
/// medium text; clicking it applies that template to `host`. Chips wrap onto multiple lines like the
/// prototype's `flex-wrap`.
fn preset_chips(ui: &mut Ui, host: &mut HostConfig) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(7.0, 7.0);
        for p in PRESETS {
            if preset_chip(ui, p.label).clicked() {
                host.game = p.game;
                host.seats = p.seats;
                host.blinds = p.blinds.to_string();
            }
        }
    });
}

/// One preset chip: a content-sized surface pill (`#16181e` + hairline, `#c6cad1` text). Returns the
/// click response. The fill darkens a hair on hover (egui has no shadow/gradient).
fn preset_chip(ui: &mut Ui, label: &str) -> egui::Response {
    let font = theme::ui_font(12.0, Weight::Medium);
    let text_color = Color32::from_rgb(0xc6, 0xca, 0xd1);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), text_color);
    let pad = Vec2::new(12.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());
    if ui.is_rect_visible(rect) {
        let stroke = if resp.hovered() {
            theme::hairline(Palette::BORDER_10)
        } else {
            theme::hairline(Palette::BORDER_07)
        };
        theme::fill_rect(ui, rect, rad::INPUT, Palette::SURFACE, stroke);
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, text_color);
    }
    resp
}

/// The Seats stepper and Visibility segmented control laid out side by side in two equal columns
/// (the prototype's `grid-template-columns:1fr 1fr`), each with its own uppercase caption above.
fn seats_and_visibility(ui: &mut Ui, host: &mut HostConfig) {
    let gap = 14.0;
    let col_w = ((ui.available_width() - gap) / 2.0).max(80.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(gap, 0.0);

        // Left column: Seats stepper.
        ui.allocate_ui(Vec2::new(col_w, 0.0), |ui| {
            ui.set_width(col_w);
            section_label(ui, "Seats");
            ui.add_space(2.0);
            match widgets::stepper_bounded(ui, host.seats, SEATS_MIN, SEATS_MAX) {
                StepperClick::Down => host.seats = host.seats.saturating_sub(1).max(SEATS_MIN),
                StepperClick::Up => host.seats = (host.seats + 1).min(SEATS_MAX),
                StepperClick::None => {}
            }
        });

        // Right column: Visibility segmented (Private / Public).
        ui.allocate_ui(Vec2::new(col_w, 0.0), |ui| {
            ui.set_width(col_w);
            section_label(ui, "Visibility");
            ui.add_space(2.0);
            let vis = [Visibility::Private, Visibility::Public];
            let labels = ["Private", "Public"];
            let selected = vis.iter().position(|v| *v == host.visibility).unwrap_or(0);
            if let Some(i) = widgets::segmented_row(ui, &labels, selected, false) {
                host.visibility = vis[i];
            }
        });
    });
}

/// The two Stakes cards in a 1fr/1fr grid: `Free play` (selected + LOCKED — accent tint/border) and
/// `ETH buy-in` (DISABLED and ignored in free play). Each card carries a 13px title + 11.5px caption.
/// Neither is interactive: Free play is the only option here, so there is nothing to click.
fn stakes_cards(ui: &mut Ui) {
    let gap = 10.0;
    let card_h = 56.0;
    let total_w = ui.available_width();
    let card_w = ((total_w - gap) / 2.0).max(80.0);

    let (row, _) = ui.allocate_exact_size(Vec2::new(total_w, card_h), Sense::hover());
    if !ui.is_rect_visible(row) {
        return;
    }
    let free_rect = Rect::from_min_size(row.min, Vec2::new(card_w, card_h));
    let eth_rect = Rect::from_min_size(
        egui::pos2(row.max.x - card_w, row.min.y),
        Vec2::new(card_w, card_h),
    );

    // Free play — selected + locked: accent tint fill + accent border, full-strength text.
    stake_card(
        ui,
        free_rect,
        "Free play",
        "Just chips",
        Palette::ACCENT_TINT,
        theme::hairline(Palette::ACCENT_BORDER),
        Palette::ACCENT_TEXT,
        Palette::TEXT_MUTED,
    );

    // ETH buy-in — disabled/ignored: muted surface, dimmed text (no accent, not clickable).
    stake_card(
        ui,
        eth_rect,
        "ETH buy-in",
        "Out of scope",
        Palette::SURFACE,
        theme::hairline(Palette::BORDER_05),
        Palette::TEXT_MUTED_DIM,
        Palette::TEXT_MUTED_DIM,
    );
}

/// Paint a single stakes card: a 10px-radius surface with a title (13px/600) over a caption
/// (11.5px/400), both left-aligned with the prototype's `13px 14px` padding.
#[allow(clippy::too_many_arguments)]
fn stake_card(
    ui: &Ui,
    rect: Rect,
    title: &str,
    caption: &str,
    fill: Color32,
    stroke: Stroke,
    title_color: Color32,
    caption_color: Color32,
) {
    ui.painter().rect(
        rect,
        CornerRadius::same(10),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let pad = Vec2::new(14.0, 13.0);
    let painter = ui.painter();
    let title_pos = egui::pos2(rect.min.x + pad.x, rect.min.y + pad.y);
    painter.text(
        title_pos,
        egui::Align2::LEFT_TOP,
        title,
        theme::ui_font(13.0, Weight::SemiBold),
        title_color,
    );
    painter.text(
        egui::pos2(title_pos.x, title_pos.y + 18.0),
        egui::Align2::LEFT_TOP,
        caption,
        theme::ui_font(11.5, Weight::Regular),
        caption_color,
    );
}
