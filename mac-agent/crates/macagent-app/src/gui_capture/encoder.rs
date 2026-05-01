//! Frame encoder trait + stub implementation.
//!
//! STUB: M5.2 ships a dummy generator that pushes arbitrary bytes every 33 ms.
//! Real VTCompressionSession encoding is deferred to M5.2.5.

use bytes::Bytes;
use macagent_core::rtc_peer::VideoTrackHandle;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::task::JoinHandle;

/// A running encoder task that feeds a video track.
pub struct EncoderTask {
    stop_flag: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl EncoderTask {
    pub fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // JoinHandle is dropped; tokio will clean up the task on next wakeup.
    }
}

/// Start the stub encoder: push a single dummy byte every 33 ms (~30 fps).
///
/// NOTE (M5.2 STUB): The byte stream is not valid H.264. webrtc-rs will
/// forward it to the peer; the iOS RTCMTLVideoView decoder will silently
/// fail on individual frames but will not crash. Pipeline reachability (track
/// negotiation + sample arrival) can be verified at M5.7 without valid video.
/// Replace with VTCompressionSession in M5.2.5.
pub fn start_stub(track: Arc<VideoTrackHandle>) -> EncoderTask {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = stop_flag.clone();

    let handle = tokio::spawn(async move {
        // A single repeated byte — minimal overhead; recognisably "dummy" in logs.
        let dummy: Bytes = Bytes::from_static(b"\x00");
        let frame_dur = Duration::from_millis(33);

        loop {
            if flag_clone.load(Ordering::Relaxed) {
                break;
            }
            let _ = track.push_sample(dummy.clone(), frame_dur).await;
            tokio::time::sleep(frame_dur).await;
        }
    });

    EncoderTask { stop_flag, handle }
}
