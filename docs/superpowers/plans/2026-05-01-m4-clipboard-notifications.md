# M4 · 剪贴板 + 通知 + iOS 输入增强 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）。

**Goal（系统化解决 iOS 作为 Mac 外设的输入 / 剪贴板 / 通知三类能力）：**

1. **iOS 输入增强（ComposeSheet）**：iOS 26 标准 `TextEditor` 多行 sheet → 用户系统/微信/搜狗/讯飞键盘的 IME 与 🎤 语音天然可用 → Send 时一次性把整段 UTF-8 字节流送进 PTY。CLI 与 GUI 共用同一组件（M4 仅落 CLI 路径，GUI 留给 M5/M6 接 InputInjector.paste_text）。
2. **剪贴板双向同步**：Mac NSPasteboard 内容变化 → 自动推到 iOS（500 ms 轮询）；iOS 用户显式按"发送到 Mac" → 写 NSPasteboard。
3. **`notify <cmd>` 命令完成推送**：用户在 Mac 终端跑 `macagent notify -- pnpm build` → shim 跑命令 + 退出时上报 → Mac Agent 触发 APNs → iPhone 收到推送（含 deep-link）。
4. **正则 watcher 推送**：iOS 上对某 session 设置正则 → producer 输出命中 → APNs + ctrl `WatcherMatched`（前台 UI 实时显示）。

**Architecture（承接 M3 v2 producer 模型）：**

- **iOS** 加 `ComposeSheet`（共用 SwiftUI 组件）+ `ClipboardStore`/`ClipboardPanel` + `WatcherStore`/`WatchersView` + `PushHandler`（UNUserNotificationCenter）。
- **Mac Agent** 加 `ClipboardBridge`（NSPasteboard 轮询）+ `NotifyEngine`（管 notify 命令注册 + 正则 watcher 维护）+ `PushClient`（HMAC 签名调 Worker `/push`）。
- **socket_proto** 加 `NotifyRegister/Complete` + `NotifyAck`（shim 与 Agent 间）。
- **ctrl 协议** 加 `ClipboardSet`（双向）、`WatchSession`/`UnwatchSession`/`WatchersList`/`WatcherMatched`（Mac/iOS 同步）。**ComposeSheet 不需要新 ctrl 类型**，复用 M3.1 的 `Input { sid, TerminalInput::Text }`。
- **Worker** 加 `POST /push`：Mac 签名请求 → Worker 用 APNS_AUTH_KEY 签 ES256 JWT → 调 APNs HTTP/2。
- **iOS 申请 Push Notification entitlement**。
- **`macagent notify` 是 macagent-app 的 clap 子命令**：fork+exec+wait + Unix socket 报到（不复用 macagent run 的复杂 PTY 路径）。

**Tech Stack（M4 新增）:**

- Mac: `tokio` Command（`pbpaste` / `pbcopy` 子进程跑剪贴板，避开 objc2-app-kit 复杂绑定）；`regex = "1"`（正则 watcher）。
- Worker: `crypto.subtle`（ES256 签 APNs JWT）+ HTTP/2 fetch APNs endpoint。
- iOS: `UserNotifications` framework（系统自带，无新依赖）；SwiftUI `TextEditor`（已有，无新依赖）。

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 ClipboardBridge / NotifyEngine、§3.2 ClipboardPanel / ComposeSheet / PushHandler、§3.3 `/push`、§4.5 / §4.6、§7 M4；以及独立 spec [`2026-04-30-ios-input-compose-design.md`](../specs/2026-04-30-ios-input-compose-design.md)（ComposeSheet 完整设计）。

**M3 debt 一并清理（M4.0 task）：**
- I2：iOS LaunchAck/Reject 给用户反馈（loading / error alert）
- I3：WebRTC reconnect 后 SessionDetailView 自动 re-attach + refresh snapshot

---

## 协议契约

### CtrlPayload 新增（Rust + Swift 同步）

**剪贴板**：
```rust
ClipboardSet { source: ClipSource, content: ClipContent }   // 双向
ClipSource    { Mac | Ios }
ClipContent   { Text { data: String } }                     // M4 仅文本；图片/RTF 推 M5+
```

