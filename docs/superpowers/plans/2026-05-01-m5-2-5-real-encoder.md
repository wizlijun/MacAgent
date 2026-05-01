# M5.2.5 · Real GUI Capture & H.264 Encode 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development（推荐）。

**Background.** M5 plumbing is complete: ctrl protocol carries `WindowsList / SuperviseExisting / SupervisedAck / RemoveSupervised / SupervisionList / StreamEnded / ViewportChanged`; `supervision_router` invokes `gui_capture::supervise_existing(sup_id, window_id, video_track)`; `RtcPeer` exposes a working H.264 `VideoTrackHandle::push_sample`; iOS `RTCMTLVideoView` is wired. The only stub left is `gui_capture/encoder.rs::start_stub` which pushes a single `\x00` byte every 33 ms — not valid H.264, decoder silently drops every frame.

**Goal.** Replace the stub with a real ScreenCaptureKit single-window capture → VideoToolbox hardware H.264 encoder → Annex-B sample feed into the existing `VideoTrackHandle`. iOS sees real live frames at ≥20 fps with <300 ms one-way latency on local Wi-Fi.

**Scope guard.** Inside `gui_capture/` only. Do **not** touch `supervision_router.rs`, `rtc_peer.rs`, `ctrl_msg.rs`, or any iOS code. The public Rust signature `GuiCapture::supervise_existing(&self, sup_id: String, window_id: u32, track: VideoTrackHandle) -> Result<()>` is fixed — it must keep its current name and behavior.

**对应 spec：**
- `docs/superpowers/specs/2026-04-30-mac-iphone-workspace-design.md` §3.1 GuiCapture（30 fps active、SCContentFilter 单窗口、VTCompressionSession H.264）、§5 ScreenCaptureKit 错误。
- `docs/superpowers/plans/2026-05-01-m5-gui-supervise.md` Task M5.2 末尾"如撞墙严重，stub 实现，真编码 push 到 M5.2.5（拆出来）"。

---

## 1. Goal & Acceptance Criteria

### Functional acceptance

1. **iOS displays a live H.264 stream from a chosen Mac window.** Pick a non-trivial window (Cursor / Chrome / Finder), tap → within 1 s `RTCMTLVideoView` shows live frames.
2. **Frame rate ≥20 fps measured at iOS** under default 30 fps capture config on local Wi-Fi.
3. **End-to-end latency <300 ms** (Mac mouse-move → iOS pixel update) on local Wi-Fi. We do not need a precise on-screen timer; visual confirmation by waving a stopwatch is acceptable.
4. **Permission denied path emits `SuperviseReject{code:"permission_denied"}`** instead of crashing.
5. **Window closed mid-stream** → Mac emits `StreamEnded{reason:"window_gone"}` within 2 s; iOS UI returns to list.
6. **`remove_supervised(sup_id)`** stops the SCStream + VTCompressionSession cleanly within 500 ms; no leaked threads or queues; second supervise on a different window works fine.
7. **No regression**: `cargo test -p macagent-core` green; existing M5 loopback negotiation test still passes; iOS xcodebuild green.

### Non-functional acceptance

8. `cargo build -p macagent-app` and `cargo clippy -p macagent-app -- -D warnings` clean.
9. CPU usage of the capture+encode pipeline ≤25 % of one P-core on Apple Silicon for a 1440×900 capture at 30 fps (sanity check via Activity Monitor; not a hard CI gate).
10. The encoder uses **hardware** path on Apple Silicon (verified by setting `kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder = true` and confirming creation succeeds; no software fallback shipped in M5.2.5).

---

## 2. Architecture Decisions

### 2.1 FFI choice for ScreenCaptureKit

**Decision: use `screencapturekit` crate (doom-fish/screencapturekit-rs, version `0.3` line, 1.x API).**

Rationale (per CLAUDE.md "先想再写"):

