//! Free-play egui front-end: the host/grid/leave flow wired to the REAL peer-to-peer poker node.
//!
//! Module map:
//! - [`theme`]     — the shared design system (tokens, visuals, fonts, painter helpers).
//! - [`model`]     — the pure free-play render state ([`model::AppState`] + [`model::Table`]).
//! - [`cards`]     — playing-card parsing + rendering.
//! - [`widgets`]   — small shared widgets (segmented pills, steppers, badges, status dots).
//! - [`screens`]   — the two top-level screens (grid, standalone host).
//! - [`components`]— the leaf building blocks the screens compose.
//! - [`project`]   — PURE projection from the live [`crate::gui_state::GuiState`] into a render table.
//! - [`conn`]      — the single live host/guest connection ([`conn::TableConn`]).
//!
//! [`FreePlayApp`] owns ONE tokio runtime, ONE optional [`conn::TableConn`] (one active table at a
//! time), and the egui [`Context`]. Each frame, before rendering, it projects the connection's
//! snapshot into the single active [`model::Table`], reflects the snapshot lifecycle into the grid
//! banners, runs the your-turn timer (display + real timeout auto-action), and reaps a finished driver. Tile /
//! keyboard actions route through [`conn::map_action`] into the live driver — there is no local
//! simulation; displayed pot/stacks/board/turn come only from projected real state.

pub mod cards;
pub mod components;
pub mod conn;
pub mod model;
pub mod project;
pub mod screens;
pub mod theme;
pub mod widgets;

use std::time::Duration;

use eframe::egui;

use poker_protocol::HostOptions;

use crate::gui_state::{Conn, GuiState};
use model::{Act, AppState, Screen};

/// Install the theme and run the free-play native window.
pub fn run() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Ten-Cent Poker",
        native_options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply_style(&cc.egui_ctx);
            Ok(Box::new(FreePlayApp::new(cc.egui_ctx.clone())) as Box<dyn eframe::App>)
        }),
    )
}

/// How often to nudge a repaint while the world is animating (timers/leave/transitions).
const ANIM_REPAINT_MS: u64 = 250;

/// Repaint cadence while a connection is live — keeps the turn timer counting down, the projected
/// state fresh, and a finished driver reaped promptly even when no observer update arrives.
const CONN_REPAINT_MS: u64 = 150;

/// Pulse period (ms) for the shared eased opacity (need-action / waiting dots, accent chips).
const PULSE_PERIOD_MS: f32 = 1_200.0;

/// The single live table's model id. Only one active table exists, so a fixed id is sufficient and
/// keeps focus / tile identity stable across frames as the snapshot is re-projected.
const LIVE_TABLE_ID: u64 = 1;

/// The free-play application. Owns the UI state, the tokio runtime, and the single live connection.
pub struct FreePlayApp {
    state: AppState,
    /// Guards one-time context setup (fonts/style) on the first `ui` frame.
    initialized: bool,

    /// The async runtime the driver task runs on.
    rt: tokio::runtime::Runtime,
    /// The egui context the driver wakes on each update (and that host/join hand to the driver).
    ctx: egui::Context,
    /// The single active host/guest connection, if any (one active table at a time).
    conn: Option<conn::TableConn>,
    /// The lifecycle state shown in the grid banner: mirrors the live snapshot while connected, then
    /// the terminal status after a driver is reaped, until the next connection starts.
    lifecycle: Conn,

    /// The action bar's current bet/raise sizing target (chips), persisted across frames and clamped
    /// into the active legal range each frame.
    bet_to: u64,
    /// Whether it was the local seat's turn last frame (rising-edge detect for the turn timer).
    was_my_turn: bool,
    /// Remaining action time for the current turn (ms) — drives the timer DISPLAY + the timeout action.
    turn_remaining_ms: f32,
    /// Whether an action (manual or the auto-fold) has already been submitted for the current turn,
    /// so the expiry fires once and a late timer tick can't double-submit.
    turn_submitted: bool,
}

