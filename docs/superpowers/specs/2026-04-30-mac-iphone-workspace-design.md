# Mac–iOS Workspace v0.1 — 设计文档

- **日期**：2026-04-30
- **状态**：草案，可进入实现规划
- **负责人**：bruce@hemory.com
- **代号**：macagent（Mac 端）+ 配套 iOS App + Cloudflare Workers 后端

---

## 1. 范围与非目标

### 1.1 一句话产品定义

把 Mac 变成一台可从 iPhone / iPad 接管的"应用级远程工作台"——不是远程桌面。iOS 设备拿到一等的 CLI 会话，可选地拉一个被监管的应用窗口画面，并与 Mac 共享剪贴板。

### 1.2 v0.1 范围

- **Mac Agent** —— 单个 Rust 二进制，作为 `LaunchAgent` 常驻；内嵌 `egui` 菜单栏 / 设置 UI。
- **iOS App** —— 通用 Swift / SwiftUI 应用，**iPhone 与 iPad，iOS 26 / iPadOS 26**；TestFlight 起步发布。**单一 app target**，自适应布局，无独立 iPad 代码分支。
- **后端** —— Cloudflare Workers（TypeScript），承担配对、信令、APNs 和 TURN 凭证。**TURN/STUN** 用 Cloudflare Calls（托管）。无自建服务器、无 VPS、无 `coturn`。
- **配对** —— 二维码一次性配对（P2 模型）。无账号体系。长期 pair 密钥经 X25519 ECDH 派生，控制平面消息全部 HMAC 签名。
- **网络模型** —— 控制平面跑在 Cloudflare 托管边缘（Workers + Durable Objects + KV），无 VPS 运维负担。媒体平面走 WebRTC P2P，必要时回退 TURN。可在局域网、移动网络、NAT 下工作。
- **CLI 通道** —— 每对设备最多 **8 个并发 PTY 会话**。每会话有 1 MB / 1 万行的环形缓冲区，外加 append-only 落盘日志。会话穿越 iOS 端断线（tmux 风格 detach），重连时由 `resume_session(last_seq)` 补齐缺口。
- **GUI 通道（B1 档）** —— 最多 **8 个监管窗口**注册，**同时只有 1 个 active 流**，切换约 200 ms。两条添加路径：`supervise_existing`（任意可见窗口）和 `supervise_launch`（白名单 App：ChatGPT Desktop、Claude Desktop、Chrome）。ScreenCaptureKit 单窗口捕获 → VideoToolbox H.264 编码 → `webrtc-rs` SRTP 推流 → iOS 官方 WebRTC framework 解码。
- **窗口适配** —— `switch_active` 时 Mac Agent 通过 Accessibility API 把目标窗口尺寸调整到 iOS 端上报的视口比例（覆盖 iPhone 竖横屏、iPad 竖横屏、iPad Split View、Stage Manager 等情形）。`remove_supervised` 时还原原始 frame。armed 状态保持最近一次 fit 的比例。
- **内容缩放** —— iOS 端 `+ / − / ⌖` 三个按钮通过 `input` 通道发送 `Cmd+= / Cmd+- / Cmd+0`。无状态透传。
- **输入注入** —— `CGEvent` 处理点击 / 滚动 / 键盘（Unicode）；长文本快速通道：写 `NSPasteboard` + 模拟 `Cmd+V`。
- **剪贴板同步** —— 双向。Mac → iOS 自动（轮询 `NSPasteboard.changeCount` 间隔 500 ms）。iOS → Mac 仅在用户显式按"发送到 Mac"时触发，避免隐私泄露。
- **通知** —— 满足以下条件触发 APNs 推送：(a) 显式 `notify <cmd>` 命令完成，(b) 每会话用户自定义正则命中。

### 1.3 v0.1 验收 App 矩阵

| 通道 | 验收对象 |
|---|---|
| CLI | Claude Code、Codex、Terminal `zsh` |
| GUI（启动 + 监管） | ChatGPT Desktop、Claude Desktop、Chrome |
| GUI（仅监管现有，标记"未验证"） | 任意可见窗口 |

### 1.4 非目标

- 不做完整桌面镜像、多显示器、文件拖拽、企业 SSO。
- 不处理子窗口、菜单、文件选择器（推迟到 B2 档）。
- iOS 端通过标准 `UITextView`（SwiftUI `TextEditor`）原生支持中文 IME 与系统/第三方键盘语音输入，参见独立 spec [`2026-04-30-ios-input-compose-design.md`](2026-04-30-ios-input-compose-design.md)。
- 不支持会话穿越 **macOS** 重启（PTY 仅从 RtcPeer detach，未脱离 Agent 进程）。
- UI 不展示多 Mac / 多 iOS 拓扑（数据模型留出空间，但 UI 仅 1:1）。
- 不通过我们的 Workers 中继媒体流量（仅信令 + APNs + TURN 凭证；媒体走 P2P/TURN 直连）。
- 无账号系统、团队、协作。

### 1.5 系统下限

| 平台 | 下限 |
|---|---|
| iOS / iPadOS | 26（通用 app，单 target） |
| macOS | 15 Sequoia |

**v0.1 验收设备矩阵**：至少一台 iPhone（任何运行 iOS 26 的型号），至少一台 iPad（任何运行 iPadOS 26 的型号，包含 Magic Keyboard / 外接键盘场景）。

---

## 2. 高层架构

