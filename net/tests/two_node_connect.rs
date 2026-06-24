//! Integration test: two real `Node`s connect over loopback transports and exchange opaque bytes
//! in both directions.
//!
//! Unlike the unit tests (pure URI/reachability logic) this spins up the full libp2p swarm twice
//! and drives real TCP (and, if available, QUIC) connections on `127.0.0.1`. It exercises the
//! crate strictly through its public API (`host`, `join`, `NodeHandle`, `NodeEvent`).
//!
//! Why we build the join URI ourselves instead of waiting for `NodeEvent::TableUriReady`:
//! `AddrBook::best_uri()` deliberately refuses to publish a loopback-only address, so on a machine
//! with no LAN/external interface the host would never emit a URI. Building the URI from the host's
//! concrete `127.0.0.1` listen address keeps the test fully self-contained and deterministic, and
//! pins it to loopback regardless of the sandbox's real interfaces.

use std::time::Duration;

use poker_net::{
    decode_table_uri, encode_table_uri, host, join, NodeEvent, NodeHandle,
};

use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};

/// Overall ceiling for the "they connected" phase.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling for each "a message arrived" phase. Gossipsub may need a heartbeat or two to mesh, so
/// the sender re-broadcasts on a ticker until then; give it generous head-room.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Collect the host's concrete loopback listen addresses (TCP first, QUIC if present) from its
/// event stream and turn them into a `tcpoker://` URI a guest can dial.
///
/// We prefer a TCP `/ip4/127.0.0.1/tcp/<port>` address because it is the most deterministic
/// transport in a sandbox, but we also carry a loopback QUIC address when one shows up so the
/// guest can pick whichever the environment supports.
async fn loopback_uri_from_host(
    events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    host_peer: PeerId,
) -> String {
    let mut tcp: Option<Multiaddr> = None;
    let mut quic: Option<Multiaddr> = None;

    tokio::time::timeout(CONNECT_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(NodeEvent::NewListenAddr(addr)) => {
                    if !is_loopback_v4(&addr) {
                        continue;
                    }
                    if is_quic(&addr) {
                        quic.get_or_insert_with(|| with_p2p(&addr, host_peer));
                    } else {
                        tcp.get_or_insert_with(|| with_p2p(&addr, host_peer));
                    }
                    // A TCP address is enough to proceed; don't wait forever for QUIC.
                    if tcp.is_some() {
                        break;
                    }
                }
                Some(_) => continue,
                None => panic!("host event stream closed before any loopback listen addr"),
            }
        }
    })
    .await
    .expect("host should report a loopback TCP listen address");

    let mut addrs = Vec::new();
    if let Some(t) = tcp {
        addrs.push(t);
    }
    if let Some(q) = quic {
        addrs.push(q);
    }
    assert!(
        !addrs.is_empty(),
        "expected at least one loopback dial address for the URI"
    );

    let uri = encode_table_uri(&addrs);
    // Sanity: the URI must round-trip through the public decoder the guest will use.
    let decoded = decode_table_uri(&uri).expect("encoded URI must decode");
    assert_eq!(decoded, addrs, "URI round-trip changed the addresses");
    uri
}

fn is_loopback_v4(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| match p {
        Protocol::Ip4(ip) => ip.is_loopback(),
        _ => false,
    })
}

fn is_quic(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::QuicV1))
}

/// Strip any trailing `/p2p` and append the given peer id, yielding a clean dialable address.
fn with_p2p(addr: &Multiaddr, peer: PeerId) -> Multiaddr {
    let mut a: Multiaddr = addr
        .iter()
        .filter(|p| !matches!(p, Protocol::P2p(_)))
        .collect();
    a.push(Protocol::P2p(peer));
    a
}

