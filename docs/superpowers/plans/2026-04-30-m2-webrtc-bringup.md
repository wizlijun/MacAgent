# M2 · WebRTC 媒体面 bring-up 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans。

**Goal:** 在 M1 已建立的 pair + 签名 ctrl 通道之上，让 Mac 与 iOS 通过 WebRTC 建立 PeerConnection，跑通一条名为 `ctrl` 的 DataChannel 上的签名心跳；跨 NAT 时回退到 Cloudflare Calls TURN。

**Architecture:**
- **WebSocket 信令复用**：M1 已有的 `WS /signal/:pair_id` 不变；M2 在它上面承载 SDP/ICE 帧（端到端 HMAC，Worker 不解读）。
- **Mac 端**：在 `macagent-core` 加 `rtc_peer` 模块，封装 `webrtc-rs`（`webrtc` crate）的单 PeerConnection；暴露"send_offer / on_answer / on_ice / open_ctrl_channel"四类 API；与 `SignalingClient` 由 `macagent-app` 黏合。
- **iOS 端**：通过 SwiftPM 引入 `stasel/WebRTC` 二进制包（Google 编译产物，社区维护的 SPM mirror），新增 `RtcClient` actor 持有 `RTCPeerConnection`。
- **Worker 端**：新加 `POST /turn/cred`，调 Cloudflare Calls API 拿短期 TURN 凭证（1h TTL），双方在过期前 5 分钟 prefetch。
- **心跳协议**：Mac 与 iOS 任一方主动发 `ctrl: {type:"hb", ts, nonce, sig}`，对端验签后回 `{type:"hb_ack", ...}`，间隔 10s。无回应 30s 触发 ICE restart。

**Tech Stack（M2 新增）:**
- Mac (Rust)：`webrtc = "0.11"`（webrtc-rs，纯 Rust）、`bytes`、`async-trait`。
- iOS (Swift)：`https://github.com/stasel/WebRTC`（SwiftPM dependency，pin 到 `137.0.0`）、约 70MB 二进制 framework。
- Worker (TS)：用 `fetch` 调 Cloudflare Calls API（`https://rtc.live.cloudflare.com/v1/turn/keys/<key_id>/credentials/generate`）。
- TURN/STUN：Cloudflare Calls，需要用户提供 `CF_CALLS_KEY_ID` 与 `CF_CALLS_KEY_API_TOKEN`（在 Cloudflare dashboard 创建一次性，已在 spec §3.3 列为 secret）。

**M1 debt 一并清理**（在合适的 task 中夹带）：
- I1：两端 Revoke 按钮调 Worker `/pair/revoke`（M2.0 单独 task）
- I2：清理死依赖（`thiserror`、`security-framework` 在 macagent-core 中暂未用→保留；`url`、`uuid` 在 macagent-app 中检查；`tray-icon`、`tao` 既然 M1.6 已退出 macagent-app 依赖，workspace 级也可剥）

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 RtcPeer、§3.2 RtcClient、§3.3 `/turn/cred`、§4.2 后续连接、§5 ICE/DTLS 失败、§7 M2 行。

---

## 协议 / 数据契约

### `POST /turn/cred`（M2.1）

**请求**：HMAC 签名（与 `/pair/revoke` 同模式）
```jsonc
{ "pair_id": "<uuid>", "ts": 1735200000000, "sig": "<base64>" }
// sig = HMAC-SHA256(device_secret, "turn-cred|<pair_id>|<ts>")
```

**响应**：
```jsonc
{
  "ice_servers": [
    { "urls": ["stun:stun.cloudflare.com:3478"] },
    {
      "urls": ["turn:turn.cloudflare.com:3478?transport=udp"],
      "username": "<short-lived>",
      "credential": "<short-lived>"
    }
  ],
  "expires_at": 1735203600000
}
```

错误码：`unknown_pair` 404、`bad_sig` 401、`ts_out_of_range` 400、`turn_unavailable` 503（Cloudflare API 5xx 透传）。

### Ctrl DataChannel 帧（M2.4 / M2.5）

继续用 M1 的 `SignedCtrl` 包络（payload + sig，HMAC E2E）。M2 新增两个 type：

