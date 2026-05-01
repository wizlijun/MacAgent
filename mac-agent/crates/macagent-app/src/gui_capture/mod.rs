//! GuiCapture — screen-recording permission check, window listing, and
//! per-window video streaming to a WebRTC track.
//!
//! Implementation status (M5.2):
//!   - check_permission: REAL  (CGPreflightScreenCaptureAccess)
//!   - list_windows:     REAL  (CGWindowListCopyWindowInfo)
//!   - supervise_existing / stream: STUB (dummy byte stream, ~30 fps)
//!   - encoder: STUB (replace with VTCompressionSession in M5.2.5)

pub mod perm;

mod encoder;
mod stream;
mod windows;

use anyhow::Result;
use macagent_core::ctrl_msg::WindowInfo;
use macagent_core::rtc_peer::VideoTrackHandle;
use std::sync::{Arc, Mutex};

type StreamEndedCb = Arc<dyn Fn(String, String) + Send + Sync>;

pub struct VideoConfig {
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub keyframe_interval_secs: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            bitrate_kbps: 3000,
            keyframe_interval_secs: 5,
        }
    }
}

pub struct GuiCapture {
    #[allow(dead_code)]
    config: VideoConfig,
    streams: Arc<stream::StreamManager>,
    on_ended: Mutex<Option<StreamEndedCb>>,
}

impl GuiCapture {
    pub fn new(config: VideoConfig) -> Self {
        Self {
            config,
            streams: Arc::new(stream::StreamManager::new()),
            on_ended: Mutex::new(None),
        }
    }

    /// Check whether the process has screen-recording permission.
    pub fn check_permission(&self) -> perm::PermissionStatus {
        perm::check()
    }

    /// List on-screen application windows. Real implementation using
    /// CGWindowListCopyWindowInfo (layer 0, has title + owner).
    pub async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        windows::list_windows()
    }

    /// Begin streaming `window_id` into `track` for the given supervision id.
    ///
    /// STUB (M5.2): starts a dummy byte-stream encoder. Replace with SCStream
    /// in M5.2.5.
    pub async fn supervise_existing(
        &self,
        sup_id: String,
        _window_id: u32,
        track: VideoTrackHandle,
    ) -> Result<()> {
        let track = Arc::new(track);
        self.streams.start(sup_id, track);
        Ok(())
    }

    /// Stop the stream for the given supervision id.
    pub async fn remove_supervised(&self, sup_id: &str) -> Result<()> {
        self.streams.stop(sup_id);
        Ok(())
    }

    /// Register a callback invoked when a stream ends unexpectedly.
    ///
    /// STUB (M5.2): never fires because the dummy encoder runs until
    /// `remove_supervised` is called. Wire up in M5.2.5 via SCStream delegate.
    pub fn on_stream_ended(&self, cb: impl Fn(String, String) + Send + Sync + 'static) {
        *self.on_ended.lock().unwrap() = Some(Arc::new(cb) as StreamEndedCb);
    }
}
