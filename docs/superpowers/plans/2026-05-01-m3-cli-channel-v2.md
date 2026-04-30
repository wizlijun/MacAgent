# M3 v2 · CLI 通道（producer 模型）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）。
>
> **本文档取代** `archived-2026-04-30-m3-cli-channel-v1.md`。v1 的 SessionManager（spawn-in-Agent）模型废弃；v2 改用受 hurryvc 启发的 producer 子进程模型。

**Goal:** iOS 上点 launcher（zsh / claude code / ...）→ Mac Agent 调 AppleScript 起 Terminal.app 新窗口跑 `macagent run -- <cmd>` → 该 producer 子进程在用户当前 tty 跑 PTY（**Mac 用户可见**）+ alacritty 解析 PTY 输出 → 经 Unix socket 把 TermSnapshot/Delta 推给 menu bar Agent → 经 ctrl DataChannel 推给 iOS → iOS 用 SwiftUI Text + AttributedString **直接渲染**结构化 lines/runs（无 SwiftTerm 依赖）。

**Architecture:**
- **Producer 子进程**：`macagent run -- <cmd>` 同 binary 的 clap 子命令；fork PTY，本地 tty 直显（用户可见 + 可输入），同时 alacritty `Term` 在 producer 进程内解析输出，每 50ms 算 diff 推 TermDelta，每 5s 推一次全量 TermSnapshot 当 keyframe。
- **Mac 用户可见性**：`macagent run` 在用户当前 Terminal 窗口里跑，Mac 用户看得见 + 可参与输入。两端输入字节交错入 PTY（chaos OK，hurryvc 风格）。
- **iOS 端**：砍 SwiftTerm 依赖；用 SwiftUI 原生 `Text` + `AttributedString` 拼 runs；离屏 history 单独 ScrollView。
- **协议**：复用 M2 的 SignedCtrl HMAC E2E 包络；CtrlPayload 扩约 15 个新变体（LaunchSession / TermSnapshot / TermDelta / TermHistorySnapshot / TermHistoryAppend / Input / Resize / SessionList / SessionAdded / SessionRemoved / SessionExited / KillSession / AttachSession / DetachSession / LaunchAck / LaunchReject / Error code 扩展）。
- **Worker 不动**。

**Tech Stack（M3 新增）:**
- Mac: `alacritty_terminal = "0.25"`、`portable-pty = "0.9"`、`tokio::net::UnixListener/UnixStream`（Unix socket）、`json5`（读 launchers config）。
- iOS: 0 新增依赖（不引入 SwiftTerm，纯 SwiftUI）。
- Worker: 不动。

**M2 debt 一并清理**（在合适的 task 中夹带）：
- I-glare：M2 final review 提到的双端都发 offer 问题——M3 不直接解，但需在 SessionListView 显示一个明确的 connect 流程让用户感知 Mac=offerer / iOS=answerer 角色。最终修在 M4。

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 ProducerRegistry/AgentSocket/Launcher/SessionRouter（v2 取代旧 SessionManager）、§3.2 TermView/SessionListView、§4.3 v2 时序、§7 M3 行（已更新）。

---

## 协议契约（写代码前的契约定义）

### CtrlPayload 新增变体（Rust + Swift 同步）

**会话管理**
```rust
LaunchSession   { req_id: String, launcher_id: String, cwd_override: Option<String> }
LaunchAck       { req_id: String, sid: String }
LaunchReject    { req_id: String, code: String, reason: String }
AttachSession   { sid: String }
DetachSession   { sid: String }
KillSession     { sid: String }
SessionList     { sessions: Vec<SessionInfo> }
SessionAdded    { session: SessionInfo }
SessionRemoved  { sid: String, reason: String }
SessionExited   { sid: String, exit_status: Option<i32>, reason: String }
```

