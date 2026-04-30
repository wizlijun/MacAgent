# M3 · CLI 通道 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans。

**Goal:** 在 M2 已建立的 WebRTC PeerConnection + ctrl DataChannel 之上，新增"按需创建 PTY 会话"的能力：iOS 通过 `ctrl: open_session` 请求；Mac 端 SessionManager 启动 PTY，把输出推到名为 `pty/<sid>` 的新 DataChannel；iOS 端用 SwiftTerm 渲染并把按键回送。断网重连后由 `resume_session` 从 ring buffer 补齐输出；超过缓冲区从落盘日志回灌。8 个并发 session 稳定。

**Architecture:**
- **Mac 侧**：`macagent-core` 加 `SessionManager` 模块（PTY + 1MB/10k 行环形缓冲 + append-only 落盘日志 + broadcast subscribe 模型）；`macagent-app` 的 `rtc_glue` 在收到 `open_session` ctrl 消息后调 SessionManager.spawn → 在 `RtcPeer` 上 `open_data_channel("pty/<sid>")` → 把 PTY 输出 pump 到 channel；`pty/<sid>` 上收到 iOS 字节直接 `pty.write`。
- **iOS 侧**：通过 SwiftPM 引入 `migueldeicaza/SwiftTerm`；`RtcClient` 暴露"已收到对端创建的 DataChannel"事件流；`SessionStore`（@Observable）维护 session 列表；`CliView` 用 SwiftTerm 渲染 + `UIKeyCommand` 接硬件键盘 + 软键盘 `type_text`。
- **协议扩展**：复用 M2 `SignedCtrl` 包络（HMAC E2E）；`CtrlPayload` 加 `OpenSession`、`SessionOpened`、`ResumeSession`、`SessionExited`、`SessionList` 五个变体。`pty/<sid>` 通道用**二进制**帧：`[seq:u32 BE][payload bytes]`，前 4 字节是序列号大端，剩余是 PTY 字节流。
- **会话生命周期**：iOS 离线 → PTY 不退出，输出继续填 ring buffer + 落盘 log → iOS 重连后发 `resume_session(sid, last_seq)` → Mac 从 ring 补缺，必要时从 log 文件回读，仍超出则发 `backlog_truncated` 错误。Session 永久存活直到 PTY 子进程退出或显式 `kill_session`。

**Tech Stack（M3 新增）:**
- Mac: `portable-pty = "0.8"`、`tokio` broadcast / mpsc、`bytes`（重新拉回）、`tokio::fs`。
- iOS: `https://github.com/migueldeicaza/SwiftTerm`（SwiftPM，Pin 到 `1.2.0+`）；约 30MB framework。
- Worker: **不改动**（DO 已经透明中继任意帧，包括二进制）。

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 SessionManager、§3.2 SessionStore/CliView、§4.3 CLI session 时序、§5 PTY 错误、§7 M3。

---

## 协议契约

### ctrl 通道新增 CtrlPayload 变体（Mac/iOS 同步）

```rust
// macagent-core::ctrl_msg::CtrlPayload 加：
OpenSession {
    sid: String,                  // 由发起方（iOS）生成，UUID v4 简短形式
    cmd: Vec<String>,             // argv[0..] e.g. ["zsh", "-l"] 或 ["claude", "code"]
    rows: u16,                    // 终端行
    cols: u16,                    // 终端列
},
SessionOpened {
    sid: String,
    channel_label: String,        // "pty/<sid>"
    pid: u32,
},
ResumeSession {
    sid: String,
    last_seq: u32,                // iOS 已收到的最后 seq
    rows: u16,                    // 客户端窗口尺寸（可能变了）
    cols: u16,
},
SessionExited {
    sid: String,
    exit_code: i32,
},
SessionList {
    sessions: Vec<SessionInfoLite>,
},
SessionInfoLite { sid: String, cmd: Vec<String>, alive: bool },
ResizeSession {
    sid: String,
    rows: u16,
    cols: u16,
},
BacklogTruncated {
    sid: String,
    kept_from_seq: u32,
},
```

### `pty/<sid>` 二进制帧

每帧：

| 偏移 | 字节 | 含义 |
|---|---|---|
| 0..4 | u32 BE | seq |
| 4.. | bytes | PTY 输出字节（UTF-8 / 任意） |

iOS → Mac 方向：seq 字段是 0（无序号语义，Mac 不存）；payload 直接 write 到 PTY。

