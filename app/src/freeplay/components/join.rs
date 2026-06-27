//! The right "Join a table" slide-over: paste a `tcpoker://` invite and dial it. Mirrors the host
//! [`super::slideover`] chrome (dimmed backdrop, 380px right-anchored panel, `tcslide` entrance) but
//! its body is a single paste field bound to [`AppState::join_uri`] plus a `Join table` CTA enabled
//! only when the pasted text looks like an invite ([`crate::freeplay::model::is_valid_invite`]).
//!
//! Renders only when `state.join_open`. Closing (`✕` / backdrop) clears the open flag via the
//! returned intent; `Join` returns the trimmed URI for the app to hand to `TableConn::join`.

use eframe::egui::{
    self, Align, Align2, Area, Id, Layout, Order, Pos2, Rect, ScrollArea, Sense, Ui, UiBuilder,
    Vec2,
};

use crate::freeplay::model::{is_valid_invite, AppState};
use crate::freeplay::theme::{self, rad, size, Palette, Weight};
use crate::freeplay::widgets;

/// What the join slide-over produced this frame.
#[derive(Clone, Default)]
pub struct JoinResponse {
    /// The `✕` or backdrop requested close.
    pub close: bool,
    /// `Join table` was clicked with a valid invite — the trimmed `tcpoker://` URI to dial.
    pub join: Option<String>,
}

/// Header band height (matches the host slide-over).
const HEADER_H: f32 = 54.0;
const HEADER_PAD_X: f32 = 20.0;
const BODY_PAD: f32 = 20.0;
/// Backdrop scrim — same dim as the host slide-over.
const SCRIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(4, 4, 6, 140);
const PANEL_BG: egui::Color32 = Palette::PANEL_BASE;
const SLIDE_SECS: f32 = 0.28;

