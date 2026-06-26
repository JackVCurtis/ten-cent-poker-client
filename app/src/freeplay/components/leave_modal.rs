//! The calm leave-table modal — a dimmed-backdrop overlay with a three-state machine:
//! - **Confirm**: title `Cash out of <table>`, sub `Leave the table and free up your seat.`, an
//!   optional amber "you're in a hand" note, a `Stay at table` secondary and a neutral-slate
//!   `Leave table` confirm (never the emerald CTA). No escrow/stack/gas panel for free play.
//! - **Processing**: spinner, `Leaving table…`, `See you next time`.
//! - **Done**: neutral `✓`, `You left the table`, `Your seat has been freed up.`, neutral `Done`.
//!
//! Reads `state.leave`; returns the user's choice for the grid to drive the state machine.
//!
//! The modal floats in its own `egui::Area` (Foreground) over a full-window dimming backdrop, mirroring
//! the prototype's `position:absolute;inset:0;z-index:60` layer. egui can't blur, so the prototype's
//! `backdrop-filter:blur(6px)` is approximated by the flat dim fill alone. The `tcpop` entrance (scale
//! `0.85→1` + fade) is approximated by lerping the modal's width toward full and multiplying its content
//! opacity, both driven by an `animate_bool_with_time` progress keyed to the live stage.

use eframe::egui::{
    self, Align2, Area, Color32, CornerRadius, Id, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2,
};

use crate::freeplay::model::{AppState, LeaveStage};
use crate::freeplay::theme::{self, rad, size, Palette, Weight};
use crate::freeplay::widgets;

/// What the user did in the leave modal this frame.
#[derive(Clone, Copy, Default)]
pub struct LeaveResponse {
    /// `Stay at table` — cancel and dismiss the modal.
    pub cancel: bool,
    /// `Leave table` confirm — advance Confirm → Processing.
    pub confirm: bool,
    /// `Done` — remove the tile and clear the leave flow.
    pub done: bool,
}

/// Modal inner padding (prototype `padding:24px`).
const MODAL_PAD: f32 = 24.0;
/// Backdrop dim fill (prototype `rgba(7,8,10,0.72)`).
const BACKDROP: Color32 = Color32::from_rgba_premultiplied(5, 6, 7, 184);
/// `tcpop` entrance duration (prototype `0.22s`).
const POP_TIME: f32 = 0.22;
/// Lowest scale the entrance lerps up from (prototype `transform:scale(0.85)`).
const POP_MIN_SCALE: f32 = 0.85;
/// Confirm sub-copy / done-body color (prototype `#7a7f88`, between secondary and muted).
const SUBTLE_TEXT: Color32 = Color32::from_rgb(0x7a, 0x7f, 0x88);
/// Amber "in a hand" note background (prototype `#101418`).
const NOTE_BG: Color32 = Color32::from_rgb(0x10, 0x14, 0x18);
/// Done-state check-mark badge fill (prototype `#1a1d23`).
const CHECK_BG: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x23);

/// Render the modal overlay if `state.leave` is set (else a no-op). Returns the user's choice. The
/// grid owns the Processing→Done timer; this only paints the current stage and surfaces clicks.
pub fn render(ui: &mut Ui, state: &AppState) -> LeaveResponse {
    let mut resp = LeaveResponse::default();
    let Some(leave) = state.leave else {
        return resp;
    };
    let name = state.table(leave.id).map(|t| t.name.as_str()).unwrap_or("");
    // "In a hand" mirrors the prototype's `coInHand: cot.yourTurn` — a hand is live when it is your turn.
    let in_hand = state.table(leave.id).map(|t| t.your_turn).unwrap_or(false);

    let ctx = ui.ctx().clone();
    let screen = ctx.screen_rect();

    // 1) Full-window dimming backdrop. Clicks are caught (so the grid behind doesn't react) but do NOT
    //    cancel — only `Stay at table` dismisses the modal, matching the prototype's inert backdrop.
    Area::new(Id::new("freeplay_leave_backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .sense(Sense::click())
        .show(&ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, CornerRadius::ZERO, BACKDROP);
        });

    // Entrance progress (0..1): rises once per stage. Keyed by stage so each step re-pops, echoing the
    // prototype re-running `animation:tcpop` on every `<sc-if>` swap.
    let progress = ctx.animate_bool_with_time(
        Id::new(("freeplay_leave_pop", stage_key(leave.stage))),
        true,
        POP_TIME,
    );
    let scale = POP_MIN_SCALE + (1.0 - POP_MIN_SCALE) * progress;
    let width = size::MODAL_W * scale;

    // 2) The centered modal panel, on top of the backdrop. Width is the scaled entrance width; height is
    //    measured from content. Pivoted at its center so the scale grows from the middle (like `tcpop`).
    let modal = Area::new(Id::new("freeplay_leave_modal"))
        .order(Order::Foreground)
        .fixed_pos(screen.center())
        .pivot(Align2::CENTER_CENTER)
        .constrain(true)
        .show(&ctx, |ui| {
            // Fade the whole panel in lockstep with the scale (the `opacity` half of `tcpop`).
            ui.multiply_opacity(progress);
            paint_modal(ui, width, leave.stage, name, in_hand, state.clock_ms)
        });
    resp.apply(modal.inner);
    resp
}

