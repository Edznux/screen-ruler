//! The floating controls panel: mode buttons and the mode's active slider.

use egui::{
    Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind,
    Ui, Vec2,
};

use crate::state::{Mode, RulerState, COLOR_RADIUS_MAX, SENSITIVITY_MAX, SNAP_DISTANCE_MAX};
use crate::ui::{overlay, theme};

const BUTTON_CORNER: u8 = 4;
const BORDER: Color32 = Color32::from_rgba_premultiplied(133, 143, 156, 128);
const BG: Color32 = Color32::from_rgba_premultiplied(66, 71, 79, 179);
const BG_HOVER: Color32 = Color32::from_rgba_premultiplied(87, 94, 107, 217);
const BG_ACTIVE: Color32 = Color32::from_rgba_premultiplied(60, 7, 25, 46);

/// Lays out the panel: the mode row, then the active mode's slider.
pub fn show(ui: &mut Ui, state: &mut RulerState, edge_count: usize) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let mut mode_clicked = None;
            for mode in Mode::ALL {
                if mode_button(ui, mode, state.mode == mode).clicked() {
                    mode_clicked = Some(mode);
                }
            }
            if let Some(mode) = mode_clicked {
                state.set_mode(mode);
            }

            if state.session {
                ui.add_space(theme::MODE_ROW_SPACING);
                session_badge(ui, state);
            }
        });

        ui.add_space(theme::CONTROLS_COLUMN_SPACING);
        active_slider(ui, state, edge_count);
    });
}

/// The `SESSION` badge plus its two export shortcuts.
fn session_badge(ui: &mut Ui, state: &mut RulerState) {
    let response = ui.add(
        egui::Button::new(
            egui::RichText::new("SESSION")
                .color(Color32::WHITE)
                .size(theme::VALUE_SIZE)
                .strong(),
        )
        .fill(theme::ACCENT)
        .corner_radius(CornerRadius::same(BUTTON_CORNER)),
    );
    if response.clicked() {
        state.request_destructive(crate::state::Destructive::SessionButton);
    }
    response.on_hover_text("Leave session mode (discards annotations)");

    if ui
        .add(small_button("MD"))
        .on_hover_text("Copy all annotations as Markdown")
        .clicked()
    {
        state.copy_annotations();
    }
    if ui
        .add(small_button("IMG"))
        .on_hover_text("Drag a region to copy it with annotations")
        .clicked()
    {
        state.arm_export();
    }
}

fn small_button(label: &'static str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(label)
            .color(Color32::WHITE)
            .size(theme::VALUE_SIZE),
    )
    .fill(BG)
    .corner_radius(CornerRadius::same(BUTTON_CORNER))
}

/// The slider the current mode owns, labelled with its value.
///
/// Only one is shown at a time: the wheel drives whichever it is, so having a
/// single visible dial keeps that mapping obvious.
fn active_slider(ui: &mut Ui, state: &mut RulerState, edge_count: usize) {
    // The dial the mode owns: its label, starting value, range, and whether its
    // read-out also reports the edge count the threshold produced.
    let (title, start, max, show_edges) = match state.mode {
        Mode::RectDrag | Mode::ShrinkToFit => {
            ("Snap distance", state.snap_distance, SNAP_DISTANCE_MAX, false)
        }
        Mode::ColorPicker => ("Average", state.color_radius, COLOR_RADIUS_MAX, false),
        _ => ("Sensitivity", state.sensitivity, SENSITIVITY_MAX, true),
    };

    let value = slider_row(ui, title, start, max, |value| {
        if show_edges {
            format!("{}  ·  {edge_count} edges", value.round())
        } else {
            format!("{} px", value.round())
        }
    });

    match state.mode {
        Mode::RectDrag | Mode::ShrinkToFit => state.set_snap_distance(value),
        Mode::ColorPicker => state.set_color_radius(value),
        _ => state.set_sensitivity(value),
    }
}

/// One labelled slider row, returning the value the user left it at.
fn slider_row(
    ui: &mut Ui,
    title: &str,
    value: f32,
    max: f32,
    readout: impl FnOnce(f32) -> String,
) -> f32 {
    let mut value = value;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .color(Color32::WHITE)
                .size(theme::TITLE_SIZE),
        );
        ui.add(
            egui::Slider::new(&mut value, 0.0..=max)
                .show_value(false)
                .trailing_fill(true),
        );
        ui.label(
            egui::RichText::new(readout(value))
                .color(Color32::WHITE)
                .size(theme::VALUE_SIZE),
        );
    });
    value
}

/// One mode button, drawing its icon with the painter rather than shipping assets.
fn mode_button(ui: &mut Ui, mode: Mode, active: bool) -> Response {
    let size = Vec2::splat(theme::MODE_BUTTON_SIZE);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let hovered = response.hovered();
    let fill = if active {
        BG_ACTIVE
    } else if hovered {
        BG_HOVER
    } else {
        BG
    };
    let border = if active { theme::ACCENT } else { BORDER };

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(BUTTON_CORNER), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(BUTTON_CORNER),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let colour = if active { theme::ACCENT } else { Color32::WHITE };
    for shape in mode_icon(mode, rect, colour) {
        painter.add(shape);
    }

    response.on_hover_text(format!("{}  ({})", mode.label(), mode.index() + 1))
}

