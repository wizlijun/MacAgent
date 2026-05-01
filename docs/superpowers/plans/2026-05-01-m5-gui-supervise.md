# M5 · GUI 监管 v0（view-only）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）。

**Goal:** iOS 上从 Mac 列表里挑一个已开窗口（Chrome / Cursor / 任意可见窗口）→ 在 iPhone / iPad 上实时看到该窗口的 30 fps 视频流。**只看不点**——M5 不接输入注入（M6）、不支持 launch 启动 App（M7）、不支持 fit_window 等比缩放（M7）。

**Architecture:**
- **Mac 端**：用 `screencapturekit` crate（ScreenCaptureKit 的 Rust 绑定）抓单窗口；VideoToolbox（通过 objc2-video-toolbox / core-foundation FFI）做 H.264 硬件编码；NAL 单元喂进 webrtc-rs `TrackLocalStaticSample` → 走 PeerConnection video track → DTLS-SRTP 加密推到 iOS。
- **iOS 端**：`RTCPeerConnection` 自动路由 incoming video track 到 `RTCMTLVideoView`；SwiftUI 包一层 `UIViewRepresentable`。
- **协议**：复用 M2 的 SignedCtrl HMAC E2E；CtrlPayload 加 ~7 个新变体（WindowsList / SuperviseExisting / SupervisedAck / RemoveSupervised / SupervisionList / StreamEnded / ViewportChanged 占位）。
- **Worker 不动**。
- **媒体平面**：M2 已经建好的 RtcPeer 上**新加 video track**；M2.2 当时只测了 DataChannel，video 分支没真接。M5.1 补这块。

**Tech Stack（M5 新增）:**
- Mac: `screencapturekit = "0.3"` 或 `objc2-screen-capture-kit`（看哪个生态成熟；spec §3.1 标注 `screencapturekit-rs` 风格）；`objc2-video-toolbox = "0.3"`（H.264 encoder）；`objc2-core-media = "0.3"`（CMSampleBuffer / CMTime）。
- iOS: 0 新增 SPM 依赖（M2.4 已引入 stasel/WebRTC，含 RTCMTLVideoView）。
- Worker: 不动。

**M4 debt 一并清理（M5.0 task）：**
- 配对完成后动态重建 PushClient + NotifyEngine（避免重启 menu bar 才能推送）

**对应 spec：** `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 GuiCapture、§3.2 GuiStreamView、§4.4 GUI 监管时序、§5 ScreenCaptureKit 错误、§7 M5。

---

## 协议契约

### 共享类型

```rust
// macagent-core::ctrl_msg 加：
pub struct WindowInfo {
    pub window_id: u32,            // ScreenCaptureKit window ID
    pub app_name: String,          // "Google Chrome" / "Cursor" / "Terminal"
    pub bundle_id: Option<String>, // "com.google.Chrome"
    pub title: String,             // "GitHub - Mac–iOS Workspace"
    pub width: u32,
    pub height: u32,
    pub on_screen: bool,
    pub is_minimized: bool,
}

pub struct SupervisionEntry {
    pub sup_id: String,            // UUID v4 短形式
    pub window_id: u32,
    pub app_name: String,
    pub title: String,
    pub status: SupervisionStatus, // active | dead
    pub source: SupervisionSource, // existing (M5) | launched (M7)
    pub started_ts: u64,
}

pub enum SupervisionStatus { Active, Dead }
pub enum SupervisionSource { Existing, Launched }   // M5 仅 Existing

pub struct Viewport {
    pub width: u32,                // 像素
    pub height: u32,
}
```

### CtrlPayload 新增

```rust
// iOS → Mac
ListWindows                                              // 请求窗口列表
SuperviseExisting { window_id: u32, viewport: Viewport } // 开始监管
RemoveSupervised  { sup_id: String }                     // 结束监管
ViewportChanged   { sup_id: String, viewport: Viewport } // 旋转 / Stage Manager 改尺寸（M7 fit_window 才用；M5 只发不响应）

