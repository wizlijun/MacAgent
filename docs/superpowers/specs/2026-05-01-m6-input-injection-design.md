# M6 · 输入注入 + 内容缩放 设计文档

> 对接 spec：`docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §M6（line 591）+ InputInjector（line 222–236）。

## 1. Scope

**IN（v0.1 M6）**：
- iOS 手势 → ctrl `Input` payload：tap / scroll / key_text / key_combo
- Mac `InputInjector` 模块：订阅 Input ctrl，校验，调 CGEvent 注入
- 权限预检 `AXIsProcessTrusted`；未授权时 Mac 发 `InputAck { code: "permission_denied" }`
- iOS 软键盘场景：单击发 tap、单指拖发 scroll、工具栏按钮发 Cmd+/Cmd-/Cmd0、修饰键吸附栏 + 特殊键栏、InputBar 长文本走 `ClipboardSet + KeyCombo Cmd+V`（复用 M4）
- iOS 外接键盘场景：`HardwareKeyController` (`UIPress` 覆盖) 直 forward 全部按键
- 注入前 `NSRunningApplication.activate(target_pid)` 把目标窗口前台化

**OUT（M7+ 或更后）**：
- 双指捏合手势 → Cmd+/-（v0.1 用工具栏按钮）
- 长按 → 二级点击 / 拖动选择文本 / 三指手势 / drag-and-drop / 自定义快捷键 preset / 录制宏
- F1–F12、Page Up/Down、Home/End
- 多屏 multi-display 坐标系
- iPadOS 系统保留键（Cmd+H / Cmd+Tab / Cmd+Space）拦截
- iPad ↔ Mac 键盘 layout 翻译（Option+E 死字符等）
- TestFlight 文案（M8）
- `fit_window` / `restore_window` / `supervise_launch`（M7）

## 2. Protocol

`mac-agent/crates/macagent-core/src/ctrl_msg.rs` `CtrlPayload` 新增：

```rust
Input {
    sup_id: String,
    payload: GuiInput,
}
InputAck {
    sup_id: String,
    code: String,            // "ok" | "permission_denied" | "no_focus" | "window_gone" | "throttled"
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuiInput {
    Tap      { x: f32, y: f32 },                   // 0..1 normalized in window content rect
    Scroll   { dx: f32, dy: f32 },                 // pixel deltas
    KeyText  { text: String },                     // CGEventKeyboardSetUnicodeString
    KeyCombo { modifiers: Vec<KeyMod>, key: String }, // {Cmd, Shift}, "=" / "esc" / "up" 等
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMod { Cmd, Shift, Opt, Ctrl }
```

要点：
- HMAC 走现有 `canonical_bytes` 递归排序（M3.fix C1 修过），嵌套字段安全
- `Tap` v0.1 只支持主按钮；M7 加 `button: "primary" | "secondary"` 时 serde 默认值不破坏旧 client
- `InputAck.code` 字符串风格与 `SuperviseReject.code` 一致；iOS 端 switch 已知 case + fallback 显示原始 message
- 长文本粘贴**没有专用 payload**——iOS 拆为 `ClipboardSet`（M4 已存在）+ `KeyCombo Cmd+V`，InputInjector 完全不感知"长文本"概念
- iOS Swift 镜像类型：`Compose/CtrlMessage.swift` 加对应 case，`canonicalJSON` 已支持嵌套排序

## 3. Mac InputInjector

新模块：`mac-agent/crates/macagent-app/src/input_injector.rs`

```rust
pub struct InputInjector {
    gui_capture: Arc<GuiCapture>,
    ctrl_tx: UnboundedSender<CtrlPayload>,
    perm_cached: Mutex<bool>,
}

impl InputInjector {
    pub async fn handle_input(&self, sup_id: String, input: GuiInput) {
        if !self.check_ax() {
            self.ack(sup_id, "permission_denied", None); return;
        }
        let Some(target) = self.gui_capture.lookup_target(&sup_id).await else {
            self.ack(sup_id, "window_gone", None); return;
        };
        unsafe { NSRunningApplication::activate(target.pid, .activateIgnoringOtherApps) };

        let res = match input {
            Tap { x, y }                   => self.post_click(target.frame, x, y),
            Scroll { dx, dy }              => self.post_scroll(dx, dy),
            KeyText { text }               => self.post_unicode(&text),
            KeyCombo { modifiers, key }    => self.post_keycombo(&modifiers, &key),
        };
        match res {
            Ok(_)  => self.ack(sup_id, "ok", None),
            Err(e) => self.ack(sup_id, "no_focus", Some(format!("{e:#}"))),
        }
    }
}
```

要点：
- `lookup_target(sup_id)` 是 `GuiCapture` 新加的方法，返回 `Option<{pid: i32, frame: CGRect}>`，**实时**查 `CGWindowListCopyWindowInfo`（不 cache，应付窗口被拖动）。失败 = 窗口已关 = `window_gone`
- 坐标换算：`global_x = frame.origin.x + x * frame.size.width`；同理 y。frame 是 macOS top-left 原点（CG 坐标系，与 iOS 一致）
- `CGEventPost` 用 `kCGHIDEventTap`（全局，注入瞬间鼠标光标会跳到点击位置——故意，M6 不做"幽灵注入"）
- `CGEventKeyboardSetUnicodeString`：每次最多 ~20 字 UTF-16 单元；超过分多次 chunk
- KeyCombo：modifier flags 先按下 → 查 `key` 的 Carbon virtual keycode 静态表 → keyDown + keyUp → 松 modifier。表打成 `&[(&str, u16)]`，未知 key 返 `Err`
- Scroll 不在 Mac 端 throttle（iOS 已 16ms throttle 到 60Hz）；如果 iOS 客户端异常 burst，Mac 端依靠 CGEvent 队列自身吞吐自然降级，不主动 drop
- AX 缺失时不重试；后台 60s 一次 re-poll，授权后自动恢复（不需 iOS 重发请求）

InputInjector 在 `ui.rs::Connect` 的 spawn 块里和 SupervisionRouter 并列实例化。ctrl 接收循环走同一个 `sup_rx` drainer（M5 已有），新增 `Input` case 分发到 `input_injector.handle_input(...).await`。

风险：
- AX 沙箱授权只能用户手动给（即使有 entitlement），第一次必拒
- `CGEventPost` 在某些 App（系统设置 / SecureInput 状态）会被静默吞，无法检测——只能看 InputAck.code 到 "ok" 但视觉上没效果。M6 不修，README 写明
- Carbon keycode 表是 ANSI 布局；非 ANSI 物理键盘（DVORAK 等）不在 v0.1 范围

## 4. iOS gestures + UI

`ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift` 重构为：

```swift
struct GuiStreamDetailView: View {
    @Bindable var store: SupervisionStore
    let entry: SupervisionEntry
    @StateObject private var inputClient: InputClient

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                GuiStreamView(track: store.activeTrack)
                    .aspectRatio(entry.width / entry.height, contentMode: .fit)
                    .gesture(tapGesture)
                    .simultaneousGesture(panGesture)
                HardwareKeyControllerView(inputClient: inputClient)
                    .allowsHitTesting(false)
            }
            if !hasHardwareKeyboard {
                ModifierStickyRow(input: inputClient)
                SpecialKeyRow(input: inputClient)
            }
            ZoomToolbar(input: inputClient)
            InputBar(input: inputClient)
        }
        .onAppear { inputClient.bind(supId: entry.supId, glue: store.glue) }
    }
}
```

新增 actor `InputClient`（@MainActor）：

```swift
@MainActor
final class InputClient: ObservableObject {
    var supId: String?
    weak var glue: RtcGlue?
    private var lastScrollEmit = Date.distantPast

