//! Per-monitor rendering and input handling.
//!
//! Every monitor gets its own window and its own egui context, but they all
//! share one [`RulerState`]. A mode switched on one screen is switched
//! everywhere; annotations stay attached to the display they were placed on.

#[cfg(test)]
mod harness;
pub mod help;
// Only the headless tests read what `layout` reports; see its module docs.
#[cfg_attr(not(test), allow(dead_code))]
pub mod layout;
#[cfg(test)]
mod layout_tests;
pub mod overlay;
pub mod panel;
pub mod theme;

use std::time::Instant;

use egui::{Color32, Context, Key, Pos2, Rect, Vec2};

use crate::color::KernelCache;
use crate::state::{Annotation, AnnotationKind, Destructive, Mode, Point, RulerState, HELP_FADE};
use crate::surface::Surface;
use crate::ui::layout::ChromeLayout;

/// A drag shorter than this counts as a click, not a selection.
const DRAG_THRESHOLD: f32 = 2.0;
/// Vertical gap between the stacked floating panels.
const PANEL_STACK_GAP: f32 = 10.0;

/// A single row of number keys is all the mode shortcuts there are, so the
/// table must stay inside what `mode_digit` can express. Otherwise `--help`
/// and the button tooltips would advertise a key that cannot be pressed.
const _: () = assert!(
    crate::state::MODE_COUNT <= 9,
    "more modes than the number keys can select — the shortcut scheme needs \
     rethinking before another mode is added"
);

/// The `1`-based digit a number key types, whether or not a mode claims it.
///
/// Deliberately not an array of the keys that happen to bind today: the digit
/// goes through [`Mode::from_digit`], so the number keys and [`Mode::ALL`]
/// cannot fall out of order the way two parallel lists could.
fn mode_digit(key: Key) -> Option<usize> {
    Some(match key {
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        Key::Num6 => 6,
        Key::Num7 => 7,
        Key::Num8 => 8,
        Key::Num9 => 9,
        _ => return None,
    })
}

/// Everything one window needs to render a frame.
pub struct FrameContext<'a> {
    pub monitor: usize,
    pub surface: &'a Surface,
    /// The monitor's screenshot, uploaded once at start-up.
    pub screenshot: egui::TextureId,
    /// The edge map as a texture, for the debug and preview overlays.
    pub edge_texture: Option<egui::TextureId>,
    pub state: &'a mut RulerState,
    pub colors: &'a mut KernelCache,
    pub total_edges: usize,
    /// Draw only the screenshot and placed annotations.
    ///
    /// Set for the single frame that composite export reads back, so the
    /// controls panel and live cursor marks stay out of the exported image.
    pub chrome_hidden: bool,
}

/// Renders one monitor and folds its input into the shared state.
pub fn draw(ctx: &Context, frame: FrameContext<'_>) {
    let FrameContext {
        monitor,
        surface,
        screenshot,
        edge_texture,
        state,
        colors,
        total_edges,
        chrome_hidden,
    } = frame;

    let canvas = {
        let (w, h) = surface.geometry.logical_size();
        Vec2::new(w, h)
    };

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let screen = ui.max_rect();
            let painter = ui.painter().clone();
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

            // The captured screenshot *is* the background. Freezing the screen
            // this way avoids relying on compositor transparency, which behaves
            // differently on every platform and was a recurring source of
            // misaligned overlays.
            painter.image(screenshot, screen, uv, Color32::WHITE);

            if chrome_hidden {
                // Export frame: the screenshot plus the placed annotations, and
                // nothing that belongs to the live UI.
                let shapes = ctx.fonts_mut(|fonts| {
                    overlay::annotations_for_monitor(fonts, &state.annotations, monitor, canvas)
                });
                painter.extend(shapes);
                return;
            }

            if let (Some(edges), Some(opacity)) = (edge_texture, edge_overlay_opacity(state)) {
                painter.image(edges, screen, uv, Color32::WHITE.gamma_multiply(opacity));
            }

            let hover = ui.input(|i| i.pointer.hover_pos());
            let active = hover.is_some();
            if let Some(pos) = hover {
                state.pointer = Some(Point {
                    monitor,
                    x: pos.x,
                    y: pos.y,
                });
                state.active_monitor = monitor;
            }

            if state.session {
                let opacity = overlay::session_border_opacity(hover, canvas);
                for shape in overlay::session_border(canvas, opacity) {
                    painter.add(shape);
                }
            }

            // Work out what the cursor is pointing at before drawing it.
            if active {
                refresh_derived(state, surface, colors, monitor);
            }

            handle_input(ctx, ui, state, surface, monitor, active);
            let shapes = ctx
                .fonts_mut(|fonts| overlay_shapes(fonts, state, surface, monitor, canvas, active));
            painter.extend(shapes);

            // The panel lives on one monitor at a time and follows the cursor,
            // rather than being duplicated on every screen.
            if state.active_monitor == monitor {
                show_panels(ctx, state, canvas, total_edges);
            }
        });
}