// Mac → iOS
WindowsList    { windows: Vec<WindowInfo> }
SupervisedAck  { sup_id: String, entry: SupervisionEntry }
SuperviseReject { window_id: u32, code: String, reason: String }
SupervisionList { entries: Vec<SupervisionEntry> }
StreamEnded     { sup_id: String, reason: String }      // window_closed / permission_lost / err
```

### Mac Agent 配置（agent.json5 加）

```jsonc
{
  // ...
  "video": {
    "active_fps": 30,
    "bitrate_kbps": 3000,
    "keyframe_interval_secs": 5,
    "codec": "h264_baseline",   // M5 锁定 baseline，未来可选 main/high
  }
}
```

### 权限流

ScreenCaptureKit 需要 **Screen Recording permission**。首次调用时 macOS 弹系统对话框；菜单栏 UI 应显示一个状态指示让用户知道权限缺失（如 `RTC: 缺少屏幕录制权限`）。

---

## 文件结构（增量）

```
mac-agent/Cargo.toml                                    ← workspace deps 加 screencapturekit, objc2-video-toolbox, objc2-core-media
mac-agent/crates/macagent-core/src/
├── ctrl_msg.rs                                         ← 加 WindowInfo / SupervisionEntry / 7 个 CtrlPayload
└── rtc_peer.rs                                         ← 改：新增 video track API（add_local_video_track / push_video_sample）

mac-agent/crates/macagent-app/src/
├── gui_capture/                                        ← 新模块
│   ├── mod.rs                                          ← GuiCapture struct + 接口
│   ├── windows.rs                                      ← list_windows() 调 ScreenCaptureKit list
│   ├── stream.rs                                       ← ScreenCaptureKit single-window stream + frame callback
│   ├── encoder.rs                                      ← VideoToolbox H.264 encoder wrapper
│   └── perm.rs                                         ← Screen Recording permission 状态查询
├── supervision_registry.rs                             ← 新：sup_id ↔ active stream
├── supervision_router.rs                               ← 新：ctrl ↔ GuiCapture 桥
└── ui.rs                                               ← 改：启动时持有 GuiCapture，注入 supervision_router；菜单栏显示权限状态

ios-app/MacIOSWorkspace/
├── Gui/                                                ← 新
│   ├── GuiStreamView.swift                             ← RTCMTLVideoView SwiftUI 包装
│   ├── SupervisionStore.swift                          ← @Observable，监管列表
│   └── WindowListView.swift                            ← 列出 Mac 上窗口，点击发 SuperviseExisting
├── PairedView.swift                                    ← 改：加 NavigationLink "桌面"
├── SessionStore.swift                                  ← 改：dispatch supervision/windows ctrl 到 SupervisionStore
└── RtcGlue.swift                                       ← 改：把 incoming video track 暴露给 SupervisionStore（renderer binding）

worker/                                                  ← 不动
```

---

## Task M5.0：M4 debt 清理 — 配对后动态重建 NotifyEngine

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（监听 paired record 变化时重建 PushClient + NotifyEngine）

### 改动

`ui.rs` 当前在 `MacAgentApp::new` 时如果有 paired record 就创建 PushClient + NotifyEngine；首次配对完成后没重建。

修复：

```rust
// 在 PairState::Paired 切换路径里，重建 NotifyEngine：
fn rebuild_notify_engine(&mut self, record: &PairRecord) {
    if let Ok(client) = PushClient::new(record.worker_url.clone(), record.pair_id.clone(), &record.mac_device_secret_b64) {
        let engine = Arc::new(NotifyEngine::new(Arc::new(client), self.ctrl_send_tx.clone()));
        self.notify_engine = Some(engine.clone());
        // 通知 SessionRouter / AgentSocket 更新引用
        self.session_router_tx.send(RouterMsg::ReplaceNotifyEngine(engine)).ok();
        self.agent_socket_tx.send(SocketMsg::ReplaceNotifyEngine(...)).ok();
    }
}
```

具体落点取决于 ui.rs 现状；最简化版可以每次进 Paired 状态都重建一次（即使是 Keychain 加载启动，第二次 rebuild 也无副作用）。

### 步骤

1. 改 ui.rs（约 40 行）
2. cargo build / test / clippy / fmt
3. commit：`fix(mac-agent): rebuild NotifyEngine on PairState::Paired transition (M4 debt)`

---

## Task M5.1：Mac RtcPeer 加 video track API

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/rtc_peer.rs`
- Modify: `mac-agent/crates/macagent-core/tests/rtc_peer_test.rs`（新增 video loopback test）