```jsonc
{ "type": "hb",     "ts": 1735200000000, "nonce": "<base64>", "sig": "<...>" }
{ "type": "hb_ack", "ts": 1735200000050, "nonce": "<echo>",  "sig": "<...>" }
```

复用 M1 的 `CtrlPayload` enum（Mac/iOS 各自加变体）+ `canonical_bytes` + HMAC 签名校验。

### SDP/ICE 中继帧（在 WS 上）

Worker DO 不解读，原样转发字符串帧。Mac/iOS 双方约定：

```jsonc
{ "kind": "sdp",     "side": "offer" | "answer", "sdp": "<rfc 8866 string>" }
{ "kind": "ice",     "candidate": "<rfc 8839 string>", "mid": "0", "mline_index": 0 }
{ "kind": "restart", "reason": "<str>" }
```

帧本身**不**HMAC 签名：M1 WS 握手已用 `device_secret` 认证当前连接归属，Worker DO 只把帧从 mac→ios 或反向。E2E 加密由 WebRTC 自身的 DTLS-SRTP 完成，SDP/ICE 内容公开化无所谓。

---

## 文件结构（增量）

```
worker/src/
├── turn.ts                       ← 新：handlePostTurnCred + Cloudflare Calls API 客户端
├── pair.ts                       ← 改：reuse hmacVerify 校验 turn-cred 签名（与 revoke 同模式）
└── index.ts                      ← 改：路由 /turn/cred

worker/test/
└── turn.test.ts                  ← 新：mock Cloudflare Calls fetch，测签名校验 + 转发响应

mac-agent/crates/macagent-core/src/
├── rtc_peer.rs                   ← 新：RtcPeer 单 PeerConnection 封装
└── lib.rs                        ← 改：pub mod rtc_peer

mac-agent/crates/macagent-core/tests/
└── rtc_peer_test.rs              ← 新：RtcPeer 创建 + offer 生成 + DataChannel state

mac-agent/crates/macagent-app/src/
├── ui.rs                         ← 改：Paired 状态加 "Connect (M2)" 按钮，启 RtcPeer
├── rtc_glue.rs                   ← 新：把 SignalingClient ↔ RtcPeer 缝在一起（async）
└── main.rs                       ← 改：tokio runtime 启动 rtc_glue

ios-app/MacIOSWorkspace/Rtc/
├── RtcClient.swift               ← 新：actor 持有 RTCPeerConnection、ctrl DataChannel
└── RtcGlue.swift                 ← 新：SignalingClient ↔ RtcClient SDP/ICE 桥接
ios-app/MacIOSWorkspace/PairedView.swift  ← 改：加 "Connect (M2)" 按钮 + 心跳 UI
ios-app/MacIOSWorkspace.xcodeproj/project.pbxproj  ← 改：加 SwiftPM dependency stasel/WebRTC

docs/                              ← 不动
```

---

## Task M2.0：M1 debt 清理 — 客户端 Revoke 调 Worker

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（revoke 按钮 → reqwest POST）
- Modify: `ios-app/MacIOSWorkspace/PairStore.swift`（revoke → URLSession POST，签名校验）

### Mac 改动

`ui.rs` 的 `revoke_pair_record()`（或类似函数）：在清 Keychain 之前先 spawn 一个 task 调 Worker `/pair/revoke`：

```rust
async fn worker_revoke(worker_url: &str, pair_id: &str, mac_device_secret_b64: &str) -> Result<()> {
    let ts: u64 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let secret = base64::decode(mac_device_secret_b64)?;
    let sig = base64::encode(hmac_sign(&secret, format!("revoke|{pair_id}|{ts}").as_bytes()));
    reqwest::Client::new()
        .post(format!("{worker_url}/pair/revoke"))
        .json(&serde_json::json!({ "pair_id": pair_id, "ts": ts, "sig": sig }))
        .send().await?
        .error_for_status()?;
    Ok(())
}
```

调用时即使 Worker 不可达也清本地 Keychain（best-effort revoke）。在 UI 显示一行"已撤销"，错误时显示"已本地撤销，远端未达"。

### iOS 改动

`PairStore.revoke()` 改为 `async throws`：