/// Opacity of the edge-map overlay, or `None` when it should be hidden.
fn edge_overlay_opacity(state: &RulerState) -> Option<f32> {
    if state.debug_edges {
        return Some(theme::DEBUG_OVERLAY_OPACITY);
    }
    // After a sensitivity change the edge map is flashed briefly so the effect
    // of the slider is visible, then fades out.
    let until = state.edge_preview_until?;
    let now = Instant::now();
    if now >= until {
        return None;
    }
    let remaining = until.duration_since(now).as_secs_f32();
    Some(remaining.min(1.0) * theme::EDGE_PREVIEW_PEAK_OPACITY)
}

/// Updates the container, colour sample and snapped pointer for the cursor.
fn refresh_derived(
    state: &mut RulerState,
    surface: &Surface,
    colors: &mut KernelCache,
    monitor: usize,
) {
    let Some(pointer) = state.pointer else { return };
    if pointer.monitor != monitor {
        return;
    }

    state.container = if state.mode == Mode::Container {
        surface
            .container_at_logical(pointer.x, pointer.y)
            .map(|rect| rect.on_monitor(monitor))
    } else {
        None
    };

    state.sample = if state.mode == Mode::ColorPicker {
        let (px, py) = surface.logical_to_pixel(pointer.x, pointer.y);
        let radius = surface
            .geometry
            .logical_to_image(state.color_radius, 0.0)
            .0
            .round()
            .max(0.0) as usize;
        let sample = colors.sample(&surface.image, px, py, radius);
        let (lx, ly) = surface.geometry.image_to_logical(px as f32, py as f32);
        Some((Point { monitor, x: lx, y: ly }, sample))
    } else {
        None
    };

    // Snapping only assists the modes that place geometry by hand, and those
    // are exactly the modes whose panel dial is the snap distance — one flag on
    // the mode table, so the dial cannot be offered where it does nothing.
    state.snapped = if state.mode.snaps() {
        surface
            .snap_logical(pointer.x, pointer.y, state.snap_distance)
            .map(|(x, y)| Point { monitor, x, y })
    } else {
        None
    };
}

/// The point a placement should use: the snapped one when snapping applies.
fn placement_point(state: &RulerState) -> Option<Point> {
    state.snapped.or(state.pointer)
}

/// The egui rectangle for a monitor-local logical rect.
fn egui_rect(rect: &crate::state::Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(rect.x, rect.y),
        Vec2::new(rect.width, rect.height),
    )
}

/// Folds keyboard and pointer input into the shared state.
fn handle_input(
    ctx: &Context,
    ui: &egui::Ui,
    state: &mut RulerState,
    surface: &Surface,
    monitor: usize,
    active: bool,
) {
    handle_keys(ctx, state, surface, monitor);

    if !active {
        return;
    }
    // Clicks on the controls panel must not also place an annotation.
    if ctx.wants_pointer_input() {
        return;
    }

    let (pressed, released, scroll) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.raw_scroll_delta.y,
        )
    });

    if scroll != 0.0 {
        // egui reports roughly 50 logical px of scroll per wheel notch. When
        // the wheel drives sensitivity, `set_sensitivity` arms the edge preview.
        let notches = (scroll / 50.0).round().clamp(-5.0, 5.0);
        state.adjust_by_wheel(notches);
    }

    let Some(pointer) = state.pointer else { return };

    if state.export_armed {
        handle_export_drag(state, pointer, pressed, released);
        return;
    }

    match state.mode {
        Mode::RectDrag | Mode::ShrinkToFit => {
            handle_rect_drag(state, surface, pointer, monitor, pressed, released)
        }
        Mode::Distance => {
            if pressed {
                handle_distance_click(state);
            }
        }
        _ => {
            if pressed {
                handle_simple_click(state, surface, monitor);
            }
        }
    }
}