### 接口扩展

```rust
//! RtcPeer add video track support（M5）

use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use std::time::Duration;

impl RtcPeer {
    /// 添加一个 H.264 video track。返回 sender 句柄供后续 push 帧。
    /// 注意：这个 API 必须在 create_offer / create_answer **之前**调用，
    /// 否则 SDP 不会 include video m-section。
    pub async fn add_local_h264_video_track(&self) -> Result<VideoTrackHandle> {
        let codec = RTCRtpCodecCapability {
            mime_type: "video/H264".to_string(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".into(),
            ..Default::default()
        };
        let track = Arc::new(TrackLocalStaticSample::new(
            codec,
            "video-cap".to_string(),     // track id
            "macagent".to_string(),      // stream id
        ));
        self.pc.add_track(track.clone()).await?;
        Ok(VideoTrackHandle { track })
    }
}

pub struct VideoTrackHandle {
    track: Arc<TrackLocalStaticSample>,
}

impl VideoTrackHandle {
    /// 推一个 H.264 NALU sample。timestamp 用 90kHz 单位。
    pub async fn push_sample(&self, data: bytes::Bytes, duration: Duration) -> Result<()> {
        self.track.write_sample(&Sample {
            data,
            duration,
            ..Default::default()
        }).await?;
        Ok(())
    }
}
```

### Tests

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_video_track_negotiation() {
    let alice = RtcPeer::new(vec![]).await.unwrap();
    let bob = RtcPeer::new(vec![]).await.unwrap();

    let _video = alice.add_local_h264_video_track().await.unwrap();
    let _ctrl = alice.open_ctrl_channel().await.unwrap();

    let offer = alice.create_offer().await.unwrap();
    assert!(offer.contains("m=video"));
    assert!(offer.contains("H264"));

    bob.apply_remote_offer(&offer).await.unwrap();
    let answer = bob.create_answer().await.unwrap();
    alice.apply_remote_answer(&answer).await.unwrap();
    
    // ICE candidates 互喂 + connected 检查（参考 M2.2 现有 test 模式）

    alice.close().await.unwrap();
    bob.close().await.unwrap();
}
```

### 步骤

1. 改 rtc_peer.rs 加 add_local_h264_video_track + VideoTrackHandle
2. 写 1 条 loopback test（验证 SDP 含 m=video）
3. cargo test -p macagent-core 全过
4. cargo clippy / fmt
5. commit：`feat(core): add H.264 video track API to RtcPeer (M5.1)`

---

## Task M5.2：Mac GuiCapture（ScreenCaptureKit + VideoToolbox）

**最大风险 task**——需要研究 screencapturekit crate API + objc2-video-toolbox 编码 pipeline + 处理 NSScreen permission UX。

**Files:**
- Modify: `mac-agent/Cargo.toml`（workspace deps 加 screencapturekit + objc2-video-toolbox + objc2-core-media）
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`
- Create: `mac-agent/crates/macagent-app/src/gui_capture/mod.rs`
- Create: `mac-agent/crates/macagent-app/src/gui_capture/windows.rs`
- Create: `mac-agent/crates/macagent-app/src/gui_capture/stream.rs`
- Create: `mac-agent/crates/macagent-app/src/gui_capture/encoder.rs`
- Create: `mac-agent/crates/macagent-app/src/gui_capture/perm.rs`