```
┌──────────────────────────┐                ┌──────────────────────────────┐
│         iOS App          │                │         Mac Agent            │
│  (Swift / SwiftUI 26)    │                │  (Rust, single binary)       │
│                          │                │                              │
│  ┌────────────────────┐  │                │  ┌────────────────────────┐  │
│  │ Pairing UI (QR)    │  │                │  │ Menu Bar / Pref UI     │  │
│  │ Workspace Tabs     │  │                │  │ (egui)                 │  │
│  │ CLI View (term)    │  │                │  └─────────┬──────────────┘  │
│  │ GUI View (WebRTC)  │  │                │            │                 │
│  │ Clipboard Panel    │  │                │  ┌─────────▼──────────────┐  │
│  │ APNs Receiver      │  │                │  │ Core Daemon            │  │
│  └────────┬───────────┘  │                │  │  ├─ PairAuth           │  │
│           │              │                │  │  ├─ SignalingClient    │  │
│           │              │                │  │  ├─ RtcPeer (webrtc-rs)│  │
│           │              │                │  │  ├─ SessionManager     │  │
│           │              │                │  │  ├─ GuiCapture         │  │
│           │              │                │  │  ├─ InputInjector      │  │
│           │              │                │  │  ├─ ClipboardBridge    │  │
│           │              │                │  │  └─ NotifyEngine       │  │
│           │              │                │  └─────────┬──────────────┘  │
└───────────┼──────────────┘                └────────────┼─────────────────┘
            │                                            │
            │   ┌────────────────────────────────┐       │
            │   │   Cloudflare Workers           │       │
            │   │   • POST /pair/create          │       │
            └──►│   • POST /pair/claim           │◄──────┘
                │   • WS   /signal/:pair_id (DO) │
                │   • POST /push (→ APNs HTTP/2) │
                │   • POST /turn/cred            │
                │   • KV: device_pairs           │
                │   • Secret: APNs AuthKey       │
                └────────────────┬───────────────┘
                                 │
                  ┌──────────────┴────────────────┐
                  ▼                               ▼
           ┌──────────────┐              ┌──────────────────┐
           │ Apple APNs   │              │ Cloudflare Calls │
           │              │              │ (TURN / STUN)    │
           └──────────────┘              └──────────────────┘

媒体平面（P2P → TURN 回退）：
   Mac Agent ◄──── DTLS-SRTP ────► iOS App
```

### 2.1 平面分离

- **控制平面** —— iOS ↔ Worker ↔ Mac Agent。始终走 HTTPS / WebSocket。承载配对、SDP/ICE 交换、TURN 凭证下发、APNs 触发请求。所有载荷使用 ECDH 派生的 per-pair 共享密钥进行 HMAC 签名。
- **媒体平面** —— iOS ↔ Mac Agent 直连 WebRTC PeerConnection。优先 P2P；对称 NAT 下回退到 Cloudflare Calls TURN。一条 PeerConnection 上同时承载 1 个 video track 和多条 DataChannel（`ctrl`、`clip`、`input`、`pty/<id>`）。

### 2.2 部署清单

| 组件 | 技术栈 | 运维负担 |
|---|---|---|
| Mac Agent | Rust 单二进制 | 代码签名 `.app` + `LaunchAgent` plist；自动更新非 v0.1 范围。 |
| iOS App | Swift | TestFlight 构建管线。 |
| 后端 | 1 个 Worker + 1 个 Durable Object 类（`SignalingRoom`）+ 1 个 KV namespace（`pairs`） | `wrangler` 部署，无长期运行的服务器。 |
| TURN/STUN | Cloudflare Calls | 全托管；按量计费（免费额度满足 MVP）。 |

---

## 3. 组件职责

每个单元**单一目的、明确接口、显式依赖**，可独立测试。

### 3.1 Mac Agent（Rust）

模块间通过 `tokio::mpsc` channel 通信。菜单栏 UI（`egui`）与各模块同进程，沿用同样的 channel 协议。

#### PairAuth
- **目的** —— 管理一次性配对 token、长期 pair 密钥、出站控制平面请求的 HMAC 签名。
- **接口** —— `create_pair() → PairToken`、`load_secret() → Option<PairSecret>`、`sign(req) → SignedReq`、`revoke()`。
- **依赖** —— macOS Keychain（`security-framework` crate）、Worker `/pair/create`。

#### SignalingClient
- **目的** —— 维持到 Worker `/signal/:pair_id` 的 WebSocket；中继本地 RtcPeer 与远端的 SDP 与 ICE candidate。
- **接口** —— `offer(sdp)`、`answer(sdp)`、`on_remote_candidate(cb)`、`state() → ConnState`。
- **依赖** —— `tokio-tungstenite`、PairAuth（每条 WS 消息都要签名）。
- **重连** —— 指数退避 1 s → 30 s 上限；不影响已建立的媒体平面。

#### RtcPeer（webrtc-rs）
- **目的** —— 单条 PeerConnection。负责 DTLS-SRTP 终结、ICE 状态机、1 条出站 video track、N 条 DataChannel。
- **DataChannel 列表** —
  - `ctrl` —— JSON 帧化的控制消息（开/恢复 session、监管、切换、推送触发、错误）。
  - `clip` —— JSON 帧化的剪贴板事件。
  - `input` —— JSON 帧化的 GUI 输入事件。
  - `pty/<id>` —— 二进制，每条活跃 CLI session 一条，含 32-bit 序列号。
- **接口** —— `start_session()`、`apply_remote_sdp(sdp)`、`add_local_candidate()`、`open_data_channel(label, opts)`、`replace_video_source(track)`。
- **依赖** —— `webrtc`（webrtc-rs）crate、Worker `/turn/cred` 提供 ICE servers、SignalingClient。

#### Producer 子进程模式（M3 v2 取代 SessionManager）

**核心模型变更**：受 hurryvc 启发，放弃 spawn-in-Agent 隐藏 PTY 模型，改为 producer 在用户终端可见。M3 引入 `macagent run -- <cmd>` CLI 子命令；该子进程把 PTY 直接挂在用户的 tty 上（Mac 用户看得见 + 可输入），同时在 producer 进程内用 `alacritty_terminal` 解析 PTY 输出，把结构化的 TermSnapshot/Delta 经 Unix socket 推给菜单栏 Mac Agent，再由 Agent 经 ctrl DataChannel 转发给 iOS。

**Mac Agent menu bar 端的对应模块**：

##### ProducerRegistry
- **目的** —— 维护 sid → producer 的映射、SessionInfo（argv、pid、cols/rows、source、started_ts）、生命周期。
- **接口** —— `register(producer_tx, hello) → sid`、`get(sid)`、`list() → [SessionInfo]`、`unregister(sid, reason)`。
- **上限** —— 硬限 8 并发；超出 `register` 返回 `ErrSessionLimit`；同时 menu bar UI 列表里展示。

##### AgentSocket
- **目的** —— Unix domain socket server，监听 `~/Library/Application Support/macagent/agent.sock`；接受 producer 子进程连接；JSON-frame（4 字节 BE 长度前缀 + JSON body）双向。
- **接口** —— `start(addr) → JoinHandle`、广播 `producer_connected/disconnected` 事件给 SessionRouter。
- **依赖** —— `tokio::net::UnixListener`。

