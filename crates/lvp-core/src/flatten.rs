//! Bounded-error curve flattening.
//!
//! Policy (see docs/engineering/decisions.md): curves become polylines whose chordal
//! deviation (sagitta) never exceeds the caller's tolerance, and no single segment spans
//! more than a quarter turn regardless of tolerance. Vertices lie exactly on the curve.

use std::f64::consts::FRAC_PI_2;

use crate::geom::Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlattenError {
    NonFiniteInput,
    NonPositiveRadius,
    NonPositiveTolerance,
}

impl std::fmt::Display for FlattenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlattenError::NonFiniteInput => write!(f, "arc has a non-finite parameter"),
            FlattenError::NonPositiveRadius => write!(f, "arc radius must be > 0"),
            FlattenError::NonPositiveTolerance => write!(f, "flattening tolerance must be > 0"),
        }
    }
}

impl std::error::Error for FlattenError {}

/// Largest angular step (radians) whose sagitta on a circle of `radius` stays within
/// `tolerance`, capped at a quarter turn.
pub fn max_arc_step(radius: f64, tolerance: f64) -> f64 {
    // sagitta = r (1 - cos(step / 2))  =>  step = 2 acos(1 - tol / r)
    let c = (1.0 - tolerance / radius).clamp(-1.0, 1.0);
    (2.0 * c.acos()).min(FRAC_PI_2)
}

/// Number of equal segments needed to flatten an arc of `sweep` radians.
pub fn arc_segment_count(radius: f64, sweep: f64, tolerance: f64) -> usize {
    let step = max_arc_step(radius, tolerance);
    ((sweep.abs() / step).ceil() as usize).max(1)
}

/// Flatten the arc centred at `center` with `radius`, starting at `start_angle` and
/// sweeping `sweep` radians (positive = counter-clockwise). Returns `n + 1` points for
/// `n` segments; the first is the arc start and the last is the arc end.
pub fn flatten_arc(
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    tolerance: f64,
) -> Result<Vec<Point>, FlattenError> {
    if !(center.is_finite() && radius.is_finite() && start_angle.is_finite() && sweep.is_finite()) {
        return Err(FlattenError::NonFiniteInput);
    }
    if radius <= 0.0 {
        return Err(FlattenError::NonPositiveRadius);
    }
    if tolerance.is_nan() || tolerance <= 0.0 {
        return Err(FlattenError::NonPositiveTolerance);
    }

    let n = arc_segment_count(radius, sweep, tolerance);
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        // Interpolate on the angle, not on cumulative steps, so the last point is exactly the end.
        let a = start_angle + sweep * (i as f64 / n as f64);
        pts.push(Point::new(
            center.x + radius * a.cos(),
            center.y + radius * a.sin(),
        ));
    }
    Ok(pts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::{PI, TAU};

    fn sagitta(radius: f64, step: f64) -> f64 {
        radius * (1.0 - (step / 2.0).cos())
    }

    /// `1 - cos` cancels catastrophically for small steps, so allow rounding slack
    /// proportional to the radius on top of a relative slack on the tolerance.
    fn slack(radius: f64, tol: f64) -> f64 {
        1e-9 * tol + 1e-14 * radius
    }

    #[test]
    fn rejects_bad_input() {
        let c = Point::new(0.0, 0.0);
        assert_eq!(
            flatten_arc(c, 0.0, 0.0, 1.0, 0.01),
            Err(FlattenError::NonPositiveRadius)
        );
        assert_eq!(
            flatten_arc(c, 1.0, 0.0, 1.0, 0.0),
            Err(FlattenError::NonPositiveTolerance)
        );
        assert_eq!(
            flatten_arc(c, 1.0, f64::NAN, 1.0, 0.01),
            Err(FlattenError::NonFiniteInput)
        );
    }

    #[test]
    fn full_circle_closes_on_itself() {
        let pts = flatten_arc(Point::new(1.0, 2.0), 5.0, 0.3, TAU, 0.01).unwrap();
        assert!(pts.len() >= 5);
        assert!(pts[0].distance(*pts.last().unwrap()) < 1e-9);
    }

    #[test]
    fn zero_sweep_is_a_single_degenerate_segment() {
        let pts = flatten_arc(Point::new(0.0, 0.0), 1.0, 0.0, 0.0, 0.01).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0], pts[1]);
    }

    #[test]
    fn coarse_tolerance_still_caps_at_quarter_turn() {
        // tolerance larger than the radius: only the quarter-turn cap applies.
        assert_eq!(arc_segment_count(1.0, TAU, 10.0), 4);
        assert_eq!(arc_segment_count(1.0, PI, 10.0), 2);
    }

    fn arc() -> impl Strategy<Value = (Point, f64, f64, f64, f64)> {
        (
            (-1e3..1e3f64, -1e3..1e3f64).prop_map(|(x, y)| Point::new(x, y)),
            1e-3..1e3f64, // radius
            -TAU..TAU,    // start angle
            -TAU..TAU,    // sweep
            1e-4..1.0f64, // tolerance
        )
    }

    proptest! {
        #[test]
        fn endpoints_are_exact((c, r, a0, sweep, tol) in arc()) {
            let pts = flatten_arc(c, r, a0, sweep, tol).unwrap();
            let start = Point::new(c.x + r * a0.cos(), c.y + r * a0.sin());
            let end = Point::new(c.x + r * (a0 + sweep).cos(), c.y + r * (a0 + sweep).sin());
            let eps = 1e-9 * (r + c.x.abs() + c.y.abs());
            prop_assert!(pts[0].distance(start) <= eps);
            prop_assert!(pts.last().unwrap().distance(end) <= eps);
        }

        #[test]
        fn vertices_lie_on_the_circle((c, r, a0, sweep, tol) in arc()) {
            let pts = flatten_arc(c, r, a0, sweep, tol).unwrap();
            let eps = 1e-9 * (r + c.x.abs() + c.y.abs());
            for p in &pts {
                prop_assert!((p.distance(c) - r).abs() <= eps, "vertex off circle by {}", (p.distance(c) - r).abs());
            }
        }

        #[test]
        fn chordal_error_within_tolerance((c, r, a0, sweep, tol) in arc()) {
            let pts = flatten_arc(c, r, a0, sweep, tol).unwrap();
            let n = pts.len() - 1;
            let step = sweep.abs() / n as f64;
            prop_assert!(step <= FRAC_PI_2 + 1e-12);
            prop_assert!(sagitta(r, step) <= tol + slack(r, tol));
        }

        #[test]
        fn segment_count_is_minimal((c, r, a0, sweep, tol) in arc()) {
            let pts = flatten_arc(c, r, a0, sweep, tol).unwrap();
            let n = pts.len() - 1;
            if n > 1 {
                // One fewer segment would violate either the tolerance or the quarter-turn cap.
                let coarser = sweep.abs() / (n - 1) as f64;
                prop_assert!(
                    sagitta(r, coarser) > tol - slack(r, tol) || coarser > FRAC_PI_2 * (1.0 - 1e-12),
                    "n={} could be reduced", n
                );
            }
        }

        #[test]
        fn angle_advances_monotonically((c, r, a0, sweep, tol) in arc()) {
            // Tiny sweeps make the cross product below indistinguishable from rounding.
            prop_assume!(sweep.abs() > 1e-6);
            let pts = flatten_arc(c, r, a0, sweep, tol).unwrap();
            // Cross product of consecutive radius vectors has the sign of the sweep.
            for w in pts.windows(2) {
                let (ux, uy) = (w[0].x - c.x, w[0].y - c.y);
                let (vx, vy) = (w[1].x - c.x, w[1].y - c.y);
                let cross = ux * vy - uy * vx;
                prop_assert!(cross.signum() == sweep.signum(), "direction flipped");
            }
        }
    }
}

