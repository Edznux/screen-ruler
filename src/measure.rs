//! Measurement primitives over an edge map.
//!
//! Everything here works in *device* pixels on a single monitor's edge map.
//! Conversion to and from logical desktop coordinates is the caller's job, so
//! these stay pure and testable without a display.

use crate::edges::EdgeMap;

/// The four axis-aligned ray directions cast from the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    North,
    South,
    West,
    East,
}

impl Direction {
    /// Unit step in image coordinates (y grows downward).
    fn step(self) -> (isize, isize) {
        match self {
            Direction::North => (0, -1),
            Direction::South => (0, 1),
            Direction::West => (-1, 0),
            Direction::East => (1, 0),
        }
    }
}

/// Distances from the cursor to the first edge in each direction, in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rays {
    pub north: usize,
    pub south: usize,
    pub west: usize,
    pub east: usize,
}

impl Rays {
    /// Total horizontal span between the west and east edges. Used by tests;
    /// the UI reports spans through `LogicalRays` instead.
    #[allow(dead_code)]
    pub fn width(self) -> usize {
        self.west + self.east
    }

    /// Total vertical span between the north and south edges. Used by tests.
    #[allow(dead_code)]
    pub fn height(self) -> usize {
        self.north + self.south
    }
}

/// Casts a ray from `(x, y)` and returns the number of steps taken before
/// stopping on an edge pixel or leaving the map.
///
/// The starting pixel is never inspected, so a cursor resting on an edge still
/// measures the span around it rather than collapsing to zero.
pub fn trace_ray(edges: &EdgeMap, x: usize, y: usize, direction: Direction) -> usize {
    if edges.is_empty() {
        return 0;
    }
    let (dx, dy) = direction.step();
    let (mut cx, mut cy) = (x as isize, y as isize);
    let (w, h) = (edges.width() as isize, edges.height() as isize);
    let mut distance = 0usize;

    loop {
        let nx = cx + dx;
        let ny = cy + dy;
        if nx < 0 || ny < 0 || nx >= w || ny >= h {
            return distance;
        }
        distance += 1;
        if edges.at(nx as usize, ny as usize) {
            return distance;
        }
        cx = nx;
        cy = ny;
    }
}

/// Casts all four rays from `(x, y)`.
pub fn cast_rays(edges: &EdgeMap, x: usize, y: usize) -> Rays {
    Rays {
        north: trace_ray(edges, x, y, Direction::North),
        south: trace_ray(edges, x, y, Direction::South),
        west: trace_ray(edges, x, y, Direction::West),
        east: trace_ray(edges, x, y, Direction::East),
    }
}

/// Result of pulling a loose point onto nearby edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub x: usize,
    pub y: usize,
    /// True when at least one axis actually moved.
    pub snapped: bool,
}

/// Pulls `(x, y)` onto the nearest edge within `radius` device pixels.
///
/// The axes snap independently: a point beside a vertical rule snaps in x while
/// keeping its y, which is what makes dragging a selection onto a UI border feel
/// predictable. `band` widens the perpendicular search so a near-miss on a
/// slightly diagonal edge still registers; pass the monitor scale factor.
pub fn snap_to_edge(edges: &EdgeMap, x: usize, y: usize, radius: usize, band: usize) -> Snap {
    if edges.is_empty() || radius == 0 {
        return Snap {
            x,
            y,
            snapped: false,
        };
    }

    let max_x = edges.width() - 1;
    let max_y = edges.height() - 1;
    let x = x.min(max_x);
    let y = y.min(max_y);
    let band = band.max(1);

    let row_min = y.saturating_sub(band);
    let row_max = (y + band).min(max_y);
    let col_min = x.saturating_sub(band);
    let col_max = (x + band).min(max_x);

    // Nearest column carrying an edge within the horizontal band.
    let mut best_x: Option<usize> = None;
    for candidate in x.saturating_sub(radius)..=(x + radius).min(max_x) {
        if edges.any_in_column(candidate, row_min, row_max) {
            let closer = best_x.is_none_or(|best| candidate.abs_diff(x) < best.abs_diff(x));
            if closer {
                best_x = Some(candidate);
            }
        }
    }

    // Nearest row carrying an edge within the vertical band.
    let mut best_y: Option<usize> = None;
    for candidate in y.saturating_sub(radius)..=(y + radius).min(max_y) {
        if edges.any_in_row(candidate, col_min, col_max) {
            let closer = best_y.is_none_or(|best| candidate.abs_diff(y) < best.abs_diff(y));
            if closer {
                best_y = Some(candidate);
            }
        }
    }

    Snap {
        x: best_x.unwrap_or(x),
        y: best_y.unwrap_or(y),
        snapped: best_x.is_some() || best_y.is_some(),
    }
}