**Watcher**：
```rust
WatchSession    { sid: String, watcher_id: String, regex: String, name: String }
UnwatchSession  { sid: String, watcher_id: String }
WatchersList    { sid: String, watchers: Vec<WatcherInfo> }
WatcherInfo     { id: String, regex: String, name: String, hits: u32, last_match: Option<String> }
WatcherMatched  { sid: String, watcher_id: String, line_text: String }
```

**ComposeSheet 不需要新 ctrl 类型**。Send 时复用 M3.1 `Input { sid, payload: TerminalInput::Text { data: <整段 utf8> } }`，传输层与 inline live-typing 无差别。

### Unix socket 协议扩展

P2A（producer 或 notify shim → Agent）新增：
```rust
NotifyRegister {
    register_id: String,
    argv: Vec<String>,
    started_at_ms: u64,
    session_hint: Option<String>,
    title: Option<String>,
}
NotifyComplete {
    register_id: String,
    exit_code: i32,
    ended_at_ms: u64,
}
```

A2P 新增：
```rust
NotifyAck { register_id: String }
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
  "thread_id": "<sid>"
}
```

响应：200 / 401 / 404 / 410（apns_unregistered，Worker 标 dead）/ 503 / 400。

KV 扩展：
```
apns_token:<pair_id>   → "<device_token_hex>"   (无 TTL；/pair/claim 时存 + iOS 端定期 refresh)
apns_dead:<pair_id>    → { reason, since }      (M2.7 已有，用于 /push 短路)
```

---

## 文件结构（增量）

```
mac-agent/crates/macagent-core/src/
├── ctrl_msg.rs                        ← 加 6 个新 CtrlPayload + 4 个共享类型
└── socket_proto.rs                    ← 加 NotifyRegister/Complete + NotifyAck

mac-agent/crates/macagent-app/src/
├── clipboard_bridge.rs                ← 新：pbpaste/pbcopy 包装
├── notify_engine.rs                   ← 新：notify 命令注册 + 正则 watcher
├── push_client.rs                     ← 新：调 Worker /push + HMAC 签名
├── notify/                            ← 新：notify 子命令 producer
│   └── mod.rs
├── session_router.rs                  ← 改：dispatch 新 ctrl 消息
└── main.rs                            ← 改：clap 加 `notify` 子命令

ios-app/MacIOSWorkspace/
├── Compose/                           ← 新（v0.1 仅 CLI 路径用）
│   └── ComposeSheet.swift             ← TextEditor sheet（CLI / GUI 共用，注入不同 onSend）
├── Clipboard/                         ← 新
│   ├── ClipboardPanel.swift
│   └── ClipboardStore.swift
├── Notify/                            ← 新
│   ├── PushHandler.swift              ← UNUserNotificationCenter delegate + token register
│   ├── WatcherStore.swift
│   └── WatchersView.swift
├── Term/InputBar.swift                ← 改：右端加 ✏️ 按钮 → 弹 ComposeSheet
├── MacIOSWorkspaceApp.swift           ← 改：UIApplicationDelegateAdaptor → PushHandler
├── PairedView.swift                   ← 改：加 NavigationLink "剪贴板"
├── SessionListView.swift::SessionDetailView ← 改：加 NavigationLink "正则提醒"
└── MacIOSWorkspace.entitlements       ← 加 aps-environment

worker/src/
├── apns.ts                            ← 新：APNs ES256 JWT + HTTP/2 client
├── push.ts                            ← 新：handlePush
├── kv.ts                              ← 改：putApnsToken / markApnsDead
├── pair.ts                            ← 改：claim 时存 ios_apns_token
└── index.ts                           ← 改：路由 /push
```

---

## Task M4.0：M3 debt 清理（I2 + I3）

