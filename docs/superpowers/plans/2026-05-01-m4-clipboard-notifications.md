# M4 · 剪贴板 + 通知 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）。

**Goal:**
1. **剪贴板双向同步**：Mac NSPasteboard 内容变化 → 自动推到 iOS（500ms 轮询）；iOS 用户显式按"发送到 Mac" → 写 NSPasteboard。
2. **`notify <cmd>` 命令完成推送**：用户在 Mac 终端跑 `macagent notify -- pnpm build` → shim 跑命令 + 退出时上报 → Mac Agent 触发 APNs → iPhone 收到推送（含 deep-link 到对应 session）。
3. **正则 watcher 推送**：iOS 上对某 session 设置正则（如 `error:`、`pull request created`）→ producer 输出命中 → APNs 推送。

**Architecture（承接 M3 v2 producer 模型）：**
- **Mac Agent** 加 `ClipboardBridge`（轮询 NSPasteboard）+ `NotifyEngine`（管 notify 命令注册 + 正则 watcher 维护）。
- **socket_proto** 加 NotifyRegister/NotifyAck/NotifyComplete 三个变体（shim 与 Agent 间）。
- **ctrl 协议** 加 ClipboardSet（双向）+ WatchSession/UnwatchSession（iOS → Mac 配置 watcher）。
- **Worker** 加 `POST /push`：Mac 签名请求 → Worker 用 APNS_AUTH_KEY 签 JWT → 调 APNs HTTP/2。
- **iOS** 申请 Push Notification entitlement + 实现 PushHandler 注册 token + 处理 deep-link。
- **`macagent notify` 子命令**：复用 macagent-app binary 的 clap 子命令；shim 内 fork+exec+wait + Unix socket 报到。

**Tech Stack（M4 新增）:**
- Mac: 已有 `tokio` + `cocoa-foundation`/`objc2-app-kit` 拿 NSPasteboard（或简单 `pbpaste`/`pbcopy` 命令兜底）。
- Worker: `jose` 类 JWT（CF Workers 自带 `crypto.subtle` 可签 ES256）。
- iOS: `UNUserNotificationCenter` 申请权限 + `UIApplicationDelegate.didRegisterForRemoteNotificationsWithDeviceToken`。

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 ClipboardBridge / NotifyEngine、§3.2 ClipboardPanel / PushHandler、§3.3 `/push`、§4.5 剪贴板、§4.6 通知、§7 M4。

**M3 debt 一并清理**（M4.0 task）：
- I2：iOS LaunchAck/Reject 给用户反馈（loading / error alert）
- I3：WebRTC reconnect 后 SessionDetailView 自动 re-attach + refresh snapshot

---

## 协议契约

### CtrlPayload 新增（Rust + Swift 同步）

**剪贴板**：
```rust
ClipboardSet  { source: ClipSource, content: ClipContent }   // 双向
ClipboardAck  { req_id: Option<String> }                     // 收到确认（最佳努力）

ClipSource    { Mac | Ios }
ClipContent   { Text { data: String } }                      // M4 仅文本；图片/RTF 推 M5+
```

**Watcher**：
```rust
WatchSession    { sid: String, watcher_id: String, regex: String, name: String }
UnwatchSession  { sid: String, watcher_id: String }
WatchersList    { sid: String, watchers: Vec<WatcherInfo> }  // Mac → iOS 同步当前 watcher 列表
WatcherInfo     { id, regex, name, hits, last_match_text? }
WatcherMatched  { sid, watcher_id, line_text }               // Mac → iOS 通知（除了 APNs 外，前台 iOS 也能立即看到）
```

**通知（command notify）**：
- `notify` shim 不直接走 ctrl；Agent 处理后只推 APNs，iOS 端通过 PushHandler 拿到。**ctrl 上不需要新 case**。

### Unix socket 协议扩展（producer/notify ↔ Agent）