/// Drives the dedicated drag used by composite export.
fn handle_export_drag(state: &mut RulerState, pointer: Point, pressed: bool, released: bool) {
    if pressed {
        state.export_drag.active = true;
        state.export_drag.has_selection = false;
        state.export_drag.monitor = pointer.monitor;
        state.export_drag.start = (pointer.x, pointer.y);
        state.export_drag.end = (pointer.x, pointer.y);
    } else if state.export_drag.active {
        state.export_drag.end = (pointer.x, pointer.y);
        if released {
            state.export_drag.active = false;
            state.export_drag.has_selection = true;
            state.finish_export();
        }
    }
}

/// Press-drag-release for the two rectangle modes.
///
/// A release that barely moved is treated as a click rather than a selection,
/// which is what makes "drag once, then click to confirm" work.
fn handle_rect_drag(
    state: &mut RulerState,
    surface: &Surface,
    pointer: Point,
    monitor: usize,
    pressed: bool,
    released: bool,
) {
    let anchor = placement_point(state).unwrap_or(pointer);

    if pressed {
        if state.drag.has_selection {
            // A click with a finished selection confirms it.
            confirm_rect_selection(state, monitor);
            return;
        }
        state.drag.active = true;
        state.drag.monitor = monitor;
        state.drag.start = (anchor.x, anchor.y);
        state.drag.end = (anchor.x, anchor.y);
        state.drag.press = (pointer.x, pointer.y);
        return;
    }

    if !state.drag.active {
        return;
    }

    state.drag.end = (anchor.x, anchor.y);
    if !released {
        return;
    }

    state.drag.active = false;
    if !state.drag.is_drag((pointer.x, pointer.y), DRAG_THRESHOLD) {
        state.drag.has_selection = false;
        return;
    }
    state.drag.has_selection = true;

    if state.mode == Mode::ShrinkToFit {
        if let Some(rect) = state.drag.rect() {
            if let Some(shrunk) = surface.shrink_logical(rect.x, rect.y, rect.width, rect.height) {
                state.drag.start = (shrunk.x, shrunk.y);
                state.drag.end = (shrunk.x + shrunk.width, shrunk.y + shrunk.height);
            }
        }
    }

    // In a session the finished rectangle is committed straight away; in quick
    // mode it waits for an explicit confirmation so it can be re-dragged.
    if state.session {
        confirm_rect_selection(state, monitor);
    }
}

/// Commits the current rectangle: annotate in a session, copy and quit otherwise.
fn confirm_rect_selection(state: &mut RulerState, monitor: usize) {
    let Some(rect) = state.active_rect() else {
        return;
    };
    if state.session {
        let mode = state.mode;
        state.add_annotation(Annotation {
            at: Point {
                monitor,
                x: rect.x,
                y: rect.y,
            },
            mode,
            kind: AnnotationKind::Rect {
                width: rect.width,
                height: rect.height,
            },
        });
        state.drag.has_selection = false;
        state.show_feedback("Annotation placed");
    } else {
        state.copy_and_quit(None);
    }
}

/// Two-click point-to-point measurement.
fn handle_distance_click(state: &mut RulerState) {
    let Some(point) = placement_point(state) else {
        return;
    };

    match state.distance_anchor.take() {
        None => state.distance_anchor = Some(point),
        Some(anchor) => {
            if state.session {
                state.add_annotation(Annotation {
                    at: anchor,
                    mode: Mode::Distance,
                    kind: AnnotationKind::Distance {
                        to_x: point.x,
                        to_y: point.y,
                    },
                });
                state.show_feedback("Annotation placed");
            } else {
                state.copy_and_quit(None);
            }
        }
    }
}

