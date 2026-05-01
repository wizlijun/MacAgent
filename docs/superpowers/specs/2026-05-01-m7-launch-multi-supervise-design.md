# M7 · Launch + Multi-supervise + Window adaptation 设计文档

> 对接 spec：`docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §M7（line 592）+ supervision API（lines 208–219）+ 窗口适配（line 25）+ 错误回退（line 560）。

## 1. Scope

**IN（v0.1 M7）**：
- ctrl 协议：`SuperviseLaunch`、`SwitchActive`、`ViewportChanged`、`FitFailed`；`SupervisionEntry` 扩 `status / original_frame / thumb_jpeg_b64`；`Viewport / WindowRect / SupStatus` 类型
- Mac `Launcher`：白名单 bundle (`com.openai.chat`、`com.anthropic.claude`、`com.google.Chrome`) 启动 + 窗口探测（200ms 轮询，5s 超时）
- Mac `WindowFitter`：AX `kAXSizeAttribute` 调窗 + `original_frame` 缓存 + `restore_window`；失败发 `FitFailed`
- Mac `SupervisionRouter` 升级：多 entry 注册表（≤8）、stop-old + start-new 切换（目标 ≤200ms）、JPEG 缩略图捕捉（最后一帧 → CGImage → JPEG Q70 → base64）
- iOS：`SupervisionStore.entries` 多元素、缩略图 tile grid（iPad 3 列 / iPhone 2 列）、`switch_active` 触发、viewport 旋转/几何变化上报、`fit_failed` toast
- iOS `LaunchAppSheet`：白名单选择 UI

**OUT（M8 或更后）**：
- 缩略图大小自适应（v0.1 固定 256×192 @ Q70，base64 内联在 SupervisionList ctrl）
- "录像式" history（armed 期间发生的事不重放）
- 鼠标手势在 armed 上预览
- 多 active 流并发（永远 1 个 active）
- 用户自定义白名单 App（v0.1 hardcoded 3 个）
- 上限 8 的 UI 编辑器（M8 polish）
- 多屏 / multi-display 坐标系
- 同 bundle 多窗口选择（`supervise_launch` 取第一个新出现的）
- AX permission 第二轮引导（M6 已落地，M7 不重做）

## 2. Protocol

### 2.1 Mac `ctrl_msg.rs` 新增

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport { pub w: u32, pub h: u32 }   // points (not pixels)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupStatus { Active, Armed, Dead }

// CtrlPayload 新增 4 个 variant：
SuperviseLaunch { bundle_id: String, viewport: Viewport },
SwitchActive    { sup_id: String, viewport: Viewport },
ViewportChanged { sup_id: String, viewport: Viewport },
FitFailed       { sup_id: String, reason: String },
```

### 2.2 `SupervisionEntry` 扩字段

```rust
pub struct SupervisionEntry {
    pub sup_id: String,                          // existing
    pub window_id: u32,                          // existing
    pub app_name: String,                        // existing
    pub title: String,                           // existing
    pub width: u32,                              // existing (M6.1)
    pub height: u32,                             // existing (M6.1)
    pub status: SupStatus,                       // NEW: armed/active/dead
    pub original_frame: Option<WindowRect>,      // NEW: pre-fit frame for restore
    pub thumb_jpeg_b64: Option<String>,          // NEW: last frame as JPEG, set on demote-to-armed
}
```

### 2.3 iOS Swift 镜像

`CtrlMessage.swift` 加 4 个 case + `Viewport / WindowRect / SupStatus` 类型 + `SupervisionEntry` 扩字段。HMAC 走现有 `canonicalBytes()`（递归排序 M3.fix 已支持）。

### 2.4 协议要点

- `SwitchActive` 必须带 viewport（iOS 已知当前 geometry，省一次 round-trip 让 Mac 立即 fit）
- `ViewportChanged` 仅给当前 active sup_id（armed entries 不需要 fit）
- `FitFailed` 后 active stream 不停，iOS 端 letterbox（M5 `aspectRatio(.fit)` 已经处理）
- `SuperviseLaunch` 成功后 Mac 发 `SupervisedAck`（已有 ctrl）+ 立即变 active；失败发 `SuperviseReject { code: "launch_failed" | "launch_timeout" | "bundle_not_allowed" }`

