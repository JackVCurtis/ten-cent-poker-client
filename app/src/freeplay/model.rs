//! The free-play state model — the in-memory data the whole free-play UI renders and mutates. Mirrors
//! the README "State Management (free-play subset)" section: a list of [`Table`]s plus top-level
//! focus/menu/leave/host-config fields, the screen route, and an animation clock. This is pure data +
//! small transition helpers; it owns no egui types so it stays trivially cloneable and testable.
//!
//! All amounts are CHIPS (free play only) — there is no staked/ETH branch here by design.

/// Which poker variant a table runs. The grid header badge and host title derive from this.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Game {
    Holdem,
    Omaha,
    Stud,
}

impl Game {
    /// Short header badge text: `NLH` / `PLO` / `STUD`.
    pub fn badge(self) -> &'static str {
        match self {
            Game::Holdem => "NLH",
            Game::Omaha => "PLO",
            Game::Stud => "STUD",
        }
    }

    /// Full preview title, e.g. `TEXAS HOLD'EM`.
    pub fn title(self) -> &'static str {
        match self {
            Game::Holdem => "TEXAS HOLD'EM",
            Game::Omaha => "OMAHA",
            Game::Stud => "SEVEN-CARD STUD",
        }
    }
}

/// Table visibility (host config). No behavioral effect in free play; purely a segmented control.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
}

/// One seat at a table. An empty seat renders as a dashed `Open` pill.
#[derive(Clone)]
pub enum Seat {
    /// A seated player: display name + chip stack. `folded` dims the pill to 0.4 opacity.
    Filled {
        name: String,
        stack: u64,
        folded: bool,
    },
    /// An open seat.
    Empty,
}

impl Seat {
    /// Convenience constructor for a seated (active, not-folded) player.
    pub fn filled(name: impl Into<String>, stack: u64) -> Self {
        Seat::Filled {
            name: name.into(),
            stack,
            folded: false,
        }
    }

    /// Convenience constructor for a seated player who has folded this hand (dimmed).
    pub fn folded(name: impl Into<String>, stack: u64) -> Self {
        Seat::Filled {
            name: name.into(),
            stack,
            folded: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Seat::Empty)
    }

    /// True if this is a seated player who has folded out of the current hand.
    pub fn is_folded(&self) -> bool {
        matches!(self, Seat::Filled { folded: true, .. })
    }
}

/// A single free table tile's full state. `seats[0]` is always the local player ("You").
#[derive(Clone)]
pub struct Table {
    pub id: u64,
    pub name: String,
    pub game: Game,
    /// Display string for blinds, in chips, e.g. `"20 / 40"`.
    pub blinds: String,
    /// Pot size in chips; `0` renders as `—` / pre-flop.
    pub pot: u64,
    /// Community board, space-separated card tokens (`"As Kd 7c"`); empty = pre-flop.
    pub board: String,
    /// Hero hole cards, space-separated (`"7d 7c"`; 4 tokens for Omaha).
    pub hero: String,
    pub seats: Vec<Seat>,
    /// Index into `seats` of the dealer button.
    pub dealer: usize,
    /// Index into `seats` of the player to act when it is NOT your turn.
    pub act: usize,
    pub your_turn: bool,
    /// Chips owed to call; `0` => the action button reads `Check`.
    pub to_call: u64,
    /// Remaining action time in ms; counts down while `your_turn`.
    pub time_left: u64,

    /// The host's shareable `tcpoker://` invite URI, once known (host tables only).
    pub invite_uri: Option<String>,
    /// A reachability warning surfaced by the host node (non-routable address, etc.), if any.
    pub reachability: Option<String>,

    // Live legal-action bounds, mirroring [`crate::gui_state::LegalActions`]. Projected from the
    // active connection's snapshot and consumed by the action bar to gate buttons + clamp sizing.
    /// Checking is legal (nothing owed).
    pub can_check: bool,
    /// Calling is legal (a bet is owed and the seat can cover at least part of it).
    pub can_call: bool,
    /// Opening a bet is legal (no outstanding bet).
    pub can_bet: bool,
    /// Raising is legal (an outstanding bet the seat can exceed).
    pub can_raise: bool,
    /// Going all-in is legal (the seat has chips).
    pub can_all_in: bool,
    /// Minimum legal opening bet total (chips).
    pub min_bet: u64,
    /// Minimum legal raise-to total (chips).
    pub min_raise_to: u64,
    /// Maximum bet/raise-to total — the seat's all-in size (chips).
    pub max_to: u64,
    /// The action bar's current bet/raise sizing target (chips). Owned by the app (persisted across
    /// frames and written into the projected table each frame, clamped into the active legal range);
    /// the action bar renders + steps it and emits `Act::Bet(bet_to)` / `Act::Raise(bet_to)`.
    pub bet_to: u64,
}