| Option | Pros | Cons |
|---|---|---|
| **`screencapturekit` crate** | Zero runtime deps. Builder API for `SCContentFilter::with_window` + `SCStreamConfiguration::with_pixel_format` + `with_fps` already match what we need. `SCStreamOutputTrait::did_output_sample_buffer(sample, type)` gives us `CMSampleBuffer` with `image_buffer()` → `CVPixelBuffer`. CMSampleBuffer/CVPixelBuffer types come bundled. Widely used in production. | YCbCr_420v supported but not aliased as "NV12"; same FourCC under the hood, that's fine. Internal binding vs objc2 unknown but irrelevant from caller's POV. |
| Pure objc2 + manual `SCStream` declarations | Zero black box, full control. | ~600 lines of `extern_class!` + `extern_methods!` + delegate plumbing for SC* types we'll only configure once. Pure overhead. |
| `cidre` | Broad Apple coverage. | Not on crates.io, "personal research" repo, single maintainer (yury). Pulling in a non-published dep adds release-engineering risk. |

The crate covers list windows, content filter for one window, configuration, output trait, and image buffer access — exactly the surface we need. **Tradeoff accepted:** if upstream is unmaintained later, we own ~150 lines that could be rewritten directly against objc2-screen-capture-kit. Not a v0 problem.

### 2.2 FFI choice for VideoToolbox

**Decision: use `objc2-video-toolbox = "0.3"` + `objc2-core-video = "0.3"` + `objc2-core-media = "0.3"`.**

Rationale:
- `objc2-video-toolbox` 0.3.2 exposes `VTCompressionSession::create / encode_frame / complete_frames / invalidate / prepare_to_encode_frames`, plus the full `kVTCompressionPropertyKey_*` static set (RealTime, AverageBitRate, MaxKeyFrameInterval, ProfileLevel, ExpectedFrameRate, AllowFrameReordering) and `kVTProfileLevel_H264_Baseline_AutoLevel`. Hardware-accelerated key (`kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder`) is in `objc2-video-toolbox`'s VTVideoEncoderList constants.
- VTCompressionSession is `!Send + !Sync`. Property-set is done through the session ref + a CFDictionary; objc2-video-toolbox does not have a `set_property` convenience method, but we can call `unsafe { VTSessionSetProperty(session_ref, key, value) }` from `objc2-video-toolbox`'s function table or fall back to `core-foundation` ergonomics (see Task M5.2.5.3).
- Already-present `core-foundation` and `core-graphics` workspace deps are sufficient for CFDictionary/CFNumber assembly.

### 2.3 Capture configuration (locked by spec)

| Param | Value | Source |
|---|---|---|
| FPS | **30** | spec §3.1 "Active = 30 fps" + agent.json5 default |
| Pixel format | **YCbCr_420v** (kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange) | needed by VTCompressionSession h264 hardware path; equivalent to NV12 |
| Bitrate | **3000 kbps** (3 Mbps) | M5 plan agent.json5 default `bitrate_kbps: 3000` |
| GOP | keyframe every **5 s** = 150 frames at 30 fps | M5 plan `keyframe_interval_secs: 5` |
| Profile | **Baseline AutoLevel** | M5 plan `codec: "h264_baseline"` |
| Capture width/height | window's logical size **clamped to ≤1920** on the longer side | spec leaves open; clamp at SCStream level for CPU savings on Retina |
| `shows_cursor` | **true** | UX expectation |

Bitrate/GOP/profile come from the existing `VideoConfig` struct in `gui_capture/mod.rs` — no new public knobs added.

### 2.4 Threading model

```
ScreenCaptureKit dispatch queue (Apple-managed)
        │
        │  did_output_sample_buffer(CMSampleBuffer)   ← runs N times/sec
        ▼
[FrameSink trait impl held by SCStream]
        │  extract image_buffer() → CVPixelBuffer (cheap, ref-count bump)
        │  send CVPixelBuffer via std::sync::mpsc::SyncSender (bounded=2, drop-oldest)
        ▼
[Encoder thread spawned by EncoderTask]
        │  recv CVPixelBuffer
        │  VTCompressionSessionEncodeFrame(...)
        │  output callback fires synchronously on encoder thread (because VT
        │  is configured with output_callback rather than handler block)
        ▼
[Output callback]
        │  CMBlockBuffer → AVCC NAL units → Annex-B (start-code prefix)
        │  prepend SPS/PPS in-band on each IDR
        │  hand bytes::Bytes + frame_duration to a Tokio mpsc::UnboundedSender
        ▼
[Tokio task spawned at start]
        │  recv Bytes; track.push_sample(bytes, dur).await
        ▼
WebRTC TrackLocalStaticSample (interceptor handles RTP/SRTP)
```