/// Click handling for the modes that measure whatever is under the cursor.
fn handle_simple_click(state: &mut RulerState, surface: &Surface, monitor: usize) {
    let Some(pointer) = state.pointer else { return };

    if !state.session {
        let crosshair = (state.mode == Mode::Crosshair).then(|| {
            let rays = surface.rays_at(pointer.x, pointer.y);
            (rays.width(), rays.height())
        });
        state.copy_and_quit(crosshair);
        return;
    }

    let annotation = match state.mode {
        Mode::Crosshair => {
            let rays = surface.rays_at(pointer.x, pointer.y);
            Annotation {
                at: pointer,
                mode: Mode::Crosshair,
                kind: AnnotationKind::Crosshair {
                    north: pointer.y - rays.north,
                    south: pointer.y + rays.south,
                    west: pointer.x - rays.west,
                    east: pointer.x + rays.east,
                },
            }
        }
        Mode::Container => {
            let Some(rect) = state.container else { return };
            Annotation {
                at: Point {
                    monitor,
                    x: rect.x,
                    y: rect.y,
                },
                mode: Mode::Container,
                kind: AnnotationKind::Rect {
                    width: rect.width,
                    height: rect.height,
                },
            }
        }
        Mode::ColorPicker => {
            let Some((at, sample)) = state.sample.clone() else {
                return;
            };
            Annotation {
                at,
                mode: Mode::ColorPicker,
                kind: AnnotationKind::Color {
                    sample,
                    radius: state.color_radius,
                },
            }
        }
        _ => return,
    };

    state.add_annotation(annotation);
    state.show_feedback("Annotation placed");
}

/// Keyboard shortcuts.
fn handle_keys(ctx: &Context, state: &mut RulerState, surface: &Surface, monitor: usize) {
    let (keys, ctrl, shift) = ctx.input(|i| {
        let pressed: Vec<Key> = i
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    ..
                } => Some(*key),
                _ => None,
            })
            .collect();
        (
            pressed,
            i.modifiers.ctrl || i.modifiers.command,
            i.modifiers.shift,
        )
    });

    for key in keys {
        if let Some(digit) = mode_digit(key) {
            if let Some(mode) = Mode::from_digit(digit) {
                state.set_mode(mode);
            }
            // A digit with no mode behind it is still consumed, so `7` cannot
            // fall through and be read as some other shortcut later.
            continue;
        }

        match key {
            Key::Tab => {
                if state.session {
                    state.request_destructive(Destructive::ToggleSession);
                } else {
                    state.clear_pending_destructive();
                    state.set_session(true);
                }
            }

            Key::C if ctrl && shift => state.arm_export(),
            Key::C if ctrl => {
                if state.session {
                    state.copy_annotations();
                } else {
                    state.copy_and_quit(crosshair_size(state, surface, monitor));
                }
            }

            Key::Z if ctrl && shift => {
                if !state.redo() {
                    state.show_feedback("Nothing to redo");
                }
            }
            Key::Z if state.session => {
                if !state.undo() {
                    state.show_feedback("Nothing to undo");
                }
            }

            Key::Enter => {
                if !state.session && state.mode.is_rect_selection() {
                    state.copy_and_quit(None);
                }
            }

            Key::Questionmark | Key::H => state.toggle_help(),

            Key::Escape => handle_escape(state),
            Key::Q => state.request_destructive(Destructive::Quit),

            _ => {}
        }
    }
}

/// Escape unwinds one level at a time before it quits.
fn handle_escape(state: &mut RulerState) {
    if state.export_armed {
        state.cancel_export();
        return;
    }
    if state.distance_anchor.is_some() {
        state.distance_anchor = None;
        return;
    }
    if state.drag.has_selection {
        state.drag.has_selection = false;
        return;
    }
    state.request_destructive(Destructive::Escape);
}