**终端数据**（所有都带 `sid`）
```rust
TermSnapshot       { sid, revision: u64, cols: u16, rows: u16,
                     cursor_row: u16, cursor_col: u16, cursor_visible: bool,
                     title: Option<String>, lines: Vec<TerminalLine> }
TermDelta          { sid, revision, cols, rows, cursor_*, lines: Vec<TerminalLine>(仅变化) }
TermHistorySnapshot { sid, revision: u64, lines: Vec<String> }
TermHistoryAppend   { sid, revision: u64, lines: Vec<String> }
TerminalLine       { index: u16, runs: Vec<TerminalRun>, wrapped: bool }
TerminalRun        { text, fg?, bg?, bold, dim, italic, underline, inverse }
TerminalColor      { Indexed { value: u8 } | Rgb { r, g, b } }
```

**输入**
```rust
Input  { sid, payload: TerminalInput }
Resize { sid, cols: u16, rows: u16 }

TerminalInput { Text { data: String } | Key { key: InputKey } }

InputKey: Enter | Tab | ShiftTab | Backspace | Escape
        | ArrowUp | ArrowDown | ArrowLeft | ArrowRight
        | Home | End | PageUp | PageDown | Delete
        | CtrlA | CtrlC | CtrlD | CtrlE | CtrlK | CtrlL
        | CtrlR | CtrlU | CtrlW | CtrlZ
        | F1..=F12
```

### Unix socket 协议（producer ↔ Agent）

JSON-framed（4 字节 BE 长度前缀 + JSON body），无 HMAC（本机进程互信）：

```rust
// producer → Agent
ProducerHello { argv, pid, cwd, cols, rows, launcher_id?, source }
TermSnapshot { revision, cols, rows, cursor_*, lines }       // 同 ctrl 但无 sid
TermDelta { revision, cols, rows, cursor_*, lines }
TermHistorySnapshot { revision, lines }
TermHistoryAppend { revision, lines }
ProducerExit { exit_status?, reason }

// Agent → producer
ProducerWelcome { sid }
Input { payload: TerminalInput }
Resize { cols, rows }
KillRequest { reason }
AttachStart                  // iOS 已 attach；producer 进入 streaming 模式
AttachStop                   // iOS detach；producer 可以暂停 delta（节省 CPU）
```

### 配置：`~/Library/Application Support/macagent/launchers.json5`（首次启动自动生成）

```jsonc
{
  "launchers": [
    { "id": "zsh",         "label": "Zsh shell",     "argv": ["zsh", "-l"],         "cwd": null },
    { "id": "claude-code", "label": "Claude Code",   "argv": ["claude", "code"],    "cwd": null },
    { "id": "codex",       "label": "Codex",         "argv": ["codex"],             "cwd": null },
    { "id": "npm-test",    "label": "npm test",      "argv": ["npm", "test"],       "cwd": null },
    { "id": "git-status",  "label": "git status",    "argv": ["git", "status"],     "cwd": null }
  ]
}
```

---

## 文件结构（增量）

```
mac-agent/Cargo.toml                          ← workspace deps 加 alacritty_terminal, portable-pty, json5
mac-agent/crates/macagent-core/
├── Cargo.toml                                ← 加 alacritty_terminal
├── src/
│   ├── lib.rs                                ← pub mod terminal; pub mod socket_proto;
│   ├── ctrl_msg.rs                           ← 扩约 15 个 CtrlPayload 变体
│   ├── socket_proto.rs                       ← 新：Unix socket Hello/Welcome/Snapshot/... 类型
│   └── terminal/                             ← 新模块
│       ├── mod.rs
│       ├── snapshot.rs                       ← alacritty Term → TerminalSnapshot/Delta（仿 hurryvc）
│       ├── history.rs                        ← 离屏 scrollback（仿 hurryvc/terminal_history.rs）
│       └── runs.rs                           ← grid cell → Vec<TerminalRun>（着色合并）
└── tests/
    └── terminal_test.rs                      ← snapshot/delta diff、history append

mac-agent/crates/macagent-app/
├── Cargo.toml                                ← 加 alacritty_terminal, portable-pty, json5, clap (likely 已有)
├── src/
│   ├── main.rs                               ← clap 子命令分发
│   ├── ui/                                   ← M2 已有 ui.rs，可保持单文件或拆模块
│   │   └── (existing)
│   ├── keychain.rs                           ← M1（不动）
│   ├── pair_qr.rs                            ← M1（不动）
│   ├── rtc_glue.rs                           ← M2，扩 ctrl 多路复用
│   ├── agent_socket.rs                       ← 新：Unix socket server
│   ├── producer_registry.rs                  ← 新：sid 分配 + SessionInfo
│   ├── session_router.rs                     ← 新：ctrl ↔ socket 桥
│   ├── launcher.rs                           ← 新：read launchers.json5 + AppleScript
│   └── run/                                  ← 新：producer 子命令
│       ├── mod.rs
│       ├── pty.rs
│       ├── parser.rs
│       └── socket_client.rs

ios-app/MacIOSWorkspace/
├── (M0+M1+M2 已有)
├── CtrlMessage.swift                         ← 扩 case
├── SessionStore.swift                        ← 新（@Observable）
├── SessionListView.swift                     ← 新
└── Term/
    ├── TermView.swift                        ← 新（lines/runs → SwiftUI Text）
    ├── TermStyle.swift                       ← 新（TerminalRun → AttributedString attrs）
    ├── HistoryView.swift                     ← 新（纯文本 ScrollView）
    └── InputBar.swift                        ← 新（软键盘 + InputKey 控制条）

worker/                                       ← 不动
docs/                                         ← 已 archive 旧 plan
```

