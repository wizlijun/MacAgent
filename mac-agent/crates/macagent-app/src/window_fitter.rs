//! AX-based window resize and restore.

use macagent_core::ctrl_msg::{Viewport, WindowRect};

const MIN_W: i32 = 400;
const MIN_H: i32 = 300;
const MAX_W: i32 = 1920;
const MAX_H: i32 = 1200;

/// Pure: compute target window size that aspect-matches the viewport.
pub fn compute_target_size(original: &WindowRect, viewport: Viewport) -> (i32, i32) {
    let vp_w = viewport.w.max(1) as f64;
    let vp_h = viewport.h.max(1) as f64;
    let mut w = original.w;
    let mut h = ((w as f64) * vp_h / vp_w).round() as i32;
    // Clamp width first
    if w > MAX_W {
        w = MAX_W;
        h = ((w as f64) * vp_h / vp_w).round() as i32;
    }
    if w < MIN_W {
        w = MIN_W;
        h = ((w as f64) * vp_h / vp_w).round() as i32;
    }
    // Then clamp height
    h = h.clamp(MIN_H, MAX_H);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_fit_keeps_width_scales_height() {
        // 1440 wide window, viewport 393x760 (iPhone portrait)
        // target_h = 1440 * (760/393) ≈ 2785 → clamped to 1200
        let original = WindowRect { x: 0, y: 0, w: 1440, h: 900 };
        let viewport = Viewport { w: 393, h: 760 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1440);
        assert_eq!(h, 1200); // clamped
    }

    #[test]
    fn aspect_fit_landscape_viewport() {
        // viewport landscape 800x500
        let original = WindowRect { x: 0, y: 0, w: 1000, h: 800 };
        let viewport = Viewport { w: 800, h: 500 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1000);
        assert_eq!(h, 625); // 1000 * (500/800)
    }

    #[test]
    fn clamp_min_size() {
        // Tiny window
        let original = WindowRect { x: 0, y: 0, w: 200, h: 150 };
        let viewport = Viewport { w: 100, h: 100 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 400); // clamped up
        assert_eq!(h, 400); // 400 * 1.0 = 400
    }

    #[test]
    fn clamp_max_size() {
        let original = WindowRect { x: 0, y: 0, w: 3840, h: 2160 };
        let viewport = Viewport { w: 1024, h: 768 };
        let (w, h) = compute_target_size(&original, viewport);
        assert_eq!(w, 1920); // clamped down to MAX_W
        assert_eq!(h, 1200); // 1920 * 0.75 = 1440 → clamped down to MAX_H
    }
}
