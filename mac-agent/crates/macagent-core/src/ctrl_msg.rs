//! ctrl 通道消息类型 + 端到端签名/校验。

use crate::pair_auth::{hmac_sign, hmac_verify};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Shared terminal types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalLine {
    pub index: u16,
    pub runs: Vec<TerminalRun>,
    pub wrapped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalRun {
    pub text: String,
    pub fg: Option<TerminalColor>,
    pub bg: Option<TerminalColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalColor {
    Indexed { value: u8 },
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalInput {
    Text { data: String },
    Key { key: InputKey },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputKey {
    Enter,
    Tab,
    ShiftTab,
    Backspace,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    CtrlA,
    CtrlC,
    CtrlD,
    CtrlE,
    CtrlK,
    CtrlL,
    CtrlR,
    CtrlU,
    CtrlW,
    CtrlZ,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfo {
    pub sid: String,
    pub label: String,
    pub argv: Vec<String>,
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    pub started_ts: u64,
    pub streaming: bool,
    pub source: SessionSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionSource {
    IosLaunched { launcher_id: String },
    UserManual,
}

// ---------------------------------------------------------------------------
// CtrlPayload
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CtrlPayload {
    // M1/M2 existing
    Ping {
        ts: u64,
        nonce: String,
    },
    Pong {
        ts: u64,
        nonce: String,
    },
    Heartbeat {
        ts: u64,
        nonce: String,
    },
    HeartbeatAck {
        ts: u64,
        nonce: String,
    },
    Error {
        code: String,
        msg: String,
    },

    // M3 v2: session management
    LaunchSession {
        req_id: String,
        launcher_id: String,
        cwd_override: Option<String>,
    },
    LaunchAck {
        req_id: String,
        sid: String,
    },
    LaunchReject {
        req_id: String,
        code: String,
        reason: String,
    },
    AttachSession {
        sid: String,
    },
    DetachSession {
        sid: String,
    },
    KillSession {
        sid: String,
    },
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    SessionAdded {
        session: SessionInfo,
    },
    SessionRemoved {
        sid: String,
        reason: String,
    },
    SessionExited {
        sid: String,
        exit_status: Option<i32>,
        reason: String,
    },

    // M3 v2: terminal data
    TermSnapshot {
        sid: String,
        revision: u64,
        cols: u16,
        rows: u16,
        cursor_row: u16,
        cursor_col: u16,
        cursor_visible: bool,
        title: Option<String>,
        lines: Vec<TerminalLine>,
    },
    TermDelta {
        sid: String,
        revision: u64,
        cols: u16,
        rows: u16,
        cursor_row: u16,
        cursor_col: u16,
        cursor_visible: bool,
        title: Option<String>,
        lines: Vec<TerminalLine>,
    },
    TermHistorySnapshot {
        sid: String,
        revision: u64,
        lines: Vec<String>,
    },
    TermHistoryAppend {
        sid: String,
        revision: u64,
        lines: Vec<String>,
    },

    // M3 v2: input
    Input {
        sid: String,
        payload: TerminalInput,
    },
    Resize {
        sid: String,
        cols: u16,
        rows: u16,
    },

    // M4: clipboard
    ClipboardSet {
        source: ClipSource,
        content: ClipContent,
    },

    // M4.6: notify watchers
    WatchSession {
        sid: String,
        watcher_id: String,
        regex: String,
        name: String,
    },
    UnwatchSession {
        sid: String,
        watcher_id: String,
    },
    WatchersList {
        sid: String,
        watchers: Vec<WatcherInfo>,
    },
    WatcherMatched {
        sid: String,
        watcher_id: String,
        line_text: String,
    },

    // M5: GUI supervision — iOS → Mac
    ListWindows,
    SuperviseExisting {
        window_id: u32,
        viewport: Viewport,
    },
    RemoveSupervised {
        sup_id: String,
    },
    ViewportChanged {
        sup_id: String,
        viewport: Viewport,
    },

    // M5: GUI supervision — Mac → iOS
    WindowsList {
        windows: Vec<WindowInfo>,
    },
    SupervisedAck {
        sup_id: String,
        entry: SupervisionEntry,
    },
    SuperviseReject {
        window_id: u32,
        code: String,
        reason: String,
    },
    SupervisionList {
        entries: Vec<SupervisionEntry>,
    },
    StreamEnded {
        sup_id: String,
        reason: String,
    },

    // M6: GUI input injection
    GuiInputCmd {
        sup_id: String,
        payload: GuiInput,
    },
    GuiInputAck {
        sup_id: String,
        code: String,
        message: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// GUI supervision types (M5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowInfo {
    pub window_id: u32,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub on_screen: bool,
    pub is_minimized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisionEntry {
    pub sup_id: String,
    pub window_id: u32,
    pub app_name: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub status: SupervisionStatus,
    pub source: SupervisionSource,
    pub started_ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SupervisionStatus {
    Active,
    Dead,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SupervisionSource {
    Existing,
    Launched,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

// ---------------------------------------------------------------------------
// GUI input types (M6)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Watcher types (M4.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatcherInfo {
    pub id: String,
    pub regex: String,
    pub name: String,
    pub hits: u32,
    pub last_match: Option<String>,
}

// ---------------------------------------------------------------------------
// Clipboard types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipSource {
    Mac,
    Ios,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipContent {
    Text { data: String },
}

// ---------------------------------------------------------------------------
// SignedCtrl + canonical_bytes + sign + verify
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCtrl {
    #[serde(flatten)]
    pub payload: CtrlPayload,
    pub sig: String, // base64
}

pub fn canonical_bytes(payload: &CtrlPayload) -> Vec<u8> {
    // Recursively sort all Object keys so nested fields (e.g. TerminalInput) are
    // stable across Rust/Swift serializers. Swift JSONSerialization uses .sortedKeys
    // which is recursive; we must match that behaviour here.
    let v = serde_json::to_value(payload).unwrap();
    serde_json::to_vec(&sort_value(&v)).unwrap()
}

fn sort_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(k, vv)| (k.clone(), sort_value(vv)))
                .collect();
            serde_json::to_value(&sorted).unwrap()
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_value).collect())
        }
        _ => v.clone(),
    }
}

pub fn sign(payload: CtrlPayload, shared_secret: &[u8]) -> SignedCtrl {
    let bytes = canonical_bytes(&payload);
    let sig = B64.encode(hmac_sign(shared_secret, &bytes));
    SignedCtrl { payload, sig }
}

pub fn verify(signed: &SignedCtrl, shared_secret: &[u8]) -> Result<()> {
    let sig = B64.decode(&signed.sig)?;
    let bytes = canonical_bytes(&signed.payload);
    if hmac_verify(shared_secret, &bytes, &sig) {
        Ok(())
    } else {
        Err(anyhow!("ctrl signature invalid"))
    }
}
