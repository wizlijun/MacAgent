# iOS 输入优化（ComposeSheet）— 设计文档

- **日期**：2026-04-30
- **状态**：草案，可进入实现规划
- **负责人**：bruce@hemory.com
- **范围**：iOS App（iPhone / iPad，iOS 26 / iPadOS 26）
- **关联主 spec**：[`2026-04-30-mac-iphone-workspace-design.md`](2026-04-30-mac-iphone-workspace-design.md)

---

## 1. 范围

### 1.1 目标

把主 spec 第 1.4 节推迟到 B2 档的"中文 IME"前置到 v0.1，并通过同一条路径覆盖 iOS 系统语音听写、第三方键盘语音（包括用户安装的微信输入法 / 搜狗 / 讯飞）以及多行长文本编辑。

### 1.2 核心思路

不动 Mac Agent，不增新通道，不引入新依赖。在 iOS 一侧加一个标准 SwiftUI sheet 作为"compose 表面"——只要让 iOS UIKit 拿到一个标准 `UITextView`，IME 与所有用户安装的键盘（含语音）就**天然可用**。压根不需要"调用微信键盘"的 API（iOS 也不允许），用户切到他熟悉的键盘按 🎤 就是了。

### 1.3 架构选择（已锁定）

| 维度 | 决定 | 说明 |
|---|---|---|
| inline vs compose | **Hybrid** | 默认沿用 inline live-typing；✏️ 按钮进 compose sheet |
| 语音转写源 | **路线 A：靠用户键盘自带** | 工程量 0；不做 app 内 🎤；不影响未来增量加 |
| 适用面 | **CliView + GuiStreamView 共用** | 同一个 SwiftUI 组件，仅 `onSend` 闭包不同 |
| 辅助功能 | **全部不做** | 无历史、无模板、无草稿、无自动 \n |

### 1.4 验收

- 真 iPhone 装微信键盘 → CliView ✏️ → 按 🎤 说"git status" → Send → 终端收到并执行
- 真 iPhone 系统拼音键盘 → GuiStreamView（Chrome 监管）✏️ → 输入"你好世界" → Send → Chrome 网页文本框出现"你好世界"
- iPad（含 Magic Keyboard 外接）同样跑通
- 退出 sheet（Cancel）不向 Mac 发任何字节

### 1.5 非目标（明确不做，留给未来）

- 命令历史 / 历史搜索 / 模糊查找
- 模板 / snippets / 别名
- 跨 session 或跨设备的草稿持久化
- app 内"长按说话"录音按钮（与路线 A 互斥地选了 A）
- 自动行为：自动追加 `\n`、自动去尾空格、自动转义
- Markdown 渲染 / 语法高亮 / 自动补全
- 多 tab compose（一次仅一个 sheet 实例）
- IME 子窗口、菜单、文件选择器（仍按主 spec 推迟到 B2）

---

## 2. 组件与架构

### 2.1 新增组件 `ComposeSheet`

```swift
struct ComposeSheet: View {
    @Binding var text: String
    let title: String                  // "Compose · pty/s1" / "Compose · Chrome"
    let onSend: (String) -> Void
    let onCancel: () -> Void
}
```

**视图组成（最少元素）**

| 元素 | 说明 |
|---|---|
| 顶部 title bar | 显示目标名（pty id 或 监管窗口 app）+ Cancel 按钮 |
| `TextEditor`（多行） | 主体；`@FocusState` 自动拿焦点；不限制行数；不做语法高亮 |
| 底部 Send 按钮 | 主操作；`text.isEmpty` 时 disabled |

**呈现方式**：iPhone 上 `.sheet`（半屏，可向下拖关）；iPad regular 宽度下走 `.popover` 或 `.sheet(.medium)`，遵循主 spec 的「单 target / 自适应」原则，不出现设备分支。

**键盘 / IME 行为**：完全交给 UIKit。`TextEditor` 是 `UITextInput` conforming view；用户安装的任何键盘（系统 / 微信 / 搜狗 / 讯飞）按 🌐 切换，按 🎤 走该键盘自有的语音管线，提交结果直接写入 `text` binding。本组件**不感知**输入法状态。

### 2.2 调用方接入

