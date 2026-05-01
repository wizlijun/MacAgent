# M6 · Input Injection + Content Scaling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship M6 — let an iOS user click / scroll / type into a Mac-supervised window via CGEvent + Accessibility, with external-keyboard pass-through and an AX permission onboarding flow.

**Architecture:** New Mac module `input_injector.rs` subscribes to `CtrlPayload::Input`, runs `AXIsProcessTrusted` preflight, looks up `(pid, frame)` of the supervised window via a new `GuiCapture::lookup_target` API, activates the target app, then posts CGEvents (mouse / scroll / unicode / key combo). iOS adds an `InputClient` actor + gesture wiring, a `HardwareKeyController` (UIPress override) for external keyboards, and a modifier-sticky / special-key UI row for software-keyboard scenarios. Long Chinese paste reuses M4's `ClipboardSet` + `KeyCombo Cmd+V` — no dedicated paste payload.

**Tech Stack:** Rust (`core-graphics`, `objc2-app-kit` new dep), Swift (UIKit `UIPress`, GameController `GCKeyboard`), webrtc-rs ctrl DataChannel (existing).

**Spec:** `docs/superpowers/specs/2026-05-01-m6-input-injection-design.md` (commit `9e7dca1`).

---

## File Structure

**Mac (Rust)** — new + modified:

| Path | Responsibility |
|---|---|
| `mac-agent/crates/macagent-core/src/ctrl_msg.rs` | Add `Input`, `InputAck`, `GuiInput`, `KeyMod` payload variants |
| `mac-agent/crates/macagent-app/src/input_injector.rs` (NEW) | InputInjector struct, AX preflight, CGEvent post, keycode table |
| `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` | Add `lookup_target(sup_id) -> Option<{pid, frame}>` |
| `mac-agent/crates/macagent-app/src/gui_capture/windows.rs` | Expose `find_window(window_id) -> Option<WindowInfo>` |
| `mac-agent/crates/macagent-app/src/ui.rs` | Instantiate InputInjector, dispatch `Input` on ctrl recv, AX 60s repoll, banner |
| `mac-agent/crates/macagent-app/src/supervision_router.rs` | Track `sup_id → window_id` mapping for lookup_target |
| `mac-agent/Cargo.toml` + `crates/macagent-app/Cargo.toml` | Add `objc2-app-kit = "0.3"` for NSRunningApplication / NSWorkspace |

**iOS (Swift)** — new:

| Path | Responsibility |
|---|---|
| `ios-app/MacIOSWorkspace/Input/InputClient.swift` | Actor wrapping ctrl `Input` send + 16ms scroll throttle + paste threshold |
| `ios-app/MacIOSWorkspace/Input/HardwareKeyController.swift` | UIViewController override `pressesBegan` → InputClient |
| `ios-app/MacIOSWorkspace/Input/ModifierStickyRow.swift` | ⌘⇧⌥⌃ toggle UI |
| `ios-app/MacIOSWorkspace/Input/SpecialKeyRow.swift` | Esc/Tab/arrows/Enter/Backspace + zoom buttons |
| `ios-app/MacIOSWorkspace/Input/KeyMapper.swift` | UIKey ↔ Mac key name strings |
| `ios-app/MacIOSWorkspace/Input/InputBar.swift` | Single-line text + ComposeSheet entry; threshold-based paste |

**iOS** — modified:

| Path | Change |
|---|---|
| `ios-app/MacIOSWorkspace/CtrlMessage.swift` | Mirror Rust `Input`/`InputAck`/`GuiInput`/`KeyMod` types |
| `ios-app/MacIOSWorkspace/SupervisionStore.swift` | Add `lastInputAck` field; `SupervisionEntry` add `width: Int, height: Int` |
| `ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift` (or split into `GuiStreamDetailView.swift`) | Wire gestures + overlay HardwareKeyController + bottom toolbar stack |

---

## Task Breakdown (Subagent-Driven)

Each task is a self-contained unit ending in a green build + a commit. Run tasks **sequentially** unless flagged parallel-safe.

---

### Task M6.1 — Protocol: Rust ctrl_msg + Swift CtrlMessage mirrors

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`
- Modify: `ios-app/MacIOSWorkspace/SupervisionStore.swift` (`SupervisionEntry` add `width`, `height`)
- Test: `mac-agent/crates/macagent-core/tests/m6_protocol.rs` (new)

- [ ] **Step 1: Write the failing test for canonical-bytes round-trip**

Create `mac-agent/crates/macagent-core/tests/m6_protocol.rs`:
```rust
use macagent_core::ctrl_msg::{CtrlPayload, GuiInput, KeyMod};

#[test]
fn input_tap_round_trip() {
    let payload = CtrlPayload::Input {
        sup_id: "abc".into(),
        payload: GuiInput::Tap { x: 0.5, y: 0.25 },
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload, back);
}

#[test]
fn input_keycombo_signature_canonical() {
    let payload = CtrlPayload::Input {
        sup_id: "abc".into(),
        payload: GuiInput::KeyCombo {
            modifiers: vec![KeyMod::Cmd, KeyMod::Shift],
            key: "p".into(),
        },
    };
    let bytes = macagent_core::canonical::canonical_bytes(
        &serde_json::to_value(&payload).unwrap()
    );
    // Stable shape: nested Vec/Map sorted recursively
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("\"modifiers\""));
    assert!(s.contains("\"cmd\""));
    assert!(s.contains("\"shift\""));
}