Mac → iOS 方向：seq 单调递增（每个 session 独立），iOS 用最后收到的 seq 在 reconnect 时回填 `ResumeSession.last_seq`。

### Session 存储

- Ring buffer：每 session `VecDeque<(u32 seq, Bytes data)>`，硬限 1 MB **总字节** + 10000 entry，超出 pop front。
- 落盘日志：`~/Library/Application Support/macagent/sessions/<sid>.log`，append-only，写入"`<seq>\t<base64-data>`\n"行格式。每天滚动 `<sid>-<YYYYMMDD>.log`，超过 7 天自动清理。
- 资源限制：`SessionManager` 持有 `HashMap<String, Arc<SessionState>>`，硬限 8 个 alive session（含已 exited 但还在保留 30 分钟内的视作不活跃，但仍占名额）。

---

## 文件结构（增量）

```
mac-agent/crates/macagent-core/src/
├── session_manager.rs            ← 新：SessionManager + SessionState + ring buffer + log
├── ctrl_msg.rs                   ← 改：加 5 个 CtrlPayload 变体
└── lib.rs                        ← 改：pub mod session_manager

mac-agent/crates/macagent-core/tests/
├── session_manager_test.rs       ← 新：spawn/read/write/replay 集成测试
└── (rtc_peer_test.rs 等不动)

mac-agent/crates/macagent-app/src/
├── rtc_glue.rs                   ← 改：collect ctrl messages，分发到 SessionManager；为每个新 session 开 pty/<sid> DataChannel
└── session_router.rs             ← 新：把 SessionManager.subscribe 流喂到 DataChannel.send_bytes，反向亦然

ios-app/MacIOSWorkspace/
├── CtrlMessage.swift             ← 改：加 5 个 case
├── SessionStore.swift            ← 新：@Observable，维护 session 列表
├── CliView.swift                 ← 新：SwiftTerm 渲染 + 输入回调
├── PairedView.swift              ← 改：加 "Sessions" 区，按钮跳转 CliView
└── MacIOSWorkspace.xcodeproj/project.pbxproj  ← 改：加 SwiftTerm SPM dep

worker/                            ← 不动
docs/                              ← 已有 plan
```

---

## Task M3.1：Mac SessionManager（PTY + ring buffer + log）

**Files:**
- Modify: `mac-agent/Cargo.toml` workspace deps 加 `portable-pty = "0.8"`、`bytes = "1.7"`
- Modify: `mac-agent/crates/macagent-core/Cargo.toml` 加上述依赖
- Create: `mac-agent/crates/macagent-core/src/session_manager.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`
- Create: `mac-agent/crates/macagent-core/tests/session_manager_test.rs`

### 接口（plan 字面）

```rust
//! macagent-core::session_manager
//!
//! 单 SessionManager 进程内实例，持有 0..N 个 PTY session。
//! 每个 session = (PTY child, ring buffer, log writer, broadcast tx)。

use anyhow::{Context, Result};
use bytes::Bytes;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};

pub const MAX_SESSIONS: usize = 8;
pub const RING_BYTES_CAP: usize = 1024 * 1024; // 1 MB
pub const RING_ENTRY_CAP: usize = 10_000;
pub const LOG_RETENTION_DAYS: u64 = 7;

pub type SessionId = String;

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub sid: SessionId,
    pub cmd: Vec<String>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub sid: SessionId,
    pub cmd: Vec<String>,
    pub alive: bool,
    pub pid: u32,
    pub last_seq: u32,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Output { seq: u32, data: Bytes },
    Exited { exit_code: i32 },
}

struct SessionInner {
    info: SessionInfo,
    ring: VecDeque<(u32, Bytes)>,
    ring_bytes: usize,
    tx: broadcast::Sender<SessionEvent>,
    writer: Box<dyn portable_pty::MasterPty + Send>,  // PTY master 写入
    log_path: PathBuf,
}

pub struct SessionManager {
    storage_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<SessionId, Arc<Mutex<SessionInner>>>>>,
}

impl SessionManager {
    pub fn new(storage_dir: PathBuf) -> Self;
    pub async fn spawn(&self, spec: SpawnSpec) -> Result<()>;
    pub async fn write(&self, sid: &SessionId, bytes: &[u8]) -> Result<()>;
    pub async fn subscribe(&self, sid: &SessionId) -> Result<broadcast::Receiver<SessionEvent>>;
    pub async fn replay(&self, sid: &SessionId, from_seq: u32) -> Result<ReplayStream>;
    pub async fn resize(&self, sid: &SessionId, rows: u16, cols: u16) -> Result<()>;
    pub async fn kill(&self, sid: &SessionId) -> Result<()>;
    pub async fn list(&self) -> Vec<SessionInfo>;
}

pub struct ReplayStream {
    pub from_disk: bool,                    // true 表示 ring 不够、从 log 回灌
    pub initial_seq: u32,                   // 实际从这个 seq 开始；可能 > from_seq（说明 truncated）
    pub events: Vec<SessionEvent>,
}
```

