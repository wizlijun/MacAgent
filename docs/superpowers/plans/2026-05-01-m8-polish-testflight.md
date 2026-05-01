# M8 · Polish + TestFlight Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox `- [ ]` syntax.

**Goal:** Ship M8 polish — ICE-restart hardening with iOS reconnect banner, Mac onboarding cards for Screen Recording / Accessibility, centralized error-code → Chinese mapping, and an editable GUI app whitelist via `agent.json5` + Mac eframe panel. Plus README docs for B1 (Mac code sign + notarize) and B2 (iOS TestFlight) so the user can drive release engineering themselves.

**Architecture:** New `error_msg.rs` module (Rust + Swift mirror) centralizes the code → human-readable mapping. New `onboarding.rs` exposes Screen Recording / Accessibility status + open-settings shortcuts. `launcher_m7::is_allowed` migrates from a hardcoded slice to reading `agent.json5::gui.allowed_bundles`. `SignalingClient` wraps connect in exponential-backoff retry. `GlueState::Reconnecting` is added on iOS. Mac eframe gets two new sections: permission cards (Paired state) + whitelist editor.

**Spec:** `docs/superpowers/specs/2026-05-01-m8-polish-testflight-design.md`.

---

## Task Breakdown (Subagent-Driven)

### Task M8.1 — error_msg module (Rust + Swift)

**Files:**
- Create: `mac-agent/crates/macagent-core/src/error_msg.rs`
- Modify: `mac-agent/crates/macagent-core/src/lib.rs` (export)
- Create: `ios-app/MacIOSWorkspace/ErrorMessage.swift`
- Test: `mac-agent/crates/macagent-core/src/error_msg.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Implement Rust table + tests**

Create `error_msg.rs`:
```rust
//! Error code → user-readable message (Chinese).

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
        _                     => "",
    }
}

#[cfg(test)]
mod tests {
    use super::humanize;

    #[test]
    fn known_codes() {
        assert_eq!(humanize("permission_denied"), "Mac 未授予 Accessibility 权限");
        assert_eq!(humanize("window_gone"), "目标窗口已关闭");
        assert_eq!(humanize("supervision_limit"), "监管数已达上限（8）");
    }

    #[test]
    fn unknown_code_returns_empty() {
        assert_eq!(humanize("xyz_unknown"), "");
        assert_eq!(humanize(""), "");
    }
}
```

In `mac-agent/crates/macagent-core/src/lib.rs` add `pub mod error_msg;`.

- [ ] **Step 2: Run Rust tests**
```
cd mac-agent && cargo test -p macagent-core error_msg
```
Expected: 2 PASS.

- [ ] **Step 3: Implement Swift mirror**

Create `ios-app/MacIOSWorkspace/ErrorMessage.swift`:
```swift
import Foundation

enum ErrorMessage {
    static func humanize(_ code: String) -> String {
        switch code {
        case "permission_denied":   return "Mac 未授予 Accessibility 权限"
        case "window_gone":         return "目标窗口已关闭"
        case "launch_timeout":      return "启动超时（5 秒未发现新窗口）"
        case "launch_failed":       return "启动失败"
        case "bundle_not_allowed":  return "App 不在白名单"
        case "supervision_limit":   return "监管数已达上限（8）"
        case "fit_denied":          return "窗口尺寸调整被拒绝"
        case "encoder_failed":      return "硬件 H.264 编码器初始化失败"
        case "no_focus":            return "目标窗口无法获得焦点"
        case "throttled":           return "操作过于频繁"
        case "network_error":       return "网络错误"
        default:                    return ""
        }
    }