##### Launcher
- **目的** —— 读 `~/Library/Application Support/macagent/launchers.json5` 用户白名单；接收 `LaunchSession { launcher_id }`，校验后通过 AppleScript 调用 Terminal.app 弹新窗口跑 `macagent run --launcher-id <id>`。
- **接口** —— `load_config()`、`launch(launcher_id, cwd_override) → Result<()>`、`reload()` (SIGHUP)。
- **默认白名单** —— `zsh` / `claude code` / `codex` / `npm test` / `git status`（首次启动自动写入配置文件）。
- **依赖** —— `osascript` 系统命令；用户首次需授予 macOS Automation 权限给 Terminal.app。

##### SessionRouter
- **目的** —— ctrl DataChannel ↔ Unix socket 的双向桥。把 iOS 来的 `Input` / `Resize` / `KillSession` 经 socket 转发给对应 producer；把 producer 来的 `TermSnapshot` / `TermDelta` / `TermHistory*` / `ProducerExit` 包成 SignedCtrl 经 ctrl DC 推给 iOS。
- **接口** —— `attach_iOS(sid)`（标记开始流；告诉 producer 进入 streaming 模式）、`detach_iOS(sid)`、内部 mpsc 路由。

**`macagent run` producer 端**（同一 binary 的子命令）：

##### ProducerProcess
- **目的** —— 在用户当前 tty 跑 `<cmd>`，PTY 输出双线流到本地 tty + alacritty parser；把结构化结果经 socket 推给 Agent。
- **职责** —
  1. 启动时：连 `agent.sock`，发 `ProducerHello { argv, pid, cols, rows, cwd, launcher_id?, source }`，等 `ProducerWelcome { sid }`
  2. 用 `portable-pty` fork 子进程；PTY master 输出 → `tee` 到本地 tty（用户看到）+ 喂 alacritty `Term`
  3. alacritty grid 增量更新；每 50ms（默认）算 diff → `TermDelta`；每 5s 推一次全量 `TermSnapshot` keyframe；scrollback 满推 `TermHistoryAppend`
  4. 收 socket `Input { TerminalInput::Text/Key }` → 翻译成 PTY stdin 字节（控制键查表，例如 CtrlC=0x03、Tab=0x09）
  5. 收 socket `Resize { cols, rows }` → 调 master.resize（PTY ioctl TIOCSWINSZ）
  6. 收 socket `KillRequest` 或 PTY 子进程退出 → 推 `ProducerExit { exit_status, reason }` → 关 socket → 进程退出
  7. Cmd+W 关 Terminal.app 窗口 → SIGHUP → producer 优雅退出
- **依赖** —— `portable-pty`、`alacritty_terminal`、`tokio::net::UnixStream`。
- **不依赖** —— Keychain（不需要 pair 密钥；Agent 持有）、reqwest（不连 worker）、webrtc-rs（不发 WebRTC）。

**配置 & 运行时**：

```
~/Library/Application Support/macagent/
├── launchers.json5         # 用户可编辑的 launcher 列表
├── agent.json5             # delta_interval_ms / keyframe_interval_ms 等可调参数
├── agent.sock              # Unix socket（运行时创建）
└── sessions/<sid>.log      # 每 session 的 append-only log（用于审计/M4 notify）
```

#### GuiCapture
- **目的** —— 管理监管集合；用 ScreenCaptureKit 抓取唯一的 active 窗口；用 VideoToolbox 编码 H.264 NALU；交付到 `RtcPeer.video`。
- **接口** —
  - `list_windows() → [WindowInfo]`
  - `list_launchable_apps() → [BundleInfo]`（白名单）
  - `supervise_existing(window_id, viewport) → SupervisionId`
  - `supervise_launch(bundle_id, viewport) → SupervisionId`
  - `list_supervised() → [SupervisionEntry]`
  - `switch_active(supervision_id, viewport)`
  - `remove_supervised(supervision_id)`
  - `fit_window(supervision_id, viewport)`（在 switch 时和 iOS 端旋转时内部调用）
  - `restore_window(supervision_id)`
  - `on_window_dead(cb)`
- **状态** —— `Vec<SupervisionEntry { id, source, status: armed|active|dead, original_frame, last_jpeg_thumb }>`，上限 8；超限返回 `ErrSupervisionLimit`，iOS UI 提示用户先移除一个。
- **添加策略** —— `supervise_existing` 与 `supervise_launch` 都"添加 + 立即切为 active"。（空集合 → 第一次添加即 active；非空 → 旧 active 退为 armed。）
- **切换** —— 关掉旧 active 的 ScreenCaptureKit；在新 active 上启动新 ScreenCaptureKit；复用同一条 WebRTC video track（不重新 SDP 协商）。
- **帧率策略** —— Active = 30 fps；armed = 不编码（切出时缓存一张 JPEG 缩略图）；当 iOS app 上报后台时编码暂停但 PeerConnection 保留。
- **依赖** —— `screencapturekit-rs`、VideoToolbox（`objc2-video-toolbox` 或 `core-foundation` FFI）、AppKit（`NSWorkspace.openApplication`）、Accessibility（`accessibility-rs` 风格封装）、Screen Recording 权限。

#### InputInjector
- **目的** —— 把"(window_id, 归一化坐标, 事件)"或键盘组合翻译成针对特定窗口的 CGEvent 注入。提供长文本粘贴快速通道。
- **接口** —
  - `click(window_id, x_norm, y_norm, button)`
  - `scroll(window_id, dx, dy)`
  - `type_text(window_id, text)`
  - `key_combo(window_id, modifiers, key)`
  - `paste_text(window_id, text)`
- **行为** —
  1. 通过 Accessibility / `CGWindowListCopyWindowInfo` 解析窗口 frame。
  2. 调用 `NSRunningApplication.activate(with: .activateIgnoringOtherApps)` 让窗口拿焦。
  3. 用窗口 frame 把归一化坐标换算成屏幕像素。
  4. 构造并 `CGEventPost`（鼠标 / 滚轮 / 键盘 / unicode）。
