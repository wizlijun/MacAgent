# M7 · Launch + Multi-supervise + Window Adaptation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship M7 — let an iOS user launch whitelisted Mac apps remotely (`supervise_launch`), register up to 8 supervised windows with thumbnails for armed entries, switch active stream in ≤200ms, and have the supervised window auto-resize to the iOS viewport via Accessibility (`fit_window`).

**Architecture:** New Mac modules `launcher_m7.rs` (NSWorkspace open + window detect) + `window_fitter.rs` (AXUIElement size set + restore). `SupervisionRouter` upgraded to multi-entry registry (HashMap<sup_id, SupervisionEntry>, max 8) with stop-old + start-new switch flow. `gui_capture/stream.rs` captures the last CVPixelBuffer in FrameSink and `demote_to_armed()` encodes it to JPEG (CoreGraphics ImageIO, 256×192 @ Q70 → base64). iOS replaces `WindowListView` with `SupervisionGrid` (LazyVGrid, 2 col on iPhone / 3 col on iPad), adds `LaunchAppSheet` for whitelist picker and `ViewportTracker` modifier for rotation/Stage-Manager geometry reporting.

**Tech Stack:** Rust (`objc2-app-kit` for NSWorkspace, ApplicationServices for AX, CoreGraphics ImageIO for JPEG), Swift (SwiftUI LazyVGrid + GeometryReader, Combine debounce), webrtc-rs (existing video track reused across switches).

**Spec:** `docs/superpowers/specs/2026-05-01-m7-launch-multi-supervise-design.md` (commit `e697ba7`).

---

## File Structure

**Mac (Rust)** — new + modified:

| Path | Responsibility |
|---|---|
| `mac-agent/crates/macagent-core/src/ctrl_msg.rs` | Add 4 CtrlPayload variants + `Viewport`, `WindowRect`, `SupStatus` types + extend `SupervisionEntry` with `status` / `original_frame` / `thumb_jpeg_b64` |
| `mac-agent/crates/macagent-app/src/launcher_m7.rs` (NEW) | Whitelist + `launch_and_find_window(bundle_id) -> Result<(pid, window_id)>` |
| `mac-agent/crates/macagent-app/src/window_fitter.rs` (NEW) | `fit(window_id, pid, viewport) -> Result<WindowRect>` + `restore(window_id, pid, original) -> Result<()>` |
| `mac-agent/crates/macagent-app/src/gui_capture/stream.rs` | FrameSink also writes last `CVPixelBuffer` to `Arc<Mutex<Option<...>>>` so `demote_to_armed()` can grab final frame |
| `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` | New `demote_to_armed(sup_id) -> Option<String>` returning JPEG-base64 thumbnail |
| `mac-agent/crates/macagent-app/src/gui_capture/thumbnail.rs` (NEW) | `cvpixelbuffer_to_jpeg_base64(pb, max_w, max_h, quality) -> Result<String>` (CoreGraphics ImageIO) |
| `mac-agent/crates/macagent-app/src/supervision_router.rs` | Replace `Option<ActiveSupervision>` with `HashMap<String, SupervisionEntry>` + `active_sup: Option<String>`; new `handle_supervise_launch / handle_switch_active / handle_viewport_changed`; `set_active` atomic flow |
| `mac-agent/crates/macagent-app/src/rtc_glue.rs` | Drainer dispatches 4 new ctrl variants to supervision router |
| `mac-agent/crates/macagent-app/src/ui.rs` | Build Launcher + WindowFitter Arcs, pass to SupervisionRouter::new |
| `mac-agent/Cargo.toml` + `crates/macagent-app/Cargo.toml` | Possibly add `objc2-image-io` if CoreGraphics ImageIO not reachable from existing deps (decided in Task M7.5) |

**iOS (Swift)** — new:

| Path | Responsibility |
|---|---|
| `ios-app/MacIOSWorkspace/Gui/SupervisionGrid.swift` | LazyVGrid + tile view + Add tile + count header |
| `ios-app/MacIOSWorkspace/Gui/SupervisionTile.swift` | Single tile rendering (active = stream, armed = JPEG, dead = placeholder) + tap/longpress |
| `ios-app/MacIOSWorkspace/Gui/LaunchAppSheet.swift` | Whitelist picker (3 hardcoded entries) |
| `ios-app/MacIOSWorkspace/Gui/ViewportTracker.swift` | `.viewportTracking(store:)` ViewModifier with GeometryReader + Combine debounce |

**iOS** — modified:

| Path | Change |
|---|---|
| `ios-app/MacIOSWorkspace/CtrlMessage.swift` | Mirror Rust 4 variants + `Viewport`, `WindowRect`, `SupStatus`; extend `SupervisionEntry` |
| `ios-app/MacIOSWorkspace/SupervisionStore.swift` | `requestSwitchActive`, `requestSuperviseLaunch`, `requestRemove`, `reportViewport`, `lastFitFailed` field |
| `ios-app/MacIOSWorkspace/Gui/WindowListView.swift` | Replace List with NavigationLink → `SupervisionGrid` |
| `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift` | Add `.viewportTracking(store:)` modifier; show `lastFitFailed` toast when set |
| `ios-app/MacIOSWorkspace/PairedView.swift` | Wire navigation entry to `SupervisionGrid` instead of (or alongside) the existing list |

---

## Task Breakdown (Subagent-Driven)

Tasks run **sequentially** unless noted parallel-safe.

---

### Task M7.1 — Protocol: Rust ctrl_msg + Swift mirrors

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`
- Test: `mac-agent/crates/macagent-core/tests/m7_protocol.rs` (NEW)

- [ ] **Step 1: Write failing tests**

Create `mac-agent/crates/macagent-core/tests/m7_protocol.rs`:

```rust
use macagent_core::ctrl_msg::{CtrlPayload, SupStatus, SupervisionEntry, Viewport, WindowRect};

