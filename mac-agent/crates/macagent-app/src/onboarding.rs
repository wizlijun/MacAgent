//! Permission status probes + open-settings shortcuts.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Denied,
    #[allow(dead_code)]
    Unknown,
}

/// Screen Recording status via CGPreflightScreenCaptureAccess (no prompt).
pub fn screen_recording_status() -> PermissionStatus {
    use crate::gui_capture::perm;
    match perm::check() {
        perm::PermissionStatus::Granted => PermissionStatus::Granted,
        perm::PermissionStatus::Denied => PermissionStatus::Denied,
        perm::PermissionStatus::NotDetermined => PermissionStatus::Denied,
    }
}

/// Accessibility status via AXIsProcessTrusted (no prompt).
pub fn accessibility_status() -> PermissionStatus {
    if ax_is_process_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

/// Open System Settings → Privacy & Security → Screen Recording pane.
pub fn open_screen_recording_settings() {
    open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
}

/// Open System Settings → Privacy & Security → Accessibility pane.
pub fn open_accessibility_settings() {
    open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
}

fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// FFI wrapper for AXIsProcessTrusted from ApplicationServices framework.
fn ax_is_process_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // SAFETY: C function with no args/state; safe to call any thread.
    unsafe { AXIsProcessTrusted() }
}