**Files:**
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift`

### 改动

1. SessionStore 加 `pendingLaunches: [String: PendingLaunch]`，`launch()` 返回 reqId 并跟踪。
2. handle `.launchAck` → 找 reqId → 自动 NavigationLink 跳进新 sid 的 SessionDetailView；handle `.launchReject` → 弹 alert。
3. SessionListView launcher 按钮根据 pendingLaunches 状态显示 ProgressView。
4. SessionDetailView 监听 `glue.glueState`（M2.5 已有），从非 connected 切回 connected 时再调一次 `attach(sid)`。

### 步骤

1. 改 SessionStore + SessionListView
2. xcodebuild build 过
3. commit：`fix(ios-app): launcher feedback (LaunchAck/Reject) and re-attach on reconnect (M3 I2+I3)`

---

## Task M4.1：Worker `POST /push` + APNs JWT

**Files:**
- Create: `worker/src/apns.ts`
- Create: `worker/src/push.ts`
- Modify: `worker/src/index.ts`、`worker/src/env.ts`、`worker/src/kv.ts`、`worker/src/pair.ts`
- Create: `worker/test/push.test.ts`

### `apns.ts`

ES256 JWT 签名 + HTTP/2 POST 到 `api.push.apple.com`（prod）或 `api.sandbox.push.apple.com`（dev）。

```typescript
async function signApnsJwt(env: Env): Promise<string> {
  const header = { alg: "ES256", kid: env.APNS_KEY_ID, typ: "JWT" };
  const payload = { iss: env.APNS_TEAM_ID, iat: Math.floor(Date.now() / 1000) };
  const headerB64 = b64urlEncode(JSON.stringify(header));
  const payloadB64 = b64urlEncode(JSON.stringify(payload));
  const signingInput = `${headerB64}.${payloadB64}`;

  const key = await importPemPrivateKey(env.APNS_AUTH_KEY!);
  const sig = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(signingInput),
  );
  const sigB64 = b64urlEncode(new Uint8Array(sig));
  return `${signingInput}.${sigB64}`;
}