#[test]
fn supervise_launch_round_trip() {
    let p = CtrlPayload::SuperviseLaunch {
        bundle_id: "com.anthropic.claude".into(),
        viewport: Viewport { w: 393, h: 760 },
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn switch_active_canonical_sorted() {
    let p = CtrlPayload::SwitchActive {
        sup_id: "abc".into(),
        viewport: Viewport { w: 768, h: 1024 },
    };
    let bytes = macagent_core::ctrl_msg::canonical_bytes(&serde_json::to_value(&p).unwrap());
    let s = std::str::from_utf8(&bytes).unwrap();
    // Top-level: sup_id < type < viewport (lexicographic)
    assert!(s.find("\"sup_id\"").unwrap() < s.find("\"type\"").unwrap());
    assert!(s.find("\"type\"").unwrap() < s.find("\"viewport\"").unwrap());
    // Nested viewport: h < w
    assert!(s.find("\"h\"").unwrap() < s.find("\"w\"").unwrap());
}

#[test]
fn viewport_changed_round_trip() {
    let p = CtrlPayload::ViewportChanged {
        sup_id: "abc".into(),
        viewport: Viewport { w: 100, h: 200 },
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn fit_failed_round_trip() {
    let p = CtrlPayload::FitFailed {
        sup_id: "abc".into(),
        reason: "ax_denied".into(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn supervision_entry_extended_fields() {
    let entry = SupervisionEntry {
        sup_id: "abc".into(),
        window_id: 123,
        app_name: "Claude".into(),
        title: "Chat".into(),
        width: 1440,
        height: 900,
        status: SupStatus::Armed,
        original_frame: Some(WindowRect { x: 100, y: 100, w: 1440, h: 900 }),
        thumb_jpeg_b64: Some("AAAA".into()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: SupervisionEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(entry, back);
}

#[test]
fn sup_status_lowercase() {
    assert_eq!(serde_json::to_string(&SupStatus::Active).unwrap(), "\"active\"");
    assert_eq!(serde_json::to_string(&SupStatus::Armed).unwrap(), "\"armed\"");
    assert_eq!(serde_json::to_string(&SupStatus::Dead).unwrap(), "\"dead\"");
}
```

- [ ] **Step 2: Run test to verify it fails**

```
cd mac-agent && cargo test -p macagent-core --test m7_protocol
```
Expected: FAIL — `Viewport` / `WindowRect` / `SupStatus` / new variants / extended fields not in scope.

- [ ] **Step 3: Add Rust types**

Modify `mac-agent/crates/macagent-core/src/ctrl_msg.rs`. Add new types near other type definitions:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupStatus {
    Active,
    Armed,
    Dead,
}
```

Extend `SupervisionEntry` with three fields:
```rust
pub struct SupervisionEntry {
    // existing fields ...
    pub status: SupStatus,
    pub original_frame: Option<WindowRect>,
    pub thumb_jpeg_b64: Option<String>,
}
```

Add 4 variants to `CtrlPayload`:
```rust
SuperviseLaunch { bundle_id: String, viewport: Viewport },
SwitchActive    { sup_id: String, viewport: Viewport },
ViewportChanged { sup_id: String, viewport: Viewport },
FitFailed       { sup_id: String, reason: String },
```

Update existing call sites that construct `SupervisionEntry` (e.g. `supervision_router.rs`, M7-protocol fixtures, M5/M6 tests). Set `status: SupStatus::Active` for the active entry, `original_frame: None`, `thumb_jpeg_b64: None` as defaults.

- [ ] **Step 4: Run test to verify it passes**

```
cargo test -p macagent-core --test m7_protocol
```
Expected: 6 tests PASS.

- [ ] **Step 5: Mirror in Swift**

Modify `ios-app/MacIOSWorkspace/CtrlMessage.swift`:

```swift
struct Viewport: Codable, Equatable {
    let w: UInt32
    let h: UInt32
}

struct WindowRect: Codable, Equatable {
    let x: Int32
    let y: Int32
    let w: Int32
    let h: Int32
}

enum SupStatus: String, Codable, Equatable {
    case active, armed, dead
}
```

Extend `SupervisionEntry` Swift mirror with the three fields (`status`, `originalFrame`, `thumbJpegB64`) using `CodingKeys` to map snake_case ⇄ camelCase.

Add 4 cases to the `CtrlPayload` enum (`superviseLaunch`, `switchActive`, `viewportChanged`, `fitFailed`) with matching encode/decode/canonicalBytes.

- [ ] **Step 6: iOS build**

```
xcodebuild -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16 Pro' build
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 7: Mac workspace check**

```
cd mac-agent && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all 90+ tests pass + 6 new = 96+ pass; clippy clean.

- [ ] **Step 8: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-core/src/ctrl_msg.rs \
        mac-agent/crates/macagent-core/tests/m7_protocol.rs \
        mac-agent/crates/macagent-app/src/supervision_router.rs \
        ios-app/MacIOSWorkspace/CtrlMessage.swift
git commit -m "feat(m7): add SuperviseLaunch/SwitchActive/ViewportChanged/FitFailed ctrl + types

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

DO NOT push.

---

### Task M7.2 — `window_fitter` pure helper (aspect-fit math)

**Files:**
- Create: `mac-agent/crates/macagent-app/src/window_fitter.rs`
- Modify: `mac-agent/crates/macagent-app/src/main.rs` (`mod window_fitter;`)

This task ships **only the pure helper** `compute_target_size` + tests. AX FFI is M7.3.

- [ ] **Step 1: Write failing tests**

Create `mac-agent/crates/macagent-app/src/window_fitter.rs` with the test module first:

```rust
//! AX-based window resize and restore.

use macagent_core::ctrl_msg::{Viewport, WindowRect};

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
        assert_eq!(h, 1200);  // clamped
    }

    #[test]
    fn aspect_fit_landscape_viewport() {
        // viewport landscape 800x500
        let original = WindowRect { x: 0, y: 0, w: 1000, h: 800 };
        let viewport = Viewport { w: 800, h: 500 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1000);
        assert_eq!(h, 625);  // 1000 * (500/800)
    }

    #[test]
    fn clamp_min_size() {
        // Tiny window
        let original = WindowRect { x: 0, y: 0, w: 200, h: 150 };
        let viewport = Viewport { w: 100, h: 100 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 400);  // clamped up
        assert_eq!(h, 400);  // 400 * 1.0 = 400
    }

    #[test]
    fn clamp_max_size() {
        let original = WindowRect { x: 0, y: 0, w: 3840, h: 2160 };
        let viewport = Viewport { w: 1024, h: 768 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1920);  // clamped down
        assert_eq!(h, 1440);  // 1920 * (768/1024) = 1440
    }
}
```

- [ ] **Step 2: Add module declaration**

In `mac-agent/crates/macagent-app/src/main.rs`:
```rust
#[allow(dead_code)]
mod window_fitter;
```

- [ ] **Step 3: Run test to verify it fails**

```
cd mac-agent && cargo test -p macagent-app window_fitter::tests
```
Expected: FAIL — `compute_target_size` not in scope.

- [ ] **Step 4: Implement helper**

Append to `window_fitter.rs` (above `#[cfg(test)] mod tests`):

```rust
const MIN_W: i32 = 400;
const MIN_H: i32 = 300;
const MAX_W: i32 = 1920;
const MAX_H: i32 = 1200;

/// Pure: compute target window size that aspect-matches the viewport.
/// Strategy (v0.1): keep width = original.w, derive height = w * (vp.h / vp.w),
/// then clamp both to [MIN, MAX].
pub fn compute_target_size(original: &WindowRect, viewport: Viewport) -> (i32, i32) {
    let vp_w = viewport.w.max(1) as f64;
    let vp_h = viewport.h.max(1) as f64;
    let mut w = original.w;
    let mut h = ((w as f64) * vp_h / vp_w).round() as i32;
    // Clamp width first
    if w > MAX_W { w = MAX_W; h = ((w as f64) * vp_h / vp_w).round() as i32; }
    if w < MIN_W { w = MIN_W; h = ((w as f64) * vp_h / vp_w).round() as i32; }
    // Then clamp height
    if h > MAX_H { h = MAX_H; }
    if h < MIN_H { h = MIN_H; }
    (w, h)
}
```

- [ ] **Step 5: Run tests + clippy**

```
cd mac-agent && cargo test -p macagent-app window_fitter::tests
cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings
```
Expected: 4 tests PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/window_fitter.rs \
        mac-agent/crates/macagent-app/src/main.rs
git commit -m "feat(m7): add window_fitter::compute_target_size aspect-fit helper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.3 — `window_fitter` AX FFI: fit + restore

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/window_fitter.rs`

Adds the actual AX calls. No new tests (FFI requires GUI session).

- [ ] **Step 1: Add AX FFI bindings**

Append to `window_fitter.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
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
    fn CFRelease(cf: CFTypeRef);
}

const K_AX_VALUE_TYPE_CG_SIZE: u32 = 1;
const K_AX_VALUE_TYPE_CG_POINT: u32 = 2;
const AX_ERROR_SUCCESS: AXError = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize { width: f64, height: f64 }
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint { x: f64, y: f64 }

fn cf_str(s: &str) -> CFString { CFString::new(s) }

unsafe fn release(cf: CFTypeRef) {
    if !cf.is_null() { CFRelease(cf); }
}
```

- [ ] **Step 2: Add window-finder + fit + restore**

Append:

```rust
/// Find the AXUIElement for `window_id` owned by `pid`. Heuristic:
/// CGWindowList tells us bounds; AX gives us a list of windows; pick the AX
/// window whose bounds best matches.
unsafe fn find_ax_window(pid: i32, target: &WindowRect) -> Result<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() { return Err(anyhow!("AXUIElementCreateApplication returned null")); }

    let attr = cf_str("AXWindows");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value);
    release(app as CFTypeRef);
    if err != AX_ERROR_SUCCESS || value.is_null() {
        return Err(anyhow!("AXWindows fetch failed: err={err}"));
    }

    // value is a CFArray of AXUIElementRef. Walk it.
    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
    let count = CFArrayGetCount(value as _);
    let mut best: AXUIElementRef = std::ptr::null();
    let mut best_dist = f64::MAX;

    for i in 0..count {
        let win = CFArrayGetValueAtIndex(value as _, i) as AXUIElementRef;
        let mut size_val: CFTypeRef = std::ptr::null();
        let mut pos_val: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), &mut size_val) != AX_ERROR_SUCCESS { continue; }
        if AXUIElementCopyAttributeValue(win, cf_str("AXPosition").as_concrete_TypeRef(), &mut pos_val) != AX_ERROR_SUCCESS {
            release(size_val); continue;
        }
        let mut sz = CGSize { width: 0.0, height: 0.0 };
        let mut pt = CGPoint { x: 0.0, y: 0.0 };
        AXValueGetValue(size_val, K_AX_VALUE_TYPE_CG_SIZE, &mut sz as *mut _ as *mut c_void);
        AXValueGetValue(pos_val, K_AX_VALUE_TYPE_CG_POINT, &mut pt as *mut _ as *mut c_void);
        release(size_val); release(pos_val);

        let dx = pt.x - target.x as f64;
        let dy = pt.y - target.y as f64;
        let dw = sz.width - target.w as f64;
        let dh = sz.height - target.h as f64;
        let dist = dx*dx + dy*dy + dw*dw + dh*dh;
        if dist < best_dist {
            best_dist = dist;
            best = win;
        }
    }
    release(value);
    if best.is_null() { Err(anyhow!("no AX window matched bounds")) }
    else { Ok(best) }
}

/// Fit the window to viewport aspect; return original frame for later restore.
pub fn fit(window_id: u32, owner_pid: i32, current: &WindowRect, viewport: Viewport) -> Result<WindowRect> {
    let _ = window_id; // window_id used by caller for logging; AX matches by frame
    unsafe {
        let win = find_ax_window(owner_pid, current).context("find AX window")?;
        // Read original size + position
        let mut sz_val: CFTypeRef = std::ptr::null();
        let mut pos_val: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), &mut sz_val) != AX_ERROR_SUCCESS {
            return Err(anyhow!("AXSize get failed"));
        }
        if AXUIElementCopyAttributeValue(win, cf_str("AXPosition").as_concrete_TypeRef(), &mut pos_val) != AX_ERROR_SUCCESS {
            release(sz_val); return Err(anyhow!("AXPosition get failed"));
        }
        let mut sz = CGSize { width: 0.0, height: 0.0 };
        let mut pt = CGPoint { x: 0.0, y: 0.0 };
        AXValueGetValue(sz_val, K_AX_VALUE_TYPE_CG_SIZE, &mut sz as *mut _ as *mut c_void);
        AXValueGetValue(pos_val, K_AX_VALUE_TYPE_CG_POINT, &mut pt as *mut _ as *mut c_void);
        release(sz_val); release(pos_val);

        let original = WindowRect {
            x: pt.x as i32, y: pt.y as i32,
            w: sz.width as i32, h: sz.height as i32,
        };

        // Compute target + set
        let (tw, th) = compute_target_size(&original, viewport);
        let target_size = CGSize { width: tw as f64, height: th as f64 };
        let target_value = AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &target_size as *const _ as *const c_void);
        if target_value.is_null() { return Err(anyhow!("AXValueCreate failed")); }
        let err = AXUIElementSetAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), target_value);
        release(target_value);
        if err != AX_ERROR_SUCCESS {
            return Err(anyhow!("AXSize set failed: err={err}"));
        }
        Ok(original)
    }
}