- **长文本路径** —— `NSPasteboard.set` + `key_combo([cmd], "v")`。粘贴 250 ms 后尽力恢复原剪贴板内容。
- **依赖** —— CoreGraphics CGEvent、AppKit、Accessibility 权限。

#### ClipboardBridge
- **目的** —— 双向剪贴板同步。
- **接口** —— `on_local_change(cb)`、`set_remote(text)`、`last_history(n) → [String]`。
- **行为** —— 500 ms 轮询 `NSPasteboard.changeCount`；变化时推到 `clip` 通道（仅文本，≤ 1 MB）。维护 5 条内存历史（不持久化）。
- **依赖** —— AppKit `NSPasteboard`、`RtcPeer.clip` 通道。

#### NotifyEngine
- **目的** —— 在 (a) `notify <cmd>` 显式标记命令完成、(b) 每会话正则命中 时触发 APNs 推送。
- **组成** —
  1. **`notify` shim 二进制** —— 安装到用户 `PATH`。它自己跑命令（`fork` + `execvp` + `wait`），让用户 stdin/stdout/stderr 保持原生体验。exec 之前打开 `~/Library/Application Support/macagent/notify.sock` 的 Unix socket，登记 `{cmd_argv, started_at, owning_session_hint}`（hint 来自 `MACAGENT_SESSION_ID` 环境变量；普通 Mac 终端没有该变量也没问题，推送照样发）。子进程退出时 shim 上报 `{exit_code, ended_at}` 并以同样的 exit code 退出。
  2. **正则 watcher** —— per-session 配置（`watch_session(id, regex, name)`）；Agent 跨重连保留直到显式移除。
- **接口** —— `register_command(req) → CommandId`（shim 调用）、`report_completion(CommandId, exit_code)`（shim 调用）、`watch_session(id, regex, name)`、`clear_watcher(id, name)`、`list_watchers() → [WatcherInfo]`。
- **推送** —— 调 Worker `POST /push`，载荷 `{pair_id, sig, title, body, deeplink}`。
- **依赖** —— SessionManager 输出流、PairAuth、Worker `/push`。

#### 菜单栏 / 设置 UI（`egui`）
- **目的** —— 配对 QR 显示、权限引导、App 白名单、正则 watcher 配置、会话列表、状态指示（权限失效或后端断开时菜单栏红点）。
- **接口** —— 纯 UI；通过 `tokio::mpsc` channel 读写各模块状态。
- **依赖** —— 上述全部模块。

### 3.2 iOS App（通用 Swift / SwiftUI，iOS 26 + iPadOS 26）

**自适应布局原则**
- 单一 app target，布局上不出现 `#if targetEnvironment(...)` 设备分支。
- 顶层用 `NavigationSplitView`：iPhone 与 iPad 竖屏窄宽度下渲染单列；iPad regular 宽度下渲染 sidebar + detail。
- 所有视图通过 `@Environment(\.horizontalSizeClass)` 做 compact 与 regular 决策；不硬编码"iPhone"或"iPad"。
- v0.1 不支持多 scene / 多 window；单 scene 必须在 iPad Split View、Slide Over、Stage Manager 下不崩溃（场景 resize 不报错；视口在 `geometry` 变化时重算上报）。
- TermView 通过 `UIKeyCommand` 接入硬件键盘（不仅依赖软键盘）。

#### PairingFlow
- AVFoundation QR 扫码。POST `/pair/claim`。pair_secret 写入 Keychain（`kSecAttrAccessibleAfterFirstUnlock`）。

#### RtcClient
- 封装 `GoogleWebRTC` iOS framework。持有一个 `RTCPeerConnection`、若干 observer、渲染器。在所有 DataChannel 之上暴露强类型 Swift API。

#### SessionStore（`@Observable`）
- 订阅 `ctrl` 通道；维护两个列表：(a) Mac Agent 上活跃的 PTY session（`SessionInfo`）；(b) GUI supervision entries（M5+）。处理 `LaunchAck/Reject`、`SessionAdded/Removed`、`TermSnapshot/Delta`、`TermHistory*` 路由到对应 viewport state。

#### TermView（M3 v2，取代旧 CliView）
- **不依赖 SwiftTerm**——producer 端 alacritty 已经把 ANSI 解析完了，iOS 收到的是结构化 `TermSnapshot/Delta { lines: [{ index, runs: [{text, fg, bg, bold, italic, underline, inverse}], wrapped }] }`。
- 用 SwiftUI `Text` + `AttributedString`（或 `LazyVStack` 行渲染）按 run 上色拼接。
- 底部 `InputBar` 子视图：软键盘文字输入 → `Input { TerminalInput::Text }`；快捷键栏（Tab / Esc / Ctrl+C / Ctrl+D / Arrow×4 / Ctrl+R / 等）→ `Input { TerminalInput::Key }`；右端 ✏️ 按钮打开 `ComposeSheet`（多行 / 中文 IME / 第三方键盘语音输入；参见 ios-input-compose-design.md）。
- 离屏 `HistoryView` 子视图：纯文本 `ScrollView`，从 `TermHistorySnapshot/Append` 累积。
- `UIKeyCommand` 把硬件键盘的 Tab / Esc / Ctrl+C / 方向键也映射成对应 `TerminalInput::Key`。
- `geometry` 改变时触发 `Resize { sid, cols, rows }` 上报。

#### SessionListView
- 显示 launchers（Mac Agent 配置的白名单）作为可点击按钮 → 发 `LaunchSession { launcher_id }`。
- 显示 active sessions（来自 `SessionStore.list()`）→ 点击进入对应 TermView。
- 状态指示：streaming / not streaming / 已退出。

#### ComposeSheet（详细见 [`2026-04-30-ios-input-compose-design.md`](2026-04-30-ios-input-compose-design.md)）
- 多行 `TextEditor` sheet，顶部 title bar + Cancel，底部 Send 按钮。
- 由 CliView 与 GuiStreamView 共用：仅 `onSend` 闭包不同（CLI → `pty/<id>` 字节流 / GUI → `input.paste_text`）。
- 中文 IME / 系统语音 / 第三方键盘语音（微信 / 搜狗 / 讯飞）全部由 UIKit 标准 `UITextView` 天然支持，本组件无需感知输入法。
- 不做自动 `\n`、不做命令历史、不做模板（明确简化）。Send 后清空 + dismiss；Cancel 不发送、不持久化。
- M4 仅落地 CLI 路径；GUI 路径在 M5/M6 时与 InputInjector.paste_text 一并接通。