P2A（producer 或 shim → Agent）新增：
```rust
NotifyRegister {
    register_id: String,                  // shim 自生 UUID
    argv: Vec<String>,
    started_at_ms: u64,
    session_hint: Option<String>,         // 来自 MACAGENT_SESSION_ID env（如父 shell 是某 session 的 PTY）
    title: Option<String>,                // 自定义 title，否则用 argv[0]
}
NotifyComplete {
    register_id: String,
    exit_code: i32,
    ended_at_ms: u64,
}
```

A2P 新增：
```rust
NotifyAck { register_id: String }     // Agent 收到 NotifyRegister 后回；shim 用此知道注册成功
```

### Worker `POST /push`

请求（HMAC 签名，与 `/pair/revoke`、`/turn/cred` 同模式）：
```jsonc
{
  "pair_id": "<uuid>",
  "ts": 1735200000000,
  "sig": "<base64 HMAC-SHA256(mac_device_secret, 'push|<pair_id>|<ts>|<title>|<body>')>",
  "title": "build done",
  "body": "exit 0 in 5m12s",
  "deeplink": "macagent://session/<sid>",
  "thread_id": "<sid>"             // APNs thread-id 让多个 session 推送在通知中心分组
}
```

响应：
- 200 `{ pushed: true, apns_id: "<uuid>" }`
- 401 bad_sig / 404 unknown_pair / 410 apns_unregistered（Worker 已标记 dead）/ 503 apns_unavailable
- ts skew > 60s → 400

KV schema 扩展：
```
apns_dead:<pair_id>    → { reason, since }                (90 天 TTL，已存在)
apns_token:<pair_id>   → "<device_token_hex>"             (无 TTL；从 /pair/claim 时 ios_apns_token 复制；deviceToken refresh 时重写)
```

---

## 文件结构（增量）

```
mac-agent/crates/macagent-core/src/
├── ctrl_msg.rs                        ← 加 6 个新 CtrlPayload + 4 个共享类型
├── socket_proto.rs                    ← 加 NotifyRegister/Complete + NotifyAck

mac-agent/crates/macagent-app/src/
├── clipboard_bridge.rs                ← 新：NSPasteboard 轮询 + 写
├── notify_engine.rs                   ← 新：notify 命令注册 + 正则 watcher
├── push_client.rs                     ← 新：调 Worker /push + HMAC 签名
├── notify/                            ← 新：notify 子命令 producer
│   ├── mod.rs                         ← 入口（fork+exec+wait + socket register/complete）
│   └── socket_client.rs               ← 与 macagent run 的 socket_client 共用？或独立简化版
├── session_router.rs                  ← 改：dispatch 新 ctrl 消息（ClipboardSet / WatchSession /...）
└── main.rs                            ← 改：新增 `notify` 子命令

ios-app/MacIOSWorkspace/
├── Clipboard/                         ← 新
│   ├── ClipboardPanel.swift           ← 显示远端剪贴板 + "发送到 Mac" 按钮
│   └── ClipboardStore.swift           ← @Observable 存最近 5 条
├── Notify/                            ← 新
│   ├── PushHandler.swift              ← UNUserNotificationCenter delegate + token register
│   ├── WatcherStore.swift             ← @Observable 维护各 session 的 watchers
│   └── WatchersView.swift             ← UI：列出当前 watchers + 添加 regex
├── MacIOSWorkspaceApp.swift           ← 改：iOS UIApplicationDelegate 钩入 PushHandler
├── PairedView.swift                   ← 改：加 ClipboardPanel + WatchersView 入口
└── MacIOSWorkspace.entitlements       ← 加 aps-environment

worker/src/
├── push.ts                            ← 新：handlePush + APNs JWT (ES256)
├── apns.ts                            ← 新：APNs HTTP/2 client + 410 处理
└── index.ts                           ← 改：路由 /push
```

---

## Task M4.0：M3 debt 清理（I2 + I3）