#### CliView
- 在已有 shortcut bar（esc / tab / ⌃ / ⌘ / ↑↓←→）右侧追加一个 ✏️ 按钮。
- 点击 → `present(ComposeSheet(title: "Compose · \(session.id)", onSend: { send(toSession: session.id, bytes: $0.data(using: .utf8)!) }, ...))`
- `send(toSession:bytes:)` 沿用 SessionStore 现有的 `pty/<id>` DataChannel write 路径——和 inline live-typing 完全同一条路径，只是一次发送多个字节。

#### GuiStreamView
- 在已有工具栏（含 + / − / ⌖ 缩放按钮）右侧追加 ✏️。
- 点击 → `present(ComposeSheet(title: "Compose · \(activeSupervision.appName)", onSend: { sendInput(.pasteText(winId: activeWindowId, text: $0)) }, ...))`
- 投递走既存 `input` DataChannel 上的 `paste_text` 帧（主 spec 4.4），Mac Agent 侧由 `InputInjector.paste_text` 兜接（NSPasteboard.set + 模拟 Cmd+V）。

### 2.3 Mac Agent 改动

**零改动。** 通道协议、通道数量、消息结构均保持。`pty/<id>` 接收任意字节流；`input.paste_text` 路径已在主 spec 第 3.1 InputInjector / 4.4 节定义。本特性纯 iOS 增量。

---

## 3. 数据流

### 3.1 CLI 路径

```
用户                 CliView                          ComposeSheet              RtcPeer
 │                    │                                 │                         │
 │ 点 ✏️              │                                 │                         │
 │                    │ present(ComposeSheet)           │                         │
 │                    ├────────────────────────────────►│                         │
 │                    │                                 │ TextEditor.focus       │
 │ 切微信键盘 / 🎤    │                                 │ ←─ UIKit/IME ─→ 文本    │
 │ 点 Send            │                                 │                         │
 │                    │  onSend(text)                   │                         │
 │                    │◄────────────────────────────────│                         │
 │                    │ pty/s1.write(text.utf8)         │                         │
 │                    ├──────────────────────────────────────────────────────────►│
 │                    │ sheet.dismiss                   │                         │
```

字节按 UTF-8 序列化后通过 `pty/<id>` 二进制 DataChannel 发出。Mac Agent 不区分这是 inline live 来的还是 compose 来的——两条路径在传输层完全相同。

### 3.2 GUI 路径

```
用户                 GuiStreamView                     ComposeSheet              RtcPeer
 │                    │                                 │                         │
 │ 点 ✏️              │                                 │                         │
 │                    │ present(ComposeSheet)           │                         │
 │                    ├────────────────────────────────►│                         │
 │                    │                                 │ TextEditor              │
 │ ─输入 / 语音─      │                                 │                         │
 │ 点 Send            │                                 │                         │
 │                    │  onSend(text)                   │                         │
 │                    │◄────────────────────────────────│                         │
 │                    │ input: { paste_text,            │                         │
 │                    │   win_id: <active>, text }      │                         │
 │                    ├──────────────────────────────────────────────────────────►│
 │                    │ sheet.dismiss                   │                         │
```

Mac Agent 收到 `paste_text` → InputInjector：`NSPasteboard.set(text)` + `key_combo([cmd], "v")`，沿用主 spec 第 3.1 节描述的"长文本快速通道"。粘贴后 250 ms 尝试恢复原剪贴板。

### 3.3 关键不变量

- Send 不追加 `\n`。所见即所发。要执行命令请在 sheet 编辑器中按一次 Return（编辑器内的 `\n` 会被一起发出）。
- Cancel 不向 Mac 发任何字节，本地 `text` 立即清空、不持久化。
- IME 组字未提交时点 Send：`UITextView.endEditing(true)` 强制 commit，再读 `text` —— UIKit 已在 commit 时把组字结果写入 binding。

---

## 4. 错误处理

仅列本特性独有的失败模式。其余沿用主 spec 第 5 节。