pub fn restore(window_id: u32, owner_pid: i32, original: &WindowRect) -> Result<()> {
    let _ = window_id;
    unsafe {
        let win = find_ax_window(owner_pid, original).context("find AX window")?;
        let sz = CGSize { width: original.w as f64, height: original.h as f64 };
        let pt = CGPoint { x: original.x as f64, y: original.y as f64 };
        let sz_val = AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &sz as *const _ as *const c_void);
        let pt_val = AXValueCreate(K_AX_VALUE_TYPE_CG_POINT, &pt as *const _ as *const c_void);
        if sz_val.is_null() || pt_val.is_null() { return Err(anyhow!("AXValueCreate failed")); }
        let _ = AXUIElementSetAttributeValue(win, cf_str("AXSize").as_concrete_TypeRef(), sz_val);
        let _ = AXUIElementSetAttributeValue(win, cf_str("AXPosition").as_concrete_TypeRef(), pt_val);
        release(sz_val); release(pt_val);
        Ok(())
    }
}
```

- [ ] **Step 2.5: Build**

```
cd mac-agent && cargo build -p macagent-app
cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings
```
Expected: clean. If `core_foundation::array::CFArrayGetCount/GetValueAtIndex` aren't pub, swap to inline `extern "C"` for those two functions.

- [ ] **Step 3: Run tests (helpers still pass; FFI not unit-testable)**

```
cd mac-agent && cargo test --workspace
```
Expected: 96+ tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/window_fitter.rs
git commit -m "feat(m7): add WindowFitter AX fit + restore (kAXSizeAttribute)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.4 — `launcher_m7` whitelist + window detection

**Files:**
- Create: `mac-agent/crates/macagent-app/src/launcher_m7.rs`
- Modify: `mac-agent/crates/macagent-app/src/main.rs` (`mod launcher_m7;`)

- [ ] **Step 1: Write failing tests**

Create `mac-agent/crates/macagent-app/src/launcher_m7.rs` with tests:

```rust
//! M7 GUI app launcher (NSWorkspace + window discovery). Distinct from
//! launcher.rs which is the M3 PTY producer launcher.

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
```

- [ ] **Step 2: Run failing test**

Add `mod launcher_m7;` (with `#[allow(dead_code)]`) to `main.rs`.

```
cd mac-agent && cargo test -p macagent-app launcher_m7::tests
```
Expected: FAIL — `is_allowed` not in scope.