export async function pushApns(env: Env, deviceToken: string, payload: object): Promise<{ ok: boolean; status: number; reason?: string }> {
  const jwt = await signApnsJwt(env);
  const isProd = env.APNS_ENV !== "sandbox";
  const host = isProd ? "api.push.apple.com" : "api.sandbox.push.apple.com";
  const res = await fetch(`https://${host}/3/device/${deviceToken}`, {
    method: "POST",
    headers: {
      authorization: `bearer ${jwt}`,
      "apns-topic": env.APNS_BUNDLE_ID!,
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

> `importPemPrivateKey` 用 PKCS8 格式解析 .p8（参考 `crypto.subtle.importKey("pkcs8", ...)`）。

### `push.ts`

```typescript
export async function handlePush(req: Request, env: Env): Promise<Response> {
  let body: { pair_id?: string; ts?: number; sig?: string; title?: string; body?: string; deeplink?: string; thread_id?: string };
  try { body = await req.json(); } catch { return Response.json({ error: "invalid_json" }, { status: 400 }); }
  if (!body.pair_id || typeof body.ts !== "number" || !body.sig || !body.title || !body.body) {
    return Response.json({ error: "missing_fields" }, { status: 400 });
  }
  if (Math.abs(Date.now() - body.ts) > 60_000) return Response.json({ error: "ts_out_of_range" }, { status: 400 });

  const pair = await getPair(env, body.pair_id);
  if (!pair) return Response.json({ error: "unknown_pair" }, { status: 404 });

  const msg = `push|${body.pair_id}|${body.ts}|${body.title}|${body.body}`;
  const sigBytes = b64decode(body.sig);
  const macOk = await hmacVerify(b64decode(pair.mac_device_secret_b64), msg, sigBytes);
  if (!macOk) return Response.json({ error: "bad_sig" }, { status: 401 });

  if (await isApnsDead(env, body.pair_id)) {
    return Response.json({ error: "apns_unregistered" }, { status: 410 });
  }

  const token = await env.PAIRS.get(`apns_token:${body.pair_id}`);
  if (!token) return Response.json({ error: "apns_token_missing" }, { status: 410 });

  if (!env.APNS_AUTH_KEY || !env.APNS_KEY_ID || !env.APNS_TEAM_ID || !env.APNS_BUNDLE_ID) {
    return Response.json({ error: "apns_not_configured" }, { status: 503 });
  }

  const result = await pushApns(env, token, {
    aps: { alert: { title: body.title, body: body.body }, "thread-id": body.thread_id, sound: "default" },
    deeplink: body.deeplink,
  });

  if (result.status === 410) {
    await markApnsDead(env, body.pair_id, "unregistered");
    return Response.json({ error: "apns_unregistered" }, { status: 410 });
  }
  if (!result.ok) return Response.json({ error: "turn_unavailable", status: result.status, reason: result.reason }, { status: 503 });
  return Response.json({ pushed: true });
}
```

### Tests（`push.test.ts`，5 条）

1. valid push（mock APNs 200）→ 200 `{ pushed: true }`
2. bad sig → 401
3. unknown pair → 404
4. ts skew → 400
5. APNs 返 410 → worker 200 内调 markApnsDead，第二次同 pair_id 直接 410

mock 用 `vitest-pool-workers` 的 `fetchMock` 拦截 `api.sandbox.push.apple.com`。secret 通过 `vitest.config.ts` `miniflare.bindings` 注入测试值（dummy ES256 .p8）。

### 步骤

1. 写 apns.ts + push.ts
2. 改 env.ts 加 secret 类型
3. 改 kv.ts 加 putApnsToken / markApnsDead
4. 改 pair.ts handlePairClaim 收到 ios_apns_token 时 `env.PAIRS.put('apns_token:' + pair_id, ios_apns_token)`
5. 改 index.ts 加路由
6. 写 5 条测试
7. `npm test` 全过（27 + 5 = 32）+ `npm run typecheck`
8. commit：`feat(worker): add POST /push with APNs ES256 JWT and 410 unregistered handling`

---

## Task M4.2：Mac ClipboardBridge

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（加 ClipboardSet 等）
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`（同步）
- Create: `mac-agent/crates/macagent-app/src/clipboard_bridge.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（启动 spawn ClipboardBridge）
- Modify: `mac-agent/crates/macagent-app/src/session_router.rs`（dispatch ClipboardSet）

### `clipboard_bridge.rs` 关键实现

```rust
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use macagent_core::ctrl_msg::{CtrlPayload, ClipSource, ClipContent};

const POLL_INTERVAL_MS: u64 = 500;
const MAX_BYTES: usize = 1024 * 1024;

pub struct ClipboardBridge {
    last_hash: AtomicI64,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
}

impl ClipboardBridge {
    pub fn new(ctrl_tx: mpsc::UnboundedSender<CtrlPayload>) -> Self {
        Self { last_hash: AtomicI64::new(0), ctrl_tx }
    }

    pub async fn run_polling(self: Arc<Self>) {
        let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
        loop {
            tick.tick().await;
            if let Some(text) = read_pasteboard_changed(&self.last_hash) {
                if text.len() <= MAX_BYTES {
                    let _ = self.ctrl_tx.send(CtrlPayload::ClipboardSet {
                        source: ClipSource::Mac,
                        content: ClipContent::Text { data: text },
                    });
                }
            }
        }
    }

    /// iOS → Mac: write to NSPasteboard via pbcopy.
    pub fn write_remote(&self, content: &ClipContent) {
        match content {
            ClipContent::Text { data } => {
                let _ = pbcopy(data.as_bytes());
                // 写入后更新 last_hash 防止立刻把刚写的反弹回 iOS
                self.last_hash.store(simple_hash(data) as i64, Ordering::SeqCst);
            }
        }
    }
}

fn read_pasteboard_changed(last_hash: &AtomicI64) -> Option<String> {
    let out = Command::new("pbpaste").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    if text.is_empty() { return None; }
    let hash = simple_hash(&text) as i64;
    let prev = last_hash.swap(hash, Ordering::SeqCst);
    if prev == hash { None } else { Some(text) }
}

fn simple_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn pbcopy(bytes: &[u8]) -> std::io::Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        std::io::Write::write_all(stdin, bytes)?;
    }
    child.wait()?;
    Ok(())
}
```

### session_router.rs 集成

handle ctrl `ClipboardSet { source: Ios, content }` → `clipboard_bridge.write_remote(&content)`，**不要**广播给其他 iOS（M4 仅一对 iOS，不存在多对端）。

### 步骤

1. 改 ctrl_msg.rs 加 ClipboardSet/ClipSource/ClipContent + canonical bytes 路径
2. 改 CtrlMessage.swift 同步
3. 写 clipboard_bridge.rs（约 80 行）
4. 改 ui.rs 启动时 `spawn(bridge.run_polling())` + 把 ClipboardBridge 实例注入 session_router
5. 改 session_router.rs handle ClipboardSet
6. cargo test / clippy / fmt
7. xcodebuild build 过
8. commit：`feat: add ClipboardBridge with NSPasteboard polling and bidirectional ctrl ClipboardSet`

---

## Task M4.3：iOS ClipboardPanel + ClipboardStore

**Files:**
- Create: `ios-app/MacIOSWorkspace/Clipboard/ClipboardStore.swift`
- Create: `ios-app/MacIOSWorkspace/Clipboard/ClipboardPanel.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（NavigationLink 入口）
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`（dispatch ClipboardSet 到 ClipboardStore）

### 设计

`ClipboardStore`（@Observable）维护：
- `history: [ClipEntry]`（最近 5 条接收，最新在前）
- `handleRemote(content)` → 写 UIPasteboard.general.string + 入 history
- `sendToMac(text)` → `glue.sendCtrl(.clipboardSet(source: .ios, content: .text(text)))`

`ClipboardPanel` UI：
- "发送到 Mac" TextField（默认拿 UIPasteboard.general.string 预填）+ 按钮
- "最近从 Mac 收到" 列表（5 条），每条点击 → 重新写 UIPasteboard

### 步骤

1. 写 ClipboardStore.swift + ClipboardPanel.swift
2. 改 PairedView 加 NavigationLink "剪贴板" → ClipboardPanel
3. 改 SessionStore（或 PairedView 内 init）创建 ClipboardStore + 把 ClipboardSet 路由过去
4. xcodebuild build 过
5. commit：`feat(ios-app): add ClipboardStore + ClipboardPanel for bidirectional sync`

---

## Task M4.4：iOS ComposeSheet（CLI 路径）

**Files:**
- Create: `ios-app/MacIOSWorkspace/Compose/ComposeSheet.swift`
- Modify: `ios-app/MacIOSWorkspace/Term/InputBar.swift`（右端加 ✏️ 按钮）
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift::SessionDetailView`（处理 ✏️ 弹 sheet）

### `ComposeSheet.swift`

完全按 [`ios-input-compose-design.md`](../specs/2026-04-30-ios-input-compose-design.md) §2.1 写：

```swift
import SwiftUI

struct ComposeSheet: View {
    @Binding var text: String
    let title: String
    let onSend: (String) -> Void
    let onCancel: () -> Void

    @FocusState private var editorFocused: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                TextEditor(text: $text)
                    .font(.system(.body, design: .monospaced))
                    .focused($editorFocused)
                    .padding(8)
                    .background(Color(uiColor: .systemBackground))
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") {
                        onCancel()
                        dismiss()
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Send") {
                        // 强制 commit IME 组字
                        UIApplication.shared.sendAction(
                            #selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil,
                        )
                        onSend(text)
                        text = ""
                        dismiss()
                    }
                    .disabled(text.isEmpty)
                }
            }
        }
        .onAppear { editorFocused = true }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}