impl FreePlayApp {
    pub fn new(ctx: egui::Context) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        FreePlayApp {
            state: AppState::new(),
            initialized: false,
            rt,
            ctx,
            conn: None,
            lifecycle: Conn::Idle,
            bet_to: 0,
            was_my_turn: false,
            turn_remaining_ms: model::TIMER_TOTAL_MS as f32,
            turn_submitted: false,
        }
    }

    /// Are any of the time-driven systems live this frame? Used to keep nudging repaints.
    fn is_animating(&self) -> bool {
        self.state.need_count() > 0
            || self.state.leave.is_some()
            || self.state.host_open
            || self.state.join_open
    }

    /// Eased pulse opacity (`~0.4..=1.0`) from the animation clock — a triangle wave.
    fn pulse(&self) -> f32 {
        let phase = (self.state.clock_ms % PULSE_PERIOD_MS) / PULSE_PERIOD_MS;
        let tri = 1.0 - (phase * 2.0 - 1.0).abs();
        1.0 - 0.6 * tri
    }

    // -----------------------------------------------------------------------
    // Connection sync (runs every frame BEFORE rendering)
    // -----------------------------------------------------------------------

    /// Reap a finished driver, then project the live snapshot into the single active table, reflect
    /// the lifecycle, and run the your-turn timer (display + timeout auto-action).
    fn sync_conn(&mut self, dt_ms: f32) {
        // Reap a finished driver: surface its terminal status, drop the connection + the live table.
        if self.conn.as_ref().is_some_and(|c| c.is_finished()) {
            let mut conn = self.conn.take().unwrap();
            self.lifecycle = conn.reap_status(&self.rt);
            self.state.tables.clear();
            self.state.focus_id = None;
            self.state.open_menu_id = None;
            self.reset_turn();
            // A driver that ended while we were still in the host control room (e.g. it failed or
            // timed out before any guest joined) must not leave the user on a silent Host screen —
            // route to the grid, whose lifecycle banner surfaces the terminal Error / GameOver.
            if self.state.screen == Screen::Host
                && matches!(self.lifecycle, Conn::Error(_) | Conn::GameOver)
            {
                self.state.screen = Screen::Grid;
            }
            return;
        }

        // Project the live snapshot into exactly one model table (or leave the grid empty if idle).
        let Some(snap) = self.conn.as_ref().map(|c| c.snapshot()) else {
            return;
        };
        self.lifecycle = snap.conn.clone();
        // Once play actually starts, leave the host control room for the grid.
        if self.state.screen == Screen::Host && matches!(snap.conn, Conn::Playing) {
            self.state.screen = Screen::Grid;
        }

        self.update_turn_timer(&snap, dt_ms);

        let mut table = project::project(&snap, LIVE_TABLE_ID);
        // Show the host's configured blinds (the same string threaded into HostOptions by start_host),
        // so the tile header matches the engine's posted blinds instead of the projection's placeholder.
        table.blinds = self.state.host.blinds.clone();
        // Timer DISPLAY comes from the app-owned turn clock, not the projection's (always-full) default.
        table.time_left = self.turn_remaining_ms.max(0.0) as u64;
        // Persist + clamp the bet/raise sizing target into the active legal range.
        let lo = if table.can_bet {
            table.min_bet
        } else {
            table.min_raise_to
        };
        let hi = table.max_to.max(lo);
        self.bet_to = self.bet_to.clamp(lo.min(hi), hi);
        table.bet_to = self.bet_to;

        self.state.tables = vec![table];
        self.state.focus_id = if snap.is_my_turn {
            Some(LIVE_TABLE_ID)
        } else {
            None
        };
    }

    /// Advance the your-turn action timer off the live snapshot: reset on the rising edge of our turn,
    /// count down while it is our turn, and submit the timeout action through the connection on expiry
    /// (a free check when nothing is owed, else a fold).
    fn update_turn_timer(&mut self, snap: &GuiState, dt_ms: f32) {
        let my_turn = snap.is_my_turn && !snap.dealing;
        if my_turn {
            if !self.was_my_turn {
                // Rising edge — a fresh decision point: reset the clock and the sizing default.
                self.turn_remaining_ms = model::TIMER_TOTAL_MS as f32;
                self.turn_submitted = false;
                self.bet_to = if snap.legal.can_bet {
                    snap.legal.min_bet
                } else {
                    snap.legal.min_raise_to
                };
            } else if !self.turn_submitted {
                self.turn_remaining_ms -= dt_ms;
                if self.turn_remaining_ms <= 0.0 {
                    self.turn_remaining_ms = 0.0;
                    self.turn_submitted = true;
                    // On expiry, auto-act via the REAL connection: take a free check when nothing is
                    // owed, else fold. Branch on can_check explicitly — CheckCall maps to Call/AllIn
                    // when a bet is owed, and we must never auto-call chips into the pot on a timeout.
                    let act = if snap.legal.can_check {
                        Act::CheckCall
                    } else {
                        Act::Fold
                    };
                    if let Some(conn) = &self.conn {
                        if let Some(a) = conn::map_action(act, &snap.legal) {
                            conn.send(a);
                        }
                    }
                }
            }
        } else {
            self.turn_remaining_ms = model::TIMER_TOTAL_MS as f32;
            self.turn_submitted = false;
        }
        self.was_my_turn = my_turn;
    }

    /// Reset the per-turn timer state (on a new connection / after reap / after leaving).
    fn reset_turn(&mut self) {
        self.was_my_turn = false;
        self.turn_remaining_ms = model::TIMER_TOTAL_MS as f32;
        self.turn_submitted = false;
    }

    // -----------------------------------------------------------------------
    // Connection lifecycle (host / join / leave)
    // -----------------------------------------------------------------------

    /// Start hosting a real free table (one active table at a time). Mental (trustless) dealing; the
    /// host waits for `host.seats` players. Stays on the current screen — `sync_conn` navigates to the
    /// grid once play starts, while the host preview / grid banner surface the live invite.
    fn start_host(&mut self) {
        if self.conn.is_some() {
            return;
        }
        // Thread the host's selected blinds into the engine so the game actually posts them (the
        // displayed header reads the same `host.blinds` string — see `sync_conn`). A malformed
        // selection falls back to the engine default rather than silently posting nothing.
        let defaults = HostOptions::default();
        let (small_blind, big_blind) = model::parse_blinds(&self.state.host.blinds)
            .unwrap_or((defaults.small_blind, defaults.big_blind));
        let opts = HostOptions {
            mental: true,
            min_players: self.state.host.seats,
            small_blind,
            big_blind,
            ..defaults
        };
        self.conn = Some(conn::TableConn::host(&self.rt, self.ctx.clone(), opts));
        self.lifecycle = Conn::Waiting;
        self.bet_to = 0;
        self.reset_turn();
        self.state.host_open = false;
        self.state.join_open = false;
    }

    /// Dial a pasted `tcpoker://` invite as a guest and play on the grid.
    fn start_guest(&mut self, uri: String) {
        if self.conn.is_some() {
            return;
        }
        self.conn = Some(conn::TableConn::join(&self.rt, self.ctx.clone(), uri));
        self.lifecycle = Conn::Connecting;
        self.bet_to = 0;
        self.reset_turn();
        self.state.host_open = false;
        self.state.join_open = false;
        self.state.join_uri.clear();
        self.state.screen = Screen::Grid;
    }

    /// Finish the leave flow: abort the live driver, then clear the table + leave modal.
    fn app_finish_leave(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.abort();
        }
        self.lifecycle = Conn::Idle;
        self.reset_turn();
        self.state.finish_leave();
    }

    // -----------------------------------------------------------------------
    // Action routing (tile / keyboard → real driver)
    // -----------------------------------------------------------------------

    /// Map a UI [`Act`] against the live legal bounds and submit it through the connection.
    fn submit_act(&mut self, act: Act) {
        let Some(conn) = &self.conn else { return };
        let snap = conn.snapshot();
        if let Some(wire) = conn::map_action(act, &snap.legal) {
            conn.send(wire);
            self.turn_submitted = true;
        }
    }

    /// Apply a sizing-stepper delta to the bet/raise target (clamped into legality next frame).
    fn adjust_bet(&mut self, delta: i64) {
        let new = (self.bet_to as i64 + delta).max(0);
        self.bet_to = new as u64;
    }

    /// Apply the grid screen's intents against the connection the app owns.
    fn apply_grid(&mut self, r: screens::grid::GridResponse) {
        if r.host_create {
            self.start_host();
        }
        if let Some(uri) = r.join {
            self.start_guest(uri);
        }
        if r.size_delta != 0 {
            self.adjust_bet(r.size_delta);
        }
        if let Some(a) = r.act {
            self.submit_act(a);
        }
        if r.leave_finished {
            self.app_finish_leave();
        }
    }

    /// Global keyboard shortcuts on the grid (inert behind any modal / slide-over): `F` folds, `C`
    /// checks/calls, `R` raises to the current sizing target — each routed through the live connection.
    /// `Space` moves focus to the next your-turn table.
    fn handle_shortcuts(&mut self, ui: &egui::Ui) {
        if self.state.leave.is_some()
            || self.state.host_open
            || self.state.join_open
            || self.state.screen != Screen::Grid
        {
            return;
        }

        let (fold, call, raise, next) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::C),
                i.key_pressed(egui::Key::R),
                i.key_pressed(egui::Key::Space),
            )
        });

        if next {
            self.state.focus_next();
        }
        let act = if fold {
            Some(Act::Fold)
        } else if call {
            Some(Act::CheckCall)
        } else if raise {
            Some(Act::Raise(self.bet_to))
        } else {
            None
        };
        if let Some(a) = act {
            // Only act on our live turn; map_action is authoritative about legality regardless.
            let on_turn = self.conn.as_ref().is_some_and(|c| c.snapshot().is_my_turn);
            if on_turn {
                self.submit_act(a);
            }
        }
    }
}