- [ ] **Step 3: Implement helper + FFI launch**

Append to `launcher_m7.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use std::time::{Duration, Instant};

const ALLOWED_BUNDLES: &[&str] = &[
    "com.openai.chat",
    "com.anthropic.claude",
    "com.google.Chrome",
];

pub fn is_allowed(bundle_id: &str) -> bool {
    ALLOWED_BUNDLES.contains(&bundle_id)
}

/// Launch the app via NSWorkspace and poll CGWindowList for a new window
/// owned by the launched pid. Returns (pid, window_id) on success.
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

fn open_application(bundle_id: &str) -> Result<i32> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let bid = NSString::from_str(bundle_id);
    let url = unsafe { workspace.URLForApplicationWithBundleIdentifier(&bid) }
        .ok_or_else(|| anyhow!("no app for bundle {bundle_id}"))?;
    // openApplicationAtURL is async on AppKit; we use the sync open instead via NSWorkspace.openURL
    let app = unsafe { workspace.openURL(&url) };
    if !app {
        return Err(anyhow!("openURL failed for {bundle_id}"));
    }
    // Loop briefly to find the running application's pid
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let apps = unsafe { workspace.runningApplications() };
        for app in apps.iter() {
            if let Some(b) = unsafe { app.bundleIdentifier() } {
                if b.to_string() == bundle_id {
                    return Ok(unsafe { app.processIdentifier() });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(anyhow!("running app not found after open"))
}

fn find_first_window_for_pid(pid: i32) -> Option<u32> {
    // Reuse gui_capture::windows::list_windows then filter
    crate::gui_capture::windows::list_windows()
        .ok()?
        .into_iter()
        .find(|w| w.owner_pid as i32 == pid && !w.title.is_empty() && (w.width * w.height) > 100 * 100)
        .map(|w| w.window_id)
}
```

> If `objc2_app_kit::NSWorkspace::openURL(_:)` isn't exposed in 0.3, fall back to `Command::new("open").arg("-b").arg(bundle_id).spawn()` + then loop on `runningApplications`. Document the choice in the report.

- [ ] **Step 4: Run tests + build**

```
cd mac-agent && cargo test -p macagent-app launcher_m7::tests
cd mac-agent && cargo build -p macagent-app
cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings
```
Expected: 2 tests PASS; build clean; clippy clean.

- [ ] **Step 5: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/launcher_m7.rs \
        mac-agent/crates/macagent-app/src/main.rs
git commit -m "feat(m7): add launcher_m7 with whitelist + NSWorkspace open + window detect

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.5 — JPEG thumbnail encoder

**Files:**
- Create: `mac-agent/crates/macagent-app/src/gui_capture/thumbnail.rs`
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` (add `mod thumbnail;`)
- Possibly modify: `mac-agent/Cargo.toml` + `crates/macagent-app/Cargo.toml` (objc2-image-io)

- [ ] **Step 1: Decide JPEG path (read deps)**

Run:
```
grep "objc2-image-io\|objc2-uniform" mac-agent/Cargo.toml
```

If not present, add to workspace deps:
```toml
objc2-image-io = "0.3"
objc2-uniform-type-identifiers = "0.3"
```

And to `crates/macagent-app/Cargo.toml`:
```toml
objc2-image-io = { workspace = true }
objc2-uniform-type-identifiers = { workspace = true }
```

If `objc2-image-io 0.3` resolution fails, fall back: declare `CGImageDestinationCreateWithData / CGImageDestinationAddImage / CGImageDestinationFinalize` via inline `extern "C"` block; document in report.

- [ ] **Step 2: Implement encoder**

Create `mac-agent/crates/macagent-app/src/gui_capture/thumbnail.rs`:

```rust
//! CVPixelBuffer → scaled CGImage → JPEG → base64 string.
//!
//! Used by gui_capture::demote_to_armed to capture the last frame of a
//! supervised stream as a thumbnail (~256×192 @ Q70).

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use core_foundation::base::TCFType;
use core_foundation::data::CFMutableData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::context::{CGContext, CGContextRef};
use core_graphics::geometry::{CGRect, CGSize};
use core_graphics::image::CGImage;
use objc2_core_video::CVPixelBuffer;
use std::ffi::c_void;

const THUMB_W: i32 = 256;
const THUMB_H: i32 = 192;
const QUALITY: f64 = 0.7;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGBitmapContextCreateImage(c: *const c_void) -> *const c_void;
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageDestinationCreateWithData(
        data: *const c_void,
        ut_type: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *const c_void;
    fn CGImageDestinationAddImage(dest: *const c_void, image: *const c_void, properties: *const c_void);
    fn CGImageDestinationFinalize(dest: *const c_void) -> bool;
}

/// Encode the buffer to a base64 JPEG string at THUMB_W × THUMB_H, Q70.
pub fn cvpixelbuffer_to_jpeg_base64(_pb: &CVPixelBuffer) -> Result<String> {
    // Step 1: build a CGImage from the pixel buffer.
    // Easiest path: use VTCreateCGImageFromCVPixelBuffer (VideoToolbox).
    let cgimage = create_cgimage_from_pb(_pb)
        .context("create CGImage from CVPixelBuffer")?;
    // Step 2: scale to THUMB_W × THUMB_H using CGContext.
    let scaled = scale_cgimage(&cgimage, THUMB_W, THUMB_H)
        .context("scale CGImage")?;
    // Step 3: JPEG-encode via CGImageDestination → CFMutableData.
    let jpeg = jpeg_encode(&scaled).context("jpeg encode")?;
    Ok(STANDARD.encode(&jpeg))
}

fn create_cgimage_from_pb(_pb: &CVPixelBuffer) -> Result<CGImage> {
    // VideoToolbox provides VTCreateCGImageFromCVPixelBuffer.
    use objc2_core_video::CVPixelBufferRef;
    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        fn VTCreateCGImageFromCVPixelBuffer(
            buffer: CVPixelBufferRef,
            options: *const c_void,
            image_out: *mut *const c_void,
        ) -> i32;
    }
    let pb_ref = _pb as *const CVPixelBuffer as CVPixelBufferRef;
    let mut img: *const c_void = std::ptr::null();
    let err = unsafe {
        VTCreateCGImageFromCVPixelBuffer(pb_ref, std::ptr::null(), &mut img)
    };
    if err != 0 || img.is_null() {
        return Err(anyhow!("VTCreateCGImageFromCVPixelBuffer failed: {err}"));
    }
    // Wrap raw CGImageRef into core_graphics::CGImage
    Ok(unsafe { CGImage::from_ptr(img as *mut _) })
}

fn scale_cgimage(src: &CGImage, w: i32, h: i32) -> Result<CGImage> {
    use core_graphics::color_space::CGColorSpace;
    let cs = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(
        None, w as usize, h as usize, 8, 0, &cs,
        core_graphics::base::kCGImageAlphaPremultipliedLast,
    );
    let dst = CGRect::new(
        &core_graphics::geometry::CGPoint::new(0.0, 0.0),
        &CGSize::new(w as f64, h as f64),
    );
    ctx.draw_image(dst, src);
    let img = ctx.create_image()
        .ok_or_else(|| anyhow!("CGContext::create_image returned None"))?;
    Ok(img)
}