### `mod.rs` 接口（约 80 行）

```rust
//! macOS GUI capture：抓单窗口 → H.264 → 喂 webrtc-rs video track。

use anyhow::Result;
use macagent_core::ctrl_msg::WindowInfo;
use macagent_core::rtc_peer::VideoTrackHandle;
use std::sync::Arc;

pub mod windows;
pub mod stream;
pub mod encoder;
pub mod perm;

pub struct GuiCapture {
    /// 当前活跃流（M5 同时只有一个）
    active: tokio::sync::Mutex<Option<ActiveStream>>,
    config: VideoConfig,
}

pub struct VideoConfig {
    pub fps: u32,            // 30
    pub bitrate_kbps: u32,   // 3000
    pub keyframe_interval_secs: u32,  // 5
}

struct ActiveStream {
    window_id: u32,
    stream: stream::ScKitStream,
    encoder: encoder::H264Encoder,
    track: VideoTrackHandle,
    sup_id: String,
}

impl GuiCapture {
    pub fn new(config: VideoConfig) -> Self {
        Self { active: tokio::sync::Mutex::new(None), config }
    }

    /// 检查 Screen Recording 权限是否已授予；未授予则触发系统弹窗。
    pub fn check_permission(&self) -> perm::PermissionStatus {
        perm::check()
    }

    /// 列出当前所有可见窗口（不含菜单栏 / wallpaper）。
    pub async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        windows::list().await
    }

    /// 开始监管单窗口，把 H.264 NALU 推到给定 video track。
    pub async fn supervise_existing(
        &self, sup_id: String, window_id: u32, track: VideoTrackHandle,
    ) -> Result<()> {
        let mut guard = self.active.lock().await;
        // M5 同时只允许一个 active；如有先 stop
        if let Some(prev) = guard.take() {
            prev.stream.stop().await?;
        }
        let stream = stream::ScKitStream::start(window_id, self.config.fps).await?;
        let encoder = encoder::H264Encoder::new(&self.config)?;
        let bridge = stream::FrameBridge::new(stream.clone(), encoder.clone(), track.clone());
        bridge.spawn();
        *guard = Some(ActiveStream { window_id, stream, encoder, track, sup_id });
        Ok(())
    }

    /// 结束监管。
    pub async fn remove_supervised(&self, sup_id: &str) -> Result<()> {
        let mut guard = self.active.lock().await;
        if let Some(active) = guard.as_ref() {
            if active.sup_id == sup_id {
                let stream = guard.take().unwrap().stream;
                stream.stop().await?;
            }
        }
        Ok(())
    }

    /// 如果窗口已经关闭，stream 会主动失败；router 监听这个事件。
    pub fn on_stream_ended(&self, cb: impl Fn(String /*sup_id*/, String /*reason*/) + Send + Sync + 'static) {
        // 内部用 broadcast::Sender 通知；router 订阅
        unimplemented!("see stream::ScKitStream::on_end")
    }
}
```

### `perm.rs`（约 30 行）

```rust
use anyhow::Result;

pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
}

/// 调 CGPreflightScreenCaptureAccess（macOS 11+）查权限。
pub fn check() -> PermissionStatus {
    // 用 core-graphics crate 的 CGPreflightScreenCaptureAccess
    // 简化：第一次抓取时若失败，调 CGRequestScreenCaptureAccess 触发系统弹窗
    unimplemented!()
}
```

### `windows.rs`（约 80 行）

```rust
//! 列出当前可见窗口。

use anyhow::Result;
use macagent_core::ctrl_msg::WindowInfo;

pub async fn list() -> Result<Vec<WindowInfo>> {
    // 用 screencapturekit crate 或 CGWindowListCopyWindowInfo
    // 过滤：on_screen=true、layer==0（普通窗口，不是 dock / menu bar / desktop）
    // 提取 window_id, owner_app_name, owner_bundle_id, title, width, height
    unimplemented!()
}
```