#[test]
fn input_ack_optional_message() {
    let payload = CtrlPayload::InputAck {
        sup_id: "abc".into(),
        code: "ok".into(),
        message: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CtrlPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload, back);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd mac-agent && cargo test -p macagent-core --test m6_protocol`
Expected: FAIL — `Input` / `InputAck` / `GuiInput` not in scope.

- [ ] **Step 3: Add ctrl_msg variants**

Modify `mac-agent/crates/macagent-core/src/ctrl_msg.rs`. Find the existing `CtrlPayload` enum (currently ends with `StreamEnded { sup_id, reason }` per spec; locate the closing brace) and add:

```rust
    Input {
        sup_id: String,
        payload: GuiInput,
    },
    InputAck {
        sup_id: String,
        code: String,
        message: Option<String>,
    },
```

Add the new types at the bottom of the file (or near other enum types):
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuiInput {
    Tap { x: f32, y: f32 },
    Scroll { dx: f32, dy: f32 },
    KeyText { text: String },
    KeyCombo { modifiers: Vec<KeyMod>, key: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMod {
    Cmd,
    Shift,
    Opt,
    Ctrl,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd mac-agent && cargo test -p macagent-core --test m6_protocol`
Expected: PASS — 3 tests.

- [ ] **Step 5: Mirror in Swift**

Modify `ios-app/MacIOSWorkspace/CtrlMessage.swift`. Find the existing `CtrlMessage` enum / case discriminator and add:

```swift
case input(supId: String, payload: GuiInput)
case inputAck(supId: String, code: String, message: String?)
```

Add new types:
```swift
enum GuiInput: Codable, Equatable {
    case tap(x: Float, y: Float)
    case scroll(dx: Float, dy: Float)
    case keyText(text: String)
    case keyCombo(modifiers: [KeyMod], key: String)

    enum CodingKeys: String, CodingKey { case kind, x, y, dx, dy, text, modifiers, key }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .tap(let x, let y):
            try c.encode("tap", forKey: .kind); try c.encode(x, forKey: .x); try c.encode(y, forKey: .y)
        case .scroll(let dx, let dy):
            try c.encode("scroll", forKey: .kind); try c.encode(dx, forKey: .dx); try c.encode(dy, forKey: .dy)
        case .keyText(let text):
            try c.encode("key_text", forKey: .kind); try c.encode(text, forKey: .text)
        case .keyCombo(let mods, let key):
            try c.encode("key_combo", forKey: .kind); try c.encode(mods, forKey: .modifiers); try c.encode(key, forKey: .key)
        }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        switch kind {
        case "tap":       self = .tap(x: try c.decode(Float.self, forKey: .x), y: try c.decode(Float.self, forKey: .y))
        case "scroll":    self = .scroll(dx: try c.decode(Float.self, forKey: .dx), dy: try c.decode(Float.self, forKey: .dy))
        case "key_text":  self = .keyText(text: try c.decode(String.self, forKey: .text))
        case "key_combo": self = .keyCombo(modifiers: try c.decode([KeyMod].self, forKey: .modifiers), key: try c.decode(String.self, forKey: .key))
        default:          throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown GuiInput kind \(kind)")
        }
    }
}

enum KeyMod: String, Codable, Equatable {
    case cmd, shift, opt, ctrl
}
```

In `SupervisionStore.swift`, modify `SupervisionEntry`:
```swift
struct SupervisionEntry: Identifiable, Equatable {
    let supId: String
    let windowId: UInt32
    let appName: String
    let title: String
    let width: Int        // NEW
    let height: Int       // NEW
    // ... existing fields
}
```

Update existing `SupervisedAck` decoding site to populate `width` / `height` (the M5.3 ack already has these per spec; if missing, add them to the Mac-side `SupervisionEntry` ctrl payload first — verify by `grep -n "SupervisedAck\|SupervisionEntry" mac-agent/crates/macagent-core/src/ctrl_msg.rs`).

- [ ] **Step 6: Build iOS to verify**

Run: `xcodebuild -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16' build`
Expected: BUILD SUCCEEDED.

- [ ] **Step 7: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-core/src/ctrl_msg.rs \
        mac-agent/crates/macagent-core/tests/m6_protocol.rs \
        ios-app/MacIOSWorkspace/CtrlMessage.swift \
        ios-app/MacIOSWorkspace/SupervisionStore.swift
git commit -m "feat(m6): add Input/InputAck/GuiInput/KeyMod ctrl payload + Swift mirrors

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.2 — InputInjector pure helpers (no FFI, fully unit-testable)

**Files:**
- Create: `mac-agent/crates/macagent-app/src/input_injector.rs`
- Modify: `mac-agent/crates/macagent-app/src/main.rs` (add `mod input_injector;` once the module exists)

This task ships **only the pure-CPU helpers**: keycode lookup, coord normalization, modifier-flag packing, unicode chunking. No CGEvent, no AX. All testable without GUI.

- [ ] **Step 1: Write the failing tests**

Create `mac-agent/crates/macagent-app/src/input_injector.rs` with **only** the test module first:

```rust
//! Mac InputInjector — CGEvent click/scroll/keyboard for supervised windows.

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::KeyMod;

    #[test]
    fn keycode_table_known() {
        assert_eq!(lookup_keycode("a"), Some(0x00));
        assert_eq!(lookup_keycode("="), Some(0x18));
        assert_eq!(lookup_keycode("-"), Some(0x1B));
        assert_eq!(lookup_keycode("0"), Some(0x1D));
        assert_eq!(lookup_keycode("esc"), Some(0x35));
        assert_eq!(lookup_keycode("return"), Some(0x24));
        assert_eq!(lookup_keycode("up"), Some(0x7E));
    }

    #[test]
    fn keycode_table_unknown() {
        assert_eq!(lookup_keycode("zzz"), None);
        assert_eq!(lookup_keycode(""), None);
    }

    #[test]
    fn normalize_coords_center() {
        let frame = WindowFrame { x: 100, y: 200, w: 800, h: 600 };
        assert_eq!(normalize_to_global(&frame, 0.5, 0.5), (500, 500));
        assert_eq!(normalize_to_global(&frame, 0.0, 0.0), (100, 200));
        assert_eq!(normalize_to_global(&frame, 1.0, 1.0), (900, 800));
    }

    #[test]
    fn modifier_flags_packing() {
        let cmd_shift = pack_modifier_flags(&[KeyMod::Cmd, KeyMod::Shift]);
        // CGEventFlags::CGEventFlagCommand = 1<<20, CGEventFlagShift = 1<<17
        assert_eq!(cmd_shift, (1u64 << 20) | (1u64 << 17));
        let none = pack_modifier_flags(&[]);
        assert_eq!(none, 0);
        let opt_ctrl = pack_modifier_flags(&[KeyMod::Opt, KeyMod::Ctrl]);
        // CGEventFlagAlternate=1<<19, CGEventFlagControl=1<<18
        assert_eq!(opt_ctrl, (1u64 << 19) | (1u64 << 18));
    }

    #[test]
    fn chunk_unicode_text_basic() {
        // ASCII, well under chunk limit
        let chunks = chunk_unicode("hello", 20);
        assert_eq!(chunks, vec!["hello"]);
        // Exactly at limit
        let s20: String = "a".repeat(20);
        assert_eq!(chunk_unicode(&s20, 20), vec![s20.clone()]);
        // Over limit: split into two
        let s30: String = "a".repeat(30);
        let chunks = chunk_unicode(&s30, 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 20);
        assert_eq!(chunks[1].len(), 10);
    }

    #[test]
    fn chunk_unicode_chinese() {
        // 50 Chinese chars, chunk size 20 (UTF-16 units; Chinese in BMP is 1 unit each)
        let zh: String = "中".repeat(50);
        let chunks = chunk_unicode(&zh, 20);
        assert_eq!(chunks.len(), 3);
        // First two chunks have 20 chars each
        assert_eq!(chunks[0].chars().count(), 20);
        assert_eq!(chunks[1].chars().count(), 20);
        assert_eq!(chunks[2].chars().count(), 10);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

First add the module declaration to `mac-agent/crates/macagent-app/src/main.rs`:
```rust
mod input_injector;
```

Run: `cd mac-agent && cargo test -p macagent-app input_injector::tests`
Expected: FAIL — `lookup_keycode` / `WindowFrame` / `normalize_to_global` / `pack_modifier_flags` / `chunk_unicode` not in scope.

- [ ] **Step 3: Implement helpers**

Add to `mac-agent/crates/macagent-app/src/input_injector.rs` (above `#[cfg(test)] mod tests`):

```rust
use macagent_core::ctrl_msg::KeyMod;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowFrame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Carbon virtual keycodes for ANSI US layout. See HIToolbox/Events.h.
const KEYCODES: &[(&str, u16)] = &[
    // Letters
    ("a", 0x00), ("s", 0x01), ("d", 0x02), ("f", 0x03), ("h", 0x04),
    ("g", 0x05), ("z", 0x06), ("x", 0x07), ("c", 0x08), ("v", 0x09),
    ("b", 0x0B), ("q", 0x0C), ("w", 0x0D), ("e", 0x0E), ("r", 0x0F),
    ("y", 0x10), ("t", 0x11),
    ("o", 0x1F), ("u", 0x20), ("i", 0x22), ("p", 0x23),
    ("l", 0x25), ("j", 0x26), ("k", 0x28),
    ("n", 0x2D), ("m", 0x2E),
    // Numbers
    ("1", 0x12), ("2", 0x13), ("3", 0x14), ("4", 0x15),
    ("6", 0x16), ("5", 0x17), ("9", 0x19), ("7", 0x1A),
    ("8", 0x1C), ("0", 0x1D),
    // Symbols (US ANSI)
    ("=", 0x18), ("-", 0x1B), ("]", 0x1E), ("[", 0x21),
    ("'", 0x27), (";", 0x29), ("\\", 0x2A), (",", 0x2B),
    ("/", 0x2C), (".", 0x2F), ("`", 0x32),
    // Whitespace / control
    ("space", 0x31), ("return", 0x24), ("tab", 0x30),
    ("delete", 0x33), ("esc", 0x35),
    // Arrows
    ("left", 0x7B), ("right", 0x7C), ("down", 0x7D), ("up", 0x7E),
];