Why this layout:
- SCKit callback thread should never block on async I/O. A bounded sync mpsc with capacity 2 + drop-oldest naturally handles encoder back-pressure: if the encoder is slow, we drop the oldest frame instead of growing memory.
- VTCompressionSession is `!Send`, so it lives on a single dedicated `std::thread` we spawn ourselves. No tokio worker pollution.
- The async hop into `push_sample` happens on a tokio task that drains an unbounded channel. `push_sample` rarely blocks (webrtc-rs `write_sample` is fast); unbounded is fine because the encoder is the rate-limiter (≤30 frames/sec × ≤300 KB).
- `EncoderTask::stop()` sets a flag, drops the SCStream (which calls `stop_capture`), the SC queue stops sending, the encoder thread sees a closed channel, calls `complete_frames` + `invalidate`, exits. Tokio drain task sees its sender drop, exits.

### 2.5 Annex-B conversion recipe

VideoToolbox hands us a `CMBlockBuffer` whose payload is **AVCC** (length-prefix) format: `[4-byte big-endian length][NAL]…`. webrtc-rs `TrackLocalStaticSample` for H.264 expects **Annex-B** (`0x00000001 [NAL]`). Standard recipe:

1. On the **first IDR-bearing sample**, pull SPS + PPS from `CMSampleBufferGetFormatDescription` → `CMVideoFormatDescriptionGetH264ParameterSetAtIndex(_, 0, …)` for SPS and `(_, 1, …)` for PPS. Cache them. Re-emit them prefixed with `0x00000001` before every keyframe so iOS decoder can re-init after packet loss.
2. For every sample, walk the CMBlockBuffer, read `length: u32 = u32::from_be_bytes(...)`, replace those 4 bytes with `0x00 0x00 0x00 0x01`, slice `length` bytes of NAL, append.
3. Concatenate `[SPS-Annex-B][PPS-Annex-B][VCL-NALs]` (only on keyframe; non-keyframes skip SPS/PPS) into one `bytes::Bytes` and push as one `Sample`.

This is a well-known recipe; ~50 lines of safe Rust over `CMBlockBufferCopyDataBytes`.

### 2.6 Window-resize / dock / minimize handling

- ScreenCaptureKit auto-handles resize: the captured frames change dimensions. We do **not** mid-stream change `VTCompressionSession` resolution (would force re-init); instead we set SC config width/height to the window's *initial* size and let SCKit downscale via aspect-fit if window grows. **This is acceptable for v0**; M7 will properly handle viewport-aware fitting.
- Window minimize / close → `SCStreamDelegate::stream_did_stop_with_error` (or the analogous hook in screencapturekit-rs). When it fires we send a `StreamEnded` signal up to `GuiCapture`. We expose a `take_end_signal()`-style oneshot or hook `on_stream_ended` (the field already exists in `GuiCapture::on_ended`; M5.2.5 will finally wire it).

### 2.7 Hardware encode policy

- Require hardware on creation (`kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder = true`). On creation failure log + return `Err` → router emits `SuperviseReject{code:"encoder_failed"}`. We do not silently fall back to software in M5.2.5.
- Apple Silicon (all M-series) and T2 Intel Macs always satisfy this. Pre-T2 Intel Macs (rare in 2026) fail loudly. Acceptable per simplicity principle.

### 2.8 Why the public surface stays the same

`GuiCapture::supervise_existing(sup_id, window_id, track)` already returns `Result<()>` and stays so. `StreamManager::start(sup_id, track)` becomes `start(sup_id, window_id, track, &VideoConfig)` — the M5.2.5 task changes `mod.rs` to pass `window_id` + a borrowed config (one extra param, internal). `supervision_router.rs` is **not** touched; this satisfies the constraint.

Justification for the internal `start` signature change: the stub did not need `window_id` (it ignored it). Real implementation does. This is internal to the `gui_capture` module — no external API breakage.

---

## 3. Task Breakdown (Subagent-Driven)

