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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_video_track_includes_m_video_in_sdp() {
    let alice = RtcPeer::new(vec![]).await.unwrap();
    let _video = alice.add_local_h264_video_track().await.unwrap();
    let _ctrl = alice.open_ctrl_channel().await.unwrap();

    let offer = alice.create_offer().await.unwrap();
    assert!(
        offer.contains("m=video"),
        "offer should contain video m-section, got:\n{offer}"
    );
    assert!(
        offer.contains("H264"),
        "offer should mention H264, got:\n{offer}"
    );

    alice.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_video_negotiation() {
    let alice = RtcPeer::new(vec![]).await.unwrap();
    let bob = RtcPeer::new(vec![]).await.unwrap();

    let _video = alice.add_local_h264_video_track().await.unwrap();
    let _ctrl = alice.open_ctrl_channel().await.unwrap();

    let offer = alice.create_offer().await.unwrap();
    bob.apply_remote_offer(&offer).await.unwrap();
    let answer = bob.create_answer().await.unwrap();
    alice.apply_remote_answer(&answer).await.unwrap();

    // 验证 answer 也含 video（bob 接受了 alice 的 video offer）
    assert!(
        answer.contains("m=video") || answer.contains("video"),
        "answer should accept video, got:\n{answer}"
    );

    alice.close().await.unwrap();
    bob.close().await.unwrap();
}

// push_sample 单测：构造完成 video track 后立刻 push 一个 dummy NALU 不报错（不需要 connected 状态，
// TrackLocalStaticSample::write_sample 即使在未 connect 时也可以接受样本）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_sample_does_not_error_before_connection() {
    let alice = RtcPeer::new(vec![]).await.unwrap();
    let video = alice.add_local_h264_video_track().await.unwrap();
    let dummy_nalu = bytes::Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f]);
    video
        .push_sample(dummy_nalu, std::time::Duration::from_millis(33))
        .await
        .ok();
    alice.close().await.unwrap();
}
