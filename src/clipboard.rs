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

/// Copies text with whichever Linux clipboard helper is installed.
///
/// Returns `None` when no helper is available, so the caller can fall back.
#[cfg(target_os = "linux")]
fn linux_copy_text(text: &str) -> Option<Result<(), String>> {
    for (program, args) in text_helpers() {
        if let Some(result) = pipe_to(program, args, text.as_bytes()) {
            return Some(result);
        }
    }
    None
}

/// Copies a PNG with whichever Linux clipboard helper is installed.
#[cfg(target_os = "linux")]
fn linux_copy_image(image: &Rgba8) -> Option<Result<(), String>> {
    let png = crate::png::encode(image);
    for (program, args) in image_helpers() {
        if let Some(result) = pipe_to(program, args, &png) {
            return Some(result);
        }
    }
    None
}

/// Clipboard helpers for text, most session-appropriate first.
#[cfg(target_os = "linux")]
fn text_helpers() -> Vec<(&'static str, Vec<&'static str>)> {
    if is_wayland() {
        vec![
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ]
    } else {
        vec![
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
            ("wl-copy", vec![]),
        ]
    }
}

/// Clipboard helpers for `image/png`.
#[cfg(target_os = "linux")]
fn image_helpers() -> Vec<(&'static str, Vec<&'static str>)> {
    if is_wayland() {
        vec![
            ("wl-copy", vec!["--type", "image/png"]),
            ("xclip", vec!["-selection", "clipboard", "-t", "image/png"]),
        ]
    } else {
        vec![
            ("xclip", vec!["-selection", "clipboard", "-t", "image/png"]),
            ("wl-copy", vec!["--type", "image/png"]),
        ]
    }
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Feeds `payload` to a helper on stdin.
///
/// Returns `None` when the program is not installed, and `Some(Err(..))` when
/// it exists but failed, so a missing helper falls through to the next one
/// while a real failure is reported.
#[cfg(target_os = "linux")]
fn pipe_to(
    program: &str,
    args: Vec<&str>,
    payload: &[u8],
) -> Option<Result<(), String>> {
    let child = Command::new(program)
        .args(&args)
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