Each task is self-contained, ends with a green build + commit. Tasks must run **sequentially** (later tasks depend on earlier types).

### Task M5.2.5.1 — Add deps + scaffolding

**Files:**
- Modify: `mac-agent/Cargo.toml` (workspace deps)
- Modify: `mac-agent/crates/macagent-app/Cargo.toml`

**Changes:**

`mac-agent/Cargo.toml` workspace deps add:
```toml
screencapturekit = "0.3"
objc2 = "0.5"
objc2-foundation = "0.2"
objc2-core-foundation = "0.3"
objc2-core-media = "0.3"
objc2-core-video = "0.3"
objc2-video-toolbox = "0.3"
```

> Pin minor versions; the implementer should bump only the patch level. If the resolver picks up incompatible objc2 0.6 transitively, downgrade — do not chase the latest.

`mac-agent/crates/macagent-app/Cargo.toml` `[dependencies]` add the same names with `{ workspace = true }`.

**Validation:**
- `cargo check -p macagent-app` succeeds (all crates resolve).
- `cargo clippy -p macagent-app -- -D warnings` clean.

**Commit:** `chore(mac-agent): add screencapturekit + objc2-video-toolbox deps for M5.2.5`

---

### Task M5.2.5.2 — Annex-B converter + parameter-set cache (pure-CPU, fully unit-testable)

This is the only piece testable without macOS frameworks running. Doing it first lets us ship a tested utility before touching any framework code.

**Files:**
- Create: `mac-agent/crates/macagent-app/src/gui_capture/annexb.rs`

**Skeleton:**
```rust
//! AVCC (length-prefix) ↔ Annex-B (start-code prefix) for H.264 NAL streams.

use bytes::{BufMut, Bytes, BytesMut};

const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

/// Rewrite `[len_be: u32][nal: len bytes]` records into `[0x00,0x00,0x00,0x01][nal]`.
/// Returns Err if the input is malformed.
pub fn avcc_to_annexb(avcc: &[u8]) -> anyhow::Result<Bytes> { /* ... */ }

/// Build a keyframe sample: SPS + PPS + VCL NALs, all in Annex-B.
pub fn build_keyframe(sps: &[u8], pps: &[u8], vcl_avcc: &[u8]) -> anyhow::Result<Bytes> { /* ... */ }

/// Build a non-keyframe sample: just VCL NALs (no SPS/PPS).
pub fn build_inter(vcl_avcc: &[u8]) -> anyhow::Result<Bytes> { /* ... */ }

#[cfg(test)]
mod tests {
    // Hand-craft a 2-NAL AVCC blob, run avcc_to_annexb, assert byte-exact output.
    // Hand-craft SPS + PPS + 1 VCL, run build_keyframe, assert byte-exact output.
    // Pass a malformed length (overrun) — expect Err.
}
```

**Validation:**
- `cargo test -p macagent-app gui_capture::annexb` green (3 tests).
- `cargo clippy -p macagent-app -- -D warnings` clean.

**Commit:** `feat(mac-agent): add AVCC → Annex-B NAL converter (M5.2.5)`

---

### Task M5.2.5.3 — VTCompressionSession wrapper (`encoder.rs` rewrite)

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/encoder.rs` (full replacement)

This task is purely "talk to VideoToolbox". No SCKit, no tokio. Synchronous API:

```rust
//! H.264 hardware encoder wrapping VTCompressionSession.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::CVPixelBuffer;
use objc2_video_toolbox::VTCompressionSession;

use crate::gui_capture::annexb;
use crate::gui_capture::VideoConfig;

/// One encoded H.264 sample, already in Annex-B form, ready for push_sample.
pub struct EncodedSample {
    pub data: Bytes,
    pub is_keyframe: bool,
    pub duration: std::time::Duration,
}