fn jpeg_encode(src: &CGImage) -> Result<Vec<u8>> {
    let data = CFMutableData::new();
    let ut_jpeg = CFString::from_static_string("public.jpeg");
    let dest = unsafe {
        CGImageDestinationCreateWithData(
            data.as_concrete_TypeRef() as *const c_void,
            ut_jpeg.as_concrete_TypeRef() as *const c_void,
            1,
            std::ptr::null(),
        )
    };
    if dest.is_null() { return Err(anyhow!("CGImageDestinationCreateWithData failed")); }

    let key = CFString::from_static_string("kCGImageDestinationLossyCompressionQuality");
    let q = CFNumber::from(QUALITY);
    let props = CFDictionary::from_CFType_pairs(&[(key, q)]);

    unsafe {
        CGImageDestinationAddImage(
            dest,
            src.as_ptr() as *const c_void,
            props.as_concrete_TypeRef() as *const c_void,
        );
        if !CGImageDestinationFinalize(dest) {
            return Err(anyhow!("CGImageDestinationFinalize returned false"));
        }
    }

    let bytes = data.bytes();
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    // No unit test — full pipeline requires real CVPixelBuffer.
    // Manual smoke verifies thumbnail appears in iOS armed tile.
}
```

- [ ] **Step 3: Wire mod into gui_capture**

Modify `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` to add:
```rust
mod thumbnail;
```

(After existing `mod` declarations.)

- [ ] **Step 4: Build + clippy**

```
cd mac-agent && cargo build -p macagent-app
cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings
```

If `core_graphics::image::CGImage::from_ptr` isn't available, use the wrapper macros provided by core-graphics 0.24 (likely `CGImage::wrap_under_create_rule`). Adjust accordingly.

If the `CGContext::create_bitmap_context` signature differs, consult `~/.cargo/registry/src/*/core-graphics-*/src/context.rs`. Same for `draw_image`.

- [ ] **Step 5: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/Cargo.toml \
        mac-agent/crates/macagent-app/Cargo.toml \
        mac-agent/crates/macagent-app/src/gui_capture/thumbnail.rs \
        mac-agent/crates/macagent-app/src/gui_capture/mod.rs
git commit -m "feat(m7): add thumbnail::cvpixelbuffer_to_jpeg_base64 (256x192 Q70)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.6 — Capture last frame in FrameSink + demote_to_armed API

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/stream.rs`
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/mod.rs`

- [ ] **Step 1: Add last_frame slot in ActiveStream**

Modify `stream.rs::ActiveStream`:
```rust
struct ActiveStream {
    sc_stream: Option<SCStream>,
    stop_flag: Arc<AtomicBool>,
    encoder_thread: Option<JoinHandle<()>>,
    tokio_task: tokio::task::JoinHandle<()>,
    last_frame: Arc<Mutex<Option<objc2_core_video::CVPixelBuffer>>>,  // NEW
}
```

Modify `FrameSink`:
```rust
struct FrameSink {
    tx: SyncSender<FramePayload>,
    last_frame: Arc<Mutex<Option<objc2_core_video::CVPixelBuffer>>>,  // NEW
    start_inst: std::time::Instant,
}
```

In `did_output_sample_buffer`, before `try_send`:
```rust
*self.last_frame.lock().unwrap() = Some(pb.clone());
```

In `StreamManager::start`, build `last_frame = Arc::new(Mutex::new(None))` and clone into both FrameSink and ActiveStream.

- [ ] **Step 2: Add take_last_frame method on ActiveStream**

```rust
impl ActiveStream {
    pub fn take_last_frame(&self) -> Option<objc2_core_video::CVPixelBuffer> {
        self.last_frame.lock().unwrap().take()
    }
}
```

- [ ] **Step 3: Add demote_to_armed in StreamManager**

```rust
impl StreamManager {
    /// Stop the active stream AND grab the last pixel buffer.
    /// Returns the buffer for thumbnail encoding.
    pub fn stop_with_last_frame(&self, sup_id: &str) -> Option<objc2_core_video::CVPixelBuffer> {
        let entry = self.active.lock().unwrap().take();
        match entry {
            Some((id, active)) if id == sup_id => {
                let frame = active.take_last_frame();
                active.stop();
                frame
            }
            other => {
                // Wrong sup_id or no active — restore
                *self.active.lock().unwrap() = other;
                None
            }
        }
    }
}
```

- [ ] **Step 4: Wire demote_to_armed in GuiCapture**

Modify `gui_capture/mod.rs`:
```rust
impl GuiCapture {
    /// Stop the active stream for sup_id and return its last frame as a
    /// base64-encoded JPEG thumbnail (or None on any failure).
    pub fn demote_to_armed(&self, sup_id: &str) -> Option<String> {
        let pb = self.streams.stop_with_last_frame(sup_id)?;
        thumbnail::cvpixelbuffer_to_jpeg_base64(&pb).ok()
    }
}
```

- [ ] **Step 5: Build + tests**

```
cd mac-agent && cargo build -p macagent-app
cd mac-agent && cargo test --workspace
cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/gui_capture/stream.rs \
        mac-agent/crates/macagent-app/src/gui_capture/mod.rs
git commit -m "feat(m7): GuiCapture::demote_to_armed captures last frame as JPEG b64

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.7 — SupervisionRouter multi-entry registry

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/supervision_router.rs`

This is the centerpiece. Replace the single-`Option<ActiveSupervision>` model with `HashMap<String, SupervisionEntry>` + `active_sup: Option<String>`.

- [ ] **Step 1: Write failing tests**

Append to `supervision_router.rs` `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn register_eight_then_ninth_rejected() {
    // Build a minimal router with stubbed gui_capture (use a local stub trait).
    // Register 8 entries via push_armed; 9th returns Err.
    // ... (implement using a TestRouter that bypasses GuiCapture FFI)
}

#[tokio::test]
async fn switch_demotes_old_to_armed() {
    // Register A (active), then B (which becomes active).
    // Verify A.status == Armed, B.status == Active, active_sup == Some(B).
}

#[tokio::test]
async fn remove_active_promotes_next_armed() {
    // Register A (active), B (armed), C (armed).
    // Remove A → expect B becomes active, A removed.
}
```

If the GuiCapture-coupled tests are too heavy, gate behind `#[ignore]` and rely on manual smoke. **Minimum: keep the limit-check test which is pure (no GuiCapture).**

- [ ] **Step 2: Run failing tests**

```
cd mac-agent && cargo test -p macagent-app supervision_router
```
Expected: FAIL — methods not yet implemented.

- [ ] **Step 3: Replace internal state with multi-entry registry**

Replace `Mutex<Option<ActiveSupervision>>` with:
```rust
struct Registry {
    entries: HashMap<String, SupervisionEntry>,
    active_sup: Option<String>,
}
```

Embed in `SupervisionRouter`:
```rust
pub struct SupervisionRouter {
    gui_capture: Arc<GuiCapture>,
    rtc_peer: Arc<RtcPeer>,
    video_track: VideoTrackHandle,
    ctrl_tx: UnboundedSender<CtrlPayload>,
    registry: Mutex<Registry>,
    launcher_m7: Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, anyhow::Result<(i32, u32)>> + Send + Sync>,
    window_fitter: Arc<WindowFitterFns>,
}
```

