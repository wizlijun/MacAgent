# M8 · Polish + TestFlight 设计文档

> 对接 spec：`docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §M8（line 593）+ 弱网（line 574）+ onboarding（line 575）。

## 1. Scope

**M8.A — AI 可执行（4 块）：**

- **A1 弱网 / ICE restart hardening**
  - Mac signaling 自动重连：指数退避（1s → 2s → 4s → 8s capped）
  - Mac SignalingClient 层捕获 ws close → 触发 reconnect → 等成功后 ICE restart
  - iOS RtcGlue.GlueState 加 `.reconnecting` 状态
  - iOS PairedView 显示「网络抖动，重连中…」黄色 banner（autohide 在 `.connected` 后）

- **A2 权限引导卡片**
  - Mac eframe Pairing 状态：QR 上方加文字「请确保 macagent 已授予以下权限：…」
  - Mac eframe Paired 状态：在 GUI supervise 选项卡顶部加未授权权限的引导卡片（Screen Recording / Accessibility）
  - 卡片设计：图标 + 一句中文说明 + 「打开系统设置」按钮（已有 NSWorkspace open URL 路径）
  - 「Screen Recording」`x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`
  - 「Accessibility」`x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`
  - 已授权时不显示

- **A3 错误码 → 人话**
  - 新模块 `mac-agent/crates/macagent-core/src/error_msg.rs` + `ios-app/MacIOSWorkspace/ErrorMessage.swift`
  - 集中表 `error_code: String -> &str`：`permission_denied`、`window_gone`、`launch_timeout`、`launch_failed`、`bundle_not_allowed`、`supervision_limit`、`fit_denied`、`encoder_failed`、`no_focus`、`throttled`、`network_error`...
  - 现有所有 toast / banner / SuperviseReject 文案改读这个表
  - 未知 code 兜底：「未知错误：<code>」+ 原 message

- **A4 白名单编辑器**
  - `~/Library/Application Support/macagent/agent.json5` 加 `gui.allowed_bundles` 字段（已有 launcher 配置在该文件，M3 引入）
  - 默认值：`["com.openai.chat", "com.anthropic.claude", "com.google.Chrome"]`（迁移 launcher_m7 hardcoded）
  - launcher_m7::is_allowed() 改读配置
  - Mac eframe 主窗口加「白名单」选项卡：List + 加号按钮（输入 bundle id）+ 删除按钮
  - 修改即写回 agent.json5；launcher 在每次 launch 时重新读（不需要重启）

**M8.B — 用户驱动（README 文档）：**

- **B1 Mac code sign + notarize**：`docs/release/MAC_RELEASE.md`
  1. 申请 Developer ID Application 证书
  2. `cargo build --release -p macagent-app`
  3. `codesign --deep --force --options runtime --entitlements ... --sign "Developer ID Application: <Name>" target/release/macagent`
  4. 打包 .dmg / .pkg
  5. `xcrun notarytool submit ... --wait`
  6. `xcrun stapler staple ...`

- **B2 iOS TestFlight**：`docs/release/IOS_RELEASE.md`
  1. App Store Connect 创建 App
  2. Apple Distribution 证书 + Provisioning Profile
  3. Xcode Archive → Distribute App → App Store Connect
  4. App Store Connect TestFlight tab 添加测试组
  5. 提交审核

**OUT（v0.1 不做）：**
- 多语言（仅中文）
- 在线更新
- Sentry / Crashlytics
- 用户埋点
- 启动画面 / About 页
- 自动从证书发现下游 deploy

## 2. Architecture

### 2.1 ICE restart hardening (A1)

`SignalingClient::connect_with_retry`:
```rust
async fn connect_with_retry(url: &str) -> Result<()> {
    let mut delay = Duration::from_secs(1);
    loop {
        match connect(url).await {
            Ok(c) => return Ok(c),
            Err(_) if delay < Duration::from_secs(8) => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => return Err(e),
        }
    }
}
```

iOS `GlueState` 加：
```swift
enum GlueState: Equatable {
    case idle, connecting, connected, reconnecting, disconnected, failed(String)
}
```

iOS PairedView 在 state 是 `.reconnecting` 时显示 banner。

### 2.2 权限引导卡片 (A2)

新模块 `mac-agent/crates/macagent-app/src/onboarding.rs`：
```rust
pub enum PermissionStatus { Granted, Denied, Unknown }

pub fn screen_recording_status() -> PermissionStatus { /* CGPreflightScreenCaptureAccess */ }
pub fn accessibility_status() -> PermissionStatus { /* AXIsProcessTrusted */ }