## 3. Mac architecture

### 3.1 Launcher（新模块 `mac-agent/crates/macagent-app/src/launcher_m7.rs`）

> 注意：现有 `launcher.rs` 是 M3 producer launcher（agent.json5 命令启动），M7 launcher 是 GUI app 启动，模块拆开避免命名冲突。

```rust
const ALLOWED_BUNDLES: &[&str] = &[
    "com.openai.chat",
    "com.anthropic.claude",
    "com.google.Chrome",
];

pub fn is_allowed(bundle_id: &str) -> bool { ALLOWED_BUNDLES.contains(&bundle_id) }

pub async fn launch_and_find_window(bundle_id: &str) -> Result<(i32, u32)> {
    if !is_allowed(bundle_id) {
        return Err(anyhow!("bundle_not_allowed"));
    }
    // 1. NSWorkspace.shared.openApplication(at: bundleURL, configuration: ...)
    //    → 用 NSWorkspace.urlForApplication(withBundleIdentifier:) 拿 URL
    //    → 拿 NSRunningApplication 取 pid
    // 2. 5s 内每 200ms 轮询 CGWindowListCopyWindowInfo
    //    过滤：owner_pid == pid，title 非空，bounds w*h > 100*100
    // 3. 返回 (pid, window_id) 或 Err("launch_timeout")
}
```

### 3.2 WindowFitter（新模块 `window_fitter.rs`）

```rust
pub fn fit(window_id: u32, owner_pid: i32, viewport: Viewport) -> Result<WindowRect> {
    // 1. AXUIElementCreateApplication(pid)
    // 2. 通过 kAXWindowsAttribute 拿 [AXUIElement]
    //    → 通过 CGWindowList 已知 frame，从 AX 列表中找 bounds 最近的（启发式）
    //      OR 用私有 _AXUIElementGetWindow 直接匹配 windowNumber（更可靠但私有）
    //      ⇒ 选启发式（私有 API M8 再考虑）
    // 3. 读 kAXSizeAttribute + kAXPositionAttribute → cache as original
    // 4. 计算 target size（v0.1 最简策略）：
    //      - 保持 target_w = original.w
    //      - target_h = original.w * (viewport.h / viewport.w)
    //      - Clamp 到 [400×300, 1920×1200]
    //    （不做"保持较大边"或"扩张/收缩择优"，M8 polish 再优化）
    // 5. AXUIElementSetAttributeValue(window_ax, kAXSizeAttribute, target)
    // 6. 返回 original
}

pub fn restore(window_id: u32, owner_pid: i32, original: WindowRect) -> Result<()> {
    // 同上找到 AXUIElement，set size + position → original
}
```

失败（AX 拒绝、窗口非可调）抛 `Err("fit_denied: <detail>")`。

### 3.3 SupervisionRouter 升级

```rust
struct Registry {
    entries: HashMap<String, SupervisionEntry>,
    active_sup: Option<String>,
}

impl SupervisionRouter {
    pub async fn handle_supervise_existing(&self, window_id: u32, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_supervise_launch(&self, bundle_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_switch_active(&self, sup_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_viewport_changed(&self, sup_id: String, viewport: Viewport) -> Result<()> { ... }
    pub async fn handle_remove_supervised(&self, sup_id: String) -> Result<()> { ... }
}
```

`set_active(sup_id, viewport)` atomic：
1. 若有当前 active：调 `gui_capture.demote_to_armed(active_sup)` → 该方法 stop SCStream + 取最后一帧 → JPEG（在 stream.rs 新加） → 写回 entry.thumb_jpeg_b64，状态改 Armed
2. `window_fitter::fit(new_sup_id.window_id, viewport)` → 写 entry.original_frame；失败发 `FitFailed`，继续
3. `gui_capture.start(new_sup_id, window_id, video_track, &cfg)` → 复用 M5 path
4. `active_sup = Some(new_sup_id)`，状态改 Active
5. 发 `SupervisionList`（更新后的所有 entries 给 iOS）