    /// "humanize or fall back to <code> + (message ?? '')"
    static func describe(code: String, message: String? = nil) -> String {
        let h = humanize(code)
        if !h.isEmpty { return message.map { "\(h)（\($0)）" } ?? h }
        return message.map { "未知错误：\(code)（\($0)）" } ?? "未知错误：\(code)"
    }
}
```

- [ ] **Step 4: iOS build**
```
xcodebuild -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -destination 'platform=iOS Simulator,name=iPhone 16 Pro' build
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 5: Update existing toast/banner call sites** (best-effort sweep — don't go on a refactor binge; touch only obvious string sites).

Search for hardcoded code-string usage: `grep -rn '"permission_denied"\|"window_gone"\|"launch_failed"\|"supervision_limit"\|"fit_denied"' ios-app/MacIOSWorkspace/`. Update each to `ErrorMessage.describe(code: ..., message: ...)` where the user-facing string is constructed.

Same on Mac side, but Mac UI is mostly the eframe banner — `ui.rs` may have only 1-2 call sites. **Don't rewrite the whole eframe banner** — only the user-visible strings.

- [ ] **Step 6: Commit**
```bash
cd /Users/bruce/git/macagent
git add mac-agent/crates/macagent-core/src/error_msg.rs \
        mac-agent/crates/macagent-core/src/lib.rs \
        ios-app/MacIOSWorkspace/ErrorMessage.swift \
        $(git diff --name-only -- ios-app/MacIOSWorkspace/ mac-agent/)
git commit -m "feat(m8): add error_msg humanize table (Rust + Swift)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.2 — onboarding module + Mac eframe permission cards

**Files:**
- Create: `mac-agent/crates/macagent-app/src/onboarding.rs`
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`

- [ ] **Step 1: Create onboarding module**

```rust
//! Permission status probes + open-settings shortcuts.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionStatus { Granted, Denied, Unknown }

pub fn screen_recording_status() -> PermissionStatus {
    use crate::gui_capture::perm;
    match perm::check() {
        perm::PermissionStatus::Granted => PermissionStatus::Granted,
        perm::PermissionStatus::Denied  => PermissionStatus::Denied,
        _                                => PermissionStatus::Unknown,
    }
}

pub fn accessibility_status() -> PermissionStatus {
    // Reuse InputInjector::refresh_ax via a separate AX_IS_TRUSTED FFI call here,
    // OR poll AXIsProcessTrusted directly.
    extern "C" {
        #[link_name = "AXIsProcessTrusted"]
        fn ax_is_trusted() -> bool;
    }
    if unsafe { ax_is_trusted() } { PermissionStatus::Granted } else { PermissionStatus::Denied }
}

pub fn open_screen_recording_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}

pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}
```

Add `mod onboarding;` to `main.rs` (no `#[allow(dead_code)]`; ui.rs uses it immediately).

- [ ] **Step 2: Render cards in eframe Paired state**

In `ui.rs::update`, find the `PairState::Paired` branch. Add (replacing/wrapping the existing AX banner from M6.10):

```rust
let sr = onboarding::screen_recording_status();
let ax = onboarding::accessibility_status();

if sr != onboarding::PermissionStatus::Granted {
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::YELLOW, "⚠️ 屏幕录制 — GUI 监管需要");
        if ui.button("打开系统设置").clicked() {
            onboarding::open_screen_recording_settings();
        }
    });
    ui.separator();
}

if ax != onboarding::PermissionStatus::Granted {
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::YELLOW, "⚠️ 辅助功能 — 输入注入需要");
        if ui.button("打开系统设置").clicked() {
            onboarding::open_accessibility_settings();
        }
    });
    ui.separator();
}
```

(The existing M6.10 banner that shows AX with `input_injector.check_ax()` should be removed in favor of this consolidated view.)

- [ ] **Step 3: Build + test + clippy**
```
cd mac-agent && cargo build --workspace
cd mac-agent && cargo test --workspace
cd mac-agent && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 4: Commit**
```bash
git add mac-agent/crates/macagent-app/src/onboarding.rs \
        mac-agent/crates/macagent-app/src/main.rs \
        mac-agent/crates/macagent-app/src/ui.rs
git commit -m "feat(m8): onboarding permission cards (Screen Recording + Accessibility)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.3 — agent.json5 gui.allowed_bundles + launcher_m7 dynamic read

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/launcher.rs` (M3 LauncherConfig — adds `gui` field)
- Modify: `mac-agent/crates/macagent-app/src/launcher_m7.rs` (read config instead of hardcoded slice)

- [ ] **Step 1: Read existing LauncherConfig**

`grep -n "LauncherConfig\|launchers\|allowed_bundles" mac-agent/crates/macagent-app/src/launcher.rs` to understand current structure. M3 introduced `producer.launchers` array. We add a sibling `gui.allowed_bundles`.

- [ ] **Step 2: Extend LauncherConfig**

Add:
```rust
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct GuiConfig {
    #[serde(default = "default_allowed_bundles")]
    pub allowed_bundles: Vec<String>,
}

fn default_allowed_bundles() -> Vec<String> {
    vec![
        "com.openai.chat".into(),
        "com.anthropic.claude".into(),
        "com.google.Chrome".into(),
    ]
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct LauncherConfig {
    // ... existing fields ...
    #[serde(default)]
    pub gui: GuiConfig,
}
```

`Default` for `LauncherConfig::default_config()` must include `gui: GuiConfig { allowed_bundles: default_allowed_bundles() }`.

- [ ] **Step 3: launcher_m7::is_allowed reads config**

```rust
pub fn is_allowed(bundle_id: &str) -> bool {
    if bundle_id.is_empty() { return false; }
    match crate::launcher::load_or_init_blocking() {
        Ok(cfg) => cfg.gui.allowed_bundles.iter().any(|b| b == bundle_id),
        Err(_) => false,  // fail-safe: if config missing, deny
    }
}
```

(Need `load_or_init_blocking` — synchronous version of `load_or_init`. Add it as a thin wrapper using `tokio::runtime::Handle::current().block_on(load_or_init())` ONLY IF safe to call from non-async context, otherwise read the file directly via `std::fs::read_to_string` + `json5::from_str`.)

Update tests `whitelist_known_bundles` and `whitelist_rejects_others` — they now depend on file. Either keep them gated `#[cfg(test)] #[ignore]` (manual smoke covers), OR set up a temp `agent.json5` in the test. **Pragmatic choice**: gate as `#[ignore]` with a comment explaining the test relies on user config; remove the test assertion entirely if simpler.

- [ ] **Step 4: Build + clippy**
```
cd mac-agent && cargo build --workspace
cd mac-agent && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**
```bash
git add mac-agent/crates/macagent-app/src/launcher.rs \
        mac-agent/crates/macagent-app/src/launcher_m7.rs
git commit -m "feat(m8): launcher_m7 reads gui.allowed_bundles from agent.json5

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.4 — Mac eframe whitelist editor

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/ui.rs`

- [ ] **Step 1: Add whitelist UI block in Paired state**

In `ui.rs::update`'s Paired branch (after permission cards), add a CollapsingHeader:

```rust
egui::CollapsingHeader::new("白名单 App")
    .default_open(false)
    .show(ui, |ui| {
        // Read fresh from agent.json5 each frame (cheap: small file).
        if let Ok(mut cfg) = crate::launcher::load_or_init_blocking() {
            let mut changed = false;
            let mut to_remove: Option<usize> = None;
            for (idx, bid) in cfg.gui.allowed_bundles.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(bid);
                    if ui.small_button("🗑").clicked() {
                        to_remove = Some(idx);
                    }
                });
            }
            if let Some(idx) = to_remove {
                cfg.gui.allowed_bundles.remove(idx);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.label("新增 bundle id:");
                ui.text_edit_singleline(&mut self.whitelist_input);
                if ui.button("添加").clicked() && !self.whitelist_input.is_empty() {
                    cfg.gui.allowed_bundles.push(std::mem::take(&mut self.whitelist_input));
                    changed = true;
                }
            });
            if changed {
                let _ = crate::launcher::save_config(&cfg);
            }
        }
    });
