//! Free-play egui front-end: the host/grid/leave flow recreated from the design handoff.
//!
//! Module map:
//! - [`theme`]     — the shared design system (tokens, visuals, fonts, painter helpers).
//! - [`model`]     — the free-play state ([`model::AppState`] + [`model::Table`]).
//! - [`cards`]     — playing-card parsing + rendering.
//! - [`widgets`]   — small shared widgets (segmented pills, steppers, badges, status dots).
//! - [`screens`]   — the two top-level screens (grid, standalone host).
//! - [`components`]— the leaf building blocks the screens compose.
//!
//! [`FreePlayApp`] owns the [`model::AppState`] and drives the whole thing: screen routing (grid as
//! the primary surface, with the host reachable both as a right slide-over and as a standalone
//! screen), the global keyboard shortcuts, and the per-frame animation/sim clock that counts down
//! your-turn timers, rotates sim turns, and times the leave flow.

pub mod theme;
pub mod model;
pub mod cards;
pub mod widgets;
pub mod screens;
pub mod components;

use eframe::egui;

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
            Ok(Box::new(FreePlayApp::new()) as Box<dyn eframe::App>)
        }),
    )
}

/// How often to nudge a repaint while the world is animating (timers/leave/transitions). The model
/// timers tick every 250ms in the prototype, so a ~250ms cadence keeps countdowns smooth without
/// burning frames when nothing is moving.
const ANIM_REPAINT_MS: u64 = 250;

/// Pulse period (ms) for the shared eased opacity (need-action / waiting dots, accent chips). Matches
/// the prototype's ~1.2s `tcpulse`.
const PULSE_PERIOD_MS: f32 = 1_200.0;

/// The free-play application. Owns all UI state; screens borrow it each frame.
pub struct FreePlayApp {
    state: AppState,
    /// Guards one-time context setup (fonts/style) on the first `ui` frame.
    initialized: bool,
}

impl FreePlayApp {
    pub fn new() -> Self {
        FreePlayApp { state: AppState::new(), initialized: false }
    }

    /// Are any of the time-driven systems live this frame? Used to decide whether to keep nudging
    /// repaints: any your-turn timer counting down, a leave flow processing, or a sim turn pending.
    fn is_animating(&self) -> bool {
        self.state.need_count() > 0
            || self.state.leave.is_some()
            || self.state.host_open
    }

    /// Eased pulse opacity (`~0.4..=1.0`) derived from the animation clock — a triangle wave so the
    /// dots/chips breathe without per-component phase tracking.
    fn pulse(&self) -> f32 {
        let phase = (self.state.clock_ms % PULSE_PERIOD_MS) / PULSE_PERIOD_MS; // 0..1
        // Triangle 0→1→0, then map into the prototype's 1.0 → 0.4 → 1.0 opacity envelope.
        let tri = 1.0 - (phase * 2.0 - 1.0).abs();
        1.0 - 0.6 * tri
    }

    /// Apply the global keyboard shortcuts, ONLY when no modal and no slide-over is open (and we're
    /// on the grid). `F` folds, `C` calls/checks, `R` raises the *focused* table; `Space` moves
    /// focus to the next table that needs action.
    fn handle_shortcuts(&mut self, ui: &egui::Ui) {
        // Gate: shortcuts are inert behind the leave modal, the host slide-over, or off the grid.
        if self.state.leave.is_some()
            || self.state.host_open
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
        // F/C/R act on the focused your-turn table; `act` itself no-ops if it isn't your turn there.
        if let Some(id) = self.state.focus_id {
            if fold {
                self.state.act(id, Act::Fold);
            } else if call {
                self.state.act(id, Act::Call);
            } else if raise {
                self.state.act(id, Act::Raise);
            }
        }
    }
}

impl Default for FreePlayApp {
    fn default() -> Self {
        Self::new()
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

        // --- Animation / sim clock ------------------------------------------------------------
        // Advance the model by the real frame delta so timers count down at wall-clock speed (the
        // first frame has no stable delta, so clamp it to the repaint cadence).
        let dt_ms = (ui.input(|i| i.stable_dt) * 1_000.0).clamp(0.0, 250.0);
        self.state.tick(dt_ms);

        // While anything is animating, keep frames flowing so countdowns/leave/transitions advance.
        if self.is_animating() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(ANIM_REPAINT_MS));
        }

        // --- Global keyboard shortcuts (only when no modal/slide-over is open) -----------------
        self.handle_shortcuts(ui);

        // --- Screen routing -------------------------------------------------------------------
        // The grid is the primary play surface; it hosts the `+ New table` slide-over and the leave
        // modal internally. The standalone host screen is reachable for completeness.
        let pulse = self.pulse();
        match self.state.screen {
            Screen::Grid => screens::grid::render(ui, &mut self.state, pulse),
            Screen::Host => {
                screens::host::render(ui, &mut self.state);
            }
        }
    }
}
