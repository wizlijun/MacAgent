//! AX-based window resize and restore.

use anyhow::{anyhow, Context, Result};
use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use macagent_core::ctrl_msg::{Viewport, WindowRect};
use std::ffi::c_void;

type AXUIElementRef = *const c_void;
type AXError = i32;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(value_type: u32, value_ptr: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, value_type: u32, value_ptr: *mut c_void) -> bool;
}

const K_AX_VALUE_TYPE_CG_SIZE: u32 = 1;
const K_AX_VALUE_TYPE_CG_POINT: u32 = 2;
const AX_ERROR_SUCCESS: AXError = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

fn cf_str(s: &str) -> CFString {
    CFString::new(s)
}

/// SAFETY: Caller guarantees `cf` is either null or a valid CF retained ref.
unsafe fn release(cf: CFTypeRef) {
    if !cf.is_null() {
        CFRelease(cf);
    }
}

const MIN_W: i32 = 400;
const MIN_H: i32 = 300;
const MAX_W: i32 = 1920;
const MAX_H: i32 = 1200;

/// Pure: compute target window size that aspect-matches the viewport.
pub fn compute_target_size(original: &WindowRect, viewport: Viewport) -> (i32, i32) {
    let vp_w = viewport.w.max(1) as f64;
    let vp_h = viewport.h.max(1) as f64;
    let mut w = original.w;
    let mut h = ((w as f64) * vp_h / vp_w).round() as i32;
    // Clamp width first
    if w > MAX_W {
        w = MAX_W;
        h = ((w as f64) * vp_h / vp_w).round() as i32;
    }
    if w < MIN_W {
        w = MIN_W;
        h = ((w as f64) * vp_h / vp_w).round() as i32;
    }
    // Then clamp height
    h = h.clamp(MIN_H, MAX_H);
    (w, h)
}

/// Find the AXUIElement for a window owned by `pid` whose frame best matches
/// `target` (smallest squared distance over x/y/w/h). The returned ref is
/// retained — caller is responsible for releasing it (currently we leak it,
/// acceptable for v0.1 since fit/restore happen at most a few times per app
/// lifetime).
///
/// SAFETY: Calls AX FFI; pid must correspond to a running app.
unsafe fn find_ax_window(pid: i32, target: &WindowRect) -> Result<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(anyhow!("AXUIElementCreateApplication returned null"));
    }

    let attr = cf_str("AXWindows");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value);
    release(app as CFTypeRef);
    if err != AX_ERROR_SUCCESS || value.is_null() {
        return Err(anyhow!("AXWindows fetch failed: err={err}"));
    }

    // value is a CFArray of AXUIElementRef. Walk it.
    let count = CFArrayGetCount(value as _);
    let mut best: AXUIElementRef = std::ptr::null();
    let mut best_dist = f64::MAX;

    for i in 0..count {
        let win = CFArrayGetValueAtIndex(value as _, i) as AXUIElementRef;
        let mut size_val: CFTypeRef = std::ptr::null();
        let mut pos_val: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(
            win,
            cf_str("AXSize").as_concrete_TypeRef(),
            &mut size_val,
        ) != AX_ERROR_SUCCESS
        {
            continue;
        }
        if AXUIElementCopyAttributeValue(
            win,
            cf_str("AXPosition").as_concrete_TypeRef(),
            &mut pos_val,
        ) != AX_ERROR_SUCCESS
        {
            release(size_val);
            continue;
        }
        let mut sz = CGSize { width: 0.0, height: 0.0 };
        let mut pt = CGPoint { x: 0.0, y: 0.0 };
        // Skip windows where AX size/position decode fails — otherwise a
        // zero-rect candidate can win the closest-bounds search.
        if !AXValueGetValue(size_val, K_AX_VALUE_TYPE_CG_SIZE, &mut sz as *mut _ as *mut c_void) {
            release(size_val);
            release(pos_val);
            continue;
        }
        if !AXValueGetValue(pos_val, K_AX_VALUE_TYPE_CG_POINT, &mut pt as *mut _ as *mut c_void) {
            release(size_val);
            release(pos_val);
            continue;
        }
        release(size_val);
        release(pos_val);

        let dx = pt.x - target.x as f64;
        let dy = pt.y - target.y as f64;
        let dw = sz.width - target.w as f64;
        let dh = sz.height - target.h as f64;
        let dist = dx * dx + dy * dy + dw * dw + dh * dh;
        if dist < best_dist {
            best_dist = dist;
            best = win;
        }
    }

    // Retain the chosen window before releasing the array — the array owns
    // the only reference to its children, so without this we'd UAF.
    if !best.is_null() {
        CFRetain(best as CFTypeRef);
    }
    release(value);
    if best.is_null() {
        Err(anyhow!("no AX window matched bounds"))
    } else {
        Ok(best)
    }
}