(Use a function pointer / dyn trait for `launcher_m7` to keep it test-stubbable; or simpler: a bare `pub async fn launch_and_find_window` import.)

`current_window_id(sup_id)` becomes:
```rust
pub async fn current_window_id(&self, sup_id: &str) -> Option<u32> {
    self.registry.lock().await.entries.get(sup_id).map(|e| e.window_id)
}
```

Add new handlers:
```rust
impl SupervisionRouter {
    pub async fn handle_supervise_existing(&self, window_id: u32, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_supervise_launch(&self, bundle_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_switch_active(&self, sup_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_viewport_changed(&self, sup_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_remove_supervised(&self, sup_id: String) -> Result<()> { ... }
}
```

`set_active(new_sup_id, viewport)` flow:
1. Acquire `registry` lock; check `entries.len() >= 8` → return `ErrSupervisionLimit` if registering, OK if switching.
2. If `active_sup` Some and != new_sup_id:
   - Drop lock briefly. Call `gui_capture.demote_to_armed(active_sup)` → write thumbnail to entries[active_sup].
   - Re-acquire; entries[active_sup].status = Armed.
3. Find entries[new_sup_id]. Read window_id + (cached) original_frame.
4. Drop lock. Call `window_fitter::fit(window_id, owner_pid, &current_rect, viewport)`:
   - On success → entries[new_sup_id].original_frame = Some(returned).
   - On failure → emit `CtrlPayload::FitFailed { sup_id, reason }`, continue.
5. Drop lock. Call `gui_capture.start(new_sup_id, window_id, video_track.clone(), &cfg)?`.
6. Re-acquire lock. entries[new_sup_id].status = Active. active_sup = Some(new_sup_id).
7. Emit `CtrlPayload::SupervisionList { entries: registry snapshot }`.

`remove(sup_id)`:
1. Lock; pop entry from entries. If was active, restore original_frame via `window_fitter::restore`. Stop gui_capture stream.
2. If active_sup == sup_id → pick next entry from entries (any) → set_active to that.
3. Emit SupervisionList.

- [ ] **Step 4: Update callers (ui.rs / rtc_glue.rs)**

This step may require touching 5–10 call sites where `SupervisionRouter` was previously single-entry. Do not push yet — just make `cargo build` pass; behavior wiring is M7.8.

- [ ] **Step 5: Run tests + build**

```
cd mac-agent && cargo build --workspace
cd mac-agent && cargo test --workspace
cd mac-agent && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: tests pass (limit check + others); clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/supervision_router.rs
git commit -m "feat(m7): multi-entry SupervisionRouter (≤8) + switch/launch/viewport handlers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.8 — Wire 4 ctrl variants in rtc_glue + ui.rs

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`

- [ ] **Step 1: Extend rtc_glue drainer match**

In `rtc_glue.rs` find the supervision drainer (post-M5.2.5 drainer that already routes ListWindows / SuperviseExisting / RemoveSupervised / GuiInputCmd). Add 4 new arms:

```rust
match payload {
    // ... existing arms ...
    CtrlPayload::SuperviseLaunch { bundle_id, viewport } => {
        if let Err(e) = sr.handle_supervise_launch(bundle_id.clone(), viewport).await {
            eprintln!("[rtc_glue] supervise_launch error: {e}");
        }
    }
    CtrlPayload::SwitchActive { sup_id, viewport } => {
        if let Err(e) = sr.handle_switch_active(sup_id.clone(), viewport).await {
            eprintln!("[rtc_glue] switch_active error: {e}");
        }
    }
    CtrlPayload::ViewportChanged { sup_id, viewport } => {
        if let Err(e) = sr.handle_viewport_changed(sup_id.clone(), viewport).await {
            eprintln!("[rtc_glue] viewport_changed error: {e}");
        }
    }
    // FitFailed is Mac → iOS only; ignore here if it appears (defensive).
    other => { sr.handle_ctrl(other).await; }
}
```

- [ ] **Step 2: Wire SupervisionRouter in ui.rs Connect block**

In `ui.rs::Connect`, after the existing `SupervisionRouter::new(...)` call, no other changes are required — the new handlers are reached purely via the drainer match. The launcher_m7 + window_fitter modules are imported by `supervision_router.rs` directly.

- [ ] **Step 3: Build + tests**

```
cd mac-agent && cargo build --workspace
cd mac-agent && cargo test --workspace
cd mac-agent && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean; all tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/rtc_glue.rs \
        mac-agent/crates/macagent-app/src/ui.rs
git commit -m "feat(m7): wire SuperviseLaunch/SwitchActive/ViewportChanged ctrl dispatch

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.9 — iOS SupervisionStore actions

**Files:**
- Modify: `ios-app/MacIOSWorkspace/SupervisionStore.swift`

Adds 4 client-side methods that send the new ctrl variants and one published field for fit_failed.

- [ ] **Step 1: Implement methods**

Add to `SupervisionStore`:

```swift
// MARK: - M7 actions

@Published var lastFitFailed: (supId: String, reason: String, ts: Date)?

func requestSwitchActive(supId: String, viewport: Viewport? = nil) {
    guard let glue = self.glue else { return }
    let vp = viewport ?? Viewport(w: 393, h: 760)  // safe default; ViewportTracker overrides
    Task { await glue.sendCtrl(.switchActive(supId: supId, viewport: vp)) }
}

func requestSuperviseLaunch(bundleId: String, viewport: Viewport? = nil) {
    guard let glue = self.glue else { return }
    let vp = viewport ?? Viewport(w: 393, h: 760)
    Task { await glue.sendCtrl(.superviseLaunch(bundleId: bundleId, viewport: vp)) }
}

func requestRemove(supId: String) {
    guard let glue = self.glue else { return }
    Task { await glue.sendCtrl(.removeSupervised(supId: supId)) }
}

func reportViewport(w: CGFloat, h: CGFloat) {
    guard let glue = self.glue,
          let active = entries.first(where: { $0.status == .active }) else { return }
    let vp = Viewport(w: UInt32(max(1, w)), h: UInt32(max(1, h)))
    Task { await glue.sendCtrl(.viewportChanged(supId: active.supId, viewport: vp)) }
}
```

In the existing ctrl-message dispatcher, add a case:
```swift
case .fitFailed(let supId, let reason):
    self.lastFitFailed = (supId, reason, Date())
```

- [ ] **Step 2: Build**

```
xcodebuild -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16 Pro' build
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 3: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/SupervisionStore.swift
git commit -m "feat(ios-m7): SupervisionStore actions (switch/launch/remove/reportViewport)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.10 — iOS SupervisionGrid + LaunchAppSheet

**Files:**
- Create: `ios-app/MacIOSWorkspace/Gui/SupervisionGrid.swift`
- Create: `ios-app/MacIOSWorkspace/Gui/SupervisionTile.swift`
- Create: `ios-app/MacIOSWorkspace/Gui/LaunchAppSheet.swift`
- Modify: `ios-app/MacIOSWorkspace/Gui/WindowListView.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`

- [ ] **Step 1: SupervisionTile**

Create `Gui/SupervisionTile.swift`:

