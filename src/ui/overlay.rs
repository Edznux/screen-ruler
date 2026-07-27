//! Measurement visuals, built as plain [`egui::Shape`] lists.
//!
//! Nothing here touches a `Ui` or reads input. Shapes are produced in
//! monitor-local *logical* coordinates, which lets the exact same code draw the
//! live overlay and render the composite PNG that Ctrl+Shift+C copies — the two
//! cannot drift apart, because there is only one implementation.

use egui::epaint::text::FontsView;
use egui::{Color32, FontId, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

use crate::state::{
    format_distance, format_size, Annotation, AnnotationKind, DELTA_BREAKDOWN_THRESHOLD,
};
use crate::ui::theme;

/// Gap kept between a floating panel and the screen edge.
const PANEL_MARGIN: f32 = 2.0;
/// Length of the end caps on crosshair rays.
pub const TICK_HALF_LENGTH: f32 = 5.0;

/// Places a floating panel near an anchor, flipping it to the other side when
/// it would overflow the monitor.
///
/// Returns the panel's top-left corner.
pub fn floating_panel_position(
    anchor: Pos2,
    size: Vec2,
    offset: Vec2,
    canvas: Vec2,
    margin: f32,
) -> Pos2 {
    let desired = anchor + offset;

    let x = if desired.x + size.x <= canvas.x - margin {
        desired.x
    } else {
        // Flip to the opposite side of the anchor rather than merely clamping,
        // so the panel never covers the thing being measured.
        (anchor.x - offset.x - size.x).max(margin)
    };
    let y = if desired.y + size.y <= canvas.y - margin {
        desired.y
    } else {
        (anchor.y - offset.y - size.y).max(margin)
    };

    let max_x = (canvas.x - size.x - margin).max(margin);
    let max_y = (canvas.y - size.y - margin).max(margin);
    Pos2::new(x.clamp(margin, max_x), y.clamp(margin, max_y))
}

/// Size of the chip that wraps text of `content` size, including its padding.
pub fn chip_size(content: Vec2) -> Vec2 {
    content + Vec2::new(theme::LABEL_H_PADDING, theme::LABEL_V_PADDING)
}

/// The shadow-and-fill backdrop every floating chip sits on.
fn chip_backdrop(rect: Rect) -> [Shape; 2] {
    [
        Shape::rect_filled(
            rect.translate(Vec2::splat(theme::LABEL_SHADOW_OFFSET)),
            theme::corner_radius(),
            theme::PANEL_SHADOW,
        ),
        Shape::rect_filled(rect, theme::corner_radius(), theme::panel_fill()),
    ]
}

/// The dark rounded measurement chip, with its drop shadow.
pub fn measurement_label(
    fonts: &mut FontsView<'_>,
    anchor: Pos2,
    text: &str,
    offset: Vec2,
    canvas: Vec2,
) -> Vec<Shape> {
    if text.is_empty() {
        return Vec::new();
    }

    let font = FontId::monospace(theme::LABEL_TEXT_SIZE);
    let galley = fonts.layout_no_wrap(text.to_string(), font, theme::TEXT);
    let size = chip_size(galley.size());
    let position = floating_panel_position(anchor, size, offset, canvas, PANEL_MARGIN);
    let rect = Rect::from_min_size(position, size);

    let mut shapes = chip_backdrop(rect).to_vec();
    shapes.push(Shape::galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::TEXT,
    ));
    shapes
}

/// Default offset of a measurement chip from its anchor.
pub fn label_offset() -> Vec2 {
    Vec2::new(theme::BASE_MARGIN, theme::LABEL_OFFSET_Y)
}

/// The four rays cast from the cursor, with end caps.
pub fn crosshair(cursor: Pos2, north: f32, south: f32, west: f32, east: f32) -> Vec<Shape> {
    let stroke = theme::accent_stroke(1.0);
    let tick = TICK_HALF_LENGTH;
    vec![
        Shape::line_segment(
            [Pos2::new(cursor.x, north), Pos2::new(cursor.x, south)],
            stroke,
        ),
        Shape::line_segment(
            [Pos2::new(west, cursor.y), Pos2::new(east, cursor.y)],
            stroke,
        ),
        // End caps mark exactly which pixel each ray stopped on.
        Shape::line_segment(
            [
                Pos2::new(cursor.x - tick, north),
                Pos2::new(cursor.x + tick, north),
            ],
            stroke,
        ),
        Shape::line_segment(
            [
                Pos2::new(cursor.x - tick, south),
                Pos2::new(cursor.x + tick, south),
            ],
            stroke,
        ),
        Shape::line_segment(
            [
                Pos2::new(west, cursor.y - tick),
                Pos2::new(west, cursor.y + tick),
            ],
            stroke,
        ),
        Shape::line_segment(
            [
                Pos2::new(east, cursor.y - tick),
                Pos2::new(east, cursor.y + tick),
            ],
            stroke,
        ),
    ]
}