---

## Task M3.1：协议类型扩展（Rust + Swift 双边）

**Goal:** 在 macagent-core/ctrl_msg.rs 与 ios-app/CtrlMessage.swift 同时加 §协议契约 列出的所有新 CtrlPayload 变体；新建 socket_proto.rs。

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`
- Create: `mac-agent/crates/macagent-core/src/socket_proto.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`

### Rust ctrl_msg.rs 扩展

加约 15 个变体（注意保留 M1/M2 的 Heartbeat / HeartbeatAck / Ping / Pong / Error）。`canonical_bytes` 扩展处理新 type tag。

```rust
// 共享类型（放 ctrl_msg.rs 或新文件 terminal_types.rs）

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
    Enter, Tab, ShiftTab, Backspace, Escape,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Home, End, PageUp, PageDown, Delete,
    CtrlA, CtrlC, CtrlD, CtrlE, CtrlK, CtrlL,
    CtrlR, CtrlU, CtrlW, CtrlZ,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
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

// 扩展现有 CtrlPayload enum：
pub enum CtrlPayload {
    // M1/M2 保留
    Ping { ts: u64, nonce: String },
    Pong { ts: u64, nonce: String },
    Heartbeat { ts: u64, nonce: String },
    HeartbeatAck { ts: u64, nonce: String },
    Error { code: String, msg: String },

    // M3 v2 新增
    LaunchSession { req_id: String, launcher_id: String, cwd_override: Option<String> },
    LaunchAck { req_id: String, sid: String },
    LaunchReject { req_id: String, code: String, reason: String },
    AttachSession { sid: String },
    DetachSession { sid: String },
    KillSession { sid: String },
    SessionList { sessions: Vec<SessionInfo> },
    SessionAdded { session: SessionInfo },
    SessionRemoved { sid: String, reason: String },
    SessionExited { sid: String, exit_status: Option<i32>, reason: String },
    TermSnapshot { sid: String, revision: u64, cols: u16, rows: u16,
                   cursor_row: u16, cursor_col: u16, cursor_visible: bool,
                   title: Option<String>, lines: Vec<TerminalLine> },
    TermDelta { sid: String, revision: u64, cols: u16, rows: u16,
                cursor_row: u16, cursor_col: u16, cursor_visible: bool,
                title: Option<String>, lines: Vec<TerminalLine> },
    TermHistorySnapshot { sid: String, revision: u64, lines: Vec<String> },
    TermHistoryAppend { sid: String, revision: u64, lines: Vec<String> }
,
    Input { sid: String, payload: TerminalInput },
    Resize { sid: String, cols: u16, rows: u16 },
}
```

### Rust socket_proto.rs（新）

把 producer ↔ Agent 的 socket 消息类型定义出来：

```rust
//! Unix socket protocol between `macagent run` producer and menu bar Agent.
//!
//! 4-byte BE length prefix + JSON body. No signing (local-only trust).