注册上限：`entries.len() >= 8` 时 `register_*` 返 `ErrSupervisionLimit`，发 `SuperviseReject { code: "supervision_limit" }`。

`remove(sup_id)`：调 `window_fitter::restore` + `gui_capture::stop` + entries.remove。若 remove 的是 active，自动 set_active 到下一个 armed（list 顺序）；都没了 → `active_sup = None`。

### 3.4 GuiCapture 加缩略图

`gui_capture/stream.rs::ActiveStream` 加：
```rust
last_frame: Arc<Mutex<Option<CVPixelBuffer>>>,
```

`FrameSink::did_output_sample_buffer` 在 push 到 frame_tx 之前 swap 一份到 `last_frame`（CFRetain 是廉价的）。

`stop_with_thumbnail()`：
1. 调 `stop()` 路径
2. 拿 `last_frame.lock().take()` → CGImage 转换 → JPEG（`objc2-image-io` 或者 `core-graphics::CGImageDestination` + `kUTTypeJPEG`）@ Q0.7 → base64 string
3. 返回 `Option<String>`

JPEG 编码走 CoreGraphics ImageIO（`CGImageDestinationCreateWithData` + `kUTTypeJPEG` + 压缩选项 Q0.7）。Rust 绑定优先用现有 deps（`objc2-core-graphics` / `objc2-image-io`，按 plan 阶段实测 lock 哪个最少改动）。如果 ImageIO 路径出问题，后备：直接用 `objc2_uniform_type_identifiers::UTType` + `objc2_image_io::CGImageDestination` 写到 `NSMutableData`。Plan M7.X 会 pin 具体 crate。

JPEG 缩略图目标尺寸：256×192（4:3 视觉感知一致）。先 CGImage scale，再 encode。~10–25KB，base64 后 ~13–33KB；8 entries 全 armed 缩略图 ≤270KB inline 在 SupervisionList ctrl 上，DataChannel 不会 chunk 问题。

## 4. iOS UX

### 4.1 SupervisionGrid（替代 WindowListView 的列表布局）

```swift
struct SupervisionGrid: View {
    @Bindable var store: SupervisionStore
    @Environment(\.horizontalSizeClass) private var hSizeClass

    var columns: Int { hSizeClass == .compact ? 2 : 3 }

    var body: some View {
        ScrollView {
            LazyVGrid(columns: ..., spacing: 12) {
                ForEach(store.entries) { entry in
                    SupervisionTile(entry: entry, store: store)
                }
                if store.entries.count < 8 { AddTile(store: store) }
            }
            .padding(12)
        }
        .navigationTitle("\(store.entries.count) / 8 监管中")
    }
}

struct SupervisionTile: View {
    let entry: SupervisionEntry
    @Bindable var store: SupervisionStore

    var body: some View {
        VStack {
            ZStack {
                if entry.status == .active {
                    GuiStreamView(videoTrack: store.activeTrack)
                        .aspectRatio(...)
                } else if let b64 = entry.thumbJpegB64,
                          let data = Data(base64Encoded: b64),
                          let img = UIImage(data: data) {
                    Image(uiImage: img).resizable().aspectRatio(.fit)
                } else {
                    Image(systemName: "rectangle.dashed").font(.largeTitle)
                }
                if entry.status == .active {
                    Color.green.frame(height: 3).offset(y: 30)  // active 标记条
                }
            }
            Text(entry.appName).font(.caption)
            Text(entry.title).font(.caption2).lineLimit(1)
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
}
```

`GuiStreamDetailView`（M6.9）继续作为「点击 active tile → 进入全屏 + 输入栏」入口。Tile grid 是入口列表。

### 4.2 LaunchAppSheet