/// A hairline selection rectangle.
pub fn selection_rect(rect: Rect) -> Shape {
    Shape::rect_stroke(
        rect,
        theme::corner_radius(),
        theme::accent_stroke(1.0),
        StrokeKind::Inside,
    )
}

/// The eyedropper marker: a sampling disc plus crosshair arms and a centre dot.
pub fn color_marker(at: Pos2, radius: f32) -> Vec<Shape> {
    let mut shapes = Vec::new();
    if radius > 0.0 {
        // Show the actual area being averaged, so the reading is explainable.
        shapes.push(Shape::circle_filled(
            at,
            radius,
            theme::ACCENT.gamma_multiply(0.22),
        ));
        shapes.push(Shape::circle_stroke(at, radius, theme::accent_stroke(1.0)));
    } else {
        // Single-pixel sampling: arms with a gap, so the marker never covers
        // the one pixel it is reading.
        let arm = 5.0;
        let gap = 2.0;
        let stroke = theme::accent_stroke(1.0);
        for (from, to) in [
            (Vec2::new(-arm, 0.0), Vec2::new(-gap, 0.0)),
            (Vec2::new(gap, 0.0), Vec2::new(arm, 0.0)),
            (Vec2::new(0.0, -arm), Vec2::new(0.0, -gap)),
            (Vec2::new(0.0, gap), Vec2::new(0.0, arm)),
        ] {
            shapes.push(Shape::line_segment([at + from, at + to], stroke));
        }
    }
    shapes.push(Shape::rect_filled(
        Rect::from_center_size(at, Vec2::splat(3.0)),
        0,
        theme::ACCENT,
    ));
    shapes
}

/// The eyedropper readout: a swatch beside hex/rgb/hsl lines.
pub fn color_bubble(
    fonts: &mut FontsView<'_>,
    anchor: Pos2,
    lines: [String; 3],
    swatch: Color32,
    canvas: Vec2,
) -> Vec<Shape> {
    let font = FontId::monospace(theme::LABEL_TEXT_SIZE);
    let galleys: Vec<_> = lines
        .iter()
        .map(|line| fonts.layout_no_wrap(line.clone(), font.clone(), theme::TEXT))
        .collect();

    let text_width = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
    let line_height = galleys.first().map_or(0.0, |g| g.size().y);
    let text_height = line_height * galleys.len() as f32 + (galleys.len() as f32 - 1.0);

    let swatch_size = 16.0;
    let row_spacing = 10.0;
    let row_width = swatch_size + row_spacing + text_width;
    let row_height = text_height.max(swatch_size);
    let size = chip_size(Vec2::new(row_width, row_height));

    let offset = Vec2::new(theme::BASE_MARGIN + 8.0, theme::LABEL_OFFSET_Y + 6.0);
    let position = floating_panel_position(anchor, size, offset, canvas, PANEL_MARGIN);
    let rect = Rect::from_min_size(position, size);

    let mut shapes = chip_backdrop(rect).to_vec();

    let row_left = rect.left() + (size.x - row_width) / 2.0;
    let row_top = rect.top() + (size.y - row_height) / 2.0;
    let swatch_rect = Rect::from_min_size(
        Pos2::new(row_left, row_top + (row_height - swatch_size) / 2.0),
        Vec2::splat(swatch_size),
    );
    shapes.push(Shape::rect_filled(swatch_rect, 0, swatch));
    shapes.push(Shape::rect_stroke(
        swatch_rect,
        0,
        Stroke::new(1.0, theme::TEXT),
        StrokeKind::Inside,
    ));

    let text_left = row_left + swatch_size + row_spacing;
    let text_top = row_top + (row_height - text_height) / 2.0;
    for (index, galley) in galleys.into_iter().enumerate() {
        shapes.push(Shape::galley(
            Pos2::new(text_left, text_top + index as f32 * (line_height + 1.0)),
            galley,
            theme::TEXT,
        ));
    }

    shapes
}