/// Total action-timer duration in ms (prototype `TOTAL`).
pub const TIMER_TOTAL_MS: u64 = 18_000;

impl Table {
    /// Hero stack = stack of `seats[0]` (the local "You" seat), or 0 if absent/empty.
    pub fn hero_stack(&self) -> u64 {
        match self.seats.first() {
            Some(Seat::Filled { stack, .. }) => *stack,
            _ => 0,
        }
    }

    /// Name of the player currently to act (for the `Waiting on <name>` line), if seated.
    pub fn acting_name(&self) -> Option<&str> {
        // Seat 0 is always the local "You" seat, so it is never the opponent "to act when it's not
        // your turn" — treat act==0 (a table with no hand in progress) as nobody-to-wait-on.
        if self.act == 0 {
            return None;
        }
        match self.seats.get(self.act) {
            Some(Seat::Filled { name, .. }) => Some(name.as_str()),
            _ => None,
        }
    }

    /// Fraction of the action timer remaining, in `0.0..=1.0`.
    pub fn timer_frac(&self) -> f32 {
        (self.time_left as f32 / TIMER_TOTAL_MS as f32).clamp(0.0, 1.0)
    }
}

/// The action a player took on a table (returned up from the action bar / keyboard). Bet/Raise carry
/// the chosen sizing (the to-total target, in chips) from the action bar's sizing control; `conn`
/// maps these to the authoritative [`poker_protocol::Action`] against the live legal bounds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Fold,
    /// Check or Call, depending on the live legal state (no outstanding bet → check).
    CheckCall,
    /// Open a bet to this total (chips).
    Bet(u64),
    /// Raise to this total (chips).
    Raise(u64),
    /// Commit the whole stack.
    AllIn,
}

/// The leave-table modal's state machine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeaveStage {
    Confirm,
    Processing,
    Done,
}

/// Active leave-table flow, if any: which table, and which stage. `elapsed_ms` accumulates only in
/// the [`LeaveStage::Processing`] stage to drive the auto-advance to [`LeaveStage::Done`].
#[derive(Clone, Copy)]
pub struct Leave {
    pub id: u64,
    pub stage: LeaveStage,
    /// Time spent in the current stage (ms); used to time the `Processing` → `Done` transition.
    pub elapsed_ms: u64,
}

/// How long the leave-table flow sits in `Processing` before auto-advancing to `Done` (prototype ~1.6s).
pub const LEAVE_PROCESSING_MS: u64 = 1_600;

/// Host-a-table configuration (free play locks the stake — there is no stake field here).
#[derive(Clone)]
pub struct HostConfig {
    pub game: Game,
    pub seats: usize,
    pub visibility: Visibility,
    /// Selected blinds display string, e.g. `"20 / 40"`.
    pub blinds: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            game: Game::Holdem,
            seats: 6,
            visibility: Visibility::Private,
            blinds: "20 / 40".to_string(),
        }
    }
}

/// Min/max seats for the host stepper (README: 2..=9, default 6).
pub const SEATS_MIN: usize = 2;
pub const SEATS_MAX: usize = 9;

/// Starting chip stack the host takes when creating a free table (prototype uses 1,500).
pub const HOST_START_STACK: u64 = 1_500;

/// Which top-level screen is showing. The standalone host is its own route; the grid hosts the
/// slide-over variant via [`AppState::slideover_open`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Host,
}

/// The whole free-play app state.
pub struct AppState {
    pub tables: Vec<Table>,
    /// Next id to assign when a table is created.
    pub next_id: u64,
    /// Keyboard-focused your-turn table, if any.
    pub focus_id: Option<u64>,
    /// Which tile's `⋯` menu is open, if any.
    pub open_menu_id: Option<u64>,
    /// Active leave-table flow, if any.
    pub leave: Option<Leave>,
    /// Whether the host slide-over is open over the grid.
    pub host_open: bool,
    /// Whether the join-table slide-over (paste a `tcpoker://` invite) is open over the grid.
    pub join_open: bool,
    /// The invite URI being typed into the join slide-over.
    pub join_uri: String,
    /// Live host-a-table config (drives both the slide-over and the standalone host screen).
    pub host: HostConfig,
    /// Which top-level screen is showing.
    pub screen: Screen,
    /// Monotonic animation clock (ms) accumulated by [`AppState::tick`]; drives pulse/spinner phases.
    pub clock_ms: f32,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

impl AppState {
    /// A fresh, empty state: no tables, no fabricated players. The app opens on the standalone host
    /// control room (the "host your first table" state); real tables appear only once you create one
    /// (and, once networking is wired, once peers actually join).
    pub fn new() -> Self {
        AppState {
            tables: Vec::new(),
            next_id: 1,
            focus_id: None,
            open_menu_id: None,
            leave: None,
            host_open: false,
            join_open: false,
            join_uri: String::new(),
            host: HostConfig::default(),
            screen: Screen::Host,
            clock_ms: 0.0,
        }
    }

