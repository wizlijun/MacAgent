//! ClipboardBridge: poll NSPasteboard via `pbpaste`, push changes to iOS via ctrl channel.
//! iOS → Mac writes are done via `pbcopy`.

use macagent_core::ctrl_msg::{ClipContent, ClipSource, CtrlPayload};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

const POLL_INTERVAL_MS: u64 = 500;
const MAX_BYTES: usize = 1024 * 1024; // 1 MB text limit

pub struct ClipboardBridge {
    last_hash: AtomicI64,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
}

impl ClipboardBridge {
    pub fn new(ctrl_tx: mpsc::UnboundedSender<CtrlPayload>) -> Self {
        Self {
            last_hash: AtomicI64::new(0),
            ctrl_tx,
        }
    }

    /// Runs the polling loop. Call via `tokio::spawn(bridge.clone().run_polling())`.
    pub async fn run_polling(self: Arc<Self>) {
        let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
        loop {
            tick.tick().await;
            if let Some(text) = read_pasteboard_changed(&self.last_hash) {
                if text.len() <= MAX_BYTES {
                    let _ = self.ctrl_tx.send(CtrlPayload::ClipboardSet {
                        source: ClipSource::Mac,
                        content: ClipContent::Text { data: text },
                    });
                }
            }
        }
    }

    /// Write iOS-originated content to NSPasteboard via `pbcopy`.
    /// On success, updates `last_hash` so the next poll tick doesn't bounce the
    /// just-written content back to iOS. On failure, leaves `last_hash` alone
    /// so a subsequent retry/poll can still surface the right content.
    pub fn write_remote(&self, content: &ClipContent) {
        match content {
            ClipContent::Text { data } => match pbcopy(data.as_bytes()) {
                Ok(()) => {
                    self.last_hash
                        .store(simple_hash(data) as i64, Ordering::SeqCst);
                }
                Err(e) => eprintln!("[clipboard] pbcopy failed: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `pbpaste`, return the text only if it changed since last call.
fn read_pasteboard_changed(last_hash: &AtomicI64) -> Option<String> {
    let out = Command::new("pbpaste").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    if text.is_empty() {
        return None;
    }
    let hash = simple_hash(&text) as i64;
    let prev = last_hash.swap(hash, Ordering::SeqCst);
    if prev == hash {
        None
    } else {
        Some(text)
    }
}

/// Write bytes to NSPasteboard via `pbcopy`.
fn pbcopy(bytes: &[u8]) -> std::io::Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(bytes)?;
    }
    child.wait()?;
    Ok(())
}

fn simple_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_hash_distinguishes_strings() {
        let h1 = simple_hash("hello");
        let h2 = simple_hash("world");
        let h3 = simple_hash("hello");
        assert_ne!(h1, h2, "different strings should have different hashes");
        assert_eq!(h1, h3, "same string should have same hash");
    }

    #[test]
    fn write_remote_updates_last_hash() {
        let (tx, _rx) = mpsc::unbounded_channel::<CtrlPayload>();
        let bridge = ClipboardBridge::new(tx);

        let content = ClipContent::Text {
            data: "test_clipboard".to_string(),
        };
        // Before write, hash is 0
        assert_eq!(bridge.last_hash.load(Ordering::SeqCst), 0);

        bridge.write_remote(&content);

        let expected = simple_hash("test_clipboard") as i64;
        assert_eq!(
            bridge.last_hash.load(Ordering::SeqCst),
            expected,
            "last_hash should be updated after write_remote"
        );
    }
}