**Files:**
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`（处理 LaunchAck/LaunchReject 把状态写回 store）
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift`（launcher 按钮加 loading state；显示 error alert）
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift::SessionDetailView`（observe glue 状态；reconnect 时重新 attach + 刷新 snapshot）

### 步骤

1. SessionStore 加 `pendingLaunches: [String: PendingLaunch]`，`launch()` 返回 reqId 并跟踪
2. handle .launchAck → `pendingLaunches[reqId]` 状态切 .succeeded(sid) 并自动 NavigationLink 跳进去；handle .launchReject → 切 .failed，UI 弹 alert
3. SessionListView 显示 launcher 时根据 pendingLaunches 显示 spinner
4. SessionDetailView 监听 glue.glueState（M2.5 已有的 GlueState）；连接恢复时再调一次 attach
5. xcodebuild build 过
6. commit：`fix(ios-app): launcher feedback (LaunchAck/Reject) and re-attach on reconnect (M3 I2+I3)`

---

## Task M4.1：Worker `POST /push` + APNs JWT 集成

**Files:**
- Create: `worker/src/apns.ts`（JWT ES256 签名 + HTTP/2 client）
- Create: `worker/src/push.ts`（handlePush）
- Modify: `worker/src/index.ts`（路由）
- Modify: `worker/src/env.ts`（加 APNS_AUTH_KEY/KEY_ID/TEAM_ID/BUNDLE_ID secret 类型）
- Modify: `worker/src/kv.ts`（加 putApnsToken/markApnsDead helpers）
- Modify: `worker/src/pair.ts`（在 /pair/claim 收到 ios_apns_token 时存 KV `apns_token:<pair_id>`）
- Create: `worker/test/push.test.ts`（5 条测试，mock APNs fetch）

### `apns.ts` 关键逻辑

APNs HTTP/2 endpoint：`https://api.push.apple.com/3/device/<token>` (production) 或 `api.sandbox.push.apple.com` (dev)。需要 ES256 JWT：

```typescript
async function signApnsJwt(env: Env): Promise<string> {
  // 1. import ES256 private key from env.APNS_AUTH_KEY (PEM)
  // 2. payload: { iss: env.APNS_TEAM_ID, iat: <unix sec> }
  // 3. header: { alg: "ES256", kid: env.APNS_KEY_ID, typ: "JWT" }
  // 4. crypto.subtle.sign with ECDSA SHA-256
  // 5. base64url encode header + payload + sig
}

export async function pushApns(env: Env, deviceToken: string, payload: object): Promise<{ ok: boolean; status: number; reason?: string }> {
  const jwt = await signApnsJwt(env);
  const isProd = env.APNS_ENV !== "sandbox";   // 默认 prod；secret 设 sandbox 时用 dev
  const host = isProd ? "api.push.apple.com" : "api.sandbox.push.apple.com";
  const res = await fetch(`https://${host}/3/device/${deviceToken}`, {
    method: "POST",
    headers: {
      "authorization": `bearer ${jwt}`,
      "apns-topic": env.APNS_BUNDLE_ID,
      "apns-push-type": "alert",
      "apns-priority": "10",
      "content-type": "application/json",
    },
    body: JSON.stringify(payload),
  });
  if (res.status === 410) return { ok: false, status: 410, reason: "unregistered" };
  if (!res.ok) {
    const errText = await res.text().catch(() => "");
    return { ok: false, status: res.status, reason: errText.slice(0, 200) };
  }
  return { ok: true, status: 200 };
}
```

### `push.ts` 关键逻辑

```typescript
export async function handlePush(req: Request, env: Env): Promise<Response> {
  // 1. 解 body { pair_id, ts, sig, title, body, deeplink?, thread_id? }
  // 2. 验签：HMAC-SHA256(pair.mac_device_secret, "push|<pair_id>|<ts>|<title>|<body>")
  //    与 /pair/revoke 同模式（用 mac_device_secret，因为发起方一定是 Mac）
  // 3. 检查 ts skew, pair 存在，未 revoked
  // 4. 检查 apns_dead:<pair_id> → 若存在直接返 410
  // 5. 拿 apns_token:<pair_id>
  // 6. 调用 pushApns(env, token, { aps: { alert: { title, body }, "thread-id": thread_id, sound: "default" }, deeplink })
  // 7. 若 410 → markApnsDead → return 410
  // 8. 200 { pushed: true }
}
```

### 测试要点

- ✅ valid push：mock APNs return 200，验证 worker 返 200
- ✅ bad_sig → 401
- ✅ unknown_pair → 404
- ✅ ts skew > 60s → 400
- ✅ APNs 返 410 → worker markApnsDead + return 410，第二次同 pair_id 直接 410（不再调 APNs）

### 步骤

1. 写 apns.ts + push.ts
2. 改 index.ts 加路由
3. 改 env.ts + kv.ts
4. 改 pair.ts（claim 时存 ios_apns_token 到 KV）
5. 写 push.test.ts
6. `npm test` 全过（27 + 5 ≈ 32）
7. `npm run typecheck`
8. commit：`feat(worker): add POST /push with APNs JWT signing and 410 unregistered handling`

---

## Task M4.2：Mac ClipboardBridge

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（加 ClipboardSet + ClipSource + ClipContent）
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`（同步）
- Create: `mac-agent/crates/macagent-app/src/clipboard_bridge.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（启动时 spawn ClipboardBridge）

### `clipboard_bridge.rs` 关键逻辑

```rust
//! Polls NSPasteboard.changeCount every 500ms; on change, push ClipboardSet to ctrl.
//! Receives remote ClipboardSet (from iOS) and writes to NSPasteboard.