### 实现要点

- `spawn`：用 `native_pty_system().openpty(PtySize { rows, cols, ... })` 拿 master+slave；slave 喂给 `CommandBuilder::new(cmd[0]).args(&cmd[1..])`；spawn 子进程；spawn 一个 task 从 master.try_clone_reader 读字节，每次读到一段：
  1. 取下一个 seq（递增）
  2. push 到 ring，pop front 直到 ring_bytes ≤ 1MB 且 entries ≤ 10k
  3. 写一行到 log（`<seq>\t<base64>\n`）
  4. broadcast SessionEvent::Output
- 子进程退出 wait 拿 exit_code → broadcast SessionEvent::Exited，标 alive=false
- `replay(from_seq)`：先尝试 ring；ring 起始 seq > from_seq 时从 log 回灌（读文件、解析 base64、按 seq 过滤）；从 log 都凑不齐 from_seq 时设 from_disk=true + initial_seq=ring 起始 seq（调用方判断是否 truncated）
- `kill`：调 `child.kill()` + 标 alive=false
- `resize`：master_pty.resize(PtySize {...})

> 注：`portable-pty` 的 master_pty 类型是 `Box<dyn MasterPty + Send>`，其 reader 通过 `try_clone_reader()` 拿，writer 通过 `take_writer()` 或者自己用 master 类型方法。仔细查 portable-pty docs.rs。

### Tests（plan 字面）

```rust
// tests/session_manager_test.rs
use bytes::Bytes;
use macagent_core::session_manager::{SessionManager, SpawnSpec, SessionEvent};
use std::path::PathBuf;
use tokio::time::{timeout, Duration};

fn tmp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("macagent-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_echo_and_read_output() {
    let mgr = SessionManager::new(tmp_dir());
    mgr.spawn(SpawnSpec {
        sid: "s1".into(),
        cmd: vec!["bash".into(), "-c".into(), "echo hello && echo world".into()],
        rows: 24, cols: 80,
    }).await.unwrap();
    let mut rx = mgr.subscribe(&"s1".into()).await.unwrap();
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_secs(5), async {
        while let Ok(evt) = rx.recv().await {
            match evt {
                SessionEvent::Output { data, .. } => {
                    buf.extend_from_slice(&data);
                    if std::str::from_utf8(&buf).unwrap_or("").contains("world") { return; }
                }
                SessionEvent::Exited { .. } => return,
            }
        }
    }).await;
    let s = String::from_utf8_lossy(&buf);
    assert!(s.contains("hello"));
    assert!(s.contains("world"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_to_pty_and_read_back() {
    let mgr = SessionManager::new(tmp_dir());
    mgr.spawn(SpawnSpec {
        sid: "s2".into(),
        cmd: vec!["cat".into()],
        rows: 24, cols: 80,
    }).await.unwrap();
    let mut rx = mgr.subscribe(&"s2".into()).await.unwrap();
    mgr.write(&"s2".into(), b"ping\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_secs(3), async {
        while let Ok(SessionEvent::Output { data, .. }) = rx.recv().await {
            buf.extend_from_slice(&data);
            if std::str::from_utf8(&buf).unwrap_or("").contains("ping") { return; }
        }
    }).await;
    assert!(String::from_utf8_lossy(&buf).contains("ping"));
    mgr.kill(&"s2".into()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_from_seq_returns_remaining_buffer() {
    let mgr = SessionManager::new(tmp_dir());
    mgr.spawn(SpawnSpec {
        sid: "s3".into(),
        cmd: vec!["bash".into(), "-c".into(), "for i in 1 2 3; do echo line$i; done".into()],
        rows: 24, cols: 80,
    }).await.unwrap();

    // 等子进程退出
    let mut rx = mgr.subscribe(&"s3".into()).await.unwrap();
    let _ = timeout(Duration::from_secs(3), async {
        while let Ok(evt) = rx.recv().await {
            if matches!(evt, SessionEvent::Exited { .. }) { return; }
        }
    }).await;

    // 从 seq=0 replay
    let stream = mgr.replay(&"s3".into(), 0).await.unwrap();
    let combined: Vec<u8> = stream.events.iter().flat_map(|e| match e {
        SessionEvent::Output { data, .. } => data.to_vec(),
        _ => vec![],
    }).collect();
    let s = String::from_utf8_lossy(&combined);
    assert!(s.contains("line1") && s.contains("line2") && s.contains("line3"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_limit_enforced() {
    let mgr = SessionManager::new(tmp_dir());
    for i in 0..8 {
        mgr.spawn(SpawnSpec {
            sid: format!("s{i}"),
            cmd: vec!["sleep".into(), "10".into()],
            rows: 24, cols: 80,
        }).await.unwrap();
    }
    // 第 9 个应失败
    let res = mgr.spawn(SpawnSpec {
        sid: "s9".into(),
        cmd: vec!["sleep".into(), "10".into()],
        rows: 24, cols: 80,
    }).await;
    assert!(res.is_err());
    for i in 0..8 {
        mgr.kill(&format!("s{i}")).await.unwrap();
    }
}
```