impl LeaveResponse {
    /// Fold a per-stage click result into the response.
    fn apply(&mut self, click: StageClick) {
        match click {
            StageClick::None => {}
            StageClick::Cancel => self.cancel = true,
            StageClick::Confirm => self.confirm = true,
            StageClick::Done => self.done = true,
        }
    }
}

/// Which button (if any) the active stage produced this frame.
#[derive(Clone, Copy)]
enum StageClick {
    None,
    Cancel,
    Confirm,
    Done,
}

/// A stable per-stage discriminator so the entrance animation restarts on each `Confirm→Processing→Done`
/// swap (egui's animation cache is keyed by `Id`, not by value).
fn stage_key(stage: LeaveStage) -> u8 {
    match stage {
        LeaveStage::Confirm => 0,
        LeaveStage::Processing => 1,
        LeaveStage::Done => 2,
    }
}

/// Lay out the framed modal body for the current stage at `width` and return any button click. The
/// frame is sized from the measured content height; content is laid out top-down inside the 24px pad.
fn paint_modal(
    ui: &mut Ui,
    width: f32,
    stage: LeaveStage,
    name: &str,
    in_hand: bool,
    clock_ms: f32,
) -> StageClick {
    let inner_w = width - MODAL_PAD * 2.0;
    // Reserve the panel area up front, then paint content into the padded inner rect. The height is the
    // stage's intrinsic content height plus top/bottom padding.
    let content_h = stage_height(ui, stage, inner_w, in_hand);
    let total = Vec2::new(width, content_h + MODAL_PAD * 2.0);
    let (frame, _) = ui.allocate_exact_size(total, Sense::hover());

    // Modal frame: `#0e0f13` + 0.10 hairline, 16px radius. The prototype's big drop shadow has no egui
    // analogue, so the border carries the panel edge on its own.
    theme::fill_rect(
        ui,
        frame,
        rad::MODAL,
        Palette::MODAL_BG,
        theme::hairline(Palette::BORDER_10),
    );

    let inner = Rect::from_min_size(
        Pos2::new(frame.left() + MODAL_PAD, frame.top() + MODAL_PAD),
        Vec2::new(inner_w, content_h),
    );
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    match stage {
        LeaveStage::Confirm => paint_confirm(&mut content, inner_w, name, in_hand),
        LeaveStage::Processing => paint_processing(&mut content, inner_w, clock_ms),
        LeaveStage::Done => paint_done(&mut content, inner_w),
    }
}

// ---------------------------------------------------------------------------
// Confirm
// ---------------------------------------------------------------------------

/// Confirm stage: left-aligned title + sub-copy, an optional amber in-hand note, then a `Stay at table`
/// / `Leave table` button row. The confirm button is a NEUTRAL SLATE button, never the emerald CTA.
fn paint_confirm(ui: &mut Ui, inner_w: f32, name: &str, in_hand: bool) -> StageClick {
    let mut click = StageClick::None;

    text_line(
        ui,
        &format!("Cash out of {name}"),
        theme::ui_font(17.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    ui.add_space(4.0);
    text_line(
        ui,
        "Leave the table and free up your seat.",
        theme::ui_font(12.5, Weight::Regular),
        SUBTLE_TEXT,
    );

    if in_hand {
        ui.add_space(14.0);
        paint_in_hand_note(ui, inner_w);
    }

    ui.add_space(20.0);
    // Two equal-width buttons with a 10px gap (prototype `display:flex;gap:10px`).
    let gap = 10.0;
    let btn_w = (inner_w - gap) / 2.0;
    let btn_h = 44.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.scope(|ui| {
            ui.set_width(btn_w);
            if widgets::neutral_button(ui, "Stay at table", btn_h).clicked() {
                click = StageClick::Cancel;
            }
        });
        ui.scope(|ui| {
            ui.set_width(btn_w);
            if widgets::slate_button(ui, "Leave table", btn_h).clicked() {
                click = StageClick::Confirm;
            }
        });
    });
    click
}