```swift
func revoke() async throws {
    if case let .paired(pair) = state {
        let ts = UInt64(Date().timeIntervalSince1970 * 1000)
        guard let secret = Data(base64Encoded: pair.deviceSecretB64) else { /* throw */ }
        let sig = PairKeys.hmacSign(secret: secret, message: Data("revoke|\(pair.pairId)|\(ts)".utf8)).base64EncodedString()
        var req = URLRequest(url: URL(string: "\(pair.workerURL)/pair/revoke")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "pair_id": pair.pairId, "ts": ts, "sig": sig,
        ])
        // best-effort：Worker 不可达不阻塞本地清理
        _ = try? await URLSession.shared.data(for: req)
    }
    try Keychain.delete("ios.pair.record")
    try Keychain.delete("ios.local.privkey")
    state = .unpaired
}
```

`PairedView` 的撤销按钮改为 `Task { try? await store.revoke() }`。

### 步骤

1. 改 ui.rs 加 worker_revoke 函数 + 在 revoke_pair_record 调用
2. 改 PairStore.revoke 为 async + 真发 POST
3. 改 PairedView 的撤销按钮回调
4. `cargo build -p macagent-app` + `cargo clippy -- -D warnings` + `cargo fmt`
5. iOS `xcodebuild build` 验证编译
6. commit：`fix: client revoke now calls Worker /pair/revoke`

---

## Task M2.1：Worker `POST /turn/cred`

**Files:**
- Create: `worker/src/turn.ts`
- Modify: `worker/src/index.ts`、`worker/src/env.ts`（加 `CF_CALLS_KEY_ID`、`CF_CALLS_KEY_API_TOKEN` secret 类型）
- Create: `worker/test/turn.test.ts`

### `worker/src/env.ts` 扩展

```typescript
export interface Env {
  SIGNALING_ROOM: DurableObjectNamespace;
  PAIRS: KVNamespace;
  // M2 新增（部署前 wrangler secret put）：
  CF_CALLS_KEY_ID?: string;
  CF_CALLS_KEY_API_TOKEN?: string;
}
```

### `worker/src/turn.ts`

```typescript
import type { Env } from "./env";
import { b64decode, hmacVerify } from "./crypto";
import { getPair } from "./kv";

interface CallsCredResp {
  iceServers: Array<{ urls: string | string[]; username?: string; credential?: string }>;
}

export async function handleTurnCred(req: Request, env: Env): Promise<Response> {
  let body: { pair_id?: string; ts?: number; sig?: string };
  try { body = await req.json(); } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (!body.pair_id || typeof body.ts !== "number" || !body.sig) {
    return Response.json({ error: "missing_fields" }, { status: 400 });
  }
  if (Math.abs(Date.now() - body.ts) > 60_000) {
    return Response.json({ error: "ts_out_of_range" }, { status: 400 });
  }

  const pair = await getPair(env, body.pair_id);
  if (!pair) {
    return Response.json({ error: "unknown_pair" }, { status: 404 });
  }

  const msg = `turn-cred|${body.pair_id}|${body.ts}`;
  const sigBytes = b64decode(body.sig);
  const macOk = await hmacVerify(b64decode(pair.mac_device_secret_b64), msg, sigBytes);
  const iosOk = !macOk && await hmacVerify(b64decode(pair.ios_device_secret_b64), msg, sigBytes);
  if (!macOk && !iosOk) {
    return Response.json({ error: "bad_sig" }, { status: 401 });
  }

  if (!env.CF_CALLS_KEY_ID || !env.CF_CALLS_KEY_API_TOKEN) {
    return Response.json({ error: "turn_not_configured" }, { status: 503 });
  }

  // 调 Cloudflare Calls 拿短期凭证
  const ttlSec = 3600;
  const callsRes = await fetch(
    `https://rtc.live.cloudflare.com/v1/turn/keys/${env.CF_CALLS_KEY_ID}/credentials/generate`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.CF_CALLS_KEY_API_TOKEN}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ ttl: ttlSec }),
    },
  );
  if (!callsRes.ok) {
    return Response.json({ error: "turn_unavailable", status: callsRes.status }, { status: 503 });
  }
  const cred = (await callsRes.json()) as CallsCredResp;
  return Response.json({
    ice_servers: cred.iceServers,
    expires_at: Date.now() + ttlSec * 1000,
  });
}
```

### `index.ts` 路由

```typescript
if (url.pathname === "/turn/cred" && request.method === "POST") {
  return handleTurnCred(request, env);
}
```

### `worker/test/turn.test.ts`

用 vitest 的 `vi.spyOn(globalThis, "fetch")` mock Cloudflare Calls 响应，验证：
- 200 + 转发 ice_servers
- 401 bad_sig
- 400 ts skew
- 404 unknown_pair
- 503 当 secret 未配置 / Cloudflare 5xx

测试 boilerplate 参考 `pair.test.ts` 的 setupPair helper。

### 步骤

1. 写 turn.test.ts（5 条测试）→ 跑测试期望红
2. 写 turn.ts + 路由
3. `npm test 2>&1 | tail` 应 27/27（22 + 5）
4. `npm run typecheck`
5. commit：`feat(worker): add POST /turn/cred with Cloudflare Calls integration`

---

## Task M2.2：Mac `RtcPeer` 模块（webrtc-rs 单 PeerConnection 封装）

**Files:**
- Modify: `mac-agent/Cargo.toml`（workspace deps 加 `webrtc = "0.11"`）
- Modify: `mac-agent/crates/macagent-core/Cargo.toml`
- Create: `mac-agent/crates/macagent-core/src/rtc_peer.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs`
- Create: `mac-agent/crates/macagent-core/tests/rtc_peer_test.rs`

### 接口

```rust
//! macagent-core::rtc_peer
//!
//! 单 PeerConnection 封装；M2 仅暴露：
//! - new(ice_servers) → 建 RTCPeerConnection
//! - create_offer() → SDP 字符串
//! - apply_remote_answer(sdp)
//! - on_local_candidate(cb)
//! - apply_remote_candidate(json)
//! - open_ctrl_channel() → DataChannel handle
//! - state() → 连通性 enum
//! - close()

