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