/// Read the current size + position of an AX window into a `WindowRect`.
///
/// SAFETY: `win` must be a valid retained AXUIElement.
unsafe fn read_window_rect(win: AXUIElementRef) -> Result<WindowRect> {
    let mut sz_val: CFTypeRef = std::ptr::null();
    let mut pos_val: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), &mut sz_val)
        != AX_ERROR_SUCCESS
    {
        return Err(anyhow!("AXSize get failed"));
    }
    if AXUIElementCopyAttributeValue(win, cf_str("AXPosition").as_concrete_TypeRef(), &mut pos_val)
        != AX_ERROR_SUCCESS
    {
        release(sz_val);
        return Err(anyhow!("AXPosition get failed"));
    }
    let mut sz = CGSize { width: 0.0, height: 0.0 };
    let mut pt = CGPoint { x: 0.0, y: 0.0 };
    // Fail loudly on decode error so callers don't cache a zero rect as `original_frame`.
    if !AXValueGetValue(sz_val, K_AX_VALUE_TYPE_CG_SIZE, &mut sz as *mut _ as *mut c_void) {
        release(sz_val);
        release(pos_val);
        return Err(anyhow!("AXValueGetValue(size) failed"));
    }
    if !AXValueGetValue(pos_val, K_AX_VALUE_TYPE_CG_POINT, &mut pt as *mut _ as *mut c_void) {
        release(sz_val);
        release(pos_val);
        return Err(anyhow!("AXValueGetValue(position) failed"));
    }
    release(sz_val);
    release(pos_val);
    Ok(WindowRect {
        x: pt.x as i32,
        y: pt.y as i32,
        w: sz.width as i32,
        h: sz.height as i32,
    })
}

/// Set `kAXSizeAttribute` on `win` to (w, h).
///
/// SAFETY: `win` must be a valid retained AXUIElement.
unsafe fn set_window_size(win: AXUIElementRef, w: i32, h: i32) -> Result<()> {
    let target_size = CGSize { width: w as f64, height: h as f64 };
    let target_value =
        AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &target_size as *const _ as *const c_void);
    if target_value.is_null() {
        return Err(anyhow!("AXValueCreate failed"));
    }
    let err =
        AXUIElementSetAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), target_value);
    release(target_value);
    if err != AX_ERROR_SUCCESS {
        return Err(anyhow!("AXSize set failed: err={err}"));
    }
    Ok(())
}

/// Fit the window to viewport aspect; return original frame for later restore.
pub fn fit(
    window_id: u32,
    owner_pid: i32,
    current: &WindowRect,
    viewport: Viewport,
) -> Result<WindowRect> {
    let _ = window_id; // window_id used by caller for logging; AX matches by frame
    // SAFETY: AX FFI calls — pid validated by caller; helpers handle CF ref lifetimes.
    let (win, original) = unsafe {
        let win = find_ax_window(owner_pid, current).context("find AX window")?;
        let original = read_window_rect(win).context("read original rect")?;
        (win, original)
    };
    let (tw, th) = compute_target_size(&original, viewport);
    // SAFETY: `win` is a valid retained AXUIElement from find_ax_window above.
    unsafe { set_window_size(win, tw, th)? };
    Ok(original)
}

/// Restore the window's original frame.
/// Heuristic: AX search picks the closest-bounds window — may pick a sibling
/// of the same app if its bounds happen to be closer to `original` than the
/// resized target. Acceptable for v0.1; M8 polish uses _AXUIElementGetWindow.
pub fn restore(window_id: u32, owner_pid: i32, original: &WindowRect) -> Result<()> {
    let _ = window_id;
    // SAFETY: AX FFI block — symmetric to `fit`; CF refs are released after set.
    unsafe {
        let win = find_ax_window(owner_pid, original).context("find AX window")?;
        let sz = CGSize { width: original.w as f64, height: original.h as f64 };
        let pt = CGPoint { x: original.x as f64, y: original.y as f64 };
        let sz_val = AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &sz as *const _ as *const c_void);
        let pt_val = AXValueCreate(K_AX_VALUE_TYPE_CG_POINT, &pt as *const _ as *const c_void);
        if sz_val.is_null() || pt_val.is_null() {
            release(sz_val);
            release(pt_val);
            return Err(anyhow!("AXValueCreate failed"));
        }
        let _ = AXUIElementSetAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), sz_val);
        let _ =
            AXUIElementSetAttributeValue(win, cf_str("AXPosition").as_concrete_TypeRef(), pt_val);
        release(sz_val);
        release(pt_val);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_fit_keeps_width_scales_height() {
        // 1440 wide window, viewport 393x760 (iPhone portrait)
        // target_h = 1440 * (760/393) ≈ 2785 → clamped to 1200
        let original = WindowRect { x: 0, y: 0, w: 1440, h: 900 };
        let viewport = Viewport { w: 393, h: 760 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1440);
        assert_eq!(h, 1200); // clamped
    }

    #[test]
    fn aspect_fit_landscape_viewport() {
        // viewport landscape 800x500
        let original = WindowRect { x: 0, y: 0, w: 1000, h: 800 };
        let viewport = Viewport { w: 800, h: 500 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1000);
        assert_eq!(h, 625); // 1000 * (500/800)
    }

    #[test]
    fn clamp_min_size() {
        // Tiny window
        let original = WindowRect { x: 0, y: 0, w: 200, h: 150 };
        let viewport = Viewport { w: 100, h: 100 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 400); // clamped up
        assert_eq!(h, 400); // 400 * 1.0 = 400
    }

    #[test]
    fn clamp_max_size() {
        let original = WindowRect { x: 0, y: 0, w: 3840, h: 2160 };
        let viewport = Viewport { w: 1024, h: 768 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1920); // clamped down to MAX_W
        assert_eq!(h, 1200); // 1920 * 0.75 = 1440 → clamped down to MAX_H
    }
}
