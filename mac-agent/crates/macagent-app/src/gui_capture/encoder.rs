//! H.264 hardware encoder wrapping VTCompressionSession.

use anyhow::{anyhow, Result};
use bytes::Bytes;
use core::ffi::c_void;
use core::ptr::NonNull;
use objc2_core_foundation::{
    kCFBooleanFalse, kCFBooleanTrue, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks, CFDictionary, CFNumber, CFNumberType, CFRetained, CFString,
    CFType,
};
use objc2_core_media::{
    kCMSampleAttachmentKey_NotSync, kCMVideoCodecType_H264, CMSampleBuffer, CMTime,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::CVPixelBuffer;
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTProfileLevel_H264_Baseline_AutoLevel,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionSetProperty,
};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use crate::gui_capture::annexb;
use crate::gui_capture::VideoConfig;

const PTS_TIMESCALE: i32 = 1_000_000;

/// One encoded H.264 sample (Annex-B), ready for `VideoTrackHandle::push_sample`.
pub struct EncodedSample {
    pub data: Bytes,
    pub is_keyframe: bool,
    pub duration: Duration,
}

pub struct H264Encoder {
    session: CFRetained<VTCompressionSession>,
    out_rx: Receiver<EncodedSample>,
    callback_ctx: *mut CallbackCtx,
    frame_dur: Duration,
    _not_send: core::marker::PhantomData<*const ()>,
}

struct CallbackCtx {
    tx: SyncSender<EncodedSample>,
    frame_dur: Duration,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, cfg: &VideoConfig) -> Result<Self> {
        if cfg.fps == 0 {
            return Err(anyhow!("VideoConfig.fps must be > 0"));
        }
        let frame_dur = Duration::from_secs_f64(1.0 / cfg.fps as f64);

        let (tx, out_rx) = sync_channel::<EncodedSample>(8);
        let callback_ctx = Box::into_raw(Box::new(CallbackCtx {
            tx,
            frame_dur,
        }));

        let spec_dict = unsafe { build_hw_required_spec()? };

        let mut session_ptr: *mut VTCompressionSession = core::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                Some(&spec_dict),
                None,
                None,
                Some(output_callback),
                callback_ctx as *mut c_void,
                NonNull::new(&mut session_ptr).unwrap(),
            )
        };
        if status != 0 || session_ptr.is_null() {
            unsafe { drop(Box::from_raw(callback_ctx)) };
            return Err(anyhow!(
                "VTCompressionSessionCreate (hardware-required) failed: status={}",
                status
            ));
        }
        let session: CFRetained<VTCompressionSession> =
            unsafe { CFRetained::from_raw(NonNull::new_unchecked(session_ptr)) };

        unsafe {
            set_bool(&session, kVTCompressionPropertyKey_RealTime, true)?;
            set_cf(
                &session,
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_H264_Baseline_AutoLevel,
            )?;
            let bitrate = (cfg.bitrate_kbps as i32).saturating_mul(1000);
            set_i32(
                &session,
                kVTCompressionPropertyKey_AverageBitRate,
                bitrate,
            )?;
            set_i32(
                &session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                cfg.fps as i32,
            )?;
            let gop = (cfg.fps as i32).saturating_mul(cfg.keyframe_interval_secs as i32);
            set_i32(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                gop.max(1),
            )?;
            set_bool(&session, kVTCompressionPropertyKey_AllowFrameReordering, false)?;
            let _ = session.prepare_to_encode_frames();
        }

        Ok(Self {
            session,
            out_rx,
            callback_ctx,
            frame_dur,
            _not_send: core::marker::PhantomData,
        })
    }

    pub fn encode(
        &mut self,
        pixel_buffer: &CVPixelBuffer,
        pts_micros: i64,
    ) -> Result<Option<EncodedSample>> {
        let pts = unsafe { CMTime::new(pts_micros, PTS_TIMESCALE) };
        let dur_micros = self.frame_dur.as_micros() as i64;
        let dur = unsafe { CMTime::new(dur_micros, PTS_TIMESCALE) };
        let mut info_flags = VTEncodeInfoFlags(0);
        let status = unsafe {
            self.session.encode_frame(
                pixel_buffer,
                pts,
                dur,
                None,
                core::ptr::null_mut(),
                &mut info_flags,
            )
        };
        if status != 0 {
            return Err(anyhow!("VTCompressionSessionEncodeFrame failed: {}", status));
        }
        Ok(self.out_rx.try_recv().ok())
    }

    pub fn finish(&mut self) -> Result<()> {
        let invalid = unsafe { CMTime::new(0, 0) };
        let status = unsafe { self.session.complete_frames(invalid) };
        if status != 0 {
            return Err(anyhow!(
                "VTCompressionSessionCompleteFrames failed: {}",
                status
            ));
        }
        Ok(())
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        unsafe {
            self.session.invalidate();
            drop(Box::from_raw(self.callback_ctx));
        }
    }
}

unsafe extern "C-unwind" fn output_callback(
    output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if output_ref_con.is_null() {
            return;
        }
        let ctx: &CallbackCtx = unsafe { &*(output_ref_con as *const CallbackCtx) };
        if status != 0 || sample_buffer.is_null() || info_flags.contains(VTEncodeInfoFlags::FrameDropped) {
            return;
        }
        let sample: &CMSampleBuffer = unsafe { &*sample_buffer };
        match build_encoded_sample(sample, ctx.frame_dur) {
            Ok(Some(enc)) => {
                let _ = ctx.tx.try_send(enc);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[encoder] failed to build encoded sample: {}", e);
            }
        }
    }));
    if result.is_err() {
        eprintln!("[encoder] panic in extern callback (recovered)");
    }
}

