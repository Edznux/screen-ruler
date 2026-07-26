//! The windowing shell: one borderless fullscreen window per monitor.
//!
//! winit is driven directly rather than through a framework because each window
//! must be pinned to a *specific* output with `Fullscreen::Borderless(Some(..))`.
//! That is the only placement primitive Wayland offers a client — a Wayland
//! client cannot position its own windows — and it is also the most reliable way
//! to cover exactly one monitor on X11, Windows and macOS.
//!
//! All windows share one GL context and one [`RulerState`]; each keeps its own
//! egui context, painter and screenshot texture.

use std::collections::HashMap;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::prelude::*;
// `GlSurface`, which provides swap_buffers/resize, arrives via the prelude.
use glutin::surface::{Surface as GlWindowSurface, SurfaceAttributesBuilder, WindowSurface};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

use crate::clipboard;
use crate::color::KernelCache;
use crate::edges;
use crate::export;
use crate::state::{Command, RulerState};
use crate::surface::Desktop;
use crate::ui;

/// Redraw cadence. Matches the 16 ms poll the previous implementation used.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// One monitor's window and its rendering resources.
struct OverlayWindow {
    window: Arc<Window>,
    gl_surface: GlWindowSurface<WindowSurface>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    painter: egui_glow::Painter,
    monitor: usize,
    /// The monitor's frozen screenshot.
    screenshot: egui::TextureHandle,
    /// The edge map, uploaded lazily because it is only needed for the debug
    /// and preview overlays and costs as much memory as the screenshot.
    edge_texture: Option<egui::TextureHandle>,
    edge_texture_stale: bool,
}

/// The application, owning every window and the shared state.
pub struct App {
    desktop: Desktop,
    state: RulerState,
    colors: KernelCache,
    windows: HashMap<WindowId, OverlayWindow>,
    gl_context: Option<PossiblyCurrentContext>,
    gl_config: Option<Config>,
    /// Set once windows exist, so a resume does not build them twice.
    initialised: bool,
    exiting: bool,
    /// A composite export awaiting the next frame of its monitor.
    pending_export: Option<crate::state::Rect>,
}

impl App {
    pub fn new(desktop: Desktop, state: RulerState) -> Self {
        Self {
            desktop,
            state,
            colors: KernelCache::new(),
            windows: HashMap::new(),
            gl_context: None,
            gl_config: None,
            initialised: false,
            exiting: false,
            pending_export: None,
        }
    }

