//! Library surface for the Ten-Cent Poker app.
//!
//! The GUI modules are exposed here (behind the `gui` feature) so unit tests and the integration
//! tests in `app/tests/` can reach the free-play wiring — the pure projection
//! ([`freeplay::project`]), the live connection ([`freeplay::conn`]), and the render model
//! ([`freeplay::model`]) — plus the shared [`gui_state`] snapshot the driver fills. A bin-only crate
//! exposes no test-visible API, hence this thin lib. The headless build compiles no GUI code, so the
//! library is empty without the `gui` feature.

#[cfg(feature = "gui")]
pub mod freeplay;
#[cfg(feature = "gui")]
pub mod gui_state;
#[cfg(feature = "gui")]
pub mod ui;