### 步骤

1. 改 mac-agent/Cargo.toml 加 portable-pty + bytes
2. 改 macagent-core/Cargo.toml 加 dep
3. 改 lib.rs `pub mod session_manager`
4. 写 session_manager.rs（约 250-350 行）
5. 写 tests/session_manager_test.rs
6. `cd /Users/bruce/git/macagent/mac-agent && cargo test -p macagent-core 2>&1 | tail -10`：现 11 + 新 4 = 15 passed
7. `cargo clippy --workspace --all-targets -- -D warnings`
8. `cargo fmt --all`
9. commit：`feat(mac-agent): add SessionManager with PTY ring buffer and on-disk log`

---

## Task M3.2：CtrlPayload 扩展 + 二进制帧 helpers

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`
- Create: `mac-agent/crates/macagent-core/src/pty_frame.rs` （二进制 seq 帧 encode/decode）
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`

### `ctrl_msg.rs` 扩展

加 5+1 个 CtrlPayload 变体（OpenSession / SessionOpened / ResumeSession / SessionExited / SessionList / ResizeSession / BacklogTruncated），并扩 `canonical_bytes` 处理新 type tag。

### `pty_frame.rs`

```rust
//! pty/<sid> DataChannel 二进制帧。

use bytes::{Buf, BufMut, Bytes, BytesMut};

pub struct PtyFrame {
    pub seq: u32,
    pub data: Bytes,
}

pub fn encode(frame: PtyFrame) -> Bytes {
    let mut buf = BytesMut::with_capacity(4 + frame.data.len());
    buf.put_u32(frame.seq);
    buf.put(frame.data);
    buf.freeze()
}

pub fn decode(bytes: &[u8]) -> Option<PtyFrame> {
    if bytes.len() < 4 { return None; }
    let mut b = bytes;
    let seq = b.get_u32();
    Some(PtyFrame { seq, data: Bytes::copy_from_slice(b) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let f = encode(PtyFrame { seq: 42, data: Bytes::from_static(b"hello") });
        let d = decode(&f).unwrap();
        assert_eq!(d.seq, 42);
        assert_eq!(&d.data[..], b"hello");
    }
}
```

### iOS `CtrlMessage.swift` 同步加 case

加 5+1 个 case + canonicalBytes/encode/init 的 switch 分支扩展。

### iOS pty 帧 helper（同 Swift 文件 `PtyFrame.swift`）

```swift
import Foundation

struct PtyFrame {
    let seq: UInt32
    let data: Data
    static func encode(_ frame: PtyFrame) -> Data {
        var out = Data(count: 4 + frame.data.count)
        out.withUnsafeMutableBytes { buf in
            buf.storeBytes(of: frame.seq.bigEndian, as: UInt32.self)
        }
        out.replaceSubrange(4..., with: frame.data)
        return out
    }
    static func decode(_ bytes: Data) -> PtyFrame? {
        guard bytes.count >= 4 else { return nil }
        let seq = bytes.prefix(4).withUnsafeBytes { $0.load(as: UInt32.self).bigEndian }
        return PtyFrame(seq: seq, data: bytes.suffix(from: 4))
    }
}
```