/// Flatten a cubic Bézier by adaptive subdivision until both control points lie within
/// `tolerance` of the chord; emits every point after `p0`, ending exactly at `p3`.
pub fn flatten_cubic(
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    tolerance: f64,
    emit: &mut dyn FnMut(Point),
) {
    fn chord_dist(a: Point, b: Point, c: Point) -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l = dx.hypot(dy);
        if l < 1e-12 {
            return (c.x - a.x).hypot(c.y - a.y);
        }
        ((c.x - a.x) * dy - (c.y - a.y) * dx).abs() / l
    }
    fn rec(
        p0: Point,
        p1: Point,
        p2: Point,
        p3: Point,
        tol: f64,
        emit: &mut dyn FnMut(Point),
        depth: u32,
    ) {
        if depth >= 16 || chord_dist(p0, p3, p1).max(chord_dist(p0, p3, p2)) <= tol {
            emit(p3);
            return;
        }
        let mid = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let p01 = mid(p0, p1);
        let p12 = mid(p1, p2);
        let p23 = mid(p2, p3);
        let p012 = mid(p01, p12);
        let p123 = mid(p12, p23);
        let m = mid(p012, p123);
        rec(p0, p01, p012, m, tol, emit, depth + 1);
        rec(m, p123, p23, p3, tol, emit, depth + 1);
    }
    rec(p0, p1, p2, p3, tolerance.max(1e-9), emit, 0);
}

/// Relative radial deviation the usual cubic-Bézier approximation of a circular arc has
/// (2.7e-4 of the radius for a quarter turn, less for shorter arcs), with some headroom.
pub const BEZIER_ARC_REL_ERROR: f64 = 3e-4;