pub struct H264Encoder {
    session: VTCompressionSession,    // !Send — never crosses thread boundary
    width: u32,
    height: u32,
    cached_sps: Option<Vec<u8>>,
    cached_pps: Option<Vec<u8>>,
    out_rx: std::sync::mpsc::Receiver<EncodedSample>,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, cfg: &VideoConfig) -> Result<Self> {
        // 1. VTCompressionSessionCreate
        //    - encoder_specification: dict with kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder=true
        //    - source_image_buffer_attributes: nil
        //    - output_callback: extern "C" fn that sends EncodedSample into out_tx
        // 2. Set properties:
        //      kVTCompressionPropertyKey_RealTime = true
        //      kVTCompressionPropertyKey_ProfileLevel = kVTProfileLevel_H264_Baseline_AutoLevel
        //      kVTCompressionPropertyKey_AverageBitRate = cfg.bitrate_kbps * 1000
        //      kVTCompressionPropertyKey_ExpectedFrameRate = cfg.fps
        //      kVTCompressionPropertyKey_MaxKeyFrameInterval = cfg.fps * cfg.keyframe_interval_secs
        //      kVTCompressionPropertyKey_AllowFrameReordering = false
        // 3. VTCompressionSessionPrepareToEncodeFrames (best-effort)
        unimplemented!()
    }

    pub fn encode(&mut self, pixel_buffer: &CVPixelBuffer, pts: i64) -> Result<Option<EncodedSample>> {
        // VTCompressionSessionEncodeFrame(session, pb, CMTime{pts, 90_000}, kCMTimeInvalid, nil, nil, &mut info_flags)
        // After call, drain self.out_rx (try_recv) — at most one entry.
        unimplemented!()
    }

    pub fn finish(&mut self) -> Result<()> {
        // VTCompressionSessionCompleteFrames(self.session, kCMTimeInvalid)
        unimplemented!()
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        // VTCompressionSessionInvalidate(self.session)
    }
}

// SAFETY: H264Encoder is owned by a single dedicated thread; VTCompressionSession
// is !Send, so we never `Send` H264Encoder across threads.
```

**Output-callback bridge:** C signature `void OutputCallback(void* outputCallbackRefCon, void* sourceFrameRefCon, OSStatus status, VTEncodeInfoFlags infoFlags, CMSampleBufferRef sampleBuffer)`. Pass `Box::into_raw(Box::new(SyncSender<EncodedSample>))` as refCon. In `extern "C"` callback:
1. Skip on `status != 0` or null sampleBuffer.
2. Detect keyframe via `CMSampleBufferGetSampleAttachmentsArray` (absence of `kCMSampleAttachmentKey_NotSync`).
3. On keyframe: pull SPS+PPS via `CMVideoFormatDescriptionGetH264ParameterSetAtIndex`, cache.
4. `CMBlockBufferCopyDataBytes` → AVCC bytes.
5. Build Annex-B (`annexb::build_keyframe` or `build_inter`).
6. `tx.send(EncodedSample{...})`.

**Validation:**
- `cargo build -p macagent-app` succeeds.
- `cargo test -p macagent-app gui_capture::encoder::smoke_create` (constructs encoder + drops it; gated `#[cfg(target_os="macos")]`).
- `cargo clippy -- -D warnings` clean.

**Commit:** `feat(mac-agent): add VTCompressionSession H.264 encoder (M5.2.5)`

---

### Task M5.2.5.4 — ScreenCaptureKit single-window stream (`stream.rs` rewrite)

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/stream.rs` (full replacement of stub)

**Skeleton:**

```rust
//! Single-window ScreenCaptureKit capture feeding an H264Encoder feeding a VideoTrackHandle.

use crate::gui_capture::encoder::{EncodedSample, H264Encoder};
use crate::gui_capture::VideoConfig;
use anyhow::{anyhow, Context, Result};
use macagent_core::rtc_peer::VideoTrackHandle;
use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct ActiveStream {
    sc_stream: Arc<Mutex<Option<SCStream>>>,
    stop_flag: Arc<AtomicBool>,
    encoder_thread: Option<JoinHandle<()>>,
    tokio_task: tokio::task::JoinHandle<()>,
    end_rx: tokio::sync::oneshot::Receiver<String>,
}

impl ActiveStream {
    pub fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(s) = self.sc_stream.lock().unwrap().take() {
            let _ = s.stop_capture();
        }
        if let Some(h) = self.encoder_thread { let _ = h.join(); }
        self.tokio_task.abort();
    }
}

struct FramePayload {
    pixel_buffer: CVPixelBuffer,
    pts_micros: i64,
}