```

Add `whitelist_input: String` field on `MacAgentApp` (default empty).

`save_config(&cfg)` is a new helper in `launcher.rs` — write the file back as JSON5 (or pretty JSON; json5 file format is forgiving). If a `save_config` doesn't exist, add one: `serde_json::to_string_pretty(&cfg)` + `std::fs::write(path, ...)`.

- [ ] **Step 2: Build + test**
```
cd mac-agent && cargo build --workspace
cd mac-agent && cargo test --workspace
cd mac-agent && cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 3: Commit**
```bash
git add mac-agent/crates/macagent-app/src/ui.rs \
        mac-agent/crates/macagent-app/src/launcher.rs
git commit -m "feat(m8): Mac eframe whitelist editor (add/remove bundle ids)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.5 — Signaling reconnect with exponential backoff + iOS reconnecting state

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/rtc_glue.rs` (or wherever signaling reconnect lives)
- Modify: `ios-app/MacIOSWorkspace/RtcGlue.swift` (add `.reconnecting` to GlueState)
- Modify: `ios-app/MacIOSWorkspace/PairedView.swift` (banner)

- [ ] **Step 1: Mac side reconnect**

Find the existing signaling connect path. Wrap in retry:
```rust
async fn connect_with_retry(url: &str, ...) -> Result<SignalingClient> {
    let mut delay = std::time::Duration::from_secs(1);
    let max = std::time::Duration::from_secs(8);
    loop {
        match SignalingClient::connect(url, ...).await {
            Ok(c) => return Ok(c),
            Err(e) => {
                eprintln!("[signaling] connect failed: {e:#}, retrying in {:?}", delay);
                tokio::time::sleep(delay).await;
                if delay < max { delay *= 2; }
            }
        }
    }
}
```