```

### `InputBar.swift` 加 ✏️

```swift
// 在 quickKeys 行末追加：
Button(action: { onCompose() }) {
    Image(systemName: "square.and.pencil")
}
.buttonStyle(.bordered)
.controlSize(.small)
```

InputBar 接收新的 closure `onCompose: () -> Void`。

### SessionDetailView 集成

```swift
@State private var composeText: String = ""
@State private var presentingCompose: Bool = false

// body 里：
InputBar(
    text: $inputText,
    onSendText: { ... },
    onKey: { ... },
    onCompose: { presentingCompose = true },
)
.sheet(isPresented: $presentingCompose) {
    ComposeSheet(
        text: $composeText,
        title: "Compose · \(label)",
        onSend: { sent in
            Task { await store.sendInput(sid: sid, text: sent) }
        },
        onCancel: {
            // text 已在 sheet 内 dismiss 时清空，无需操作
        },
    )
}
```

### 单元测试（XCTest 内）

`ComposeSheetTests.swift`（约 40 行）：
- 注入 text "hello" → 模拟 Send → 断言 onSend 收到精确 "hello"
- Cancel → 不触发 onSend
- 含中文 + emoji + 嵌入 `\n` → onSend 收到原样

可用 `@StateObject` 包裹 mock 闭包；不需要 ViewInspector，直接构造 ComposeSheet 调 onSend 即可。

### 步骤

1. 写 ComposeSheet.swift
2. 改 InputBar.swift 加 ✏️ 按钮
3. 改 SessionDetailView 接 sheet
4. 加 ComposeSheetTests.swift（3 条单测）
5. xcodebuild test 过（PairKeysTests 4 + 默认 + ComposeSheetTests 3）
6. commit：`feat(ios-app): add ComposeSheet for multi-line + IME + voice input on CLI path`

> **限制**：M4 只接 CLI 路径（onSend → `Input { sid, TerminalInput::Text }`）。GUI 路径（onSend → `paste_text`）等 M5/M6 再接 InputInjector。

---

## Task M4.5：`macagent notify` 子命令

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/socket_proto.rs`（加 NotifyRegister/Complete + NotifyAck）
- Modify: `mac-agent/crates/macagent-app/src/main.rs`（clap 子命令）
- Create: `mac-agent/crates/macagent-app/src/notify/mod.rs`

