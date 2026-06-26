//! egui/eframe desktop client. Compiled only under the `gui` feature so the default (headless)
//! build needs no display/windowing libraries.
//!
//! Architecture: eframe owns the main-thread event loop; a tokio runtime owned by [`PokerApp`] runs
//! the async host/guest driver on its own threads. The driver and the UI share a [`GuiState`]
//! snapshot behind a `std::sync::Mutex`: the driver writes it via an observer (and wakes the UI with
//! `request_repaint`), the UI reads a clone each frame. The local player's betting actions flow the
//! other way over a bounded channel into the driver's `select!` loop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use tokio::sync::mpsc;

use poker_protocol::{
    run_guest_interactive, run_host_interactive, Action, DriverError, DriverUpdate, GameReport,
    HostOptions,
};

use crate::gui_state::{card_str, Conn, GuiState, ResultView, Role, Screen};

/// Default cap on hands per session — effectively "until someone leaves" for a manual test.
const SESSION_HANDS: u64 = 1000;

pub fn run() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Ten-Cent Poker",
        native_options,
        Box::new(|cc| Ok(Box::new(PokerApp::new(cc.egui_ctx.clone())) as Box<dyn eframe::App>)),
    )
}

struct PokerApp {
    rt: tokio::runtime::Runtime,
    egui_ctx: egui::Context,
    state: Arc<Mutex<GuiState>>,
    /// Sends the local player's chosen action into the running driver (bounded depth 1).
    action_tx: Option<mpsc::Sender<Action>>,
    driver: Option<tokio::task::JoinHandle<Result<GameReport, DriverError>>>,
    screen: Screen,
    // Lobby form.
    join_uri: String,
    seats: usize,
    mental: bool,
    /// Optional fixed listen port (for port-forwarding to host remotely). Empty = ephemeral.
    port: String,
    // Action-bar bet/raise amount.
    bet_to: u64,
}

