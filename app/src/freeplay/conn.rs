//! The single live host/guest connection behind the free-play UI. ONE active table at a time: the
//! grid is a shell, but only one [`TableConn`] (one running driver task) exists at once.
//!
//! A [`TableConn`] owns the same plumbing the old single-table UI used (see `crate::ui`): a
//! [`GuiState`] snapshot shared with the driver task behind a `std::sync::Mutex`, a bounded
//! [`mpsc::Sender`] feeding the local seat's chosen [`Action`] into the driver's `select!` loop, and
//! the driver [`JoinHandle`]. The observer installed on the driver IS `crate::ui::apply_update`: each
//! [`DriverUpdate`] is folded into the snapshot under the mutex and the egui [`Context`] is repainted.
//!
//! [`map_action`] is the pure bridge from a UI [`Act`] (with the action bar's sizing) to the
//! authoritative wire [`Action`], clamped/gated against the live [`LegalActions`] bounds.

use std::sync::{Arc, Mutex};

use eframe::egui;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use poker_protocol::{
    run_guest_interactive, run_host_interactive, Action, DriverError, DriverUpdate, GameReport,
    HostOptions,
};

use crate::freeplay::model::Act;
use crate::gui_state::{Conn, GuiState, LegalActions, Role};
use crate::ui::apply_update;

/// Map a UI [`Act`] to the authoritative wire [`Action`], using the live [`LegalActions`] bounds to
/// gate legality and clamp bet/raise sizing. Returns `None` when the action is not currently legal
/// (the action bar should not have offered it, but the engine is authoritative regardless).
///
/// - `Fold` → `Fold`.
/// - `CheckCall` → `Check` when checking is legal, else `Call` (or `AllIn` when the call is all-in).
/// - `Bet(n)` → `Bet(n')` with `n'` clamped to `[min_bet, max_to]`, only when betting is legal.
/// - `Raise(n)` → `Raise(n')` with `n'` clamped to `[min_raise_to, max_to]`, only when raising is legal.
/// - `AllIn` → `AllIn` when all-in is legal.
pub fn map_action(act: Act, legal: &LegalActions) -> Option<Action> {
    match act {
        Act::Fold => Some(Action::Fold),
        Act::CheckCall => {
            if legal.can_check {
                Some(Action::Check)
            } else if legal.call_is_all_in {
                Some(Action::AllIn)
            } else if legal.can_call {
                Some(Action::Call)
            } else {
                None
            }
        }
        // `max_to.max(min_*)` keeps the clamp range non-inverted: `u64::clamp` panics if min > max,
        // which the engine's bounds never produce but a degenerate short-stack snapshot could.
        Act::Bet(n) => legal
            .can_bet
            .then(|| Action::Bet(n.clamp(legal.min_bet, legal.max_to.max(legal.min_bet)))),
        Act::Raise(n) => legal.can_raise.then(|| {
            Action::Raise(n.clamp(legal.min_raise_to, legal.max_to.max(legal.min_raise_to)))
        }),
        Act::AllIn => legal.can_all_in.then_some(Action::AllIn),
    }
}

/// Keep an already-recorded terminal [`Conn::Error`] (it carries the real cause); otherwise use
/// `fallback` (a clean game-over).
fn keep_error_or(current: Conn, fallback: Conn) -> Conn {
    if matches!(current, Conn::Error(_)) {
        current
    } else {
        fallback
    }
}

/// One live host/guest connection: the shared snapshot, the action channel into the driver, and the
/// driver task handle.
#[allow(dead_code)]
pub struct TableConn {
    /// Snapshot shared with the driver task (it writes via the observer; the UI reads a clone).
    state: Arc<Mutex<GuiState>>,
    /// Feeds the local seat's chosen action into the running driver (bounded depth 1).
    action_tx: mpsc::Sender<Action>,
    /// The driver task; `None` once reaped.
    driver: Option<JoinHandle<Result<GameReport, DriverError>>>,
}

impl TableConn {
    /// Start hosting a table on `rt`, waking `ctx` on each update. Spawns `run_host_interactive`
    /// with `crate::ui::apply_update` as the observer. Seeds the snapshot as `Role::Host` /
    /// `Conn::Waiting` (the host is waiting for enough players to join).
    pub fn host(rt: &tokio::runtime::Runtime, ctx: egui::Context, opts: HostOptions) -> Self {
        let (action_tx, rx) = mpsc::channel::<Action>(1);
        let state = Arc::new(Mutex::new(GuiState::default()));
        {
            let mut s = state.lock().unwrap();
            s.role = Some(Role::Host);
            s.conn = Conn::Waiting;
        }
        let task_state = state.clone();
        let driver = rt.spawn(async move {
            let mut observer = move |u: DriverUpdate| apply_update(&task_state, &ctx, u);
            run_host_interactive(rx, opts, |_uri| {}, &mut observer).await
        });
        TableConn {
            state,
            action_tx,
            driver: Some(driver),
        }
    }

    /// Join the table at `uri` on `rt`, waking `ctx` on each update. Spawns `run_guest_interactive`
    /// with `crate::ui::apply_update` as the observer. Seeds the snapshot as `Role::Guest` /
    /// `Conn::Connecting` (the guest is dialing the host).
    pub fn join(rt: &tokio::runtime::Runtime, ctx: egui::Context, uri: String) -> Self {
        let (action_tx, rx) = mpsc::channel::<Action>(1);
        let state = Arc::new(Mutex::new(GuiState::default()));
        {
            let mut s = state.lock().unwrap();
            s.role = Some(Role::Guest);
            s.conn = Conn::Connecting;
        }
        let task_state = state.clone();
        let driver = rt.spawn(async move {
            let mut observer = move |u: DriverUpdate| apply_update(&task_state, &ctx, u);
            run_guest_interactive(&uri, rx, None, true, &mut observer).await
        });
        TableConn {
            state,
            action_tx,
            driver: Some(driver),
        }
    }

