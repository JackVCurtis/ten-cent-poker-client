//! The tile `⋯` dropdown menu and its outside-click catcher. Items: `Hand history`, `Table
//! settings`, a divider, then the muted `Leave table` row (with a small `↗`). A full-window
//! transparent catcher behind the menu closes it on any outside click.
//!
//! Rendered as an overlay anchored to the tile's top-right; the parent supplies the tile rect.
//! Both layers are `egui::Area`s in `Order::Foreground` so the dropdown floats over sibling tiles
//! (mirroring the prototype's `z-index:30/31`), since direct painting into the tile `Ui` would be
//! occluded by tiles drawn later in the grid.

use eframe::egui::{
    self, Align2, Area, Color32, CornerRadius, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};

use crate::freeplay::theme::{self, rad, Palette, Weight};

/// What the user selected in the menu this frame; the tile/grid reacts.
#[derive(Clone, Copy, Default)]
pub struct MenuResponse {
    /// The outside-click catcher (or a selection) requests the menu close.
    pub close: bool,
    /// `Leave table` was chosen — open the leave-table modal for this tile.
    pub leave: bool,
}

/// Dropdown width (prototype `width:198px`).
const MENU_W: f32 = 198.0;
/// Outer padding inside the dropdown frame (prototype `padding:6px`).
const PAD: f32 = 6.0;
/// Per-item horizontal padding (prototype `padding:9px 11px`; the 9px vertical is baked into
/// [`ROW_H`]).
const ITEM_PAD_X: f32 = 11.0;
/// Item corner radius (prototype `border-radius:8px`).
const ITEM_RAD: u8 = 8;
/// Divider inset + vertical margin (prototype `margin:5px 6px`).
const DIV_INSET: f32 = 6.0;
const DIV_MARGIN: f32 = 5.0;
/// One menu row's height: the 12.5px label's ~1.2 line height (~15px) plus 9px padding each side.
const ROW_H: f32 = 33.0;
/// Anchor offset from the tile's top-right (prototype `top:42px; right:10px`).
const ANCHOR_DOWN: f32 = theme::size::TILE_HEADER_H;
const ANCHOR_RIGHT: f32 = 10.0;

/// The neutral `Leave table` row text (prototype `#c6cad1`) — brighter than the menu's secondary
/// items but deliberately NOT the accent CTA color.
const LEAVE_TEXT: Color32 = Color32::from_rgb(0xc6, 0xca, 0xd1);

/// Render the dropdown anchored to the top-right of `tile_rect`, plus its full-window outside-click
/// catcher. Returns which action (if any) was taken.
pub fn render(ui: &mut Ui, tile_rect: Rect) -> MenuResponse {
    let mut resp = MenuResponse::default();
    let ctx = ui.ctx().clone();
    // Scope the Area ids by tile so concurrently-open menus (there is only ever one in practice)
    // never collide; `tile_rect.min` is stable per tile within a frame.
    let key = (tile_rect.min.x.to_bits(), tile_rect.min.y.to_bits());

    // 1) Full-window transparent catcher (prototype `inset:0; z-index:30`). A click anywhere closes
    //    the menu; the dropdown sits above it and swallows clicks on the items themselves.
    let win = ctx.content_rect();
    let catcher = Area::new(Id::new(("freeplay_menu_catcher", key)))
        .order(Order::Foreground)
        .fixed_pos(win.min)
        .interactable(true)
        .sense(Sense::click())
        .show(&ctx, |ui| ui.allocate_exact_size(win.size(), Sense::click()));
    if catcher.inner.1.clicked() {
        resp.close = true;
    }

    // 2) The framed dropdown (prototype `z-index:31`), anchored so its top-right corner sits 10px in
    //    from the tile's right edge and just below the 42px header.
    let anchor = Pos2::new(tile_rect.right() - ANCHOR_RIGHT, tile_rect.top() + ANCHOR_DOWN);
    let dropdown = Area::new(Id::new(("freeplay_menu", key)))
        .order(Order::Foreground)
        .fixed_pos(anchor)
        .pivot(Align2::RIGHT_TOP)
        .constrain(true)
        .show(&ctx, |ui| paint_dropdown(ui));
    if let Some(choice) = dropdown.inner {
        match choice {
            // `Hand history` / `Table settings` are inert in the free-play scope, but selecting any
            // item dismisses the menu like the prototype.
            Choice::Inert => resp.close = true,
            Choice::Leave => {
                resp.leave = true;
                resp.close = true;
            }
        }
    }
    resp
}

