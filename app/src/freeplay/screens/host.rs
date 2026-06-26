//! The standalone host control room — a two-column screen under the [`titlebar`]: the 392px
//! [`host_rail`] config on the left, and a live mini-table preview on the right that updates as the
//! config changes (game title, `Free play`, blinds + seat count, host seat marked, other seats
//! `Open seat`). Free play only: no buy-in/escrow/contract chip, no signing overlay.
//!
//! Returns whether `Create free table` was clicked, so the app can append the table and navigate.
//!
//! The right preview is its own compact felt (not the gameplay [`felt`]): it shows a config summary
//! in the oval centre and ring seats labelled `Host` / `Open seat`, matching the prototype's live
//! preview rather than a hand in progress. egui has no gradients, so the prototype's radial backdrop
//! and the oval's linear surface are approximated with layered flat fills (as the gameplay felt does).
//!
//! [`titlebar`]: crate::freeplay::components::titlebar
//! [`host_rail`]: crate::freeplay::components::host_rail
//! [`felt`]: crate::freeplay::components::felt

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
};

use crate::freeplay::components::{host_rail, titlebar};
use crate::freeplay::model::{AppState, Game, HostConfig};
use crate::freeplay::theme::{self, size, Palette, Weight};

/// What the host screen produced this frame.
#[derive(Clone, Default)]
pub struct HostScreenResponse {
    /// `Create free table` was clicked — the app starts a real host connection.
    pub create: bool,
    /// The join slide-over's `Join table` fired with this `tcpoker://` URI — the app dials it.
    pub join: Option<String>,
}

/// Left-rail interior padding (prototype `padding:24px 24px 12px`).
const RAIL_PAD: f32 = 24.0;
/// Inset of the `Live preview` pill / table-name caption from the preview corners (prototype `18px`).
const PREVIEW_INSET: f32 = 18.0;
/// Native oval size in the prototype (`width:460px; height:250px`); the preview scales to fit.
const OVAL_W: f32 = 460.0;
const OVAL_H: f32 = 250.0;
/// Ring geometry from the prototype's preview `seatPos` (`a=258 b=168 cx=230 cy=125`), in oval px.
const RING_A: f32 = 258.0;
const RING_B: f32 = 168.0;
const RING_CX: f32 = 230.0;
const RING_CY: f32 = 125.0;

/// Render the standalone host screen (titlebar + rail + live preview), editing `state.host`. When the
/// rail's `Create free table` CTA fires, this creates the table from `state.host` and routes to the
/// grid, and reports `create` so the caller can run any additional navigation.
pub fn render(ui: &mut Ui, state: &mut AppState) -> HostScreenResponse {
    let mut resp = HostScreenResponse::default();

    // Paint the host window base behind everything (the prototype's `#0b0c0f`).
    let full = ui.max_rect();
    ui.painter()
        .rect_filled(full, CornerRadius::ZERO, Palette::APP_BG_HOST);

    // --- title bar across the top, with the muted `New Table` context label ---
    titlebar::render(ui, "New Table");

    // The body fills the area beneath the title bar; split it into the 392px rail and the preview.
    let body = Rect::from_min_max(
        Pos2::new(full.left(), full.top() + size::TITLE_BAR_H),
        full.max,
    );
    let rail_rect = Rect::from_min_max(
        body.min,
        Pos2::new(body.left() + size::HOST_RAIL_W, body.bottom()),
    );
    let preview_rect = Rect::from_min_max(Pos2::new(rail_rect.right(), body.top()), body.max);

    // The single active table's live invite (present once a host connection is up), surfaced in the
    // preview so the host can copy + share the real URI.
    let invite = state.tables.first().and_then(|t| t.invite_uri.clone());
    let reach = state.tables.first().and_then(|t| t.reachability.clone());

    render_rail(ui, rail_rect, state, &mut resp);
    render_preview(
        ui,
        preview_rect,
        &state.host,
        invite.as_deref(),
        reach.as_deref(),
    );

    // The "Join a table instead" link opens the join slide-over; its CTA bubbles a URI up to the app.
    let jr = crate::freeplay::components::join::render(ui, state);
    if jr.close {
        state.join_open = false;
    }
    if let Some(uri) = jr.join {
        resp.join = Some(uri);
        state.join_open = false;
    }

    // Starting the host connection (and any screen routing) is the app's job — it owns the runtime
    // and the `TableConn`. This screen only surfaces the intents.
    resp
}