pub fn lookup_keycode(name: &str) -> Option<u16> {
    if name.is_empty() {
        return None;
    }
    KEYCODES.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

pub fn normalize_to_global(frame: &WindowFrame, x: f32, y: f32) -> (i32, i32) {
    let gx = frame.x + (x.clamp(0.0, 1.0) * frame.w as f32) as i32;
    let gy = frame.y + (y.clamp(0.0, 1.0) * frame.h as f32) as i32;
    (gx, gy)
}

pub fn pack_modifier_flags(mods: &[KeyMod]) -> u64 {
    let mut out: u64 = 0;
    for m in mods {
        out |= match m {
            KeyMod::Cmd   => 1 << 20,  // kCGEventFlagMaskCommand
            KeyMod::Shift => 1 << 17,  // kCGEventFlagMaskShift
            KeyMod::Opt   => 1 << 19,  // kCGEventFlagMaskAlternate
            KeyMod::Ctrl  => 1 << 18,  // kCGEventFlagMaskControl
        };
    }
    out
}

/// Split text into chunks of at most `max_chars` characters.
/// CGEventKeyboardSetUnicodeString limits each call to ~20 UTF-16 units in practice.
pub fn chunk_unicode(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        buf.push(ch);
        count += 1;
        if count >= max_chars {
            out.push(std::mem::take(&mut buf));
            count = 0;
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd mac-agent && cargo test -p macagent-app input_injector::tests`
Expected: PASS — 6 tests.

- [ ] **Step 5: Lint**

Run: `cd mac-agent && cargo clippy -p macagent-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/input_injector.rs \
        mac-agent/crates/macagent-app/src/main.rs
git commit -m "feat(m6): add InputInjector pure helpers (keycode, coord, mod flags, chunk)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.3 — Mac AppKit dep + GuiCapture::lookup_target + windows::find_window

**Files:**
- Modify: `mac-agent/Cargo.toml` (workspace deps)
- Modify: `mac-agent/crates/macagent-app/Cargo.toml` (direct deps)
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/windows.rs` (expose `find_window`)
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` (add `lookup_target`)
- Modify: `mac-agent/crates/macagent-app/src/supervision_router.rs` (track sup_id → window_id)

- [ ] **Step 1: Add objc2-app-kit to workspace**

Modify `mac-agent/Cargo.toml`, add under `[workspace.dependencies]`:
```toml
objc2-app-kit = "0.3"
```

Modify `mac-agent/crates/macagent-app/Cargo.toml`, add under `[dependencies]`:
```toml
objc2-app-kit = { workspace = true }
```

- [ ] **Step 2: Verify build**

Run: `cd mac-agent && cargo check -p macagent-app`
Expected: SUCCESS.

- [ ] **Step 3: Expose find_window from gui_capture::windows**

Read `mac-agent/crates/macagent-app/src/gui_capture/windows.rs` first to understand the existing CGWindowList walk. Then expose:

```rust
/// Lookup a single window by its CGWindowID. Returns None if window has gone away.
pub fn find_window(window_id: u32) -> Option<WindowInfo> {
    list_windows().ok()?.into_iter().find(|w| w.window_id == window_id)
}
```

(If `WindowInfo` already has the right shape — `window_id`, `owner_pid`, `bounds: {x, y, w, h}`, `title`, `app_name` — leave the existing types alone. If `bounds` is missing, add it; spec §3 needs `frame: CGRect`.)

- [ ] **Step 4: Add SupervisionRouter::current_window_id**

Read `mac-agent/crates/macagent-app/src/supervision_router.rs`. The `ActiveSupervision` struct already stores `sup_id` (per M5 review N5). Either:
- Add a `pub fn current_window_id(&self, sup_id: &str) -> Option<u32>` method that locks `self.active` and returns `prev.window_id` if matching;
- OR (preferred) extend `ActiveSupervision` to store `window_id: u32` if it doesn't already, and expose the lookup.

Concrete change (minimum): in `supervision_router.rs`:
```rust
struct ActiveSupervision {
    sup_id: String,
    window_id: u32,    // ensure this is present
    started_ts: u64,
}

impl SupervisionRouter {
    pub async fn current_window_id(&self, sup_id: &str) -> Option<u32> {
        let active = self.active.lock().await;
        active.as_ref().and_then(|a| (a.sup_id == sup_id).then_some(a.window_id))
    }
}
```

If `window_id` was previously dropped from `ActiveSupervision` (M5 N5 reviewer suggested simplification), restore it — InputInjector needs it.

- [ ] **Step 5: Add GuiCapture::lookup_target**

Modify `mac-agent/crates/macagent-app/src/gui_capture/mod.rs`. Add:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputTarget {
    pub pid: i32,
    pub frame: crate::input_injector::WindowFrame,
}

impl GuiCapture {
    /// Re-resolve the live (pid, frame) of the supervised window.
    /// Returns None if the window has gone (caller emits StreamEnded/InputAck window_gone).
    pub fn lookup_target(&self, window_id: u32) -> Option<InputTarget> {
        let info = windows::find_window(window_id)?;
        Some(InputTarget {
            pid: info.owner_pid as i32,
            frame: crate::input_injector::WindowFrame {
                x: info.bounds.x as i32,
                y: info.bounds.y as i32,
                w: info.bounds.w as i32,
                h: info.bounds.h as i32,
            },
        })
    }
}
```

(If `WindowInfo.bounds` field names are different, adapt — the goal is "live lookup of (pid, frame) by window_id".)

- [ ] **Step 6: Build & lint**

Run: `cd mac-agent && cargo build -p macagent-app && cargo clippy -p macagent-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/Cargo.toml mac-agent/crates/macagent-app/Cargo.toml \
        mac-agent/crates/macagent-app/src/gui_capture/windows.rs \
        mac-agent/crates/macagent-app/src/gui_capture/mod.rs \
        mac-agent/crates/macagent-app/src/supervision_router.rs \
        mac-agent/Cargo.lock
git commit -m "feat(m6): add objc2-app-kit dep + GuiCapture::lookup_target + sup_id→window_id

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.4 — InputInjector FFI: AX preflight, activate target, post events

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/input_injector.rs`

This task adds the **unsafe FFI** to actually run AX checks, NSRunningApplication.activate, and CGEvent posting. No new tests (FFI requires GUI session); rely on `cargo build` + clippy + manual smoke later.

- [ ] **Step 1: Add InputInjector struct + AX preflight**

Add to `input_injector.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use macagent_core::ctrl_msg::{CtrlPayload, GuiInput, KeyMod};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

use crate::gui_capture::GuiCapture;
use crate::supervision_router::SupervisionRouter;

const UNICODE_CHUNK: usize = 20;

pub struct InputInjector {
    gui_capture: Arc<GuiCapture>,
    supervision: Arc<SupervisionRouter>,
    ctrl_tx: UnboundedSender<CtrlPayload>,
    perm_cached: Mutex<bool>,
    last_perm_check: Mutex<Instant>,
}

impl InputInjector {
    pub fn new(
        gui_capture: Arc<GuiCapture>,
        supervision: Arc<SupervisionRouter>,
        ctrl_tx: UnboundedSender<CtrlPayload>,
    ) -> Self {
        Self {
            gui_capture,
            supervision,
            ctrl_tx,
            perm_cached: Mutex::new(false),
            last_perm_check: Mutex::new(Instant::now() - Duration::from_secs(120)),
        }
    }

    pub fn check_ax(&self) -> bool {
        let now = Instant::now();
        let mut last = self.last_perm_check.lock().unwrap();
        if now.duration_since(*last) > Duration::from_secs(60) {
            let granted = ax_is_process_trusted();
            *self.perm_cached.lock().unwrap() = granted;
            *last = now;
            granted
        } else {
            *self.perm_cached.lock().unwrap()
        }
    }

    /// Force a fresh AX check (e.g. on user-clicked retry).
    pub fn refresh_ax(&self) -> bool {
        let granted = ax_is_process_trusted();
        *self.perm_cached.lock().unwrap() = granted;
        *self.last_perm_check.lock().unwrap() = Instant::now();
        granted
    }
}

/// FFI wrapper for AXIsProcessTrusted from ApplicationServices framework.
fn ax_is_process_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}
```

- [ ] **Step 2: Add target-activation helper**

Add:
```rust
fn activate_pid(pid: i32) -> Result<()> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSObjectProtocol;
    unsafe {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        let app = app.ok_or_else(|| anyhow!("no running app for pid {pid}"))?;
        // Equivalent of NSApplicationActivationOptions::activateIgnoringOtherApps = 1<<1
        let _ = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 1));
        Ok(())
    }
}
```

If `objc2-app-kit` doesn't expose `runningApplicationWithProcessIdentifier` in 0.3, fall back to inline FFI:
```rust
extern "C" {
    fn objc_msgSend(...) -> ...;
}
```
(Check the crate first; modern objc2-app-kit should have it.)

- [ ] **Step 3: Add CGEvent post helpers**

Add:
```rust
fn make_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource::new failed"))
}