### 步骤

1. 改 ctrl_msg.rs 扩 enum + canonical
2. 写 pty_frame.rs + 单测
3. 改 iOS CtrlMessage.swift 同步
4. 写 iOS PtyFrame.swift
5. `cargo test --workspace -p macagent-core` 全过；现 15 + 1 = 16
6. xcodebuild build 过
7. commit：`feat: extend ctrl_msg with session variants and add pty_frame helpers (Mac+iOS)`

---

## Task M3.3：Mac session_router — SessionManager ↔ pty DataChannel 桥接

**Files:**
- Create: `mac-agent/crates/macagent-app/src/session_router.rs`
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs`（在 ctrl on_message 中分发 OpenSession/Resume/Resize/Kill）
- Modify: `mac-agent/crates/macagent-app/src/main.rs` 或 `ui.rs`（持有 SessionManager 实例）

### 关键逻辑

```rust
//! 桥接：每个 PTY session 一条 pty/<sid> DataChannel
//!
//! - 收到 ctrl::OpenSession(sid, cmd, rows, cols)
//!   → SessionManager.spawn(...)
//!   → peer.create_data_channel("pty/<sid>") → CtrlChannel-like wrapper
//!   → spawn 一个 task：subscribe SessionManager events → encode pty_frame → dc.send_binary
//!   → 在 dc.on_binary_message：bytes → SessionManager.write(sid, bytes[4..])
//!     （ignore seq 因为是 client → server 方向，无序号需求）
//!   → 回 ctrl::SessionOpened(sid, "pty/<sid>", pid)
//!
//! - 收到 ctrl::ResumeSession(sid, last_seq, rows, cols)
//!   → SessionManager.replay(sid, last_seq) → 把 events 全部 send_binary
//!   → 然后切到 live subscribe
//!   → 如 from_disk + initial_seq > last_seq+1：先回 ctrl::BacklogTruncated(sid, kept_from_seq)
//!   → 同时 SessionManager.resize(sid, rows, cols)
//!
//! - 收到 ctrl::ResizeSession → SessionManager.resize
//! - 收到 ctrl::KillSession（如果有）→ SessionManager.kill；ctrl 上回 SessionExited
```

### 步骤

1. 写 session_router.rs（约 200 行）
2. 改 rtc_glue.rs 在 ctrl message 处理回调里 dispatch
3. cargo build / clippy / fmt 过
4. **不**写新 unit test（这个层是 integration glue，靠 M3.7 e2e 真机测）
5. commit：`feat(mac-agent): wire session_router between rtc_glue and SessionManager`

---

## Task M3.4：iOS SwiftTerm SPM + CliView

**Files:**
- Modify: `ios-app/MacIOSWorkspace.xcodeproj/project.pbxproj`（加 `migueldeicaza/SwiftTerm` SPM dep，pin `1.2.0+`）
- Create: `ios-app/MacIOSWorkspace/CliView.swift`

### CliView 设计

```swift
import SwiftUI
import SwiftTerm

struct CliView: View {
    let session: Session
    let store: SessionStore
    @State var presentClose = false

    var body: some View {
        TerminalRepresentable(session: session, store: store)
            .navigationTitle(session.cmd.first ?? "session")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(role: .destructive) { presentClose = true } label: {
                        Image(systemName: "xmark.circle")
                    }
                }
            }
            .alert("关闭会话?", isPresented: $presentClose) {
                Button("关闭", role: .destructive) {
                    Task { await store.killSession(session.sid) }
                }
                Button("取消", role: .cancel) {}
            }
    }
}

private struct TerminalRepresentable: UIViewRepresentable {
    let session: Session
    let store: SessionStore

    func makeUIView(context: Context) -> TerminalView {
        let term = TerminalView()
        term.terminalDelegate = context.coordinator
        // SwiftTerm 提供 send: ([UInt8]) -> Void 给 delegate；调用方实现
        Task {
            await store.attach(session: session, sink: { [weak term] bytes in
                term?.feed(byteArray: bytes)
            })
        }
        return term
    }

    func updateUIView(_ uiView: TerminalView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(session: session, store: store)
    }

