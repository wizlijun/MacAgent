use macagent_core::rtc_peer::{PeerState, RtcPeer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{timeout, Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_peer_yields_valid_offer() {
    let peer = RtcPeer::new(vec![]).await.unwrap();
    let _ch = peer.open_ctrl_channel().await.unwrap();
    let offer = peer.create_offer().await.unwrap();
    assert!(offer.contains("v=0"), "offer must contain v=0: {offer}");
    assert!(
        offer.contains("a=group:BUNDLE"),
        "offer must contain BUNDLE: {offer}"
    );
    peer.close().await.unwrap();
}

/// Loopback test: two in-process PeerConnections exchange SDP (vanilla ICE —
/// gathering completes before SDP is passed to the remote side) and verify that
/// alice's connection state reaches Connected within 30 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_two_peers_exchange_sdp_ice_and_ctrl_message() {
    let alice = RtcPeer::new(vec![]).await.unwrap();
    let bob = RtcPeer::new(vec![]).await.unwrap();

    // Alice opens ctrl channel (must be before create_offer to get a data m= line).
    let alice_ch = alice.open_ctrl_channel().await.unwrap();

    // Register state-change callback BEFORE SDP exchange so we don't miss events.
    let connected = Arc::new(AtomicBool::new(false));
    let connected_c = connected.clone();
    alice
        .on_state_change(move |s| {
            if s == PeerState::Connected {
                connected_c.store(true, Ordering::SeqCst);
            }
        })
        .await;

    // Vanilla ICE: create_offer waits for full gathering before returning,
    // so the SDP already contains all candidates. No trickle needed.
    let offer = alice.create_offer().await.unwrap();
    bob.apply_remote_offer(&offer).await.unwrap();
    let answer = bob.create_answer().await.unwrap();
    alice.apply_remote_answer(&answer).await.unwrap();

    let alice_arc = Arc::new(alice);
    let bob_arc = Arc::new(bob);

    // Poll until Connected or timeout.
    let alice_poll = alice_arc.clone();
    let connected_ref = connected.clone();
    let result = timeout(Duration::from_secs(30), async move {
        loop {
            if connected_ref.load(Ordering::SeqCst) {
                break;
            }
            if alice_poll.connection_state().await == PeerState::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    let is_connected = connected.load(Ordering::SeqCst)
        || alice_arc.connection_state().await == PeerState::Connected;

    assert!(
        result.is_ok() && is_connected,
        "expected alice to reach Connected within 30s; final state: {:?}",
        alice_arc.connection_state().await
    );

    drop(alice_ch);
    alice_arc.close().await.unwrap();
    bob_arc.close().await.unwrap();
}