fn build_encoded_sample(sample: &CMSampleBuffer, frame_dur: Duration) -> Result<Option<EncodedSample>> {
    let is_keyframe = unsafe { detect_keyframe(sample) };
    let avcc = unsafe { copy_data_bytes(sample)? };
    let data = if is_keyframe {
        let (sps, pps) = unsafe { extract_sps_pps(sample) }
            .ok_or_else(|| anyhow!("keyframe missing SPS/PPS"))?;
        annexb::build_keyframe(&sps, &pps, &avcc)?
    } else {
        annexb::build_inter(&avcc)?
    };
    Ok(Some(EncodedSample {
        data,
        is_keyframe,
        duration: frame_dur,
    }))
}

unsafe fn detect_keyframe(sample: &CMSampleBuffer) -> bool {
    let Some(arr) = sample.sample_attachments_array(false) else {
        return true;
    };
    if arr.count() < 1 {
        return true;
    }
    let dict_ptr = unsafe { arr.value_at_index(0) };
    if dict_ptr.is_null() {
        return true;
    }
    let dict: &CFDictionary = unsafe { &*(dict_ptr as *const CFDictionary) };
    let key_ptr: *const CFString = kCMSampleAttachmentKey_NotSync;
    let val_ptr = unsafe { dict.value(key_ptr as *const c_void) };
    if val_ptr.is_null() {
        return true;
    }
    let val: &CFType = unsafe { &*(val_ptr as *const CFType) };
    let false_ptr = match unsafe { kCFBooleanFalse } {
        Some(b) => b as *const _ as *const CFType,
        None => return false,
    };
    core::ptr::eq(val as *const CFType, false_ptr)
}

unsafe fn copy_data_bytes(sample: &CMSampleBuffer) -> Result<Vec<u8>> {
    let bb = sample
        .data_buffer()
        .ok_or_else(|| anyhow!("sample has no data buffer"))?;
    let len = bb.data_length();
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len];
    let dst = NonNull::new(buf.as_mut_ptr() as *mut c_void)
        .ok_or_else(|| anyhow!("vec ptr was null"))?;
    let status = unsafe { bb.copy_data_bytes(0, len, dst) };
    if status != 0 {
        return Err(anyhow!("CMBlockBufferCopyDataBytes failed: {}", status));
    }
    Ok(buf)
}

unsafe fn extract_sps_pps(sample: &CMSampleBuffer) -> Option<(Vec<u8>, Vec<u8>)> {
    let fd = sample.format_description()?;
    let sps = read_param_set(&fd, 0)?;
    let pps = read_param_set(&fd, 1)?;
    Some((sps, pps))
}

unsafe fn read_param_set(
    fd: &objc2_core_media::CMFormatDescription,
    idx: usize,
) -> Option<Vec<u8>> {
    let mut ptr: *const u8 = core::ptr::null();
    let mut size: usize = 0;
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            fd,
            idx,
            &mut ptr,
            &mut size,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    if status != 0 || ptr.is_null() || size == 0 {
        return None;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr, size) };
    Some(slice.to_vec())
}

unsafe fn build_hw_required_spec() -> Result<CFRetained<CFDictionary>> {
    let key: *const CFString = kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder;
    let val_ref = unsafe { kCFBooleanTrue }
        .ok_or_else(|| anyhow!("kCFBooleanTrue is null"))?;
    let val: *const c_void = val_ref as *const _ as *const c_void;
    let mut keys: [*const c_void; 1] = [key as *const c_void];
    let mut values: [*const c_void; 1] = [val];
    let dict = unsafe {
        CFDictionary::new(
            None,
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    .ok_or_else(|| anyhow!("CFDictionaryCreate returned null"))?;
    Ok(dict)
}

unsafe fn set_bool(
    session: &VTCompressionSession,
    key: &CFString,
    value: bool,
) -> Result<()> {
    let b = if value {
        unsafe { kCFBooleanTrue }
    } else {
        unsafe { kCFBooleanFalse }
    }
    .ok_or_else(|| anyhow!("CFBoolean static is null"))?;
    let session_ref: &CFType = session;
    let status = unsafe { VTSessionSetProperty(session_ref, key, Some(b)) };
    if status != 0 {
        return Err(anyhow!("VTSessionSetProperty(bool) status={}", status));
    }
    Ok(())
}

unsafe fn set_cf(
    session: &VTCompressionSession,
    key: &CFString,
    value: &CFString,
) -> Result<()> {
    let session_ref: &CFType = session;
    let val_cf: &CFType = value;
    let status = unsafe { VTSessionSetProperty(session_ref, key, Some(val_cf)) };
    if status != 0 {
        return Err(anyhow!("VTSessionSetProperty(cf) status={}", status));
    }
    Ok(())
}

unsafe fn set_i32(
    session: &VTCompressionSession,
    key: &CFString,
    value: i32,
) -> Result<()> {
    let num = unsafe {
        CFNumber::new(
            None,
            CFNumberType::SInt32Type,
            &value as *const i32 as *const c_void,
        )
    }
    .ok_or_else(|| anyhow!("CFNumberCreate returned null"))?;
    let session_ref: &CFType = session;
    let num_cf: &CFType = &num;
    let status = unsafe { VTSessionSetProperty(session_ref, key, Some(num_cf)) };
    if status != 0 {
        return Err(anyhow!("VTSessionSetProperty(i32) status={}", status));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn smoke_create() {
        let cfg = VideoConfig::default();
        let enc = H264Encoder::new(640, 480, &cfg)
            .expect("hardware encoder must be available on this dev machine");
        drop(enc);
    }
}