| 失败 | 处理 |
|---|---|
| 无 active session（CLI）/ 无 active supervision（GUI） | ✏️ 按钮 disabled（保留可见以提示能力存在），无法点开 sheet |
| Sheet 打开期间 `pty/<id>` 通道因 RtcPeer 重连关闭 | Send 时 ctrl 回 `session_gone`；iOS 弹 toast「会话已断开」；sheet 不关、`text` 不丢；用户可 Cancel 或等重连后再 Send |
| `paste_text` 因目标窗口失焦失败 | 沿用主 spec 5 节 `{input_dropped, sup_id, reason}`；iOS 弹 toast；sheet 不关 |
| Sheet 打开期间用户切到后台（multitasking） | UIKit 标准：组字状态保留；切回前台 sheet 仍在；`text` 仍在 |
| `text` 长度大于 1 MB | Send 按钮 disabled 并显示字节计数（与主 spec ClipboardBridge 1 MB 阈值对齐）|
| 监管窗口在 sheet 打开期间被关闭 | 收到主 spec 5 节 `{stream_ended, window_closed}` 时同步关 sheet 并 toast |

---

## 5. 测试策略

| 层 | 工具 | 覆盖 |
|---|---|---|
| iOS 单元 | XCTest + ViewInspector | `ComposeSheet` 注入 text / 点 Send / 断言 onSend 收到精确字符串（含中文、含 emoji、含 `\n`）；Cancel 不触发 onSend |
| iOS UI 测试 | XCUITest | iPhone / iPad 各跑一次：sheet 弹出布局、键盘弹起不遮 Send、`.medium` 高度 dismiss 手势 |
| 端到端（mock-iOS + Mac Agent） | 主 spec 已规划的 webrtc-rs mock-iOS client | CLI 路径：sheet → Send → 断言 PTY 收到字节；GUI 路径：sheet → Send → 断言 InputInjector 收到 `paste_text` |
| 手测（不进 CI） | 真 iPhone | 微信键盘 🎤 中文语音 → Send → 终端正确收到；系统拼音 → GuiStreamView → Chrome 文本框正确出字 |
| 手测（不进 CI） | 真 iPad + Magic Keyboard | sheet 在外接键盘下行为正常（Esc 关闭、Cmd+Return 提交可选） |

不进自动测试：iOS 第三方键盘（依赖用户安装），仅手测覆盖。

---

## 6. 对主 spec 文档的修改点

应用以下编辑到 `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md`：

| 节 | 修改 |
|---|---|
| 1.4 非目标 | 删去"不处理中文 IME、子窗口、菜单、文件选择器（推迟到 B2 档）"中的"中文 IME"；保留子窗口 / 菜单 / 文件选择器仍推迟。补一行："iOS 端通过标准 UITextView 支持 IME 与系统/键盘语音，参见独立 spec ios-input-compose-design.md。" |
| 3.2 iOS App 自适应布局原则 | 新增子节 `ComposeSheet`，引用本文件 § 2.1 |
| 3.2 CliView | 在「底部快捷键栏」一句后追加："右端有 ✏️ 按钮打开 ComposeSheet（多行 / IME / 语音）。" |
| 3.2 GuiStreamView | 在 `+ / − / ⌖ 按钮` 一句后追加："✏️ 按钮打开 ComposeSheet，发出 input.paste_text。" |
| 4.3 CLI 会话生命周期 | 在示意图后补一句："`pty/<id>.write` 既承载 inline live-typing 字节，也承载 ComposeSheet Send 整段字节，传输层无差别。" |
| 4.4 GUI 监管与流送 | 在「输入」分段下补一行："`input: {paste_text, win, text}` 也是 ComposeSheet 的 GUI 投递路径。" |

不新增里程碑。本特性同时落进现有：
- **M3 · CLI 通道** —— 加入 ComposeSheet（CLI 路径）
- **M5 · GUI 监管 v0** 或 **M6 · 输入注入** —— 加入 ComposeSheet（GUI 路径），具体看实现顺序

---

## 7. 待解问题

均为可在实现阶段或 v0.1 手测后微调的细节，不阻塞实现规划：

- **iPad 外接键盘 Cmd+Return 绑定 Send**：v0.1 默认**不绑定**（保持「最轻量」），仅 tap Send 提交。手测发现刚需可在 M3 / M6 同期补一行 `keyboardShortcut(.return, modifiers: .command)`。
- **Send 后 `text` 行为**：v0.1 默认**清空**（避免重复发送），sheet 同时 dismiss。如手测发现需「修改后重发」工作流，再考虑保留 + 显式 Clear 按钮。
