//! Circular arcs: construction from DXF bulges and angle pairs, evaluation, flattening.

use crate::flatten::{self, FlattenError};
use crate::geom::Point;

/// A circular arc sweeping `sweep` radians from `start_angle` (positive = counter-clockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub center: Point,
    pub radius: f64,
    pub start_angle: f64,
    pub sweep: f64,
}

impl Arc {
    /// Point at parameter `t` in `[0, 1]` along the sweep.
    pub fn point_at(&self, t: f64) -> Point {
        let a = self.start_angle + self.sweep * t;
        Point::new(
            self.center.x + self.radius * a.cos(),
            self.center.y + self.radius * a.sin(),
        )
    }

    pub fn start(&self) -> Point {
        self.point_at(0.0)
    }

    pub fn end(&self) -> Point {
        self.point_at(1.0)
    }

    /// DXF `ARC` semantics: angles in degrees, always counter-clockwise from `start_deg`
    /// to `end_deg`. Equal angles denote a full circle.
    pub fn from_dxf_angles(center: Point, radius: f64, start_deg: f64, end_deg: f64) -> Arc {
        let mut sweep_deg = (end_deg - start_deg).rem_euclid(360.0);
        if sweep_deg == 0.0 {
            sweep_deg = 360.0;
        }
        Arc {
            center,
            radius,
            start_angle: start_deg.to_radians(),
            sweep: sweep_deg.to_radians(),
        }
    }