    // -----------------------------------------------------------------------
    // Lookups / derived counts
    // -----------------------------------------------------------------------

    /// Look up a table by id.
    pub fn table(&self, id: u64) -> Option<&Table> {
        self.tables.iter().find(|t| t.id == id)
    }

    /// Mutable table by id.
    pub fn table_mut(&mut self, id: u64) -> Option<&mut Table> {
        self.tables.iter_mut().find(|t| t.id == id)
    }

    /// Number of active (live) tables — every table in free play.
    pub fn active_count(&self) -> usize {
        self.tables.len()
    }

    /// Count of tables currently awaiting your action.
    pub fn need_count(&self) -> usize {
        self.tables.iter().filter(|t| t.your_turn).count()
    }

    /// Total chips you have in play across every table (the toolbar "chips in play" readout).
    pub fn chips_in_play(&self) -> u64 {
        self.tables.iter().map(|t| t.hero_stack()).sum()
    }

    /// Grid column count per the README: 1 table → 1, 2–4 → 2, 5+ → 3.
    pub fn grid_cols(&self) -> usize {
        match self.tables.len() {
            0 | 1 => 1,
            2..=4 => 2,
            _ => 3,
        }
    }

    // -----------------------------------------------------------------------
    // Focus transitions
    // -----------------------------------------------------------------------

    /// Set `focus_id` to the first your-turn table (or clear it when none remain). Called after you
    /// act so focus follows live action.
    pub fn move_focus(&mut self) {
        self.focus_id = self.tables.iter().find(|t| t.your_turn).map(|t| t.id);
    }

    /// `Space` handler — cycle focus to the next your-turn table after the current one. With no
    /// current focus (or it no longer needs action) lands on the first your-turn table.
    pub fn focus_next(&mut self) {
        let turns: Vec<u64> = self
            .tables
            .iter()
            .filter(|t| t.your_turn)
            .map(|t| t.id)
            .collect();
        if turns.is_empty() {
            self.focus_id = None;
            return;
        }
        let idx = self
            .focus_id
            .and_then(|f| turns.iter().position(|&id| id == f));
        let next = match idx {
            Some(i) => turns[(i + 1) % turns.len()],
            None => turns[0],
        };
        self.focus_id = Some(next);
    }

    // -----------------------------------------------------------------------
    // Leave-table flow
    // -----------------------------------------------------------------------

    /// Open the calm leave-table confirm modal for `id` (also closes any open ⋯ menu).
    pub fn begin_leave(&mut self, id: u64) {
        self.open_menu_id = None;
        self.leave = Some(Leave {
            id,
            stage: LeaveStage::Confirm,
            elapsed_ms: 0,
        });
    }

    /// Advance the leave flow `Confirm` → `Processing`. The `Processing` → `Done` step is timed and
    /// driven by [`tick`]; this method only kicks off processing (the modal's confirm button).
    pub fn advance_leave(&mut self) {
        if let Some(l) = self.leave.as_mut() {
            match l.stage {
                LeaveStage::Confirm => {
                    l.stage = LeaveStage::Processing;
                    l.elapsed_ms = 0;
                }
                LeaveStage::Processing => {
                    l.stage = LeaveStage::Done;
                    l.elapsed_ms = 0;
                }
                LeaveStage::Done => {}
            }
        }
    }

    /// Cancel the leave flow ("Stay at table").
    pub fn cancel_leave(&mut self) {
        self.leave = None;
    }

    /// Finish the leave flow ("Done"): remove the tile, clear the modal, and fix focus + sim state so
    /// the active count and your-turn focus stay coherent after the grid reflows.
    pub fn finish_leave(&mut self) {
        let Some(l) = self.leave.take() else { return };
        let id = l.id;
        self.tables.retain(|t| t.id != id);
        if self.focus_id == Some(id) {
            self.move_focus();
        }
        if self.open_menu_id == Some(id) {
            self.open_menu_id = None;
        }
        // Empty grid → fall back to the standalone host control room (the "host your first table"
        // state), so the host screen + its live preview are a reachable state rather than dead code.
        if self.tables.is_empty() {
            self.screen = Screen::Host;
        }
    }

