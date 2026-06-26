//! The multi-table grid screen — the main play surface. Composes the [`titlebar`], the [`toolbar`],
//! a lifecycle banner (connecting / waiting-with-invite / game-over / error), an auto-fitting grid of
//! [`tile`]s (1/2/3 columns by table count, 14px gaps, 16px outer pad), and overlays the host
//! [`slideover`], the [`join`] slide-over, and the [`leave_modal`] when active.
//!
//! ONE active table at a time: the grid is a shell, but only a single live host/guest connection
//! exists. The screen reads `state` and RETURNS intents ([`GridResponse`]) the app applies against
//! the connection it owns — the per-tile action routes to the live driver, not a local simulation.
//!
//! [`titlebar`]: crate::freeplay::components::titlebar
//! [`toolbar`]: crate::freeplay::components::toolbar
//! [`tile`]: crate::freeplay::components::tile
//! [`slideover`]: crate::freeplay::components::slideover
//! [`join`]: crate::freeplay::components::join
//! [`leave_modal`]: crate::freeplay::components::leave_modal

use eframe::egui::{
    Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Ui, UiBuilder, Vec2,
};

use crate::freeplay::components::{join, leave_modal, slideover, tile, titlebar, toolbar};
use crate::freeplay::model::{Act, AppState};
use crate::freeplay::theme::{self, size, Palette, Weight};
use crate::gui_state::Conn;

/// Max tile footprint — keeps a tile a card, not a window-filling slab, when few tables are open.
const MAX_TILE_W: f32 = 620.0;
const MAX_TILE_H: f32 = 420.0;

/// What the grid surfaces to the app this frame. The app owns the tokio runtime and the single live
/// [`TableConn`](crate::freeplay::conn::TableConn), so connection-affecting intents flow up here.
#[derive(Default)]
pub struct GridResponse {
    /// The host slide-over's `Create free table` fired — start a real host.
    pub host_create: bool,
    /// The join slide-over's `Join table` fired with this `tcpoker://` URI — dial it.
    pub join: Option<String>,
    /// The live table's action bar / shortcut chose an action — route it through the connection.
    pub act: Option<Act>,
    /// The action bar's sizing stepper requested this signed change to the bet/raise target (chips).
    pub size_delta: i64,
    /// The leave flow finished (`Done`) — abort the connection and clear the table.
    pub leave_finished: bool,
}

/// One tile's outcome this frame, tagged with its table id so the post-render pass can apply the
/// matching transition after the immutable borrow used to paint the grid is released.
struct TileOutcome {
    id: u64,
    act: Option<Act>,
    size_delta: i64,
    toggle_menu: bool,
    close_menu: bool,
    leave: bool,
}

/// Render the whole grid screen. `pulse` is the shared eased opacity (0..=1) the toolbar/tiles use for
/// their pulsing accents; `lifecycle` is the live connection's [`Conn`] state for the status banner.
pub fn render(ui: &mut Ui, state: &mut AppState, pulse: f32, lifecycle: &Conn) -> GridResponse {
    let mut resp = GridResponse::default();

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
    if tb.join_table {
        state.join_open = true;
    }
    if tb.next_table {
        state.focus_next();
    }

    // Body beneath the two bars. A lifecycle banner (if any) takes the top; the grid fills the rest.
    let body = Rect::from_min_max(
        Pos2::new(
            full.left(),
            full.top() + size::TITLE_BAR_H + size::TOOLBAR_H,
        ),
        full.max,
    );
    let banner_h = render_banner(ui, body, lifecycle, state);
    let grid_body = Rect::from_min_max(Pos2::new(body.left(), body.top() + banner_h), body.max);
    let (act, size_delta) = render_grid(ui, grid_body, state, pulse);
    resp.act = act;
    resp.size_delta = size_delta;

    // Host slide-over: `Create free table` now starts a REAL host (the app handles it); close dismisses.
    let so = slideover::render(ui, state);
    if so.create {
        resp.host_create = true;
        state.host_open = false;
    } else if so.close {
        state.host_open = false;
    }

    // Join slide-over: a valid pasted invite bubbles up for the app to dial.
    let jr = join::render(ui, state);
    if jr.close {
        state.join_open = false;
    }
    if let Some(uri) = jr.join {
        resp.join = Some(uri);
        state.join_open = false;
    }

    // Leave modal: Confirm/Cancel drive the local state machine; Done bubbles up so the app can abort
    // the live connection before the tile is removed (the Processing→Done timer is `AppState::tick`).
    let lm = leave_modal::render(ui, state);
    if lm.confirm {
        state.advance_leave();
    } else if lm.cancel {
        state.cancel_leave();
    } else if lm.done {
        resp.leave_finished = true;
    }

    resp
}

