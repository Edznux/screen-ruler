//! The floating controls panel: mode buttons and the mode's active slider.

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::state::{Control, Mode, RulerState, MAX_CONTROLS, MODE_COUNT};
use crate::ui::layout::{self, PanelLayout, SliderRow};
use crate::ui::{overlay, theme};

const BUTTON_CORNER: u8 = 4;
const BORDER: Color32 = Color32::from_rgba_premultiplied(133, 143, 156, 128);
const BG: Color32 = Color32::from_rgba_premultiplied(66, 71, 79, 179);
const BG_HOVER: Color32 = Color32::from_rgba_premultiplied(87, 94, 107, 217);
const BG_ACTIVE: Color32 = Color32::from_rgba_premultiplied(60, 7, 25, 46);

/// Lays out the panel: the mode row, then the active mode's sliders.
///
/// Returns where each widget went, so the layout can be inspected without a
/// display. See [`crate::ui::layout`].
pub fn show(ui: &mut Ui, state: &mut RulerState, edge_count: usize) -> PanelLayout {
    ui.vertical(|ui| {
        let (mode_buttons, session_buttons) = mode_row(ui, state);
        ui.add_space(theme::CONTROLS_COLUMN_SPACING);
        let sliders = active_sliders(ui, state, edge_count);

        PanelLayout {
            mode_buttons,
            sliders,
            session_buttons,
        }
    })
    .inner
}

/// The row of mode buttons, plus the session badge when there is a session.
fn mode_row(
    ui: &mut Ui,
    state: &mut RulerState,
) -> (
    [(Mode, Rect); MODE_COUNT],
    Option<[Rect; layout::SESSION_BUTTONS]>,
) {
    ui.horizontal(|ui| {
        let mut mode_clicked = None;
        let mode_buttons = Mode::ALL.map(|mode| {
            let response = mode_button(ui, mode, state.mode == mode);
            if response.clicked() {
                mode_clicked = Some(mode);
            }
            (mode, response.rect)
        });
        if let Some(mode) = mode_clicked {
            state.set_mode(mode);
        }

        let session_buttons = state.session.then(|| {
            ui.add_space(theme::MODE_ROW_SPACING);
            session_badge(ui, state)
        });

        (mode_buttons, session_buttons)
    })
    .inner
}