```swift
import SwiftUI

struct SupervisionTile: View {
    let entry: SupervisionEntry
    @Bindable var store: SupervisionStore

    var body: some View {
        VStack(spacing: 4) {
            ZStack {
                content
                if entry.status == .active { activeBadge }
            }
            .frame(maxWidth: .infinity)
            .aspectRatio(4/3, contentMode: .fit)
            .background(Color.gray.opacity(0.2))
            .clipShape(RoundedRectangle(cornerRadius: 8))

            Text(entry.appName).font(.caption).lineLimit(1)
            Text(entry.title).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        }
        .onTapGesture {
            if entry.status != .active {
                store.requestSwitchActive(supId: entry.supId)
            }
        }
        .contextMenu {
            Button("移除", role: .destructive) {
                store.requestRemove(supId: entry.supId)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if entry.status == .active {
            GuiStreamView(videoTrack: store.activeTrack)
        } else if let b64 = entry.thumbJpegB64,
                  let data = Data(base64Encoded: b64),
                  let img = UIImage(data: data) {
            Image(uiImage: img)
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else {
            Image(systemName: "rectangle.dashed")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
        }
    }

    private var activeBadge: some View {
        VStack {
            Spacer()
            Rectangle().fill(Color.green).frame(height: 3)
        }
    }
}
```

- [ ] **Step 2: SupervisionGrid**

Create `Gui/SupervisionGrid.swift`:

```swift
import SwiftUI

struct SupervisionGrid: View {
    @Bindable var store: SupervisionStore
    @Environment(\.horizontalSizeClass) private var hSizeClass
    @State private var showingAdd = false
    @State private var showingLaunch = false

    private var columnCount: Int { hSizeClass == .compact ? 2 : 3 }

    private var columns: [GridItem] {
        Array(repeating: GridItem(.flexible(), spacing: 12), count: columnCount)
    }

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 12) {
                ForEach(store.entries) { entry in
                    NavigationLink(destination: GuiStreamDetailView(store: store, entry: entry)) {
                        SupervisionTile(entry: entry, store: store)
                    }
                    .buttonStyle(.plain)
                }
                if store.entries.count < 8 {
                    Button { showingAdd = true } label: { addTile }
                        .buttonStyle(.plain)
                }
            }
            .padding(12)
        }
        .navigationTitle("\(store.entries.count) / 8 监管中")
        .confirmationDialog("添加监管", isPresented: $showingAdd) {
            Button("监管现有窗口") { /* hand off to existing window-list sheet */ }
            Button("启动 App") { showingLaunch = true }
            Button("取消", role: .cancel) { }
        }
        .sheet(isPresented: $showingLaunch) {
            LaunchAppSheet(store: store)
        }
    }

    private var addTile: some View {
        VStack {
            Image(systemName: "plus.rectangle.on.rectangle")
                .font(.system(size: 40))
                .foregroundStyle(.secondary)
            Text("添加").font(.caption)
        }
        .frame(maxWidth: .infinity)
        .aspectRatio(4/3, contentMode: .fit)
        .background(Color.gray.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
```

- [ ] **Step 3: LaunchAppSheet**

Create `Gui/LaunchAppSheet.swift`:

```swift
import SwiftUI

struct LaunchAppSheet: View {
    let store: SupervisionStore
    @Environment(\.dismiss) private var dismiss

    private static let bundles: [(id: String, name: String, icon: String)] = [
        ("com.openai.chat",       "ChatGPT",        "bubble.left"),
        ("com.anthropic.claude",  "Claude Desktop", "sparkles"),
        ("com.google.Chrome",     "Google Chrome",  "globe"),
    ]

    var body: some View {
        NavigationStack {
            List(Self.bundles, id: \.id) { app in
                Button {
                    store.requestSuperviseLaunch(bundleId: app.id)
                    dismiss()
                } label: {
                    Label(app.name, systemImage: app.icon)
                }
            }
            .navigationTitle("启动 App")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Wire navigation**

Modify `WindowListView.swift` (or `PairedView.swift` — wherever the existing supervised list entry lives) so the user lands on `SupervisionGrid` instead of the legacy List. The simplest path: change the existing `NavigationLink { ... }` body to `NavigationLink { SupervisionGrid(store: store) }`.

If there's no top-level entry, add one in `PairedView` next to the existing "桌面" link.

- [ ] **Step 5: Build**

```
xcodebuild ... build
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Gui/SupervisionGrid.swift \
        ios-app/MacIOSWorkspace/Gui/SupervisionTile.swift \
        ios-app/MacIOSWorkspace/Gui/LaunchAppSheet.swift \
        ios-app/MacIOSWorkspace/Gui/WindowListView.swift \
        ios-app/MacIOSWorkspace/PairedView.swift
git commit -m "feat(ios-m7): SupervisionGrid + Tile + LaunchAppSheet

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.11 — iOS ViewportTracker + fit_failed toast

**Files:**
- Create: `ios-app/MacIOSWorkspace/Gui/ViewportTracker.swift`
- Modify: `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift`

- [ ] **Step 1: ViewportTracker**

Create `Gui/ViewportTracker.swift`:

```swift
import SwiftUI
import Combine

struct ViewportTracker: ViewModifier {
    let store: SupervisionStore

    func body(content: Content) -> some View {
        content.background(
            GeometryReader { geo in
                Color.clear
                    .onAppear { store.reportViewport(w: geo.size.width, h: geo.size.height) }
                    .onChange(of: geo.size) { _, newSize in
                        store.reportViewport(w: newSize.width, h: newSize.height)
                    }
            }
        )
    }
}

extension View {
    func viewportTracking(store: SupervisionStore) -> some View {
        modifier(ViewportTracker(store: store))
    }
}
```

> No Combine debounce in v0.1 (YAGNI per spec §1 OUT). If Stage Manager dragging produces too many ctrl messages in manual smoke, add a 200ms debounce in M8.

- [ ] **Step 2: Add viewportTracking + fit_failed toast in GuiStreamDetailView**

Modify `GuiStreamDetailView.swift`:

```swift
var body: some View {
    VStack(spacing: 0) {
        // ... existing content ...
    }
    .viewportTracking(store: store)
    .overlay(alignment: .top) {
        if let f = store.lastFitFailed,
           Date().timeIntervalSince(f.ts) < 5 {
            HStack {
                Image(systemName: "exclamationmark.triangle.fill")
                Text("无法调整窗口尺寸（letterbox 显示）")
                    .font(.callout)
                Spacer()
            }
            .padding(8)
            .background(Color.yellow.opacity(0.9))
            .transition(.move(edge: .top))
        }
    }
}
```

- [ ] **Step 3: Build**

