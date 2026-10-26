//! The local changes to the vendored polyline kernel (vendor/cavalier_contours/NOTICE).
//!
//! A segment end that lies on the other segment's arc within the kernel's tolerance is a
//! "sticky" intersect: the kernel reports the end point instead of the computed crossing
//! next to it. Upstream it also replaced a crossing far from that end when the crossing
//! next to the end fell just outside the segment, so the far crossing was lost (and, as
//! the end point is skipped as the next segment's start, often nothing was reported).

use std::f64::consts::PI;

use cavalier_contours::core::math::Vector2;
use cavalier_contours::polyline::{PlineSource, PlineSourceMut, PlineVertex, Polyline};
use proptest::prelude::*;

/// The kernel's position tolerance (`pos_equal_eps` of its intersection defaults).
const EPS: f64 = 1e-5;

fn point_at(c: Vector2<f64>, r: f64, a: f64) -> Vector2<f64> {
    Vector2::new(c.x + r * a.cos(), c.y + r * a.sin())
}

/// One arc segment of the circle (`c`, `r`) from angle `a0` to `a1` (counter-clockwise if
/// `a1 > a0`, at most pi either way), as an open polyline.
fn arc(c: Vector2<f64>, r: f64, a0: f64, a1: f64) -> Polyline<f64> {
    let (p, q) = (point_at(c, r, a0), point_at(c, r, a1));
    let mut pl = Polyline::new();
    pl.add_vertex(PlineVertex::new(p.x, p.y, ((a1 - a0) / 4.0).tan()));
    pl.add(q.x, q.y, 0.0);
    pl
}

fn intersects(a: &Polyline<f64>, b: &Polyline<f64>) -> Vec<Vector2<f64>> {
    let ix = a.find_intersects(b);
    ix.basic_intersects.iter().map(|i| i.point).collect()
}

fn near(points: &[Vector2<f64>], p: Vector2<f64>, tol: f64) -> bool {
    points.iter().any(|q| (*q - p).length() <= tol)
}

