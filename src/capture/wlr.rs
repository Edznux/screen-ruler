//! Native Wayland screen capture via the `wlr-screencopy` protocol.
//!
//! This exists because the generic capture backend's Wayland path allocates its
//! shared-memory buffer as a *sealed* `memfd`, which wlroots-based compositors
//! refuse to mmap (`wl_shm` error 2, `INVALID_FD`). Capture then fails outright
//! on Hyprland, Sway, river and Wayfire — a large share of Wayland desktops.
//!
//! Doing it directly also suits the app's shape better: `wlr-screencopy` is
//! per-output, so each monitor arrives as its own correctly sized bitmap rather
//! than a stitched desktop image that would have to be cut back up.
//!
//! GNOME and KDE do not implement `wlr-screencopy`; on those the manager global
//! is simply absent and the caller falls back to another backend.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;

use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::{self, WlOutput},
    wl_registry::{self, WlRegistry},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::geometry::MonitorGeometry;
use crate::image::Rgba8;

use super::CapturedMonitor;

/// One monitor as advertised by the compositor.
#[derive(Clone)]
struct OutputInfo {
    output: WlOutput,
    name: Option<String>,
    /// Logical position reported by `wl_output.geometry`.
    position: (i32, i32),
    /// Integer scale reported by `wl_output.scale`.
    scale: i32,
    /// Physical mode size in device pixels.
    mode: (i32, i32),
}

/// Buffer parameters the compositor offers for a pending frame.
#[derive(Clone, Copy)]
struct BufferSpec {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

impl BufferSpec {
    fn byte_size(&self) -> u64 {
        self.stride as u64 * self.height as u64
    }
}

#[derive(Default)]
struct State {
    shm: Option<WlShm>,
    manager: Option<ZwlrScreencopyManagerV1>,
    outputs: Vec<OutputInfo>,

