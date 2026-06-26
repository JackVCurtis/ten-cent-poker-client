//! End-to-end lifecycle test for the free-play connection layer: a real host [`TableConn`] and a
//! real guest [`TableConn`] play a networked game over loopback, mirroring the in-process harness in
//! `protocol/tests/two_peer_game.rs` but driven through the GUI's [`TableConn`] wrapper (shared
//! `GuiState` snapshot + driver task) instead of the bare `run_host`/`run_guest` drivers.
//!
//! Gated behind the `gui` feature so the headless build skips it entirely.

#![cfg(feature = "gui")]

use std::time::{Duration, Instant};

use eframe::egui;

use poker_app::freeplay::conn::TableConn;
use poker_app::gui_state::Conn;
use poker_protocol::HostOptions;

/// A host and a guest, wired through `TableConn`, must reach live play (or a completed hand) over
/// loopback. The host's snapshot surfaces the table URI; the guest joins it. mDNS is off so
/// concurrent loopback test tables do not auto-discover each other; the placeholder deal keeps it fast.
#[test]
fn host_and_guest_reach_play_through_table_conn() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build tokio runtime");

    // Headless egui context: TableConn only calls `request_repaint` on it.
    let ctx = egui::Context::default();

    let opts = HostOptions {
        hands: 2,
        mental: false,
        enable_mdns: false,
        min_players: 2,
        ..HostOptions::default()
    };

    let host = TableConn::host(&rt, ctx.clone(), opts);

    // Poll the host snapshot for the shareable URI (~30s).
    let uri = poll(Duration::from_secs(30), || {
        host.snapshot().table_uri.clone()
    })
    .expect("host produced a table URI within 30s");
    assert!(
        uri.starts_with("tcpoker://"),
        "URI looks like an invite: {uri}"
    );

    let guest = TableConn::join(&rt, ctx.clone(), uri);

    // Poll until BOTH peers reach live play, OR have a completed hand (~120s).
    let reached = poll(Duration::from_secs(120), || {
        let h = host.snapshot();
        let g = guest.snapshot();
        (played(&h.conn, h.last_result.is_some()) && played(&g.conn, g.last_result.is_some()))
            .then_some(())
    });
    assert!(
        reached.is_some(),
        "host and guest did not reach play within 120s"
    );

    host.abort();
    guest.abort();
}

/// True once a peer is in (or past) live play: `Playing`/`GameOver`, or it has settled a hand.
fn played(conn: &Conn, has_result: bool) -> bool {
    matches!(conn, Conn::Playing | Conn::GameOver) || has_result
}

/// Poll `f` every 100ms until it returns `Some` or `timeout` elapses.
fn poll<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