/// The `SESSION` badge plus its two export shortcuts.
fn session_badge(ui: &mut Ui, state: &mut RulerState) -> [Rect; layout::SESSION_BUTTONS] {
    let session = ui.add(
        egui::Button::new(
            egui::RichText::new("SESSION")
                .color(Color32::WHITE)
                .size(theme::VALUE_SIZE)
                .strong(),
        )
        .fill(theme::ACCENT)
        .corner_radius(CornerRadius::same(BUTTON_CORNER)),
    );
    let session_rect = session.rect;
    if session.clicked() {
        state.request_destructive(crate::state::Destructive::SessionButton);
    }
    session.on_hover_text("Leave session mode (discards annotations)");

    let markdown = ui.add(small_button("MD"));
    let markdown_rect = markdown.rect;
    if markdown
        .on_hover_text("Copy all annotations as Markdown")
        .clicked()
    {
        state.copy_annotations();
    }

    let image = ui.add(small_button("IMG"));
    let image_rect = image.rect;
    if image
        .on_hover_text("Drag a region to copy it with annotations")
        .clicked()
    {
        state.arm_export();
    }

    [session_rect, markdown_rect, image_rect]
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

/// One slider per dial the current mode declares, in the declared order.
///
/// The list comes from [`Mode::controls`], the same lookup the wheel reads, so
/// what the panel shows and what the wheel moves cannot describe different
/// modes. The wheel drives only the first; the rest are panel-only, which is
/// why they have to be visible here rather than left implicit.
fn active_sliders(
    ui: &mut Ui,
    state: &mut RulerState,
    edge_count: usize,
) -> [Option<SliderRow>; MAX_CONTROLS] {
    // `MAX_CONTROLS` comes from the same table as `controls()`, so the slots
    // cannot run short.
    let mut rows = [None; MAX_CONTROLS];

    for (index, &control) in state.mode.controls().iter().enumerate() {
        if index > 0 {
            ui.add_space(theme::CONTROLS_ROW_SPACING);
        }

        let (value, row) = slider_row(ui, control, state.control_value(control), edge_count);
        state.set_control_value(control, value);
        rows[index] = Some(row);
    }

    rows
}

/// One labelled slider row: the value the user left it at, and where it landed.
fn slider_row(
    ui: &mut Ui,
    control: Control,
    value: f32,
    edge_count: usize,
) -> (f32, SliderRow) {
    let mut value = value;
    let (label, track) = ui
        .horizontal(|ui| {
            // A fixed label column, so a mode's sliders start at the same x
            // however long their titles are.
            let (label, _) = ui.allocate_exact_size(
                Vec2::new(theme::CONTROL_LABEL_WIDTH, theme::MODE_BUTTON_SIZE / 2.0),
                Sense::hover(),
            );
            ui.painter().text(
                label.left_center(),
                Align2::LEFT_CENTER,
                control.title(),
                FontId::proportional(theme::TITLE_SIZE),
                Color32::WHITE,
            );
            let slider = ui.add(
                egui::Slider::new(&mut value, 0.0..=control.max())
                    .show_value(false)
                    .trailing_fill(true),
            );
            let readout = if control.reports_edges() {
                format!("{}  ·  {edge_count} edges", value.round())
            } else {
                format!("{} px", value.round())
            };
            ui.label(
                egui::RichText::new(readout)
                    .color(Color32::WHITE)
                    .size(theme::VALUE_SIZE),
            );
            (label, slider.rect)
        })
        .inner;

    (
        value,
        SliderRow {
            control,
            label,
            track,
        },
    )
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

    response.on_hover_text(format!("{}  ({})", mode.label(), mode.digit()))
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

/// Draws a transient message chip, horizontally centred with its top at `top`.
///
/// The caller places it below the chrome it must not cover, rather than the
/// chip guessing a fixed offset: the panel grows a row in session mode and the
/// help overlay comes and goes, so any constant here would eventually collide
/// with one of them — which is exactly what a hard-coded `y` used to do.
///
/// `warning` marks a confirm-before-discard prompt, which carries the accent
/// border so it reads as a question rather than as passing feedback.
/// A message chip, laid out but not yet drawn.
///
/// Measuring is separate from drawing so the caller can centre the chip in the
/// same frame it appears. Letting the [`egui::Area`] centre it instead means
/// `Area::anchor`, which positions from the size recorded *last* frame — so a
/// chip whose text changes length is drawn in the old position for one visible
/// frame and then snaps sideways, which is exactly what a prompt asking a
/// question must not do.
pub struct MessageChip {
    galley: std::sync::Arc<egui::Galley>,
    /// Including the chip's padding: what the caller must reserve.
    pub size: Vec2,
}

/// Lays out a chip's text so it can be placed before it is painted.
pub fn measure_message(ctx: &egui::Context, text: &str) -> MessageChip {
    let galley = ctx.fonts_mut(|fonts| {
        fonts.layout_no_wrap(
            text.to_string(),
            FontId::proportional(theme::VALUE_SIZE),
            Color32::WHITE,
        )
    });
    let size = overlay::chip_size(galley.size());
    MessageChip { galley, size }
}

/// Draws a pre-measured chip, filling the rectangle its size reserved.
pub fn message_bubble(ui: &mut Ui, chip: MessageChip, warning: bool) {
    let (rect, _) = ui.allocate_exact_size(chip.size, Sense::hover());
    let galley = chip.galley;

    let painter = ui.painter();
    painter.rect_filled(rect, theme::corner_radius(), theme::panel_fill());
    if warning {
        painter.rect_stroke(
            rect,
            theme::corner_radius(),
            theme::accent_stroke(2.0),
            StrokeKind::Inside,
        );
    }
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}