### `stream.rs` + `encoder.rs`

最复杂的两个文件。简化骨架：

`stream.rs`：
- `ScKitStream::start(window_id, fps)`：调 `SCStream` API 启动单窗口捕获，设置 `SCContentFilter` 单 window；frame delegate 收到 CMSampleBuffer 后转 callback；返回结构含 `stop()` 方法
- 把每帧 CVPixelBuffer 转给 encoder

`encoder.rs`：
- `H264Encoder::new(config)`：用 VTCompressionSession 创建 H.264 编码器
- `encode(pixel_buffer) → Vec<NALU bytes>`：CVImageBuffer → CMSampleBuffer encoded → 提取 H.264 NAL 单元 → 把 SPS/PPS/I-frame/P-frame 拼成可送 webrtc-rs 的 sample（每个 sample 是一帧）

`FrameBridge`：用 mpsc 把 stream 帧 → encoder → video track。

### 实现策略

- **优先尝试 `screencapturekit` crate** （由 svtlabs 维护，~80% 覆盖 ScreenCaptureKit），文档不全，可能需要查源码
- 若 crate 不够用，**退到** `objc2-screen-capture-kit` + 直接调 ObjC2 binding
- VideoToolbox：用 `objc2-video-toolbox` 的 `VTCompressionSession` API
- **如果 implementer 撞墙严重**（screencapturekit crate 不够 mature 或 VideoToolbox FFI 太复杂），上报 BLOCKED；此时可以接受 stub 实现：先让接口编译，stream/encoder 内部产生 dummy 数据（如黑色帧）让 ctrl 流跑通，真编码 push 到 M5.2.5（拆出来）

### 步骤

1. 改 Cargo.toml × 2 加 deps
2. 写 5 个 src 文件（stub 形式或真实现）
3. 写 1-2 个简单单元测试（permission check、window list 解析）
4. cargo build -p macagent-app 编译过
5. cargo test 全过
6. cargo clippy / fmt
7. **手测**：menu bar Agent 启动后调 `gui_capture.list_windows()` 应返回真实窗口列表（debug print 出来对比）
8. commit：`feat(mac-agent): add GuiCapture (ScreenCaptureKit + VideoToolbox H.264)`

> 这个 task 可能需要 **2-3 次 fixup**。给 implementer 必要的探索空间。

---

## Task M5.3：ctrl_msg + Swift 端协议扩展

**Files:**
- Modify: `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（加 7 个 CtrlPayload + WindowInfo / SupervisionEntry / Viewport / Status / Source）
- Modify: `ios-app/MacIOSWorkspace/CtrlMessage.swift`（同步）
- Create: `mac-agent/crates/macagent-core/tests/m5_protocol_test.rs`（round-trip + canonical bytes）

### 步骤

1. 改 ctrl_msg.rs 加类型 + canonical_bytes 自动覆盖（M3.fix 递归）
2. 改 CtrlMessage.swift 同步
3. 写测试 3-5 条（round-trip + canonical）
4. cargo test 全过
5. xcodebuild build 过
6. commit：`feat(core): extend CtrlPayload with M5 GUI supervision types`

---

## Task M5.4：Mac supervision_router

**Files:**
- Create: `mac-agent/crates/macagent-app/src/supervision_router.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`（创建 GuiCapture + supervision_router 注入 rtc_glue）
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs`（在 ctrl 消息 handle 处分发到 supervision_router）

### 关键逻辑