/// Paint the left config-rail column: the panel base + right-edge hairline, then the (scrollable)
/// [`host_rail`] inside the prototype's 24px padding. Surfaces the rail's `create` into `resp`.
fn render_rail(ui: &mut Ui, rect: Rect, state: &mut AppState, resp: &mut HostScreenResponse) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, Palette::PANEL_BASE);
    // Right-edge hairline separating the rail from the preview (prototype `border-right`).
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        theme::hairline(Palette::BORDER_07),
    );

    // Embed the standalone (non-compact) rail inside the padded, scrollable interior.
    let inner = rect.shrink(RAIL_PAD);
    let mut body = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    body.set_clip_rect(inner);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut body, |ui| {
            let rail = host_rail::render(ui, state, false);
            if rail.create {
                resp.create = true;
            }
            if rail.join {
                state.join_open = true;
            }
        });
}

/// Paint the right live-preview column: the radial-ish backdrop, the `Live preview` pill + the live
/// invite caption (replacing the old static table name), and the centred mini-table that restates
/// `cfg` (game title, `Free play`, blinds + seats) with host/open seats around the rim. `invite` /
/// `reach` come from the active host connection's projected table, once it is up.
fn render_preview(
    ui: &mut Ui,
    rect: Rect,
    cfg: &HostConfig,
    invite: Option<&str>,
    reach: Option<&str>,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    paint_preview_backdrop(ui, rect);

    // `Live preview` pill, top-left (accent-tint capsule with a dot + accent label).
    paint_live_pill(
        ui,
        Pos2::new(rect.left() + PREVIEW_INSET, rect.top() + PREVIEW_INSET),
    );
    // Invite caption + Copy, top-right — the REAL shareable URI once hosting (else a muted hint).
    paint_invite(ui, rect, invite, reach);

    // The oval table, centred and scaled down from its native 460×250 to fit the column with margin.
    let max_w = (rect.width() - 80.0).max(120.0);
    let max_h = (rect.height() - 120.0).max(80.0);
    let scale = (max_w / OVAL_W).min(max_h / OVAL_H).min(1.0).max(0.0);
    let oval = Rect::from_center_size(rect.center(), Vec2::new(OVAL_W * scale, OVAL_H * scale));
    paint_oval(ui, oval);
    paint_oval_center(ui, oval, cfg);
    paint_ring_seats(ui, oval, cfg.seats, scale);
}

/// Paint the top-right invite caption: while hosting, the live `tcpoker://` URI (truncated mono) with
/// a small `Copy` button beneath and any reachability warning below the oval; before hosting, a muted
/// "create a table for an invite" hint. Replaces the prototype's static `Friday Night Grind` text.
fn paint_invite(ui: &Ui, rect: Rect, invite: Option<&str>, reach: Option<&str>) {
    let right = rect.right() - PREVIEW_INSET;
    let top = rect.top() + PREVIEW_INSET + 8.0;
    match invite {
        None => {
            ui.painter().text(
                Pos2::new(right, top),
                Align2::RIGHT_CENTER,
                "create a table for an invite",
                theme::mono_font(11.0, Weight::Regular),
                Palette::TEXT_MUTED_DIM,
            );
        }
        Some(uri) => {
            // Truncated URI (keep the scheme + a tail) so a long multiaddr fits the caption.
            let shown = truncate_uri(uri);
            ui.painter().text(
                Pos2::new(right, top),
                Align2::RIGHT_CENTER,
                shown,
                theme::mono_font(11.0, Weight::Regular),
                Palette::TEXT_SECONDARY,
            );
            // `Copy` button beneath the caption.
            let font = theme::ui_font(11.0, Weight::SemiBold);
            let label = "Copy invite";
            let pad = Vec2::new(10.0, 5.0);
            let gw = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), Palette::ON_ACCENT)
                .size();
            let size = gw + pad * 2.0;
            let btn = Rect::from_min_size(Pos2::new(right - size.x, top + 14.0), size);
            let resp = ui.interact(btn, ui.id().with("host_preview_copy"), egui::Sense::click());
            let fill = if resp.hovered() {
                Color32::from_rgb(0x3a, 0xe0, 0xac)
            } else {
                Palette::ACCENT
            };
            theme::fill_rect(ui, btn, theme::rad::INPUT, fill, Stroke::NONE);
            ui.painter().text(
                btn.center(),
                Align2::CENTER_CENTER,
                label,
                font,
                Palette::ON_ACCENT,
            );
            if resp.clicked() {
                ui.ctx().copy_text(uri.to_string());
            }
            // Reachability warning (amber) bottom-left of the preview column.
            if let Some(w) = reach {
                ui.painter().text(
                    Pos2::new(rect.left() + PREVIEW_INSET, rect.bottom() - PREVIEW_INSET),
                    Align2::LEFT_BOTTOM,
                    format!("\u{26a0} {w}"),
                    theme::ui_font(11.0, Weight::Medium),
                    Palette::TIMER_AMBER,
                );
            }
        }
    }
}