```
xcodebuild ... build
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 4: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Gui/ViewportTracker.swift \
        ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift
git commit -m "feat(ios-m7): ViewportTracker + fit_failed toast in GuiStreamDetailView

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M7.12 — Manual smoke test (no commit)

Run on real Mac + real iPhone (or iPad):

1. Pair devices (existing flow); enable AX permission for macagent (M6 done).
2. iOS Add tile → "监管现有窗口" → pick Chrome window. Verify stream appears.
3. iOS Add tile → "启动 App" → pick Claude Desktop. Verify Claude launches and the new window appears + auto-fits (window resizes to viewport aspect).
4. With 3 entries registered (active = Claude), tap a Chrome tile (armed) → ≤200ms switch; old Claude tile shows JPEG thumbnail.
5. Rotate iPhone (portrait → landscape): the active Mac window resizes to landscape aspect.
6. iPad Split View: drag the macagent app to half-width. Verify Mac window resizes.
7. Add tile while at 8 → expect SuperviseReject toast "监管数已达上限".
8. Supervise System Preferences (impossible to resize) → expect `fit_failed` toast on iOS + stream still rendering (letterbox).
9. Close the supervised Mac window → entry status changes to dead → next armed auto-promoted.
10. Long-press an armed tile → "移除" → entry disappears + restore_window applied (Mac window returns to original size).

If any of (3)–(10) fail: do not declare M7 done. Diagnose, fix, rerun.

**No commit.**

---

### Task M7.13 — M7 final review (subagent dispatch)

Caller dispatches `superpowers:code-reviewer` against `e697ba7..HEAD` (skip the spec commit):

- Spec coverage: every spec §1–§8 maps to a commit/task.
- Unsafe FFI audit: AX (window_fitter), VTCreateCGImageFromCVPixelBuffer (thumbnail), CGImageDestination (thumbnail), NSWorkspace (launcher_m7).
- Threading: `set_active` flow holds locks across `await`? Should be **NO** — drop `MutexGuard` before awaiting any FFI/async call.
- Switch-active 200ms budget: any chance the flow takes >200ms? Identify the slowest leg.
- 8-entry limit enforced consistently across `handle_supervise_existing` + `handle_supervise_launch`.
- iOS `requestSwitchActive` defaults viewport to 393×760 — verify the `viewportTracking` modifier overrides this ASAP after navigation.
- `WindowFitter::find_ax_window` heuristic correctness with multiple windows of similar bounds.
- No new `unsafe impl Send` beyond what's strictly required.

If the reviewer finds Critical/Important issues, dispatch a fixup subagent (M5/M5.2.5/M6 pattern).

---

## Risks + Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `core_foundation::array::CFArrayGetCount/GetValueAtIndex` private/changed | Medium | window_fitter compile error | Inline `extern "C" { fn CFArrayGetCount(...) -> CFIndex; }` declaration. |
| `objc2_core_video::CVPixelBuffer` ref-counting wrong → crash on thumbnail | Medium | Thumbnail panic on demote | Wrap encoder call in `std::panic::catch_unwind`; on failure return None (entry shows placeholder). |
| `core_graphics::CGImage::from_ptr` API shape unknown | Low | thumbnail.rs compile error | Use `CGImage::wrap_under_create_rule(ptr)` (core-graphics 0.24 idiom); fall back to inline FFI if needed. |
| AX heuristic picks wrong window when multiple windows of similar bounds exist (e.g. two Chrome instances) | Medium | fit_window resizes wrong window | Acceptable for v0.1 (rare); noted in OUT. M8 polish: use private `_AXUIElementGetWindow` API. |
| Switch flow holds lock across `gui_capture.start().await` (dead-lock) | Medium | Switch hangs | Drop `Mutex<Registry>` guard before any `.await`; re-acquire after. Reviewer must verify (M7.13). |
| `NSWorkspace.openURL` rejected by sandbox / app removed | Low | launch_failed | 5s timeout + `SuperviseReject { code: "launch_timeout" }`; iOS toast. |
| 8 base64 thumbnails @ ~33KB each = ~270KB inline in SupervisionList ctrl | Low | ctrl message size | DataChannel chunks at 16KB; webrtc-rs handles fragmentation. M8 may move to dedicated thumbnail channel. |
| iOS rotation fires multiple GeometryReader updates → spam ctrl | Medium | Network overhead | YAGNI for v0.1; manual smoke; debounce in M8 if needed. |
| `set_active` interrupts encoded frame mid-flight → glitch | Medium | 100–200ms freeze on switch | Acceptable (matches spec ~200ms target); video track stays attached so iOS shows last frame frozen. |

---

## Out of Scope (M7 explicitly does NOT do)

- 缩略图大小自适应（v0.1 固定 256×192 @ Q70）
- "录像式" history（armed 期间事件不重放）
- 多 active 流并发
- 用户编辑白名单（hardcoded 3 个 bundles）
- 上限 8 的 UI 编辑器（M8 polish）
- 多屏 / multi-display 坐标系
- 同 bundle 多窗口选择器
- supervise_launch URL scheme / 启动参数
- AX 私有 API `_AXUIElementGetWindow` 精确匹配（M8 polish；v0.1 启发式 bounds 距离）
- TestFlight 文案 / 错误代码本地化（M8）
- ViewportChanged 防抖（v0.1 直发；M8 加 200ms debounce）

---

## 自检 (run before declaring plan ready)

1. **Spec coverage** — spec sections § map to tasks:
   - §1 Scope IN → Tasks M7.1–M7.11
   - §2 Protocol → Task M7.1
   - §3 Mac architecture → Tasks M7.2–M7.8
   - §4 iOS UX → Tasks M7.9–M7.11
   - §5 Testing → embedded in tasks; manual = M7.12
   - §6 Files → File Structure section
   - §7 Risks → Risks + Mitigations section
   - §8 Out of Scope → Out of Scope section

2. **No placeholders** — every step has real code or commands. No "TBD", "implement appropriate", "similar to above".

3. **Type consistency** —
   - Rust `Viewport { w: u32, h: u32 }` ⇄ Swift `Viewport(w: UInt32, h: UInt32)` ✓
   - Rust `WindowRect { x, y, w, h: i32 }` ⇄ Swift `WindowRect(x, y, w, h: Int32)` ✓
   - Rust `SupStatus` lowercase JSON `"active"|"armed"|"dead"` ⇄ Swift enum same ✓
   - `SupervisionEntry.status` / `original_frame` / `thumb_jpeg_b64` consistent across all references ✓

4. **Bite-sized tasks** — most tasks ≤7 steps; M7.7 (router rewrite) is the largest with ~6 steps + sub-changes.

5. **CLAUDE.md alignment**:
   - 简单优先: aspect-fit math is the simplest "keep width, derive height". No multi-fallback strategies. JPEG fixed at 256×192 Q70.
   - 精准改动: 4 new ctrl variants + 3 entry fields. No drive-by refactors.
   - 不偷懒: AX failure returns FitFailed, not silent fallback.

---

## Plan 完成后下一步

Suggested execution: **Subagent-Driven** (per established pattern).

Estimate per task:
- M7.1 — 35 min (protocol + Swift mirrors + 6 tests)
- M7.2 — 20 min (pure helpers + 4 tests)
- M7.3 — 50 min (AX FFI; highest single risk after M6.4)
- M7.4 — 30 min (NSWorkspace open + window detect)
- M7.5 — 50 min (JPEG via CGImageDestination; FFI fragile)
- M7.6 — 25 min (FrameSink last_frame + demote_to_armed wiring)
- M7.7 — 60 min (router rewrite; centerpiece)
- M7.8 — 15 min (drainer wiring)
- M7.9 — 25 min (iOS Store actions)
- M7.10 — 50 min (3 SwiftUI views + nav rewire)
- M7.11 — 20 min (modifier + toast)
- M7.12 — manual (user)
- M7.13 — review subagent

**Total ~6.5 hours of focused subagent work + 1 manual smoke pass.** Allow 1–2 fixup rounds for M7.3 (AX) and M7.5 (JPEG).
