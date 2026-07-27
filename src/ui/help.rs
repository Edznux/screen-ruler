//! The shortcut overlay, shown briefly at start-up and on demand with `?` / `H`.

use std::borrow::Cow;

use egui::{Color32, Ui};

use crate::state::{Mode, RulerState, MODE_COUNT};
use crate::ui::theme;

/// Builds the shortcut lines, tailored to the current mode and session state.
///
/// Context-sensitive on purpose: showing the drag hints while in crosshair
/// mode, or the quick-copy hint during a session, taught the wrong thing.
///
/// `Cow` because only the key-range line varies: the rest are static, and this
/// runs every frame the overlay is up.
pub fn shortcut_lines(state: &RulerState) -> Vec<Cow<'static, str>> {
    let mut lines: Vec<Cow<'static, str>> = vec![
        // Generated, not written out: `--help` derives the same range from the
        // mode table, and an overlay that disagreed with it would be telling
        // the user a key that does nothing.
        format!("Keys 1-{MODE_COUNT} — switch measurement mode").into(),
    ];

    lines.push(
        if state.session {
            "Ctrl+C — copy all annotations as Markdown"
        } else {
            "Ctrl+C — copy measurement, then quit"
        }
        .into(),
    );

    lines.push(
        match (state.mode, state.session) {
            (Mode::Distance, true) => "Click — place point A, then point B (persistent annotation)",
            (Mode::Distance, false) => "Click — place point A, then point B to copy, then quit",
            (mode, false) if mode.is_rect_selection() => "Drag — create selection",
            (_, true) => "Click — place annotation (persistent)",
            (_, false) => "Click — copy measurement, then quit",
        }
        .into(),
    );

    if !state.session && state.mode.is_rect_selection() {
        lines.push("Enter — copy current selection, then quit".into());
    }

    if state.session {
        lines.push("Ctrl+Z / Ctrl+Shift+Z — undo / redo annotation".into());
        lines.push("Ctrl+Shift+C — drag a region to copy it with annotations".into());
        lines.push("Tab or Esc — leave session mode".into());
    } else {
        lines.push("Tab — enter session mode (persistent annotations)".into());
    }

    // A mode can show more than one slider, and the wheel only drives the
    // first, so naming "the mode's control" would be ambiguous where it matters.
    lines.push("Wheel — adjust the top slider for this mode".into());
    lines.push("? or H — toggle this overlay    ·    Q or Esc — quit".into());

    lines
}

/// Draws the overlay at `opacity`, which the caller animates.
pub fn show(ui: &mut Ui, state: &RulerState, opacity: f32) {
    if opacity <= 0.01 {
        return;
    }

    egui::Frame::new()
        .fill(theme::panel_fill().gamma_multiply(opacity))
        .corner_radius(theme::corner_radius())
        .inner_margin(egui::Margin::symmetric(
            theme::BASE_MARGIN as i8,
            10,
        ))
        .show(ui, |ui| {
            ui.set_width(theme::PANEL_WIDTH);
            ui.label(
                egui::RichText::new("Shortcuts")
                    .color(Color32::WHITE.gamma_multiply(opacity))
                    .size(theme::TITLE_SIZE)
                    .strong(),
            );
            for line in shortcut_lines(state) {
                ui.label(
                    egui::RichText::new(line.as_ref())
                        .color(Color32::WHITE.gamma_multiply(opacity))
                        .size(theme::VALUE_SIZE),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(mode: Mode, session: bool) -> RulerState {
        let mut state = RulerState::new(85.0, false);
        state.set_mode(mode);
        state.set_session(session);
        state
    }

    #[test]
    fn the_key_range_comes_from_the_mode_table() {
        // Regression: this line said "Keys 1-6" verbatim while `--help` derived
        // its range from the table, so a seventh mode would have looked
        // keyboard-inaccessible to anyone reading the on-screen overlay.
        let lines = shortcut_lines(&state_with(Mode::Crosshair, false));
        assert!(
            lines
                .iter()
                .any(|l| l.contains(&format!("Keys 1-{MODE_COUNT}"))),
            "{lines:?}"
        );
    }

    #[test]
    fn quick_mode_advertises_copy_and_quit() {
        let lines = shortcut_lines(&state_with(Mode::Crosshair, false));
        assert!(
            lines.iter().any(|l| l.contains("copy measurement, then quit")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("Tab — enter session mode")));
    }

    #[test]
    fn session_mode_advertises_markdown_export_and_undo() {
        let lines = shortcut_lines(&state_with(Mode::Crosshair, true));
        assert!(lines.iter().any(|l| l.contains("as Markdown")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("undo / redo")), "{lines:?}");
        assert!(
            !lines.iter().any(|l| l.contains("then quit")),
            "quick-copy hint leaked into session mode: {lines:?}"
        );
    }

    #[test]
    fn rect_modes_advertise_dragging_and_enter() {
        let lines = shortcut_lines(&state_with(Mode::RectDrag, false));
        assert!(lines.iter().any(|l| l.contains("Drag — create selection")));
        assert!(lines.iter().any(|l| l.starts_with("Enter —")));

        // Enter only copies a selection, so it is irrelevant elsewhere.
        let crosshair = shortcut_lines(&state_with(Mode::Crosshair, false));
        assert!(!crosshair.iter().any(|l| l.starts_with("Enter —")));
    }

    #[test]
    fn distance_mode_explains_the_two_click_flow() {
        let lines = shortcut_lines(&state_with(Mode::Distance, false));
        assert!(
            lines.iter().any(|l| l.contains("place point A, then point B")),
            "{lines:?}"
        );
    }
}