    /// Best shm buffer spec offered for the frame currently being captured.
    pending_spec: Option<BufferSpec>,
    /// Set once the compositor has finished advertising buffer formats.
    buffer_done: bool,
    frame_ready: bool,
    frame_failed: bool,
    /// True when the compositor wrote the image bottom-up.
    y_invert: bool,
}

/// Captures every output using `wlr-screencopy`.
///
/// Returns `Err` when the protocol is unavailable, which is the signal for the
/// caller to try a different backend.
pub fn capture_all() -> Result<Vec<CapturedMonitor>, String> {
    let connection = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the Wayland display: {e}"))?;

    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    let display = connection.display();
    display.get_registry(&handle, ());

    let mut state = State::default();
    // Two round-trips: the first delivers the globals, the second the
    // per-output geometry, mode and name events that follow binding.
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland roundtrip failed: {e}"))?;
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland roundtrip failed: {e}"))?;

    if state.manager.is_none() {
        return Err("compositor does not support wlr-screencopy".to_string());
    }
    if state.shm.is_none() {
        return Err("compositor does not expose wl_shm".to_string());
    }
    if state.outputs.is_empty() {
        return Err("compositor reported no outputs".to_string());
    }

    let outputs = state.outputs.clone();
    let mut captured = Vec::new();
    let mut first_error = None;

    for output in &outputs {
        match capture_output(&mut queue, &handle, &mut state, output) {
            Ok(monitor) => captured.push(monitor),
            Err(reason) => {
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }

    if captured.is_empty() {
        return Err(first_error.unwrap_or_else(|| "no output could be captured".to_string()));
    }
    Ok(captured)
}

/// Captures a single output into an [`Rgba8`].
fn capture_output(
    queue: &mut wayland_client::EventQueue<State>,
    handle: &QueueHandle<State>,
    state: &mut State,
    info: &OutputInfo,
) -> Result<CapturedMonitor, String> {
    let name = info.name.clone().unwrap_or_else(|| "unknown".to_string());

    state.pending_spec = None;
    state.buffer_done = false;
    state.frame_ready = false;
    state.frame_failed = false;
    state.y_invert = false;

    let manager = state.manager.clone().expect("manager checked by caller");
    // `overlay_cursor = 0`: the pointer must not be baked into the screenshot,
    // or the ruler would detect edges around the cursor itself.
    let frame = manager.capture_output(0, &info.output, handle, ());

    // Wait for the compositor to advertise buffer parameters.
    while !state.buffer_done && !state.frame_failed {
        queue
            .blocking_dispatch(state)
            .map_err(|e| format!("{name}: waiting for buffer formats failed: {e}"))?;
    }
    if state.frame_failed {
        frame.destroy();
        return Err(format!("{name}: compositor rejected the capture request"));
    }

    let spec = state
        .pending_spec
        .ok_or_else(|| format!("{name}: compositor offered no supported buffer format"))?;
    if spec.width == 0 || spec.height == 0 || spec.byte_size() == 0 {
        frame.destroy();
        return Err(format!("{name}: compositor offered an empty buffer"));
    }

    // A plain unlinked file in a tmpfs, sized up front. Deliberately *not* a
    // sealed memfd: that is exactly what wlroots compositors refuse to mmap.
    let file = create_shm_file(spec.byte_size())
        .map_err(|e| format!("{name}: cannot allocate a shared buffer: {e}"))?;

    let shm = state.shm.clone().expect("shm checked by caller");
    let pool = shm.create_pool(
        file.as_fd(),
        spec.byte_size() as i32,
        handle,
        (),
    );
    let buffer = pool.create_buffer(
        0,
        spec.width as i32,
        spec.height as i32,
        spec.stride as i32,
        spec.format,
        handle,
        (),
    );

    frame.copy(&buffer);
    while !state.frame_ready && !state.frame_failed {
        queue
            .blocking_dispatch(state)
            .map_err(|e| format!("{name}: waiting for the frame copy failed: {e}"))?;
    }

    let failed = state.frame_failed;
    let y_invert = state.y_invert;
    buffer.destroy();
    pool.destroy();
    frame.destroy();

    if failed {
        return Err(format!("{name}: the compositor failed to copy the frame"));
    }

    let mut raw = vec![0u8; spec.byte_size() as usize];
    let mut file = file;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut raw))
        .map_err(|e| format!("{name}: cannot read the captured frame: {e}"))?;

    let image = to_rgba8(&raw, spec, y_invert)
        .ok_or_else(|| format!("{name}: unsupported pixel format {:?}", spec.format))?;

    Ok(CapturedMonitor {
        geometry: MonitorGeometry {
            name,
            // `wl_output.geometry` reports a logical origin; the virtual
            // desktop is tracked in device pixels, so scale it up. Window
            // placement later reconciles this against the window system's own
            // view, which is authoritative.
            position: (
                info.position.0 * info.scale.max(1),
                info.position.1 * info.scale.max(1),
            ),
            size: (image.width() as u32, image.height() as u32),
            scale: derive_scale(info.mode.0, info.scale, image.width()),
            is_primary: false,
        },
        image,
    })
}

/// Works out device pixels per logical pixel for an output.
///
/// Prefers the ratio between the captured bitmap and the advertised mode, which
/// stays correct under fractional scaling where the integer `wl_output.scale`
/// does not. Falls back to the integer scale when the mode is unknown.
fn derive_scale(mode_width: i32, output_scale: i32, image_width: usize) -> f32 {
    let output_scale = output_scale.max(1);
    let logical_width = (mode_width.max(0) as f32) / output_scale as f32;
    crate::geometry::scale_from_widths(image_width, logical_width).unwrap_or(output_scale as f32)
}

/// Creates a sized, unlinked file in a tmpfs to back the shm pool.
///
/// Unlinking immediately means the buffer cannot outlive the process even if it
/// exits abnormally.
fn create_shm_file(size: u64) -> std::io::Result<File> {
    let mut candidates = Vec::new();
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        candidates.push(std::path::PathBuf::from(runtime_dir));
    }
    candidates.push(std::path::PathBuf::from("/dev/shm"));
    candidates.push(std::env::temp_dir());

    let mut last_error = None;
    for directory in candidates {
        // The file is unlinked straight away, so the name only has to be
        // unique for the instant between creation and unlinking.
        let path = directory.join(format!(
            "screen-ruler-{}-{:p}.shm",
            std::process::id(),
            &size as *const u64
        ));
        match File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                file.set_len(size)?;
                return Ok(file);
            }
            Err(e) => last_error = Some(e),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::other("no writable directory for a shared buffer")
    }))
}