/// Vector icon for a mode, drawn to fit `rect`.
fn mode_icon(mode: Mode, rect: Rect, colour: Color32) -> Vec<Shape> {
    let stroke = Stroke::new(1.5, colour);
    let c = rect.center();
    let mut shapes = Vec::new();

    match mode {
        Mode::Crosshair => {
            let ray = 9.0;
            let cap = 3.0;
            shapes.push(Shape::line_segment(
                [Pos2::new(c.x - ray, c.y), Pos2::new(c.x + ray, c.y)],
                stroke,
            ));
            shapes.push(Shape::line_segment(
                [Pos2::new(c.x, c.y - ray), Pos2::new(c.x, c.y + ray)],
                stroke,
            ));
            for (a, b) in [
                ((-ray, -cap), (-ray, cap)),
                ((ray, -cap), (ray, cap)),
                ((-cap, -ray), (cap, -ray)),
                ((-cap, ray), (cap, ray)),
            ] {
                shapes.push(Shape::line_segment(
                    [
                        Pos2::new(c.x + a.0, c.y + a.1),
                        Pos2::new(c.x + b.0, c.y + b.1),
                    ],
                    stroke,
                ));
            }
        }
        Mode::RectDrag => {
            shapes.extend(dashed_rect(rect.shrink(6.0), stroke));
        }
        Mode::Container => {
            // Nested rectangles: an inner container inside its parent.
            shapes.push(outline(rect.shrink(5.0), stroke));
            shapes.push(outline(rect.shrink(9.0), stroke));
        }
        Mode::ShrinkToFit => {
            shapes.push(outline(rect.shrink(5.0), stroke));
            shapes.push(outline(rect.shrink(10.0), stroke));
            // Arrows pointing inward on each side.
            for (tip, wing_a, wing_b) in [
                ((c.x, rect.top() + 6.0), (c.x - 2.0, 3.0), (c.x + 2.0, 3.0)),
                (
                    (c.x, rect.bottom() - 6.0),
                    (c.x - 2.0, -3.0),
                    (c.x + 2.0, -3.0),
                ),
            ] {
                let tip = Pos2::new(tip.0, tip.1);
                shapes.push(Shape::line_segment(
                    [tip, Pos2::new(wing_a.0, tip.y + wing_a.1)],
                    stroke,
                ));
                shapes.push(Shape::line_segment(
                    [tip, Pos2::new(wing_b.0, tip.y + wing_b.1)],
                    stroke,
                ));
            }
            for (tip_x, dx) in [(rect.left() + 6.0, 3.0), (rect.right() - 6.0, -3.0)] {
                let tip = Pos2::new(tip_x, c.y);
                shapes.push(Shape::line_segment(
                    [tip, Pos2::new(tip.x + dx, c.y - 2.0)],
                    stroke,
                ));
                shapes.push(Shape::line_segment(
                    [tip, Pos2::new(tip.x + dx, c.y + 2.0)],
                    stroke,
                ));
            }
        }
        Mode::ColorPicker => {
            // A pipette on the diagonal, plus a droplet at its tip.
            let tip = Pos2::new(c.x - 7.0, c.y + 7.0);
            let body_start = Pos2::new(c.x - 3.0, c.y + 3.0);
            let body_end = Pos2::new(c.x + 5.0, c.y - 5.0);
            shapes.push(Shape::line_segment([tip, body_start], stroke));
            shapes.push(Shape::line_segment(
                [
                    Pos2::new(body_start.x - 2.0, body_start.y + 2.0),
                    Pos2::new(body_end.x - 2.0, body_end.y + 2.0),
                ],
                stroke,
            ));
            shapes.push(Shape::line_segment(
                [
                    Pos2::new(body_start.x + 2.0, body_start.y - 2.0),
                    Pos2::new(body_end.x + 2.0, body_end.y - 2.0),
                ],
                stroke,
            ));
            shapes.push(Shape::line_segment(
                [
                    Pos2::new(body_end.x - 3.0, body_end.y + 1.0),
                    Pos2::new(body_end.x + 1.0, body_end.y - 3.0),
                ],
                Stroke::new(3.0, colour),
            ));
            shapes.push(Shape::circle_filled(tip, 1.4, colour));
        }
        Mode::Distance => {
            let a = Pos2::new(rect.left() + 8.0, rect.top() + 10.0);
            let b = Pos2::new(rect.right() - 8.0, rect.bottom() - 6.0);
            let faded = Stroke::new(1.0, colour.gamma_multiply(0.65));
            shapes.extend(Shape::dashed_line(
                &[a, Pos2::new(b.x, a.y)],
                faded,
                4.0,
                3.0,
            ));
            shapes.extend(Shape::dashed_line(
                &[Pos2::new(b.x, a.y), b],
                faded,
                4.0,
                3.0,
            ));
            shapes.push(Shape::line_segment([a, b], stroke));
            shapes.push(Shape::circle_filled(a, 2.0, colour));
            shapes.push(Shape::circle_filled(b, 2.0, colour));
        }
    }

    shapes
}

fn outline(rect: Rect, stroke: Stroke) -> Shape {
    Shape::rect_stroke(rect, 0, stroke, StrokeKind::Inside)
}

fn dashed_rect(rect: Rect, stroke: Stroke) -> Vec<Shape> {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    corners
        .windows(2)
        .flat_map(|pair| Shape::dashed_line(&[pair[0], pair[1]], stroke, 3.0, 2.5))
        .collect()
}

/// Draws a transient message chip centred near the top of the monitor.
pub fn message_bubble(ui: &Ui, canvas: Vec2, text: &str) {
    if text.is_empty() {
        return;
    }
    let painter = ui.painter();
    let font = FontId::proportional(theme::VALUE_SIZE);
    let galley = painter.layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let size = overlay::chip_size(galley.size());
    let rect = Rect::from_center_size(Pos2::new(canvas.x / 2.0, 64.0), size);

    painter.rect_filled(rect, theme::corner_radius(), theme::panel_fill());
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}