/// Current crosshair span, when that is what is being measured.
fn crosshair_size(state: &RulerState, surface: &Surface, monitor: usize) -> Option<(f32, f32)> {
    let pointer = state.pointer?;
    if state.mode != Mode::Crosshair || pointer.monitor != monitor {
        return None;
    }
    let rays = surface.rays_at(pointer.x, pointer.y);
    Some((rays.width(), rays.height()))
}

/// The measurement marks for the current mode, plus every annotation.
///
/// Split out from the painting so it can be inspected without one: this decides
/// *what* is drawn, which is the part with rules in it. Everything in
/// [`overlay`] is already shaped this way; this is the composition of them.
pub fn overlay_shapes(
    fonts: &mut egui::epaint::text::FontsView<'_>,
    state: &RulerState,
    surface: &Surface,
    monitor: usize,
    canvas: Vec2,
    active: bool,
) -> Vec<egui::Shape> {
    let mut shapes = overlay::annotations_for_monitor(fonts, &state.annotations, monitor, canvas);

    if !active {
        return shapes;
    }
    let Some(pointer) = state.pointer else {
        return shapes;
    };
    let cursor = Pos2::new(pointer.x, pointer.y);

    if state.export_armed {
        if let Some(rect) = state.export_drag.rect() {
            shapes.push(overlay::selection_rect(egui_rect(&rect)));
        }
        return shapes;
    }

    // `state.snapped` is only ever set in a mode that snaps — `set_mode` clears
    // it on the way out — so the marker needs no per-mode guard of its own. It
    // used to be listed under `Container`, where it could never appear.
    if let Some(snapped) = state.snapped {
        shapes.extend(overlay::snapped_marker(Pos2::new(snapped.x, snapped.y)));
    }

    match state.mode {
        Mode::Crosshair => {
            let rays = surface.rays_at(pointer.x, pointer.y);
            shapes.extend(overlay::crosshair(
                cursor,
                pointer.y - rays.north,
                pointer.y + rays.south,
                pointer.x - rays.west,
                pointer.x + rays.east,
            ));
            shapes.extend(overlay::measurement_label(
                fonts,
                cursor,
                &crate::state::format_size(rays.width(), rays.height()),
                overlay::label_offset(),
                canvas,
            ));
        }
        Mode::RectDrag | Mode::ShrinkToFit | Mode::Container => {
            // `DragState::rect` already reports only a live or finished
            // selection, so no extra gate is needed here.
            if let Some(rect) = state.active_rect().or_else(|| state.drag.rect()) {
                let bounds = egui_rect(&rect);
                shapes.push(overlay::selection_rect(bounds));
                shapes.extend(overlay::measurement_label(
                    fonts,
                    bounds.min,
                    &crate::state::format_size(rect.width, rect.height),
                    overlay::label_offset(),
                    canvas,
                ));
            }
        }
        Mode::ColorPicker => {
            if let Some((at, sample)) = &state.sample {
                let at = Pos2::new(at.x, at.y);
                shapes.extend(overlay::color_marker(at, state.color_radius));
                shapes.extend(overlay::color_bubble(
                    fonts,
                    at,
                    sample.notations(),
                    Color32::from_rgb(sample.r, sample.g, sample.b),
                    canvas,
                ));
            }
        }
        Mode::Distance => {
            if let Some(anchor) = state.distance_anchor {
                let to = placement_point(state).unwrap_or(pointer);
                shapes.extend(overlay::distance(
                    fonts,
                    Pos2::new(anchor.x, anchor.y),
                    Pos2::new(to.x, to.y),
                    canvas,
                ));
            }
        }
    }

    shapes
}

