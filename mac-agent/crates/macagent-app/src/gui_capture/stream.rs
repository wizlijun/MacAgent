//! Single-window ScreenCaptureKit capture feeding an `H264Encoder` and
//! draining encoded samples into a `VideoTrackHandle`.

use anyhow::{anyhow, Result};
use core_foundation::base::TCFType;
use core_media_rs::{cm_sample_buffer::CMSampleBuffer, cm_time::CMTime};
use core_video_rs::cv_pixel_buffer::CVPixelBuffer;
use macagent_core::rtc_peer::VideoTrackHandle;
use objc2_core_video::CVPixelBuffer as ObjcCVPixelBuffer;
use screencapturekit::{
    output::sc_stream_frame_info::{SCFrameStatus, SCStreamFrameInfo},
    shareable_content::SCShareableContent,
    stream::{
        configuration::{pixel_format::PixelFormat, SCStreamConfiguration},
        content_filter::SCContentFilter,
        delegate_trait::SCStreamDelegateTrait,
        output_trait::SCStreamOutputTrait,
        output_type::SCStreamOutputType,
        SCStream,
    },
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::gui_capture::encoder::H264Encoder;
use crate::gui_capture::VideoConfig;

const FRAME_QUEUE_DEPTH: u32 = 5;
const MAX_LONG_SIDE: u32 = 1920;

/// One captured frame on its way from the SC dispatch queue to the encoder.
struct FramePayload {
    pixel_buffer: objc2_core_foundation::CFRetained<ObjcCVPixelBuffer>,
    pts_micros: i64,
}

// SAFETY: CVPixelBuffer is a CoreFoundation object whose lifetime is governed by
// CFRetain/CFRelease. Sending the retained handle to another thread is safe; we
// never mutate the buffer concurrently (encode_frame only reads).
unsafe impl Send for FramePayload {}

/// Send-able slot holding the most recent retained `CVPixelBuffer`.
struct LastFrame(objc2_core_foundation::CFRetained<ObjcCVPixelBuffer>);

// SAFETY: same rationale as `FramePayload` — Core Foundation refcounting is
// thread-safe; the buffer is read-only after capture.
unsafe impl Send for LastFrame {}

type LastFrameSlot = Arc<Mutex<Option<LastFrame>>>;

struct FrameSink {
    tx: SyncSender<FramePayload>,
    last_frame: LastFrameSlot,
    start_inst: Instant,
}

impl SCStreamOutputTrait for FrameSink {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Screen) {
            return;
        }
        if let Ok(info) = SCStreamFrameInfo::from_sample_buffer(&sample) {
            if !matches!(info.status(), SCFrameStatus::Complete) {
                return;
            }
        }
        let cv_pb: CVPixelBuffer = match sample.get_pixel_buffer() {
            Ok(pb) => pb,
            Err(_) => return,
        };
        let raw = cv_pb.as_concrete_TypeRef() as *mut ObjcCVPixelBuffer;
        let Some(non_null) = NonNull::new(raw) else { return };
        // SAFETY: pointer obtained from a live CMSampleBuffer; CFRetain bumps
        // the refcount so the buffer outlives the sample we drop on return.
        let retained = unsafe { objc2_core_foundation::CFRetained::retain(non_null) };
        let pts = self.start_inst.elapsed().as_micros() as i64;
        // Stash a clone for demote_to_armed before potentially dropping via try_send.
        *self.last_frame.lock().unwrap() = Some(LastFrame(retained.clone()));
        let _ = self.tx.try_send(FramePayload {
            pixel_buffer: retained,
            pts_micros: pts,
        });
    }
}

struct StreamDelegate {
    end_tx: Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
}

impl SCStreamDelegateTrait for StreamDelegate {
    fn did_stop_with_error(&self, _stream: SCStream, error: core_foundation::error::CFError) {
        if let Some(tx) = self.end_tx.lock().unwrap().take() {
            let _ = tx.send(format!("stream_error: {:?}", error));
        }
    }
}

struct ActiveStream {
    sc_stream: Option<SCStream>,
    stop_flag: Arc<AtomicBool>,
    encoder_thread: Option<JoinHandle<()>>,
    tokio_task: tokio::task::JoinHandle<()>,
    last_frame: LastFrameSlot,
}

impl ActiveStream {
    fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(s) = self.sc_stream.take() {
            let _ = s.stop_capture();
            drop(s);
        }
        if let Some(h) = self.encoder_thread.take() {
            let _ = h.join();
        }
        self.tokio_task.abort();
    }

    /// Take the most recent retained pixel buffer, if any.
    fn take_last_frame(&self) -> Option<objc2_core_foundation::CFRetained<ObjcCVPixelBuffer>> {
        self.last_frame.lock().unwrap().take().map(|f| f.0)
    }
}