struct FrameSink {
    tx: SyncSender<FramePayload>,
    start_inst: std::time::Instant,
}

impl SCStreamOutputTrait for FrameSink {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Screen) { return; }
        let Some(pb) = sample.image_buffer() else { return };
        let pts = self.start_inst.elapsed().as_micros() as i64;
        let _ = self.tx.try_send(FramePayload { pixel_buffer: pb, pts_micros: pts });
    }
}

pub struct StreamManager {
    active: Mutex<Option<(String, ActiveStream)>>,
}

impl StreamManager {
    pub fn new() -> Self { Self { active: Mutex::new(None) } }

    pub fn start(
        &self,
        sup_id: String,
        window_id: u32,
        track: Arc<VideoTrackHandle>,
        cfg: &VideoConfig,
    ) -> Result<tokio::sync::oneshot::Receiver<String>> {
        // 1. SCShareableContent::get(); find SCWindow with matching CGWindowID.
        // 2. Determine capture w/h: clamp window.frame() longer side to 1920.
        // 3. SCStreamConfiguration: width/height, .with_pixel_format(YCbCr_420v),
        //    .with_fps(cfg.fps), .with_shows_cursor(true).
        // 4. (frame_tx, frame_rx) = sync_channel(2)
        // 5. SCStream::new + add_output_handler(FrameSink, Screen)
        // 6. SCStreamDelegate to catch stream_did_stop_with_error → end_tx oneshot.
        // 7. spawn std::thread "encoder":
        //      let mut enc = H264Encoder::new(w, h, cfg)?;
        //      while let Ok(payload) = frame_rx.recv() {
        //          if let Ok(Some(sample)) = enc.encode(&payload.pixel_buffer, payload.pts_micros) {
        //              let _ = sample_tx.send(sample);
        //          }
        //      }
        //      let _ = enc.finish();
        //  (sample_tx, sample_rx) = tokio::sync::mpsc::unbounded_channel()
        // 8. spawn tokio task: while let Some(s) = sample_rx.recv().await { track.push_sample(s.data, s.duration).await; }
        // 9. sc_stream.start_capture()?
        // 10. Stash ActiveStream in self.active under sup_id; return end_rx.
        unimplemented!()
    }

