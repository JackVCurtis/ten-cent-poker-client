//! One table tile: the rounded `#0d0f12` card with its your-turn / focused ring, a 42px header
//! (name, game badge, muted `FREE` badge, blinds label, optional accent `FOCUS` tag, `⋯` menu
//! button), the [`super::felt`] body, and the [`super::action_bar`] bottom strip. When this tile's
//! menu is open it overlays [`super::menu`]. The tile is the unit the grid lays out.

use eframe::egui::{self, pos2, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};

use crate::freeplay::components::{action_bar, felt, menu};
use crate::freeplay::model::{Act, AppState, Table};
use crate::freeplay::theme::{self, rad, size, Palette, Weight};

/// What happened on a tile this frame; the grid screen applies the state transition.
#[derive(Clone, Copy, Default)]
pub struct TileResponse {
    /// The player took an action on this table.
    pub act: Option<Act>,
    /// The `⋯` menu button was clicked (toggle this tile's menu).
    pub toggle_menu: bool,
    /// The menu requested to close (outside click or a selection).
    pub close_menu: bool,
    /// `Leave table` was chosen from the menu.
    pub leave: bool,
}

/// Header content padding (prototype `padding:0 14px`).
const HEADER_PAD_X: f32 = 14.0;
/// Gap between the name-cluster items (prototype `gap:9px`).
const HEADER_LEFT_GAP: f32 = 9.0;
/// Gap between the right-cluster items — blinds / FOCUS / `⋯` (prototype `gap:10px`).
const HEADER_RIGHT_GAP: f32 = 10.0;
/// Footprint of the three-dot menu button (drawn as dots, not a glyph, so the fallback font can't
/// turn it into a missing-glyph box).
const MENU_DOTS_W: f32 = 16.0;
/// The bottom-strip content height: the 40px hero cards plus `padding:11px` top/bottom.
const STRIP_CONTENT_H: f32 = 62.0;
/// Timer-rail height added to the strip on your turn (prototype `height:3px`).
const STRIP_TIMER_H: f32 = 3.0;

/// Render the tile for table `id` into `rect`. `focused` marks the keyboard-focused your-turn tile
/// (brighter ring + `FOCUS` tag). `pulse` is the shared eased opacity for glow/waiting dots. Reads
/// `state` (table data + `open_menu_id`); returns what the user did for the grid to apply.
pub fn render(ui: &mut Ui, rect: Rect, state: &AppState, id: u64, focused: bool, pulse: f32) -> TileResponse {
    let mut resp = TileResponse::default();
    let Some(table) = state.table(id) else { return resp };

    // FOCUS tag only when this is genuinely a focused your-turn tile (the prototype's
    // `focused = t.id===focusId && t.yourTurn`).
    let focused = focused && table.your_turn;

    paint_frame(ui, rect, table, focused, pulse);

    // Carve the tile top-to-bottom: 42px header, the felt body (flex middle), the bottom strip.
    let header_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), size::TILE_HEADER_H));
    let strip_h = STRIP_CONTENT_H + if table.your_turn { STRIP_TIMER_H } else { 0.0 };
    let strip_rect = Rect::from_min_max(
        pos2(rect.min.x, rect.max.y - strip_h),
        rect.max,
    );
    let felt_rect = Rect::from_min_max(
        pos2(rect.min.x, header_rect.max.y),
        pos2(rect.max.x, strip_rect.min.y),
    );

    // HEADER — name, badges, blinds, optional FOCUS tag, and the `⋯` menu button.
    if header(ui, header_rect, table, focused) {
        resp.toggle_menu = true;
    }
    // Header bottom hairline (prototype `border-bottom:1px solid rgba(255,255,255,0.05)`).
    ui.painter().hline(
        (rect.min.x)..=(rect.max.x),
        header_rect.max.y - 0.5,
        theme::hairline(Palette::BORDER_05),
    );

    // FELT — the oval table, board + pot, and opponent seats.
    felt::render(ui, felt_rect, table, pulse);

    // BOTTOM STRIP — hero cards + actions (your turn) or the muted waiting line.
    if let Some(act) = action_bar::render(ui, strip_rect, table, pulse) {
        resp.act = Some(act);
    }

    // MENU OVERLAY — only when this tile owns the open menu; floats above sibling tiles.
    if state.open_menu_id == Some(id) {
        let m = menu::render(ui, rect);
        if m.close {
            resp.close_menu = true;
        }
        if m.leave {
            resp.leave = true;
        }
    }

    resp
}

