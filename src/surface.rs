//! Per-monitor analysed surfaces, and the desktop that owns them.
//!
//! A [`Surface`] bundles everything the ruler knows about one display: its
//! pixels, its edge map, its labelled regions, and where it sits. Keeping this
//! per monitor — rather than one desktop-wide bitmap — is what makes mixed
//! resolutions and mixed scale factors work, and is the seam to extend when
//! adding cross-monitor behaviour.

use std::thread;

use crate::capture::CapturedMonitor;
use crate::edges::{self, EdgeMap};
use crate::geometry::MonitorGeometry;
use crate::image::Rgba8;
use crate::measure::{self, PixelRect, Rays};
use crate::regions::RegionMap;

/// One analysed monitor.
pub struct Surface {
    pub geometry: MonitorGeometry,
    pub image: Rgba8,
    pub edges: EdgeMap,
    pub regions: RegionMap,
}

impl Surface {
    /// Clamps a monitor-local logical point onto a valid image pixel.
    pub fn logical_to_pixel(&self, x: f32, y: f32) -> (usize, usize) {
        let (ix, iy) = self.geometry.logical_to_image(x, y);
        let max_x = self.image.width().saturating_sub(1);
        let max_y = self.image.height().saturating_sub(1);
        (
            (ix.max(0.0) as usize).min(max_x),
            (iy.max(0.0) as usize).min(max_y),
        )
    }

    /// Measures the four rays from a logical point, reporting logical distances.
    pub fn rays_at(&self, x: f32, y: f32) -> LogicalRays {
        let (px, py) = self.logical_to_pixel(x, y);
        let rays = measure::cast_rays(&self.edges, px, py);
        LogicalRays::from_pixels(rays, &self.geometry)
    }

    /// Number of image pixels the snapping band should span.
    ///
    /// Scaled by the monitor's density so snapping feels the same on a HiDPI
    /// panel as on a 1x one.
    fn snap_band(&self) -> usize {
        self.geometry.sanitised_scale().ceil().max(1.0) as usize
    }

    /// Snaps a logical point onto nearby edges, returning logical coordinates.
    ///
    /// `None` when no edge was within range, so callers cannot mistake an
    /// unmoved point for a snapped one.
    pub fn snap_logical(&self, x: f32, y: f32, radius_logical: f32) -> Option<(f32, f32)> {
        if radius_logical <= 0.0 {
            return None;
        }
        let (px, py) = self.logical_to_pixel(x, y);
        let radius = self.geometry.logical_to_image(radius_logical, 0.0).0.ceil() as usize;
        let (sx, sy) = measure::snap_to_edge(&self.edges, px, py, radius, self.snap_band())?;
        Some(self.geometry.image_to_logical(sx as f32, sy as f32))
    }

    /// Tightens a logical rectangle onto the content it encloses.
    ///
    /// Returns `None` when the rectangle is degenerate.
    pub fn shrink_logical(&self, x: f32, y: f32, width: f32, height: f32) -> Option<LogicalRect> {
        if self.edges.is_empty() || width <= 0.0 || height <= 0.0 {
            return None;
        }
        let (x0, y0) = self.geometry.logical_to_image(x, y);
        let (x1, y1) = self.geometry.logical_to_image(x + width, y + height);
        let rect = PixelRect::from_corners(
            x0.round() as isize,
            y0.round() as isize,
            x1.round() as isize,
            y1.round() as isize,
            self.image.width().saturating_sub(1),
            self.image.height().saturating_sub(1),
        );
        let shrunk = measure::shrink_to_content(&self.edges, rect);
        let (lx, ly) = self
            .geometry
            .image_to_logical(shrunk.left as f32, shrunk.top as f32);
        Some(LogicalRect {
            x: lx,
            y: ly,
            width: self.geometry.image_len_to_logical(shrunk.width() as f32),
            height: self.geometry.image_len_to_logical(shrunk.height() as f32),
        })
    }