    final class Coordinator: NSObject, TerminalViewDelegate {
        let session: Session
        let store: SessionStore
        init(session: Session, store: SessionStore) {
            self.session = session; self.store = store
        }
        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            Task { await store.sendInput(sid: session.sid, bytes: Data(data)) }
        }
        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            Task { await store.resize(sid: session.sid, cols: UInt16(newCols), rows: UInt16(newRows)) }
        }
        func setTerminalTitle(source: TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func scrolled(source: TerminalView, position: Double) {}
    }
}
```

### 步骤

1. 改 pbxproj 加 SwiftTerm SPM dep（同 M2.4 SPM 流程）
2. 写 CliView.swift
3. xcodebuild build 过
4. **不**改 PairedView（M3.5 加跳转）
5. commit：`feat(ios-app): integrate SwiftTerm SPM and add CliView`

---

## Task M3.5：iOS SessionStore + PairedView 集成

**Files:**
- Create: `ios-app/MacIOSWorkspace/SessionStore.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（加 "Sessions" 区 + NavigationLink 到 CliView）
- Modify: `ios-app/MacIOSWorkspace/RtcGlue.swift`（暴露 attach/sendInput/resize/openSession 给 SessionStore）
- Modify: `ios-app/MacIOSWorkspace/RtcClient.swift`（加 onIncomingDataChannel 流，把对端创建的 pty/<sid> channel 暴露出来）

### SessionStore 设计

```swift
import Foundation
import Observation

struct Session: Identifiable, Equatable {
    let sid: String
    let cmd: [String]
    var alive: Bool
    var lastSeq: UInt32
}

@MainActor
@Observable
final class SessionStore {
    private(set) var sessions: [Session] = []
    private var sinks: [String: ([UInt8]) -> Void] = [:]
    private var rtcGlue: RtcGlue?

    func bind(_ glue: RtcGlue) {
        self.rtcGlue = glue
        Task {
            for await msg in await glue.ctrlMessages() {
                await handleCtrl(msg)
            }
        }
        Task {
            for await frame in await glue.ptyFrames() {
                if let cb = sinks[frame.sid] {
                    cb(Array(frame.data))
                    if let idx = sessions.firstIndex(where: { $0.sid == frame.sid }) {
                        sessions[idx].lastSeq = max(sessions[idx].lastSeq, frame.seq)
                    }
                }
            }
        }
    }

    func openSession(cmd: [String], rows: UInt16 = 24, cols: UInt16 = 80) async {
        let sid = String(UUID().uuidString.prefix(8))
        await rtcGlue?.sendCtrl(.openSession(sid: sid, cmd: cmd, rows: rows, cols: cols))
    }

    func attach(session: Session, sink: @escaping ([UInt8]) -> Void) async {
        sinks[session.sid] = sink
    }

    func sendInput(sid: String, bytes: Data) async {
        await rtcGlue?.sendPtyBytes(sid: sid, data: bytes)
    }

    func resize(sid: String, cols: UInt16, rows: UInt16) async {
        await rtcGlue?.sendCtrl(.resizeSession(sid: sid, rows: rows, cols: cols))
    }

    func killSession(_ sid: String) async {
        // 发 ctrl::KillSession 或直接关 channel；spec 没 KillSession 变体则用 ctrl 上自定义
    }

    private func handleCtrl(_ p: CtrlPayload) async {
        switch p {
        case .sessionOpened(let sid, _, _):
            sessions.append(Session(sid: sid, cmd: [], alive: true, lastSeq: 0))
        case .sessionExited(let sid, _):
            if let i = sessions.firstIndex(where: { $0.sid == sid }) { sessions[i].alive = false }
        case .sessionList(let list):
            sessions = list.map { Session(sid: $0.sid, cmd: $0.cmd, alive: $0.alive, lastSeq: 0) }
        case .backlogTruncated(let sid, let from):
            // 给 sink 推一段 marker bytes（如 "\r\n[…content truncated…]\r\n"）
            sinks[sid]?(Array("\r\n[…content truncated…]\r\n".utf8))
        default: break
        }
    }
}
```

### PairedView 增量

加一个 `Section("会话")`：
- 列已有 sessions
- 按钮 "新建 zsh"、"新建 Claude Code"（hardcoded for M3）→ tap 后 `openSession(cmd:)`
- `NavigationLink` 进 CliView

### 步骤