impl PokerApp {
    fn new(egui_ctx: egui::Context) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        PokerApp {
            rt,
            egui_ctx,
            state: Arc::new(Mutex::new(GuiState::default())),
            action_tx: None,
            driver: None,
            screen: Screen::Lobby,
            join_uri: String::new(),
            seats: 3,
            mental: true,
            port: String::new(),
            bet_to: 0,
        }
    }

    fn start_host(&mut self) {
        let (tx, rx) = mpsc::channel::<Action>(1);
        self.action_tx = Some(tx);
        {
            let mut s = self.state.lock().unwrap();
            *s = GuiState::default();
            s.role = Some(Role::Host);
            s.conn = Conn::Waiting;
        }
        let opts = HostOptions {
            hands: SESSION_HANDS,
            min_players: self.seats,
            mental: self.mental,
            // Empty or unparseable → ephemeral port; a value pins it for port-forwarding.
            listen_port: self.port.trim().parse().ok(),
            ..HostOptions::default()
        };
        let state = self.state.clone();
        let ctx = self.egui_ctx.clone();
        self.driver = Some(self.rt.spawn(async move {
            let mut observer = move |u: DriverUpdate| apply_update(&state, &ctx, u);
            run_host_interactive(rx, opts, |_uri| {}, &mut observer).await
        }));
        self.screen = Screen::Table;
    }

    fn start_guest(&mut self) {
        let uri = self.join_uri.trim().to_string();
        let (tx, rx) = mpsc::channel::<Action>(1);
        self.action_tx = Some(tx);
        {
            let mut s = self.state.lock().unwrap();
            *s = GuiState::default();
            s.role = Some(Role::Guest);
            s.conn = Conn::Connecting;
        }
        let state = self.state.clone();
        let ctx = self.egui_ctx.clone();
        self.driver = Some(self.rt.spawn(async move {
            let mut observer = move |u: DriverUpdate| apply_update(&state, &ctx, u);
            run_guest_interactive(&uri, rx, None, true, &mut observer).await
        }));
        self.screen = Screen::Table;
    }

    fn send_action(&mut self, action: Action) {
        if let Some(tx) = &self.action_tx {
            // Bounded depth-1: if one is already in flight, drop the extra (a double-click).
            let _ = tx.try_send(action);
        }
    }

    fn leave_table(&mut self) {
        if let Some(h) = self.driver.take() {
            h.abort();
        }
        self.action_tx = None;
        *self.state.lock().unwrap() = GuiState::default();
        self.screen = Screen::Lobby;
    }

    /// Reap a finished driver task and reflect its terminal status in the snapshot.
    fn reap_driver(&mut self) {
        let finished = self.driver.as_ref().is_some_and(|h| h.is_finished());
        if !finished {
            return;
        }
        let h = self.driver.take().unwrap();
        let mut s = self.state.lock().unwrap();
        match self.rt.block_on(h) {
            Ok(Ok(_report)) => {
                if !matches!(s.conn, Conn::Error(_)) {
                    s.conn = Conn::GameOver;
                }
            }
            Ok(Err(e)) => s.conn = Conn::Error(e.to_string()),
            Err(_join) => {
                if !matches!(s.conn, Conn::Error(_)) {
                    s.conn = Conn::GameOver;
                }
            }
        }
        drop(s);
        self.action_tx = None;
    }

    fn lobby_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ten-Cent Poker");
        ui.separator();
        ui.label("Host a table, or join one by pasting its URI.");
        ui.add_space(10.0);

        ui.group(|ui| {
            ui.strong("Host a table");
            ui.horizontal(|ui| {
                ui.label("Players:");
                ui.add(egui::DragValue::new(&mut self.seats).range(2..=9));
            });
            ui.checkbox(
                &mut self.mental,
                "Trustless dealing (hides hole cards; recommended)",
            );
            ui.horizontal(|ui| {
                ui.label("Listen port:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.port)
                        .hint_text("auto")
                        .desired_width(80.0),
                );
                ui.label("(set + forward this port on your router to host remotely)");
            });
            if ui.button("Host").clicked() {
                self.start_host();
            }
        });

        ui.add_space(10.0);

        ui.group(|ui| {
            ui.strong("Join a table");
            ui.add(
                egui::TextEdit::singleline(&mut self.join_uri)
                    .hint_text("tcpoker://…")
                    .desired_width(f32::INFINITY),
            );
            let ok = self.join_uri.trim().starts_with("tcpoker://");
            if ui.add_enabled(ok, egui::Button::new("Join")).clicked() {
                self.start_guest();
            }
        });
    }

    fn table_ui(&mut self, ui: &mut egui::Ui) {
        let snap = self.state.lock().unwrap().clone();

        let mut leave = false;
        ui.horizontal(|ui| {
            ui.heading("Ten-Cent Poker");
            if ui.button("Leave table").clicked() {
                leave = true;
            }
        });
        if leave {
            self.leave_table();
            return;
        }
        ui.separator();

        // Lobby / lifecycle banners.
        match &snap.conn {
            Conn::Waiting => {
                ui.label("Waiting for players to join…");
                if let Some(uri) = &snap.table_uri {
                    ui.add_space(6.0);
                    ui.label("Share this table URI with the other players:");
                    let mut uri_disp = uri.clone();
                    ui.add(egui::TextEdit::singleline(&mut uri_disp).desired_width(f32::INFINITY));
                    if ui.button("Copy URI").clicked() {
                        ui.ctx().copy_text(uri.clone());
                    }
                } else {
                    ui.label("(assembling table URI…)");
                }
                if let Some(w) = &snap.reachability_warning {
                    ui.colored_label(egui::Color32::from_rgb(220, 140, 0), format!("⚠ {w}"));
                }
            }
            Conn::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting to host…");
                });
            }
            Conn::Error(e) => {
                ui.colored_label(egui::Color32::RED, format!("Table ended: {e}"));
            }
            Conn::GameOver => {
                ui.label("Game over — the host stopped dealing.");
            }
            _ => {}
        }

        if matches!(snap.conn, Conn::Playing | Conn::GameOver) && !snap.seats.is_empty() {
            ui.add_space(8.0);
            self.render_hand(ui, &snap);
        }

        if let Some(r) = &snap.last_result {
            ui.add_space(8.0);
            ui.separator();
            render_result(ui, r);
        }

        // Animate the dealing spinner.
        if snap.dealing {
            ui.ctx().request_repaint_after(Duration::from_millis(400));
        }
    }

    fn render_hand(&mut self, ui: &mut egui::Ui, snap: &GuiState) {
        // Header line.
        if let Some(hn) = snap.hand_no {
            let mode = if snap.is_mental {
                "trustless"
            } else {
                "placeholder"
            };
            let street = snap
                .street
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "—".to_string());
            ui.label(format!("Hand #{hn} · {mode} · {street}"));
        }

        if snap.dealing {
            ui.horizontal(|ui| {
                ui.spinner();
                let phase = snap
                    .deal_phase
                    .map(|p| format!("{p:?}"))
                    .unwrap_or_default();
                ui.label(format!("Dealing… {phase}"));
            });
        }

        // Seats.
        ui.add_space(4.0);
        for sv in &snap.seats {
            let mut text = format!("{:<14} stack {}", sv.label, sv.stack);
            if sv.committed > 0 {
                text += &format!("  ·  bet {}", sv.committed);
            }
            if sv.is_button {
                text += "  [BTN]";
            }
            if sv.folded {
                text += "  [folded]";
            }
            if sv.all_in {
                text += "  [all-in]";
            }
            if sv.is_to_act {
                ui.colored_label(egui::Color32::from_rgb(90, 200, 120), format!("▶ {text}"));
            } else {
                ui.label(text);
            }
        }

        // Board + pot.
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Board:");
            if snap.board.is_empty() {
                ui.label("—");
            }
            for c in &snap.board {
                ui.label(card_str(c));
            }
        });
        ui.strong(format!("Pot: {}", snap.pot));

        // Your hand.
        ui.horizontal(|ui| {
            ui.label("Your hand:");
            match &snap.my_hole {
                Some([a, b]) => {
                    ui.strong(format!("{} {}", card_str(a), card_str(b)));
                }
                None => {
                    ui.label(if snap.dealing {
                        "(dealing…)"
                    } else {
                        "(folded / not dealt in)"
                    });
                }
            }
        });

        // Action bar (only on our turn, and only once the deal is ready).
        if snap.is_my_turn && !snap.dealing {
            let legal = snap.legal.clone();
            // Default the bet/raise amount into the active legal range.
            let lo = if legal.can_bet {
                legal.min_bet
            } else {
                legal.min_raise_to
            };
            if self.bet_to < lo || self.bet_to > legal.max_to {
                self.bet_to = lo;
            }

            ui.add_space(6.0);
            ui.separator();
            ui.strong("Your turn");
            let mut chosen: Option<Action> = None;
            ui.horizontal(|ui| {
                if ui.button("Fold").clicked() {
                    chosen = Some(Action::Fold);
                }
                if legal.can_check && ui.button("Check").clicked() {
                    chosen = Some(Action::Check);
                }
                if legal.can_call {
                    let label = if legal.call_is_all_in {
                        format!("Call {} (all-in)", legal.call_amount)
                    } else {
                        format!("Call {}", legal.call_amount)
                    };
                    if ui.button(label).clicked() {
                        chosen = Some(if legal.call_is_all_in {
                            Action::AllIn
                        } else {
                            Action::Call
                        });
                    }
                }
            });
            ui.horizontal(|ui| {
                if legal.can_bet {
                    ui.add(
                        egui::DragValue::new(&mut self.bet_to).range(legal.min_bet..=legal.max_to),
                    );
                    if ui.button("Bet").clicked() {
                        chosen = Some(Action::Bet(self.bet_to));
                    }
                }
                if legal.can_raise {
                    ui.add(
                        egui::DragValue::new(&mut self.bet_to)
                            .range(legal.min_raise_to..=legal.max_to),
                    );
                    if ui.button("Raise to").clicked() {
                        chosen = Some(Action::Raise(self.bet_to));
                    }
                }
                if legal.can_all_in && ui.button("All-in").clicked() {
                    chosen = Some(Action::AllIn);
                }
            });
            if let Some(a) = chosen {
                self.send_action(a);
            }
        }
    }
}

