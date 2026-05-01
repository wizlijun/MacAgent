//! Active supervision stream management.
//!
//! STUB (M5.2.5.3 transitional): the previous dummy-byte encoder was removed in
//! favor of the real `H264Encoder`. The full SCStream wiring is M5.2.5.4; until
//! then `start` is a no-op placeholder that simply tracks supervised ids.

use macagent_core::rtc_peer::VideoTrackHandle;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub struct StreamManager {
    active: Mutex<HashSet<String>>,
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(HashSet::new()),
        }
    }

    pub fn start(&self, sup_id: String, _track: Arc<VideoTrackHandle>) {
        self.active.lock().unwrap().insert(sup_id);
    }

    pub fn stop(&self, sup_id: &str) -> bool {
        self.active.lock().unwrap().remove(sup_id)
    }
}
