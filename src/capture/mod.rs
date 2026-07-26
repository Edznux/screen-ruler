//! Per-monitor screen capture.
//!
//! Each monitor is captured separately rather than as one stitched
//! virtual-desktop bitmap. That is what allows every display to keep its own
//! resolution and scale factor instead of being resampled into a single
//! lowest-common-denominator image.

use std::thread;

use crate::geometry::MonitorGeometry;
use crate::image::Rgba8;

#[cfg(target_os = "linux")]
mod wlr;

/// One monitor's pixels plus the geometry the capture backend reported for it.
pub struct CapturedMonitor {
    pub geometry: MonitorGeometry,
    pub image: Rgba8,
}

/// Why capture failed, in terms a user can act on.
#[derive(Debug)]
pub enum CaptureError {
    /// The platform reported no usable displays.
    NoMonitors,
    /// Every monitor failed to capture; carries the first underlying reason.
    AllMonitorsFailed(String),
    /// The capture backend itself could not be reached.
    Backend(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoMonitors => write!(f, "no displays were reported by the system"),
            CaptureError::AllMonitorsFailed(reason) => {
                write!(f, "no display could be captured: {reason}")
            }
            CaptureError::Backend(reason) => write!(f, "screen capture is unavailable: {reason}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// True when a Wayland session is in use.
///
/// Decides which capture backend to try first and which clipboard helper to
/// prefer, so it is defined once here rather than re-derived at each call site.
pub fn is_wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// A platform-specific hint shown when capture fails, so the user knows what to
/// grant or install rather than just seeing an error.
pub fn permission_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Grant Screen Recording permission in System Settings > Privacy & Security, \
         then relaunch."
    } else if cfg!(target_os = "windows") {
        "Check that the app is allowed to capture the screen and is not blocked by a \
         protected-content policy."
    } else if is_wayland_session() {
        "On Wayland, allow the screen-capture request from the desktop portal when prompted. \
         A portal implementation (xdg-desktop-portal plus a backend such as \
         xdg-desktop-portal-wlr, -gnome or -kde) must be running."
    } else {
        "Check that the X server allows screen capture for this session."
    }
}

/// Captures every connected monitor, trying each available backend in turn.
///
/// On Wayland the native `wlr-screencopy` path is preferred because the generic
/// backend's shared-memory allocation is rejected by wlroots compositors. Every
/// backend failure is collected so that, if none succeed, the reported error
/// explains what was actually tried.
pub fn capture_all() -> Result<Vec<CapturedMonitor>, CaptureError> {
    let mut attempts: Vec<String> = Vec::new();

    #[cfg(target_os = "linux")]
    if is_wayland_session() {
        match wlr::capture_all() {
            Ok(monitors) if !monitors.is_empty() => return Ok(finish(monitors)),
            Ok(_) => attempts.push("wlr-screencopy: no monitors returned".to_string()),
            Err(reason) => attempts.push(format!("wlr-screencopy: {reason}")),
        }
    }

    match capture_all_xcap() {
        Ok(monitors) => return Ok(monitors),
        Err(error) => attempts.push(error.to_string()),
    }

    Err(CaptureError::AllMonitorsFailed(attempts.join("; ")))
}

/// Puts monitors in a deterministic left-to-right, top-to-bottom order.
///
/// Stable indices matter for reproducing multi-screen bugs and for keeping
/// annotations attached to the same display across a re-analysis.
fn finish(mut monitors: Vec<CapturedMonitor>) -> Vec<CapturedMonitor> {
    monitors.sort_by_key(|m| (m.geometry.position.1, m.geometry.position.0));
    monitors
}

/// Captures every monitor through the cross-platform backend.
///
/// Monitors are captured concurrently: each display costs a round-trip to the
/// compositor or window server, so doing them in sequence visibly stalls
/// start-up on a multi-head setup. A monitor that fails is skipped rather than
/// aborting the run, so one flaky output does not take down the whole tool.
fn capture_all_xcap() -> Result<Vec<CapturedMonitor>, CaptureError> {
    let monitors = xcap::Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
    if monitors.is_empty() {
        return Err(CaptureError::NoMonitors);
    }

    let results: Vec<Result<CapturedMonitor, String>> = thread::scope(|scope| {
        let handles: Vec<_> = monitors
            .iter()
            .map(|monitor| scope.spawn(move || capture_one(monitor)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("capture thread panicked".to_string()))
            })
            .collect()
    });

    let mut captured = Vec::new();
    let mut first_error = None;
    for result in results {
        match result {
            Ok(monitor) => captured.push(monitor),
            Err(reason) => {
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }

    if captured.is_empty() {
        return Err(CaptureError::AllMonitorsFailed(
            first_error.unwrap_or_else(|| "unknown reason".to_string()),
        ));
    }

    Ok(finish(captured))
}

/// Captures a single monitor and packages it with its reported geometry.
fn capture_one(monitor: &xcap::Monitor) -> Result<CapturedMonitor, String> {
    let name = monitor
        .name()
        .or_else(|_| monitor.friendly_name())
        .unwrap_or_else(|_| "unknown".to_string());

    let describe = |what: &str, e: xcap::XCapError| format!("{name}: {what}: {e}");

    let position = (
        monitor.x().map_err(|e| describe("x", e))?,
        monitor.y().map_err(|e| describe("y", e))?,
    );
    let scale = monitor.scale_factor().unwrap_or(1.0);
    let is_primary = monitor.is_primary().unwrap_or(false);

    let image = monitor
        .capture_image()
        .map_err(|e| describe("capture", e))?;
    let (width, height) = (image.width() as usize, image.height() as usize);
    let image = Rgba8::from_raw(width, height, image.into_raw())
        .ok_or_else(|| format!("{name}: capture returned a malformed buffer"))?;

    Ok(CapturedMonitor {
        geometry: MonitorGeometry {
            name,
            position,
            // The captured bitmap is the authority on pixel extent: the
            // reported width/height can disagree with it under scaling, and
            // the edge map has to line up with the pixels we actually have.
            size: (width as u32, height as u32),
            scale,
            is_primary,
        },
        image,
    })
}
