//! M7 GUI app launcher (NSWorkspace + window discovery). Distinct from
//! launcher.rs which is the M3 PTY producer launcher.

use anyhow::{anyhow, Context, Result};
use std::time::{Duration, Instant};

const ALLOWED_BUNDLES: &[&str] = &[
    "com.openai.chat",
    "com.anthropic.claude",
    "com.google.Chrome",
];

/// Returns true if `bundle_id` is on the M7 launch whitelist.
pub fn is_allowed(bundle_id: &str) -> bool {
    ALLOWED_BUNDLES.contains(&bundle_id)
}

/// Launch the whitelisted app and poll up to 5s for its first usable window.
pub async fn launch_and_find_window(bundle_id: &str) -> Result<(i32, u32)> {
    if !is_allowed(bundle_id) {
        return Err(anyhow!("bundle_not_allowed"));
    }
    let pid = open_application(bundle_id).context("open_application")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(wid) = find_first_window_for_pid(pid) {
            return Ok((pid, wid));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(anyhow!("launch_timeout"))
}

/// Resolve the bundle URL, openURL it, then poll runningApplications for the pid.
fn open_application(bundle_id: &str) -> Result<i32> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    let workspace = NSWorkspace::sharedWorkspace();
    let bid = NSString::from_str(bundle_id);
    let url = workspace
        .URLForApplicationWithBundleIdentifier(&bid)
        .ok_or_else(|| anyhow!("no app for bundle {bundle_id}"))?;
    // Use sync openURL: (deprecated but functional on macOS 14+); the async
    // openURL:configuration:completionHandler: variant requires block2 plumbing.
    if !workspace.openURL(&url) {
        return Err(anyhow!("openURL failed for {bundle_id}"));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let apps = workspace.runningApplications();
        for app in apps.iter() {
            if let Some(b) = app.bundleIdentifier() {
                if b.to_string() == bundle_id {
                    return Ok(app.processIdentifier());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(anyhow!("running app not found after open"))
}

/// Pick the first on-screen window (≥100×100, non-empty title) owned by `pid`.
fn find_first_window_for_pid(pid: i32) -> Option<u32> {
    // WindowInfo doesn't carry owner pid, so re-resolve each candidate via find_window.
    for w in crate::gui_capture::windows::list_windows().ok()? {
        if w.title.is_empty() || (w.width * w.height) <= 100 * 100 {
            continue;
        }
        if let Some(fw) = crate::gui_capture::windows::find_window(w.window_id) {
            if fw.pid == pid {
                return Some(w.window_id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_known_bundles() {
        assert!(is_allowed("com.openai.chat"));
        assert!(is_allowed("com.anthropic.claude"));
        assert!(is_allowed("com.google.Chrome"));
    }

    #[test]
    fn whitelist_rejects_others() {
        assert!(!is_allowed("com.apple.systempreferences"));
        assert!(!is_allowed(""));
        assert!(!is_allowed("com.evil.malware"));
    }
}