#### GuiStreamView
- 用 `RTCMTLVideoView` 渲染 active 流。手势识别映射：
  - 单指点击 → `click(left, x, y)`。
  - 长按 → `click(right, x, y)`。
  - 双指 pan → `scroll(dx, dy)`。
  - Pinch → 保留（v0.1 no-op；未来可映射 `Cmd+=`/`Cmd+-`，但目前显式按钮覆盖）。
  - 软键盘 → `type_text`。
  - `+ / − / ⌖` 按钮 → `key_combo([cmd], "=" / "-" / "0")`。
  - ✏️ 按钮 → 打开 `ComposeSheet`（共用 CliView 的 IME / 语音多行编辑组件）→ Send 时发 `input.paste_text`（M5/M6 实现，M4 仅 CLI 路径）。
- 在出现和旋转时上报可渲染视口尺寸（point × scale）；作为 `switch_active` / `supervise_*` 的一部分发送。

#### ClipboardPanel
- 远端剪贴板的只读镜像 + 复制到 iOS 按钮。Local → Mac 必须显式按"发送到 Mac"。本地 5 条内存历史。

#### PushHandler
- 申请 APNs entitlement；启动时把 device token 注册到 Worker。点击通知 deep-link 到对应 session 的 TermView。

### 3.3 Cloudflare Workers（TypeScript）

单 Worker，含一个 Durable Object 类（`SignalingRoom`）、一个 KV namespace（`pairs`）、五个 secret（`APNS_AUTH_KEY`、`APNS_KEY_ID`、`APNS_TEAM_ID`、`CF_CALLS_APP_ID`、`CF_CALLS_APP_SECRET`）。

| 路由 | 职责 |
|---|---|
| `POST /pair/create` | Mac Agent 注册；Worker 生成 `pair_token`（约 6 字符短码 + 256-bit 密钥），写 KV 5 分钟 TTL，返回 token 与 `room_id`。 |
| `POST /pair/claim` | iOS 提交 `{pair_token, ios_pubkey, apns_token}`。Worker 校验 token、派生 `pair_id`、把 `{pair_id, mac_pubkey, ios_pubkey, apns_token}` 写入 KV，并通过 `SignalingRoom` DO 通知正在等的 Mac Agent 取走 iOS pubkey。 |
| `WS /signal/:pair_id` | 两端通过签名握手认证。DO 中继 SDP 与 ICE candidate。空闲 5 分钟 → DO hibernate。 |
| `POST /push` | Mac Agent 发送签名 push 请求。Worker 取出该 pair 的 `apns_token`，用 secret 签 APNs JWT，调 APNs HTTP/2 投递。处理 410 Unregistered：在 KV 标记 token 死亡。 |
| `POST /turn/cred` | 返回短期（1 小时）TURN 凭证（通过 Cloudflare Calls 签发）。两端在过期前 5 分钟 prefetch。 |

KV schema（key → value）：

```
pair_token:<token>     → { mac_pubkey, room_id, expires }            (5 分钟 TTL)
pair:<pair_id>         → { mac_pubkey, ios_pubkey, apns_token, ... } (无 TTL)
apns_dead:<pair_id>    → { reason, since }                           (410 时写)
```

---

## 4. 数据流

### 4.1 首次配对（QR）

```
Mac Agent                    Cloudflare Worker                    iOS App
   │                                │                                │
   │ 1. POST /pair/create           │                                │
   │   {mac_pubkey}                 │                                │
   ├───────────────────────────────►│                                │
   │ 2. {pair_token, room_id}       │ KV.put(pair_token, …, 5m TTL)  │
   │◄───────────────────────────────│                                │
   │                                │                                │
   │ 3. WS /signal/:room_id (等待)  │                                │
   ├───────────────────────────────►│                                │
   │                                │                                │
   │ 4. egui 显示 QR(pair_token,    │                                │
   │    room_id, worker_url)        │   5. 用户扫码                  │
   │                                │   ◄────────────────────────────│
   │                                │                                │
   │                                │   6. POST /pair/claim          │
   │                                │      {pair_token, ios_pubkey,  │
   │                                │       apns_token}              │
   │                                │◄───────────────────────────────│
   │                                │ 7. KV.put(pair:<pair_id>, …)   │
   │                                │                                │
   │ 8. DO push: {peer_joined,      │                                │
   │    ios_pubkey, pair_id}        │  9. {pair_id, mac_pubkey,      │
   │◄───────────────────────────────│      worker_url}               │
   │                                │───────────────────────────────►│
   │                                │                                │
   │ 10. Keychain.persist(...)      │  10. Keychain.persist(...)     │
```

`shared_secret` 通过 X25519 ECDH 从双方公钥派生；Worker 看到公钥但不持有共享密钥。后续所有控制平面消息都附带 `HMAC(shared_secret, body)`。

**`room_id` 与 `pair_id` 切换语义**：第 3 步 Mac Agent 的 WebSocket 开在临时的 `room_id` 上（匿名，仅在 claim 成功或 5 分钟 token 过期前有效）。第 8 步之后双方都关掉 `room_id` 上的 WS，重新连接到 `/signal/:pair_id`，即基于持久 pair 记录的长期信令通道。

### 4.2 后续连接

iOS 启动 → 双方各自 WebSocket 连到 `/signal/:pair_id`（DO 若已 hibernate 则 wake）→ 各自调 `/turn/cred` 取 TURN 凭证 → iOS 创建 SDP offer，经 DO 中继到 Mac → Mac 回 answer → ICE candidates 双向交换 → DTLS-SRTP 完成 → DataChannel 全部打开 → DO 进入 idle（WS 空闲 5 分钟后 hibernate）。

### 4.3 CLI 会话生命周期（M3 v2 producer 模型）

