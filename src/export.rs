//! Composite image export.
//!
//! Rather than re-rendering annotations through a second code path, the export
//! reads back the window's own framebuffer for one frame drawn without any UI
//! chrome. What lands on the clipboard is therefore exactly what was on screen,
//! and there is no second renderer to keep in sync.

use crate::geometry::MonitorGeometry;
use crate::image::Rgba8;
use crate::state::Rect;

/// Converts a logical export rectangle into device pixels within the window.
///
/// Returns `None` when the rectangle does not overlap the window at all.
pub fn device_crop(
    rect: Rect,
    geometry: &MonitorGeometry,
    window: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let scale = geometry.sanitised_scale();
    let left = (rect.x * scale).round().max(0.0) as u32;
    let top = (rect.y * scale).round().max(0.0) as u32;
    let width = (rect.width * scale).round().max(0.0) as u32;
    let height = (rect.height * scale).round().max(0.0) as u32;

    if width == 0 || height == 0 || left >= window.0 || top >= window.1 {
        return None;
    }

    // Clamp to the window: a drag can end slightly outside it.
    let width = width.min(window.0 - left);
    let height = height.min(window.1 - top);
    if width == 0 || height == 0 {
        return None;
    }
    Some((left, top, width, height))
}

/// Cuts a region out of a full-window RGBA readback.
///
/// `pixels` must be top-down, which is what `read_screen_rgba` produces.
pub fn crop_readback(
    pixels: &[u8],
    window: (u32, u32),
    crop: (u32, u32, u32, u32),
) -> Option<Rgba8> {
    let (win_w, win_h) = (window.0 as usize, window.1 as usize);
    let (left, top, width, height) = (
        crop.0 as usize,
        crop.1 as usize,
        crop.2 as usize,
        crop.3 as usize,
    );

    if pixels.len() < win_w * win_h * 4 || left + width > win_w || top + height > win_h {
        return None;
    }

    let mut out = Vec::with_capacity(width * height * 4);
    for row in top..top + height {
        let start = (row * win_w + left) * 4;
        out.extend_from_slice(&pixels[start..start + width * 4]);
    }
    Rgba8::from_raw(width, height, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(scale: f32) -> MonitorGeometry {
        MonitorGeometry {
            name: "TEST".to_string(),
            position: (0, 0),
            size: (1920, 1080),
            scale,
            is_primary: true,
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            monitor: 0,
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn logical_rects_scale_into_device_pixels() {
        let crop = device_crop(rect(10.0, 20.0, 100.0, 50.0), &geometry(2.0), (1920, 1080));
        assert_eq!(crop, Some((20, 40, 200, 100)));

        let unscaled = device_crop(rect(10.0, 20.0, 100.0, 50.0), &geometry(1.0), (1920, 1080));
        assert_eq!(unscaled, Some((10, 20, 100, 50)));
    }

    #[test]
    fn a_rect_running_past_the_window_is_clamped() {
        let crop = device_crop(rect(1900.0, 1070.0, 200.0, 200.0), &geometry(1.0), (1920, 1080));
        assert_eq!(crop, Some((1900, 1070, 20, 10)));
    }

    #[test]
    fn degenerate_and_offscreen_rects_are_rejected() {
        assert!(device_crop(rect(10.0, 10.0, 0.0, 50.0), &geometry(1.0), (1920, 1080)).is_none());
        assert!(device_crop(rect(5000.0, 10.0, 50.0, 50.0), &geometry(1.0), (1920, 1080)).is_none());
        assert!(device_crop(rect(10.0, 5000.0, 50.0, 50.0), &geometry(1.0), (1920, 1080)).is_none());
    }

    #[test]
    fn cropping_extracts_the_requested_region() {
        // 4x3 window where each pixel encodes its own coordinates.
        let mut pixels = Vec::new();
        for y in 0..3u8 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x, y, 0, 255]);
            }
        }

        let cropped = crop_readback(&pixels, (4, 3), (1, 1, 2, 2)).expect("crop");
        assert_eq!((cropped.width(), cropped.height()), (2, 2));
        assert_eq!(cropped.pixel(0, 0), [1, 1, 0, 255]);
        assert_eq!(cropped.pixel(1, 0), [2, 1, 0, 255]);
        assert_eq!(cropped.pixel(0, 1), [1, 2, 0, 255]);
        assert_eq!(cropped.pixel(1, 1), [2, 2, 0, 255]);
    }

    #[test]
    fn cropping_rejects_a_region_outside_the_readback() {
        let pixels = vec![0u8; 4 * 3 * 4];
        assert!(crop_readback(&pixels, (4, 3), (3, 0, 2, 2)).is_none());
        assert!(crop_readback(&pixels, (4, 3), (0, 2, 2, 2)).is_none());
    }

    #[test]
    fn cropping_rejects_a_truncated_readback() {
        let short = vec![0u8; 8];
        assert!(crop_readback(&short, (4, 3), (0, 0, 2, 2)).is_none());
    }
}