use serde::{Deserialize, Serialize};
use crate::ctrl_msg::{TerminalLine, TerminalInput, SessionSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum P2A {
    ProducerHello { argv: Vec<String>, pid: u32, cwd: Option<String>,
                    cols: u16, rows: u16, source: SessionSource },
    TermSnapshot { revision: u64, cols: u16, rows: u16,
                   cursor_row: u16, cursor_col: u16, cursor_visible: bool,
                   title: Option<String>, lines: Vec<TerminalLine> },
    TermDelta { revision: u64, cols: u16, rows: u16,
                cursor_row: u16, cursor_col: u16, cursor_visible: bool,
                title: Option<String>, lines: Vec<TerminalLine> },
    TermHistorySnapshot { revision: u64, lines: Vec<String> },
    TermHistoryAppend { revision: u64, lines: Vec<String> },
    ProducerExit { exit_status: Option<i32>, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum A2P {
    ProducerWelcome { sid: String },
    Input { payload: TerminalInput },
    Resize { cols: u16, rows: u16 },
    KillRequest { reason: String },
    AttachStart,
    AttachStop,
}

/// 4-byte BE 长度前缀的帧编解码 helper。
pub mod codec {
    use bytes::{Buf, BufMut, BytesMut};
    use serde::{de::DeserializeOwned, Serialize};

    pub fn encode<T: Serialize>(value: &T) -> anyhow::Result<BytesMut> {
        let body = serde_json::to_vec(value)?;
        let mut buf = BytesMut::with_capacity(4 + body.len());
        buf.put_u32(body.len() as u32);
        buf.extend_from_slice(&body);
        Ok(buf)
    }

    pub fn try_decode<T: DeserializeOwned>(buf: &mut BytesMut) -> anyhow::Result<Option<T>> {
        if buf.len() < 4 { return Ok(None); }
        let len = (&buf[..4]).get_u32() as usize;
        if buf.len() < 4 + len { return Ok(None); }
        buf.advance(4);
        let body = buf.split_to(len);
        let value = serde_json::from_slice(&body)?;
        Ok(Some(value))
    }
}
```

### Swift CtrlMessage.swift 扩展

每个 CtrlPayload 新变体加一个 Swift `case`，更新 `encode(to:)` 与 `init(from:)`。同时定义共享类型：

```swift
struct TerminalLine: Codable, Equatable {
    let index: UInt16
    let runs: [TerminalRun]
    let wrapped: Bool
}

struct TerminalRun: Codable, Equatable {
    let text: String
    let fg: TerminalColor?
    let bg: TerminalColor?
    let bold: Bool
    let dim: Bool
    let italic: Bool
    let underline: Bool
    let inverse: Bool
}

enum TerminalColor: Codable, Equatable { /* indexed / rgb */ }

enum InputKey: String, Codable {
    case enter, tab, shiftTab = "shift_tab", backspace, escape
    case arrowUp = "arrow_up", arrowDown = "arrow_down"
    case arrowLeft = "arrow_left", arrowRight = "arrow_right"
    case home, end, pageUp = "page_up", pageDown = "page_down", delete
    case ctrlA = "ctrl_a", ctrlC = "ctrl_c", ctrlD = "ctrl_d"
    case ctrlE = "ctrl_e", ctrlK = "ctrl_k", ctrlL = "ctrl_l"
    case ctrlR = "ctrl_r", ctrlU = "ctrl_u", ctrlW = "ctrl_w", ctrlZ = "ctrl_z"
    case f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12
}

enum TerminalInput: Codable, Equatable {
    case text(String)
    case key(InputKey)
}

struct SessionInfo: Codable, Equatable, Identifiable {
    var id: String { sid }
    let sid: String
    let label: String
    let argv: [String]
    let pid: UInt32
    let cols: UInt16
    let rows: UInt16
    let startedTs: UInt64
    let streaming: Bool
    let source: SessionSource
}

enum SessionSource: Codable, Equatable {
    case iosLaunched(launcherId: String)
    case userManual
}

// CtrlPayload 加 case：
case launchSession(reqId: String, launcherId: String, cwdOverride: String?)
case launchAck(reqId: String, sid: String)
case launchReject(reqId: String, code: String, reason: String)
case attachSession(sid: String)
case detachSession(sid: String)
case killSession(sid: String)
case sessionList(sessions: [SessionInfo])
case sessionAdded(session: SessionInfo)
case sessionRemoved(sid: String, reason: String)
case sessionExited(sid: String, exitStatus: Int32?, reason: String)
case termSnapshot(sid: String, revision: UInt64, cols: UInt16, rows: UInt16,
                  cursorRow: UInt16, cursorCol: UInt16, cursorVisible: Bool,
                  title: String?, lines: [TerminalLine])
case termDelta(sid: String, revision: UInt64, cols: UInt16, rows: UInt16,
               cursorRow: UInt16, cursorCol: UInt16, cursorVisible: Bool,
               title: String?, lines: [TerminalLine])
case termHistorySnapshot(sid: String, revision: UInt64, lines: [String])
case termHistoryAppend(sid: String, revision: UInt64, lines: [String])
case input(sid: String, payload: TerminalInput)
case resize(sid: String, cols: UInt16, rows: UInt16)
```

### 步骤

1. 改 ctrl_msg.rs 加变体 + canonical_bytes 处理
2. 创建 socket_proto.rs + tests
3. 改 lib.rs 暴露
4. 改 CtrlMessage.swift 同步 case + encode/decode + canonical bytes
5. `cd mac-agent && cargo test -p macagent-core 2>&1 | tail -10`：原有测试 + 新增 socket_proto round-trip + ctrl_msg JSON canonical 测试 6+
6. `cargo clippy --workspace --all-targets -- -D warnings`
7. `cargo fmt --all`
8. iOS xcodebuild build 过
9. commit：

```bash
cd /Users/bruce/git/macagent
git add mac-agent/ ios-app/
git commit -m "feat(core): extend CtrlPayload with M3 v2 session/term types and add socket_proto"
```

---

## Task M3.2：macagent-core terminal 模块（alacritty 包装）

**Goal:** 把 hurryvc/src/terminal.rs + terminal_history.rs 的核心逻辑搬到 macagent-core/src/terminal/，做必要的 macagent 适配。

**Files:**
- Modify: `mac-agent/crates/macagent-core/Cargo.toml`（加 alacritty_terminal）
- Create: `mac-agent/crates/macagent-core/src/terminal/mod.rs`
- Create: `mac-agent/crates/macagent-core/src/terminal/snapshot.rs`
- Create: `mac-agent/crates/macagent-core/src/terminal/history.rs`
- Create: `mac-agent/crates/macagent-core/src/terminal/runs.rs`
- Create: `mac-agent/crates/macagent-core/tests/terminal_test.rs`

### 实现要点

**`terminal/snapshot.rs`**：从 alacritty `Term<T>` 拍下完整 snapshot；diff 两 snapshots 出 delta（仅变化的 lines）。**直接参考 hurryvc/src/terminal.rs**——逻辑等价，只是用 macagent-core 的 TerminalSnapshot/Delta 结构。

```rust
use alacritty_terminal::{Term, event::EventListener};
use crate::ctrl_msg::{TerminalLine, TerminalRun, TerminalColor};

pub struct TerminalSnapshot {
    pub revision: u64,
    pub cols: u16, pub rows: u16,
    pub cursor_row: u16, pub cursor_col: u16,
    pub cursor_visible: bool,
    pub title: Option<String>,
    pub lines: Vec<TerminalLine>,
}

pub struct TerminalDelta { /* 同上字段；lines 只含变化的 */ }

pub fn snapshot_from_term<T: EventListener>(term: &Term<T>, revision: u64) -> TerminalSnapshot;
pub fn diff_snapshots(prev: &TerminalSnapshot, next: &TerminalSnapshot) -> Option<TerminalDelta>;
```

**`terminal/runs.rs`**：grid cell → Vec<TerminalRun>，相邻同样式 cell 合并。**直接参考 hurryvc/src/terminal.rs 的 row_runs / cell_text / same_style / color_to_wire / named_to_wire**。

**`terminal/history.rs`**：scrollback 满后被推出 viewport 的行单独捕获成纯文本，alternate screen 切换处理。**直接参考 hurryvc/src/terminal_history.rs**。

### Tests

```rust
// tests/terminal_test.rs
#[test] fn snapshot_diff_returns_none_when_unchanged() { /* ... */ }
#[test] fn snapshot_diff_only_reports_changed_lines() { /* ... */ }
#[test] fn run_merge_combines_same_style_cells() { /* ... */ }
#[test] fn history_appends_when_scrollback_overflows() { /* ... */ }
#[test] fn history_handles_alt_screen_switch() { /* ... */ }
```

### 步骤

1. 改 Cargo.toml 加 alacritty_terminal
2. 写 4 个文件 + tests
3. `cargo test -p macagent-core` 全过
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo fmt --all`
6. commit：

```bash
git commit -m "feat(core): add terminal module (alacritty snapshot/delta + history) ported from hurryvc"
```

---

## Task M3.3：`macagent run` producer 子命令

**Goal:** 在 macagent-app 加 clap 子命令 `run`，实现 producer：fork PTY、本机 tty 直显、alacritty 解析、Unix socket 推 snapshot。

**Files:**
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`（加 portable-pty, clap）
- Modify: `mac-agent/crates/macagent-app/src/main.rs`（clap 子命令分发）
- Create: `mac-agent/crates/macagent-app/src/run/mod.rs`
- Create: `mac-agent/crates/macagent-app/src/run/pty.rs`
- Create: `mac-agent/crates/macagent-app/src/run/parser.rs`
- Create: `mac-agent/crates/macagent-app/src/run/socket_client.rs`

### 关键代码骨架

```rust
// main.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run menu bar UI (default if no subcommand)
    Ui,
    /// Run as producer in current terminal
    Run(RunArgs),
    /// List active sessions (debug)
    List,
}

#[derive(clap::Args)]
struct RunArgs {
    #[arg(long)]
    launcher_id: Option<String>,
    #[arg(last = true)]
    command: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Run(args)) => run::main(args),
        Some(Command::List) => list::main(),
        _ => ui::main(),
    }
}
```

```rust
// run/mod.rs
pub fn main(args: RunArgs) -> anyhow::Result<()> {
    // 1. 连 ~/Library/Application Support/macagent/agent.sock
    // 2. 发 ProducerHello { argv: args.command, pid, cwd, cols, rows, ... }
    // 3. 等 ProducerWelcome { sid }
    // 4. fork PTY 跑 args.command
    // 5. 起 tokio task 同时：
    //    - PTY master.read → tee 到本地 stdout（用户可见）+ 喂 alacritty Term
    //    - alacritty 增量更新；每 50ms 算 diff → P2A::TermDelta socket 推
    //    - socket 收 A2P::Input → PTY master.write
    //    - 等 PTY 子进程退出 → P2A::ProducerExit → 退出
    Ok(())
}
```

### 步骤

1. 改 main.rs 加 clap Subcommand
2. 写 run/ 模块（参考 hurryvc/src/producer.rs 的 backend 结构，简化 macOS-only）
3. `cargo build -p macagent-app`
4. 简单测：在 Mac 终端 `macagent run -- echo hello`，看是否 echo 正常 + 错误 "agent socket 不可达" 是否清晰
5. `cargo clippy / fmt`
6. commit：

```bash
git commit -m "feat(mac-agent): add `macagent run` producer subcommand (PTY + alacritty + socket client)"
```

---

## Task M3.4：Agent 端 Unix socket server + ProducerRegistry + Launcher

**Files:**
- Create: `mac-agent/crates/macagent-app/src/agent_socket.rs`
- Create: `mac-agent/crates/macagent-app/src/producer_registry.rs`
- Create: `mac-agent/crates/macagent-app/src/launcher.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui/mod.rs`（启动 socket server）

### 步骤

1. 写 agent_socket.rs：UnixListener + accept loop + JSON frame
2. 写 producer_registry.rs：HashMap<sid, ProducerHandle{tx, info}>，sid 用 uuid v4 prefix
3. 写 launcher.rs：read launchers.json5（用 json5 crate），AppleScript 调 Terminal.app
4. ui/mod.rs 启动时 `tokio::spawn` socket server task
5. 单元测：
    - launcher 解析配置文件
    - registry register/unregister
6. `cargo build -p macagent-app` + `cargo clippy --workspace --all-targets -- -D warnings`
7. commit：

```bash
git commit -m "feat(mac-agent): add agent_socket, producer_registry, launcher (AppleScript Terminal.app)"
```

---

## Task M3.5：session_router 桥接

**Files:**
- Create: `mac-agent/crates/macagent-app/src/session_router.rs`
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs`（接 ctrl 消息分发到 router）