/// The point-to-point measurement: the line, its endpoints, an optional dashed
/// right-angle triangle, and the distance chips.
pub fn distance(
    fonts: &mut FontsView<'_>,
    from: Pos2,
    to: Pos2,
    canvas: Vec2,
) -> Vec<Shape> {
    let mut shapes = vec![Shape::line_segment([from, to], theme::accent_stroke(2.0))];
    shapes.push(Shape::circle_filled(from, 2.5, theme::ACCENT));
    shapes.push(Shape::circle_filled(to, 2.5, theme::ACCENT));

    let delta = to - from;

    // The dx/dy breakdown only helps when the line is meaningfully diagonal.
    if delta.x.abs().min(delta.y.abs()) > DELTA_BREAKDOWN_THRESHOLD {
        let corner = Pos2::new(to.x, from.y);
        let dashed = Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7));
        shapes.extend(Shape::dashed_line(&[from, corner], dashed, 4.0, 4.0));
        shapes.extend(Shape::dashed_line(&[corner, to], dashed, 4.0, 4.0));

        shapes.extend(measurement_label(
            fonts,
            Pos2::new((from.x + to.x) / 2.0, from.y),
            &format_distance(delta.x.abs()),
            Vec2::new(0.0, if delta.y >= 0.0 { 8.0 } else { -28.0 }),
            canvas,
        ));
        shapes.extend(measurement_label(
            fonts,
            Pos2::new(to.x, (from.y + to.y) / 2.0),
            &format_distance(delta.y.abs()),
            Vec2::new(
                if delta.x >= 0.0 { 10.0 } else { -10.0 },
                if delta.y >= 0.0 { 0.0 } else { -20.0 },
            ),
            canvas,
        ));
    }

    // The total sits perpendicular to the line so it never lies on top of it.
    let length = delta.length().max(1e-6);
    shapes.extend(measurement_label(
        fonts,
        from + delta / 2.0,
        &format_distance(delta.length()),
        Vec2::new(-delta.y / length, delta.x / length) * 14.0,
        canvas,
    ));

    shapes
}

/// Renders one placed annotation exactly as it appears live.
pub fn annotation(fonts: &mut FontsView<'_>, annotation: &Annotation, canvas: Vec2) -> Vec<Shape> {
    let at = Pos2::new(annotation.at.x, annotation.at.y);
    let mut shapes = Vec::new();

    match &annotation.kind {
        AnnotationKind::Crosshair {
            north,
            south,
            west,
            east,
        } => {
            shapes.extend(crosshair(at, *north, *south, *west, *east));
            shapes.extend(measurement_label(
                fonts,
                at,
                &format_size(east - west, south - north),
                label_offset(),
                canvas,
            ));
        }
        AnnotationKind::Rect { width, height } => {
            shapes.push(selection_rect(Rect::from_min_size(
                at,
                Vec2::new(width.max(1.0), height.max(1.0)),
            )));
            shapes.extend(measurement_label(
                fonts,
                at,
                &format_size(*width, *height),
                label_offset(),
                canvas,
            ));
        }
        AnnotationKind::Color { sample, radius } => {
            shapes.extend(color_marker(at, *radius));
            shapes.extend(color_bubble(
                fonts,
                at,
                sample.notations(),
                Color32::from_rgb(sample.r, sample.g, sample.b),
                canvas,
            ));
        }
        AnnotationKind::Distance { to_x, to_y } => {
            shapes.extend(distance(fonts, at, Pos2::new(*to_x, *to_y), canvas));
        }
    }

    shapes
}

/// Renders every annotation belonging to one monitor.
pub fn annotations_for_monitor(
    fonts: &mut FontsView<'_>,
    annotations: &[Annotation],
    monitor: usize,
    canvas: Vec2,
) -> Vec<Shape> {
    annotations
        .iter()
        .filter(|a| a.at.monitor == monitor)
        .flat_map(|a| annotation(fonts, a, canvas))
        .collect()
}

/// Opacity for the session-mode border, fading out as the cursor approaches it.
///
/// Without this the border would sit on top of whatever is being measured near
/// a screen edge.
pub fn session_border_opacity(cursor: Option<Pos2>, canvas: Vec2) -> f32 {
    let Some(cursor) = cursor else {
        return theme::SESSION_BORDER_OPACITY;
    };
    let distance = cursor
        .x
        .min(cursor.y)
        .min(canvas.x - cursor.x)
        .min(canvas.y - cursor.y)
        .max(0.0);

    if distance >= theme::SESSION_BORDER_FADE_DISTANCE {
        return theme::SESSION_BORDER_OPACITY;
    }
    let t = distance / theme::SESSION_BORDER_FADE_DISTANCE;
    theme::SESSION_BORDER_NEAR_OPACITY
        + (theme::SESSION_BORDER_OPACITY - theme::SESSION_BORDER_NEAR_OPACITY) * t
}