/// Draws the controls panel, transient messages and the help overlay, in that
/// order down the screen.
///
/// Each is placed below whatever the one above actually measured, rather than
/// at a constant: the panel grows a row in session mode and another for a mode
/// with two dials, so any fixed offset only holds for the layout it was tuned
/// against. The message chip used to carry one and rendered on top of the
/// panel.
///
/// The message sits *above* the help overlay, not below it, so that its
/// position depends only on the panel. Stacking it last made a confirmation
/// prompt appear far down the screen while help was up and then jump upward the
/// moment help faded — chrome asking a question has to hold still. Help moving
/// instead is harmless: it is a passive reference that fades on its own.
fn show_panels(
    ctx: &Context,
    state: &mut RulerState,
    canvas: Vec2,
    total_edges: usize,
) -> ChromeLayout {
    // `PANEL_WIDTH` is the *content* width; the frame adds a margin either side,
    // so the panel on screen is wider than that. Centring on the content alone
    // put it half a margin right of the monitor's centre — invisible until the
    // message chip below it started centring properly and the two disagreed.
    let panel_width = theme::PANEL_WIDTH + 2.0 * theme::BASE_MARGIN;
    let left = (canvas.x - panel_width) / 2.0;

    let controls = egui::Area::new(egui::Id::new("controls"))
        .fixed_pos(Pos2::new(left, theme::BASE_MARGIN))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill())
                .corner_radius(theme::corner_radius())
                .inner_margin(egui::Margin::symmetric(theme::BASE_MARGIN as i8, 8))
                .show(ui, |ui| {
                    ui.set_width(theme::PANEL_WIDTH);
                    panel::show(ui, state, total_edges)
                })
                .inner
        });
    let panel_layout = controls.inner;
    let mut next_y = controls.response.rect.bottom() + PANEL_STACK_GAP;

    // A pending confirmation outranks passing feedback: it is asking a
    // question, and the feedback that armed it is no longer the useful message.
    let prompt = state.destructive_prompt();
    let warning = prompt.is_some();
    let message = prompt
        .map(str::to_string)
        .or_else(|| state.feedback_message())
        // An empty message is no message: drawing it would reserve a chip-shaped
        // hole and push the help overlay down for nothing.
        .filter(|message| !message.is_empty());
    let message_rect = message.map(|message| {
        // Measured first, then placed: centring is done here, against the same
        // `canvas` the panel above is centred on, rather than by `Area::anchor`
        // — which centres from last frame's size and so slides the chip
        // sideways for a frame whenever the wording changes length.
        let chip = panel::measure_message(ctx, &message);
        let chip_left = ((canvas.x - chip.size.x) / 2.0).max(0.0);

        let area = egui::Area::new(egui::Id::new("message"))
            .fixed_pos(Pos2::new(chip_left, next_y))
            // Transient chrome must not eat the input aimed at the screen
            // behind it: `wants_pointer_input` is true over *any* interactable
            // area, and `handle_input` bails out on that. The export prompt is
            // the worst case — it lands exactly where it is telling the user to
            // start dragging.
            .interactable(false)
            .show(ctx, |ui| {
                panel::message_bubble(ui, chip, warning);
            });
        next_y = area.response.rect.bottom() + PANEL_STACK_GAP;
        area.response.rect
    });

    let opacity = help_opacity(state);
    let help_rect = (opacity > 0.01).then(|| {
        egui::Area::new(egui::Id::new("help"))
            .fixed_pos(Pos2::new(left, next_y))
            // Read-only, like the message chip: it covers a large part of the
            // screen and has nothing to click.
            .interactable(false)
            .show(ctx, |ui| {
                help::show(ui, state, opacity);
            })
            .response
            .rect
    });

    ChromeLayout {
        controls: controls.response.rect,
        message: message_rect,
        help: help_rect,
        panel: panel_layout,
    }
}

/// Opacity of the help overlay, driving the start-up fade-out.
fn help_opacity(state: &mut RulerState) -> f32 {
    if !state.help_visible {
        return 0.0;
    }
    let Some(hide_at) = state.help_auto_hide_at else {
        return 1.0;
    };

    let now = Instant::now();
    if now < hide_at {
        return 1.0;
    }
    let elapsed = now.duration_since(hide_at).as_secs_f32();
    let fade = HELP_FADE.as_secs_f32();
    if elapsed >= fade {
        state.help_visible = false;
        state.help_auto_hide_at = None;
        return 0.0;
    }
    1.0 - elapsed / fade
}