    pub fn stop(&self, sup_id: &str) -> bool { unimplemented!() }
}
```

**Validation:**
- `cargo build -p macagent-app` succeeds.
- `cargo test -p macagent-app` green (existing tests unaffected).
- `cargo clippy -- -D warnings` clean.

**Commit:** `feat(mac-agent): wire SCStream → VTCompressionSession → VideoTrackHandle (M5.2.5)`

---

### Task M5.2.5.5 — `mod.rs` glue + `on_stream_ended` activation

**Files:**
- Modify: `mac-agent/crates/macagent-app/src/gui_capture/mod.rs`
- (Maybe) Modify: `mac-agent/crates/macagent-app/src/ui.rs` — add stream-end callback registration only if not already wired.

**Changes (delta only):**

1. `supervise_existing` now passes `window_id` and `&self.config` to `StreamManager::start`.
2. `StreamManager::start` returns `oneshot::Receiver<String>`. Spawn a tokio task that awaits it and, on resolve, invokes `self.on_ended` callback with `(sup_id, reason)`.

```rust
pub async fn supervise_existing(
    &self,
    sup_id: String,
    window_id: u32,
    track: VideoTrackHandle,
) -> Result<()> {
    let track = Arc::new(track);
    let end_rx = self.streams.start(sup_id.clone(), window_id, track, &self.config)?;
    let cb = self.on_ended.lock().unwrap().clone();
    let sid = sup_id.clone();
    tokio::spawn(async move {
        if let Ok(reason) = end_rx.await {
            if let Some(cb) = cb { cb(sid, reason); }
        }
    });
    Ok(())
}
```

> **Decision rule for the implementer:** read `ui.rs` first. If `GuiCapture::on_stream_ended` is never registered today (likely; M5.2 stub never fired), add the registration at the GuiCapture creation site. Forward `(sup_id, reason)` into the existing `ctrl_tx` as `CtrlPayload::StreamEnded { sup_id, reason }`. Minimum viable wiring (~8 lines), justified because the path was dead before.

**Validation:**
- `cargo build -p macagent-app` green.
- Existing tests still pass.

**Commit:** `feat(mac-agent): wire StreamEnded callback for window-gone + permission-lost (M5.2.5)`

---

### Task M5.2.5.6 — Manual smoke test (no commit)

1. `cargo build -p macagent-app --release`.
2. Launch agent. Pair with iPhone (existing flow).
3. iPhone → 桌面 → tap a window with animation (Cursor cursor blink, video, `cmatrix` in Terminal).
4. Expected:
   - First-time SCStream creation prompts Screen Recording permission. After granting, restart agent.
   - iPhone shows window in <1 s.
   - Frame rate visibly fluid (≥20 fps).
   - Mouse-move on Mac → iPhone updates within ~½ s.
5. Close captured window on Mac → iPhone returns to list within 2 s, no crash.
6. Pick different window → it streams.
7. Activity Monitor: `macagent` CPU <30% on Apple Silicon.

If any of (4)–(7) fail: bug. Do **not** commit a half-working state. CLAUDE.md "不偷懒".

**No commit; outcome recorded in M5.2.5.7 review.**

---

### Task M5.2.5.7 — M5.2.5 final review

Dispatch reviewer subagent. Walk threading invariants in §2.4. Audit unsafe blocks. Confirm no `unwrap()` on user-controllable paths. Confirm no `panic!` in callback code (panic in extern "C" callback = UB).

---

## 4. Risks + Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `objc2-video-toolbox` 0.3 missing some property keys (e.g. `RequireHardwareAcceleratedVideoEncoder`) | Medium | Implementer falls back to manual `extern "C"` decls | Declare inline: `extern "C" { static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef; }`, link `VideoToolbox`. |
| Panic inside `extern "C"` callback | Medium | UB / abort | Wrap callback bodies in `std::panic::catch_unwind`; on panic, log + set stop flag. |
| `CVPixelBuffer` `Send` confusion | Medium | Compile error | Tiny newtype with `unsafe impl Send {}`; CVPixelBuffer is CFRetain/Release-managed and thread-safe per Apple. Document with one-line justification. |
| `screencapturekit` crate's delegate hook for `stream_did_stop_with_error` not exposed in 0.3 | Medium | Can't detect window-close auto-end | Fallback: poll `CGWindowListCopyWindowInfo` once/sec from tokio drain task; if window_id disappears, fire end with reason `"window_gone"`. ~15 lines. |
| Hardware encoder unavailable on test machine | Low (Apple Silicon dev) | Encoder creation fails | Acceptance §10: required hardware-only, fail loudly. |
| Latency >300 ms because of tokio scheduling delay | Low | Misses §3 acceptance | Switch tokio drain to `mpsc::channel(8)` (bounded); or have encoder thread call `push_sample` via `Handle::block_on`. |
| webrtc-rs `write_sample` rejects samples lacking SPS/PPS in keyframe | Low | iOS green frames | §2.5 recipe always includes them; reviewer checks first-keyframe layout. |
| FPS clamping by SCKit when window partially occluded | Low | Frame rate drops below 20 | Out of scope; documented. |

---

## 5. Out of Scope (M5.2.5 explicitly does NOT do)

- Multi-supervise / armed states.
- `fit_window` (M7).
- `supervise_launch` (M7).
- Input injection (M6).
- Software encoder fallback.
- Adaptive bitrate.
- Audio capture.
- HDR / 10-bit.
- CI for capture pipeline (manual smoke test only).
- Screenshot fallback for "armed" supervisions (M7).

---

## 6. Plan 完成后下一步

Suggested execution: **Subagent-Driven**.
- M5.2.5.1 (deps) — ~5 min.
- M5.2.5.2 (annex-b) — ~30 min, fully testable.
- M5.2.5.3 (encoder) — biggest single risk; allow 1–2 fixups for FFI quirks.
- M5.2.5.4 (stream) — moderate risk.
- M5.2.5.5 (glue) — trivial.
- M5.2.5.6 (smoke) — manual.
- M5.2.5.7 (review) — reviewer subagent.

Total estimate: 1 working day for a focused subagent loop with 2–3 fixup rounds.