```
iOS                            Mac Agent (menu bar)            Producer (Terminal.app 窗口)
 │ ctrl: {LaunchSession,         │                              │
 │        launcher_id:"claude-   │                              │
 │        code", req_id:"r1"}    │                              │
 ├──────────────────────────────►│ Launcher.load_config 查 argv  │
 │                               │ → osascript open Terminal.app │
 │                               │   with `macagent run          │
 │                               │     --launcher-id claude-code`│
 │                               │                              │ ── 用户看见新窗口冒出 ──
 │                               │                              │ macagent run:
 │                               │                              │   连 agent.sock
 │                               │ socket: ProducerHello {       │   fork PTY (claude code)
 │                               │   argv, pid, cols, rows }     │   alacritty Term 初始化
 │                               │◄──────────────────────────────│
 │                               │ ProducerRegistry.register     │
 │                               │ → sid="s1"                    │
 │                               │ socket: ProducerWelcome {     │
 │                               │   sid: "s1" }                 │
 │                               ├──────────────────────────────►│
 │ ctrl: {LaunchAck, req_id:"r1",│                              │   PTY → 本机 tty (用户可见)
 │        sid:"s1"}              │                              │   PTY → alacritty Term
 │◄──────────────────────────────│                              │
 │                               │                              │
 │ ctrl: {AttachSession, sid:"s1"}                              │
 ├──────────────────────────────►│ SessionRouter.attach_iOS(s1)  │
 │                               │ socket: 通知 producer streaming│
 │                               ├──────────────────────────────►│
 │                               │                              │   每 50ms 算 diff
 │                               │ socket: TermDelta {           │   → TermDelta
 │                               │   revision, lines }           │
 │                               │◄──────────────────────────────│
 │ ctrl: TermDelta {sid, ...}    │ session_router 包成 SignedCtrl│
 │◄──────────────────────────────│                              │
 │                               │                              │
 │ TermView 拼 lines/runs 渲染    │                              │
 │                               │                              │
 │ ctrl: Input {sid, Text "ls\r"}│                              │
 ├──────────────────────────────►│ socket: Input {Text "ls\r"}  │
 │                               ├──────────────────────────────►│
 │                               │                              │   PTY.write("ls\r")
 │                               │                              │   PTY 回 "$ ls\r\n..."
 │                               │ socket: TermDelta             │   alacritty 解析
 │                               │◄──────────────────────────────│
 │ ctrl: TermDelta               │                              │
 │◄──────────────────────────────│                              │
 │                               │                              │
 │  ── 用户在 Terminal 直接输入 ─┼──────────────────────────────► PTY (chaos OK，与 iOS 输入交错)
 │                               │                              │
 │  ── iOS 后台/网络中断 ─────────│                              │
 │                               │ producer 继续跑（Terminal 仍开）│
 │  ── iOS 回到前台，重新 attach ──                              │
 │ ctrl: AttachSession           │                              │
 ├──────────────────────────────►│                              │
 │                               │ 立即推一次最新 TermSnapshot   │
 │                               │ (alacritty Term 当前 grid)    │
 │ ctrl: TermSnapshot            │                              │
 │◄──────────────────────────────│                              │
 │                               │                              │
 │  ── 用户 Cmd+W 关 Terminal ──                                 │
 │                               │ socket disconnect             │
 │                               │◄──────────────────────────────│
 │                               │ ProducerRegistry.unregister   │
 │ ctrl: SessionRemoved {        │                              │
 │   sid:"s1", reason:           │                              │
 │   "window_closed"}            │                              │
 │◄──────────────────────────────│                              │
```

**关键点**：

- iOS 离线时**不需要** ring buffer / replay 机制——producer 一直在 Terminal.app 窗口里跑（用户可能也在看），alacritty Term 持续维护当前 grid 状态。iOS 重连只需 `AttachSession` 触发一次 `TermSnapshot` 全量帧即可。
- 离屏 history（>scrollback 行数被推出 viewport 的）由 `TermHistorySnapshot/Append` 单独通道推；iOS 端的 HistoryView 累积。
- 用户在 Mac Terminal 里输入 + iOS 也在输入：两路字节都进 PTY stdin，**交错混合**——这是有意设计（hurryvc 风格的 chaos）。iOS UI 显示一个不显眼标签 `🟢 Mac 端有人` 提示。
- 产品特性"60s 断网重连"在 v2 模型里**自动满足**：alacritty Term 一直在维护，无需 replay。
- `Input { sid, payload: TerminalInput::Text }` 既承载 inline live-typing（每个按键即发），也承载 ComposeSheet Send 整段字节，传输层无差别。

### 4.4 GUI 监管与流送

```
iOS                                       Mac Agent
 │ ctrl: {list_windows}                    │
 ├────────────────────────────────────────►│ GuiCapture.list_windows()
 │ ctrl: {windows: [{id:42, app:"Chrome",  │
 │   title:"…", w:1440, h:900}, …]}        │
 │◄────────────────────────────────────────│

 ── 路径 A：监管已开窗口 ──
 │ ctrl: {supervise_existing, win:42,      │
 │        viewport:{w:393, h:760}}         │
 ├────────────────────────────────────────►│ GuiCapture.supervise_existing(...)
 │                                         │   记录 original frame
 │                                         │   AX fit_window（aspect-fit）
 │                                         │   start ScreenCaptureKit on 42
 │                                         │   active = supId
 │ ctrl: {supervised, sup:s2, active:true} │
 │◄────────────────────────────────────────│
 │  ── 视频帧通过 SRTP 流到 iOS ──         │

 ── 路径 B：启动白名单 App ──
 │ ctrl: {supervise_launch,                │
 │        bundle:"com.openai.chat",        │
 │        viewport:{...}}                  │
 ├────────────────────────────────────────►│ NSWorkspace.openApplication
 │                                         │ 等 windowDidBecomeKey
 │                                         │ supervise_existing(new_win, vp)
 │ ctrl: {supervised, sup:s3, active:true} │
 │◄────────────────────────────────────────│

 ── 切换 active ──
 │ ctrl: {switch_active, sup:s2,           │
 │        viewport:{...}}                  │
 ├────────────────────────────────────────►│ stop ScreenCaptureKit on prev
 │                                         │ fit_window on new active
 │                                         │ start ScreenCaptureKit on new
 │                                         │ 复用同一 video track
 │ ctrl: {active_changed, sup:s2}          │
 │◄────────────────────────────────────────│  (≈ 200 ms)

 ── 输入 ──
 │ input: {tap, win:..., x:0.42, y:0.71}   │ InputInjector.click(...)
 │ input: {scroll, win:..., dx:0, dy:-120} │ InputInjector.scroll(...)
 │ input: {key_combo, win:..., mods:[cmd], │
 │         key:"="}                        │ InputInjector.key_combo(...)
 │ input: {paste_text, win:..., text:"…"}  │ NSPasteboard.set + Cmd+V
```