/// Paint the lifecycle status banner across the top of `body` and return the height it consumed (0 when
/// there is nothing to show — `Idle`/`Playing`). `Waiting` surfaces the shareable invite + a `Copy`
/// button (and any reachability warning) read from the projected table.
fn render_banner(ui: &mut Ui, body: Rect, lifecycle: &Conn, state: &AppState) -> f32 {
    let invite = state.tables.first().and_then(|t| t.invite_uri.as_deref());
    let reach = state.tables.first().and_then(|t| t.reachability.as_deref());
    match lifecycle {
        Conn::Connecting => simple_banner(
            ui,
            body,
            "Connecting to host\u{2026}",
            Palette::TEXT_SECONDARY,
            true,
        ),
        Conn::Waiting => waiting_banner(ui, body, invite, reach),
        Conn::GameOver => simple_banner(
            ui,
            body,
            "Game over \u{2014} the host stopped dealing.",
            Palette::TEXT_MUTED,
            false,
        ),
        Conn::Error(e) => simple_banner(
            ui,
            body,
            &format!("Table ended: {e}"),
            Palette::TIMER_RED,
            false,
        ),
        // Idle / Playing: no banner — live state speaks for itself.
        _ => 0.0,
    }
}

/// A one-line status band (optional spinner + message). Returns its height.
fn simple_banner(ui: &mut Ui, body: Rect, text: &str, color: Color32, spinner: bool) -> f32 {
    let h = 38.0;
    let rect = Rect::from_min_size(body.min, Vec2::new(body.width(), h));
    paint_band(ui, rect);
    let inner = rect.shrink2(Vec2::new(size::OUTER_PAD, 0.0));
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
    if spinner {
        cui.spinner();
    }
    cui.label(
        RichText::new(text)
            .font(theme::ui_font(12.5, Weight::Medium))
            .color(color),
    );
    h
}

/// The host's "waiting for players" band: the status line, the shareable invite + `Copy`, and any
/// reachability warning. Returns its height.
fn waiting_banner(ui: &mut Ui, body: Rect, invite: Option<&str>, reach: Option<&str>) -> f32 {
    let h = if reach.is_some() { 96.0 } else { 74.0 };
    let rect = Rect::from_min_size(body.min, Vec2::new(body.width(), h));
    paint_band(ui, rect);
    let inner = rect.shrink2(Vec2::new(size::OUTER_PAD, 10.0));
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    cui.label(
        RichText::new("Waiting for players to join\u{2026}")
            .font(theme::ui_font(13.0, Weight::SemiBold))
            .color(Palette::TEXT_PRIMARY),
    );
    cui.add_space(4.0);
    match invite {
        Some(uri) => {
            cui.horizontal(|ui| {
                ui.label(
                    RichText::new(short_uri(uri))
                        .font(theme::mono_font(11.0, Weight::Regular))
                        .color(Palette::TEXT_SECONDARY),
                );
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(uri.to_string());
                }
            });
        }
        None => {
            cui.label(
                RichText::new("(assembling table invite\u{2026})")
                    .font(theme::ui_font(12.0, Weight::Regular))
                    .color(Palette::TEXT_MUTED),
            );
        }
    }
    if let Some(w) = reach {
        cui.add_space(4.0);
        cui.label(
            RichText::new(format!("\u{26a0} {w}"))
                .font(theme::ui_font(11.5, Weight::Medium))
                .color(Palette::TIMER_AMBER),
        );
    }
    h
}

/// Paint a banner band: the toolbar chrome fill + a bottom hairline.
fn paint_band(ui: &Ui, rect: Rect) {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, Palette::TOOLBAR);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        theme::hairline(Palette::BORDER_05),
    );
}

/// Truncate a long `tcpoker://` URI for a single-line caption (keep the head + tail).
fn short_uri(uri: &str) -> String {
    const HEAD: usize = 22;
    const TAIL: usize = 22;
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

/// Lay the tiles into an equal-cell grid inside `body` and return the action / sizing intents collected
/// from the tiles this frame (one active table, so at most one action). Menu/leave toggles are applied
/// in place; the act + sizing step bubble up to the app to route through the live connection.
fn render_grid(ui: &mut Ui, body: Rect, state: &mut AppState, pulse: f32) -> (Option<Act>, i64) {
    if state.tables.is_empty() {
        return (None, 0);
    }

    let area = body.shrink(size::OUTER_PAD);
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return (None, 0);
    }

    let n = state.tables.len();
    let cols = state.grid_cols().max(1);
    let rows = n.div_ceil(cols);

    let gap = size::GRID_GAP;
    let cell_w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let cell_h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return (None, 0);
    }
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
        if r.act.is_some() || r.size_delta != 0 || r.toggle_menu || r.close_menu || r.leave {
            outcomes.push(TileOutcome {
                id: table.id,
                act: r.act,
                size_delta: r.size_delta,
                toggle_menu: r.toggle_menu,
                close_menu: r.close_menu,
                leave: r.leave,
            });
        }
    }

    // Apply pass: menu/leave toggles fold into `state`; act + sizing bubble up to the app.
    let mut act_out = None;
    let mut size_out = 0i64;
    for o in outcomes {
        if o.act.is_some() {
            act_out = o.act;
        }
        size_out += o.size_delta;
        if o.leave {
            state.begin_leave(o.id);
        } else if o.toggle_menu {
            state.open_menu_id = if state.open_menu_id == Some(o.id) {
                None
            } else {
                Some(o.id)
            };
        } else if o.close_menu {
            state.open_menu_id = None;
        }
    }
    (act_out, size_out)
}