pub fn open_screen_recording_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}
```

Mac eframe `update()` 在 Paired 状态调用一个 `render_permission_cards(ui, ...)` helper，未授权时显示卡片。已授权时返回 ()。

### 2.3 错误码集中表 (A3)

`mac-agent/crates/macagent-core/src/error_msg.rs`：
```rust
pub fn humanize(code: &str) -> &'static str {
    match code {
        "permission_denied"   => "Mac 未授予 Accessibility 权限",
        "window_gone"         => "目标窗口已关闭",
        "launch_timeout"      => "启动超时（5 秒未发现新窗口）",
        "launch_failed"       => "启动失败",
        "bundle_not_allowed"  => "App 不在白名单",
        "supervision_limit"   => "监管数已达上限（8）",
        "fit_denied"          => "窗口尺寸调整被拒绝",
        "encoder_failed"      => "硬件 H.264 编码器初始化失败",
        "no_focus"            => "目标窗口无法获得焦点",
        "throttled"           => "操作过于频繁",
        "network_error"       => "网络错误",
        _                     => "",  // 调用方在 "" 时回退到原始 code 字符串
    }
}
```

iOS Swift 镜像 `ErrorMessage.swift` 同表。

call sites 修改：所有 toast / banner / `SuperviseReject` 显示路径都通过 `humanize(code)`，未知时显示 `"未知错误：\(code)" + (message ?? "")`。

### 2.4 白名单编辑器 (A4)

`~/Library/Application Support/macagent/agent.json5` 加字段（保留 launcher 字段不变）：
```json5
{
  // ... existing producer launchers ...
  "gui": {
    "allowed_bundles": [
      "com.openai.chat",
      "com.anthropic.claude",
      "com.google.Chrome"
    ]
  }
}
```

`launcher_m7.rs::is_allowed(bundle_id)` 改读 `agent.json5`，每次调用都重新读（文件 IO 廉价；spec 风格）。

Mac eframe 加一个新 panel（在主窗口）：
- 显示当前 allowed_bundles 列表
- 加号按钮 → 弹 TextField → 输入 bundle id → 写回
- 删除按钮 → 移除并写回
- 写回后 launcher_m7 立即拿到新值（每次 launch 读文件）

不做：
- 启动 App 浏览器（让用户从 /Applications 选）—— 太复杂，v0.1 用户手输 bundle id
- 验证 bundle id 存在性 —— 留给 launch 失败时显示「launch_failed」

## 3. Manual smoke

1. **A1 弱网**：开 macagent + 配对 iPhone → 关 Wi-Fi 30s → iOS 显示「重连中」banner → 开 Wi-Fi → banner 消失，stream 恢复
2. **A2 权限**：清掉 macagent 的 Screen Recording 授权 → 重启 macagent → eframe Paired 显示 Screen Recording 引导卡片 → 点「打开系统设置」→ 系统设置定位到对应面板
3. **A3 文案**：iOS 收到 `permission_denied` → toast 显示「Mac 未授予 Accessibility 权限」（不是原始 code）
4. **A4 白名单**：删除 ChatGPT bundle → iOS supervise_launch ChatGPT → 收到 `bundle_not_allowed` → 添加回 → 重试成功

## 4. Acceptance（spec line 593）

- ✅ 弱网恢复（A1 manual smoke #1）
- ✅ 权限引导文案（A2 manual smoke #2）
- ✅ 错误 UI 文案（A3 manual smoke #3）
- ✅ 菜单栏白名单编辑器（A4 manual smoke #4）
- ⏸ code sign + notarize（B1 README，用户跑）
- ⏸ TestFlight 提交（B2 README，用户跑）
- ⏸ 5 分钟新用户体验（B1+B2 完成后才能 e2e 验）

## 5. 文件清单

**Mac 新增：**
- `mac-agent/crates/macagent-core/src/error_msg.rs`
- `mac-agent/crates/macagent-app/src/onboarding.rs`

**Mac 修改：**
- `mac-agent/crates/macagent-app/src/signaling.rs` 或 `rtc_glue.rs` — connect_with_retry
- `mac-agent/crates/macagent-app/src/launcher_m7.rs` — is_allowed 读 agent.json5
- `mac-agent/crates/macagent-app/src/launcher.rs` — 扩展 LauncherConfig 加 gui.allowed_bundles
- `mac-agent/crates/macagent-app/src/ui.rs` — 接 onboarding cards + whitelist panel + reconnect 文案
- `mac-agent/crates/macagent-core/src/lib.rs` — 导出 error_msg

**iOS 新增：**
- `ios-app/MacIOSWorkspace/ErrorMessage.swift`

**iOS 修改：**
- `ios-app/MacIOSWorkspace/RtcGlue.swift` — GlueState 加 `.reconnecting`
- `ios-app/MacIOSWorkspace/PairedView.swift` — reconnecting banner
- `ios-app/MacIOSWorkspace/Gui/SupervisionTile.swift` + `SuperviseRejectInfo` 渲染处 — humanize code
- `ios-app/MacIOSWorkspace/Gui/GuiStreamDetailView.swift` — humanize fit_failed reason
- `ios-app/MacIOSWorkspace/Notify/WatchersView.swift` — humanize APNs error
- `ios-app/MacIOSWorkspace/Term/InputBar.swift` 等所有 toast / error 文案处

**Docs 新增：**
- `docs/release/MAC_RELEASE.md`
- `docs/release/IOS_RELEASE.md`

## 6. Risks + Mitigations

| Risk | Mitigation |
|---|---|
| ICE restart 在某些 NAT 后失败 | iOS 显示 `.failed`，提示用户重新 Connect；spec 已说 manual recover 可接受 |
| Whitelist 文件解析错误 | launcher_m7 fallback 到默认 3 个 hardcoded bundles + log 错误 |
| 错误码表与现有 code 不同步 | 集成测试：枚举所有发出 SuperviseReject / FitFailed / InputAck 的 code，断言都在表中（或显式标注 OK to fall back） |
| onboarding 卡片在小窗口下挤压 | eframe 用 `egui::ScrollArea`；卡片包在 collapsing header 里 |
| 用户输入非法 bundle id | launcher_m7 launch 失败发 `launch_failed`，A3 文案兜底 |

## 7. Out of Scope

- 多语言（仅中文）
- 自动更新通道
- 错误日志收集 / Sentry / Crashlytics
- 用户行为埋点
- 启动画面 / About / Version 页
- Touch ID 解锁
- 自动从证书发现完成 deploy
- 验证 bundle id 存在性
- 启动 App 浏览器（白名单从 /Applications 选）
- iOS Live Activity / Dynamic Island 集成