### 关键逻辑

```rust
//! ctrl ↔ Unix socket 双向桥
//! - ctrl: LaunchSession → launcher.start; 等 producer 上来后回 LaunchAck
//! - ctrl: AttachSession → router.attach(sid); socket 发 AttachStart
//! - ctrl: Input → socket 发 A2P::Input
//! - ctrl: Resize → socket 发 A2P::Resize
//! - socket: P2A::TermDelta → ctrl: TermDelta { sid, ... }
//! - socket: P2A::ProducerExit → ctrl: SessionExited; registry.unregister
//! - socket disconnect 在已 register 状态 → ctrl: SessionRemoved {reason="window_closed"}
```

### 步骤

1. 写 session_router.rs（含 mpsc 路由）
2. 改 rtc_glue.rs 在 ctrl on_message 处分发到 router
3. cargo build / clippy / fmt
4. commit：

```bash
git commit -m "feat(mac-agent): add session_router bridging ctrl DataChannel and Unix socket"
```

---

## Task M3.6：iOS TermView 渲染层

**Files:**
- Create: `ios-app/MacIOSWorkspace/Term/TermView.swift`
- Create: `ios-app/MacIOSWorkspace/Term/TermStyle.swift`
- Create: `ios-app/MacIOSWorkspace/Term/HistoryView.swift`
- Create: `ios-app/MacIOSWorkspace/Term/InputBar.swift`

