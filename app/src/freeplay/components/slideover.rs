//! The right slide-over wrapper that hosts the [`super::host_rail`] over the grid. Paints a dimmed
//! backdrop across the whole window, a 380px right-anchored panel (`#0d0e12`) with a `Host a table`
//! header + `✕` close button, and embeds the config rail (compact variant) inside a scroll area.
//! Closes on `✕` or a backdrop click; `Create free table` closes it too (handled by the caller).
//!
//! The prototype's `tcslide` entrance (`translateX(100%) → 0` over 0.28s) is approximated by easing
//! the panel's X offset in from its own width to 0, driven by an open-start timestamp stashed in egui
//! memory (the model owns no per-frame UI clock for this) and a repaint request while it animates.
//!
//! Both layers are `egui::Area`s in `Order::Foreground` so the overlay floats above the grid tiles
//! regardless of where in the screen layout `render` is invoked. Renders only when `state.host_open`.

use eframe::egui::{
    self, Align, Align2, Area, Id, Layout, Order, Pos2, Rect, ScrollArea, Sense, Ui, UiBuilder,
    Vec2,
};

use crate::freeplay::model::AppState;
use crate::freeplay::theme::{self, size, Palette, Weight};

/// What the slide-over produced this frame.
#[derive(Clone, Copy, Default)]
pub struct SlideoverResponse {
    /// The `✕` or backdrop requested close.
    pub close: bool,
    /// `Create free table` was clicked inside the rail.
    pub create: bool,
}

/// Header band height (prototype `height:54px`).
const HEADER_H: f32 = 54.0;
/// Horizontal padding inside the header (prototype `padding:0 20px`).
const HEADER_PAD_X: f32 = 20.0;
/// Body padding around the embedded rail (prototype `padding:20px`).
const BODY_PAD: f32 = 20.0;
/// Backdrop scrim (`rgba(7,8,10,0.55)`). egui can't blur, so the prototype's `backdrop-filter` is
/// dropped and only the dim is applied.
const SCRIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(4, 4, 6, 140);
/// Panel left edge (`#0d0e12`).
const PANEL_BG: egui::Color32 = Palette::PANEL_BASE;
/// `tcslide` entrance duration (prototype `0.28s`).
const SLIDE_SECS: f32 = 0.28;

/// Render the slide-over if `state.host_open`. Forwards the embedded rail's edits into `state.host`
/// and returns close/create intents for the grid to apply.
pub fn render(ui: &mut Ui, state: &mut AppState) -> SlideoverResponse {
    let mut resp = SlideoverResponse::default();
    if !state.host_open {
        // Reset the entrance clock so the next open re-plays the slide from off-screen.
        ui.ctx().memory_mut(|m| m.data.remove::<f64>(slide_start_id()));
        return resp;
    }

    let ctx = ui.ctx().clone();
    let screen = ctx.screen_rect();
    let now = ctx.input(|i| i.time);

    // Entrance progress: ease the panel in from `translateX(100%)` to `0` over `SLIDE_SECS`. The open
    // timestamp is stashed on first visible frame; while animating we keep requesting repaints.
    let start =
        ctx.memory_mut(|m| *m.data.get_temp_mut_or_insert_with(slide_start_id(), || now));
    let t = (((now - start) as f32) / SLIDE_SECS).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t); // approximates cubic-bezier(0.22,1,0.36,1)
    let offset_x = size::SLIDEOVER_W * (1.0 - eased);
    if t < 1.0 {
        ctx.request_repaint();
    }

    // 1) Full-window dimmed backdrop (prototype `inset:0; rgba(7,8,10,0.55)`). A click anywhere on the
    //    scrim closes the slide-over. The panel sits above it and swallows clicks on its own controls.
    let backdrop = Area::new(Id::new("freeplay_slideover_backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .sense(Sense::click())
        .show(&ctx, |ui| {
            let (rect, r) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, SCRIM);
            r
        });
    if backdrop.inner.clicked() {
        resp.close = true;
    }

    // 2) The 380px right-anchored panel (prototype `width:380px; height:100%; border-left hairline`),
    //    slid in by `offset_x`. Anchored to the screen's right edge so it tracks window resizes.
    let panel_left = screen.right() - size::SLIDEOVER_W + offset_x;
    let panel_rect = Rect::from_min_max(
        Pos2::new(panel_left, screen.top()),
        Pos2::new(panel_left + size::SLIDEOVER_W, screen.bottom()),
    );

    let panel = Area::new(Id::new("freeplay_slideover_panel"))
        .order(Order::Foreground)
        .fixed_pos(panel_rect.min)
        .show(&ctx, |ui| {
            ui.set_clip_rect(panel_rect);
            // Panel base + the left-edge hairline (the prototype's `box-shadow` drop is omitted —
            // egui has no blur — leaving the border alone to separate the panel from the scrim).
            ui.painter().rect_filled(panel_rect, egui::CornerRadius::ZERO, PANEL_BG);
            ui.painter().vline(
                panel_rect.left() + 0.5,
                panel_rect.y_range(),
                theme::hairline(Palette::BORDER_10),
            );
            paint_body(ui, panel_rect, state, &mut resp)
        });
    let _ = panel.inner;

    resp
}