iOS 旋转 / Stage Manager 改尺寸 → 通过 `ctrl: {viewport_changed, sup, viewport}` 上报；Agent 重新 `fit_window`。`remove_supervised` 调 `restore_window` 还原原始 frame。

### 4.5 剪贴板同步

- **Mac → iOS** —— ClipboardBridge 每 500 ms 轮询 `NSPasteboard.changeCount`；变化时读 `string(forType: .string)`（≤ 1 MB），推 `{clip_set, text}` 到 `clip`。
- **iOS → Mac** —— 仅在用户按 `ClipboardPanel` 的"发送到 Mac"时触发。发 `{clip_set, text}` 到 `clip`，Agent 调 `NSPasteboard.set`。
- 双方各保留 5 条内存历史；不上后端、不持久化。

### 4.6 通知（notify 命令 + 正则 watcher）

```
用户 in CLI                Mac Agent                  Worker            APNs        iOS
   │ $ notify pnpm build      │                          │                │           │
   │  (shim → Unix socket)    │                          │                │           │
   ├─────────────────────────►│ register_command(...)    │                │           │
   │                          │   → CommandId            │                │           │
   │  (shim fork+exec)        │                          │                │           │
   │   …5 分钟后：exit 0…     │                          │                │           │
   │  (shim → socket)         │                          │                │           │
   ├─────────────────────────►│ report_completion(0)     │                │           │
   │                          │ POST /push (signed)      │                │           │
   │                          │   {pair_id, title:"build │                │           │
   │                          │   done", body:"…", sid}  │                │           │
   │                          ├─────────────────────────►│                │           │
   │                          │                          │ APNs JWT       │           │
   │                          │                          ├───────────────►│           │
   │                          │                          │                ├──────────►│
   │                          │                          │                │   推送    │
```

正则 watcher 走同一条 `/push`，在 NotifyEngine 检测到会话输出行命中已注册的正则时触发。

---

## 5. 错误处理

仅列出本系统特有的失败模式（通用网络错误等不展开）。

| 失败 | 处理 |
|---|---|
| WebRTC ICE/DTLS 握手失败 | iOS 监听到 `iceConnectionState=failed` → 自动 ICE restart，最多 3 次；持续失败则呈现"连接丢失"UI。Mac 镜像处理。 |
| Worker WebSocket 断开 | 指数退避重连（1 s → 30 s 上限）。已建立的媒体平面不受影响，仅在新建 session / ICE restart 时才用得到。 |
| TURN 凭证 mid-session 过期 | 双方在到期前 5 分钟 prefetch；仅对新 ICE candidate 生效，已有连接不中断。 |
| Mac 睡眠 / Wi-Fi 漫游 | PeerConnection 自动 ICE restart；PTY 不受影响（已从 RtcPeer detach）。重连后 iOS 发 `resume_session(last_seq)`。 |
| PTY 子进程退出 | SessionManager 把 exit code 写到 `ctrl`；session 标 `exited`，保留 30 分钟供查看 backlog；iOS 显示 exit 码徽章。 |
| `notify` shim 连不上 Agent（daemon 没跑） | shim 仍然 exec 命令（不阻塞用户工作流），向 stderr 打一行警告，按命令的 exit code 退出。不发推送。 |
| `notify` shim 中途丢 Agent socket | shim 仍 wait() 子进程并以同样 exit code 退出；推送静默跳过。Agent 重启后不追踪在飞 notify 命令。 |
| ScreenCaptureKit：被监管窗口被关闭 | GuiCapture 收到 stream-end 回调 → 在 `ctrl` 发 `{stream_ended, reason:"window_closed", sup_id}`；iOS UI 回到窗口列表；若是 active 则自动切到下一个 armed，否则进 idle。 |
| 运行时权限被撤销（Screen Recording / Accessibility） | 启动时 + 每次 `start_stream` / `inject_input` 前 `CGPreflightScreenCaptureAccess` / `AXIsProcessTrusted` 预检。失效则菜单栏红点 + 在 `ctrl` 发 `{permission_lost, kind}` 给 iOS。 |
| APNs token 失效（410 Unregistered） | Worker 写 `apns_dead:<pair_id>`。后续 `/push` 跳过投递并返回 `apns_unavailable`；通过 `ctrl` 通知 Mac Agent 让对端重新注册。 |
| pair 被吊销 / `pair_id` 不存在 | Worker 返回 401 + `error: pair_revoked`。Mac Agent 清 Keychain，提示用户重新二维码配对。 |
| InputInjector：目标窗口不在前台 | `NSRunningApplication.activate` 失败 → `{input_dropped, sup_id, reason}`；iOS 提示窗口不可用并回列表。 |
| Ring buffer 溢出 + 日志读不到 | 发 `{backlog_truncated, kept_from_seq:N}`；iOS 渲染"…内容已截断…"分隔条。 |
| 并发 peer（强制 1:1） | 第二条来自第三台设备的 WebSocket 同 `pair_id` 收 `peer_busy`；先到的赢。 |
| AX `fit_window` 被 App 拒绝 | 发 `{fit_failed, reason}`；iOS GuiStreamView 退到 aspect-fit 渲染（letterbox / pillarbox）。 |

---

## 6. 测试策略