```swift
struct LaunchAppSheet: View {
    let store: SupervisionStore
    @Environment(\.dismiss) var dismiss
    
    private static let bundles: [(id: String, name: String, icon: String)] = [
        ("com.openai.chat", "ChatGPT", "bubble.left"),
        ("com.anthropic.claude", "Claude", "sparkles"),
        ("com.google.Chrome", "Chrome", "globe"),
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
        }
    }
}
```

`AddTile` 显示 `+`，tap 弹出 actionSheet：「监管现有窗口」（→ M5 路径）/「启动 App」（→ LaunchAppSheet）。

### 4.3 ViewportTracker

```swift
struct ViewportTracker: ViewModifier {
    let store: SupervisionStore

    func body(content: Content) -> some View {
        content.background(GeometryReader { geo in
            Color.clear
                .onAppear { store.reportViewport(w: geo.size.width, h: geo.size.height) }
                .onChange(of: geo.size) { _, new in
                    store.reportViewport(w: new.width, h: new.height)
                }
        })
    }
}
```

附在 `GuiStreamDetailView` 上；`store.reportViewport` 内部判断是否有 active sup，有则发 `viewport_changed`。Stage Manager / Split View 由 GeometryReader 自动捕获。

### 4.4 fit_failed 处理

`SupervisionStore` 收到 `FitFailed` ctrl → set `lastFitFailed = (sup_id, reason)` → `GuiStreamDetailView` 顶部 toast 5s 自动消失：「无法调整窗口尺寸（letterbox 显示）」。Stream 不停，UI 不阻塞。

## 5. Testing & Acceptance

### 5.1 Mac 单测

- `launcher_m7::test_bundle_whitelist` — `is_allowed` 已知/未知 bundle
- `launcher_m7::test_launch_timeout_returns_err` — 用 mock NSWorkspace（test seam）；此点可能跳过，纯 manual
- `window_fitter::aspect_fit_calc` — pure function：given window 1440×900 + viewport 393×760 → target 393×900 or 853×900（pick wider preserve），assert clamp 边界
- `supervision_router::register_eight_then_ninth_rejected` — 注册 8 后第 9 返 `ErrSupervisionLimit`
- `supervision_router::switch_demotes_old_to_armed` — 模拟切换：旧 active.status 应为 Armed，new active.status 应为 Active
- `supervision_router::remove_active_promotes_next_armed` — remove active 后下一个 armed 自动激活

### 5.2 iOS 单测

- `SupervisionStoreTests::switch_active_local_state` — handle SupervisionList ctrl → entries 顺序 + status 正确
- `ViewportTrackerTests::report_only_when_active_exists` — 没 active 时 reportViewport 不发 ctrl

### 5.3 Manual smoke

1. iOS 监管 Chrome 一个窗口（M5 路径）→ 看到画面
2. iOS Add tile → Launch App → 选 Claude Desktop → app 启动 + 窗口自动 fit + 切为 active
3. 注册 3 个监管，tap armed tile → ≤200ms 看到新 active stream + 旧 active 在 tile 显示 JPEG 缩略图
4. iPhone 旋转（active 状态）→ Mac 窗口尺寸变化（高度变化、宽度变化）
5. iPad Split View 拉宽 GuiStreamDetailView → 视口变化 → Mac fit
6. 监管 系统设置 App（不可调窗）→ 收到 `fit_failed` toast + 流仍渲染（letterbox）
7. 监管中关闭 Mac 窗口 → entry status 变 Dead → 自动切到下一个 armed
8. 注册满 8 → AddTile 灰掉 / 选 Add Window 时 SuperviseReject toast「监管数已达上限」

### 5.4 Acceptance（spec line 592）

- ✅ 从 iPhone / iPad 启动 Claude Desktop（manual #2）
- ✅ 监管 3 个 App 流畅切换（manual #3）
- ✅ 窗口比例匹配设备视口（manual #4 + #5，iPhone 竖横屏 + iPad Split View）