    /// Runs the event loop until the user quits.
    pub fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    /// Creates one fullscreen window per captured monitor.
    fn create_windows(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let available: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let geometries = self.desktop.geometries();

        // Bootstrap a GL display and config from a throwaway window template.
        let template = ConfigTemplateBuilder::new().with_alpha_size(0);
        let (_, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(base_attributes()))
            .build(event_loop, template, |configs| {
                // Prefer the config with the most samples; any is workable.
                configs
                    .reduce(|best, config| {
                        if config.num_samples() > best.num_samples() {
                            config
                        } else {
                            best
                        }
                    })
                    .expect("the platform offers at least one GL config")
            })
            .map_err(|e| format!("cannot create an OpenGL config: {e}"))?;

        let gl_display = gl_config.display();
        let context_attributes = ContextAttributesBuilder::new()
            // Ask for OpenGL ES too: it is what most Wayland stacks provide.
            .with_context_api(ContextApi::OpenGl(None))
            .build(None);
        let fallback_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(None);

        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| gl_display.create_context(&gl_config, &fallback_attributes))
        }
        .map_err(|e| format!("cannot create an OpenGL context: {e}"))?;

        // One context, shared by every window: each is made current against its
        // own surface just before that window is painted.
        let mut not_current = Some(not_current);
        let mut current_context: Option<PossiblyCurrentContext> = None;
        // Loading GL function pointers queries GL_VERSION, so it can only
        // happen once a context is actually current — that is, inside the loop
        // below, after the first window's surface exists.
        let mut gl: Option<Arc<glow::Context>> = None;

        for (index, geometry) in geometries.iter().enumerate() {
            let handle = match_monitor(&available, geometry, index);
            let attributes = base_attributes()
                // Fullscreen-on-a-named-output is the placement primitive that
                // works everywhere; explicit x/y positioning does not exist on
                // Wayland and is unreliable under X11 window managers.
                .with_fullscreen(Some(Fullscreen::Borderless(handle.clone())));

            let window = glutin_winit::finalize_window(event_loop, attributes, &gl_config)
                .map_err(|e| format!("cannot create a window for {}: {e}", geometry.name))?;
            let window = Arc::new(window);

            let size = window.inner_size();
            let width = NonZeroU32::new(size.width.max(1)).expect("non-zero width");
            let height = NonZeroU32::new(size.height.max(1)).expect("non-zero height");
            let raw_handle = window
                .window_handle()
                .map_err(|e| format!("no window handle for {}: {e}", geometry.name))?
                .as_raw();
            let surface_attributes =
                SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_handle, width, height);
            let gl_surface = unsafe {
                gl_display
                    .create_window_surface(&gl_config, &surface_attributes)
                    .map_err(|e| format!("cannot create a GL surface for {}: {e}", geometry.name))?
            };

            let context = match current_context.take() {
                Some(context) => context,
                None => not_current
                    .take()
                    .expect("the context is created exactly once")
                    .make_current(&gl_surface)
                    .map_err(|e| format!("cannot activate the GL context: {e}"))?,
            };
            context
                .make_current(&gl_surface)
                .map_err(|e| format!("cannot activate the GL context: {e}"))?;

            let gl = gl
                .get_or_insert_with(|| {
                    Arc::new(unsafe {
                        glow::Context::from_loader_function(|symbol| {
                            match CString::new(symbol) {
                                Ok(symbol) => {
                                    gl_display.get_proc_address(symbol.as_c_str()).cast()
                                }
                                Err(_) => std::ptr::null(),
                            }
                        })
                    })
                })
                .clone();

            let painter = egui_glow::Painter::new(gl, "", None, false)
                .map_err(|e| format!("cannot create a GL painter: {e}"))?;

            let max_texture_side = painter.max_texture_side();
            let egui_ctx = egui::Context::default();
            egui_ctx.set_visuals(egui::Visuals::dark());
            let mut egui_state = egui_winit::State::new(
                egui_ctx.clone(),
                egui::ViewportId::ROOT,
                &window,
                Some(window.scale_factor() as f32),
                None,
                Some(max_texture_side),
            );

            // egui only learns the driver's texture-size limit from the first
            // frame's input, and defaults to a conservative 2048 until then —
            // far smaller than a monitor. Run one empty frame so the screenshot
            // is validated against the real limit. Its texture delta is handed
            // to the painter rather than dropped, so the font atlas it creates
            // is actually uploaded.
            let warmup = egui_ctx.run(egui_state.take_egui_input(&window), |_| {});
            let mut painter = painter;
            painter.paint_and_update_textures(
                [size.width.max(1), size.height.max(1)],
                warmup.pixels_per_point,
                &[],
                &warmup.textures_delta,
            );

            let surface = self
                .desktop
                .surface(index)
                .ok_or_else(|| format!("no captured surface for monitor {index}"))?;
            let (backdrop, downscaled) = to_color_image(&surface.image, max_texture_side);
            if let Some(factor) = downscaled {
                eprintln!(
                    "screen-ruler: {} is {}x{} but this driver caps textures at \
                     {max_texture_side}; showing the backdrop at 1/{factor} scale \
                     (measurements are unaffected).",
                    geometry.name,
                    surface.image.width(),
                    surface.image.height()
                );
            }
            let screenshot = egui_ctx.load_texture(
                format!("screenshot-{index}"),
                backdrop,
                egui::TextureOptions::NEAREST,
            );

            // Take focus explicitly. A fullscreen always-on-top overlay that
            // never receives key events would have no way to quit, so this is
            // worth insisting on rather than relying on the window manager.
            if index == 0 {
                window.focus_window();
            }

            self.windows.insert(
                window.id(),
                OverlayWindow {
                    window,
                    gl_surface,
                    egui_ctx,
                    egui_state,
                    painter,
                    monitor: index,
                    screenshot,
                    edge_texture: None,
                    edge_texture_stale: true,
                },
            );

            current_context = Some(context);
        }

        // Reconcile the capture backend's geometry with the window system's,
        // which is authoritative for anything to do with placement and scale.
        self.reconcile_geometry();

        self.gl_context = current_context;
        self.gl_config = Some(gl_config);
        self.initialised = true;
        Ok(())
    }

    /// Adopts each window's real position and scale factor.
    ///
    /// The capture backend and the window system can disagree — notably under
    /// fractional scaling — and drawing has to follow the window system.
    fn reconcile_geometry(&mut self) {
        for overlay in self.windows.values() {
            let Some(surface) = self.desktop.surface(overlay.monitor) else {
                continue;
            };
            let mut geometry = surface.geometry.clone();

            if let Some(monitor) = overlay.window.current_monitor() {
                let position = monitor.position();
                geometry.position = (position.x, position.y);
            }

            let size = overlay.window.inner_size();
            let scale_factor = overlay.window.scale_factor() as f32;
            if scale_factor > 0.0 {
                let logical_width = size.width as f32 / scale_factor;
                if let Some(scale) =
                    crate::geometry::scale_from_widths(surface.image.width(), logical_width)
                {
                    geometry.scale = scale;
                }
            }

            self.desktop.set_geometry(overlay.monitor, geometry);
        }
    }

    /// Renders one window.
    fn redraw(&mut self, id: WindowId) {
        let Some(context) = self.gl_context.as_ref() else {
            return;
        };
        let Some(overlay) = self.windows.get_mut(&id) else {
            return;
        };
        let Some(surface) = self.desktop.surface(overlay.monitor) else {
            return;
        };

        if context.make_current(&overlay.gl_surface).is_err() {
            return;
        }

        if overlay.edge_texture_stale && self.state.debug_edges {
            overlay.edge_texture = Some(overlay.egui_ctx.load_texture(
                format!("edges-{}", overlay.monitor),
                edge_color_image(&surface.edges),
                egui::TextureOptions::NEAREST,
            ));
            overlay.edge_texture_stale = false;
        }

        let raw_input = overlay.egui_state.take_egui_input(&overlay.window);
        let screenshot = overlay.screenshot.id();
        let edge_texture = overlay.edge_texture.as_ref().map(|t| t.id());
        let monitor = overlay.monitor;
        let total_edges = self.desktop.edge_count();

        // An export renders one chrome-free frame that is read straight back.
        let exporting = self
            .pending_export
            .filter(|rect| rect.monitor == monitor);

        let output = overlay.egui_ctx.run(raw_input, |ctx| {
            ui::draw(
                ctx,
                ui::FrameContext {
                    monitor,
                    surface,
                    screenshot,
                    edge_texture,
                    state: &mut self.state,
                    colors: &mut self.colors,
                    total_edges,
                    chrome_hidden: exporting.is_some(),
                },
            );
        });

        overlay
            .egui_state
            .handle_platform_output(&overlay.window, output.platform_output);

        let primitives = overlay
            .egui_ctx
            .tessellate(output.shapes, output.pixels_per_point);
        let size = overlay.window.inner_size();
        let window_size = (size.width.max(1), size.height.max(1));
        overlay.painter.paint_and_update_textures(
            [window_size.0, window_size.1],
            output.pixels_per_point,
            &primitives,
            &output.textures_delta,
        );

        if let Some(rect) = exporting {
            // Read back before swapping: the freshly drawn frame is still the
            // one bound for reading.
            let cropped = export::device_crop(rect, &surface.geometry, window_size).and_then(
                |crop| {
                    let readback = overlay.painter.read_screen_rgba([window_size.0, window_size.1]);
                    export::crop_readback(readback.as_raw(), window_size, crop)
                },
            );
            self.pending_export = None;

            match cropped {
                Some(image) => {
                    if let Err(e) = clipboard::copy_image(&image) {
                        eprintln!("screen-ruler: could not copy the image: {e}");
                        self.state.show_feedback("Could not copy the image");
                    }
                }
                None => self.state.show_feedback("Nothing to export"),
            }
            // The export frame is not meant to be seen; redraw normally.
            overlay.window.request_redraw();
            return;
        }

        let _ = overlay.gl_surface.swap_buffers(context);
    }

    /// Carries out the side effects the UI requested.
    fn process_commands(&mut self, event_loop: &ActiveEventLoop) {
        for command in self.state.take_commands() {
            match command {
                Command::Quit => {
                    self.exiting = true;
                    event_loop.exit();
                }
                Command::CopyText(text) => {
                    if let Err(e) = clipboard::copy_text(&text) {
                        eprintln!("screen-ruler: could not copy to the clipboard: {e}");
                    }
                }
                Command::CopyTextAndQuit(text) => {
                    if let Err(e) = clipboard::copy_text(&text) {
                        eprintln!("screen-ruler: could not copy to the clipboard: {e}");
                    }
                    println!("{text}");
                    self.exiting = true;
                    event_loop.exit();
                }
                Command::CopyRegionImage(rect) => self.pending_export = Some(rect),
                Command::Recompute => self.recompute(),
            }
        }
    }

    /// Re-analyses every monitor after a sensitivity change.
    ///
    /// The edge preview is armed by `set_sensitivity`, which is the only thing
    /// that can queue this command.
    fn recompute(&mut self) {
        let (low, high) = edges::sensitivity_to_thresholds(self.state.sensitivity);
        self.desktop.recompute(low, high);
        for overlay in self.windows.values_mut() {
            overlay.edge_texture_stale = true;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialised {
            return;
        }
        if let Err(e) = self.create_windows(event_loop) {
            eprintln!("screen-ruler: {e}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.exiting {
            return;
        }

        if let Some(overlay) = self.windows.get_mut(&id) {
            let response = overlay.egui_state.on_window_event(&overlay.window, &event);
            if response.repaint {
                overlay.window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.exiting = true;
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(context), Some(overlay)) =
                    (self.gl_context.as_ref(), self.windows.get(&id))
                {
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        overlay.gl_surface.resize(context, w, h);
                    }
                }
                self.reconcile_geometry();
            }
            WindowEvent::RedrawRequested => {
                self.redraw(id);
                self.process_commands(event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exiting {
            return;
        }
        // A steady redraw keeps the live measurement, fades and timed messages
        // moving without having to wire every one of them to an event.
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL));
        for overlay in self.windows.values() {
            overlay.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Painters own GL resources that must be released while their context
        // is still alive.
        if let Some(context) = self.gl_context.as_ref() {
            for overlay in self.windows.values_mut() {
                if context.make_current(&overlay.gl_surface).is_ok() {
                    overlay.painter.destroy();
                }
            }
        }
    }
}

/// Window attributes shared by every overlay window.
fn base_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Screen Ruler")
        .with_decorations(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_transparent(false)
}

/// Pairs a captured monitor with the window system's handle for it.
///
/// Names are the most reliable key — both winit and the capture backend read
/// them from the same connector — with position as a fallback and list order as
/// a last resort, so an unnamed output still gets a window.
fn match_monitor(
    available: &[MonitorHandle],
    geometry: &crate::geometry::MonitorGeometry,
    index: usize,
) -> Option<MonitorHandle> {
    available
        .iter()
        .find(|handle| handle.name().as_deref() == Some(geometry.name.as_str()))
        .or_else(|| {
            available.iter().find(|handle| {
                let position = handle.position();
                (position.x, position.y) == geometry.position
            })
        })
        .or_else(|| available.get(index))
        .cloned()
}

/// Uploads a captured screenshot as an egui image, downscaling it if the driver
/// cannot hold a texture that large.
///
/// Only the displayed backdrop loses fidelity: measurements are cast against
/// the full-resolution edge map, so a downscale costs sharpness, not accuracy.
/// Returns the image alongside the downscale factor applied, if any, so the
/// caller can explain the reduction rather than this converter printing it.
fn to_color_image(
    image: &crate::image::Rgba8,
    max_texture_side: usize,
) -> (egui::ColorImage, Option<usize>) {
    let longest = image.width().max(image.height());
    if longest <= max_texture_side || max_texture_side == 0 {
        let full = egui::ColorImage::from_rgba_unmultiplied(
            [image.width(), image.height()],
            image.as_bytes(),
        );
        return (full, None);
    }

    let factor = longest.div_ceil(max_texture_side);
    let reduced = image.downscale(factor);
    let scaled = egui::ColorImage::from_rgba_unmultiplied(
        [reduced.width(), reduced.height()],
        reduced.as_bytes(),
    );
    (scaled, Some(factor))
}

/// Renders an edge map as a white-on-transparent image for the debug overlay.
fn edge_color_image(edges: &edges::EdgeMap) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(edges.width() * edges.height() * 4);
    for is_edge in edges.as_slice() {
        if *is_edge {
            pixels.extend_from_slice(&[255, 255, 255, 255]);
        } else {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([edges.width(), edges.height()], &pixels)
}
