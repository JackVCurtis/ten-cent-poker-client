//! Integration test: the RELIABLE deal channel (libp2p request-response) delivers a payload
//! point-to-point and carries the original author verbatim, and a send to an unconnected peer
//! surfaces as a `DealFailed` error rather than hanging.
//!
//! This exercises `NodeHandle::send_deal` / `NodeEvent::DealReceived` end-to-end over real loopback
//! transports — the primitive `poker-protocol`'s driver builds the trustless-deal fan-out on.

use std::time::Duration;

use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};
use poker_net::{
    decode_table_uri, encode_table_uri, host, join, NetError, NodeEvent, NodeHandle,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEAL_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_deal_delivers_and_carries_author_both_ways() {
    let mut host_node = host(None).expect("host node should start");
    let host_peer = host_node.handle.local_peer_id();

    let uri = loopback_uri_from_host(&mut host_node.events, host_peer).await;
    let mut guest_node = join(&uri, None).expect("guest node should start from URI");
    let guest_peer = guest_node.handle.local_peer_id();

    let host_events = &mut host_node.events;
    let guest_events = &mut guest_node.events;
    tokio::join!(
        await_connected(host_events, "host", Some(guest_peer)),
        await_connected(guest_events, "guest", Some(host_peer)),
    );

    // Host -> guest. Use a DISTINCT random author to prove it is carried verbatim and not
    // overwritten by the transport sender (`relayed_by`).
    let author_a = PeerId::random();
    assert_deal_delivered(
        host_node.handle.clone(),
        host_events,
        guest_events,
        guest_peer,
        author_a,
        host_peer,
        b"reveal token batch for seat 2".to_vec(),
        "host->guest",
    )
    .await;

    // Guest -> host, reverse direction, another distinct author.
    let author_b = PeerId::random();
    assert_deal_delivered(
        guest_node.handle.clone(),
        guest_events,
        host_events,
        host_peer,
        author_b,
        guest_peer,
        b"shuffle proof for turn 1".to_vec(),
        "guest->host",
    )
    .await;

    host_node.handle.shutdown().await;
    guest_node.handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_deal_to_unconnected_peer_fails() {
    let poker_net::Node {
        handle,
        mut events,
    } = host(None).expect("host node should start");
    let author = handle.local_peer_id();
    let stranger = PeerId::random(); // never connected, no known address

    // Drain events so the swarm never stalls while we await the outbound failure.
    let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });

    let res = tokio::time::timeout(
        DEAL_TIMEOUT,
        handle.send_deal(stranger, author, b"nobody home".to_vec()),
    )
    .await
    .expect("send_deal to an unconnected peer should resolve, not hang");

    assert!(
        matches!(res, Err(NetError::DealFailed { .. })),
        "expected DealFailed, got {res:?}"
    );

    drain.abort();
    handle.shutdown().await;
}

/// Send one deal from `sender` to `target` and assert the receiver observes it with the carried
/// `expected_author` and `expected_relayed_by`, and the same bytes. Drains the sender's events
/// concurrently so its event channel never backs up.
#[allow(clippy::too_many_arguments)]
async fn assert_deal_delivered(
    sender: NodeHandle,
    sender_events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    receiver_events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    target: PeerId,
    author: PeerId,
    expected_relayed_by: PeerId,
    payload: Vec<u8>,
    label: &str,
) {
    let recv = tokio::time::timeout(DEAL_TIMEOUT, async {
        loop {
            tokio::select! {
                _ = sender_events.recv() => {}
                ev = receiver_events.recv() => match ev {
                    Some(NodeEvent::DealReceived { author, relayed_by, data }) => {
                        return (author, relayed_by, data);
                    }
                    Some(_) => {}
                    None => panic!("{label}: receiver stream closed before deal"),
                }
            }
        }
    });

    let send = sender.send_deal(target, author, payload.clone());
    let (send_res, recv_res) = tokio::join!(send, recv);

    send_res.unwrap_or_else(|e| panic!("{label}: send_deal failed: {e}"));
    let (got_author, got_relayed_by, got_data) =
        recv_res.unwrap_or_else(|_| panic!("{label}: receiver never got the deal in {DEAL_TIMEOUT:?}"));

    assert_eq!(got_data, payload, "{label}: payload bytes differ");
    assert_eq!(got_author, author, "{label}: carried author differs");
    assert_eq!(
        got_relayed_by, expected_relayed_by,
        "{label}: relayed_by should be the transport sender"
    );
}

// ---- helpers (mirrors two_node_connect.rs; tests are separate compilation units) ----

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
    let uri = encode_table_uri(&addrs);
    assert_eq!(decode_table_uri(&uri).unwrap(), addrs);
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

fn with_p2p(addr: &Multiaddr, peer: PeerId) -> Multiaddr {
    let mut a: Multiaddr = addr
        .iter()
        .filter(|p| !matches!(p, Protocol::P2p(_)))
        .collect();
    a.push(Protocol::P2p(peer));
    a
}

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
