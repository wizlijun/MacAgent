# Mac Release (Developer ID + Notarize)

Direct-distribution release for `macagent` (menu-bar binary, not Mac App Store).
Outcome: a notarized, stapled `.zip` you can hand to users; Gatekeeper accepts it on first launch.

## Prerequisites

- Apple Developer Program membership (paid).
- **Developer ID Application** certificate installed in login Keychain.
  - Distinct from `Developer ID Installer` (signs `.pkg`) and `Apple Distribution` (App Store / TestFlight). For a `.zip` of a binary you need **Developer ID Application**.
  - Verify: `security find-identity -v -p codesigning` — look for a line starting with `Developer ID Application: <Name> (TEAMID)`.
- Xcode command line tools: `xcode-select --install`. We need `codesign`, `xcrun notarytool`, `xcrun stapler`.
- An **app-specific password** for `notarytool` (NOT your Apple ID password).
  - Create at appleid.apple.com → Sign-In and Security → App-Specific Passwords.
  - Name it e.g. `notarytool-macagent`. Copy the 4×4 password (`xxxx-xxxx-xxxx-xxxx`).
- Your **Team ID** (10-char alphanumeric).
  - Find via `xcrun notarytool history --apple-id you@example.com --password 'xxxx-xxxx-xxxx-xxxx' --team-id ...` (chicken-and-egg) — easier: developer.apple.com → Membership → Team ID. Also visible in Keychain cert subject.

## 1. Build

Release build of the menu-bar binary. The `macagent-app` crate produces a single bin named `macagent`.

```bash
cd /path/to/macagent/mac-agent
cargo build --release -p macagent-app
```

Output: `mac-agent/target/release/macagent` (Mach-O executable).

Verify it runs unsigned (will warn about Gatekeeper but launch):

```bash
./target/release/macagent --version 2>&1 || true
file target/release/macagent
# Expected: Mach-O 64-bit executable arm64 (or x86_64)
```

> Universal binary (arm64+x86_64): build twice with `--target` and `lipo` together. Out of scope for v0.1 — ship native arch only.

## 2. Code sign

Use the entitlements file at `mac-agent/macagent.entitlements` (committed in this repo). Required because the binary embeds `webrtc-rs` (JIT-style memory) and dynamically loads private system frameworks (ScreenCaptureKit / VideoToolbox). Without these entitlements the binary either fails to notarize or SIGKILLs at runtime under hardened runtime.

```bash
cd /path/to/macagent/mac-agent

codesign \
  --force \
  --options runtime \
  --timestamp \
  --entitlements macagent.entitlements \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  target/release/macagent
```

Flag rationale:
- `--options runtime` — enables hardened runtime. **Required** for notarization; without it, notarytool rejects the submission.
- `--timestamp` — embeds a secure timestamp from Apple's TSA. Required for notarization.
- `--force` — re-sign if already signed.
- `--entitlements` — path to plist. The plist must be the unsigned/plain-text version (not a binary `.entitlements` blob).

Verify the signature:

```bash
codesign --verify --verbose=2 target/release/macagent
codesign --display --entitlements - target/release/macagent
```

Expected first command output:

```
target/release/macagent: valid on disk
target/release/macagent: satisfies its Designated Requirement
```

Expected second command: prints the entitlements plist back to stdout. If it shows a binary plist (starts with `bplist00`), pipe through `plutil -convert xml1 -o - -`.

### Common pitfalls

- **`Developer ID Installer` won't work.** Codesign accepts it but notarization rejects the result. Double-check the cert name is exactly `Developer ID Application:`.
- **Self-signed / Apple Development certs won't work for distribution.** They sign locally fine but notarization rejects.
- `errSecInternalComponent` from codesign usually means the private key for the cert isn't in the Keychain (you have the public cert but not the private key). Re-import the `.p12` Apple gave you when you created the cert.

## 3. Package + Notarize

Notarytool requires a `.zip`, `.pkg`, or `.dmg`. Bare Mach-O binaries are not accepted.