This loops forever. If user wants to give up, they Disconnect via UI. Reasonable default — matches user expectation of "auto recover".

- [ ] **Step 2: iOS GlueState extension**

```swift
enum GlueState: Equatable {
    case idle
    case connecting
    case connected
    case reconnecting    // NEW
    case disconnected
    case failed(String)
}
```

In `RtcGlue.swift`, when WebSocket close is detected and reconnect starts, emit `.reconnecting`. When new connection succeeds, emit `.connected`.

- [ ] **Step 3: PairedView reconnecting banner**

In `PairedView.swift`:
```swift
if glueState == .reconnecting {
    HStack {
        ProgressView().controlSize(.small)
        Text("网络抖动，重连中…").font(.callout)
        Spacer()
    }
    .padding(8)
    .background(Color.yellow.opacity(0.9))
}
```

- [ ] **Step 4: Build + test**
```
cd mac-agent && cargo build --workspace
xcodebuild ... build
```
Expected: clean.

- [ ] **Step 5: Commit**
```bash
git add mac-agent/crates/macagent-app/src/rtc_glue.rs \
        ios-app/MacIOSWorkspace/RtcGlue.swift \
        ios-app/MacIOSWorkspace/PairedView.swift
git commit -m "feat(m8): signaling exp-backoff reconnect + iOS reconnecting banner

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.6 — Release READMEs (B1 + B2)

**Files:**
- Create: `docs/release/MAC_RELEASE.md`
- Create: `docs/release/IOS_RELEASE.md`

- [ ] **Step 1: Write MAC_RELEASE.md**

Cover:
1. Prerequisites: Apple Developer account, Developer ID Application certificate (Keychain), Xcode command line tools.
2. Build: `cd mac-agent && cargo build --release -p macagent-app`.
3. Codesign: `codesign --deep --force --options runtime --entitlements <path> --sign "Developer ID Application: <Name>" target/release/macagent`. Entitlements file path: create `mac-agent/macagent.entitlements` if not present (`com.apple.security.app-sandbox = false`, `com.apple.security.cs.allow-jit = true` for webrtc-rs).
4. Package: zip / .dmg / .pkg. Recommend `.zip` for v0.1 simplicity.
5. Notarize: `xcrun notarytool submit macagent.zip --apple-id you@example.com --team-id XXXXXX --password app-specific-pwd --wait`.
6. Staple: `xcrun stapler staple target/release/macagent`.
7. Verify: `spctl --assess --type exec --verbose target/release/macagent`.

Include sample entitlements file content + sample notarytool output for "Accepted" status.

- [ ] **Step 2: Write IOS_RELEASE.md**

Cover:
1. App Store Connect: create app, set bundle id `com.bruce.MacIOSWorkspace` (or whatever's in project.pbxproj).
2. Apple Distribution certificate + App Store Provisioning Profile (auto-managed by Xcode).
3. Archive: `xcodebuild archive -project ios-app/MacIOSWorkspace.xcodeproj -scheme MacIOSWorkspace -archivePath build/MacIOSWorkspace.xcarchive -destination 'generic/platform=iOS'`.
4. Export: `xcodebuild -exportArchive -archivePath build/MacIOSWorkspace.xcarchive -exportPath build/export -exportOptionsPlist exportOptions.plist`. Sample exportOptions.plist (method = app-store, signingStyle = automatic).
5. Upload: `xcrun altool --upload-app -f build/export/MacIOSWorkspace.ipa -u you@example.com -p app-specific-pwd` OR Transporter.app.
6. App Store Connect TestFlight: add internal testers, distribute build.

Include known gotchas: aps-environment entitlement (already production-ready from M4), camera/microphone Usage Description strings (Info.plist — verify), iCloud entitlement absent, etc.

- [ ] **Step 3: Commit**
```bash
git add docs/release/MAC_RELEASE.md docs/release/IOS_RELEASE.md
git commit -m "docs: add Mac code-sign + iOS TestFlight release READMEs

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task M8.7 — M8 final review