```rust
// supervision_router.rs

pub struct SupervisionRouter {
    gui_capture: Arc<GuiCapture>,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    rtc_peer: Arc<RtcPeer>,
}

impl SupervisionRouter {
    pub async fn handle_ctrl(&self, payload: CtrlPayload) -> Result<()> {
        match payload {
            CtrlPayload::ListWindows => {
                let windows = self.gui_capture.list_windows().await?;
                let _ = self.ctrl_tx.send(CtrlPayload::WindowsList { windows });
            }
            CtrlPayload::SuperviseExisting { window_id, viewport } => {
                let track = self.rtc_peer.add_local_h264_video_track().await?;
                let sup_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                match self.gui_capture.supervise_existing(sup_id.clone(), window_id, track).await {
                    Ok(()) => {
                        let entry = SupervisionEntry { /* ... */ };
                        let _ = self.ctrl_tx.send(CtrlPayload::SupervisedAck { sup_id, entry });
                    }
                    Err(e) => {
                        let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                            window_id, code: "supervise_failed".into(), reason: e.to_string(),
                        });
                    }
                }
            }
            CtrlPayload::RemoveSupervised { sup_id } => {
                self.gui_capture.remove_supervised(&sup_id).await?;
                let _ = self.ctrl_tx.send(CtrlPayload::StreamEnded { sup_id, reason: "removed".into() });
            }
            _ => {}  // 其它 ctrl 不处理
        }
        Ok(())
    }
}
```

> **注意 SDP renegotiation**：第一次 add_local_h264_video_track 之后 PeerConnection 已经协商过 video m-section；后续 supervise 不需要再 add track，复用同一个 track（M2.2 RtcPeer 模型支持）。

### 步骤

1. 写 supervision_router.rs（约 150 行）
2. 改 ui.rs 创建 GuiCapture + SupervisionRouter
3. 改 rtc_glue.rs 把 ctrl 消息 dispatch 到 supervision_router
4. cargo test / clippy / fmt
5. commit：`feat(mac-agent): add supervision_router connecting ctrl to GuiCapture and RtcPeer`

---

## Task M5.5：iOS GuiStreamView（RTCMTLVideoView 包装）

**Files:**
- Create: `ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift`
- Modify: `ios-app/MacIOSWorkspace/RtcGlue.swift`（暴露 incoming video track stream / RTCVideoRenderer 绑定）
- Modify: `ios-app/MacIOSWorkspace/RtcClient.swift`（监听 didAddRtpReceiver / videoTrack callback）

### 设计

GoogleWebRTC iOS framework 在收到 incoming video track 时会触发 `RTCPeerConnectionDelegate.peerConnection(_:didAdd:)` 或 `didStartReceivingOn:`。需要：

1. RtcClient 监听这些 callback，把 RTCVideoTrack 通过 AsyncStream 暴露
2. GuiStreamView 创建 RTCMTLVideoView，把 video track add 进去渲染

```swift
// GuiStreamView.swift

import SwiftUI
import WebRTC

struct GuiStreamView: UIViewRepresentable {
    let videoTrack: RTCVideoTrack?
    
    func makeUIView(context: Context) -> RTCMTLVideoView {
        let view = RTCMTLVideoView()
        view.contentMode = .scaleAspectFit
        view.videoContentMode = .scaleAspectFit
        if let track = videoTrack {
            track.add(view)
        }
        return view
    }
    
    func updateUIView(_ uiView: RTCMTLVideoView, context: Context) {
        // M5 同时只有 1 个 active track，updateUIView 不需要切换；
        // M7 多 supervise 时这里要根据当前 active 切换 attach 的 track
    }
}
```

```swift
// RtcClient.swift 加：

extension RtcClient {
    /// 暴露 incoming video tracks 给 SupervisionStore。
    func incomingVideoTracks() -> AsyncStream<RTCVideoTrack> {
        // 在 PeerObserver.onAddRtpReceiver 里 yield
    }
}
```

### 步骤

1. 改 RtcClient.swift 加 incomingVideoTracks AsyncStream
2. 写 GuiStreamView.swift
3. xcodebuild build 过
4. commit：`feat(ios-app): add GuiStreamView (RTCMTLVideoView) for incoming video stream`

---

## Task M5.6：iOS SupervisionStore + WindowListView + PairedView 入口

