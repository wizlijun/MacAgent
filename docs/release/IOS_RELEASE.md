# iOS Release (TestFlight)

Release flow for `MacIOSWorkspace` to TestFlight (internal/external testers).
Outcome: an `.ipa` uploaded to App Store Connect, processed, and available to testers.

## Prerequisites

- Apple Developer Program membership (paid; same one used for Mac).
- **Apple Distribution** certificate in login Keychain.
  - Distinct from `Apple Development` (used for device builds during dev). Distribution-signed builds are required for App Store Connect upload.
  - Verify: `security find-identity -v -p codesigning | grep "Apple Distribution"`.
- App Store Connect record for the app:
  - Bundle ID: `com.agentanyway.MacIOSWorkspace` (must match `PRODUCT_BUNDLE_IDENTIFIER` in `ios-app/MacIOSWorkspace.xcodeproj/project.pbxproj`). Bundle IDs are globally unique within Apple — once registered to a team, no other team can use it.
  - Register the bundle ID at developer.apple.com → Identifiers if not already.
  - Create the app record at appstoreconnect.apple.com → My Apps → `+`.
- An **App Store Provisioning Profile** for that bundle ID. Easiest: Xcode → Signing & Capabilities → enable "Automatically manage signing" with your team selected. Xcode creates and refreshes the profile.
- App-specific password for upload (same as Mac flow; create a separate one or reuse).
- Team ID (10-char). developer.apple.com → Membership.

## 0. Pre-flight checks (do once per release)

| Check                              | Where                                                                                | Why                                                                 |
|------------------------------------|--------------------------------------------------------------------------------------|---------------------------------------------------------------------|
| `aps-environment` entitlement      | `ios-app/MacIOSWorkspace/MacIOSWorkspace.entitlements`                               | Currently `development`. **Change to `production` before TestFlight upload** — otherwise pushes won't fire on App Store builds. |
| `NSCameraUsageDescription`         | `INFOPLIST_KEY_NSCameraUsageDescription` in `project.pbxproj` (currently set: 用于扫描 Mac 上的配对二维码) | Required because the app uses camera (QR scanner). App Review rejects without it. |
| `NSMicrophoneUsageDescription`     | Add to `INFOPLIST_KEY_*` if mic is ever requested                                    | Currently not used; do not declare unless you actually request mic. |
| Build number monotonic             | `CURRENT_PROJECT_VERSION` in `project.pbxproj`                                       | App Store Connect rejects re-uploads of the same `(version, build)` tuple. Bump `CURRENT_PROJECT_VERSION` every upload. |
| Marketing version                  | `MARKETING_VERSION` (e.g. `0.1.0`)                                                   | Visible to TestFlight users. Bump on user-visible release.          |
| iCloud / Push capabilities present | Capabilities tab in Xcode                                                            | If declared but unused, App Review may flag. Currently only push-related entitlement is declared. |

> **`aps-environment = development` is the single biggest TestFlight gotcha.** Builds uploaded with development APS won't receive production push tokens; iOS clients silently get tokens that the production APNs server rejects. Flip to `production` (or add a release-only entitlements file) before archiving.

## 1. Archive

Archive must use a `generic/platform=iOS` destination — a simulator or specific-device destination produces an unusable archive.

```bash
cd /path/to/macagent

xcodebuild archive \
  -project ios-app/MacIOSWorkspace.xcodeproj \
  -scheme MacIOSWorkspace \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath build/MacIOSWorkspace.xcarchive \
  CODE_SIGN_STYLE=Automatic \
  DEVELOPMENT_TEAM=T5G56DH47L
```

Expected tail:

```
** ARCHIVE SUCCEEDED **
```

Verify the archive structure:

```bash
ls build/MacIOSWorkspace.xcarchive/Products/Applications/
# Expected: MacIOSWorkspace.app
```

If you see `BUILD SUCCEEDED` instead of `ARCHIVE SUCCEEDED`, you ran `build` not `archive` — the resulting `.xcarchive` will be missing the IPA-friendly layout.

### Common archive failures

- `error: No profiles for 'com.agentanyway.MacIOSWorkspace' were found`. Open Xcode once with the team selected so it can register the device-less profile, or generate manually at developer.apple.com → Profiles.
- `error: Provisioning profile "iOS Team Provisioning Profile" doesn't include the currently selected device`. Make sure `-destination` is `generic/platform=iOS`, not a specific device.

## 2. Export IPA

Create `build/exportOptions.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store-connect</string>
    <key>signingStyle</key>
    <string>automatic</string>
    <key>teamID</key>
    <string>T5G56DH47L</string>
    <key>destination</key>
    <string>export</string>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
```