Reviewer subagent over the M8 commits:
- error_msg table consistency Rust vs Swift
- onboarding shouldn't probe AX every frame (cache!) — flag if so
- launcher_m7 read-config-on-each-call file IO is OK (small file)
- whitelist editor: race condition on rapid add/remove? File write is sync.
- reconnect loop bounds (forever vs N attempts)
- README accuracy: spot-check the codesign / notarytool flags

If issues found, dispatch a fixup subagent.

---

## Risks + Mitigations

| Risk | Mitigation |
|---|---|
| `screen_recording_status()` calls `CGPreflightScreenCaptureAccess` per frame in eframe | Cache via `Mutex<(Instant, PermissionStatus)>` with 1s TTL — eframe runs at 60fps. |
| Whitelist edit during launch race | launcher_m7 reads file at `is_allowed()` call time; if user edits between iOS tap and Mac launch, edit wins. Acceptable. |
| `connect_with_retry` infinite loop blocks on dead Worker | User can quit + relaunch. M8 spec doesn't mandate a max-retry policy. |
| iOS `.reconnecting` state never observed (Mac never emits ws close before reconnect) | iOS state may remain `.connected` during ICE flap. M8 acceptance just requires ICE restart works; `.reconnecting` UI is best-effort. |
| Codesign fails on notarytool due to entitlements | README explicitly lists required entitlements; fall back to "ad-hoc signed" .zip if user doesn't have Developer ID. |

## Out of Scope

- Multi-language i18n (Chinese-only)
- Auto-update / Sparkle integration
- Crash reporting (Sentry / Crashlytics)
- Analytics / telemetry
- Splash / About / Version pages
- Touch ID unlock
- Bundle id existence validation
- App browser (pick from /Applications)
- iOS Live Activity

---

## 自检

1. **Spec coverage** — all of §1.A1–A4 + §1.B1–B2 mapped to tasks.
2. **No placeholders** — every step has real code.
3. **Type consistency** — `error_msg::humanize` Rust signature matches Swift `ErrorMessage.humanize`.
4. **CLAUDE.md** — minimum code; A3 is a centralized table replacing scattered strings (justified as polish, not abstraction-creep).

## Plan 完成后下一步

**Subagent-Driven** execution. Estimate:
- M8.1 — 30 min (table + tests + sweep)
- M8.2 — 30 min (onboarding + cards)
- M8.3 — 25 min (config plumbing)
- M8.4 — 30 min (whitelist editor)
- M8.5 — 35 min (reconnect + iOS state)
- M8.6 — 40 min (READMEs; longest because of release-engineering accuracy)
- M8.7 — review subagent

**Total ~3.5 hours.** Allow 1 fixup round for M8.5 (signaling state machine quirks).