/// Paint the tile container: the optional your-turn / focused glow, then the `#0d0f12` base fill, then
/// the border whose color/weight escalates with the your-turn → focused states. egui can't blur, so
/// the prototype's `box-shadow` glow is approximated with [`theme::soft_ring`] layered strokes.
fn paint_frame(ui: &Ui, rect: Rect, table: &Table, focused: bool, pulse: f32) {
    // Glow under the fill so it reads as a soft halo around the tile edge. Focused tiles glow
    // brighter/wider (prototype `0 0 44px -8px rgba(47,214,160,0.5)` vs `0 0 32px -8px …0.3`).
    if table.your_turn {
        let (base_a, layers) = if focused { (130.0, 6) } else { (80.0, 5) };
        let glow_a = (base_a * pulse.clamp(0.0, 1.0)) as u8;
        let glow = Color32::from_rgba_premultiplied(
            Palette::ACCENT.r(),
            Palette::ACCENT.g(),
            Palette::ACCENT.b(),
            glow_a,
        );
        theme::soft_ring(ui, rect, rad::TILE, glow, layers);
    }

    // Base tile fill.
    ui.painter()
        .rect_filled(rect, CornerRadius::same(rad::TILE), Palette::TILE_BASE);

    // Border: hairline by default; accent at ~0.5 alpha on your turn; a thicker solid emerald ring
    // when focused (prototype `1.5px solid #2fd6a0`).
    let (stroke_w, border) = if focused {
        (1.5, Palette::ACCENT)
    } else if table.your_turn {
        (1.0, Color32::from_rgba_premultiplied(0x17, 0x6b, 0x50, 128)) // rgba(47,214,160,0.5)
    } else {
        (1.0, Palette::BORDER_07)
    };
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(rad::TILE),
        Stroke::new(stroke_w, border),
        StrokeKind::Inside,
    );

    // Focused tiles also carry a faint 1px inset accent ring just inside the border (prototype
    // `box-shadow:0 0 0 1px rgba(47,214,160,0.35)`), painted as a second inset stroke.
    if focused {
        ui.painter().rect_stroke(
            rect.shrink(1.5),
            CornerRadius::same(rad::TILE),
            Stroke::new(1.0, Color32::from_rgba_premultiplied(0x10, 0x4b, 0x38, 89)),
            StrokeKind::Inside,
        );
    }
}

/// Paint the 42px header row and lay out its two clusters. Returns whether the `⋯` menu button was
/// clicked this frame. The name ellipsizes against the available width so the badges/blinds always
/// fit, matching the prototype's `min-width:0; overflow:hidden; text-overflow:ellipsis`.
fn header(ui: &mut Ui, rect: Rect, table: &Table, focused: bool) -> bool {
    let inner = Rect::from_min_max(
        pos2(rect.min.x + HEADER_PAD_X, rect.min.y),
        pos2(rect.max.x - HEADER_PAD_X, rect.max.y),
    );

    // Right cluster first (laid out right-to-left) so we know how much width it claims; the name
    // cluster then ellipsizes into whatever space is left.
    let menu_clicked = right_cluster(ui, inner, table, focused);

    // The right cluster reports its consumed width via the shared id; recompute it the same way so
    // the left cluster's clip never overlaps the blinds/FOCUS/`⋯`.
    let right_w = right_cluster_width(ui, table, focused);
    let left_max_x = inner.max.x - right_w - HEADER_RIGHT_GAP;
    let left = Rect::from_min_max(inner.min, pos2(left_max_x.max(inner.min.x), inner.max.y));
    left_cluster(ui, left, table);

    menu_clicked
}

/// Paint the left header cluster (name, game badge, FREE badge) inside `rect`, left-aligned and
/// vertically centred. The name ellipsizes (hard clip) so it never overruns the badges.
fn left_cluster(ui: &mut Ui, rect: Rect, table: &Table) {
    // Reserve the badge widths first by measuring them, so the name clip stops before them.
    let name_font = theme::ui_font(13.0, Weight::SemiBold);
    let game = table.game.badge();
    let game_w = badge_width(ui, game, true);
    let free_w = badge_width(ui, "FREE", false);
    let badges_w = game_w + HEADER_LEFT_GAP + free_w;
    let name_max_w = (rect.width() - badges_w - HEADER_LEFT_GAP).max(0.0);

    // Name (ellipsized), painted directly so the clip is exact.
    let name_anchor = pos2(rect.min.x, rect.center().y);
    paint_clipped_name(ui, name_anchor, name_max_w, &table.name, name_font, Palette::TEXT_PRIMARY_DIM);

    // Badges sit just right of the name's reserved width. Place them via a left-to-right child Ui
    // anchored at the badge start so each widget self-sizes (badges allocate in-flow). The badges are
    // ~17px tall; centre them against the 42px header by nudging the child Ui down.
    let badge_x = rect.min.x + name_max_w + HEADER_LEFT_GAP;
    let badge_h = 17.0;
    let badge_rect = Rect::from_min_max(
        pos2(badge_x, rect.center().y - badge_h / 2.0),
        pos2(rect.max.x, rect.center().y + badge_h / 2.0),
    );
    let mut badges = ui.new_child(
        UiBuilder::new()
            .max_rect(badge_rect)
            .layout(Layout::left_to_right(egui::Align::Center)),
    );
    badges.spacing_mut().item_spacing = Vec2::new(HEADER_LEFT_GAP, 0.0);
    crate::freeplay::widgets::game_badge(&mut badges, game);
    crate::freeplay::widgets::free_badge(&mut badges);
}