/// Render the join slide-over if `state.join_open`. Edits `state.join_uri` in place and returns the
/// close / join intents for the app to apply (start the guest connection, close the panel).
pub fn render(ui: &mut Ui, state: &mut AppState) -> JoinResponse {
    let mut resp = JoinResponse::default();
    if !state.join_open {
        ui.ctx()
            .memory_mut(|m| m.data.remove::<f64>(slide_start_id()));
        return resp;
    }

    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let now = ctx.input(|i| i.time);

    let start = ctx.memory_mut(|m| *m.data.get_temp_mut_or_insert_with(slide_start_id(), || now));
    let t = (((now - start) as f32) / SLIDE_SECS).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    let offset_x = size::SLIDEOVER_W * (1.0 - eased);
    if t < 1.0 {
        ctx.request_repaint();
    }

    // 1) Full-window dimmed backdrop; a click closes the slide-over.
    let backdrop = Area::new(Id::new("freeplay_join_backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .sense(Sense::click())
        .show(&ctx, |ui| {
            let (rect, r) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::ZERO, SCRIM);
            r
        });
    if backdrop.inner.clicked() {
        resp.close = true;
    }

    // 2) The 380px right-anchored panel, slid in by `offset_x`.
    let panel_left = screen.right() - size::SLIDEOVER_W + offset_x;
    let panel_rect = Rect::from_min_max(
        Pos2::new(panel_left, screen.top()),
        Pos2::new(panel_left + size::SLIDEOVER_W, screen.bottom()),
    );

    Area::new(Id::new("freeplay_join_panel"))
        .order(Order::Foreground)
        .fixed_pos(panel_rect.min)
        .show(&ctx, |ui| {
            ui.set_clip_rect(panel_rect);
            // Swallow pointer events across the whole panel so clicks on its empty regions don't
            // hit-test through to the full-window backdrop and dismiss the slide-over. The panel's
            // own painter fills don't register a widget, so without this only the small cluster of
            // controls would block the scrim. Registered first so the close `✕` / paste field /
            // `Join` CTA — added afterwards on this same layer — still win the hit-test.
            ui.interact(
                panel_rect,
                ui.id().with("join_panel_guard"),
                Sense::click_and_drag(),
            );
            ui.painter()
                .rect_filled(panel_rect, egui::CornerRadius::ZERO, PANEL_BG);
            ui.painter().vline(
                panel_rect.left() + 0.5,
                panel_rect.y_range(),
                theme::hairline(Palette::BORDER_10),
            );
            paint_body(ui, panel_rect, state, &mut resp);
        });

    resp
}

/// Lay out the panel interior: the 54px `Join a table` header (+ `✕`), then the scrollable body with
/// the invite paste field and the `Join table` CTA. Surfaces the `✕` close and the join intent.
fn paint_body(ui: &mut Ui, panel: Rect, state: &mut AppState, resp: &mut JoinResponse) {
    // --- header band ---
    let header = Rect::from_min_max(panel.min, Pos2::new(panel.right(), panel.top() + HEADER_H));
    ui.painter().text(
        Pos2::new(header.left() + HEADER_PAD_X, header.center().y),
        Align2::LEFT_CENTER,
        "Join a table",
        theme::ui_font(16.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    let close_sz = 28.0;
    let close_rect = Rect::from_center_size(
        Pos2::new(
            header.right() - HEADER_PAD_X - close_sz / 2.0,
            header.center().y,
        ),
        Vec2::splat(close_sz),
    );
    let close = ui.interact(close_rect, ui.id().with("join_close"), Sense::click());
    let close_color = if close.hovered() {
        Palette::TEXT_SECONDARY
    } else {
        Palette::TEXT_MUTED
    };
    let c = close_rect.center();
    let arm = 5.5;
    let xstroke = egui::Stroke::new(1.6, close_color);
    ui.painter().line_segment(
        [
            egui::pos2(c.x - arm, c.y - arm),
            egui::pos2(c.x + arm, c.y + arm),
        ],
        xstroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(c.x - arm, c.y + arm),
            egui::pos2(c.x + arm, c.y - arm),
        ],
        xstroke,
    );
    if close.clicked() {
        resp.close = true;
    }
    ui.painter().hline(
        header.x_range(),
        header.bottom() - 0.5,
        theme::hairline(Palette::BORDER_07),
    );

    // --- scrollable body: caption, paste field, Join CTA ---
    let body =
        Rect::from_min_max(Pos2::new(panel.left(), header.bottom()), panel.max).shrink(BODY_PAD);
    let mut body_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .layout(Layout::top_down(Align::Min)),
    );
    body_ui.set_clip_rect(body);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut body_ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            ui.label(
                egui::RichText::new("INVITE LINK")
                    .font(theme::ui_font(11.0, Weight::SemiBold))
                    .color(Palette::TEXT_MUTED),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Paste the host's tcpoker:// invite to take a seat.")
                    .font(theme::ui_font(12.5, Weight::Regular))
                    .color(Palette::TEXT_MUTED),
            );
            ui.add_space(8.0);

            ui.add(
                egui::TextEdit::multiline(&mut state.join_uri)
                    .hint_text("tcpoker://…")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(theme::mono_font(12.0, Weight::Regular)),
            );
            ui.add_space(16.0);

            let valid = is_valid_invite(&state.join_uri);
            // Reuse the shared primary CTA when enabled; a muted slate look when not (no click).
            if valid {
                if widgets::primary_button(ui, "Join table", 46.0).clicked() {
                    resp.join = Some(state.join_uri.trim().to_string());
                }
            } else {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::hover());
                theme::fill_rect(
                    ui,
                    rect,
                    rad::PANEL,
                    Palette::SURFACE,
                    theme::hairline(Palette::BORDER_07),
                );
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Join table",
                    theme::ui_font(13.5, Weight::SemiBold),
                    Palette::TEXT_MUTED_DIM,
                );
            }
        });
}

/// Stable memory key for the entrance-animation open timestamp.
fn slide_start_id() -> Id {
    Id::new("freeplay_join_open_at")
}

/// Ease-out cubic — the host slide-over's entrance curve.
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}