pub struct RtcPeer { /* webrtc::peer_connection::RTCPeerConnection 包装 */ }

pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeerState { New, Connecting, Connected, Disconnected, Failed, Closed }

pub struct CtrlChannel { /* RTCDataChannel 包装 */ }

impl RtcPeer {
    pub async fn new(ice: Vec<IceServer>) -> Result<Self>;
    pub async fn create_offer(&self) -> Result<String>;
    pub async fn apply_remote_answer(&self, sdp: &str) -> Result<()>;
    pub async fn apply_remote_candidate(&self, candidate_json: &str) -> Result<()>;
    pub async fn on_local_candidate(&self, cb: impl Fn(String) + Send + Sync + 'static);
    pub async fn open_ctrl_channel(&self) -> Result<CtrlChannel>;
    pub async fn state(&self) -> PeerState;
    pub async fn close(&self) -> Result<()>;
}

impl CtrlChannel {
    pub async fn send_text(&self, msg: &str) -> Result<()>;
    pub async fn on_message(&self, cb: impl Fn(String) + Send + Sync + 'static);
}
```

### Tests

```rust
// rtc_peer_test.rs
#[tokio::test(flavor = "current_thread")]
async fn create_peer_with_no_ice_servers_yields_offer() {
    let peer = RtcPeer::new(vec![]).await.unwrap();
    let _ch = peer.open_ctrl_channel().await.unwrap(); // 必须先开 channel 再 createOffer 否则 SDP 没 m=
    let offer = peer.create_offer().await.unwrap();
    assert!(offer.contains("v=0"));
    assert!(offer.contains("a=group:BUNDLE"));
    peer.close().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn loopback_two_peers_exchange_sdp_and_ice() {
    // 起 alice + bob 两个 RtcPeer，跑完整 offer/answer + ICE 互相喂
    // 验证 connected
    // 在 ctrl channel 上发一条字符串、收到
    // 这是非平凡的集成测试，约 80 行
}
```

### 实现

`webrtc` crate 的标准用法。骨架：

```rust
use anyhow::{Context, Result};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
// ... 详细见 webrtc-rs docs.rs

impl RtcPeer {
    pub async fn new(ice: Vec<IceServer>) -> Result<Self> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = webrtc::interceptor::registry::Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        let cfg = RTCConfiguration {
            ice_servers: ice.into_iter().map(|s| RTCIceServer {
                urls: s.urls,
                username: s.username.unwrap_or_default(),
                credential: s.credential.unwrap_or_default(),
                ..Default::default()
            }).collect(),
            ..Default::default()
        };
        let pc = api.new_peer_connection(cfg).await?;
        Ok(Self { pc: Arc::new(pc) })
    }
    // ...其他方法
}
```

### 步骤

1. 加 `webrtc = "0.11"` 到 workspace deps，引入到 macagent-core
2. 写 `rtc_peer.rs` （约 200-300 行）
3. 写 `tests/rtc_peer_test.rs`（约 100-150 行 loopback 测试）
4. `cargo test -p macagent-core 2>&1 | tail -10` 应 9/9 (4 pair + 2 signaling + 2 rtc + 1 lib)
5. `cargo clippy -p macagent-core --all-targets -- -D warnings`
6. `cargo fmt --all`
7. commit：`feat(mac-agent): add RtcPeer (webrtc-rs single PeerConnection wrapper)`

---

## Task M2.3：Mac `rtc_glue` — SignalingClient ↔ RtcPeer 桥接

**Files:**
- Create: `mac-agent/crates/macagent-app/src/rtc_glue.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（Paired 状态加 "Connect" 按钮）

### 设计

```rust
//! 把 WS 上的 {kind: sdp/ice} 帧 ↔ RtcPeer 方法调用桥接。

pub async fn run_glue(
    pair: PairRecord,
    local_keys: PairAuth,
    ice_servers: Vec<IceServer>,
    on_state: impl Fn(GlueState) + Send + Sync + 'static,
) -> Result<()> {
    // 1. 拿 turn cred (POST /turn/cred 签名)
    // 2. 建 SignalingClient WS 到 /signal/<pair_id>?device=mac&...
    // 3. 建 RtcPeer with ice_servers
    // 4. 创建 ctrl_channel（DataChannel）
    // 5. peer.on_local_candidate → 包成 {kind:"ice", ...} 经 WS 发出
    // 6. WS 上收到 {kind:"sdp", side:"answer"} → peer.apply_remote_answer
    // 7. WS 上收到 {kind:"ice"} → peer.apply_remote_candidate
    // 8. peer.create_offer → 经 WS 发 {kind:"sdp", side:"offer"}
    // 9. ctrl_channel.on_message → 调用回调（M2.6 的心跳处理）
}
```

### 步骤

1. 写 rtc_glue.rs，约 200 行
2. 改 ui.rs 在 PairState::Paired 视图加 "Connect" 按钮，按下时 `tokio::spawn(run_glue(...))` 把 GlueState 反馈到 UI
3. 不写单测（async + 真 WS 困难），靠 M2.8 真机验证
4. cargo test / clippy / fmt 仍过
5. commit：`feat(mac-agent): add rtc_glue connecting SignalingClient and RtcPeer`

---

## Task M2.4：iOS GoogleWebRTC SwiftPM + RtcClient

**Files:**
- Modify: `ios-app/MacIOSWorkspace.xcodeproj/project.pbxproj`（加 SPM dep `https://github.com/stasel/WebRTC`，pin 到 `137.0.0`）
- Create: `ios-app/MacIOSWorkspace/RtcClient.swift`
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（加 "Connect" 按钮）

### SwiftPM 集成

Xcode → File → Add Package Dependencies → 输 `https://github.com/stasel/WebRTC` → Up to Next Major `137.0.0`。

> 这一步**用户介入**：implementer 不能用 Xcode GUI，必须用户在 Xcode 里手动添加，或 implementer 改 pbxproj 文件加 `XCRemoteSwiftPackageReference` 与 `XCSwiftPackageProductDependency` 节点（约 30 行 pbxproj 改动）。

### `RtcClient.swift`

```swift
import WebRTC
import Foundation

actor RtcClient {
    private let factory: RTCPeerConnectionFactory
    private let pc: RTCPeerConnection
    private var ctrlChannel: RTCDataChannel?

    init(iceServers: [RTCIceServer]) {
        self.factory = RTCPeerConnectionFactory()
        let cfg = RTCConfiguration()
        cfg.iceServers = iceServers
        cfg.sdpSemantics = .unifiedPlan
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        self.pc = factory.peerConnection(with: cfg, constraints: constraints, delegate: nil)!
    }

    func openCtrlChannel() -> RTCDataChannel? {
        let cfg = RTCDataChannelConfiguration()
        cfg.isOrdered = true
        ctrlChannel = pc.dataChannel(forLabel: "ctrl", configuration: cfg)
        return ctrlChannel
    }

    func createOffer() async throws -> String {
        let offer = try await pc.offer(for: .init(mandatoryConstraints: nil, optionalConstraints: nil))
        try await pc.setLocalDescription(offer)
        return offer.sdp
    }

    func applyRemoteAnswer(_ sdp: String) async throws {
        let desc = RTCSessionDescription(type: .answer, sdp: sdp)
        try await pc.setRemoteDescription(desc)
    }

    func applyRemoteCandidate(_ candidateJson: String) async throws {
        // 解析 json -> RTCIceCandidate
        // try await pc.add(candidate)
    }

    func sendCtrl(_ json: String) async throws {
        let data = json.data(using: .utf8)!
        ctrlChannel?.sendData(RTCDataBuffer(data: data, isBinary: false))
    }

    func close() {
        pc.close()
    }
}
```

### `PairedView.swift` 加按钮

类似 ping 测试按钮，启动 RtcGlue。

### 步骤

1. SPM 集成（**用户**或 implementer 改 pbxproj）
2. 写 RtcClient.swift
3. 写 RtcGlue.swift（与 Mac rtc_glue 对应；调 SignalingClient + RtcClient）
4. 改 PairedView 加 "Connect" 按钮 + 心跳 UI
5. xcodebuild build 验证编译
6. commit：`feat(ios-app): integrate stasel/WebRTC and add RtcClient + RtcGlue`

---

## Task M2.5：心跳协议 + ICE restart

**Files:**
- Modify: macagent-core/src/ctrl_msg.rs（CtrlPayload 加 hb/hb_ack）
- Modify: ios-app/MacIOSWorkspace/CtrlMessage.swift（同步加 hb/hb_ack）
- Modify: rtc_glue.rs / RtcGlue.swift 的心跳逻辑

### 行为

- 一旦 PeerConnection state == Connected：启动一个每 10s 发 hb 的 task
- 收到对端 hb → 验签 + 回 hb_ack
- 收到 hb_ack → reset miss counter
- 连续 3 次 miss（30s）→ 触发 ICE restart：`peer.restart_ice()` + 重发 offer 经 WS

### 步骤

1. 改 ctrl_msg.rs 加 Ping/Pong 之外的 hb/hb_ack 变体（或复用 ping 改名）
2. iOS 同步
3. rtc_glue 加 heartbeat task + ice_restart 逻辑
4. cargo / xcodebuild 编译过
5. commit：`feat: add ctrl heartbeat and ICE restart on missed beats`

---

## Task M2.6：真机端到端 WebRTC 联调

**Files:** 无新增。

### 准备

1. **创建 Cloudflare Calls 凭证**：
   - 用户登录 Cloudflare dashboard → Calls (RTC) → Create new key（Realtime TURN）
   - 复制 Key ID 与 API Token
2. **配置 Worker secret**：
   ```bash
   cd worker
   echo "<key_id>" | npx wrangler secret put CF_CALLS_KEY_ID
   echo "<api_token>" | npx wrangler secret put CF_CALLS_KEY_API_TOKEN
   npx wrangler deploy
   ```

### 联调步骤（用户）

1. Mac Agent 启 → 已有 paired 态 → 点 "Connect"
2. iPhone 真机 → PairedView → 点 "Connect"
3. 两端日志应见：
   - `POST /turn/cred 200`
   - WS `{kind:"sdp",side:"offer"}` mac → ios
   - WS `{kind:"sdp",side:"answer"}` ios → mac
   - 多条 `{kind:"ice",...}` 双向
   - `peer state: connected`
   - 心跳 hb/hb_ack 互相确认
4. **断网测试**：iPhone 切飞行模式 5 秒再开 → 应触发 ICE restart 自动重连，UI 显示"重连中..."然后回 Connected
5. **跨 NAT 测试**：iPhone 改用 4G 蜂窝（不在同 Wi-Fi）→ TURN 中继生效，仍能联通

### 验收

- 至少 1 次成功心跳来回
- 1 次主动断网后自动 ICE restart 成功
- Cloudflare Calls 控制台 `分钟数` 计数器开始走（说明 TURN 在用）

无 git commit；纯人工验证。

---

## Task M2.7：M1 debt I2 死依赖清理

**Files:**
- Modify: `mac-agent/Cargo.toml`（workspace deps 删 `tray-icon`、`tao` 如未引用）
- Modify: `mac-agent/crates/macagent-core/Cargo.toml`（删 `url`、`thiserror`、`security-framework` 如未引用）
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`（删 `uuid` 如未引用）

> 注：`security-framework` 在 macagent-core 当前未用，但 M3 PairAuth 持久化扩展可能用到；保留还是删除，**implementer 用 grep 实证决定**：未来里程碑明确需要 → 保留 + 加 `# kept for M3 PairAuth`；当前 + 短期内无用 → 删。

### 步骤

1. `grep -r 'use thiserror' mac-agent/crates/` 等命令找实际引用
2. 删未用 deps
3. `cargo build --workspace` 应仍过
4. commit：`chore(mac-agent): remove unused crate dependencies`

---

## Task M2.8：M2 Final review

dispatch reviewer subagent，按 M1 final review 同样模式审 commits `bfafd56..HEAD`。

---

## M2 验收清单

- [ ] `cd worker && npm test` 全绿（27/27）
- [ ] `cd mac-agent && cargo test --workspace` 全绿（含 rtc_peer loopback test）
- [ ] `cd ios-app && xcodebuild test ...` 全绿（SPM 集成不破单测）
- [ ] 真机：Mac ↔ iPhone WebRTC PeerConnection 建连成功（同 Wi-Fi + 4G 跨网各一次）
- [ ] 心跳（hb/hb_ack）每 10s 互通
- [ ] 主动断网 5s → ICE restart 自动恢复
- [ ] Cloudflare Calls TURN 控制台显示流量
- [ ] CI 三条 workflow 全绿

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：spec §3.1 RtcPeer / §3.2 RtcClient / §3.3 /turn/cred / §4.2 / §5 ICE 失败、TURN 过期、漫游 — 全部映射到 M2.1-M2.6。
2. **占位符扫描**：仅 SPM 添加可能需用户介入（M2.4 顶部明确说明）；其余无 TBD/TODO。
3. **类型一致性**：`SignedCtrl` Mac/iOS/Worker 三方继续 M1 模型；新增 hb/hb_ack 在 ctrl_msg.rs / CtrlMessage.swift 同时加。`/turn/cred` 请求体三方一致（pair_id+ts+sig）。`{kind:"sdp"|"ice"|"restart"}` 信令帧 Mac/iOS 双方约定一致（不签名）。
4. **范围检查**：M2 = WebRTC 建连 + 心跳；**不**含 PTY（M3）、视频流（M5）、剪贴板（M4）、输入注入（M6）。
5. **风险**：
   - Cloudflare Calls 是相对新的服务（2024 GA），API 偶尔变；M2.1 mock 测试只覆盖签名+转发，真链路烟测靠 M2.6
   - webrtc-rs 0.11 与 GoogleWebRTC 137 互通：业界已知存在微小 SDP 差异，必要时 implementer 给 SDP munging 层（mask）
   - SPM 添加 stasel/WebRTC ~70MB，构建时间显著增加（首次 +5 分钟），CI 缓存策略可能要调

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven** —— M2.0/M2.1/M2.7 短任务高度自动化；M2.2/M2.3 较重，可能需要 fixup；M2.4 含用户介入（SPM）；M2.5/M2.6 真机
2. **Inline** —— 顺序顶上，关键节点 checkpoint

请用户选 1 或 2 后再开始执行。