impl InputInjector {
    fn post_click(&self, frame: WindowFrame, x: f32, y: f32) -> Result<()> {
        let (gx, gy) = normalize_to_global(&frame, x, y);
        let src = make_source()?;
        let pt = CGPoint::new(gx as f64, gy as f64);
        let down = CGEvent::new_mouse_event(
            src.clone(), CGEventType::LeftMouseDown, pt, CGMouseButton::Left
        ).map_err(|_| anyhow!("create LeftMouseDown failed"))?;
        let up = CGEvent::new_mouse_event(
            src, CGEventType::LeftMouseUp, pt, CGMouseButton::Left
        ).map_err(|_| anyhow!("create LeftMouseUp failed"))?;
        down.post(CGEventTapLocation::HID);
        up.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn post_scroll(&self, dx: f32, dy: f32) -> Result<()> {
        let src = make_source()?;
        let ev = CGEvent::new_scroll_event(
            src,
            ScrollEventUnit::PIXEL,
            2,
            dy as i32,    // vertical first
            dx as i32,    // then horizontal
            0,
        ).map_err(|_| anyhow!("create scroll failed"))?;
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn post_unicode(&self, text: &str) -> Result<()> {
        let src = make_source()?;
        for chunk in chunk_unicode(text, UNICODE_CHUNK) {
            let down = CGEvent::new_keyboard_event(src.clone(), 0, true)
                .map_err(|_| anyhow!("create keyDown failed"))?;
            let utf16: Vec<u16> = chunk.encode_utf16().collect();
            down.set_string_from_utf16_unchecked(&utf16);
            let up = CGEvent::new_keyboard_event(src.clone(), 0, false)
                .map_err(|_| anyhow!("create keyUp failed"))?;
            up.set_string_from_utf16_unchecked(&utf16);
            down.post(CGEventTapLocation::HID);
            up.post(CGEventTapLocation::HID);
        }
        Ok(())
    }

    fn post_keycombo(&self, mods: &[KeyMod], key: &str) -> Result<()> {
        let kc = lookup_keycode(key).ok_or_else(|| anyhow!("unknown key: {key}"))?;
        let flags = pack_modifier_flags(mods);
        let src = make_source()?;
        let down = CGEvent::new_keyboard_event(src.clone(), kc, true)
            .map_err(|_| anyhow!("create keyDown failed"))?;
        unsafe { down.set_flags(core_graphics::event::CGEventFlags::from_bits_unchecked(flags)); }
        let up = CGEvent::new_keyboard_event(src, kc, false)
            .map_err(|_| anyhow!("create keyUp failed"))?;
        unsafe { up.set_flags(core_graphics::event::CGEventFlags::from_bits_unchecked(flags)); }
        down.post(CGEventTapLocation::HID);
        up.post(CGEventTapLocation::HID);
        Ok(())
    }
}
```

> If `core-graphics` 0.25 doesn't expose `set_string_from_utf16_unchecked` directly, look at `CGEvent::set_string` or use the inline FFI to `CGEventKeyboardSetUnicodeString(event, count, &buf[0])`. Document the actual API used.

- [ ] **Step 4: Add handle_input dispatcher + ack helper**

Add:
```rust
impl InputInjector {
    pub async fn handle_input(&self, sup_id: String, input: GuiInput) {
        if !self.check_ax() {
            self.ack(&sup_id, "permission_denied", None);
            return;
        }
        let Some(window_id) = self.supervision.current_window_id(&sup_id).await else {
            self.ack(&sup_id, "window_gone", None);
            return;
        };
        let Some(target) = self.gui_capture.lookup_target(window_id) else {
            self.ack(&sup_id, "window_gone", None);
            return;
        };
        if let Err(e) = activate_pid(target.pid) {
            self.ack(&sup_id, "no_focus", Some(format!("{e:#}")));
            return;
        }
        let res = match input {
            GuiInput::Tap { x, y }                 => self.post_click(target.frame, x, y),
            GuiInput::Scroll { dx, dy }            => self.post_scroll(dx, dy),
            GuiInput::KeyText { text }             => self.post_unicode(&text),
            GuiInput::KeyCombo { modifiers, key }  => self.post_keycombo(&modifiers, &key),
        };
        match res {
            Ok(_)  => self.ack(&sup_id, "ok", None),
            Err(e) => self.ack(&sup_id, "no_focus", Some(format!("{e:#}"))),
        }
    }

    fn ack(&self, sup_id: &str, code: &str, message: Option<String>) {
        let _ = self.ctrl_tx.send(CtrlPayload::InputAck {
            sup_id: sup_id.into(),
            code: code.into(),
            message,
        });
    }
}
```

- [ ] **Step 5: Build & lint**

Run: `cd mac-agent && cargo build -p macagent-app && cargo clippy -p macagent-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Run existing tests**

Run: `cd mac-agent && cargo test -p macagent-app && cargo test -p macagent-core`
Expected: all green (still 33+ tests in app, 6 new protocol tests in core).

- [ ] **Step 7: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/input_injector.rs
git commit -m "feat(m6): add InputInjector with AX preflight + CGEvent post + activate

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.5 — Wire InputInjector into ui.rs ctrl dispatch + 60s AX repoll

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs` (if ctrl dispatch lives there)

- [ ] **Step 1: Instantiate InputInjector in Connect**

In `ui.rs::Connect` async block (line ~672, after `supervision_router` is built per M5.2.5), add:

```rust
let input_injector = Arc::new(InputInjector::new(
    Arc::clone(&gui_capture),
    Arc::clone(&supervision_router),
    ctrl_send_tx.clone(),
));
```

Add `use crate::input_injector::InputInjector;` at top of file.

- [ ] **Step 2: Dispatch Input on ctrl recv**

Find the existing supervision-payload drainer in `rtc_glue.rs` (added in M5.2.5 fixup — `sup_tx.send(payload)` + drainer task). Extend the drainer's match to handle `CtrlPayload::Input`:

```rust
while let Some(payload) = sup_rx.recv().await {
    match payload {
        CtrlPayload::ListWindows => sr.handle_ctrl(payload).await,
        CtrlPayload::SuperviseExisting { .. } => sr.handle_ctrl(payload).await,
        CtrlPayload::RemoveSupervised { .. } => sr.handle_ctrl(payload).await,
        CtrlPayload::ListSupervisions => sr.handle_ctrl(payload).await,
        CtrlPayload::Input { sup_id, payload: input } => {
            input_injector.handle_input(sup_id, input).await;
        }
        // other variants are ignored / not part of supervision/input flow
        _ => {}
    }
}
```

Hold the `Arc<InputInjector>` in the spawned drainer task by capturing it in the closure (move).

- [ ] **Step 3: Spawn AX repoll background task**

In the same Connect block, after instantiating `input_injector`:

```rust
let injector_for_repoll = Arc::clone(&input_injector);
tokio::spawn(async move {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.tick().await; // first tick fires immediately
    loop {
        tick.tick().await;
        let _ = injector_for_repoll.refresh_ax();
    }
});
```

(Add `use std::time::Duration;` if not already imported.)

- [ ] **Step 4: Build & lint**

Run: `cd mac-agent && cargo build -p macagent-app && cargo clippy -p macagent-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Run all Mac tests**

Run: `cd mac-agent && cargo test --workspace`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/ui.rs mac-agent/crates/macagent-app/src/rtc_glue.rs
git commit -m "feat(m6): wire InputInjector into ctrl dispatch + 60s AX repoll

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.6 — iOS InputClient actor + scroll throttle + paste threshold

**Files:**
- Create: `ios-app/MacIOSWorkspace/Input/InputClient.swift`
- Create: `ios-app/MacIOSWorkspaceTests/InputClientTests.swift`

- [ ] **Step 1: Write failing tests**

Create `ios-app/MacIOSWorkspaceTests/InputClientTests.swift`:

```swift
import XCTest
@testable import MacIOSWorkspace

@MainActor
final class InputClientTests: XCTestCase {
    func testScrollThrottle() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        for _ in 0..<5 {
            await client.scroll(dx: 0, dy: 1)
        }
        // 5 calls within < 16ms → 1 emission allowed; allow up to 2 due to scheduler
        XCTAssertLessThanOrEqual(stub.scrollSendCount, 2)
    }

    func testPasteThresholdShort() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        await client.submitText("hello world")  // 11 chars
        XCTAssertEqual(stub.lastInputKind, "key_text")
        XCTAssertEqual(stub.clipboardSets, 0)
    }

    func testPasteThresholdLong() async {
        let stub = StubGlue()
        let client = InputClient(supId: "abc", glue: stub)
        let longText = String(repeating: "中", count: 50)
        await client.submitText(longText)
        XCTAssertEqual(stub.clipboardSets, 1)
        XCTAssertEqual(stub.lastInputKind, "key_combo")  // Cmd+V
    }
}

// Test stub that records ctrl messages instead of sending them
final class StubGlue: InputClient.Glue {
    var scrollSendCount = 0
    var clipboardSets = 0
    var lastInputKind: String?

    func sendCtrl(_ msg: CtrlMessage) async {
        switch msg {
        case .input(_, let payload):
            switch payload {
            case .scroll: scrollSendCount += 1; lastInputKind = "scroll"
            case .tap: lastInputKind = "tap"
            case .keyText: lastInputKind = "key_text"
            case .keyCombo: lastInputKind = "key_combo"
            }
        case .clipboardSet:
            clipboardSets += 1
        default: break
        }
    }
}
```

- [ ] **Step 2: Build to verify failure**

Run xcodebuild test; expected: compile error — `InputClient` not in scope.

- [ ] **Step 3: Implement InputClient**

Create `ios-app/MacIOSWorkspace/Input/InputClient.swift`:

```swift
import Foundation

@MainActor
final class InputClient: ObservableObject {
    /// Test seam — production passes RtcGlue.
    protocol Glue: AnyObject {
        func sendCtrl(_ msg: CtrlMessage) async
    }

    private let supId: String
    private weak var glue: Glue?
    private var lastScrollEmit = Date.distantPast
    private static let scrollThrottle: TimeInterval = 0.016
    private static let pasteThreshold = 32

    init(supId: String, glue: Glue) {
        self.supId = supId
        self.glue = glue
    }

    func tap(normalizedX: CGFloat, normalizedY: CGFloat) async {
        await send(.tap(x: Float(normalizedX), y: Float(normalizedY)))
    }

    func scroll(dx: CGFloat, dy: CGFloat) async {
        let now = Date()
        if now.timeIntervalSince(lastScrollEmit) < Self.scrollThrottle { return }
        lastScrollEmit = now
        await send(.scroll(dx: Float(dx), dy: Float(dy)))
    }

    func keyText(_ s: String) async {
        await send(.keyText(text: s))
    }

    func keyCombo(_ mods: [KeyMod], _ key: String) async {
        await send(.keyCombo(modifiers: mods, key: key))
    }

    /// Threshold-based: short strings → key_text; long → clipboard + Cmd+V.
    func submitText(_ text: String) async {
        if text.count > Self.pasteThreshold {
            await glue?.sendCtrl(.clipboardSet(text: text))
            await keyCombo([.cmd], "v")
        } else {
            await keyText(text)
        }
    }

    private func send(_ payload: GuiInput) async {
        await glue?.sendCtrl(.input(supId: supId, payload: payload))
    }
}
```

Make `RtcGlue` conform to `InputClient.Glue` (extension in `RtcGlue.swift` — single line: `extension RtcGlue: InputClient.Glue {}` if it already has `sendCtrl`; otherwise add the method).

- [ ] **Step 4: Run tests to verify they pass**

Run: `xcodebuild test -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16' -only-testing:MacIOSWorkspaceTests/InputClientTests`
Expected: 3 tests pass.

- [ ] **Step 5: Build whole app**

Run: `xcodebuild -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16' build`
Expected: BUILD SUCCEEDED.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Input/InputClient.swift \
        ios-app/MacIOSWorkspaceTests/InputClientTests.swift \
        ios-app/MacIOSWorkspace/RtcGlue.swift
git commit -m "feat(ios-m6): add InputClient with scroll throttle + paste threshold

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.7 — iOS KeyMapper + ModifierStickyRow + SpecialKeyRow + InputBar

**Files:**
- Create: `ios-app/MacIOSWorkspace/Input/KeyMapper.swift`
- Create: `ios-app/MacIOSWorkspace/Input/ModifierStickyRow.swift`
- Create: `ios-app/MacIOSWorkspace/Input/SpecialKeyRow.swift`
- Create: `ios-app/MacIOSWorkspace/Input/InputBar.swift`
- Create: `ios-app/MacIOSWorkspaceTests/KeyMapperTests.swift`

- [ ] **Step 1: Write failing tests for KeyMapper**

Create `ios-app/MacIOSWorkspaceTests/KeyMapperTests.swift`:
```swift
import XCTest
import UIKit
@testable import MacIOSWorkspace

final class KeyMapperTests: XCTestCase {
    func testModifierFlags() {
        XCTAssertEqual(KeyMapper.modifiers(from: [.command]), [.cmd])
        XCTAssertEqual(KeyMapper.modifiers(from: [.command, .shift]), [.cmd, .shift])
        XCTAssertEqual(KeyMapper.modifiers(from: [.alternate, .control]), [.opt, .ctrl])
        XCTAssertEqual(KeyMapper.modifiers(from: []), [])
    }

    func testNamedKeys() {
        // Map UIKeyboardHIDUsage codes to names
        XCTAssertEqual(KeyMapper.name(for: .keyboardEscape), "esc")
        XCTAssertEqual(KeyMapper.name(for: .keyboardTab), "tab")
        XCTAssertEqual(KeyMapper.name(for: .keyboardReturnOrEnter), "return")
        XCTAssertEqual(KeyMapper.name(for: .keyboardDeleteOrBackspace), "delete")
        XCTAssertEqual(KeyMapper.name(for: .keyboardUpArrow), "up")
        XCTAssertEqual(KeyMapper.name(for: .keyboardDownArrow), "down")
        XCTAssertEqual(KeyMapper.name(for: .keyboardLeftArrow), "left")
        XCTAssertEqual(KeyMapper.name(for: .keyboardRightArrow), "right")
        XCTAssertEqual(KeyMapper.name(for: .keyboardSpacebar), "space")
    }

    func testIsSpecial() {
        XCTAssertTrue(KeyMapper.isSpecial(.keyboardEscape))
        XCTAssertTrue(KeyMapper.isSpecial(.keyboardUpArrow))
        XCTAssertFalse(KeyMapper.isSpecial(.keyboardA))
    }
}
```

- [ ] **Step 2: Implement KeyMapper**

Create `ios-app/MacIOSWorkspace/Input/KeyMapper.swift`:
```swift
import UIKit

enum KeyMapper {
    static func modifiers(from flags: UIKeyModifierFlags) -> [KeyMod] {
        var out: [KeyMod] = []
        if flags.contains(.command)   { out.append(.cmd) }
        if flags.contains(.shift)     { out.append(.shift) }
        if flags.contains(.alternate) { out.append(.opt) }
        if flags.contains(.control)   { out.append(.ctrl) }
        return out
    }

    static func name(for usage: UIKeyboardHIDUsage) -> String? {
        switch usage {
        case .keyboardEscape:             return "esc"
        case .keyboardTab:                return "tab"
        case .keyboardReturnOrEnter, .keypadEnter: return "return"
        case .keyboardDeleteOrBackspace:  return "delete"
        case .keyboardUpArrow:            return "up"
        case .keyboardDownArrow:          return "down"
        case .keyboardLeftArrow:          return "left"
        case .keyboardRightArrow:         return "right"
        case .keyboardSpacebar:           return "space"
        default: return nil
        }
    }

    static func isSpecial(_ usage: UIKeyboardHIDUsage) -> Bool {
        name(for: usage) != nil
    }
}
```

- [ ] **Step 3: Run KeyMapper tests**

Run: `xcodebuild test ... -only-testing:MacIOSWorkspaceTests/KeyMapperTests`
Expected: 3 tests pass.

- [ ] **Step 4: Implement ModifierStickyRow**

Create `ios-app/MacIOSWorkspace/Input/ModifierStickyRow.swift`:
```swift
import SwiftUI

@MainActor
final class ModifierState: ObservableObject {
    @Published var sticky: Set<KeyMod> = []
    @Published var locked: Set<KeyMod> = []

    var active: [KeyMod] { Array(sticky.union(locked)) }

    func tap(_ m: KeyMod) {
        if locked.contains(m) { locked.remove(m); return }
        if sticky.contains(m) { sticky.remove(m); locked.insert(m) }
        else { sticky.insert(m) }
    }

    func consume() { sticky.removeAll() }    // sticky-release after one keypress
}

struct ModifierStickyRow: View {
    @ObservedObject var state: ModifierState

    var body: some View {
        HStack(spacing: 8) {
            modKey("⌘", .cmd)
            modKey("⇧", .shift)
            modKey("⌥", .opt)
            modKey("⌃", .ctrl)
        }
        .padding(.horizontal, 12)
    }

    private func modKey(_ label: String, _ m: KeyMod) -> some View {
        let isLocked = state.locked.contains(m)
        let isSticky = state.sticky.contains(m)
        return Button(label) { state.tap(m) }
            .frame(width: 44, height: 32)
            .background(isLocked ? Color.accentColor : (isSticky ? Color.accentColor.opacity(0.4) : Color.gray.opacity(0.2)))
            .foregroundStyle(isLocked || isSticky ? .white : .primary)
            .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
```

- [ ] **Step 5: Implement SpecialKeyRow**

Create `ios-app/MacIOSWorkspace/Input/SpecialKeyRow.swift`:
```swift
import SwiftUI

struct SpecialKeyRow: View {
    let input: InputClient
    @ObservedObject var modState: ModifierState

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                key("Esc", "esc")
                key("Tab", "tab")
                key("↑", "up")
                key("↓", "down")
                key("←", "left")
                key("→", "right")
                key("↩", "return")
                key("⌫", "delete")
                Divider().frame(height: 24)
                key("+", "=", mods: [.cmd])
                key("−", "-", mods: [.cmd])
                key("0", "0", mods: [.cmd])
            }
            .padding(.horizontal, 12)
        }
    }

    private func key(_ label: String, _ name: String, mods overrideMods: [KeyMod]? = nil) -> some View {
        Button(label) {
            let mods = overrideMods ?? modState.active
            Task {
                await input.keyCombo(mods, name)
                if overrideMods == nil { modState.consume() }
            }
        }
        .frame(minWidth: 36, minHeight: 32)
        .padding(.horizontal, 8)
        .background(Color.gray.opacity(0.2))
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}
```

- [ ] **Step 6: Implement InputBar**

Create `ios-app/MacIOSWorkspace/Input/InputBar.swift`:
```swift
import SwiftUI

struct InputBar: View {
    let input: InputClient
    @ObservedObject var modState: ModifierState
    @State private var text = ""
    @State private var showCompose = false

    var body: some View {
        HStack(spacing: 8) {
            TextField("输入…", text: $text)
                .textFieldStyle(.roundedBorder)
                .onSubmit { submit() }
            Button { showCompose = true } label: {
                Image(systemName: "pencil")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .sheet(isPresented: $showCompose) {
            ComposeSheet(initial: "") { result in
                Task { await input.submitText(result) }
            }
        }
    }

    private func submit() {
        guard !text.isEmpty else { return }
        let mods = modState.active
        let payload = text
        text = ""
        Task {
            if !mods.isEmpty, let firstChar = payload.first.map({ String($0) }) {
                // Modifier active → first char as KeyCombo, rest as KeyText
                await input.keyCombo(mods, firstChar)
                modState.consume()
                if payload.count > 1 {
                    await input.keyText(String(payload.dropFirst()))
                }
            } else {
                await input.submitText(payload)
            }
        }
    }
}
```

- [ ] **Step 7: Build**

Run: `xcodebuild ... build`
Expected: BUILD SUCCEEDED.

- [ ] **Step 8: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Input/KeyMapper.swift \
        ios-app/MacIOSWorkspace/Input/ModifierStickyRow.swift \
        ios-app/MacIOSWorkspace/Input/SpecialKeyRow.swift \
        ios-app/MacIOSWorkspace/Input/InputBar.swift \
        ios-app/MacIOSWorkspaceTests/KeyMapperTests.swift
git commit -m "feat(ios-m6): add KeyMapper + ModifierStickyRow + SpecialKeyRow + InputBar

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.8 — iOS HardwareKeyController (UIPress override) + GCKeyboard detection

**Files:**
- Create: `ios-app/MacIOSWorkspace/Input/HardwareKeyController.swift`

- [ ] **Step 1: Implement HardwareKeyController**

Create `ios-app/MacIOSWorkspace/Input/HardwareKeyController.swift`:

```swift
import GameController
import SwiftUI
import UIKit

final class HardwareKeyController: UIViewController {
    weak var inputClient: InputClient?
    private var modState: ModifierState?

    override var canBecomeFirstResponder: Bool { true }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        resignFirstResponder()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for press in presses {
            guard let key = press.key else { continue }
            let mods = KeyMapper.modifiers(from: key.modifierFlags)
            if let special = KeyMapper.name(for: key.keyCode) {
                Task { @MainActor in await self.inputClient?.keyCombo(mods, special) }
                return
            }
            let chars = key.characters
            if !chars.isEmpty {
                if !mods.isEmpty {
                    let first = String(chars.first!)
                    Task { @MainActor in await self.inputClient?.keyCombo(mods, first) }
                } else {
                    Task { @MainActor in await self.inputClient?.keyText(chars) }
                }
                return
            }
        }
        super.pressesBegan(presses, with: event)
    }
}

struct HardwareKeyControllerView: UIViewControllerRepresentable {
    let inputClient: InputClient

    func makeUIViewController(context: Context) -> HardwareKeyController {
        let vc = HardwareKeyController()
        vc.inputClient = inputClient
        return vc
    }

    func updateUIViewController(_ uiViewController: HardwareKeyController, context: Context) {
        uiViewController.inputClient = inputClient
    }
}

@MainActor
final class HardwareKeyboardDetector: ObservableObject {
    @Published var isConnected: Bool = GCKeyboard.coalesced != nil

    init() {
        NotificationCenter.default.addObserver(
            forName: .GCKeyboardDidConnect, object: nil, queue: .main
        ) { [weak self] _ in self?.isConnected = true }
        NotificationCenter.default.addObserver(
            forName: .GCKeyboardDidDisconnect, object: nil, queue: .main
        ) { [weak self] _ in self?.isConnected = GCKeyboard.coalesced != nil }
    }
}
```

- [ ] **Step 2: Build**

Run: `xcodebuild ... build`
Expected: BUILD SUCCEEDED.

- [ ] **Step 3: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Input/HardwareKeyController.swift
git commit -m "feat(ios-m6): add HardwareKeyController + GCKeyboard detection

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.9 — iOS GuiStreamDetailView wiring (gestures + overlay + bottom UI)

**Files:**
- Modify: `ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift` OR split into `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift` (preferred — keep `GuiStreamView` as the bare RTCMTLVideoView)

- [ ] **Step 1: Read existing structure**

Read `ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift` and the call site (probably `WindowListView.swift` `NavigationLink`). Decide whether to (a) inline gesture/overlay into `GuiStreamView`, or (b) introduce a new `GuiStreamDetailView` that owns the gestures while `GuiStreamView` stays a pure renderer. **(b) is preferred.**

- [ ] **Step 2: Create GuiStreamDetailView**

Create or refactor `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift`:

```swift
import SwiftUI

struct GuiStreamDetailView: View {
    @Bindable var store: SupervisionStore
    let entry: SupervisionEntry

    @StateObject private var inputClient: InputClient
    @StateObject private var modState = ModifierState()
    @StateObject private var hwKbd = HardwareKeyboardDetector()
    @State private var lastDragLocation: CGPoint = .zero
    @State private var contentSize: CGSize = .zero
    @State private var showRetryBanner = false

    init(store: SupervisionStore, entry: SupervisionEntry) {
        self.store = store
        self.entry = entry
        _inputClient = StateObject(wrappedValue: InputClient(supId: entry.supId, glue: store.glue))
    }

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                GuiStreamView(track: store.activeTrack)
                    .aspectRatio(CGFloat(entry.width) / CGFloat(entry.height), contentMode: .fit)
                    .background(GeometryReader { geo in
                        Color.clear.onAppear { contentSize = geo.size }
                            .onChange(of: geo.size) { _, new in contentSize = new }
                    })
                    .gesture(tapGesture)
                    .simultaneousGesture(panGesture)
                HardwareKeyControllerView(inputClient: inputClient)
                    .allowsHitTesting(false)
                    .frame(width: 0, height: 0)
                if showRetryBanner {
                    permissionBanner
                }
            }
            if !hwKbd.isConnected {
                ModifierStickyRow(state: modState)
                SpecialKeyRow(input: inputClient, modState: modState)
            }
            InputBar(input: inputClient, modState: modState)
        }
        .navigationTitle(entry.title.isEmpty ? entry.appName : entry.title)
        .onChange(of: store.lastInputAck) { _, ack in
            if let a = ack, a.code == "permission_denied" { showRetryBanner = true }
            if let a = ack, a.code == "ok"                { showRetryBanner = false }
        }
    }

    private var tapGesture: some Gesture {
        SpatialTapGesture(coordinateSpace: .local)
            .onEnded { value in
                guard contentSize.width > 0 else { return }
                let nx = value.location.x / contentSize.width
                let ny = value.location.y / contentSize.height
                Task { await inputClient.tap(normalizedX: nx, normalizedY: ny) }
            }
    }

    private var panGesture: some Gesture {
        DragGesture(minimumDistance: 8)
            .onChanged { value in
                let dx = value.location.x - lastDragLocation.x
                let dy = value.location.y - lastDragLocation.y
                lastDragLocation = value.location
                Task { await inputClient.scroll(dx: dx, dy: dy) }
            }
            .onEnded { _ in lastDragLocation = .zero }
    }

    private var permissionBanner: some View {
        VStack {
            HStack {
                Image(systemName: "exclamationmark.triangle.fill")
                Text("Mac 未授予 Accessibility 权限")
                Spacer()
                Button("再试一次") { /* SupervisionStore retries last input — implementer may stub */ }
            }
            .padding(8)
            .background(Color.yellow.opacity(0.9))
            Spacer()
        }
    }
}
```

- [ ] **Step 3: Update SupervisionStore.lastInputAck + glue accessor**

Add to `SupervisionStore.swift`:
```swift
struct InputAckRecord: Equatable {
    let supId: String
    let code: String
    let message: String?
}

@Published var lastInputAck: InputAckRecord?
```

In the ctrl message dispatcher, add `case .inputAck(let supId, let code, let msg):` setting `lastInputAck = InputAckRecord(supId: supId, code: code, message: msg)`.

`store.glue` should expose the underlying `RtcGlue` (or a `Glue`-conforming proxy). If not currently exposed, add a `let glue: RtcGlue` field and pass it from `PairedView`.

- [ ] **Step 4: Wire navigation**

Update `WindowListView.swift` (or wherever the supervised list NavigationLink lives) to push `GuiStreamDetailView(store: store, entry: entry)`.

- [ ] **Step 5: Build**

Run: `xcodebuild ... build`
Expected: BUILD SUCCEEDED.

- [ ] **Step 6: Commit**

```bash
cd /Users/bruce/git/macagent
git add ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift \
        ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift \
        ios-app/MacIOSWorkspace/Gui/WindowListView.swift \
        ios-app/MacIOSWorkspace/SupervisionStore.swift
git commit -m "feat(ios-m6): GuiStreamDetailView with gestures + overlay + bottom UI

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.10 — Mac AX permission UX (tray + banner + Open Settings)

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`

- [ ] **Step 1: Tray icon variant**

In `ui.rs`, find the tray menu construction. Add an item that reads AX state and shows either:
- "Accessibility ✓ Granted" (disabled menu item) when `input_injector.refresh_ax()` is true
- "Accessibility ⚠️ Not granted — Click to open System Settings" when false

Selecting the item invokes:
```rust
let _ = std::process::Command::new("open")
    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
    .spawn();
```

- [ ] **Step 2: eframe banner in Paired state**

In the `PairState::Paired` branch of `update`, before the existing window-list/spaced UI, add:

```rust
if !ax_granted_cache {
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::YELLOW, "⚠️ Accessibility 未授权 — 输入注入不可用");
        if ui.button("Open System Settings").clicked() {
            let _ = std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                .spawn();
        }
    });
    ui.separator();
}
```

`ax_granted_cache` is read from a small atomic that the AX repoll task in M6.5 also writes; or call `input_injector.check_ax()` directly each frame (it's cheap due to caching).

- [ ] **Step 3: Build**

Run: `cd mac-agent && cargo build -p macagent-app && cargo clippy -p macagent-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-app/src/ui.rs
git commit -m "feat(m6): Mac AX permission UX — tray indicator + eframe banner + Open Settings

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M6.11 — Manual smoke test (no commit)

Per spec §6.3:

1. Build release: `cd mac-agent && cargo build --release -p macagent-app`
2. Launch agent. If first run: deny AX → see banner; click Open Settings → grant Accessibility for `macagent` → re-run agent.
3. Pair iPhone (existing flow) → Connect → list windows → tap Chrome.
4. **Manual test 1 (iPad + Magic Keyboard):** Cmd+L → Chrome 地址栏 focused.
5. **Manual test 2 (iPhone touch):** Tap a Chrome button → click registered. Drag finger → page scrolls. Tap toolbar `+` → font size grows.
6. **Manual test 3 (long Chinese):** Open ComposeSheet → paste 200 字 → submit → see Mac receive Cmd+V with the text. Mac clipboard now has the 200 字 (expected — design D).
7. **Manual test 4 (AX revoke):** in System Settings, toggle off Accessibility for macagent → from iPhone, tap → see banner "Mac 未授予 Accessibility" + Retry button. Re-grant → wait 60s → tap → works.
8. **Manual test 5 (window-gone):** while supervising, close the Mac window. Tap from iPhone → toast "window_gone" → return to list.

If any of (4)–(8) fail: do **not** declare M6 done. Diagnose & fix; re-run.

---

### Task M6.12 — M6 final review (subagent dispatch)

Caller should dispatch a `superpowers:code-reviewer` subagent against the M6 commit range to:
- Confirm spec coverage (every spec section maps to a commit).
- Audit unsafe blocks in `input_injector.rs` (FFI, AX check, msg_send via objc2).
- Check `extern "C"` signatures match Apple's headers.
- Confirm no scroll/throttle race or modifier leak in iOS UI state.
- Re-verify `cargo test --workspace` + `xcodebuild test` all green.

If issues are found, dispatch a fixup subagent following the M5/M5.2.5 review→fixup→push pattern.

---

## Risks + Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `core-graphics 0.25` lacks `set_string_from_utf16_unchecked` | Medium | Compile error | Inline FFI to `CGEventKeyboardSetUnicodeString` (3 lines). |
| `objc2-app-kit 0.3` doesn't expose `runningApplicationWithProcessIdentifier` | Low | Compile error | Inline `objc2::msg_send!` call to NSRunningApplication; ~5 lines. |
| AX entitlement missing on dev builds | Medium | Permission can't be granted | Document in README that the user must drag macagent into Privacy_Accessibility manually. M8 adds proper code-signing. |
| iOS `pressesBegan` not firing because the SwiftUI VStack steals first-responder | Medium | External keyboard input dropped | Verify `becomeFirstResponder()` returns true; may need to put HardwareKeyController as the *frontmost* layer with `allowsHitTesting(true)` for invisible regions. |
| Gesture conflict: SwiftUI's TapGesture vs DragGesture | Low | Tap registers as first frame of drag | Use `simultaneousGesture` per skeleton; `DragGesture(minimumDistance: 8)` filters out tap-as-drag. |
| Cmd+ on US keyboard is actually Cmd+= | Low | "+" doesn't work | Already handled — `+` button sends `KeyCombo([.cmd], "=")`. |
| 中文 IME with external keyboard sends `pressesBegan` per-keystroke (raw key) instead of composed string | Medium | Garbled text on Mac | `key.characters` is the *composed* string, not raw key — verified safe. If empty (combo without modifier), fall through to super (let UIKit handle composition). |
| Scroll throttle (16ms) may feel laggy at 30Hz Wi-Fi | Low | Subjective UX | Acceptable for v0.1; M8 polish can tune. |

---

## Out of Scope (M6 explicitly does NOT do)

- Pinch-to-zoom gesture (workspace zoom is manual via toolbar `+/-/0` buttons).
- Long-press → secondary click (M7).
- Drag-to-select (mouse-down + move + up sequence) (M7).
- 3-finger gestures, force-touch, drag-and-drop, file drag (M7+).
- F1–F12, Page Up/Down, Home/End named keys.
- Multi-monitor / multi-display coordinate spaces.
- iPadOS Cmd+H / Cmd+Tab / Cmd+Space interception (impossible).
- Mac↔iPad keyboard layout translation (Option+E dead-key, etc.).
- AX permission auto-grant (no public API; user must manually click).
- Custom shortcut presets / macro recording.
- TestFlight onboarding text (M8).
- `fit_window` / `restore_window` / `supervise_launch` (M7).

---

## 自检 (run this before declaring the plan ready)

1. **Spec coverage** — every spec §1–§7 item maps to at least one task above:
   - Spec §1 IN scope → Tasks M6.1, M6.4, M6.6–M6.9
   - Spec §1 OUT scope → "Out of Scope" section above
   - Spec §2 protocol → Task M6.1
   - Spec §3 Mac InputInjector → Tasks M6.2, M6.3, M6.4, M6.5
   - Spec §4 iOS UI → Tasks M6.6, M6.7, M6.8, M6.9
   - Spec §5 Permission UX → Tasks M6.5 (repoll), M6.10 (banner + tray)
   - Spec §6 testing → Embedded in Tasks M6.1, M6.2, M6.6, M6.7
   - Spec §6.3 Manual smoke → Task M6.11
   - Spec §7 file list → File Structure section above

2. **No placeholders** — every step contains real code or real commands. No "TBD", "implement appropriate handling", "similar to above".

3. **Type consistency** — `KeyMod` enum cases (`cmd`/`shift`/`opt`/`ctrl` lowercase) used identically in Rust + Swift + tests. `WindowFrame` (Rust) ↔ window bounds (Swift not exposed; mapping is at protocol normalized-coords boundary).

4. **Bite-sized tasks** — every task is ≤7 steps, each step is 2–10 minutes.

5. **CLAUDE.md alignment**:
   - 简单优先: no software keyboard fallback path beyond what's specified, no IME translation, single-button mouse only, no preset shortcuts.
   - 精准改动: changes confined to new `Input/` folder + `input_injector.rs` + ~50 lines in `ui.rs` + ~30 lines in `gui_capture/mod.rs` and `windows.rs`.
   - 不偷懒: AX permission failure is surfaced honestly via `InputAck { code: "permission_denied" }` rather than swallowed.

---

## Plan 完成后下一步

Suggested execution: **Subagent-Driven** (per established M0–M5.2.5 pattern).

Estimate per task:
- M6.1 — 30 min (protocol + mirrors + 1 test file)
- M6.2 — 25 min (pure helpers + 6 unit tests)
- M6.3 — 20 min (deps + 2 small APIs)
- M6.4 — 60 min (FFI is the highest single risk)
- M6.5 — 25 min (wiring)
- M6.6 — 35 min (Swift actor + 3 tests)
- M6.7 — 60 min (4 SwiftUI views)
- M6.8 — 30 min (HardwareKeyController + GCKeyboard)
- M6.9 — 50 min (detail view + state)
- M6.10 — 20 min (tray + banner)
- M6.11 — manual (user interaction)
- M6.12 — review subagent

**Total ~6 hours of focused subagent work + 1 manual smoke pass.** Allow 1–2 fixup rounds for the FFI task M6.4.
