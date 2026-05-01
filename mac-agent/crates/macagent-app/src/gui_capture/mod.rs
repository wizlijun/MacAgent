//! GuiCapture — screen-recording permission check, window listing, and
//! per-window video streaming to a WebRTC track.

pub mod perm;

mod annexb;
mod encoder;
mod stream;
#[allow(dead_code)]
mod thumbnail;
pub(crate) mod windows;

use anyhow::Result;
use macagent_core::ctrl_msg::WindowInfo;
use macagent_core::rtc_peer::VideoTrackHandle;
use std::sync::{Arc, Mutex};

use crate::input_injector::WindowFrame;

/// Live (pid, frame) for a supervised window. Used by InputInjector each event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputTarget {
    pub pid: i32,
    pub frame: WindowFrame,
}

type StreamEndedCb = Arc<dyn Fn(String, String) + Send + Sync>;

#[derive(Clone)]
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
    pub async fn supervise_existing(
        &self,
        sup_id: String,
        window_id: u32,
        track: VideoTrackHandle,
    ) -> Result<()> {
        let track = Arc::new(track);
        let end_rx = self
            .streams
            .start(sup_id.clone(), window_id, track, &self.config)?;
        let cb = self.on_ended.lock().unwrap().clone();
        let sid = sup_id;
        tokio::spawn(async move {
            if let Ok(reason) = end_rx.await {
                if let Some(cb) = cb {
                    cb(sid, reason);
                }
            }
        });
        Ok(())
    }

    /// Re-resolve the live (pid, frame) of the supervised window.
    /// Returns None if the window has gone (caller emits window_gone).
    pub fn lookup_target(&self, window_id: u32) -> Option<InputTarget> {
        let f = windows::find_window(window_id)?;
        Some(InputTarget {
            pid: f.pid,
            frame: WindowFrame { x: f.x, y: f.y, w: f.w, h: f.h },
        })
    }

    /// Stop the stream for the given supervision id.
    pub async fn remove_supervised(&self, sup_id: &str) -> Result<()> {
        self.streams.stop(sup_id);
        Ok(())
    }

    /// Register a callback invoked when a stream ends unexpectedly. Receives
    /// `(sup_id, reason)` where reason is e.g. `"stream_error: ..."`.
    pub fn on_stream_ended(&self, cb: impl Fn(String, String) + Send + Sync + 'static) {
        *self.on_ended.lock().unwrap() = Some(Arc::new(cb) as StreamEndedCb);
    }
}