pub struct StreamManager {
    active: Mutex<Option<(String, ActiveStream)>>,
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub fn start(
        &self,
        sup_id: String,
        window_id: u32,
        track: Arc<VideoTrackHandle>,
        cfg: &VideoConfig,
    ) -> Result<tokio::sync::oneshot::Receiver<String>> {
        if cfg.fps == 0 {
            return Err(anyhow!("VideoConfig.fps must be > 0"));
        }

        let content =
            SCShareableContent::get().map_err(|e| anyhow!("SCShareableContent::get: {:?}", e))?;
        let window = content
            .windows()
            .into_iter()
            .find(|w| w.window_id() == window_id)
            .ok_or_else(|| anyhow!("window {} not found", window_id))?;

        let frame = window.get_frame();
        let (capture_w, capture_h) = clamp_to_max_long_side(
            frame.size.width.round().max(2.0) as u32,
            frame.size.height.round().max(2.0) as u32,
            MAX_LONG_SIDE,
        );

        let frame_interval = CMTime {
            value: 1,
            timescale: cfg.fps as i32,
            flags: 1,
            epoch: 0,
        };
        let config = SCStreamConfiguration::new()
            .set_width(capture_w)
            .map_err(|e| anyhow!("set_width: {:?}", e))?
            .set_height(capture_h)
            .map_err(|e| anyhow!("set_height: {:?}", e))?
            .set_pixel_format(PixelFormat::YCbCr_420v)
            .map_err(|e| anyhow!("set_pixel_format: {:?}", e))?
            .set_minimum_frame_interval(&frame_interval)
            .map_err(|e| anyhow!("set_minimum_frame_interval: {:?}", e))?
            .set_shows_cursor(true)
            .map_err(|e| anyhow!("set_shows_cursor: {:?}", e))?
            .set_queue_depth(FRAME_QUEUE_DEPTH)
            .map_err(|e| anyhow!("set_queue_depth: {:?}", e))?;

        let filter = SCContentFilter::new().with_desktop_independent_window(&window);

        let (frame_tx, frame_rx) = sync_channel::<FramePayload>(2);
        let (sample_tx, mut sample_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::gui_capture::encoder::EncodedSample>();
        let (end_tx, end_rx) = tokio::sync::oneshot::channel::<String>();
        let stop_flag = Arc::new(AtomicBool::new(false));

        let delegate = StreamDelegate {
            end_tx: Mutex::new(Some(end_tx)),
        };
        let mut sc_stream = SCStream::new_with_delegate(&filter, &config, delegate);
        let last_frame: LastFrameSlot = Arc::new(Mutex::new(None));
        sc_stream.add_output_handler(
            FrameSink {
                tx: frame_tx,
                last_frame: last_frame.clone(),
                start_inst: Instant::now(),
            },
            SCStreamOutputType::Screen,
        );

        let cfg_owned = cfg.clone();
        let stop_flag_enc = stop_flag.clone();
        let encoder_thread = std::thread::spawn(move || {
            let mut enc = match H264Encoder::new(capture_w, capture_h, &cfg_owned) {
                Ok(e) => e,
                Err(err) => {
                    eprintln!("[gui_capture] H264Encoder init failed: {err}");
                    return;
                }
            };
            while !stop_flag_enc.load(Ordering::Relaxed) {
                let payload = match frame_rx.recv() {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let pb_ref: &ObjcCVPixelBuffer = &payload.pixel_buffer;
                match enc.encode(pb_ref, payload.pts_micros) {
                    Ok(Some(sample)) => {
                        if sample_tx.send(sample).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!("[gui_capture] encode failed: {err}");
                    }
                }
            }
            let _ = enc.finish();
        });

        let tokio_task = tokio::spawn(async move {
            while let Some(s) = sample_rx.recv().await {
                if track.push_sample(s.data, s.duration).await.is_err() {
                    break;
                }
            }
        });

        sc_stream
            .start_capture()
            .map_err(|e| anyhow!("SCStream::start_capture: {:?}", e))?;

        let active = ActiveStream {
            sc_stream: Some(sc_stream),
            stop_flag,
            encoder_thread: Some(encoder_thread),
            tokio_task,
            last_frame,
        };

        let prev = {
            let mut guard = self.active.lock().unwrap();
            let prev = guard.take().map(|(_, a)| a);
            *guard = Some((sup_id, active));
            prev
        };
        if let Some(prev) = prev {
            prev.stop();
        }

        Ok(end_rx)
    }

    pub fn stop(&self, sup_id: &str) -> bool {
        let entry = {
            let mut guard = self.active.lock().unwrap();
            match &*guard {
                Some((id, _)) if id == sup_id => guard.take(),
                _ => None,
            }
        };
        if let Some((_, active)) = entry {
            active.stop();
            true
        } else {
            false
        }
    }

    /// Stop the active stream for `sup_id` and return its last captured frame.
    pub fn stop_with_last_frame(
        &self,
        sup_id: &str,
    ) -> Option<objc2_core_foundation::CFRetained<ObjcCVPixelBuffer>> {
        let entry = {
            let mut guard = self.active.lock().unwrap();
            match &*guard {
                Some((id, _)) if id == sup_id => guard.take(),
                _ => None,
            }
        };
        let (_, active) = entry?;
        let frame = active.take_last_frame();
        active.stop();
        frame
    }
}

fn clamp_to_max_long_side(w: u32, h: u32, max: u32) -> (u32, u32) {
    let longer = w.max(h);
    if longer <= max {
        return (w & !1, h & !1);
    }
    let scale = max as f64 / longer as f64;
    let nw = ((w as f64) * scale).round().max(2.0) as u32;
    let nh = ((h as f64) * scale).round().max(2.0) as u32;
    (nw & !1, nh & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_smaller() {
        assert_eq!(clamp_to_max_long_side(1280, 720, 1920), (1280, 720));
    }

    #[test]
    fn clamp_2880_to_1920() {
        let (w, h) = clamp_to_max_long_side(2880, 1800, 1920);
        assert_eq!(w, 1920);
        assert!(h <= 1200);
        assert_eq!(w & 1, 0);
        assert_eq!(h & 1, 0);
    }

    #[test]
    fn clamp_even_aligns_odd_dims() {
        let (w, h) = clamp_to_max_long_side(801, 601, 1920);
        assert_eq!(w & 1, 0);
        assert_eq!(h & 1, 0);
    }
}