proptest! {
    // About a quarter of the generated arc pairs are not the configuration in question.
    #![proptest_config(ProptestConfig {
        cases: 512,
        max_global_rejects: 1 << 20,
        ..ProptestConfig::default()
    })]

    /// A line that crosses a circle at `x` and ends within the tolerance of it at `v`
    /// (inside or outside): the crossing at `x` is reported.
    #[test]
    fn a_line_ending_on_an_arc_keeps_its_other_crossing(
        r in 0.05..5.0f64,
        at in 0.0..2.0 * PI,
        apart in 0.3..(PI - 0.3),
        depth in -0.8 * EPS..0.8 * EPS,
        beyond in 0.1..1.0f64,
    ) {
        let c = Vector2::new(1.0, -2.0);
        let x = point_at(c, r, at);
        let v = point_at(c, r + depth, at + apart);
        let a = x + (x - v).scale(beyond);
        let mut line = Polyline::new();
        line.add(a.x, a.y, 0.0);
        line.add(v.x, v.y, 0.0);
        let circle = arc(c, r, at - 0.1, at + apart + 0.1);
        let found = intersects(&line, &circle);
        prop_assert!(near(&found, x, 1e-9 * r.max(1.0) + 1e-9), "{found:?} misses {x:?}");
        let found = intersects(&circle, &line);
        prop_assert!(near(&found, x, 1e-9 * r.max(1.0) + 1e-9), "{found:?} misses {x:?}");
    }

    /// Two arcs crossing at `p` whose circles cross again at `q`, where the second arc
    /// stops a little short of `q` (outside the tolerance) but close enough to the first
    /// circle to lie on it: the crossing at `p` is reported.
    #[test]
    fn an_arc_ending_on_an_arc_keeps_its_other_crossing(
        r1 in 0.2..5.0f64,
        ratio in 0.5..2.0f64,
        q_at in 0.0..2.0 * PI,
        angle in 0.2..0.6f64,
        short in 1.3 * EPS..1.6 * EPS,
    ) {
        let c1 = Vector2::new(-1.0, 0.5);
        let r2 = r1 * ratio;
        let q = point_at(c1, r1, q_at);
        // The second circle passes through `q`, crossing the first at `angle`.
        let inward = (c1 - q).normalize();
        let (s, k) = angle.sin_cos();
        let toward = Vector2::new(inward.x * k - inward.y * s, inward.x * s + inward.y * k);
        let c2 = q + toward.scale(r2);
        // The other crossing, `p`: the mirror image of `q` in the line of centres.
        let axis = (c2 - c1).normalize();
        let along = (q - c1).dot(axis);
        let p = c1 + axis.scale(2.0 * along) - (q - c1);
        let angle_of = |c: Vector2<f64>, z: Vector2<f64>| (z.y - c.y).atan2(z.x - c.x);
        // Signed angle from `a` to `b`, the short way round.
        let turn = |a: f64, b: f64| {
            let mut d = b - a;
            while d <= -PI {
                d += 2.0 * PI;
            }
            while d > PI {
                d -= 2.0 * PI;
            }
            d
        };
        // Second arc, round `c2` from before `p` towards `q`, ending short of `q`.
        let (p2, q2) = (angle_of(c2, p), angle_of(c2, q));
        let to_q = turn(p2, q2);
        prop_assume!(to_q.abs() > 0.3 && to_q.abs() < PI - 0.3);
        let way = to_q.signum();
        let end = q2 - way * short / r2;
        let second = arc(c2, r2, p2 - way * 0.1, p2 + to_q - way * short / r2);
        let e = point_at(c2, r2, end);
        // Only the configuration in question: the end lies on the first circle within the
        // tolerance, `q` is outside the second arc beyond it.
        prop_assume!(((e - c1).length() - r1).abs() < 0.95 * EPS);
        // First arc, through both crossings.
        let (p1, q1) = (angle_of(c1, p), angle_of(c1, q));
        let span = turn(p1, q1);
        prop_assume!(span.abs() < PI - 0.3);
        let (lo, hi) = if span > 0.0 { (p1, p1 + span) } else { (p1 + span, p1) };
        let first = arc(c1, r1, lo - 0.1, hi + 0.1);
        let tol = 1e-9 * r1.max(1.0) + 1e-9;
        let found = intersects(&first, &second);
        prop_assert!(near(&found, p, tol), "{found:?} misses {p:?}");
        let found = intersects(&second, &first);
        prop_assert!(near(&found, p, tol), "{found:?} misses {p:?}");
    }
}

/// The case found in a customer's board: a round pad and a polygonal one, a vertex of the
/// polygon 9.5 nm inside the circle. The kernel reported three crossings (an odd number
/// between two closed loops), the exact boolean could not close its boundary, and the
/// whole cluster fell back to the polygon boolean.
#[test]
fn a_polygon_vertex_just_inside_a_circle_keeps_all_four_crossings() {
    let mut circle = Polyline::new_closed();
    circle.add(15.029676, 23.988456, 1.0);
    circle.add(15.279672, 23.988456, 1.0);
    let mut pad = Polyline::new_closed();
    for (x, y) in [
        (15.248755331348402, 24.07040762815727),
        (15.243839694381515, 24.08762038000611),
        (15.232709188399774, 24.10164136790249),
        (15.217061819460826, 24.110338270241165),
        (15.199278109272722, 24.112383255557866),
        (15.109577375199445, 24.105025381495675),
        (15.092364623350603, 24.100109744528787),
        (15.078343635454223, 24.088979238547047),
        (15.06964673311555, 24.07333186960826),
        (15.067601747798847, 24.05554815942006),
        (15.257748252201154, 23.960773840580007),
    ] {
        pad.add(x, y, 0.0);
    }
    assert_eq!(intersects(&circle, &pad).len(), 4);
    assert_eq!(intersects(&pad, &circle).len(), 4);
}