Notes:
- `method`: `app-store-connect` for TestFlight / App Store. Older docs say `app-store` — both work today, `app-store-connect` is the current name.
- `signingStyle`: `automatic` lets Xcode pull the right cert + profile. `manual` requires `signingCertificate` and `provisioningProfiles` keys explicitly.
- `uploadSymbols: true` keeps dSYMs attached so App Store Connect can symbolicate crashes.

Export:

```bash
xcodebuild -exportArchive \
  -archivePath build/MacIOSWorkspace.xcarchive \
  -exportPath build/export \
  -exportOptionsPlist build/exportOptions.plist
```

Expected:

```
** EXPORT SUCCEEDED **
```

Output: `build/export/MacIOSWorkspace.ipa`.

## 3. Upload to App Store Connect

Two paths — pick one.

### Option A: `xcrun altool` (CLI)

```bash
xcrun altool --upload-app \
  --type ios \
  --file build/export/MacIOSWorkspace.ipa \
  --username you@example.com \
  --password 'xxxx-xxxx-xxxx-xxxx'
```

Expected (final line):

```
No errors uploading 'build/export/MacIOSWorkspace.ipa'.
```

> `altool` is being deprecated in favor of `xcrun notarytool` for notarization and `xcrun altool` for uploads (still supported for now). If you hit `altool: command not found` on newer Xcode, use `xcrun altool` or switch to Transporter.

### Option B: Transporter.app (GUI)

Mac App Store → Transporter (free Apple app). Drag `build/export/MacIOSWorkspace.ipa`, sign in with Apple ID + app-specific password, click Deliver. Same outcome, slower for scripting but verbose error messages.

## 4. TestFlight in App Store Connect

After upload, App Store Connect needs 5–30 minutes to "process" the build (encrypt, scan, generate App Thinning variants).

1. App Store Connect → My Apps → MacIOSWorkspace → TestFlight tab.
2. Wait for the build to leave "Processing" — status becomes "Ready to Submit" or "Missing Compliance".
3. **Export Compliance**: when prompted, declare whether the app uses encryption beyond HTTPS. macagent uses WebRTC (DTLS-SRTP) → "Yes", but standard exemptions apply (encryption is for end-user data confidentiality, not regulated). Pick the appropriate exemption; if unsure, consult Apple's docs at the prompt.
4. **Internal testers** (your team): no review needed. Add testers under TestFlight → Internal Testing → `+` → pick the build. They get a TestFlight email link.
5. **External testers** (non-team): requires a beta review by Apple (typically <24h, can be days). Add a public link or invite by email.

## Troubleshooting

- **Build never appears in App Store Connect after `altool` says success.** Check email for "App Store Connect: Issues with your delivery". Common cause: `ITMS-90685` (CFBundleVersion not numeric / not monotonic). Bump `CURRENT_PROJECT_VERSION` and re-upload.
- **`Invalid Bundle. The bundle ... contains disallowed file 'Frameworks/...'`.** A nested framework is missing required Mach-O bits. Usually fixed by clean build (`rm -rf build/ && xcodebuild clean ...`).
- **Push notifications don't fire on TestFlight build but worked in dev.** `aps-environment` is still `development`. Flip to `production`, archive, re-upload.
- **`Missing Push Notification Entitlement`** warning email after upload. The entitlement file lacks `aps-environment` for the resolved configuration, or the App ID at developer.apple.com doesn't have Push Notifications capability enabled. Enable both.
- **`Invalid Code Signature Identifier. The identifier "X" in your code signature for "Y" must match its Bundle Identifier "Z"`.** A nested bundle (extension, framework) has its bundle id out of sync. For macagent this currently has no extensions, so this is a future hazard.
- **Beta App Review rejection — "missing privacy strings".** Even if camera isn't used at launch, if any code path requests it, you must declare `NSCameraUsageDescription`. Same for microphone, photos, etc.
- **Crash on launch only on TestFlight builds, not Xcode-installed Debug.** Almost always a Release-config bug: stripped symbols, dead-code-elimination removing a `@objc` method, or a Swift optimization edge case. Reproduce by running Release config locally: Edit Scheme → Run → Build Configuration → Release.

## Versioning convention

For TestFlight iteration, bump `CURRENT_PROJECT_VERSION` (build number) every upload, even within the same `MARKETING_VERSION`. Example progression:

| MARKETING_VERSION | CURRENT_PROJECT_VERSION | Use                                        |
|-------------------|--------------------------|--------------------------------------------|
| 0.1.0             | 1                        | First TestFlight build                     |
| 0.1.0             | 2                        | Hot-fix during same beta cycle             |
| 0.1.1             | 3                        | Next user-visible release                  |

App Store Connect rejects re-upload of an existing `(MARKETING_VERSION, CURRENT_PROJECT_VERSION)` pair with a generic error — bump and try again.