/// An inclusive rectangle in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

impl PixelRect {
    /// Builds a normalised rect from two corners, clamped to `(max_x, max_y)`.
    pub fn from_corners(
        x0: isize,
        y0: isize,
        x1: isize,
        y1: isize,
        max_x: usize,
        max_y: usize,
    ) -> Self {
        let clamp_x = |v: isize| v.clamp(0, max_x as isize) as usize;
        let clamp_y = |v: isize| v.clamp(0, max_y as isize) as usize;
        Self {
            left: clamp_x(x0.min(x1)),
            top: clamp_y(y0.min(y1)),
            right: clamp_x(x0.max(x1)),
            bottom: clamp_y(y0.max(y1)),
        }
    }

    pub fn width(self) -> usize {
        self.right - self.left
    }

    pub fn height(self) -> usize {
        self.bottom - self.top
    }
}

/// Rectangles smaller than this are left untouched by shrink-to-fit; there is
/// no meaningful content to tighten onto.
const MIN_SHRINK_SIZE: usize = 5;

/// Tightens `rect` inward until each side rests on the outermost edge pixel it
/// contains, discarding surrounding whitespace.
///
/// Returns `rect` unchanged when it is too small or contains no edges at all.
pub fn shrink_to_content(edges: &EdgeMap, rect: PixelRect) -> PixelRect {
    if edges.is_empty() || rect.width() < MIN_SHRINK_SIZE || rect.height() < MIN_SHRINK_SIZE {
        return rect;
    }

    let top = (rect.top..=rect.bottom)
        .find(|row| edges.any_in_row(*row, rect.left, rect.right))
        .unwrap_or(rect.top);
    let bottom = (top..=rect.bottom)
        .rev()
        .find(|row| edges.any_in_row(*row, rect.left, rect.right))
        .unwrap_or(rect.bottom);
    let left = (rect.left..=rect.right)
        .find(|col| edges.any_in_column(*col, top, bottom))
        .unwrap_or(rect.left);
    let right = (left..=rect.right)
        .rev()
        .find(|col| edges.any_in_column(*col, top, bottom))
        .unwrap_or(rect.right);

    PixelRect {
        left,
        top,
        right,
        bottom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges_from(rows: &[&str]) -> EdgeMap {
        let h = rows.len();
        let w = rows[0].len();
        let mut data = Vec::with_capacity(w * h);
        for row in rows {
            assert_eq!(row.len(), w, "ragged test fixture");
            data.extend(row.chars().map(|c| c == '#'));
        }
        EdgeMap::new(w, h, data).expect("valid map")
    }

    #[test]
    fn ray_stops_on_the_first_edge() {
        let edges = edges_from(&[
            "..........",
            "..........",
            "..#....#..",
            "..........",
            "..........",
        ]);
        assert_eq!(trace_ray(&edges, 5, 2, Direction::West), 3);
        assert_eq!(trace_ray(&edges, 5, 2, Direction::East), 2);
    }

    #[test]
    fn ray_runs_to_the_boundary_when_unobstructed() {
        let edges = edges_from(&["....", "....", "....", "...."]);
        assert_eq!(trace_ray(&edges, 1, 1, Direction::West), 1);
        assert_eq!(trace_ray(&edges, 1, 1, Direction::East), 2);
        assert_eq!(trace_ray(&edges, 1, 1, Direction::North), 1);
        assert_eq!(trace_ray(&edges, 1, 1, Direction::South), 2);
    }

    #[test]
    fn ray_ignores_an_edge_under_the_cursor_itself() {
        // Standing on an edge must still measure the surrounding span.
        let edges = edges_from(&["#..#"]);
        assert_eq!(trace_ray(&edges, 0, 0, Direction::East), 3);
    }

    #[test]
    fn casting_all_rays_reports_span_totals() {
        let edges = edges_from(&[
            "..#....",
            ".......",
            "#.....#",
            ".......",
            "..#....",
        ]);
        let rays = cast_rays(&edges, 2, 2);
        assert_eq!(rays.north, 2);
        assert_eq!(rays.south, 2);
        assert_eq!(rays.west, 2);
        assert_eq!(rays.east, 4);
        assert_eq!(rays.width(), 6);
        assert_eq!(rays.height(), 4);
    }

    #[test]
    fn snapping_pulls_each_axis_independently() {
        // A vertical rule at x == 4 and nothing horizontal nearby.
        let edges = edges_from(&[
            "....#.....",
            "....#.....",
            "....#.....",
            "....#.....",
            "....#.....",
        ]);
        let snap = snap_to_edge(&edges, 6, 2, 5, 1);
        assert!(snap.snapped);
        assert_eq!(snap.x, 4, "x should snap onto the rule");
        assert_eq!(snap.y, 2, "y has no edge to snap to and must stay put");
    }

    #[test]
    fn snapping_prefers_the_nearest_edge() {
        let edges = edges_from(&["#....#...."]);
        let snap = snap_to_edge(&edges, 4, 0, 5, 1);
        assert_eq!(snap.x, 5);
    }

    #[test]
    fn snapping_is_a_no_op_outside_the_radius() {
        let edges = edges_from(&["#........."]);
        let snap = snap_to_edge(&edges, 8, 0, 3, 1);
        assert!(!snap.snapped);
        assert_eq!((snap.x, snap.y), (8, 0));

        // A zero radius disables snapping entirely.
        let disabled = snap_to_edge(&edges, 1, 0, 0, 1);
        assert!(!disabled.snapped);
        assert_eq!(disabled.x, 1);
    }

    #[test]
    fn shrink_tightens_onto_content() {
        let edges = edges_from(&[
            "..........",
            "..........",
            "...####...",
            "...#..#...",
            "...####...",
            "..........",
            "..........",
        ]);
        let rect = PixelRect {
            left: 0,
            top: 0,
            right: 9,
            bottom: 6,
        };
        let shrunk = shrink_to_content(&edges, rect);
        assert_eq!(
            shrunk,
            PixelRect {
                left: 3,
                top: 2,
                right: 6,
                bottom: 4
            }
        );
    }

    #[test]
    fn shrink_leaves_empty_or_tiny_rects_alone() {
        let blank = edges_from(&[
            "..........",
            "..........",
            "..........",
            "..........",
            "..........",
            "..........",
        ]);
        let rect = PixelRect {
            left: 0,
            top: 0,
            right: 9,
            bottom: 5,
        };
        assert_eq!(shrink_to_content(&blank, rect), rect);

        let tiny = PixelRect {
            left: 0,
            top: 0,
            right: 3,
            bottom: 3,
        };
        assert_eq!(shrink_to_content(&blank, tiny), tiny);
    }

    #[test]
    fn corner_construction_normalises_and_clamps() {
        let rect = PixelRect::from_corners(9, 8, 2, 1, 5, 5);
        assert_eq!(
            rect,
            PixelRect {
                left: 2,
                top: 1,
                right: 5,
                bottom: 5
            }
        );

        let negative = PixelRect::from_corners(-10, -10, 3, 3, 9, 9);
        assert_eq!(
            negative,
            PixelRect {
                left: 0,
                top: 0,
                right: 3,
                bottom: 3
            }
        );
    }
}