    /// A clone of the current shared snapshot (one lock per frame).
    pub fn snapshot(&self) -> GuiState {
        self.state.lock().unwrap().clone()
    }

    /// Submit the local seat's chosen action (dropped if one is already in flight).
    pub fn send(&self, a: Action) {
        // Bounded depth-1: if one is already in flight, drop the extra (a double-click).
        let _ = self.action_tx.try_send(a);
    }

    /// Whether the driver task has exited (so the caller can reap it and reflect a terminal state).
    pub fn is_finished(&self) -> bool {
        self.driver.as_ref().map_or(true, |h| h.is_finished())
    }

    /// Join the (already-finished) driver task and return the terminal [`Conn`] it implies, so the UI
    /// can surface a clean game-over vs an error. Takes the [`JoinHandle`] so it is reaped once; the
    /// task is finished (see [`is_finished`](Self::is_finished)), so the `block_on` returns at once.
    /// A clean run becomes [`Conn::GameOver`]; a driver error becomes [`Conn::Error`]; an `Error`
    /// already recorded by the observer is preserved.
    pub fn reap_status(&mut self, rt: &tokio::runtime::Runtime) -> Conn {
        let current = self.state.lock().unwrap().conn.clone();
        let terminal = match self.driver.take() {
            Some(h) => match rt.block_on(h) {
                Ok(Ok(_report)) => keep_error_or(current, Conn::GameOver),
                Ok(Err(e)) => Conn::Error(e.to_string()),
                Err(_join) => keep_error_or(current, Conn::GameOver),
            },
            None => keep_error_or(current, Conn::GameOver),
        };
        // Reflect the terminal status back into the snapshot so a later `snapshot()` agrees.
        self.state.lock().unwrap().conn = terminal.clone();
        terminal
    }

    /// Tear the connection down, aborting the driver task.
    pub fn abort(self) {
        if let Some(h) = self.driver {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A baseline `LegalActions` with everything disabled; tests flip on just the field under test.
    fn legal() -> LegalActions {
        LegalActions::default()
    }

    #[test]
    fn fold_maps_to_fold() {
        assert_eq!(map_action(Act::Fold, &legal()), Some(Action::Fold));
    }

    #[test]
    fn checkcall_checks_when_check_is_legal() {
        let l = LegalActions {
            can_check: true,
            ..legal()
        };
        assert_eq!(map_action(Act::CheckCall, &l), Some(Action::Check));
    }

    #[test]
    fn checkcall_calls_when_call_is_legal_and_not_all_in() {
        let l = LegalActions {
            can_check: false,
            can_call: true,
            call_is_all_in: false,
            call_amount: 40,
            ..legal()
        };
        assert_eq!(map_action(Act::CheckCall, &l), Some(Action::Call));
    }

    #[test]
    fn checkcall_goes_all_in_when_the_call_is_all_in() {
        let l = LegalActions {
            can_check: false,
            can_call: true,
            call_is_all_in: true,
            ..legal()
        };
        assert_eq!(map_action(Act::CheckCall, &l), Some(Action::AllIn));
    }

    #[test]
    fn checkcall_is_none_when_neither_check_nor_call_is_legal() {
        assert_eq!(map_action(Act::CheckCall, &legal()), None);
    }

    #[test]
    fn bet_clamps_into_min_max() {
        let l = LegalActions {
            can_bet: true,
            min_bet: 10,
            max_to: 1500,
            ..legal()
        };
        assert_eq!(map_action(Act::Bet(5), &l), Some(Action::Bet(10)));
        assert_eq!(map_action(Act::Bet(100), &l), Some(Action::Bet(100)));
        assert_eq!(map_action(Act::Bet(2000), &l), Some(Action::Bet(1500)));
    }

    #[test]
    fn bet_is_none_when_betting_is_illegal() {
        let l = LegalActions {
            can_bet: false,
            min_bet: 10,
            max_to: 1500,
            ..legal()
        };
        assert_eq!(map_action(Act::Bet(100), &l), None);
    }

    #[test]
    fn raise_clamps_into_min_max() {
        let l = LegalActions {
            can_raise: true,
            min_raise_to: 80,
            max_to: 1500,
            ..legal()
        };
        assert_eq!(map_action(Act::Raise(50), &l), Some(Action::Raise(80)));
        assert_eq!(map_action(Act::Raise(200), &l), Some(Action::Raise(200)));
        assert_eq!(map_action(Act::Raise(5000), &l), Some(Action::Raise(1500)));
    }

    #[test]
    fn raise_is_none_when_raising_is_illegal() {
        let l = LegalActions {
            can_raise: false,
            min_raise_to: 80,
            max_to: 1500,
            ..legal()
        };
        assert_eq!(map_action(Act::Raise(200), &l), None);
    }

    #[test]
    fn all_in_maps_when_legal() {
        let l = LegalActions {
            can_all_in: true,
            ..legal()
        };
        assert_eq!(map_action(Act::AllIn, &l), Some(Action::AllIn));
    }

    #[test]
    fn all_in_is_none_when_illegal() {
        let l = LegalActions {
            can_all_in: false,
            ..legal()
        };
        assert_eq!(map_action(Act::AllIn, &l), None);
    }
}