**Files:**
- Create: `ios-app/MacIOSWorkspace/Gui/SupervisionStore.swift`
- Create: `ios-app/MacIOSWorkspace/Gui/WindowListView.swift`
- Modify: `ios-app/MacIOSWorkspace/SessionStore.swift`（dispatch ListWindows/WindowsList 等到 SupervisionStore）
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift`（创建 SupervisionStore 注入；加 NavigationLink "桌面"）

### `SupervisionStore.swift`

```swift
@MainActor
@Observable
final class SupervisionStore {
    private(set) var windows: [WindowInfo] = []
    private(set) var entries: [SupervisionEntry] = []
    private(set) var activeTrack: RTCVideoTrack?
    private weak var glue: RtcGlue?

    init(glue: RtcGlue?) { self.glue = glue }

    func bindIncomingTracks() async {
        guard let glue else { return }
        for await track in await glue.incomingVideoTracks() {
            activeTrack = track
        }
    }

    // Inbound from ctrl
    func handleWindowsList(_ list: [WindowInfo]) { windows = list }
    func handleSupervisedAck(_ entry: SupervisionEntry) {
        if !entries.contains(where: { $0.sup_id == entry.sup_id }) {
            entries.append(entry)
        }
    }
    func handleSuperviseReject(windowId: UInt32, code: String, reason: String) {
        // 弹 alert via observable error state
    }
    func handleStreamEnded(supId: String, reason: String) {
        entries.removeAll { $0.sup_id == supId }
        // M5 同时只有 1 个 active；都 ended 后 activeTrack 归 nil
        if entries.isEmpty {
            activeTrack = nil
        }
    }

    // Outbound
    func refreshWindows() async {
        await glue?.sendCtrl(.listWindows)
    }
    func supervise(windowId: UInt32, viewport: Viewport) async {
        await glue?.sendCtrl(.superviseExisting(window_id: windowId, viewport: viewport))
    }
    func remove(supId: String) async {
        await glue?.sendCtrl(.removeSupervised(sup_id: supId))
    }
}
```

### `WindowListView.swift`

列窗口（自动 refresh on appear），点击 → 发 SuperviseExisting → 进入 GuiStreamDetailView：

```swift
struct WindowListView: View {
    @Bindable var store: SupervisionStore