```bash
cd /path/to/macagent/mac-agent
ditto -c -k --keepParent target/release/macagent target/release/macagent.zip
```

> Use `ditto`, not `zip`. The `zip` CLI strips extended attributes and the embedded code signature can become invalid.

Submit:

```bash
xcrun notarytool submit target/release/macagent.zip \
  --apple-id you@example.com \
  --team-id TEAMID \
  --password 'xxxx-xxxx-xxxx-xxxx' \
  --wait
```

`--wait` blocks until Apple finishes (typically 1–5 minutes; can be 30+ on a bad day). Expected tail:

```
Successfully uploaded file
  id: 12345678-90ab-cdef-1234-567890abcdef
  ...
Current status: Accepted .........

Conclusion: Accepted
```

If `Conclusion: Invalid`, fetch the log:

```bash
xcrun notarytool log <submission-id> \
  --apple-id you@example.com --team-id TEAMID --password 'xxxx-xxxx-xxxx-xxxx'
```

The log JSON enumerates per-file issues. Most common failures and fixes:

| Failure                                            | Fix                                                                  |
|----------------------------------------------------|----------------------------------------------------------------------|
| `The signature does not include a secure timestamp.` | Re-codesign with `--timestamp`.                                      |
| `The executable does not have the hardened runtime enabled.` | Re-codesign with `--options runtime`.                                |
| `The binary uses an SDK older than the 10.9 SDK.`  | Rebuild with current toolchain; `MACOSX_DEPLOYMENT_TARGET=10.13+`.   |
| `disallowed entitlement <entitlement>`             | Don't request entitlements you don't actually need.                  |

## 4. Staple + verify

Stapling pins the notarization ticket onto the artifact so Gatekeeper can verify offline.

```bash
# Notarytool gave us a ticket for the .zip; we staple the *binary* (or the .app bundle if you have one).
# For a single binary inside a .zip, you must staple the binary, then re-zip.
xcrun stapler staple target/release/macagent
```

Expected:

```
Processing: .../target/release/macagent
The staple and validate action worked!
```

Re-zip the stapled binary for distribution:

```bash
ditto -c -k --keepParent target/release/macagent target/release/macagent-stapled.zip
```

Verify Gatekeeper acceptance:

```bash
spctl --assess --type exec --verbose target/release/macagent
```

Expected:

```
target/release/macagent: accepted
source=Notarized Developer ID
```

If you see `source=Developer ID` (without "Notarized"), stapling didn't take — re-run `stapler staple` and check `stapler validate target/release/macagent`.

## Troubleshooting

- **First-launch quarantine prompt still appears for users.** Expected on first download — Gatekeeper validates the staple offline and approves; subsequent launches are silent.
- **`codesign` succeeds but `spctl` says `rejected, source=obsolete resource envelope`.** You're signing a binary that has nested resources (e.g. an `.app` wrapper). For a single Mach-O this should not happen; if you wrap into an `.app`, codesign with `--deep` and sign frameworks first.
- **notarytool rejects with `The binary is not signed with a valid Developer ID certificate.`** The cert chain didn't reach Apple's root. Check your Keychain — you should see `Developer ID Certification Authority` and `Apple Root CA` as parents. If not, download from Apple PKI: https://www.apple.com/certificateauthority/
- **Hardened runtime SIGKILL at launch (`Code Signature Invalid`, exit 9).** Entitlements were not embedded. Re-run codesign with `--entitlements` and confirm `codesign --display --entitlements -` shows them back.
- **`webrtc` panics on first ICE gather under hardened runtime.** Missing `com.apple.security.cs.allow-unsigned-executable-memory`. Already in `macagent.entitlements`; check it wasn't stripped.
- **Apple Silicon vs Intel.** `cargo build --release` produces native arch only. Test on both archs if you advertise universal support. `lipo -info target/release/macagent` shows the embedded archs.

## Distribution

Hand users `macagent-stapled.zip`. They unzip, drag to `~/Applications` (or anywhere), launch. First launch shows the standard "downloaded from Internet" dialog; subsequent launches are silent.

Auto-update / Sparkle integration is out of scope for v0.1.