impl eframe::App for PokerApp {
    // eframe 0.34: the framework hands us the central `Ui` directly.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.reap_driver();
        // While a game is running, keep the UI ticking so a silent driver exit (e.g. a peer leaving,
        // which emits no observer update) is reaped promptly and live state stays fresh.
        if self.driver.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        match self.screen {
            Screen::Lobby => self.lobby_ui(ui),
            Screen::Table => self.table_ui(ui),
        }
    }
}

/// Render a completed hand's result.
fn render_result(ui: &mut egui::Ui, r: &ResultView) {
    ui.label(format!("Last hand (#{})", r.hand_no));
    ui.horizontal(|ui| {
        ui.label("Board:");
        for c in &r.board {
            ui.label(card_str(c));
        }
        if let Some([a, b]) = &r.my_hole {
            ui.label(format!("· your hand {} {}", card_str(a), card_str(b)));
        }
    });
    ui.label(format!(
        "Deltas: {:?}   Stacks: {:?}",
        r.deltas, r.final_stacks
    ));
}

/// Observer the driver task calls (on the tokio runtime) as the game progresses: fold each update
/// into the shared snapshot and wake the UI thread. Shared with the free-play connection layer
/// (`crate::freeplay::conn`), which installs this exact observer on its driver task.
pub fn apply_update(state: &Arc<Mutex<GuiState>>, ctx: &egui::Context, u: DriverUpdate) {
    {
        let mut s = state.lock().unwrap();
        match u {
            DriverUpdate::Uri(uri) => s.table_uri = Some(uri),
            DriverUpdate::Reachability(r) => s.reachability_warning = Some(r),
            DriverUpdate::State(t) => {
                if !matches!(s.conn, Conn::Error(_)) {
                    s.conn = Conn::Playing;
                }
                s.update_from_table(t);
            }
            DriverUpdate::HandResult {
                outcome,
                local_hole,
            } => {
                s.last_result = Some(ResultView::from_outcome(&outcome, local_hole));
            }
            DriverUpdate::Ended => {
                if !matches!(s.conn, Conn::Error(_)) {
                    s.conn = Conn::GameOver;
                }
            }
        }
    }
    ctx.request_repaint();
}