use std::process::Command;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use macagent_core::ctrl_msg::{CtrlPayload, ClipSource, ClipContent};

const POLL_INTERVAL_MS: u64 = 500;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;  // 1 MB

pub struct ClipboardBridge {
    last_change_count: AtomicI64,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    history: tokio::sync::Mutex<Vec<String>>,
}

impl ClipboardBridge {
    pub fn new(ctrl_tx: mpsc::UnboundedSender<CtrlPayload>) -> Self { ... }
    
    /// 启动后台轮询循环。
    pub async fn run(self: Arc<Self>) {
        let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
        loop {
            tick.tick().await;
            if let Some(text) = read_pasteboard_if_changed(&self.last_change_count) {
                if text.len() <= MAX_CONTENT_BYTES {
                    self.history.lock().await.push(text.clone());
                    self.history.lock().await.truncate(5);
                    let _ = self.ctrl_tx.send(CtrlPayload::ClipboardSet {
                        source: ClipSource::Mac,
                        content: ClipContent::Text { data: text },
                    });
                }
            }
        }
    }
    
    /// 收到来自 iOS 的 ClipboardSet → 写 NSPasteboard。
    pub fn write_remote(&self, content: &ClipContent) {
        match content {
            ClipContent::Text { data } => {
                // 用 pbcopy 命令最简单（避开 objc binding）
                let mut child = Command::new("pbcopy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .ok();
                if let Some(child) = child.as_mut() {
                    if let Some(stdin) = child.stdin.as_mut() {
                        let _ = std::io::Write::write_all(stdin, data.as_bytes());
                    }
                    let _ = child.wait();
                }
            }
        }
    }
}

/// 用 NSPasteboard 直接读 changeCount + string contents 最准；用 pbpaste 兜底。
fn read_pasteboard_if_changed(last_count: &AtomicI64) -> Option<String> {
    // 简单实现：每次跑 pbpaste 拿当前内容；记 hash 判断变化
    // 更准实现：objc2-app-kit::NSPasteboard::changeCount() 比对
    // M4 简化版：用 pbpaste；hash 比对（不是真正的 changeCount，但够用）
    let output = Command::new("pbpaste").output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let hash = simple_hash(&text);
    let prev = last_count.swap(hash, Ordering::SeqCst);
    if prev != hash {
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}

fn simple_hash(s: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as i64
}
```

### 集成到 ui.rs

ui.rs 启动 menu bar 时同时 spawn 一个 ClipboardBridge task，把 ctrl_tx 接到 rtc_glue 的发送通道（与 session_router 共用同一通道）。

session_router 收到 iOS 来的 ClipboardSet → 调 clipboard_bridge.write_remote。

### 测试

- unit test: `simple_hash` 不同字符串产生不同 hash（弱测试，足够）
- 手动：运行 menu bar agent，复制几段文本到 Mac 剪贴板，看 ctrl_tx 收到 ClipboardSet。M4.7 真机验证全链路。

### 步骤

1. 改 ctrl_msg.rs 加 ClipboardSet + ClipSource + ClipContent
2. 改 CtrlMessage.swift 同步
3. 写 clipboard_bridge.rs
4. 改 ui.rs spawn bridge
5. 改 session_router.rs 路由 ClipboardSet
6. cargo test / clippy / fmt
7. xcodebuild build 过
8. commit：`feat: add ClipboardBridge with NSPasteboard polling and bidirectional ctrl ClipboardSet`

---

## Task M4.3：iOS ClipboardPanel

**Files:**
- Create: `ios-app/MacIOSWorkspace/Clipboard/ClipboardStore.swift`
- Create: `ios-app/MacIOSWorkspace/Clipboard/ClipboardPanel.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（加 NavigationLink 进 ClipboardPanel）
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`（dispatch ClipboardSet）

### `ClipboardStore.swift` 设计

```swift
@MainActor
@Observable
final class ClipboardStore {
    /// 最近 5 条接收的剪贴板（最新在前）
    private(set) var history: [ClipEntry] = []
    private let glue: RtcGlue?
    
    init(glue: RtcGlue?) { self.glue = glue }
    
    func handleRemote(_ content: ClipContent) {
        switch content {
        case .text(let text):
            history.insert(.init(text: text, ts: .now), at: 0)
            history = Array(history.prefix(5))
            // 自动复制到 UIPasteboard（iOS → 用户在 iOS 剪贴板可粘到任意 App）
            UIPasteboard.general.string = text
        }
    }
    
    func sendToMac(_ text: String) async {
        await glue?.sendCtrl(.clipboardSet(source: .ios, content: .text(text)))
    }
}

struct ClipEntry: Identifiable {
    let id = UUID()
    let text: String
    let ts: Date
}
```

### `ClipboardPanel.swift` 设计

UI 含：
- "发送到 Mac" 文本输入框 + 按钮（默认拿 UIPasteboard.general.string）
- "最近接收" 列表（5 条历史，每条点击可复制到 UIPasteboard）

### 步骤

1. 写 ClipboardStore + ClipboardPanel
2. 改 PairedView 加入口（"剪贴板" NavigationLink）
3. 改 SessionStore（或新建 RootStore）dispatch ClipboardSet 到 ClipboardStore
4. xcodebuild build 过
5. commit：`feat(ios-app): add ClipboardStore + ClipboardPanel for bidirectional sync`

---

## Task M4.4：`macagent notify` 子命令

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/socket_proto.rs`（加 NotifyRegister/Complete + NotifyAck）
- Modify: `mac-agent/crates/macagent-app/src/main.rs`（clap 加 `notify` 子命令）
- Create: `mac-agent/crates/macagent-app/src/notify/mod.rs`

### `notify/mod.rs` 关键逻辑

```rust
use clap::Args;

#[derive(Args, Debug)]
pub struct NotifyArgs {
    /// 命令 title 显示在 push 通知里（默认 argv[0]）
    #[arg(long)]
    pub title: Option<String>,
    
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

pub fn run_main(args: NotifyArgs) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(notify_run(args))
}

async fn notify_run(args: NotifyArgs) -> anyhow::Result<i32> {
    let register_id = uuid::Uuid::new_v4().to_string();
    let started_at_ms = current_ms();
    let session_hint = std::env::var("MACAGENT_SESSION_ID").ok();
    
    // 连 socket（如失败：fallback：仍然 exec 命令、不发 push、stderr 警告）
    let socket_result = SocketClient::connect().await;
    let mut socket = match socket_result {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("warning: macagent agent socket unreachable ({}), command will run without notification", e);
            None
        }
    };
    
    // 发 NotifyRegister，等 NotifyAck
    if let Some(s) = socket.as_mut() {
        s.send(P2A::NotifyRegister { register_id: register_id.clone(), argv: args.command.clone(),
            started_at_ms, session_hint, title: args.title.clone() }).await?;
        match tokio::time::timeout(Duration::from_secs(3), s.recv()).await {
            Ok(Ok(A2P::NotifyAck { register_id: _ })) => {},
            _ => {
                eprintln!("warning: agent did not ack within 3s, continuing without notification");
                socket = None;
            }
        }
    }
    
    // fork+exec+wait the command（继承父进程 stdin/stdout/stderr）
    let mut child = std::process::Command::new(&args.command[0])
        .args(&args.command[1..])
        .spawn()?;
    let status = child.wait()?;
    let exit_code = status.code().unwrap_or(-1);
    let ended_at_ms = current_ms();
    
    // 发 NotifyComplete
    if let Some(s) = socket.as_mut() {
        let _ = s.send(P2A::NotifyComplete { register_id, exit_code, ended_at_ms }).await;
    }
    
    Ok(exit_code)
}
```

### 步骤

1. 改 socket_proto.rs 加 3 个变体
2. 改 main.rs clap dispatch 加 Notify(NotifyArgs)
3. 写 notify/mod.rs
4. cargo build / clippy / fmt
5. cargo test 全过
6. 手测：`cargo run -p macagent-app -- notify -- echo hello`，agent 没起 → 应见 warning + echo hello + exit 0
7. commit：`feat(mac-agent): add `macagent notify` subcommand for command completion notifications`

---

## Task M4.5：NotifyEngine + Watcher

**Files:**
- Create: `mac-agent/crates/macagent-app/src/notify_engine.rs`
- Create: `mac-agent/crates/macagent-app/src/push_client.rs`
- Modify: `mac-agent/crates/macagent-app/src/agent_socket.rs`（处理 NotifyRegister/Complete）
- Modify: `mac-agent/crates/macagent-app/src/session_router.rs`（dispatch WatchSession/UnwatchSession + 监听 TermDelta 跑 regex）
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（加 WatchSession/UnwatchSession/WatchersList/WatcherInfo/WatcherMatched）

### `notify_engine.rs`

维护两类对象：
1. **In-flight notify commands**：`HashMap<register_id, NotifyEntry { argv, started_at, session_hint }>`，收到 NotifyComplete → 算 duration → 调 push_client.send_push(title, body, deeplink_for_session_hint?)
2. **Per-session watchers**：`HashMap<sid, Vec<Watcher { id, regex, name, hits, last_match }>>`，session_router 喂入新 line（解 TermDelta 中文本）→ 对每个 watcher 做 `regex::Regex::is_match` → 命中 → push_client + ctrl WatcherMatched

### `push_client.rs`

发 HMAC 签名请求到 Worker `/push`：

```rust
pub struct PushClient {
    worker_url: String,
    pair_id: String,
    mac_device_secret: Vec<u8>,
}

impl PushClient {
    pub async fn send(&self, title: &str, body: &str, deeplink: Option<&str>, thread_id: Option<&str>) -> Result<()> {
        let ts = current_ms();
        let msg = format!("push|{}|{}|{}|{}", self.pair_id, ts, title, body);
        let sig = base64::encode(hmac_sign(&self.mac_device_secret, msg.as_bytes()));
        reqwest::Client::new()
            .post(format!("{}/push", self.worker_url))
            .json(&serde_json::json!({
                "pair_id": self.pair_id,
                "ts": ts,
                "sig": sig,
                "title": title,
                "body": body,
                "deeplink": deeplink,
                "thread_id": thread_id,
            }))
            .send().await?
            .error_for_status()?;
        Ok(())
    }
}
```

### 步骤

1. 改 ctrl_msg.rs 加 watcher 相关 case + iOS 同步
2. 改 socket_proto.rs（M4.4 已加）
3. 改 agent_socket.rs handle NotifyRegister/Complete
4. 写 push_client.rs
5. 写 notify_engine.rs
6. 改 session_router.rs：
   - 处理 WatchSession/UnwatchSession ctrl 消息
   - 在 producer TermDelta 转发给 iOS 的同时也喂给 NotifyEngine 跑 regex
7. cargo test 全过
8. commit：`feat(mac-agent): add NotifyEngine (regex watchers + notify completion) and push_client`

---

## Task M4.6：iOS APNs 集成

**Files:**
- Modify: `ios-app/MacIOSWorkspace.entitlements`（如不存在则在 pbxproj 中创建）
- Create: `ios-app/MacIOSWorkspace/Notify/PushHandler.swift`
- Modify: `ios-app/MacIOSWorkspace/MacIOSWorkspaceApp.swift`（UIApplicationDelegate 钩入）
- Create: `ios-app/MacIOSWorkspace/Notify/WatcherStore.swift`
- Create: `ios-app/MacIOSWorkspace/Notify/WatchersView.swift`
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift::SessionDetailView`（加 watchers 入口按钮）

### Entitlement & 权限

`.entitlements`：
```xml
<key>aps-environment</key>
<string>development</string>   <!-- 真机 release 改 production -->
```

Info.plist (via pbxproj `INFOPLIST_KEY_*`)：
- `NSUserNotificationsUsageDescription`：理由文本

App Store Connect / Developer Portal：
- 创建 APNs Auth Key（.p8）→ 拿 Key ID + Team ID
- 启用 App ID 的 Push Notification capability

### `PushHandler.swift`

```swift
import UserNotifications
import UIKit

@MainActor
final class PushHandler: NSObject, ObservableObject, UNUserNotificationCenterDelegate, UIApplicationDelegate {
    @Published var deviceToken: String?
    
    func requestAuthorization() async {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        do {
            let granted = try await center.requestAuthorization(options: [.alert, .badge, .sound])
            if granted {
                await UIApplication.shared.registerForRemoteNotifications()
            }
        } catch {
            print("UNUserNotificationCenter authorization error:", error)
        }
    }
    
    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        let hex = token.map { String(format: "%02x", $0) }.joined()
        deviceToken = hex
        // 重新调 PairingFlow 把新 token 同步到 Worker：POST /pair/refresh-apns?
        // M4 简化：要求用户重新配对一次（或后续 worker 加 refresh-apns 端点）
        print("APNs token:", hex)
    }
    
    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        print("APNs register failed:", error)
    }
    