/// The amber "you're in a hand" note: a rounded `#101418` row with a small amber dot and wrapped copy.
fn paint_in_hand_note(ui: &mut Ui, inner_w: f32) {
    let dot_d = 6.0;
    let inner_gap = 9.0;
    let pad = Vec2::new(13.0, 11.0);
    let font = theme::ui_font(12.0, Weight::Regular);
    let text = "You're in a hand — you'll be seated out and cashed out the moment it ends.";

    // Wrap the copy to the row's text column so the note grows to fit two lines.
    let text_w = inner_w - pad.x * 2.0 - dot_d - inner_gap;
    let galley = ui.painter().layout(
        text.to_string(),
        font.clone(),
        Palette::TEXT_SECONDARY,
        text_w.max(1.0),
    );
    let row_h = galley.size().y.max(dot_d) + pad.y * 2.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(inner_w, row_h), Sense::hover());

    theme::fill_rect(ui, rect, 10, NOTE_BG, theme::hairline(Palette::BORDER_07));
    // Amber dot, vertically aligned to the first text line.
    let dot_center = Pos2::new(
        rect.left() + pad.x + dot_d / 2.0,
        rect.top() + pad.y + dot_d / 2.0,
    );
    theme::dot(ui, dot_center, dot_d, Palette::TIMER_AMBER);
    ui.painter().galley(
        Pos2::new(dot_center.x + dot_d / 2.0 + inner_gap, rect.top() + pad.y),
        galley,
        Palette::TEXT_SECONDARY,
    );
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

/// Processing stage: a centered spinner, `Leaving table…`, and a mono `See you next time` sub-line. No
/// on-chain settlement for free play.
fn paint_processing(ui: &mut Ui, inner_w: f32, clock_ms: f32) -> StageClick {
    ui.add_space(18.0);
    paint_spinner(ui, inner_w, 38.0, clock_ms);
    ui.add_space(18.0);
    centered_line(
        ui,
        inner_w,
        "Leaving table\u{2026}",
        theme::ui_font(16.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    ui.add_space(5.0);
    centered_line(
        ui,
        inner_w,
        "See you next time",
        theme::mono_font(12.0, Weight::Regular),
        SUBTLE_TEXT,
    );
    ui.add_space(18.0);
    StageClick::None
}

/// Paint the rotating spinner: a faint full ring with a brighter ~quarter arc rotating off `clock_ms`
/// (prototype `tcspin 0.8s linear`). The prototype's periwinkle is recast to a calm neutral tone so the
/// modal never reads as celebratory. Keeps the frame repainting while visible.
fn paint_spinner(ui: &mut Ui, inner_w: f32, diameter: f32, clock_ms: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(inner_w, diameter), Sense::hover());
    ui.ctx().request_repaint(); // keep the rotation animating each frame
    let center = rect.center();
    let radius = diameter / 2.0 - 1.5;
    let painter = ui.painter();

    // Faint track ring (prototype `rgba(...,0.2)`), neutral.
    painter.circle_stroke(center, radius, Stroke::new(3.0, Palette::BORDER_10));

    // Rotating arc: one full turn every 800ms, sampled as a short segment of points.
    let turn = (clock_ms % 800.0) / 800.0; // 0..1
    let head = turn * std::f32::consts::TAU;
    let sweep = std::f32::consts::TAU * 0.28; // ~quarter-plus arc
    let seg = 16;
    let mut pts: Vec<Pos2> = Vec::with_capacity(seg + 1);
    for i in 0..=seg {
        let a = head + sweep * (i as f32 / seg as f32);
        pts.push(Pos2::new(
            center.x + radius * a.cos(),
            center.y + radius * a.sin(),
        ));
    }
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(3.0, Palette::TEXT_SECONDARY),
    ));
}

// ---------------------------------------------------------------------------
// Done
// ---------------------------------------------------------------------------