    func tap(normalizedX: CGFloat, normalizedY: CGFloat) async { ... }
    func scroll(dx: CGFloat, dy: CGFloat) async { /* 16ms throttle */ }
    func keyText(_ s: String) async { ... }
    func keyCombo(_ mods: [KeyMod], _ key: String) async { ... }
    func pasteLong(_ s: String) async {
        await glue?.sendCtrl(.clipboardSet(text: s))
        await keyCombo([.cmd], "v")
    }
}
```

### 4.1 软键盘场景手势

| 手势 | 行为 |
|---|---|
| 单击 RTCMTLVideoView | tap (`x`, `y` normalized within view bounds，aspect-fit 之后的内容矩形) |
| 单指拖（>8pt） | scroll，onChange 取 translation 差分，16ms throttle |
| 工具栏 +/-/0 | `KeyCombo([.cmd], "=")` / `"-"` / `"0"` |
| InputBar 文本 onSubmit | `text.count > 32` → `pasteLong`；否则 `keyText` |
| ✏️ 长文本 → ComposeSheet 提交 | 复用 M4 路径，阈值切换同上 |

### 4.2 修饰键吸附栏 + 特殊键栏

```
┌─────────────────────────────────┐
│  ⌘   ⇧   ⌥   ⌃                   │  tap = sticky-release，长按 = lock
├─────────────────────────────────┤
│ Esc  Tab ↑ ↓ ← → ↩ ⌫            │  特殊键，按下时合并当前修饰键吸附
└─────────────────────────────────┘
```

- 修饰键吸附：tap 一次 → 高亮 → InputBar 下一字符发 `KeyCombo` 后修饰自动释放；长按 → 实心高亮 = lock，再 tap 释放
- 特殊键 = `KeyCombo { modifiers: <当前吸附>, key: "esc" | "tab" | "return" | "delete" | "up" | "down" | "left" | "right" }`，按完释放修饰
- ComposeSheet（多行）只走 `keyText` / `pasteLong`，不与修饰键栏交互

### 4.3 外接键盘场景（iPad + Magic Keyboard 等）

```swift
final class HardwareKeyController: UIViewController {
    var inputClient: InputClient?
    override var canBecomeFirstResponder: Bool { true }
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated); becomeFirstResponder()
    }
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for press in presses {
            guard let key = press.key else { continue }
            let mods = mapModifiers(key.modifierFlags)
            let name = mapKeyName(key)
            if !mods.isEmpty || isSpecial(key) {
                Task { await inputClient?.keyCombo(mods, name) }
            } else {
                Task { await inputClient?.keyText(key.characters) }
            }
            return  // consume
        }
        super.pressesBegan(presses, with: event)
    }
}
```

- 用 `UIViewControllerRepresentable` 包装叠在 `GuiStreamView` 顶层（`allowsHitTesting(false)` 不挡触控），`viewDidAppear` 自动 `becomeFirstResponder`
- 检测 `GCKeyboard.coalesced != nil`（GameController.framework）→ `hasHardwareKeyboard = true` → 隐藏修饰键吸附栏 + 特殊键栏（contextual UI）
- iPadOS 系统保留键（Cmd+H / Cmd+Tab / Cmd+Space / Globe+L）无法拦截，按下会触发 iPadOS 自身行为，不会到 Mac——README 写明
- 中文 IME：iPadOS 仍正常出候选条；`pressesBegan` 拿到的 `key.characters` 是上屏字符（非 raw key），走 `keyText` 与软键盘 IME 路径一致

### 4.4 SupervisionEntry 字段扩展

`SupervisedAck` 已经带 width/height（M5 spec），但 `SupervisionEntry` Swift 类型确认要带 `width: u32, height: u32` 字段。RTCMTLVideoView 用 `aspectRatio(width/height, contentMode: .fit)` 让画面不拉伸，归一化 `(x,y)` 因此对齐窗口内容矩形，与 Mac 反查 frame 直接吻合。

## 5. Permission UX

### 5.1 Mac 端

- 启动 + 每次 Connect 时 `AXIsProcessTrusted()`，缓存到 `InputInjector::perm_cached`
- 菜单栏 tray 图标变体：`AX⚠️`（红点，未授）/ `AX✓`（绿点，已授）
- eframe 主窗口（Paired 状态）顶部 banner：「输入注入需要 Accessibility 权限」+ 「Open System Settings」按钮
- 按钮动作：`NSWorkspace.shared.open(URL("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"))`
- 后台 60s 一次 `AXIsProcessTrusted` 轮询，授权后自动清掉 banner

### 5.2 iOS 端

- 收到 `InputAck { code: "permission_denied" }` → `SupervisionStore.lastReject` 设值 → `GuiStreamDetailView` 顶部半透明 banner：「Mac 未授予 Accessibility」+ 「再试一次」按钮
- 「再试一次」 = 重发最后一次失败的 Input（无复杂 retry 队列）
- banner 不阻塞滚动 / tap 视觉反馈

### 5.3 不做

- 自动引导用户拖 macagent.app 到 Privacy_Accessibility 列表（无公开 API）
- TCC.db 检测（违规绕过）
- iOS 远程触发 Mac AX 授权对话框（macOS 不允许）
- AXObserverCreateWithCFRunLoopSource 推送式权限变化通知（轮询足够）

## 6. Testing & Acceptance

### 6.1 Mac 单元测试（`input_injector.rs` 内）

1. `keycode_table_lookup` — `"esc"` → 53、`"="` → 24、`"-"` → 27、`"0"` → 29、`"a"` → 0、`"return"` → 36、`"up"` → 126；未知 key 返 `Err`
2. `normalize_to_global_coords` — frame `(100, 200, 800, 600)` + normalized `(0.5, 0.5)` → `(500, 500)`；边界 `(0, 0)` 和 `(1, 1)` 都对
3. `modifier_flags_packing` — `[Cmd, Shift]` → `kCGEventFlagMaskCommand | kCGEventFlagMaskShift`，bit-exact
4. `chunk_unicode_text` — 50 字中文按 20 字 chunk → 3 段

### 6.2 iOS 单元测试（`MacIOSWorkspaceTests/`）

5. `InputClientThrottleTests` — 16ms 内连续 5 次 scroll → 1–2 次 ctrl payload
6. `KeyMapperTests` — `mapModifiers([.command, .shift])` → `[.cmd, .shift]`；`mapKeyName(UIKey for Esc)` → `"esc"`；`mapKeyName(UIKey for ↑)` → `"up"`
7. `PasteThresholdTests` — 32 字以下 `keyText`、33 字以上 `clipboardSet + keyCombo Cmd+V`

### 6.3 Manual smoke

1. iPad + Magic Keyboard：进 GuiStreamDetailView，按外接键盘 Cmd+L → Chrome 地址栏聚焦
2. iPhone 纯触控：单击 Chrome 网页按钮 → 看到点击；单指滑动 → 滚动；工具栏 Cmd+ → 字号变大
3. 长文本粘贴 200 字中文 → Mac 剪贴板覆写 + Cmd+V 粘贴成功
4. 撤销 AX → 重试 → iOS banner「未授予 Accessibility」+「再试一次」→ 重新授权 60s 内自动恢复
5. 监管中关掉目标窗口 → 再点击 → `InputAck { code: "window_gone" }` → iOS toast 并回 windows list

### 6.4 Acceptance（spec line 591）

- ✅ 点 Chrome 网页按钮（manual #2 + #1）
- ✅ 滚动（manual #2）
- ✅ 长文本粘贴（manual #3）
- ✅ Chrome 与 Electron 系 App 跑 Cmd+/Cmd-/Cmd0（manual #2）
- ✅ Accessibility onboarding（manual #4）

## 7. 文件清单

新增：
- `mac-agent/crates/macagent-app/src/input_injector.rs`
- `ios-app/MacIOSWorkspace/Input/InputClient.swift`
- `ios-app/MacIOSWorkspace/Input/HardwareKeyController.swift`
- `ios-app/MacIOSWorkspace/Input/ModifierStickyRow.swift`
- `ios-app/MacIOSWorkspace/Input/SpecialKeyRow.swift`
- `ios-app/MacIOSWorkspace/Input/ZoomToolbar.swift`
- `ios-app/MacIOSWorkspace/Input/KeyMapper.swift`

修改：
- `mac-agent/crates/macagent-core/src/ctrl_msg.rs` — `CtrlPayload::{Input, InputAck}` + `GuiInput` + `KeyMod`
- `mac-agent/crates/macagent-app/src/gui_capture/mod.rs` — `lookup_target(sup_id)` 公开方法
- `mac-agent/crates/macagent-app/src/ui.rs` — 实例化 InputInjector + 接 ctrl_recv 分发
- `mac-agent/crates/macagent-app/src/rtc_glue.rs` — 在 ctrl 分发增加 `Input` case
- `ios-app/MacIOSWorkspace/CtrlMessage.swift` — Swift 镜像类型
- `ios-app/MacIOSWorkspace/Gui/GuiStreamView.swift` 或新拆 `GuiStreamDetailView.swift` — 整合手势 + 工具栏
- `ios-app/MacIOSWorkspace/SupervisionStore.swift` — `lastInputAck` 字段、SupervisionEntry 加 width/height