    // Foreground notification handling
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        return [.banner, .sound]
    }
    
    // Tap handling - deep-link to session
    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let userInfo = response.notification.request.content.userInfo
        if let deeplink = userInfo["deeplink"] as? String {
            // route via NotificationCenter post → some root view subscribes
            NotificationCenter.default.post(name: .macagentDeepLink, object: deeplink)
        }
    }
}

extension Notification.Name {
    static let macagentDeepLink = Notification.Name("macagentDeepLink")
}
```

### 步骤

1. 在 Xcode 加 entitlement 文件（手改 pbxproj 或用户在 Xcode 里加 capability）
2. 写 PushHandler.swift
3. 改 MacIOSWorkspaceApp.swift 加 UIApplicationDelegateAdaptor
4. 写 WatcherStore + WatchersView
5. 改 SessionDetailView 加 watcher 入口
6. xcodebuild build 过（不带 push 真测）
7. commit：`feat(ios-app): add PushHandler (APNs entitlement + token register) and watchers UI`

> **用户需要做**：在 Xcode 里 Capability 加 Push Notifications；Apple Developer Portal 配 App ID + Push key。M4.7 联调时一并处理。

---

## Task M4.7：真机端到端 APNs + watcher 联调

需要：
1. 用户在 Apple Developer 创建 APNs Auth Key（.p8）→ 拿 Key ID + Team ID
2. wrangler 配 secret：
```bash
echo "<paste .p8 content>" | npx wrangler secret put APNS_AUTH_KEY
echo "<key_id>" | npx wrangler secret put APNS_KEY_ID
echo "<team_id>" | npx wrangler secret put APNS_TEAM_ID
echo "com.hemory.macagent" | npx wrangler secret put APNS_BUNDLE_ID    # iOS app bundle id
echo "sandbox" | npx wrangler secret put APNS_ENV                      # dev 期间
npx wrangler deploy
```
3. iPhone 真机：iOS app 新版（含 APNs）→ 重新配对（让 Worker 拿到 ios_apns_token）→ Connect WebRTC

### 验收

1. **clipboard**：Mac 复制 "hello" → iPhone PairedView → Clipboard 入口看到 "hello" 出现 + UIPasteboard.string == "hello"
2. **clipboard reverse**：iPhone 输入 "world" → 点"发送到 Mac" → Mac 终端 `pbpaste` 输出 "world"
3. **notify**：Mac 终端 `cargo run -p macagent-app -- notify -- sleep 5; echo done` → iPhone 5 秒后收到推送 "done | exit 0 in 5s"
4. **watcher**：iOS 设 watcher `regex:"error.*"` → producer 输出包含 "error: file not found" → iPhone 收到推送 + ctrl WatcherMatched 在 watchers UI 实时显示
5. **deep-link**：点击推送 → iOS app 自动跳到对应 session 的 TermView

### 步骤

无 commit；纯人工验证。结果记录到 final review。

---

## Task M4.8：M4 final review

dispatch reviewer subagent，按 M3 final review 同样模式审 commits `52255d8..HEAD`。

---

## M4 验收清单

- [ ] worker npm test 全绿（27 + 5 = 32）
- [ ] mac-agent cargo test --workspace 全绿（核心 + app 加新单测）
- [ ] ios-app xcodebuild test 全绿
- [ ] CI 三条 workflow 全绿
- [ ] 真机：剪贴板 Mac→iOS auto sync ✓
- [ ] 真机：剪贴板 iOS→Mac via "Send to Mac" 按钮 ✓
- [ ] 真机：`notify -- sleep 5; echo done` 后 iPhone 收推送 ✓
- [ ] 真机：watcher regex 命中 → 推送 + UI 实时显示 ✓
- [ ] 真机：点推送 deep-link 进 TermView ✓

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：spec §3.1 ClipboardBridge / NotifyEngine、§3.2 ClipboardPanel / PushHandler、§3.3 /push、§4.5 / §4.6 / §7 M4 → 全部映射到 M4.0-M4.7。
2. **占位符扫描**：M4.6 entitlement 添加可能需用户介入（同 M2.4 SPM 风格）；其余无 TBD/TODO。
3. **类型一致性**：CtrlPayload Mac/iOS 同步；canonical_bytes M3.fix 已递归排序，新增 case 自动受益。
4. **M3 debt 在 M4.0 清掉**，避免污染 M4。
5. **风险**：
   - APNs Auth Key 需用户在 Apple Developer 操作；若无 Apple Developer 账号会卡住
   - iOS Push entitlement 真机需要 provisioning profile，TestFlight build 比较稳
   - NSPasteboard 直接绑定（objc2-app-kit）麻烦；M4.2 退到 pbpaste/pbcopy 子进程是务实简化
   - watcher regex 性能：每条 TermDelta 行 × N watchers，无问题（M4 上限 8 sessions × ~10 watchers × <1ms 正则）

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven**（推荐）——延续 M0-M3 节奏；M4.0/M4.4/M4.5 自动化高；M4.6 含用户介入（entitlement）；M4.7 真机
2. **Inline Execution**

请用户选 1 或 2 后再开始执行。
