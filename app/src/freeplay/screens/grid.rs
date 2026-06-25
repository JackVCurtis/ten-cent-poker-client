//! The multi-table grid screen — the main play surface. Composes the [`titlebar`], the [`toolbar`],
//! an auto-fitting grid of [`tile`]s (1/2/3 columns by table count, 14px gaps, 16px outer pad), and
//! overlays the host [`slideover`] and the [`leave_modal`] when active. Owns the per-frame plumbing
//! of toolbar/tile/menu/slideover/modal responses back into [`AppState`] transitions.
//!
//! Keyboard shortcuts and timer animation are wired by the Integration agent; this renders the
//! screen and applies the structural transitions surfaced by its child components.
//!
//! [`titlebar`]: crate::freeplay::components::titlebar
//! [`toolbar`]: crate::freeplay::components::toolbar
//! [`tile`]: crate::freeplay::components::tile
//! [`slideover`]: crate::freeplay::components::slideover
//! [`leave_modal`]: crate::freeplay::components::leave_modal

use eframe::egui::{CornerRadius, Pos2, Rect, Ui, Vec2};

use crate::freeplay::components::{leave_modal, slideover, tile, titlebar, toolbar};
use crate::freeplay::model::{Act, AppState};
use crate::freeplay::theme::{size, Palette};

/// Max tile footprint — keeps a tile a card, not a window-filling slab, when few tables are open.
const MAX_TILE_W: f32 = 620.0;
const MAX_TILE_H: f32 = 420.0;

/// One tile's outcome this frame, tagged with its table id so the post-render pass can apply the
/// matching [`AppState`] transition after the immutable borrow used to paint the grid is released.
struct TileOutcome {
    id: u64,
    act: Option<Act>,
    toggle_menu: bool,
    close_menu: bool,
    leave: bool,
}

/// Render the whole grid screen. `pulse` is the shared eased opacity (0..=1) the toolbar/tiles use
/// for their pulsing accents, supplied by the animation driver.
pub fn render(ui: &mut Ui, state: &mut AppState, pulse: f32) {
    // Window base behind everything (`#0a0b0e`).
    let full = ui.max_rect();
    ui.painter()
        .rect_filled(full, CornerRadius::ZERO, Palette::APP_BG_GRID);

    // Top chrome: the 46px title bar (muted `My Tables` context label) and the 54px toolbar.
    titlebar::render(ui, "My Tables");

    let tb = toolbar::render(ui, state, pulse);
    if tb.new_table {
        state.host_open = true;
    }
    if tb.next_table {
        state.focus_next();
    }

    // Grid body fills the area beneath the two bars (prototype `flex:1; padding:16px; overflow:hidden`).
    let body = Rect::from_min_max(
        Pos2::new(full.left(), full.top() + size::TITLE_BAR_H + size::TOOLBAR_H),
        full.max,
    );
    render_grid(ui, body, state, pulse);

    // Overlays float above the grid in their own areas. Apply the slide-over's create/close intents.
    let so = slideover::render(ui, state);
    if so.create {
        state.create_from_host();
    } else if so.close {
        state.host_open = false;
    }

    // The leave modal drives its three-state machine via these intents (the Processing→Done timer is
    // owned by `AppState::tick`, not here).
    let lm = leave_modal::render(ui, state);
    if lm.confirm {
        state.advance_leave();
    } else if lm.cancel {
        state.cancel_leave();
    } else if lm.done {
        state.finish_leave();
    }
}

/// Lay the tiles into an equal-cell grid inside `body`: `state.grid_cols()` columns, `grid-auto-rows:1fr`
/// (every row the same height) with 14px gaps and a 16px outer pad. Tile responses are collected during
/// the immutable-borrow paint pass, then applied to `state` afterward so a tile can mutate the model
/// (act / toggle menu / leave) without aliasing the borrow that rendered it.
fn render_grid(ui: &mut Ui, body: Rect, state: &mut AppState, pulse: f32) {
    if state.tables.is_empty() {
        return;
    }

    let area = body.shrink(size::OUTER_PAD);
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return;
    }

    let n = state.tables.len();
    let cols = state.grid_cols().max(1);
    // `grid-auto-rows:1fr` over the actual tile count: enough rows to hold every tile at `cols` wide.
    let rows = n.div_ceil(cols);

    let gap = size::GRID_GAP;
    let cell_w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let cell_h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return;
    }
    // Cap each tile to a card-like size and centre it in its cell, so a single (or few) table(s)
    // don't balloon a felt oval across the whole window. With many tables the cells shrink below the
    // cap and tiles fill them as normal.
    let tile_w = cell_w.min(MAX_TILE_W);
    let tile_h = cell_h.min(MAX_TILE_H);

    // Paint pass: tiles read `state` immutably; gather what each one did.
    let mut outcomes: Vec<TileOutcome> = Vec::with_capacity(n);
    let focus_id = state.focus_id;
    for (i, table) in state.tables.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let min = Pos2::new(
            area.left() + col as f32 * (cell_w + gap),
            area.top() + row as f32 * (cell_h + gap),
        );
        let cell = Rect::from_min_size(min, Vec2::new(cell_w, cell_h));
        let tile_rect = Rect::from_center_size(cell.center(), Vec2::new(tile_w, tile_h));

        let focused = focus_id == Some(table.id);
        let r = tile::render(ui, tile_rect, state, table.id, focused, pulse);
        if r.act.is_some() || r.toggle_menu || r.close_menu || r.leave {
            outcomes.push(TileOutcome {
                id: table.id,
                act: r.act,
                toggle_menu: r.toggle_menu,
                close_menu: r.close_menu,
                leave: r.leave,
            });
        }
    }

    // Apply pass: now that the immutable grid borrow is released, fold the tile intents into `state`.
    for o in outcomes {
        if let Some(act) = o.act {
            state.act(o.id, act);
        }
        if o.leave {
            state.begin_leave(o.id);
        } else if o.toggle_menu {
            // Toggle this tile's `⋯` menu (close it if it was already the open one).
            state.open_menu_id = if state.open_menu_id == Some(o.id) {
                None
            } else {
                Some(o.id)
            };
        } else if o.close_menu {
            state.open_menu_id = None;
        }
    }
}