| 层 | 工具 | 覆盖 |
|---|---|---|
| Mac Agent 单元 | `cargo test`，对 PTY / ScreenCaptureKit / NSPasteboard / CGEvent 用 `mockall` | Ring buffer 边界、NotifyEngine 正则语义、PairAuth 签名流程、InputInjector 坐标换算、GuiCapture 状态机。 |
| Worker 单元 | `vitest` + `miniflare`（Workers + DO + KV 模拟器） | 配对状态机、签名校验、APNs JWT 生成、TURN 凭证签发、KV TTL 行为。 |
| Worker 集成 | `wrangler dev` + Rust 测试 client 跑全 HTTP/WS 表面 | 配对握手 → DO 中继 → mock APNs 投递。 |
| 端到端（Mac × iOS） | macOS GitHub runner；Mac Agent + iOS simulator（`xcrun simctl`）；脚本化场景 | 配对 → 开 PTY → 写 / 读 → 断网 60 s → resume → 关闭。 |
| WebRTC 媒体回路 | 同一台 macOS runner；1 个 Mac Agent + 1 个用 `webrtc-rs` 写的 mock-iOS 客户端（用帧计数器代替 `RTCMTLVideoView`） | SDP/ICE 握手、video track 流、DataChannel 来回。 |
| GUI App 验收（手动） | 真 iPhone × 真 iPad × 真 Mac 矩阵：ChatGPT Desktop、Claude Desktop、Chrome | 可见、可点击、可滚动、长文本粘贴、supervise / launch / switch；iPad 还需验证 Split View / Stage Manager / 硬件键盘。 |
| 弱网 / 漫游（手动） | Network Link Conditioner + Wi-Fi → 蜂窝切换 | ICE restart 时长、`resume_session` 正确性。 |
| 权限 / onboarding（手动） | 全新 macOS + iOS 安装 | 不读文档 5 分钟内完成配对 + 开第一个 CLI session。 |

不进自动测试范围：APNs 真推送（用 sandbox 手测）、Cloudflare Calls TURN 真凭证（CI mock，每月一次真链路烟测）。

---

## 7. 里程碑

| ID | 范围 | 验收 |
|---|---|---|
| **M0 · 骨架** | Cargo workspace、Xcode 项目、Cloudflare Worker 脚手架；Rust / Swift / Workers 三条 CI；空菜单栏 + 空 iOS App + worker `/health` | 三条流水线全绿；二进制可构建；菜单栏图标出现；iOS 模拟器启动；`wrangler dev` 响应。 |
| **M1 · 配对 + 控制平面** | PairAuth + SignalingClient + Worker `/pair/*` + Durable Object + KV；ECDH 密钥交换；签名 `ctrl` 通道；菜单栏 QR；iOS 扫码流程 | 真 iPhone 配对真 Mac 成功；两端重启都能恢复；revoke 流程跑通。 |
| **M2 · WebRTC 媒体面打通** | webrtc-rs RtcPeer、Cloudflare Calls TURN 凭证、ICE/DTLS 建立、空 `ctrl` DataChannel 心跳 | 跨 NAT WebRTC 建连成功；ICE restart 通过。 |
| **M3 · CLI 通道**（核心交付物 #1，v2 producer 模型） | `macagent run` producer 子命令；alacritty 解析 PTY；Unix socket 报到；Mac Agent 端 ProducerRegistry + AgentSocket + Launcher（含 launchers.json5 + AppleScript）+ SessionRouter；iOS TermView（不依赖 SwiftTerm，直接渲染 lines/runs）+ SessionListView | 在 iPhone / iPad 上跑 Claude Code / Codex / shell；Mac 端用户**看得见** Terminal 窗口；iOS 断网 60 s 重连后 attach 即取最新 grid（无需 replay 历史字节）；8 个并发会话稳定；用户 Cmd+W 关 Terminal 等于杀 session；用户 + iOS 同时输入字节交错入 PTY（chaos 可接受）。 |
| **M4 · 剪贴板 + 通知 + iOS 输入增强** | ClipboardBridge 双向、iOS Clipboard panel、`macagent notify` 子命令、NotifyEngine 正则 watcher、Worker `/push` + APNs；iOS ComposeSheet（多行 / 中文 IME / 第三方键盘语音）落地 CLI 路径 | `macagent notify -- pnpm build` 推送可触达，含 deep-link；5 个正则场景测试通过；真 iPhone + 微信键盘语音输入"git status" → 终端正确执行；中文文本通过 ComposeSheet 输入到 PTY 不丢字。 |
| **M5 · GUI 监管 v0**（核心交付物 #2） | GuiCapture + ScreenCaptureKit 单窗口 + VideoToolbox H.264 + WebRTC video track；仅 `supervise_existing`；只看不点 | 在 iPhone / iPad 上看到 Chrome 30 fps；switch / remove 流程跑通。 |
| **M6 · 输入注入 + 内容缩放** | InputInjector：CGEvent click/scroll/keyboard、`key_combo`、`paste_text`；`input` 通道；Accessibility onboarding | 点 Chrome 网页按钮、滚动、长文本粘贴、对 Chrome 与 Electron 系 App 跑 Cmd+/Cmd-/Cmd0。 |
| **M7 · 启动接管 + 多监管切换 + 窗口适配** | `supervise_launch`、≈ 200 ms 切换、armed 缩略图、AX `fit_window` + `restore_window`、视口感知比例 | 从 iPhone / iPad 启动 Claude Desktop，监管 3 个 App 流畅切换，窗口比例匹配设备视口（覆盖 iPhone 竖横屏与 iPad Split View）。 |
| **M8 · 打磨 + TestFlight** | 弱网恢复、权限引导文案、错误 UI 文案、菜单栏白名单编辑器、code sign + notarize、TestFlight 提交 | 全新用户从下载到第一次"打开 ChatGPT 看回答"在 5 分钟内；TestFlight 内测发布。 |

**依赖链** —— M0 → M1 → M2 →（M3 ‖ M5）→（M4 ‖ M6）→ M7 → M8。

**单人估算** —— M0=1 周、M1=2 周、M2=2 周、M3=3 周、M4=2 周、M5=3 周、M6=2 周、M7=2 周、M8=2 周。**总计约 19 周（约 4.5 个月）。**

---

## 8. 待解问题 / 未来工作（v0.1 之外）

- **B2 GUI**：IME（中文输入法）、子窗口、弹窗、文件选择器、拖拽。
- **多 Mac / 多 iOS 设备**：数据模型已支持，UI 推迟。
- **会话穿越 macOS 重启**：需要 `dtach` 风格的 PTY 持久化。性价比低，暂不做。
- **账号层**：未来引入团队 / 付费功能时，把 Sign-in-with-Apple 接到 pair_id 模型上。
- **数据分析 / 可观测性**：推迟。Worker 自带日志，Mac Agent 本地调试日志即可，不上遥测。
- **自建后端 fallback**：若后端成本或合规出问题，Worker 逻辑可移植到 axum 服务器 + coturn（早期 brainstorm 的 R3 方案）。