### `notify/mod.rs` 关键逻辑（约 100 行）

参考 plan §M4.4 旧版骨架：clap Args / fork+exec+wait / Unix socket NotifyRegister + NotifyComplete / 失败兜底（agent socket 不可达时仍执行命令）。

### 步骤

1. 改 socket_proto.rs
2. 改 main.rs clap dispatch 加 Notify
3. 写 notify/mod.rs
4. cargo build / test 全过
5. 手测：`cargo run -p macagent-app -- notify -- echo hello`，agent 没起 → 应见清晰 warning + echo 输出 + exit 0
6. commit：`feat(mac-agent): add `macagent notify` subcommand for command completion notifications`

---

## Task M4.6：NotifyEngine + PushClient + 正则 watcher

**Files:**
- Create: `mac-agent/crates/macagent-app/src/notify_engine.rs`
- Create: `mac-agent/crates/macagent-app/src/push_client.rs`
- Modify: `mac-agent/crates/macagent-app/src/agent_socket.rs`（处理 NotifyRegister/Complete）
- Modify: `mac-agent/crates/macagent-app/src/session_router.rs`（dispatch WatchSession 等 + 监听 TermDelta 跑 regex）
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（加 watcher 相关 case）

### `push_client.rs`（约 60 行）

参考旧版骨架。HMAC 签名调 Worker `/push`。

### `notify_engine.rs`（约 200 行）