/// Truncate a long `tcpoker://` URI for the caption: keep the leading scheme and a trailing slice with
/// an ellipsis in between, so the readable head + tail fit without overflowing the column.
fn truncate_uri(uri: &str) -> String {
    const HEAD: usize = 12;
    const TAIL: usize = 16;
    if uri.chars().count() <= HEAD + TAIL + 1 {
        return uri.to_string();
    }
    let head: String = uri.chars().take(HEAD).collect();
    let tail: String = uri
        .chars()
        .rev()
        .take(TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\u{2026}{tail}")
}

/// Approximate the preview's `radial-gradient(130% 120% at 50% 28%, #111418 0%, #0a0b0e 72%)`: fill
/// the column with the dark outer stop, then lay a lighter ellipse near the 50%/28% highlight.
fn paint_preview_backdrop(ui: &Ui, rect: Rect) {
    // Clip to the preview column so the highlight ellipse (larger than `rect`) can't bleed up over the
    // title bar or out past the column edges.
    let painter = ui.painter().with_clip_rect(rect);
    let outer = Color32::from_rgb(0x0a, 0x0b, 0x0e);
    painter.rect_filled(rect, CornerRadius::ZERO, outer);
    let hl_center = Pos2::new(rect.center().x, rect.top() + rect.height() * 0.28);
    painter.add(egui::Shape::ellipse_filled(
        hl_center,
        Vec2::new(rect.width() * 0.6, rect.height() * 0.6),
        Color32::from_rgb(0x11, 0x14, 0x18),
    ));
}

/// Paint the `Live preview` status pill: an accent-tint capsule (6px dot + accent-text label) whose
/// top-left corner sits at `top_left`. Mirrors the prototype's rounded accent badge.
fn paint_live_pill(ui: &Ui, top_left: Pos2) {
    let font = theme::ui_font(11.0, Weight::Medium);
    let label = "Live preview";
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), Palette::ACCENT_TEXT);
    let dot_d = 6.0;
    let inner_gap = 7.0;
    let pad = Vec2::new(11.0, 5.0);
    let size = Vec2::new(
        dot_d + inner_gap + galley.size().x + pad.x * 2.0,
        galley.size().y.max(dot_d) + pad.y * 2.0,
    );
    let rect = Rect::from_min_size(top_left, size);
    // Accent-tint fill + accent border, fully rounded into a capsule.
    let radius = CornerRadius::same((size.y / 2.0) as u8);
    ui.painter().rect(
        rect,
        radius,
        Palette::ACCENT_TINT,
        theme::hairline(Palette::ACCENT_BORDER),
        StrokeKind::Inside,
    );
    let dot_center = Pos2::new(rect.left() + pad.x + dot_d / 2.0, rect.center().y);
    theme::dot(ui, dot_center, dot_d, Palette::ACCENT);
    ui.painter().text(
        Pos2::new(dot_center.x + dot_d / 2.0 + inner_gap, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        Palette::ACCENT_TEXT,
    );
}

/// Paint the oval table surface: the `#13181c→#0e1216` vertical gradient (approximated by a base top
/// ellipse with a darker bottom ellipse nudged down) plus the faint accent rim — same approach as the
/// gameplay felt, but with the preview's slightly different top stop.
fn paint_oval(ui: &Ui, oval: Rect) {
    let painter = ui.painter();
    let radius = oval.size() / 2.0;
    let center = oval.center();

    painter.add(egui::Shape::ellipse_filled(
        center,
        radius,
        Color32::from_rgb(0x13, 0x18, 0x1c),
    ));
    let lower_center = Pos2::new(center.x, center.y + radius.y * 0.18);
    painter.add(egui::Shape::ellipse_filled(
        lower_center,
        Vec2::new(radius.x * 0.96, radius.y * 0.82),
        Palette::FELT_BOTTOM,
    ));
    // Faint accent rim (`rgba(47,214,160,0.22)`); the gameplay felt uses ~0.18, the preview a hair more.
    painter.add(egui::Shape::ellipse_stroke(
        center,
        radius,
        Stroke::new(1.0, Color32::from_rgba_premultiplied(10, 47, 35, 56)),
    ));
}