    /// Arc for a polyline segment `p0 -> p1` with a DXF bulge (`tan(sweep / 4)`; positive is
    /// counter-clockwise). `None` for a zero bulge (straight segment) or a degenerate chord.
    pub fn from_bulge(p0: Point, p1: Point, bulge: f64) -> Option<Arc> {
        if bulge == 0.0 || !bulge.is_finite() {
            return None;
        }
        let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
        let chord = dx.hypot(dy);
        if chord <= 0.0 {
            return None;
        }
        let sweep = 4.0 * bulge.atan();
        let radius = chord * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
        // Signed distance from the chord midpoint to the centre along the chord's left normal.
        let h = chord * (1.0 - bulge * bulge) / (4.0 * bulge);
        let mid = Point::new((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
        let center = Point::new(mid.x - dy / chord * h, mid.y + dx / chord * h);
        let start_angle = (p0.y - center.y).atan2(p0.x - center.x);
        Some(Arc {
            center,
            radius,
            start_angle,
            sweep,
        })
    }

    /// Whether the direction `angle` (radians, any range) lies on the arc's sweep.
    pub fn contains_angle(&self, angle: f64) -> bool {
        let tau = std::f64::consts::TAU;
        let (start, sweep) = if self.sweep >= 0.0 {
            (self.start_angle, self.sweep)
        } else {
            (self.start_angle + self.sweep, -self.sweep)
        };
        let rel = (angle - start).rem_euclid(tau);
        rel <= sweep + 1e-12 || (tau - rel) <= 1e-12
    }

    /// Polyline approximation within `tolerance` (see [`flatten::flatten_arc`]).
    pub fn flatten(&self, tolerance: f64) -> Result<Vec<Point>, FlattenError> {
        flatten::flatten_arc(
            self.center,
            self.radius,
            self.start_angle,
            self.sweep,
            tolerance,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    #[test]
    fn dxf_angles_are_ccw_and_wrap() {
        let a = Arc::from_dxf_angles(Point::new(0.0, 0.0), 1.0, 270.0, 90.0);
        assert!((a.sweep - PI).abs() < 1e-12);
        assert!((a.start().y + 1.0).abs() < 1e-12);
        let full = Arc::from_dxf_angles(Point::new(0.0, 0.0), 1.0, 30.0, 30.0);
        assert!((full.sweep - TAU).abs() < 1e-12);
    }

    #[test]
    fn unit_bulge_is_a_semicircle() {
        let a = Arc::from_bulge(Point::new(0.0, 0.0), Point::new(2.0, 0.0), 1.0).unwrap();
        assert!((a.radius - 1.0).abs() < 1e-12);
        assert!(a.center.distance(Point::new(1.0, 0.0)) < 1e-12);
        assert!((a.sweep - PI).abs() < 1e-12);
        // Counter-clockwise from (0,0) to (2,0) passes below the chord.
        assert!(a.point_at(0.5).y < 0.0);
    }

    #[test]
    fn contains_angle_respects_direction() {
        let ccw = Arc::from_dxf_angles(Point::new(0.0, 0.0), 1.0, 0.0, 90.0);
        assert!(
            ccw.contains_angle(0.5)
                && !ccw.contains_angle(-0.5)
                && ccw.contains_angle(std::f64::consts::FRAC_PI_2)
        );
        let cw = Arc {
            center: Point::new(0.0, 0.0),
            radius: 1.0,
            start_angle: std::f64::consts::FRAC_PI_2,
            sweep: -std::f64::consts::FRAC_PI_2,
        };
        assert!(cw.contains_angle(0.5) && !cw.contains_angle(2.0));
    }

    #[test]
    fn zero_bulge_is_straight() {
        assert_eq!(
            Arc::from_bulge(Point::new(0.0, 0.0), Point::new(1.0, 1.0), 0.0),
            None
        );
    }

    fn bulge() -> impl Strategy<Value = f64> {
        // |bulge| in [1e-3, 3]: tiny bulges make the radius huge and the test numerically moot.
        prop_oneof![1e-3..3.0f64, -3.0..-1e-3f64]
    }

    fn coord() -> impl Strategy<Value = f64> {
        -1e3..1e3f64
    }

    proptest! {
        #[test]
        fn bulge_arc_hits_both_endpoints(x0 in coord(), y0 in coord(), x1 in coord(), y1 in coord(), b in bulge()) {
            let (p0, p1) = (Point::new(x0, y0), Point::new(x1, y1));
            prop_assume!(p0.distance(p1) > 1e-3);
            let a = Arc::from_bulge(p0, p1, b).unwrap();
            let eps = 1e-9 * (a.radius + x0.abs() + y0.abs() + 1.0);
            prop_assert!(a.start().distance(p0) <= eps, "start off by {}", a.start().distance(p0));
            prop_assert!(a.end().distance(p1) <= eps, "end off by {}", a.end().distance(p1));
        }

        #[test]
        fn bulge_defines_sweep_and_sagitta(x0 in coord(), y0 in coord(), x1 in coord(), y1 in coord(), b in bulge()) {
            let (p0, p1) = (Point::new(x0, y0), Point::new(x1, y1));
            prop_assume!(p0.distance(p1) > 1e-3);
            let a = Arc::from_bulge(p0, p1, b).unwrap();
            prop_assert!((a.sweep - 4.0 * b.atan()).abs() < 1e-12);
            // The arc midpoint sits `|bulge| * chord / 2` off the chord midpoint (DXF definition).
            let chord = p0.distance(p1);
            let mid = Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let sagitta = a.point_at(0.5).distance(mid);
            let eps = 1e-9 * (a.radius + x0.abs() + y0.abs() + 1.0);
            prop_assert!((sagitta - b.abs() * chord / 2.0).abs() <= eps);
        }

        #[test]
        fn bulge_sign_is_turn_direction(x0 in coord(), y0 in coord(), x1 in coord(), y1 in coord(), b in bulge()) {
            let (p0, p1) = (Point::new(x0, y0), Point::new(x1, y1));
            prop_assume!(p0.distance(p1) > 1e-3);
            let a = Arc::from_bulge(p0, p1, b).unwrap();
            // Positive bulge: the arc midpoint lies to the right of the directed chord.
            let m = a.point_at(0.5);
            let cross = (x1 - x0) * (m.y - y0) - (y1 - y0) * (m.x - x0);
            prop_assert_eq!(cross < 0.0, b > 0.0);
        }

        #[test]
        fn dxf_sweep_is_in_half_open_turn(start in -720.0..720.0f64, end in -720.0..720.0f64) {
            let a = Arc::from_dxf_angles(Point::new(0.0, 0.0), 1.0, start, end);
            prop_assert!(a.sweep > 0.0 && a.sweep <= TAU + 1e-12);
            prop_assert!(a.sweep >= FRAC_PI_2 || a.sweep < FRAC_PI_2); // finite
        }
    }
}