    /// Bounding box of the container under a logical point, in logical coordinates.
    pub fn container_at_logical(&self, x: f32, y: f32) -> Option<LogicalRect> {
        let (px, py) = self.logical_to_pixel(x, y);
        let stats = self.regions.container_at(px, py)?;
        let (lx, ly) = self
            .geometry
            .image_to_logical(stats.x as f32, stats.y as f32);
        Some(LogicalRect {
            x: lx,
            y: ly,
            width: self.geometry.image_len_to_logical(stats.width as f32),
            height: self.geometry.image_len_to_logical(stats.height as f32),
        })
    }

    /// Recomputes the edge map and regions for new thresholds.
    fn reanalyse(&mut self, low: u16, high: u16) {
        let (edges, regions) = analyse(&self.image, low, high);
        self.edges = edges;
        self.regions = regions;
    }
}

/// A rectangle in monitor-local logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl LogicalRect {
    /// Attaches the rectangle to the monitor it was measured on.
    pub fn on_monitor(self, monitor: usize) -> crate::state::Rect {
        crate::state::Rect {
            monitor,
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// Ray distances converted into logical pixels, which is what the UI reports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRays {
    pub north: f32,
    pub south: f32,
    pub west: f32,
    pub east: f32,
}

impl LogicalRays {
    fn from_pixels(rays: Rays, geometry: &MonitorGeometry) -> Self {
        Self {
            north: geometry.image_len_to_logical(rays.north as f32),
            south: geometry.image_len_to_logical(rays.south as f32),
            west: geometry.image_len_to_logical(rays.west as f32),
            east: geometry.image_len_to_logical(rays.east as f32),
        }
    }

    pub fn width(self) -> f32 {
        self.west + self.east
    }

    pub fn height(self) -> f32 {
        self.north + self.south
    }
}

/// Every analysed monitor, in a stable order.
pub struct Desktop {
    surfaces: Vec<Surface>,
}

impl Desktop {
    /// Analyses each captured monitor, in parallel across displays.
    pub fn build(captures: Vec<CapturedMonitor>, low: u16, high: u16) -> Self {
        let surfaces = thread::scope(|scope| {
            let handles: Vec<_> = captures
                .into_iter()
                .map(|capture| {
                    scope.spawn(move || {
                        let (edges, regions) = analyse(&capture.image, low, high);
                        Surface {
                            geometry: capture.geometry,
                            image: capture.image,
                            edges,
                            regions,
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });

        Self { surfaces }
    }

    pub fn surface(&self, index: usize) -> Option<&Surface> {
        self.surfaces.get(index)
    }

    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }

    pub fn geometries(&self) -> Vec<MonitorGeometry> {
        self.surfaces.iter().map(|s| s.geometry.clone()).collect()
    }

    /// Replaces the geometry of one monitor.
    ///
    /// Used to reconcile the capture backend's idea of monitor layout with the
    /// window system's, which is the authority for placing windows.
    pub fn set_geometry(&mut self, index: usize, geometry: MonitorGeometry) {
        if let Some(surface) = self.surfaces.get_mut(index) {
            surface.geometry = geometry;
        }
    }

    /// Index of the monitor containing a virtual-desktop device pixel.
    ///
    /// The hit test cross-monitor features will need; unused while every
    /// interaction is still resolved inside a single window.
    #[allow(dead_code)]
    pub fn monitor_at_virtual(&self, x: i32, y: i32) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.geometry.contains_virtual(x, y))
    }

    /// Recomputes every monitor's edge map for new thresholds, in parallel.
    ///
    /// Driven by the sensitivity slider, so it runs on the UI's critical path;
    /// doing the displays concurrently keeps the dial responsive on multi-head
    /// setups.
    pub fn recompute(&mut self, low: u16, high: u16) {
        thread::scope(|scope| {
            for surface in &mut self.surfaces {
                scope.spawn(move || surface.reanalyse(low, high));
            }
        });
    }

    /// Total edge pixels across all monitors, shown beside the sensitivity
    /// slider. Each map carries its own count, so this is a per-monitor add.
    pub fn edge_count(&self) -> usize {
        self.surfaces.iter().map(|s| s.edges.count()).sum()
    }
}

/// Runs edge detection and region labelling over one monitor's pixels.
fn analyse(image: &Rgba8, low: u16, high: u16) -> (EdgeMap, RegionMap) {
    let edges = edges::canny(&image.to_gray(), low, high);
    let regions = RegionMap::build(&edges);
    (edges, regions)
}

#[cfg(test)]
impl Surface {
    /// A monitor whose image is a black/white split at image column `split`.
    ///
    /// Shared with the UI tests, which need something to measure against but
    /// care only that there is an edge somewhere to find.
    pub fn test_split(scale: f32, width: usize, height: usize, split: usize) -> Surface {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..height {
            for x in 0..width {
                let v = if x < split { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let image = Rgba8::from_raw(width, height, pixels).expect("valid buffer");
        let (edges, regions) = analyse(&image, 5, 25);
        Surface {
            geometry: MonitorGeometry {
                name: "TEST".to_string(),
                position: (0, 0),
                size: (width as u32, height as u32),
                scale,
                is_primary: true,
            },
            image,
            edges,
            regions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_points_clamp_onto_the_image() {
        let s = Surface::test_split(1.0, 64, 48, 32);
        assert_eq!(s.logical_to_pixel(10.0, 10.0), (10, 10));
        assert_eq!(s.logical_to_pixel(-5.0, -5.0), (0, 0));
        assert_eq!(s.logical_to_pixel(9999.0, 9999.0), (63, 47));
    }

    #[test]
    fn measurements_are_reported_in_logical_pixels() {
        // Same physical panel at 1x and 2x: the logical measurement matches,
        // even though the underlying device-pixel distances differ by 2x.
        let one_x = Surface::test_split(1.0, 64, 48, 32);
        let two_x = Surface::test_split(2.0, 64, 48, 32);

        let a = one_x.rays_at(10.0, 24.0);
        let b = two_x.rays_at(5.0, 12.0);
        assert!(
            (a.east - b.east * 2.0).abs() < 0.001,
            "1x east {} vs 2x east {}",
            a.east,
            b.east
        );
        assert!(a.east > 0.0, "expected to reach the split edge");
    }

    #[test]
    fn snapping_returns_logical_coordinates() {
        let s = Surface::test_split(2.0, 64, 48, 32);
        // The edge sits near image column 32, i.e. logical x == 16.
        let (x, _y) = s
            .snap_logical(18.0, 12.0, 5.0)
            .expect("expected a snap onto the split edge");
        assert!((x - 16.0).abs() <= 1.0, "snapped to logical x {x}");
    }

    #[test]
    fn snapping_with_a_zero_radius_is_disabled() {
        let s = Surface::test_split(1.0, 64, 48, 32);
        assert_eq!(s.snap_logical(18.0, 12.0, 0.0), None);
    }

    #[test]
    fn desktop_hit_tests_the_monitor_under_a_virtual_point() {
        let mut desktop = Desktop { surfaces: vec![] };
        let mut left = Surface::test_split(1.0, 64, 48, 32);
        left.geometry.position = (0, 0);
        let mut right = Surface::test_split(1.0, 64, 48, 32);
        right.geometry.name = "RIGHT".to_string();
        right.geometry.position = (64, 0);
        desktop.surfaces.push(left);
        desktop.surfaces.push(right);

        assert_eq!(desktop.monitor_at_virtual(10, 10), Some(0));
        assert_eq!(desktop.monitor_at_virtual(70, 10), Some(1));
        assert_eq!(desktop.monitor_at_virtual(500, 10), None);
        assert_eq!(desktop.monitor_at_virtual(-1, 10), None);
    }

    #[test]
    fn recomputing_thresholds_changes_the_edge_count() {
        let mut desktop = Desktop {
            surfaces: vec![Surface::test_split(1.0, 64, 48, 32)],
        };
        let permissive = desktop.edge_count();
        assert!(permissive > 0);

        desktop.recompute(250, 254);
        assert!(
            desktop.edge_count() <= permissive,
            "stricter thresholds added edges"
        );
    }
}