    var body: some View {
        List {
            Section("当前监管") {
                if store.entries.isEmpty {
                    Text("无").foregroundStyle(.secondary)
                } else {
                    ForEach(store.entries, id: \.sup_id) { entry in
                        NavigationLink(destination: GuiStreamDetailView(store: store, entry: entry)) {
                            Text("\(entry.app_name) – \(entry.title)")
                        }
                    }
                }
            }
            Section("可监管窗口") {
                if store.windows.isEmpty {
                    Text("点 ↻ 刷新").foregroundStyle(.secondary)
                } else {
                    ForEach(store.windows, id: \.window_id) { w in
                        Button("\(w.app_name) – \(w.title)") {
                            Task {
                                await store.supervise(
                                    windowId: w.window_id,
                                    viewport: Viewport(width: 393, height: 852),  // M5 hardcoded iPhone 16 Pro 视口
                                )
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("桌面窗口")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { Task { await store.refreshWindows() } } label: { Image(systemName: "arrow.clockwise") }
            }
        }
        .task { await store.refreshWindows() }
    }
}

struct GuiStreamDetailView: View {
    @Bindable var store: SupervisionStore
    let entry: SupervisionEntry

    var body: some View {
        VStack {
            GuiStreamView(videoTrack: store.activeTrack)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.black)
        }
        .navigationTitle(entry.app_name)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(role: .destructive) {
                    Task {
                        await store.remove(supId: entry.sup_id)
                    }
                } label: { Image(systemName: "stop.circle") }
            }
        }
    }
}
```

### 步骤

1. 写 SupervisionStore + WindowListView
2. 改 SessionStore handle 新 ctrl case 转给 supervisionStore
3. 改 PairedView 创建 SupervisionStore + bindIncomingTracks task + NavigationLink "桌面"
4. xcodebuild build 过
5. commit：`feat(ios-app): add SupervisionStore + WindowListView + Gui entry in PairedView`

---

## Task M5.7：真机端到端验证

需要：
- 用户授予 Mac Screen Recording 权限（首次 SCStream 调用时弹窗）
- iPhone 真机连上 Mac
- iPhone 进 PairedView → 桌面 → 看窗口列表 → 选 Chrome → 看到实时画面 30 fps

### 验收

1. iOS 看到 macOS 当前所有可见窗口（Chrome / Cursor / Finder / Terminal 等）
2. 选 Chrome → 弹"允许 macagent 录制屏幕？"权限对话框 → 用户授权
3. iPhone 看到 Chrome 实时画面（30 fps，~3 Mbps，正常网络下流畅）
4. 用户在 Mac 端关 Chrome → iPhone 收到 StreamEnded → UI 自动回退
5. 用户点 iOS 上的 stop.circle → Mac stream 停 → SupervisedAck removed
6. 同时连 4G + 跨 Wi-Fi（M2.6 已验证 TURN 路径），重新 supervise → 仍能拿到画面（可能更慢）

### 步骤

无 commit；纯人工验证。结果记录到 final review。

---

## Task M5.8：M5 final review

dispatch reviewer subagent，按 M4 final review 同样模式审 commits `e285232..HEAD`。

---

## M5 验收清单

- [ ] worker npm test 不变（32/32）
- [ ] mac-agent cargo test --workspace 全绿（含 RtcPeer video track loopback）
- [ ] ios-app xcodebuild build 过（含 GuiStreamView / SupervisionStore）
- [ ] CI 三条 workflow 全绿
- [ ] 真机：iOS 看到 Mac 窗口列表 ✓
- [ ] 真机：选 Chrome → 30 fps 实时画面 ✓
- [ ] 真机：关 Chrome → iOS 自动 StreamEnded ✓
- [ ] 真机：跨网（4G）仍能拿到画面 ✓

---

## 自检（写完 plan 后做的）

1. **Spec 覆盖**：spec §3.1 GuiCapture / §3.2 GuiStreamView / §4.4 GUI 时序 / §5 ScreenCaptureKit 错误 / §7 M5 → 全部映射；M6（输入注入）+ M7（launch + multi-supervise）明确不做。
2. **占位符扫描**：M5.2 GuiCapture 内部实现可能需要 stub（screencapturekit crate 成熟度未知），plan 已显式标注。
3. **类型一致性**：CtrlPayload Mac/iOS 同步；Viewport 类型在两端字段名一致；canonical_bytes M3.fix 递归排序自动覆盖。
4. **M4 debt M5.0 清理**，M5 后续 task 不会卡在 NotifyEngine 旧引用。
5. **风险**：
   - **M5.2 是 M5 的最大风险点**——ScreenCaptureKit Rust 绑定生态未知；如果 screencapturekit crate 不够用，可能需要 objc2 直接调；最差情况 stub 走通 ctrl/视频管线，真编码留 M5.2.5
   - VideoToolbox 硬件编码 + webrtc-rs `TrackLocalStaticSample` 的 NAL 帧组装可能需要细调（profile/level、SPS/PPS 携带方式）
   - Screen Recording 权限第一次弹窗需要用户配合；CI 上无法自动测试（标 manual-only）
   - GoogleWebRTC iOS 137 vs webrtc-rs 0.11 的 H.264 RTP 实现差异（profile-level-id、packetization-mode）—— spec 已知风险

---

## Plan 完成后下一步

执行选项：

1. **Subagent-Driven**（推荐）——延续 M0-M4 节奏；M5.0/M5.1/M5.3/M5.4 自动化高；M5.2 是最大风险点（可能需要多次 fixup 或 stub 实现）；M5.5/M5.6 iOS UI；M5.7 真机
2. **Inline Execution**

请用户选 1 或 2。
