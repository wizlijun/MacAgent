//! Screen capture permission check via CGPreflightScreenCaptureAccess.

use core_graphics::access::ScreenCaptureAccess;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Denied,
    /// macOS returns false both for Denied and NotDetermined; we surface this
    /// distinction by checking whether the app has ever been prompted (not
    /// available without TCC DB access), so we collapse them into Denied.
    NotDetermined,
}

/// Returns the current screen-recording permission status without prompting.
pub fn check() -> PermissionStatus {
    if ScreenCaptureAccess.preflight() {
        PermissionStatus::Granted
    } else {
        // CGPreflightScreenCaptureAccess does not distinguish Denied from
        // NotDetermined at the API level; surface NotDetermined so callers
        // can decide whether to call CGRequestScreenCaptureAccess.
        PermissionStatus::NotDetermined
    }
}