/// Lay out the panel interior: the 54px `Host a table` header (+ `✕`), then the scrollable body that
/// embeds the compact host rail. Surfaces the `✕` close and the rail's `create` into `resp`.
fn paint_body(ui: &mut Ui, panel: Rect, state: &mut AppState, resp: &mut SlideoverResponse) {
    // --- header band ---
    let header = Rect::from_min_max(
        panel.min,
        Pos2::new(panel.right(), panel.top() + HEADER_H),
    );
    // Title (Hanken Grotesk 700 / 16px).
    ui.painter().text(
        Pos2::new(header.left() + HEADER_PAD_X, header.center().y),
        Align2::LEFT_CENTER,
        "Host a table",
        theme::ui_font(16.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    // `✕` close button — a muted 18px glyph at the far right, brightening on hover.
    let close_sz = 28.0;
    let close_rect = Rect::from_center_size(
        Pos2::new(header.right() - HEADER_PAD_X - close_sz / 2.0, header.center().y),
        Vec2::splat(close_sz),
    );
    let close = ui.interact(close_rect, ui.id().with("slideover_close"), Sense::click());
    let close_color = if close.hovered() { Palette::TEXT_SECONDARY } else { Palette::TEXT_MUTED };
    // Drawn X (two strokes) — the fallback font renders U+2715 as a missing-glyph box.
    let c = close_rect.center();
    let arm = 5.5;
    let xstroke = egui::Stroke::new(1.6, close_color);
    ui.painter()
        .line_segment([egui::pos2(c.x - arm, c.y - arm), egui::pos2(c.x + arm, c.y + arm)], xstroke);
    ui.painter()
        .line_segment([egui::pos2(c.x - arm, c.y + arm), egui::pos2(c.x + arm, c.y - arm)], xstroke);
    if close.clicked() {
        resp.close = true;
    }
    // Header bottom hairline (prototype `border-bottom: rgba(255,255,255,0.06)`).
    ui.painter().hline(
        header.x_range(),
        header.bottom() - 0.5,
        theme::hairline(Palette::BORDER_07),
    );

    // --- scrollable body hosting the compact host rail ---
    let body = Rect::from_min_max(Pos2::new(panel.left(), header.bottom()), panel.max)
        .shrink(BODY_PAD);
    let mut body_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .layout(Layout::top_down(Align::Min)),
    );
    body_ui.set_clip_rect(body);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut body_ui, |ui| {
            // The compact rail trims its standalone heading/presets and binds directly to `state.host`;
            // its footer carries the `Create free table` CTA whose click we surface as `create`.
            let rail = super::host_rail::render(ui, state, true);
            if rail.create {
                resp.create = true;
            }
        });
}

/// Stable memory key for the entrance-animation open timestamp.
fn slide_start_id() -> Id {
    Id::new("freeplay_slideover_open_at")
}

/// Ease-out cubic — a close stand-in for the prototype's `cubic-bezier(0.22,1,0.36,1)` (fast in,
/// gentle settle) used for the `translateX` slide.
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}