/// Which row, if any, was clicked this frame.
enum Choice {
    /// `Hand history` or `Table settings` — closes the menu with no further effect.
    Inert,
    /// `Leave table` — begin the leave flow.
    Leave,
}

/// Paint the framed dropdown body and lay out its rows; returns any clicked row.
fn paint_dropdown(ui: &mut Ui) -> Option<Choice> {
    // Size the frame: width is fixed; height is the sum of three rows + a divider band + padding.
    let row_h = ROW_H;
    let div_h = DIV_MARGIN * 2.0 + 1.0;
    let inner_w = MENU_W - PAD * 2.0;
    let total_h = PAD * 2.0 + row_h * 3.0 + div_h;

    let (frame, _) = ui.allocate_exact_size(Vec2::new(MENU_W, total_h), Sense::hover());

    // Frame: menu bg + 0.10 hairline, panel radius. (No gradient/shadow in egui — the prototype's
    // `box-shadow` drop is approximated by the frame border alone.)
    theme::fill_rect(ui, frame, rad::PANEL, Palette::MENU_BG, theme::hairline(Palette::BORDER_10));

    let mut choice = None;
    let mut y = frame.top() + PAD;
    let x = frame.left() + PAD;

    // Two secondary rows.
    if menu_item(ui, item_rect(x, y, inner_w, row_h), "Hand history", Palette::TEXT_SECONDARY, None) {
        choice = Some(Choice::Inert);
    }
    y += row_h;
    if menu_item(ui, item_rect(x, y, inner_w, row_h), "Table settings", Palette::TEXT_SECONDARY, None) {
        choice = Some(Choice::Inert);
    }
    y += row_h;

    // Divider (prototype 1px `rgba(255,255,255,0.07)`, inset 6px, 5px margin top/bottom).
    let div_y = y + DIV_MARGIN + 0.5;
    ui.painter().hline(
        (frame.left() + DIV_INSET)..=(frame.right() - DIV_INSET),
        div_y,
        theme::hairline(Palette::BORDER_07),
    );
    y += div_h;

    // `Leave table` — neutral brighter text. NOT highlighted/promoted. (Trailing ↗ dropped: the
    // fallback font renders it as a missing-glyph box; restore once real fonts are bundled.)
    if menu_item(ui, item_rect(x, y, inner_w, row_h), "Leave table", LEAVE_TEXT, None) {
        choice = Some(Choice::Leave);
    }
    choice
}

/// The shared item label font (Hanken Grotesk 500 / 12.5px in the prototype).
fn item_font() -> egui::FontId {
    theme::ui_font(12.5, Weight::Medium)
}

/// Build an item rect at `(x, y)` with the inner content width and row height.
fn item_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
}

/// Paint one menu row and report whether it was clicked. Hover lifts a subtle surface fill behind
/// the row (the prototype shows a pointer cursor; egui has no CSS `:hover` so we tint instead).
/// `trailing`, when set, draws a muted glyph (the `↗`) right-aligned within the row.
fn menu_item(ui: &mut Ui, rect: Rect, label: &str, color: Color32, trailing: Option<&str>) -> bool {
    let resp = ui.interact(rect, ui.id().with(("item", label)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(ITEM_RAD), Palette::SURFACE);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(ITEM_RAD),
            Stroke::new(1.0, Palette::BORDER_05),
            StrokeKind::Inside,
        );
    }
    let text_left = Pos2::new(rect.left() + ITEM_PAD_X, rect.center().y);
    ui.painter()
        .text(text_left, Align2::LEFT_CENTER, label, item_font(), color);
    if let Some(glyph) = trailing {
        let text_right = Pos2::new(rect.right() - ITEM_PAD_X, rect.center().y);
        ui.painter().text(
            text_right,
            Align2::RIGHT_CENTER,
            glyph,
            theme::ui_font(12.0, Weight::Regular),
            Palette::TEXT_MUTED,
        );
    }
    resp.clicked()
}