### 关键代码

```swift
// TermStyle.swift
extension TerminalRun {
    func attributed() -> AttributedString {
        var s = AttributedString(text)
        if let fg { s.foregroundColor = fg.toSwiftUI() }
        if let bg { s.backgroundColor = bg.toSwiftUI() }
        if bold { s.font = .system(.body, design: .monospaced).bold() }
        // italic / underline / inverse 处理...
        return s
    }
}

// TermView.swift
struct TermView: View {
    let snapshot: TerminalSnapshot
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(snapshot.lines, id: \.index) { line in
                Text(line.runs.map { $0.attributed() }.reduce(AttributedString(""), +))
                    .font(.system(.body, design: .monospaced))
                    .lineLimit(1)
            }
        }
    }
}

// InputBar.swift
struct InputBar: View {
    @Binding var text: String
    let onSendText: (String) -> Void
    let onKey: (InputKey) -> Void

    var body: some View {
        VStack {
            HStack {
                TextField("输入", text: $text, onCommit: { onSendText(text); text = "" })
                Button("Send") { onSendText(text); text = "" }
            }
            ScrollView(.horizontal) {
                HStack {
                    ForEach([.tab, .escape, .ctrlC, .ctrlD, .arrowUp, .arrowDown], id: \.self) { k in
                        Button(label(for: k)) { onKey(k) }
                    }
                }
            }
        }
    }
}
```