/// The four edge bars that frame a monitor while a session is active.
pub fn session_border(canvas: Vec2, opacity: f32) -> Vec<Shape> {
    let thickness = theme::SESSION_BORDER_THICKNESS;
    let color = theme::ACCENT.gamma_multiply(opacity);
    let horizontal = Vec2::new(canvas.x, thickness);
    let vertical = Vec2::new(thickness, canvas.y);

    [
        (Pos2::ZERO, horizontal),
        (Pos2::new(0.0, canvas.y - thickness), horizontal),
        (Pos2::ZERO, vertical),
        (Pos2::new(canvas.x - thickness, 0.0), vertical),
    ]
    .into_iter()
    .map(|(corner, size)| Shape::rect_filled(Rect::from_min_size(corner, size), 0, color))
    .collect()
}

/// Marker drawn at a snapped pointer position, so a snap is visible.
pub fn snapped_marker(at: Pos2) -> Vec<Shape> {
    let stroke = theme::accent_stroke(1.0);
    let arm = 6.0;
    vec![
        Shape::line_segment(
            [Pos2::new(at.x - arm, at.y), Pos2::new(at.x + arm, at.y)],
            stroke,
        ),
        Shape::line_segment(
            [Pos2::new(at.x, at.y - arm), Pos2::new(at.x, at.y + arm)],
            stroke,
        ),
        Shape::circle_stroke(at, 3.0, stroke),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: Vec2 = Vec2::new(1000.0, 800.0);

    #[test]
    fn a_panel_with_room_sits_at_its_offset() {
        let position = floating_panel_position(
            Pos2::new(100.0, 100.0),
            Vec2::new(80.0, 30.0),
            Vec2::new(14.0, 4.0),
            CANVAS,
            2.0,
        );
        assert_eq!(position, Pos2::new(114.0, 104.0));
    }

    #[test]
    fn a_panel_flips_instead_of_overflowing_the_right_edge() {
        let anchor = Pos2::new(980.0, 100.0);
        let size = Vec2::new(80.0, 30.0);
        let position = floating_panel_position(anchor, size, Vec2::new(14.0, 4.0), CANVAS, 2.0);

        assert!(
            position.x + size.x <= CANVAS.x,
            "panel overflowed: {position:?}"
        );
        // Flipped to the far side of the anchor rather than merely clamped.
        assert!(position.x < anchor.x, "expected a flip, got {position:?}");
    }

    #[test]
    fn a_panel_flips_instead_of_overflowing_the_bottom_edge() {
        let anchor = Pos2::new(100.0, 790.0);
        let size = Vec2::new(80.0, 30.0);
        let position = floating_panel_position(anchor, size, Vec2::new(14.0, 4.0), CANVAS, 2.0);
        assert!(position.y + size.y <= CANVAS.y, "{position:?}");
        assert!(position.y < anchor.y, "{position:?}");
    }

    #[test]
    fn a_panel_larger_than_the_canvas_still_lands_on_screen() {
        let position = floating_panel_position(
            Pos2::new(500.0, 400.0),
            Vec2::new(4000.0, 3000.0),
            Vec2::new(14.0, 4.0),
            CANVAS,
            2.0,
        );
        assert_eq!(position, Pos2::new(2.0, 2.0), "should pin to the margin");
    }

    #[test]
    fn crosshair_draws_two_rays_and_four_caps() {
        let shapes = crosshair(Pos2::new(50.0, 50.0), 10.0, 90.0, 20.0, 80.0);
        assert_eq!(shapes.len(), 6);
    }

    #[test]
    fn the_session_border_fades_as_the_cursor_nears_an_edge() {
        let centre = session_border_opacity(Some(Pos2::new(500.0, 400.0)), CANVAS);
        assert_eq!(centre, theme::SESSION_BORDER_OPACITY);

        let corner = session_border_opacity(Some(Pos2::new(0.0, 0.0)), CANVAS);
        assert_eq!(corner, theme::SESSION_BORDER_NEAR_OPACITY);

        let near = session_border_opacity(Some(Pos2::new(50.0, 400.0)), CANVAS);
        assert!(
            near > theme::SESSION_BORDER_NEAR_OPACITY && near < theme::SESSION_BORDER_OPACITY,
            "expected a partial fade, got {near}"
        );
    }

    #[test]
    fn the_session_border_is_fully_visible_without_a_cursor() {
        assert_eq!(
            session_border_opacity(None, CANVAS),
            theme::SESSION_BORDER_OPACITY
        );
    }

    #[test]
    fn the_session_border_covers_all_four_edges() {
        assert_eq!(session_border(CANVAS, 1.0).len(), 4);
    }

    #[test]
    fn a_zero_radius_colour_marker_leaves_the_sampled_pixel_uncovered() {
        // Four gapped arms plus the centre dot, and no filled disc.
        assert_eq!(color_marker(Pos2::new(10.0, 10.0), 0.0).len(), 5);
        // With a radius there is a disc, its outline, and the centre dot.
        assert_eq!(color_marker(Pos2::new(10.0, 10.0), 6.0).len(), 3);
    }
}