/// Paint the oval's centred config summary column: the spaced game title, the big `Free play` line,
/// and the muted `Blinds <blinds> · <n> seats` line. No buy-in amount — free play locks the stake.
fn paint_oval_center(ui: &Ui, oval: Rect, cfg: &HostConfig) {
    let painter = ui.painter();
    let cx = oval.center().x;
    let cy = oval.center().y;

    // Game title (mono 600 11px, dimmest muted) — egui has no letter-spacing so the caps carry it.
    painter.text(
        Pos2::new(cx, cy - 26.0),
        Align2::CENTER_CENTER,
        cfg.game.title(),
        theme::mono_font(11.0, Weight::SemiBold),
        Palette::TEXT_MUTED_DIM,
    );
    // `Free play` (mono 700 23px, primary) — the README's stand-in for the staked buy-in amount.
    painter.text(
        Pos2::new(cx, cy),
        Align2::CENTER_CENTER,
        "Free play",
        theme::mono_font(23.0, Weight::Bold),
        Palette::TEXT_PRIMARY,
    );
    // Blinds + seat count (Hanken 400 11px, dimmest muted). Stud is anted, not blinded — match the
    // prototype's `stakesHeader = isStud ? 'Antes' : 'Blinds'`.
    let seats_word = if cfg.seats == 1 { "seat" } else { "seats" };
    let stakes_header = if cfg.game == Game::Stud {
        "Antes"
    } else {
        "Blinds"
    };
    painter.text(
        Pos2::new(cx, cy + 24.0),
        Align2::CENTER_CENTER,
        format!(
            "{} {} \u{b7} {} {}",
            stakes_header, cfg.blinds, cfg.seats, seats_word
        ),
        theme::ui_font(11.0, Weight::Regular),
        Palette::TEXT_MUTED_DIM,
    );
}

/// Place the preview seats around the oval rim (prototype preview `seatPos`): seat 0 is the host
/// (accent pill, `YOU` avatar, `Host` / `seat 1`), the rest are dashed `Open seat` pills.
fn paint_ring_seats(ui: &Ui, oval: Rect, seats: usize, scale: f32) {
    let n = seats.max(1);
    for i in 0..n {
        let center = ring_pos(oval, i, n);
        if i == 0 {
            paint_host_seat(ui, center, scale);
        } else {
            paint_open_seat(ui, center, scale);
        }
    }
}

/// The centre of preview seat `i` of `n` on the oval rim (prototype preview `seatPos`, with the
/// native `a/b/cx/cy` mapped through the oval box and scaled to the rendered oval size).
fn ring_pos(oval: Rect, i: usize, n: usize) -> Pos2 {
    let th = std::f32::consts::FRAC_PI_2 + (i as f32) * 2.0 * std::f32::consts::PI / (n as f32);
    // Native preview coordinates are in the 460×250 oval box; convert to a 0..1 fraction of it.
    let fx = (RING_CX + RING_A * th.cos()) / OVAL_W;
    let fy = (RING_CY + RING_B * th.sin()) / OVAL_H;
    Pos2::new(
        oval.left() + fx * oval.width(),
        oval.top() + fy * oval.height(),
    )
}