/// Drive an event stream until we observe a `PeerConnected(expected)` (or any peer connection if
/// `expected` is `None`). Times out with a clear message on failure.
async fn await_connected(
    events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    who: &str,
    expected: Option<PeerId>,
) {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(NodeEvent::PeerConnected(peer)) => match expected {
                    Some(want) if peer == want => break,
                    Some(_) => continue,
                    None => break,
                },
                Some(_) => continue,
                None => panic!("{who}: event stream closed before peer connected"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{who}: never reported a peer connection within {CONNECT_TIMEOUT:?}"));
}

/// Repeatedly broadcast `payload` from `sender` (on a ticker, to survive a not-yet-formed mesh)
/// while draining `sender_events`, until `receiver_events` yields a `Message` — then assert the
/// bytes and the source peer match. Returns once verified.
async fn assert_broadcast_delivered(
    sender: NodeHandle,
    sender_events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    receiver_events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    payload: Vec<u8>,
    label: &str,
) {
    let sender_peer = sender.local_peer_id();

    // Pump the sender: keep its swarm event loop alive (drain events) and re-broadcast until the
    // receiver acknowledges. The very first publish can fail with InsufficientPeers; that's fine.
    let pump_payload = payload.clone();
    let pump = tokio::spawn(async move {
        // Fire one immediately, then on a 200ms ticker. Right after connect, gossipsub has not
        // yet learned the peer's subscriptions, so the first publishes can fail with
        // NoPeersSubscribedToTopic / InsufficientPeers — that's expected; keep retrying until the
        // mesh forms (or the node is gone).
        let _ = sender.broadcast(pump_payload.clone()).await;
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if let Err(poker_net::NetError::NodeStopped) =
                sender.broadcast(pump_payload.clone()).await
            {
                break;
            }
        }
    });

    // We must also keep the SENDER's event stream drained, or its 256-slot event channel could
    // fill and stall the swarm. Do that concurrently with watching the receiver.
    let result = tokio::time::timeout(MESSAGE_TIMEOUT, async {
        loop {
            tokio::select! {
                // Drain sender events so its swarm never blocks on a full event channel.
                _ = sender_events.recv() => {}
                ev = receiver_events.recv() => {
                    match ev {
                        Some(NodeEvent::Message { from, data }) => {
                            return (from, data);
                        }
                        Some(_) => continue,
                        None => panic!("{label}: receiver event stream closed before message"),
                    }
                }
            }
        }
    })
    .await;

    pump.abort();

    let (from, data) = result
        .unwrap_or_else(|_| panic!("{label}: receiver never got the broadcast within {MESSAGE_TIMEOUT:?}"));

    assert_eq!(data, payload, "{label}: received bytes differ from sent bytes");
    assert_eq!(
        from, sender_peer,
        "{label}: message source peer id does not match the publisher"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_connect_and_exchange_bytes_both_ways() {
    // 1. Start the host on real loopback transports.
    let mut host_node = host(None).expect("host node should start");
    let host_peer = host_node.handle.local_peer_id();

    // 2. Learn the host's actual loopback listen address(es) and build the join URI from them.
    let uri = loopback_uri_from_host(&mut host_node.events, host_peer).await;
    assert!(uri.starts_with("tcpoker://"), "URI should use the scheme: {uri}");

    // 3. Start a second node that JOINS via that URI (this dials the host outbound).
    let mut guest_node = join(&uri, None).expect("guest node should start from URI");
    let guest_peer = guest_node.handle.local_peer_id();
    assert_ne!(host_peer, guest_peer, "two nodes must have distinct peer ids");

    // 4. Wait until BOTH sides report the connection (poll the streams, with a timeout).
    let host_events = &mut host_node.events;
    let guest_events = &mut guest_node.events;
    tokio::join!(
        await_connected(host_events, "host", Some(guest_peer)),
        await_connected(guest_events, "guest", Some(host_peer)),
    );

    // 5. Host -> guest: broadcast a payload and assert the guest receives the SAME bytes.
    assert_broadcast_delivered(
        host_node.handle.clone(),
        host_events,
        guest_events,
        b"the flop is queen-jack-ten".to_vec(),
        "host->guest",
    )
    .await;

    // 6. Guest -> host: a second message in the REVERSE direction.
    assert_broadcast_delivered(
        guest_node.handle.clone(),
        guest_events,
        host_events,
        b"i raise to twenty cents".to_vec(),
        "guest->host",
    )
    .await;

    // 7. Clean shutdown of both swarms.
    host_node.handle.shutdown().await;
    guest_node.handle.shutdown().await;
}