/// Done stage: a calm neutral check-mark badge, `You left the table`, the freed-seat body, and a neutral
/// `Done` button. No tx hash for free play.
fn paint_done(ui: &mut Ui, inner_w: f32) -> StageClick {
    let mut click = StageClick::None;

    ui.add_space(10.0);
    paint_check(ui, inner_w, 42.0);
    ui.add_space(16.0);
    centered_line(
        ui,
        inner_w,
        "You left the table",
        theme::ui_font(18.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    ui.add_space(5.0);
    centered_line(
        ui,
        inner_w,
        "Your seat has been freed up.",
        theme::ui_font(12.5, Weight::Regular),
        SUBTLE_TEXT,
    );
    ui.add_space(20.0);
    if widgets::slate_button(ui, "Done", 44.0).clicked() {
        click = StageClick::Done;
    }
    click
}

/// Paint the centered done-state check badge: a 42px `#1a1d23` circle with a 0.10 hairline and a muted
/// `✓`. Deliberately neutral (no accent) so the close reads calm, not celebratory.
fn paint_check(ui: &mut Ui, inner_w: f32, diameter: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(inner_w, diameter), Sense::hover());
    let center = rect.center();
    let radius = diameter / 2.0;
    let painter = ui.painter();
    painter.circle_filled(center, radius, CHECK_BG);
    painter.circle_stroke(center, radius, Stroke::new(1.0, Palette::BORDER_10));
    // Drawn checkmark (two strokes) — the fallback font renders U+2713 as a missing-glyph box.
    let s = diameter * 0.16;
    let stroke = Stroke::new(1.8, Palette::TEXT_SECONDARY);
    let p1 = egui::pos2(center.x - s * 1.4, center.y);
    let p2 = egui::pos2(center.x - s * 0.3, center.y + s * 1.1);
    let p3 = egui::pos2(center.x + s * 1.5, center.y - s * 1.1);
    painter.line_segment([p1, p2], stroke);
    painter.line_segment([p2, p3], stroke);
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// Allocate a left-aligned single text line, advancing the top-down layout by its measured height.
fn text_line(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// Allocate a full-width row and paint `text` centered in it (the processing/done copy).
fn centered_line(ui: &mut Ui, inner_w: f32, text: &str, font: egui::FontId, color: Color32) {
    let line = line_h(ui, font.clone());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(inner_w, line), Sense::hover());
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, text, font, color);
}

// ---------------------------------------------------------------------------
// Content-height measurement (so the frame can be sized before content is laid out)
// ---------------------------------------------------------------------------

/// Measure the content height of `stage` at `inner_w` so the frame rect can be allocated first. Mirrors
/// the per-stage layout (heights + `add_space` gaps) exactly.
fn stage_height(ui: &Ui, stage: LeaveStage, inner_w: f32, in_hand: bool) -> f32 {
    match stage {
        LeaveStage::Confirm => {
            let title_h = line_h(ui, theme::ui_font(17.0, Weight::Bold));
            let sub_h = line_h(ui, theme::ui_font(12.5, Weight::Regular));
            let mut h = title_h + 4.0 + sub_h;
            if in_hand {
                h += 14.0 + in_hand_note_height(ui, inner_w);
            }
            h + 20.0 + 44.0 // button-row gap + button height
        }
        LeaveStage::Processing => {
            18.0 + 38.0 // spinner
                + 18.0
                + line_h(ui, theme::ui_font(16.0, Weight::Bold))
                + 5.0
                + line_h(ui, theme::mono_font(12.0, Weight::Regular))
                + 18.0
        }
        LeaveStage::Done => {
            10.0 + 42.0 // check badge
                + 16.0
                + line_h(ui, theme::ui_font(18.0, Weight::Bold))
                + 5.0
                + line_h(ui, theme::ui_font(12.5, Weight::Regular))
                + 20.0
                + 44.0 // Done button
        }
    }
}

/// Height of the amber in-hand note for `inner_w`: wrapped copy plus 11px top/bottom padding.
fn in_hand_note_height(ui: &Ui, inner_w: f32) -> f32 {
    let dot_d = 6.0;
    let inner_gap = 9.0;
    let pad_x = 13.0;
    let pad_y = 11.0;
    let font = theme::ui_font(12.0, Weight::Regular);
    let text = "You're in a hand — you'll be seated out and cashed out the moment it ends.";
    let text_w = (inner_w - pad_x * 2.0 - dot_d - inner_gap).max(1.0);
    let galley = ui
        .painter()
        .layout(text.to_string(), font, Palette::TEXT_SECONDARY, text_w);
    galley.size().y.max(dot_d) + pad_y * 2.0
}

/// Row height of a single line in `font`, measured by laying out a representative glyph (avoids the
/// `&mut Fonts`-only `row_height`).
fn line_h(ui: &Ui, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap("Ag".to_string(), font, Palette::TEXT_PRIMARY)
        .size()
        .y
}