    // -----------------------------------------------------------------------
    // Host → create
    // -----------------------------------------------------------------------

    /// Build a placeholder free table from a host config and append it to the grid. The host takes
    /// seat 0 with [`HOST_START_STACK`] chips; every other seat is `Open`. Returns the new table's id.
    ///
    /// This is a PURE helper retained for tests / the standalone-host preview shape only. The LIVE app
    /// never appends a simulated table here: the single active table is projected each frame from the
    /// real connection's snapshot (see `crate::freeplay::project`).
    pub fn create_free_table(&mut self, cfg: &HostConfig) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let mut seats = Vec::with_capacity(cfg.seats);
        seats.push(Seat::filled("You", HOST_START_STACK));
        for _ in 1..cfg.seats {
            seats.push(Seat::Empty);
        }

        // No cards yet — a freshly hosted table has no hand in progress. Hole cards and the board
        // are populated from real game state once a hand is dealt.
        self.tables.push(Table {
            id,
            name: "New Table".to_string(),
            game: cfg.game,
            blinds: cfg.blinds.clone(),
            pot: 0,
            board: String::new(),
            hero: String::new(),
            seats,
            dealer: 0,
            act: 0,
            your_turn: false,
            to_call: 0,
            time_left: 0,
            invite_uri: None,
            reachability: None,
            can_check: false,
            can_call: false,
            can_bet: false,
            can_raise: false,
            can_all_in: false,
            min_bet: 0,
            min_raise_to: 0,
            max_to: 0,
            bet_to: 0,
        });
        id
    }

    /// Convenience: create a free table from the live `self.host` config and close the slide-over.
    pub fn create_from_host(&mut self) -> u64 {
        let cfg = self.host.clone();
        let id = self.create_free_table(&cfg);
        self.host_open = false;
        id
    }

    // -----------------------------------------------------------------------
    // Animation / sim clock
    // -----------------------------------------------------------------------

    /// Advance the UI-only animation/leave clocks by `dt_ms`: bump the animation clock and progress
    /// the leave `Processing` timer. The your-turn timer DISPLAY and its real auto-fold are owned by
    /// the app's connection layer (driven off the live snapshot), not this local model — displayed
    /// pot/stacks/board/turn come only from the projected real state.
    pub fn tick(&mut self, dt_ms: f32) {
        self.clock_ms += dt_ms;
        let dt = dt_ms.max(0.0) as u64;

        // Leave-table Processing → Done timer.
        if let Some(l) = self.leave.as_mut() {
            if l.stage == LeaveStage::Processing {
                l.elapsed_ms += dt;
                if l.elapsed_ms >= LEAVE_PROCESSING_MS {
                    l.stage = LeaveStage::Done;
                    l.elapsed_ms = 0;
                }
            }
        }
    }
}

/// Whether `s` looks like a pasteable table invite: a `tcpoker://…` URI (leading/trailing
/// whitespace ignored). Gates the join slide-over's `Join` CTA. Cheap syntactic check only — the
/// driver does the authoritative decode when it actually dials.
pub fn is_valid_invite(s: &str) -> bool {
    s.trim().starts_with("tcpoker://")
}

/// Parse a blinds display string (`"sb / bb"`, chips) into `(small_blind, big_blind)`, or `None` if
/// either side is missing or non-numeric. The inverse of the `"{sb} / {bb}"` display used by the
/// host config / tile header — the host threads this into the engine's posted blinds so the running
/// game matches the selected (and displayed) string instead of a hardcoded default.
pub fn parse_blinds(s: &str) -> Option<(u64, u64)> {
    let (sb, bb) = s.split_once('/')?;
    Some((sb.trim().parse().ok()?, bb.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_invite_accepts_tcpoker_uris() {
        assert!(is_valid_invite("tcpoker://x"));
        // Surrounding whitespace is trimmed before the prefix check.
        assert!(is_valid_invite("  tcpoker://x "));
        // Rejects empty and other schemes.
        assert!(!is_valid_invite(""));
        assert!(!is_valid_invite("http://x"));
    }

    #[test]
    fn parse_blinds_reads_sb_and_bb() {
        // The display formats used by the host config / presets round-trip.
        assert_eq!(parse_blinds("20 / 40"), Some((20, 40)));
        assert_eq!(parse_blinds("100 / 200"), Some((100, 200)));
        // Whitespace around the separator is optional.
        assert_eq!(parse_blinds("5/10"), Some((5, 10)));
        // Malformed strings yield None (the caller falls back to the engine default).
        assert_eq!(parse_blinds(""), None);
        assert_eq!(parse_blinds("20"), None);
        assert_eq!(parse_blinds("x / y"), None);
    }
}