### 步骤

1. 写 4 个文件
2. xcodebuild build 过（仍无 SwiftTerm 依赖）
3. commit：

```bash
git commit -m "feat(ios-app): add TermView + TermStyle + HistoryView + InputBar (no SwiftTerm)"
```

---

## Task M3.7：iOS SessionStore + SessionListView + PairedView 集成

**Files:**
- Create: `ios-app/MacIOSWorkspace/SessionStore.swift`
- Create: `ios-app/MacIOSWorkspace/SessionListView.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（加 NavigationLink 进 SessionListView）
- Modify: `ios-app/MacIOSWorkspace/RtcGlue.swift`（暴露 ctrl 消息 stream + 发 ctrl 方法）

### 步骤

1. 写 SessionStore.swift（@Observable，订阅 ctrl）
2. 写 SessionListView.swift（launchers 按钮 + active sessions 列表 + NavigationLink TermView）
3. 改 PairedView 加进入 SessionListView 入口
4. 改 RtcGlue 暴露 ctrlMessages AsyncStream + sendCtrl 方法（M2.5 已部分完成）
5. xcodebuild build 过
6. commit：

```bash
git commit -m "feat(ios-app): add SessionStore + SessionListView and wire into PairedView"
```

---

## Task M3.8：真机端到端 + AppleScript 授权 + 8 并发

**Files:** 无新增。

### 验收

1. 用户首次跑 menu bar Agent → iOS attach Connect → 点"Claude Code"
2. macOS 弹 Automation 权限授予 Terminal.app（用户授）
3. Terminal.app 弹新窗口跑 `macagent run --launcher-id claude-code -- claude code`
4. iOS 看到新 session 出现，进入 TermView 看到 claude code 启动
5. iOS 输入 `ls\r` → Mac Terminal 看到 + claude 响应；同时用户在 Mac 终端打字也能进 PTY
6. 同时开 8 个 launcher → 8 个 Terminal 窗口；iOS 列表 8 条；切换流畅
7. Cmd+W 关一个窗 → iOS SessionRemoved；session 从列表消失
8. iOS 杀 process / 切飞行模式 60s → 重连后 attach → SessionList 仍在 → 进入 TermView 看到最新 grid

### 步骤

无 commit；纯人工验证。结果记录到 final review。

---

## Task M3.9：M3 final review

dispatch reviewer subagent，按 M2 final review 模式审 commits `<M2 last>..HEAD`。

---

## M3 v2 验收清单

- [ ] `worker npm test` 不变（27/27）
- [ ] `mac-agent cargo test --workspace` 全绿（含 terminal_test 5+ + socket_proto round-trip + ctrl_msg canonical）
- [ ] `ios-app xcodebuild test` 全绿（PairKeysTests 4 + 默认 2 + 新增 InputKey/SessionInfo 编解码测试）
- [ ] CI 三条 workflow 全绿
- [ ] 真机：iOS 点 launcher → Mac Terminal 弹窗 + 跑命令 + iOS 看到 grid
- [ ] Mac 用户在 Terminal 打字 + iOS 同时打字（chaos 模式可见）
- [ ] 8 并发 launcher 不卡
- [ ] Cmd+W 关窗 = SessionRemoved + iOS 列表更新
- [ ] iOS 离线 60s 重连 → attach 拿最新 snapshot

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：spec §3.1 ProducerRegistry/AgentSocket/Launcher/SessionRouter / §3.2 TermView/SessionListView / §4.3 v2 时序 / §7 M3 acceptance 全部映射到 M3.1-M3.8。
2. **占位符扫描**：M3.4 launcher AppleScript 实际命令需 implementer 写出（plan 给了思路但完整 osascript 命令由 implementer 决定）；M3.6 InputBar 控制键完整列表由 implementer 决定。其余无 TBD/TODO。
3. **类型一致性**：CtrlPayload Mac/iOS 同步；TerminalLine/Run/Color/InputKey 三方（Rust ctrl_msg + Rust socket_proto + Swift CtrlMessage）一致；canonical bytes Mac/iOS 同样产出。
4. **范围**：M3 v2 仅 CLI；**不**含剪贴板（M4）、视频（M5）、输入注入到 GUI App（M6）。
5. **风险**：
   - alacritty_terminal 0.25 与 hurryvc 用的版本一致；API 应稳定
   - portable-pty 跟 hurryvc 同；macOS-only 实现
   - AppleScript Automation 权限弹窗在测试时会打断 CI；M3.8 仅手动测试
   - M2 已经的 RtcGlue ctrl 消息处理可能需要扩 AsyncStream 暴露——影响小

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven**（推荐）——延续 M0/M1/M2 节奏。M3.1-M3.5 Mac+协议大头，M3.6-M3.7 iOS 重点，M3.8 真机
2. **Inline Execution**

请用户选 1 或 2。