```rust
pub struct NotifyEngine {
    push_client: Arc<PushClient>,
    in_flight: Arc<Mutex<HashMap<String /*register_id*/, NotifyEntry>>>,
    watchers: Arc<RwLock<HashMap<String /*sid*/, Vec<Watcher>>>>,
}

struct Watcher {
    id: String,
    regex: Regex,
    name: String,
    hits: u32,
    last_match: Option<String>,
}

impl NotifyEngine {
    pub async fn register_notify(&self, p2a: NotifyRegister) { /* push to in_flight */ }
    pub async fn complete_notify(&self, p2a: NotifyComplete) {
        if let Some(entry) = self.in_flight.lock().await.remove(&p2a.register_id) {
            let title = entry.title.clone().unwrap_or_else(|| entry.argv[0].clone());
            let duration = format_duration(p2a.ended_at_ms - entry.started_at_ms);
            let body = format!("exit {} in {}", p2a.exit_code, duration);
            let deeplink = entry.session_hint.as_ref().map(|sid| format!("macagent://session/{}", sid));
            self.push_client.send(&title, &body, deeplink.as_deref(), entry.session_hint.as_deref()).await.ok();
        }
    }

    pub async fn add_watcher(&self, sid: String, watcher_id: String, regex: String, name: String) -> Result<()> { /* compile regex */ }
    pub async fn remove_watcher(&self, sid: &str, watcher_id: &str) {}
    pub async fn list_watchers(&self, sid: &str) -> Vec<WatcherInfo> {}

    /// 调用方式：session_router 在每条 producer TermDelta（含新增的 line.runs.text）后，
    /// 把每行的纯文本喂进来。
    pub async fn feed_session_line(&self, sid: &str, line_text: &str, ctrl_tx: &mpsc::UnboundedSender<CtrlPayload>) {
        let mut watchers = self.watchers.write().await;
        if let Some(list) = watchers.get_mut(sid) {
            for w in list.iter_mut() {
                if w.regex.is_match(line_text) {
                    w.hits += 1;
                    w.last_match = Some(line_text.to_string());
                    let _ = self.push_client.send(
                        &format!("{} matched", w.name),
                        line_text,
                        Some(&format!("macagent://session/{}", sid)),
                        Some(sid),
                    ).await;
                    let _ = ctrl_tx.send(CtrlPayload::WatcherMatched {
                        sid: sid.to_string(),
                        watcher_id: w.id.clone(),
                        line_text: line_text.to_string(),
                    });
                }
            }
        }
    }
}
```

### session_router.rs 集成

handle ctrl `WatchSession { sid, watcher_id, regex, name }` → `notify_engine.add_watcher(...)` → ctrl 推 `WatchersList { sid, watchers }`。

producer TermDelta 进来时（每条变化的 line），调 `notify_engine.feed_session_line(sid, line.runs.into_iter().map(|r|r.text).join(""), &ctrl_tx)`。

### Tests

`notify_engine` 单测（约 100 行）：
- add/remove/list watcher
- feed_session_line 命中 regex 触发 push（mock push_client）
- complete_notify 计算 duration + 调 push

### 步骤

1. 改 ctrl_msg.rs 加 watcher 相关 case + iOS 同步
2. 写 push_client.rs + 单测
3. 写 notify_engine.rs + 单测
4. 改 agent_socket.rs handle NotifyRegister/Complete
5. 改 session_router.rs：watcher ctrl + TermDelta feed
6. cargo test 全过
7. commit：`feat(mac-agent): add NotifyEngine (regex watchers + notify completion) and push_client`

---

## Task M4.7：iOS APNs PushHandler + WatchersView

**Files:**
- Modify: `ios-app/MacIOSWorkspace.entitlements`（如不存在则在 pbxproj 加）
- Create: `ios-app/MacIOSWorkspace/Notify/PushHandler.swift`
- Modify: `ios-app/MacIOSWorkspace/MacIOSWorkspaceApp.swift`（UIApplicationDelegateAdaptor）
- Create: `ios-app/MacIOSWorkspace/Notify/WatcherStore.swift`
- Create: `ios-app/MacIOSWorkspace/Notify/WatchersView.swift`
- Modify: `ios-app/MacIOSWorkspace/SessionListView.swift::SessionDetailView`（NavigationLink "正则提醒"）

### Entitlement & Capability

`.entitlements` 加 `aps-environment = development`（真机 release 改 production）。Info.plist 不需要 push 单独 key。**Xcode 里加 Push Notifications Capability** 用户介入项。

Apple Developer Portal 用户需要：
- 给 App ID 启用 Push Notifications
- 创建 APNs Auth Key（.p8）→ Key ID + Team ID

### PushHandler

参考 旧版骨架 + WatcherStore（@Observable，订阅 ctrl `WatchersList` / `WatcherMatched`）+ WatchersView UI（list + add regex form）。

### 步骤

