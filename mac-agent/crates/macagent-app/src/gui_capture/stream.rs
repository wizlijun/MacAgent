//! Active supervision stream management.
//!
//! STUB: M5.2 — runs dummy encoder per supervised window.
//! Real ScreenCaptureKit SCStream integration is deferred to M5.2.5.

use crate::gui_capture::encoder::{self, EncoderTask};
use macagent_core::rtc_peer::VideoTrackHandle;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct StreamManager {
    /// sup_id -> running encoder task.
    tasks: Mutex<HashMap<String, EncoderTask>>,
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Start a stub stream for `sup_id` feeding `track`.
    /// If a stream for the same `sup_id` already exists it is stopped first.
    pub fn start(&self, sup_id: String, track: Arc<VideoTrackHandle>) {
        let task = encoder::start_stub(track);
        let mut guard = self.tasks.lock().unwrap();
        if let Some(old) = guard.remove(&sup_id) {
            old.stop();
        }
        guard.insert(sup_id, task);
    }

    /// Stop and remove the stream for `sup_id`. Returns true if found.
    pub fn stop(&self, sup_id: &str) -> bool {
        let mut guard = self.tasks.lock().unwrap();
        if let Some(task) = guard.remove(sup_id) {
            task.stop();
            true
        } else {
            false
        }
    }
}