## 6. 文件清单

**Mac 新增**：
- `mac-agent/crates/macagent-app/src/launcher_m7.rs`
- `mac-agent/crates/macagent-app/src/window_fitter.rs`

**Mac 修改**：
- `mac-agent/crates/macagent-core/src/ctrl_msg.rs`（4 新 variant + 3 类型 + 3 entry 字段）
- `mac-agent/crates/macagent-app/src/gui_capture/stream.rs`（last_frame capture）
- `mac-agent/crates/macagent-app/src/gui_capture/mod.rs`（demote_to_armed API + JPEG 转换）
- `mac-agent/crates/macagent-app/src/supervision_router.rs`（multi-entry registry，handle_* 方法集）
- `mac-agent/crates/macagent-app/src/ui.rs`（实例化 launcher + fitter，wire 4 新 ctrl variants）
- `mac-agent/crates/macagent-app/src/rtc_glue.rs`（drainer 增 4 新 case 路由到 supervision router）

**iOS 新增**：
- `ios-app/MacIOSWorkspace/Gui/SupervisionGrid.swift`
- `ios-app/MacIOSWorkspace/Gui/LaunchAppSheet.swift`
- `ios-app/MacIOSWorkspace/Gui/ViewportTracker.swift`

**iOS 修改**：
- `ios-app/MacIOSWorkspace/CtrlMessage.swift`（4 新 case + 3 类型 + entry 扩字段）
- `ios-app/MacIOSWorkspace/SupervisionStore.swift`（switch / launch / remove / reportViewport actions + lastFitFailed）
- `ios-app/MacIOSWorkspace/Gui/WindowListView.swift`（替换为 SupervisionGrid 入口或并存）
- `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift`（添加 ViewportTracker modifier + fit_failed toast）

## 7. Risks + Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| AX 找不到对应 window_id（启发式失败） | Medium | fit 不工作 | 启发式比对 frame 距离 < 10pt；失败抛 `fit_denied`，发 `FitFailed`，UI letterbox 兜底 |
| 切换中 SCStream 创建 >200ms（permission re-prompt 等） | Low | 切换有顿挫 | M5.2.5 实测 50–100ms；Activity Monitor 监控；超过 200ms 是 manual smoke 失败但非 critical |
| JPEG 编码 CFData / CGImageDestination FFI 出错 | Low | 缩略图缺失 | thumb_jpeg_b64 是 `Option<String>` —— None 时 iOS 显示占位图标，不影响功能 |
| `NSWorkspace.openApplication` 异步行为 + window 出现时机 | Medium | launch 5s 超时 | 5s 超时给足；Claude/Chrome 冷启动 ≤3s，热启动 ≤1s |
| 同一 bundle 多窗口（Chrome 多个普通窗口）选错 | Medium | 选到错的窗口 | v0.1 选第一个新出现的（按发现顺序）；OUT 项明确 |
| Viewport 上报频次过高（拖动 Stage Manager 中） | Low | 网络抖动 | iOS 端 200ms 防抖（Combine debounce） |
| 注册满 8 后 launch 一次仍要发 SuperviseReject | Low | UI race | iOS 端先 check `entries.count`；仍以 Mac SuperviseReject 为准 |
| `restore_window` 在 App 已退出时失败 | Low | 无影响 | 静默忽略 `Err`；无副作用 |

## 8. Out of Scope（M7 explicitly does NOT do）

- 缩略图自适应大小 / 高质量预览（v0.1 256×192 @ Q70 固定）
- 切换中过渡动画（iOS 端瞬切）
- 多 active 流 / 画中画 / 平铺多窗口
- 多 display / Spaces / Mission Control 集成
- App 切到第三方 Space 时跟随
- supervised_launch 启动参数 / URL scheme（v0.1 只 openApplication）
- 用户编辑白名单（v0.1 hardcoded）
- 同 bundle 多窗口选择器
- "录像式" history / armed 期间事件回放
- TestFlight 文案 / 错误代码本地化（M8）