impl Default for FreePlayApp {
    fn default() -> Self {
        Self::new(egui::Context::default())
    }
}

impl eframe::App for FreePlayApp {
    // eframe 0.34 hands us the central `Ui` directly.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.initialized {
            theme::install_fonts(ui.ctx());
            theme::apply_style(ui.ctx());
            self.initialized = true;
        }

        // --- UI-only animation / leave clock ---------------------------------------------------
        let dt_ms = (ui.input(|i| i.stable_dt) * 1_000.0).clamp(0.0, 250.0);
        self.state.tick(dt_ms);

        // --- Connection sync (project snapshot → table, run turn timer, reap) ------------------
        self.sync_conn(dt_ms);

        // Keep frames flowing while anything is moving (animations) or a connection is live (so the
        // turn timer counts down and a silent driver exit is reaped promptly).
        if self.is_animating() {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(ANIM_REPAINT_MS));
        }
        if self.conn.is_some() {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(CONN_REPAINT_MS));
        }

        // --- Global keyboard shortcuts (route through the live connection) ----------------------
        self.handle_shortcuts(ui);

        // --- Screen routing -------------------------------------------------------------------
        let pulse = self.pulse();
        let lifecycle = self.lifecycle.clone();
        match self.state.screen {
            Screen::Grid => {
                let r = screens::grid::render(ui, &mut self.state, pulse, &lifecycle);
                self.apply_grid(r);
            }
            Screen::Host => {
                let r = screens::host::render(ui, &mut self.state);
                if r.create {
                    self.start_host();
                }
                if let Some(uri) = r.join {
                    self.start_guest(uri);
                }
            }
        }
    }
}
