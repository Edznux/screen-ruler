//! Runs the real UI without a display, so its geometry can be asserted on.
//!
//! egui does not need a window, a GPU or a compositor to lay out: given a
//! screen rectangle it will place every panel and widget on the CPU and hand
//! back the rectangles. That is enough to answer the questions that previously
//! needed someone to look at a screen — does the confirmation prompt cover the
//! controls panel, do a mode's two sliders line up, does a measurement chip
//! anchored in a corner stay on the monitor.
//!
//! Crucially this drives [`super::show_panels`] and [`super::overlay_shapes`]
//! themselves rather than models of them, so the numbers asserted here are the
//! numbers the compositor is handed. A second implementation of the layout
//! could only ever agree with itself.
//!
//! What it does *not* cover: colour, text rendering, and anything downstream of
//! the painter. A chip in the right place with white-on-white text passes every
//! test here.

use std::sync::OnceLock;

use egui::{Context, Pos2, RawInput, Rect, Shape, Vec2};

use crate::edges::DEFAULT_SENSITIVITY;
use crate::state::{Annotation, Mode, Point, RulerState};
use crate::surface::Surface;
use crate::ui::layout::ChromeLayout;

/// Stand-in for the edge count the panel prints beside the sensitivity slider.
const TOTAL_EDGES: usize = 4_096;

/// The monitor the overlay tests measure against.
///
/// Small on purpose. Edge detection is the expensive part of building one, and
/// a cramped canvas exercises the label-flipping logic harder than a roomy one:
/// a chip that would run off a 1920 px screen runs off this one much sooner.
pub const OVERLAY_CANVAS: Vec2 = Vec2::new(384.0, 216.0);

/// Built once — canny over even a small image is not free, and every overlay
/// test wants the same surface.
fn overlay_surface() -> &'static Surface {
    static SURFACE: OnceLock<Surface> = OnceLock::new();
    SURFACE.get_or_init(|| {
        Surface::test_split(
            1.0,
            OVERLAY_CANVAS.x as usize,
            OVERLAY_CANVAS.y as usize,
            OVERLAY_CANVAS.x as usize / 2,
        )
    })
}

/// A monitor with no monitor behind it.
pub struct Harness {
    ctx: Context,
    canvas: Vec2,
    pub state: RulerState,
}