/// Recognise a cubic Bézier that represents a circular arc and return the arc's bulge
/// (`tan(sweep / 4)`, positive counter-clockwise). The curve qualifies when its end
/// tangents make equal angles with the chord, the sweep is below a half turn, and sampled
/// points lie within `max(tolerance, BEZIER_ARC_REL_ERROR × radius)` of the circle. That
/// relative allowance is the error the Bézier itself makes when it approximates an arc,
/// so an SVG or PDF circle comes back as the circle it was drawn from.
pub fn cubic_as_arc(p0: Point, p1: Point, p2: Point, p3: Point, tolerance: f64) -> Option<f64> {
    let (cx, cy) = (p3.x - p0.x, p3.y - p0.y);
    let chord = cx.hypot(cy);
    let (t0x, t0y) = (p1.x - p0.x, p1.y - p0.y);
    let (t1x, t1y) = (p3.x - p2.x, p3.y - p2.y);
    if chord < 1e-12 || t0x.hypot(t0y) < 1e-12 || t1x.hypot(t1y) < 1e-12 {
        return None;
    }
    let angle = |ax: f64, ay: f64, bx: f64, by: f64| (ax * by - ay * bx).atan2(ax * bx + ay * by);
    // Signed angle from the start tangent to the chord, and from the chord to the end
    // tangent: equal for a circular arc, and together the sweep.
    let a0 = angle(t0x, t0y, cx, cy);
    let a1 = angle(cx, cy, t1x, t1y);
    if (a0 - a1).abs() > 1e-3 || a0.abs() < 1e-9 {
        return None;
    }
    let sweep = a0 + a1;
    if sweep.abs() >= std::f64::consts::PI - 1e-9 {
        return None;
    }
    let bulge = (sweep / 4.0).tan();
    let arc = crate::arc::Arc::from_bulge(p0, p3, bulge)?;
    let allowed = tolerance.max(BEZIER_ARC_REL_ERROR * arc.radius);
    for i in 1..8 {
        let t = i as f64 / 8.0;
        let u = 1.0 - t;
        let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let x = b0 * p0.x + b1 * p1.x + b2 * p2.x + b3 * p3.x;
        let y = b0 * p0.y + b1 * p1.y + b2 * p2.y + b3 * p3.y;
        let d = (x - arc.center.x).hypot(y - arc.center.y);
        if (d - arc.radius).abs() > allowed {
            return None;
        }
    }
    Some(bulge)
}

#[cfg(test)]
mod cubic_tests {
    use super::*;

    #[test]
    fn cubic_flattening_respects_tolerance_and_endpoints() {
        // Quarter circle of radius 10 as a Bézier (kappa 0.5523).
        let k = 0.552_284_75 * 10.0;
        let (p0, p1, p2, p3) = (
            Point::new(10.0, 0.0),
            Point::new(10.0, k),
            Point::new(k, 10.0),
            Point::new(0.0, 10.0),
        );
        let mut pts = vec![p0];
        flatten_cubic(p0, p1, p2, p3, 0.001, &mut |p| pts.push(p));
        assert_eq!(*pts.last().unwrap(), p3);
        assert!(pts.len() > 8);
        // Every vertex is within the Bézier's deviation from the true circle (< 0.03 %) plus
        // the tolerance.
        for p in &pts {
            let r = p.x.hypot(p.y);
            assert!((r - 10.0).abs() < 0.01, "{p:?}");
        }
    }

    #[test]
    fn bezier_arcs_are_recognised_and_other_cubics_are_not() {
        let k = 0.552_284_75;
        // Quarter circle r = 10 counter-clockwise: bulge tan(pi/8).
        let b = cubic_as_arc(
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0 * k),
            Point::new(10.0 * k, 10.0),
            Point::new(0.0, 10.0),
            0.001,
        )
        .unwrap();
        assert!((b - (std::f64::consts::PI / 8.0).tan()).abs() < 1e-9);
        // The same arc traversed clockwise has the opposite bulge.
        let b = cubic_as_arc(
            Point::new(0.0, 10.0),
            Point::new(10.0 * k, 10.0),
            Point::new(10.0, 10.0 * k),
            Point::new(10.0, 0.0),
            0.001,
        )
        .unwrap();
        assert!(b < 0.0);
        // A quarter ellipse (a = 10, b = 5) is not an arc.
        assert!(
            cubic_as_arc(
                Point::new(10.0, 0.0),
                Point::new(10.0, 5.0 * k),
                Point::new(10.0 * k, 5.0),
                Point::new(0.0, 5.0),
                0.001
            )
            .is_none()
        );
        // An S-curve and a straight cubic are not arcs.
        assert!(
            cubic_as_arc(
                Point::new(0.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(2.0, -1.0),
                Point::new(3.0, 0.0),
                0.001
            )
            .is_none()
        );
        assert!(
            cubic_as_arc(
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(2.0, 0.0),
                Point::new(3.0, 0.0),
                0.001
            )
            .is_none()
        );
        // Handles 20 % too long: same tangents, but the curve leaves the circle.
        assert!(
            cubic_as_arc(
                Point::new(10.0, 0.0),
                Point::new(10.0, 12.0 * k),
                Point::new(12.0 * k, 10.0),
                Point::new(0.0, 10.0),
                0.001
            )
            .is_none()
        );
    }
}
