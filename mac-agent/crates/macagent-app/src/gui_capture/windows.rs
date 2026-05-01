//! Real window listing via CGWindowListCopyWindowInfo.

use anyhow::Result;
use core_foundation::array::{CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use macagent_core::ctrl_msg::WindowInfo;
use std::ffi::c_void;

/// List on-screen, non-desktop application windows that have both an owner
/// name and a non-empty title, at layer 0 (normal app windows).
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let raw = match copy_window_info(option, kCGNullWindowID) {
        Some(a) => a,
        None => return Ok(vec![]),
    };

    let count = raw.len();
    let arr_ref: CFArrayRef = raw.as_concrete_TypeRef();
    let mut out = Vec::new();

    for i in 0..count {
        let ptr = unsafe { CFArrayGetValueAtIndex(arr_ref, i) };
        if ptr.is_null() {
            continue;
        }
        // Wrap as untyped CFDictionary without extra retain.
        let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(ptr as CFDictionaryRef) };

        // --- window_id ---
        let window_id = match dict_number_u32(&dict, unsafe { kCGWindowNumber }) {
            Some(v) => v,
            None => continue,
        };

        // --- layer: keep only layer 0 (normal app windows) ---
        let layer = dict_number_i32(&dict, unsafe { kCGWindowLayer }).unwrap_or(1);
        if layer != 0 {
            continue;
        }

        // --- owner (app) name — required ---
        let app_name = match dict_string(&dict, unsafe { kCGWindowOwnerName }) {
            Some(s) => s,
            None => continue,
        };

        // --- window title — required and non-empty ---
        let title = match dict_string(&dict, unsafe { kCGWindowName }) {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };

        // --- on_screen ---
        let on_screen = dict_bool(&dict, unsafe { kCGWindowIsOnscreen }).unwrap_or(false);

        // --- bounds ---
        let (width, height) = dict_bounds(&dict).unwrap_or((0, 0));

        out.push(WindowInfo {
            window_id,
            app_name,
            bundle_id: None, // CGWindowListCopyWindowInfo does not expose bundle id
            title,
            width,
            height,
            on_screen,
            is_minimized: false, // Not available without Accessibility API
        });
    }

    Ok(out)
}

/// Live window-target lookup: pid + global-screen frame for a single window_id.
/// Returns None if the window has gone away (caller should treat as `window_gone`).
pub struct FoundWindow {
    pub pid: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn find_window(window_id: u32) -> Option<FoundWindow> {
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let raw = copy_window_info(option, kCGNullWindowID)?;
    let count = raw.len();
    let arr_ref: CFArrayRef = raw.as_concrete_TypeRef();
    for i in 0..count {
        let ptr = unsafe { CFArrayGetValueAtIndex(arr_ref, i) };
        if ptr.is_null() {
            continue;
        }
        let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(ptr as CFDictionaryRef) };
        let wid = dict_number_u32(&dict, unsafe { kCGWindowNumber })?;
        if wid != window_id {
            continue;
        }
        let pid = dict_number_i32(&dict, unsafe { kCGWindowOwnerPID })?;
        let (x, y, w, h) = dict_bounds_full(&dict)?;
        return Some(FoundWindow { pid, x, y, w, h });
    }
    None
}

// ---------------------------------------------------------------------------
// Low-level helpers: look up typed values in an untyped CFDictionary.
// All key statics are CFStringRef exported from CoreGraphics.
// ---------------------------------------------------------------------------

/// Look up a raw value pointer in the dictionary by CFStringRef key.
fn dict_value(dict: &CFDictionary, key_ref: CFStringRef) -> Option<*const c_void> {
    // Wrap the key as CFString without retaining (it's a static symbol).
    let key: CFString = unsafe { TCFType::wrap_under_get_rule(key_ref) };
    let mut value: *const c_void = std::ptr::null();
    let found = unsafe {
        CFDictionaryGetValueIfPresent(dict.as_concrete_TypeRef(), key.as_CFTypeRef(), &mut value)
    };
    if found != 0 && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

fn dict_number_u32(dict: &CFDictionary, key_ref: CFStringRef) -> Option<u32> {
    let v = dict_value(dict, key_ref)?;
    let num: CFNumber = unsafe { TCFType::wrap_under_get_rule(v as CFNumberRef) };
    num.to_i64().map(|n| n as u32)
}

fn dict_number_i32(dict: &CFDictionary, key_ref: CFStringRef) -> Option<i32> {
    let v = dict_value(dict, key_ref)?;
    let num: CFNumber = unsafe { TCFType::wrap_under_get_rule(v as CFNumberRef) };
    num.to_i32()
}

fn dict_string(dict: &CFDictionary, key_ref: CFStringRef) -> Option<String> {
    let v = dict_value(dict, key_ref)?;
    let s: CFString = unsafe { TCFType::wrap_under_get_rule(v as CFStringRef) };
    Some(s.to_string())
}

fn dict_bool(dict: &CFDictionary, key_ref: CFStringRef) -> Option<bool> {
    use core_foundation::boolean::CFBooleanRef;
    let v = dict_value(dict, key_ref)?;
    let b: CFBoolean = unsafe { TCFType::wrap_under_get_rule(v as CFBooleanRef) };
    Some(b == CFBoolean::true_value())
}

/// Extract (x, y, w, h) ints from the CGWindowBounds sub-dictionary.
fn dict_bounds_full(dict: &CFDictionary) -> Option<(i32, i32, i32, i32)> {
    let v = dict_value(dict, unsafe { kCGWindowBounds })?;
    let bounds: CFDictionary = unsafe { TCFType::wrap_under_get_rule(v as CFDictionaryRef) };
    let read = |name: &str| -> Option<i32> {
        let key = CFString::new(name);
        let mut out: *const c_void = std::ptr::null();
        let f = unsafe {
            CFDictionaryGetValueIfPresent(bounds.as_concrete_TypeRef(), key.as_CFTypeRef(), &mut out)
        };
        if f == 0 || out.is_null() {
            return None;
        }
        let n: CFNumber = unsafe { TCFType::wrap_under_get_rule(out as CFNumberRef) };
        n.to_i64().map(|v| v as i32)
    };
    Some((read("X")?, read("Y")?, read("Width")?, read("Height")?))
}

/// Extract (width, height) from the CGWindowBounds sub-dictionary.
fn dict_bounds(dict: &CFDictionary) -> Option<(u32, u32)> {
    let v = dict_value(dict, unsafe { kCGWindowBounds })?;
    let bounds: CFDictionary = unsafe { TCFType::wrap_under_get_rule(v as CFDictionaryRef) };
    let w_key = CFString::new("Width");
    let h_key = CFString::new("Height");
    let mut wv: *const c_void = std::ptr::null();
    let mut hv: *const c_void = std::ptr::null();
    let wf = unsafe {
        CFDictionaryGetValueIfPresent(bounds.as_concrete_TypeRef(), w_key.as_CFTypeRef(), &mut wv)
    };
    let hf = unsafe {
        CFDictionaryGetValueIfPresent(bounds.as_concrete_TypeRef(), h_key.as_CFTypeRef(), &mut hv)
    };
    if wf == 0 || hf == 0 || wv.is_null() || hv.is_null() {
        return None;
    }
    let w: CFNumber = unsafe { TCFType::wrap_under_get_rule(wv as CFNumberRef) };
    let h: CFNumber = unsafe { TCFType::wrap_under_get_rule(hv as CFNumberRef) };
    Some((w.to_i64()? as u32, h.to_i64()? as u32))
}
