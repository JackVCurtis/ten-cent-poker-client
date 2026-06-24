//! Runtime smoke test: a host and an invitee exchange an opaque broadcast over loopback.
//! Lives in tests/ so it exercises only the public API.

use std::time::Duration;

use poker_net::{host, join, NodeEvent};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_and_join_exchange_bytes() {
    let mut host_node = host(None).expect("host starts");

    // Wait for the host to assemble a shareable URI from its listen addresses.
    let uri = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match host_node.events.recv().await {
                Some(NodeEvent::TableUriReady(uri)) => break uri,
                Some(_) => continue,
                None => panic!("host event stream closed before URI"),
            }
        }
    })
    .await
    .expect("host should produce a table URI");

    let mut guest_node = join(&uri, None).expect("guest starts from URI");

    // Once the guest connects, the host broadcasts; the guest should receive the bytes.
    let host_handle = host_node.handle.clone();
    let payload = b"deal me in".to_vec();

    // Drive both event loops until the guest sees the message (with periodic re-broadcasts,
    // since gossipsub needs the mesh to form first).
    let payload_for_task = payload.clone();
    let pump = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let _ = host_handle.broadcast(payload_for_task.clone()).await;
                }
                ev = host_node.events.recv() => {
                    if ev.is_none() { break; }
                }
            }
        }
    });

    let got = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match guest_node.events.recv().await {
                Some(NodeEvent::Message { data, .. }) => break data,
                Some(_) => continue,
                None => panic!("guest event stream closed"),
            }
        }
    })
    .await
    .expect("guest should receive the broadcast");

    assert_eq!(got, payload);

    pump.abort();
    host_node_shutdown(guest_node).await;
}

async fn host_node_shutdown(node: poker_net::Node) {
    node.handle.shutdown().await;
}
