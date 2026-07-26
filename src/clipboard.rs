//! Clipboard access.
//!
//! On Windows and macOS the system takes ownership of the copied data, so a
//! plain `arboard` call survives the process exiting. X11 and Wayland do not
//! work that way: the *source application* owns the selection and the clipboard
//! empties the moment it dies. Because this tool's whole point is "measure,
//! copy, quit", it prefers an external helper (`wl-copy`, `xclip`, `xsel`) on
//! Linux, since those fork a daemon that keeps the selection alive.

#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

use crate::image::Rgba8;

/// Copies text to the clipboard.
pub fn copy_text(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(result) = linux_copy_text(text) {
        return result;
    }

    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())
}

/// Copies an image to the clipboard.
pub fn copy_image(image: &Rgba8) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if let Some(result) = linux_copy_image(image) {
        return result;
    }

    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: image.width(),
            height: image.height(),
            bytes: std::borrow::Cow::Borrowed(image.as_bytes()),
        })
        .map_err(|e| e.to_string())
}

/// One clipboard helper: the program to run and the arguments it needs.
#[cfg(target_os = "linux")]
type Helper = (&'static str, &'static [&'static str]);

/// Text helpers, X11 first. `wl-copy` is listed last so a Wayland session can
/// promote it with a single rotation.
#[cfg(target_os = "linux")]
const TEXT_HELPERS: [Helper; 3] = [
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("wl-copy", &[]),
];

/// Helpers for `image/png`, in the same order.
#[cfg(target_os = "linux")]
const IMAGE_HELPERS: [Helper; 2] = [
    ("xclip", &["-selection", "clipboard", "-t", "image/png"]),
    ("wl-copy", &["--type", "image/png"]),
];

/// Copies text with whichever Linux clipboard helper is installed.
///
/// Returns `None` when no helper is available, so the caller can fall back.
#[cfg(target_os = "linux")]
fn linux_copy_text(text: &str) -> Option<Result<(), String>> {
    try_helpers(&TEXT_HELPERS, text.as_bytes())
}

/// Copies a PNG with whichever Linux clipboard helper is installed.
#[cfg(target_os = "linux")]
fn linux_copy_image(image: &Rgba8) -> Option<Result<(), String>> {
    try_helpers(&IMAGE_HELPERS, &crate::png::encode(image))
}

/// Feeds `payload` to the first helper that is installed, preferring the one
/// that matches the session: on Wayland `wl-copy` moves to the front.
#[cfg(target_os = "linux")]
fn try_helpers(helpers: &[Helper], payload: &[u8]) -> Option<Result<(), String>> {
    let mut helpers = helpers.to_vec();
    if crate::capture::is_wayland_session() {
        helpers.rotate_right(1);
    }
    helpers
        .into_iter()
        .find_map(|(program, args)| pipe_to(program, args, payload))
}

/// Feeds `payload` to a helper on stdin.
///
/// Returns `None` when the program is not installed, and `Some(Err(..))` when
/// it exists but failed, so a missing helper falls through to the next one
/// while a real failure is reported.
#[cfg(target_os = "linux")]
fn pipe_to(program: &str, args: &[&str], payload: &[u8]) -> Option<Result<(), String>> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    let mut child = match child {
        Ok(child) => child,
        // Not installed: let the caller try the next helper.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(Err(format!("{program}: {e}"))),
    };

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| format!("{program}: no stdin"))
        .and_then(|mut stdin| {
            stdin
                .write_all(payload)
                .map_err(|e| format!("{program}: {e}"))
        });

    if let Err(e) = write_result {
        let _ = child.kill();
        return Some(Err(e));
    }

    // These helpers deliberately outlive us to serve the selection; do not wait
    // for them to exit or the copy would block until the clipboard is replaced.
    Some(Ok(()))
}