/// Paint the right header cluster (blinds label, optional accent FOCUS tag, the `⋯` menu button),
/// right-aligned and vertically centred. Returns whether the `⋯` button was clicked.
fn right_cluster(ui: &mut Ui, rect: Rect, table: &Table, focused: bool) -> bool {
    let mut clicked = false;
    let cy = rect.center().y;
    let mut x = rect.max.x;

    // `⋯` menu button (far right), drawn as three dots so the fallback font can't render a tofu box.
    let menu_hit = Rect::from_min_max(pos2(x - MENU_DOTS_W, rect.min.y), pos2(x, rect.max.y));
    let menu_resp = ui.interact(menu_hit, ui.id().with(("tile_menu_btn", table.id)), Sense::click());
    let menu_color = if menu_resp.hovered() { Palette::TEXT_SECONDARY } else { Color32::from_rgb(0x56, 0x5a, 0x62) };
    // Three dots right-aligned to `x`, 5px apart, vertically centred.
    for k in 0..3 {
        ui.painter().circle_filled(pos2(x - 2.0 - k as f32 * 5.0, cy), 1.4, menu_color);
    }
    if menu_resp.clicked() {
        clicked = true;
    }
    x -= MENU_DOTS_W + HEADER_RIGHT_GAP;

    // FOCUS tag (accent) — only on the focused your-turn tile.
    if focused {
        let (label, fill, text) = ("FOCUS", Palette::ACCENT, Palette::ON_ACCENT);
        let font = theme::ui_font(9.0, Weight::SemiBold);
        let pad = Vec2::new(7.0, 2.0);
        let lw = ui.painter().layout_no_wrap(label.to_string(), font.clone(), text).size();
        let tag = Rect::from_min_size(pos2(x - lw.x - pad.x * 2.0, cy - (lw.y + pad.y * 2.0) / 2.0), lw + pad * 2.0);
        theme::fill_rect(ui, tag, rad::BADGE, fill, Stroke::NONE);
        ui.painter().text(tag.center(), Align2::CENTER_CENTER, label, font, text);
        x -= lw.x + pad.x * 2.0 + HEADER_RIGHT_GAP;
    }

    // Blinds label (mono, dim).
    let blinds_font = theme::mono_font(10.0, Weight::Regular);
    ui.painter().text(pos2(x, cy), Align2::RIGHT_CENTER, &table.blinds, blinds_font, Palette::TEXT_MUTED_DIM);

    clicked
}

/// Width the right header cluster will occupy, computed the same way as [`right_cluster`] paints it so
/// the left cluster can be clipped to the remaining space without overlap.
fn right_cluster_width(ui: &Ui, table: &Table, focused: bool) -> f32 {
    let painter = ui.painter();
    let blinds_w = painter
        .layout_no_wrap(table.blinds.clone(), theme::mono_font(10.0, Weight::Regular), Palette::TEXT_MUTED_DIM)
        .size()
        .x;
    let mut w = MENU_DOTS_W + HEADER_RIGHT_GAP + blinds_w;
    if focused {
        let focus_w = painter
            .layout_no_wrap("FOCUS".to_string(), theme::ui_font(9.0, Weight::SemiBold), Palette::ON_ACCENT)
            .size()
            .x
            + 14.0; // 7px horizontal padding each side
        w += focus_w + HEADER_RIGHT_GAP;
    }
    w
}

/// Measure a badge's footprint (label + the shared 6px horizontal padding) so the left cluster can
/// reserve room for the game/FREE badges before clipping the name. `mono` matches the badge's font.
fn badge_width(ui: &Ui, label: &str, mono: bool) -> f32 {
    let font = if mono {
        theme::mono_font(9.5, Weight::SemiBold)
    } else {
        theme::ui_font(9.0, Weight::SemiBold)
    };
    ui.painter().layout_no_wrap(label.to_string(), font, Palette::TEXT_SECONDARY).size().x + 12.0
}

/// Paint a table name left-aligned at the vertical centre `anchor`, truncated with a trailing `…` to
/// `max_w` — the prototype's `text-overflow:ellipsis`. Uses a single-row [`LayoutJob`] with an overflow
/// character so over-long names degrade cleanly instead of being hard-cut mid-glyph.
fn paint_clipped_name(ui: &Ui, anchor: egui::Pos2, max_w: f32, name: &str, font: egui::FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(
        name.to_string(),
        egui::text::TextFormat { font_id: font, color, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let top_left = pos2(anchor.x, anchor.y - galley.size().y / 2.0);
    ui.painter().galley(top_left, galley, color);
}