impl Harness {
    /// A harness for a monitor of the given logical size.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            ctx: Context::default(),
            canvas: Vec2::new(width, height),
            state: RulerState::new(DEFAULT_SENSITIVITY, false),
        }
    }

    /// A harness on a typical 1080p monitor.
    pub fn hd() -> Self {
        Self::new(1920.0, 1080.0)
    }

    /// A harness sized to match [`overlay_surface`], for the overlay tests.
    pub fn overlay() -> Self {
        Self::new(OVERLAY_CANVAS.x, OVERLAY_CANVAS.y)
    }

    pub fn canvas(&self) -> Vec2 {
        self.canvas
    }

    /// The whole monitor, as a rectangle.
    pub fn screen(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.canvas)
    }

    // ------------------------------------------------------------- setup

    pub fn set_mode(&mut self, mode: Mode) -> &mut Self {
        self.state.set_mode(mode);
        self
    }

    /// Puts the cursor at a monitor-local logical point.
    pub fn cursor_at(&mut self, x: f32, y: f32) -> &mut Self {
        self.state.pointer = Some(Point {
            monitor: 0,
            x,
            y,
        });
        self
    }

    /// Enters session mode with one annotation placed.
    ///
    /// The annotation is what makes a destructive action arm a confirmation
    /// rather than fire immediately, so it is part of reaching that state.
    pub fn in_session(&mut self) -> &mut Self {
        self.state.set_session(true);
        self.state
            .add_annotation(Annotation::test_rect(40.0, 40.0, 120.0, 80.0));
        self
    }

    /// Arms a confirm-before-discard prompt. Requires [`Harness::in_session`].
    pub fn with_prompt(&mut self) -> &mut Self {
        self.state
            .request_destructive(crate::state::Destructive::Quit);
        assert!(
            self.state.destructive_prompt().is_some(),
            "with_prompt needs in_session first: with nothing to lose, a \
             destructive action fires instead of asking",
        );
        self
    }

    pub fn with_feedback(&mut self, message: &str) -> &mut Self {
        self.state.show_feedback(message);
        self
    }

    pub fn with_help(&mut self, visible: bool) -> &mut Self {
        self.state.help_visible = visible;
        // Pin it: the start-up fade is time-based, and a layout test should not
        // depend on how long it took to get here.
        self.state.help_auto_hide_at = None;
        self
    }

    // ------------------------------------------------------------ running

    /// Runs `passes` headless egui frames and returns what the last one built.
    ///
    /// More than one pass matters because egui sizes some containers from what
    /// they measured last time; the final pass is what a user would see.
    fn frame<R>(&mut self, passes: usize, mut body: impl FnMut(&Context, &mut RulerState) -> R) -> R {
        let input = RawInput {
            screen_rect: Some(self.screen()),
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        let state = &mut self.state;

        let mut captured = None;
        for _ in 0..passes {
            // The textures and shapes egui produces are the painter's business;
            // only what `body` reports matters here.
            let _ = ctx.run(input.clone(), |ctx| {
                captured = Some(body(ctx, state));
            });
        }
        captured.expect("at least one pass runs")
    }

    /// Lays out the floating chrome and reports where it landed.
    pub fn layout(&mut self) -> ChromeLayout {
        let canvas = self.canvas;
        self.frame(2, move |ctx, state| {
            super::show_panels(ctx, state, canvas, TOTAL_EDGES)
        })
    }

    /// Whether egui would claim a pointer at `pos`, and so whether
    /// [`super::handle_input`] bails out instead of measuring there.
    ///
    /// True for anything with widgets to click. False is what transient chrome
    /// needs: a chip that claims the pointer eats the very drag it is asking
    /// for.
    pub fn pointer_is_captured_at(&mut self, pos: Pos2) -> bool {
        let input = RawInput {
            screen_rect: Some(self.screen()),
            events: vec![egui::Event::PointerMoved(pos)],
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        let canvas = self.canvas;
        let state = &mut self.state;

        let mut captured = false;
        for _ in 0..2 {
            let _ = ctx.run(input.clone(), |ctx| {
                super::show_panels(ctx, state, canvas, TOTAL_EDGES);
            });
            captured = ctx.wants_pointer_input();
        }
        captured
    }

    /// The shapes the measurement overlay would paint this frame.
    ///
    /// Requires [`Harness::overlay`]: the shapes are measured against a real
    /// [`Surface`], and that surface is built at one size.
    pub fn shapes(&mut self) -> Vec<Shape> {
        assert_eq!(
            self.canvas, OVERLAY_CANVAS,
            "overlay shapes are measured against a surface built for \
             Harness::overlay()",
        );
        self.frame(1, |ctx, state| {
            ctx.fonts_mut(|fonts| {
                super::overlay_shapes(fonts, state, overlay_surface(), 0, OVERLAY_CANVAS, true)
            })
        })
    }

    /// The area those shapes cover, ignoring ones with no extent.
    pub fn painted_bounds(&mut self) -> Option<Rect> {
        union_of(self.shapes().iter().map(Shape::visual_bounding_rect))
    }

    /// The area the *text* covers.
    ///
    /// The measurement chips are the part that has to stay on screen: a clipped
    /// reading is unreadable, whereas a crosshair tick running half a cap past
    /// the edge is just a mark meeting the edge of the display. Asserting on
    /// text alone keeps the check strict where it matters instead of loose
    /// everywhere.
    pub fn text_bounds(&mut self) -> Option<Rect> {
        let mut rects = Vec::new();
        for shape in self.shapes() {
            collect_text_bounds(&shape, &mut rects);
        }
        union_of(rects.into_iter())
    }
}

/// `Rect::visual_bounding_rect` reports `Rect::NOTHING` for an empty shape,
/// which is not a region anyone can see and must not be unioned in.
fn union_of(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects
        .filter(|rect| rect.is_finite() && rect.is_positive())
        .reduce(|a, b| a.union(b))
}

/// Text shapes are nested inside the `Shape::Vec`s that chips are built from,
/// so this has to walk rather than scan the top level.
fn collect_text_bounds(shape: &Shape, into: &mut Vec<Rect>) {
    match shape {
        Shape::Text(text) => into.push(text.visual_bounding_rect()),
        Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text_bounds(shape, into);
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------- assertions

/// The floating panels, named for diagnostics.
///
/// Panels only — not the widgets inside them, which are expected to sit within
/// their parent and would report an overlap with it.
pub fn panels(layout: &ChromeLayout) -> Vec<(&'static str, Rect)> {
    let mut panels = vec![("controls", layout.controls)];
    if let Some(rect) = layout.message {
        panels.push(("message", rect));
    }
    if let Some(rect) = layout.help {
        panels.push(("help", rect));
    }
    panels
}

/// Everything placed, panels and the widgets within them.
pub fn everything(layout: &ChromeLayout) -> Vec<(&'static str, Rect)> {
    let mut all = panels(layout);
    all.extend(
        layout
            .panel
            .mode_buttons
            .iter()
            .map(|(_, rect)| ("mode button", *rect)),
    );
    if let Some(buttons) = &layout.panel.session_buttons {
        all.extend(buttons.iter().map(|rect| ("session button", *rect)));
    }
    for slider in layout.panel.sliders() {
        all.push(("slider label", slider.label));
        all.push(("slider track", slider.track));
    }
    all
}

/// Fails when two named rectangles overlap by more than a hair.
///
/// The tolerance absorbs the sub-pixel rounding egui does when centring, which
/// is not a collision anyone can see.
#[track_caller]
pub fn assert_no_overlaps(case: &str, regions: &[(&'static str, Rect)]) {
    const TOLERANCE: f32 = 0.5;

    for (index, (name_a, a)) in regions.iter().enumerate() {
        for (name_b, b) in &regions[index + 1..] {
            let overlap = a.intersect(*b);
            assert!(
                overlap.width() <= TOLERANCE || overlap.height() <= TOLERANCE,
                "{case}: {name_a} {a:?} overlaps {name_b} {b:?} by {:.1}x{:.1} px",
                overlap.width(),
                overlap.height(),
            );
        }
    }
}

/// Fails when any named rectangle leaves the monitor.
#[track_caller]
pub fn assert_within(case: &str, screen: Rect, regions: &[(&'static str, Rect)]) {
    for (name, rect) in regions {
        assert!(
            screen.contains_rect(*rect),
            "{case}: {name} {rect:?} is outside the {screen:?} monitor",
        );
    }
}
