//! CVPixelBuffer -> scaled CGImage -> JPEG -> base64. ~256x192 @ Q70.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use core_foundation::base::{CFRelease, TCFType};
use core_foundation::base::{CFIndex, CFTypeRef};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::base::kCGImageAlphaPremultipliedLast;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use objc2_core_video::CVPixelBuffer;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

const THUMB_W: i32 = 256;
const THUMB_H: i32 = 192;
const QUALITY: f64 = 0.7;

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTCreateCGImageFromCVPixelBuffer(
        buffer: *mut c_void,
        options: *const c_void,
        image_out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreateMutable(allocator: *const c_void, capacity: CFIndex) -> *mut c_void;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFDataGetLength(data: *const c_void) -> CFIndex;
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageDestinationCreateWithData(
        data: *mut c_void,
        ut_type: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(dest: *mut c_void, image: *mut c_void, properties: *const c_void);
    fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
}

/// Encode the buffer to a base64 JPEG string at THUMB_W x THUMB_H, Q70.
#[allow(dead_code)] // wired in by M7.6
pub fn cvpixelbuffer_to_jpeg_base64(pb: &CVPixelBuffer) -> Result<String> {
    catch_unwind(AssertUnwindSafe(|| {
        let cgimage = create_cgimage_from_pb(pb).context("create CGImage from CVPixelBuffer")?;
        let scaled = scale_cgimage(&cgimage, THUMB_W, THUMB_H).context("scale CGImage")?;
        let jpeg = jpeg_encode(&scaled).context("jpeg encode")?;
        Ok(STANDARD.encode(&jpeg))
    }))
    .map_err(|_| anyhow!("thumbnail encoder panicked"))?
}

/// Wrap VTCreateCGImageFromCVPixelBuffer; returns owned CGImage on success.
fn create_cgimage_from_pb(pb: &CVPixelBuffer) -> Result<CGImage> {
    let pb_ptr = pb as *const CVPixelBuffer as *mut c_void;
    let mut img: *mut c_void = ptr::null_mut();
    // SAFETY: pb_ptr borrows the live CVPixelBuffer; img receives a +1 retained CGImageRef.
    let err = unsafe { VTCreateCGImageFromCVPixelBuffer(pb_ptr, ptr::null(), &mut img) };
    if err != 0 || img.is_null() {
        return Err(anyhow!("VTCreateCGImageFromCVPixelBuffer failed: {err}"));
    }
    // SAFETY: img is a +1 retained CGImageRef; from_ptr takes ownership of that retain.
    Ok(unsafe { CGImage::from_ptr(img.cast()) })
}

/// Draw `src` into a fresh w x h RGBA bitmap context and return the resulting CGImage.
fn scale_cgimage(src: &CGImage, w: i32, h: i32) -> Result<CGImage> {
    let cs = CGColorSpace::create_device_rgb();
    let ctx = CGContext::create_bitmap_context(
        None,
        w as usize,
        h as usize,
        8,
        0,
        &cs,
        kCGImageAlphaPremultipliedLast,
    );
    let dst = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
    ctx.draw_image(dst, src);
    ctx.create_image()
        .ok_or_else(|| anyhow!("CGContext::create_image returned None"))
}

/// JPEG-encode `src` via CGImageDestination + CFMutableData; return raw bytes.
fn jpeg_encode(src: &CGImage) -> Result<Vec<u8>> {
    let ut_jpeg = CFString::from_static_string("public.jpeg");
    let key = CFString::from_static_string("kCGImageDestinationLossyCompressionQuality");
    let q = CFNumber::from(QUALITY);
    let props = CFDictionary::from_CFType_pairs(&[(key, q)]);

    // SAFETY: data_mut is a +1 CFMutableDataRef we own; we CFRelease it before returning.
    let data_mut: *mut c_void = unsafe { CFDataCreateMutable(ptr::null(), 0) };
    if data_mut.is_null() {
        return Err(anyhow!("CFDataCreateMutable failed"));
    }

    // SAFETY: data_mut/ut_jpeg/props are valid CF objects; dest is +1 retained on success.
    let dest = unsafe {
        CGImageDestinationCreateWithData(
            data_mut,
            ut_jpeg.as_concrete_TypeRef() as *const c_void,
            1,
            ptr::null(),
        )
    };
    if dest.is_null() {
        // SAFETY: data_mut is non-null and owned; release before erroring.
        unsafe { CFRelease(data_mut as CFTypeRef) };
        return Err(anyhow!("CGImageDestinationCreateWithData failed"));
    }

    // SAFETY: src.as_ptr() is a live CGImage; props is a valid CFDictionary; dest finalized once.
    let ok = unsafe {
        CGImageDestinationAddImage(
            dest,
            src.as_ptr() as *mut c_void,
            props.as_concrete_TypeRef() as *const c_void,
        );
        CGImageDestinationFinalize(dest)
    };

    // SAFETY: read bytes from CFMutableData (which is-a CFData). Slice borrowed only here.
    let bytes = if ok {
        unsafe {
            let len = CFDataGetLength(data_mut as *const c_void) as usize;
            let ptr = CFDataGetBytePtr(data_mut as *const c_void);
            if ptr.is_null() || len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(ptr, len).to_vec()
            }
        }
    } else {
        Vec::new()
    };

    // SAFETY: dest and data_mut are +1 retained CF objects we own; release once each.
    unsafe {
        CFRelease(dest as CFTypeRef);
        CFRelease(data_mut as CFTypeRef);
    }

    if !ok {
        return Err(anyhow!("CGImageDestinationFinalize returned false"));
    }
    if bytes.is_empty() {
        return Err(anyhow!("CFData empty after finalize"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    // No unit test: full pipeline requires a real CVPixelBuffer.
}
