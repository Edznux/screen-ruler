//! Visual design tokens, ported from the previous `RulerTheme.qml`.
//!
//! Kept in one place so the on-screen overlay and the exported composite image
//! cannot drift apart.

use egui::{Color32, CornerRadius, Stroke};

/// Brand accent used for every measurement mark.
pub const ACCENT: Color32 = Color32::from_rgb(0xE6, 0x19, 0x5E);
/// Floating panel background, before opacity is applied.
pub const PANEL_BG: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const TEXT: Color32 = Color32::WHITE;
pub const PANEL_SHADOW: Color32 = Color32::from_black_alpha(56);

/// Opacity applied to floating panels.
pub const PANEL_OPACITY: f32 = 0.9;

pub const BASE_MARGIN: f32 = 14.0;
pub const CORNER_RADIUS: u8 = 5;
pub const LABEL_OFFSET_Y: f32 = 4.0;
pub const LABEL_SHADOW_OFFSET: f32 = 2.0;
pub const LABEL_H_PADDING: f32 = 18.0;
pub const LABEL_V_PADDING: f32 = 12.0;

/// Opacity of the debug edge-map overlay.
pub const DEBUG_OVERLAY_OPACITY: f32 = 0.3;
/// Peak opacity of the transient edge preview shown after a sensitivity change.
pub const EDGE_PREVIEW_PEAK_OPACITY: f32 = 0.5;

pub const PANEL_WIDTH: f32 = 380.0;
pub const CONTROLS_COLUMN_SPACING: f32 = 8.0;
pub const MODE_ROW_SPACING: f32 = 8.0;
pub const MODE_BUTTON_SIZE: f32 = 30.0;

pub const TITLE_SIZE: f32 = 14.0;
pub const VALUE_SIZE: f32 = 13.0;
pub const LABEL_TEXT_SIZE: f32 = 13.0;

/// Session-mode border, drawn as a frame around each monitor.
pub const SESSION_BORDER_THICKNESS: f32 = 5.0;
pub const SESSION_BORDER_OPACITY: f32 = 0.85;
/// The border fades out when the cursor comes within this many pixels, so it
/// never obscures what is being measured near a screen edge.
pub const SESSION_BORDER_FADE_DISTANCE: f32 = 100.0;
pub const SESSION_BORDER_NEAR_OPACITY: f32 = 0.1;

pub fn corner_radius() -> CornerRadius {
    CornerRadius::same(CORNER_RADIUS)
}

/// Hairline stroke used for measurement marks.
pub fn accent_stroke(width: f32) -> Stroke {
    Stroke::new(width, ACCENT)
}

/// Panel background with the shared panel opacity baked in.
pub fn panel_fill() -> Color32 {
    PANEL_BG.gamma_multiply(PANEL_OPACITY)
}
