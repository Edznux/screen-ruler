//! Where the floating chrome landed, reported back by the code that placed it.
//!
//! The panel, the message chip and the help overlay are positioned from each
//! other's measured sizes, which is correct but invisible: nothing outside a
//! running compositor could previously say whether two of them overlapped, or
//! whether a mode's sliders lined up. Returning the rectangles makes those
//! questions answerable from a test, against the same numbers the real UI drew
//! with rather than a reimplementation of them.
//!
//! Every collection here is fixed-size. The panel is laid out on every frame,
//! and a `Vec` per field would mean heap traffic on the render path in service
//! of something only tests read — the sizes are all known from the mode table,
//! so there is no reason to pay it.
//!
//! A release build never reads any of it back, which is why the module carries
//! an `allow(dead_code)` outside tests. That is not the same as the data being
//! dead: dropping it would remove the only way to check the layout without a
//! compositor.

use egui::Rect;

use crate::state::{Control, Mode, MAX_CONTROLS, MODE_COUNT};

/// SESSION, MD and IMG.
pub const SESSION_BUTTONS: usize = 3;

/// One labelled slider row.
#[derive(Clone, Copy, Debug)]
pub struct SliderRow {
    pub control: Control,
    /// The fixed-width label column.
    pub label: Rect,
    /// The draggable track.
    pub track: Rect,
}

/// The contents of the controls panel frame.
#[derive(Clone, Copy, Debug)]
pub struct PanelLayout {
    pub mode_buttons: [(Mode, Rect); MODE_COUNT],
    /// One entry per dial the current mode declares; the remainder are `None`.
    pub sliders: [Option<SliderRow>; MAX_CONTROLS],
    /// The `SESSION` badge and its two export buttons, while a session runs.
    pub session_buttons: Option<[Rect; SESSION_BUTTONS]>,
}

impl PanelLayout {
    /// The slider rows actually shown, in the order the mode declares them.
    pub fn sliders(&self) -> impl Iterator<Item = &SliderRow> {
        self.sliders.iter().flatten()
    }
}

/// The floating chrome of one monitor, for one frame.
#[derive(Clone, Copy, Debug)]
pub struct ChromeLayout {
    pub controls: Rect,
    pub message: Option<Rect>,
    pub help: Option<Rect>,
    pub panel: PanelLayout,
}