/// The host seat pill: accent-tint surface + accent border, a `YOU` accent avatar, and the
/// `Host` / `seat 1` text column. Sized off `scale` so it tracks the shrunken preview oval.
fn paint_host_seat(ui: &Ui, center: Pos2, scale: f32) {
    let pad = Vec2::new(10.0, 8.0) * scale.max(0.6);
    let avatar_d = 22.0 * scale.max(0.6);
    let gap = 8.0 * scale.max(0.6);
    let name_font = theme::ui_font(11.0 * scale.max(0.7), Weight::SemiBold);
    let sub_font = theme::mono_font(9.0 * scale.max(0.7), Weight::Regular);

    let painter = ui.painter();
    let name_g =
        painter.layout_no_wrap("Host".to_string(), name_font.clone(), Palette::ACCENT_TEXT);
    let sub_g = painter.layout_no_wrap(
        "seat 1".to_string(),
        sub_font.clone(),
        Palette::ACCENT_STACK,
    );
    let col_w = name_g.size().x.max(sub_g.size().x);
    let col_h = name_g.size().y + sub_g.size().y;

    let content_w = avatar_d + gap + col_w;
    let content_h = avatar_d.max(col_h);
    let size = Vec2::new(content_w, content_h) + pad * 2.0;
    let rect = Rect::from_center_size(center, size);
    if !ui.is_rect_visible(rect) {
        return;
    }

    // Accent-tint fill + ~0.4-alpha accent border (prototype `rgba(47,214,160,0.1)` / `0.4` border).
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::rad::PANEL),
        Palette::ACCENT_TINT,
        theme::hairline(Palette::ACCENT_BORDER),
        StrokeKind::Inside,
    );

    let inner_left = rect.left() + pad.x;
    let cy = rect.center().y;
    // `YOU` accent avatar with near-black initials.
    let avatar_center = Pos2::new(inner_left + avatar_d / 2.0, cy);
    ui.painter()
        .circle_filled(avatar_center, avatar_d / 2.0, Palette::ACCENT);
    ui.painter().text(
        avatar_center,
        Align2::CENTER_CENTER,
        "YOU",
        theme::ui_font(9.0 * scale.max(0.7), Weight::Bold),
        Palette::ON_ACCENT,
    );
    // `Host` / `seat 1` text column.
    let text_left = inner_left + avatar_d + gap;
    let col_top = cy - col_h / 2.0;
    ui.painter().text(
        Pos2::new(text_left, col_top),
        Align2::LEFT_TOP,
        "Host",
        name_font,
        Color32::from_rgb(0xd4, 0xf5, 0xe8),
    );
    ui.painter().text(
        Pos2::new(text_left, col_top + name_g.size().y),
        Align2::LEFT_TOP,
        "seat 1",
        sub_font,
        Palette::ACCENT_STACK,
    );
}

/// An `Open seat` pill: surface fill + dashed hairline border + muted label, centred on `center`.
fn paint_open_seat(ui: &Ui, center: Pos2, scale: f32) {
    let font = theme::ui_font(11.0 * scale.max(0.7), Weight::Medium);
    let galley =
        ui.painter()
            .layout_no_wrap("Open seat".to_string(), font.clone(), Palette::TEXT_MUTED);
    let pad = Vec2::new(10.0, 9.0) * scale.max(0.6);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_center_size(center, size);
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(theme::rad::PANEL),
        Palette::SURFACE,
    );
    paint_dashed_border(ui, rect, theme::rad::PANEL as f32, Palette::BORDER_DASH);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Open seat",
        font,
        Palette::TEXT_MUTED,
    );
}

/// Approximate a dashed rounded-rect border by dashing the four straight edges (inset by the corner
/// radius) and stroking short solid arcs at the corners — egui has no native dashed rounded stroke.
/// Mirrors the seat component's `paint_dashed_border` for the preview's `Open seat` pills.
fn paint_dashed_border(ui: &Ui, rect: Rect, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let (dash, gap) = (3.0, 3.0);
    let painter = ui.painter();

    let top = [
        Pos2::new(rect.left() + r, rect.top()),
        Pos2::new(rect.right() - r, rect.top()),
    ];
    let bottom = [
        Pos2::new(rect.left() + r, rect.bottom()),
        Pos2::new(rect.right() - r, rect.bottom()),
    ];
    let left = [
        Pos2::new(rect.left(), rect.top() + r),
        Pos2::new(rect.left(), rect.bottom() - r),
    ];
    let right = [
        Pos2::new(rect.right(), rect.top() + r),
        Pos2::new(rect.right(), rect.bottom() - r),
    ];
    for seg in [top, bottom, left, right] {
        painter.add(egui::Shape::dashed_line(&seg, stroke, dash, gap));
    }

    let half_pi = std::f32::consts::FRAC_PI_2;
    let pi = std::f32::consts::PI;
    let corners = [
        (Pos2::new(rect.left() + r, rect.top() + r), pi, 1.5 * pi),
        (
            Pos2::new(rect.right() - r, rect.top() + r),
            1.5 * pi,
            2.0 * pi,
        ),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0, half_pi),
        (Pos2::new(rect.left() + r, rect.bottom() - r), half_pi, pi),
    ];
    for (c, a0, a1) in corners {
        let segs = 4;
        let pts: Vec<Pos2> = (0..=segs)
            .map(|k| {
                let a = a0 + (a1 - a0) * (k as f32 / segs as f32);
                Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        painter.add(egui::Shape::line(pts, stroke));
    }
}