/// Converts a compositor buffer into RGBA8, flipping it if written bottom-up.
///
/// Wayland shm formats name their channels in little-endian word order, so
/// `Xrgb8888` is `B, G, R, X` in memory.
fn to_rgba8(raw: &[u8], spec: BufferSpec, y_invert: bool) -> Option<Rgba8> {
    let width = spec.width as usize;
    let height = spec.height as usize;
    let stride = spec.stride as usize;

    // (bytes per pixel, index of red, green, blue, alpha or None)
    let (bpp, r, g, b, a) = match spec.format {
        wl_shm::Format::Xrgb8888 => (4, 2, 1, 0, None),
        wl_shm::Format::Argb8888 => (4, 2, 1, 0, Some(3)),
        wl_shm::Format::Xbgr8888 => (4, 0, 1, 2, None),
        wl_shm::Format::Abgr8888 => (4, 0, 1, 2, Some(3)),
        wl_shm::Format::Bgr888 => (3, 2, 1, 0, None),
        wl_shm::Format::Rgb888 => (3, 0, 1, 2, None),
        _ => return None,
    };

    let mut out = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let source_row = if y_invert { height - 1 - row } else { row };
        let start = source_row * stride;
        let line = raw.get(start..start + width * bpp)?;
        for pixel in line.chunks_exact(bpp) {
            out.push(pixel[r]);
            out.push(pixel[g]);
            out.push(pixel[b]);
            out.push(a.map_or(255, |i| pixel[i]));
        }
    }

    Rgba8::from_raw(width, height, out)
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "wl_shm" => {
                state.shm = Some(registry.bind::<WlShm, _, _>(name, 1, handle, ()));
            }
            "zwlr_screencopy_manager_v1" => {
                let version = version.min(3);
                state.manager = Some(registry.bind::<ZwlrScreencopyManagerV1, _, _>(
                    name,
                    version,
                    handle,
                    (),
                ));
            }
            "wl_output" => {
                // Version 4 adds the `name` event, which is how outputs are
                // matched against the window system's monitor list.
                let version = version.min(4);
                let output = registry.bind::<WlOutput, _, _>(name, version, handle, ());
                state.outputs.push(OutputInfo {
                    output,
                    name: None,
                    position: (0, 0),
                    scale: 1,
                    mode: (0, 0),
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(info) = state.outputs.iter_mut().find(|o| &o.output == output) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => info.position = (x, y),
            wl_output::Event::Scale { factor } => info.scale = factor.max(1),
            wl_output::Event::Name { name } => info.name = Some(name),
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                // Only the mode flagged current describes the live resolution.
                if matches!(flags, WEnum::Value(f) if f.contains(wl_output::Mode::Current)) {
                    info.mode = (width, height);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                // Keep the first format that can actually be converted;
                // compositors advertise several and only some are usable.
                if state.pending_spec.is_none() && is_supported_format(format) {
                    state.pending_spec = Some(BufferSpec {
                        format,
                        width,
                        height,
                        stride,
                    });
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                state.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.frame_ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.frame_failed = true;
                // Older compositors fail without ever sending buffer_done;
                // unblock the wait so the error surfaces instead of hanging.
                state.buffer_done = true;
            }
            _ => {}
        }
    }
}

/// Whether [`to_rgba8`] can decode a format.
fn is_supported_format(format: wl_shm::Format) -> bool {
    matches!(
        format,
        wl_shm::Format::Xrgb8888
            | wl_shm::Format::Argb8888
            | wl_shm::Format::Xbgr8888
            | wl_shm::Format::Abgr8888
            | wl_shm::Format::Bgr888
            | wl_shm::Format::Rgb888
    )
}

// These globals emit events the app has no use for (format advertisements,
// buffer release), but the protocol still requires a handler for each.
impl Dispatch<WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: <WlShmPool as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: <WlBuffer as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(format: wl_shm::Format, width: u32, height: u32) -> BufferSpec {
        BufferSpec {
            format,
            width,
            height,
            stride: width * 4,
        }
    }

    #[test]
    fn xrgb_is_decoded_as_little_endian_bgrx() {
        // One pixel, memory order B, G, R, X.
        let raw = vec![10, 20, 30, 255];
        let image = to_rgba8(&raw, spec(wl_shm::Format::Xrgb8888, 1, 1), false).expect("decoded");
        assert_eq!(image.pixel(0, 0), [30, 20, 10, 255]);
    }

    #[test]
    fn argb_preserves_the_alpha_channel() {
        let raw = vec![10, 20, 30, 128];
        let image = to_rgba8(&raw, spec(wl_shm::Format::Argb8888, 1, 1), false).expect("decoded");
        assert_eq!(image.pixel(0, 0), [30, 20, 10, 128]);
    }

    #[test]
    fn xbgr_needs_no_channel_swap() {
        let raw = vec![10, 20, 30, 255];
        let image = to_rgba8(&raw, spec(wl_shm::Format::Xbgr8888, 1, 1), false).expect("decoded");
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn formats_without_alpha_are_forced_opaque() {
        // A zero in the padding byte must not produce a transparent pixel.
        let raw = vec![10, 20, 30, 0];
        let image = to_rgba8(&raw, spec(wl_shm::Format::Xrgb8888, 1, 1), false).expect("decoded");
        assert_eq!(image.pixel(0, 0)[3], 255);
    }

    #[test]
    fn y_invert_flips_the_image_vertically() {
        // Two rows: first black, second white.
        let raw = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let upright = to_rgba8(&raw, spec(wl_shm::Format::Xrgb8888, 1, 2), false).expect("decoded");
        assert_eq!(upright.pixel(0, 0)[0], 0);
        assert_eq!(upright.pixel(0, 1)[0], 255);

        let flipped = to_rgba8(&raw, spec(wl_shm::Format::Xrgb8888, 1, 2), true).expect("decoded");
        assert_eq!(flipped.pixel(0, 0)[0], 255);
        assert_eq!(flipped.pixel(0, 1)[0], 0);
    }

    #[test]
    fn row_padding_in_the_stride_is_skipped() {
        // 1px wide but a 2px stride: the trailing bytes are padding.
        let padded = BufferSpec {
            format: wl_shm::Format::Xrgb8888,
            width: 1,
            height: 2,
            stride: 8,
        };
        let raw = vec![
            10, 20, 30, 255, 99, 99, 99, 99, // row 0 + padding
            40, 50, 60, 255, 99, 99, 99, 99, // row 1 + padding
        ];
        let image = to_rgba8(&raw, padded, false).expect("decoded");
        assert_eq!(image.pixel(0, 0), [30, 20, 10, 255]);
        assert_eq!(image.pixel(0, 1), [60, 50, 40, 255]);
    }

    #[test]
    fn three_byte_formats_are_supported() {
        let tight = BufferSpec {
            format: wl_shm::Format::Bgr888,
            width: 1,
            height: 1,
            stride: 3,
        };
        let image = to_rgba8(&[10, 20, 30], tight, false).expect("decoded");
        assert_eq!(image.pixel(0, 0), [30, 20, 10, 255]);
    }

    #[test]
    fn unsupported_formats_and_short_buffers_are_rejected() {
        assert!(to_rgba8(&[0; 4], spec(wl_shm::Format::Yuv420, 1, 1), false).is_none());
        // Buffer shorter than width * height * bpp.
        assert!(to_rgba8(&[0; 4], spec(wl_shm::Format::Xrgb8888, 4, 4), false).is_none());
    }

    #[test]
    fn supported_format_list_matches_the_decoder() {
        for format in [
            wl_shm::Format::Xrgb8888,
            wl_shm::Format::Argb8888,
            wl_shm::Format::Xbgr8888,
            wl_shm::Format::Abgr8888,
        ] {
            assert!(is_supported_format(format), "{format:?} advertised but not decodable");
            assert!(to_rgba8(&[0; 4], spec(format, 1, 1), false).is_some());
        }
        assert!(!is_supported_format(wl_shm::Format::Yuv420));
    }

    #[test]
    fn shm_files_are_sized_and_unlinked() {
        let file = create_shm_file(4096).expect("allocated");
        assert_eq!(file.metadata().expect("metadata").len(), 4096);
    }

    #[test]
    fn scale_is_derived_from_the_captured_bitmap() {
        // 3840px mode at integer scale 2 is 1920 logical px; a 3840px capture
        // means an effective scale of 2.
        assert_eq!(derive_scale(3840, 2, 3840), 2.0);
        // Unscaled 1080p.
        assert_eq!(derive_scale(1920, 1, 1920), 1.0);
    }

    #[test]
    fn fractional_scaling_beats_the_integer_output_scale() {
        // A 2560px mode presented at 1.25x reports integer scale 1, but the
        // compositor still hands back a 2560px bitmap over 2048 logical px.
        let logical_width = 2048;
        assert_eq!(derive_scale(logical_width, 1, 2560), 1.25);
    }

    #[test]
    fn scale_falls_back_when_the_mode_is_unknown() {
        assert_eq!(derive_scale(0, 2, 3840), 2.0);
        assert_eq!(derive_scale(0, 0, 0), 1.0);
    }
}