1. 写 SessionStore.swift
2. 改 RtcGlue.swift 加 `ctrlMessages()` AsyncStream + `ptyFrames()` AsyncStream + `sendCtrl(_:)` + `sendPtyBytes(sid:data:)`
3. 改 RtcClient.swift 加 onIncomingDataChannel stream（处理 Mac 端创建的 pty/<sid> DC）
4. 改 PairedView.swift 加会话 Section + NavigationLink
5. xcodebuild build 过
6. commit：`feat(ios-app): add SessionStore and wire CliView into PairedView with new-session buttons`

---

## Task M3.6：resume 流程 + backlog truncated UI

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/session_router.rs`（resume 路径）
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`（重连后调 resumeSession）
- Modify: `ios-app/MacIOSWorkspace/RtcGlue.swift`（PeerState.connected 时触发 resume 列表里所有 session）

### 步骤

1. SessionStore 持有 reconnect counter / token，PeerState.connected 时给每个 alive session 发 ResumeSession(sid, lastSeq)
2. session_router 的 resume 分支：先发 BacklogTruncated（如需），再 replay events，最后切 live
3. CliView feed BacklogTruncated 标记（`\r\n[…content truncated…]\r\n`）
4. xcodebuild build 过；cargo test 全过
5. commit：`feat: add session resume on reconnect and backlog_truncated divider`

---

## Task M3.7：8 并发 session 压力 + e2e 测试

**Files:** （主要是手动 + 简单脚本验证）

### 自动测试（macagent-core）

加一条 `tests/session_manager_concurrency_test.rs`：8 个并发 echo session 同时 spawn + 分别写 100 行 + verify all received。

### 手动验证（M2.6 后续）

按 M3.8 描述。

### 步骤

1. 写 concurrency_test.rs
2. cargo test 过
3. commit：`test(mac-agent): add 8 concurrent sessions stress test`

---

## Task M3.8：真机 CLI 测试

**Files:** 无新增。

### 验收

1. iPhone 真机进 Paired + Connect → "新建 zsh" → 进 CliView → 看到 prompt
2. 输入 `ls -la` → 输出实时回显
3. 跑 `claude code` 或 `npm install`（长任务）
4. iPhone 切飞行模式 60s → 切回 → 重新 Connect → CliView 自动 resume 看到完整输出
5. 同时开 8 个 session（4 zsh + 2 nvim + 2 claude code）→ 切换流畅，无掉字

记录每条结果。

---

## Task M3.9：M3 Final Review

dispatch reviewer subagent，按 M2 final review 模式审 commits `fd409f9..HEAD`。

---

## M3 验收清单

- [ ] `cd worker && npm test` 不变（27/27）
- [ ] `cd mac-agent && cargo test --workspace` 全绿（含 session_manager 4 + concurrency 1）
- [ ] `cd ios-app && xcodebuild test ...` 全绿
- [ ] CI 三条 workflow 全绿
- [ ] 真机：在 iPhone/iPad 上跑 `claude code` / `npm test` / `git status` 正常
- [ ] 60s 断网 → 重连 → 输出完整补回
- [ ] 8 并发稳定

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：spec §3.1 SessionManager / §3.2 CliView / §4.3 CLI 时序 / §5 PTY 错误 / §7 M3 全部映射到 M3.1–M3.8。
2. **占位符扫描**：仅 SwiftTerm SPM 添加可能需用户介入（M3.4 顶部已注明）；其余无 TBD/TODO。
3. **类型一致性**：CtrlPayload 5+1 个新变体在 Mac/iOS 同时加；canonical bytes（key sortedKeys + utf8）一致；pty_frame 二进制格式 [u32 BE seq][bytes] Mac/iOS 同。
4. **范围**：M3 仅 PTY；**不**含剪贴板（M4）、视频（M5）、输入注入（M6）。
5. **风险**：
   - portable-pty 的 reader/writer thread 模型与 tokio 互动需小心（用 spawn_blocking 包裹 sync read，再 mpsc 喂 async）
   - SwiftTerm 1.x API 可能有 breaking 变化（pin 1.2.0+ 留余地）
   - 8 并发 session 在 macOS 默认 fd ulimit (256) 下没问题，但 stress 测试需注意
   - WebRTC DataChannel 二进制帧大小限制（默认 16KB），单次 PTY 输出 > 16KB 需要分片（M3.1 在 router 层做：把单次 broadcast event 切片）

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven**（推荐）——M3.1/M3.2 高自动化；M3.3/M3.5 较重；M3.4 含 SPM 用户介入；M3.7/M3.8 真机
2. **Inline** ——顺序执行，关键节点 checkpoint

请用户选 1 或 2 后开始。