1. 加 entitlement + Capability
2. 写 PushHandler.swift + WatcherStore.swift + WatchersView.swift
3. 改 MacIOSWorkspaceApp.swift 加 UIApplicationDelegateAdaptor
4. 改 SessionDetailView 加 NavigationLink "正则提醒" → WatchersView
5. xcodebuild build 过（不带 push 真测）
6. commit：`feat(ios-app): add PushHandler (APNs entitlement + token register) and watchers UI`

---

## Task M4.8：真机端到端

需要：
1. APNs Auth Key（.p8 / Key ID / Team ID）
2. wrangler secret 配齐 + deploy
3. iPhone 重新配对让 Worker 拿到 ios_apns_token

### 验收（5 类）

1. **clipboard auto sync**：Mac `pbcopy "hello"` → iPhone Clipboard 入口看到 "hello" + UIPasteboard.string == "hello"
2. **clipboard reverse**：iPhone "发送到 Mac" "world" → Mac 终端 `pbpaste` 输出 "world"
3. **ComposeSheet IME**：iPhone 装微信键盘 → SessionDetailView ✏️ → 切微信键盘 → 按 🎤 说"git status" → Send → 终端收到并执行
4. **ComposeSheet 中文**：系统拼音键盘 → ComposeSheet 输入"你好世界" → Send → PTY 收到 6 字节正确 UTF-8
5. **notify**：Mac 终端 `cargo run -p macagent-app -- notify -- sleep 5; echo done` → iPhone 5 秒后收到推送 "done | exit 0 in 5s"
6. **watcher**：iOS 设 watcher `regex:"error.*"` → producer 输出 "error: file not found" → iPhone 推送 + WatchersView 实时显示
7. **deep-link**：点击推送 → iOS app 自动跳到对应 session 的 SessionDetailView

### 步骤

无 commit；纯人工验证。结果记录到 final review。

---

## Task M4.9：M4 final review

dispatch reviewer subagent，按 M3 final review 模式审 commits `f0464d5..HEAD`。

---

## M4 验收清单

- [ ] worker npm test 全绿（27 + 5 = 32）
- [ ] mac-agent cargo test --workspace 全绿
- [ ] ios-app xcodebuild test 全绿（含 ComposeSheetTests 3 条）
- [ ] CI 三条 workflow 全绿
- [ ] 真机：剪贴板双向 ✓
- [ ] 真机：notify 推送 ✓
- [ ] 真机：watcher regex 推送 ✓
- [ ] 真机：deep-link ✓
- [ ] 真机：微信键盘语音 → CLI ✓
- [ ] 真机：中文 IME → CLI ✓

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：主 spec §3.1 ClipboardBridge / NotifyEngine、§3.2 ClipboardPanel / ComposeSheet / PushHandler、§3.3 /push、§4.5 / §4.6 / §7 M4 → 全部映射；独立 spec ios-input-compose-design.md §2 / §3 → ComposeSheet (M4.4) 落地。
2. **占位符扫描**：M4.7 entitlement 添加 + Apple Developer Portal 操作需用户介入（已显式标注）；其余无 TBD/TODO。
3. **类型一致性**：CtrlPayload Mac/iOS 同步；ComposeSheet 复用 M3.1 已锁定的 `Input { TerminalInput::Text }`，无新协议。
4. **范围一致**：M4 ComposeSheet 仅 CLI 路径；GUI 路径在 M5/M6 接 InputInjector.paste_text（与独立 spec §6 milestone 一致）。
5. **风险**：
   - APNs Auth Key 需 Apple Developer 账号；TestFlight 真机验证更稳
   - NSPasteboard 用 pbpaste/pbcopy 子进程：足够文本同步
   - watcher regex 性能：可接受
   - ComposeSheet 中 IME 组字未提交时点 Send：通过 `resignFirstResponder` 强制 commit；UIKit 默认行为兜底

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven**（推荐）——延续 M0-M3 节奏；M4.0-M4.6 自动化高；M4.7 entitlement + APNs 需用户介入；M4.8 真机
2. **Inline Execution**

请用户选 1 或 2。
