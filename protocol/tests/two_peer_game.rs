//! End-to-end driver test: a real host node and a real guest node play a networked game over
//! loopback, exercising `run_host` / `run_guest`, the gossipsub mesh, the NoSubscribersYet
//! retry, and the replicated table. Both peers must agree on every hand outcome.
//!
//! This is the integration shape M5 / integration-test authors target.

use poker_protocol::{run_guest_with_config, run_host, CheckFoldBot, HostOptions};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_and_guest_play_three_hands_and_agree() {
    let opts = HostOptions {
        hands: 3,
        starting_stack: 1000,
        small_blind: 5,
        big_blind: 10,
        min_players: 2,
        keypair: None,
        // mDNS OFF in tests: dial explicitly via the URI; no cross-test loopback auto-discovery.
        enable_mdns: false,
        // Fast multi-hand agreement check: use the placeholder deal (the trustless deal is
        // exercised end-to-end over the wire in `networked_hand.rs`).
        mental: false,
        listen_port: None,
    };

    // Host runs in a task; it surfaces the URI via a oneshot so the guest can join.
    let (uri_tx, uri_rx) = tokio::sync::oneshot::channel::<String>();
    let mut uri_tx = Some(uri_tx);
    let host = tokio::spawn(async move {
        run_host(CheckFoldBot, opts, move |uri| {
            if let Some(tx) = uri_tx.take() {
                let _ = tx.send(uri.to_string());
            }
        })
        .await
    });

    // Wait for the URI, then join.
    let uri = tokio::time::timeout(std::time::Duration::from_secs(30), uri_rx)
        .await
        .expect("host produced a URI in time")
        .expect("uri channel ok");

    let guest =
        tokio::spawn(async move { run_guest_with_config(&uri, CheckFoldBot, None, false).await });

    let host_report = tokio::time::timeout(std::time::Duration::from_secs(120), host)
        .await
        .expect("host finished in time")
        .expect("host task ok")
        .expect("host game ok");

    let guest_report = tokio::time::timeout(std::time::Duration::from_secs(120), guest)
        .await
        .expect("guest finished in time")
        .expect("guest task ok")
        .expect("guest game ok");

    assert_eq!(host_report.hands.len(), 3, "host played 3 hands");
    assert_eq!(
        host_report.hands.len(),
        guest_report.hands.len(),
        "both peers saw the same number of hands"
    );

    // Every hand's outcome must match between the two peers (replicated determinism).
    for (h, g) in host_report.hands.iter().zip(guest_report.hands.iter()) {
        assert_eq!(
            h.outcome, g.outcome,
            "host and guest disagree on hand #{}",
            h.outcome.hand_no
        );
        // Chips conserved each hand.
        assert_eq!(h.outcome.deltas.iter().sum::<i64>(), 0);
    }
}
