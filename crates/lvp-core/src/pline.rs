//! Line-and-arc polylines: the canonical curve representation of the core.
//!
//! We use `cavalier_contours`' `Polyline<f64>` directly (vertices carry a DXF-style bulge
//! for the arc to the next vertex) and add the helpers the rest of the core needs. The one
//! kernel rule enforced here: every segment's `|bulge| <= 1` (at most a half circle), which
//! `cavalier_contours` requires; [`add_bulge_segment`] splits larger arcs.

use cavalier_contours::core::math::Vector2;
use cavalier_contours::polyline::{
    PlineOrientation, PlineSource, PlineSourceMut, PlineVertex, Polyline, seg_midpoint,
    seg_tangent_vector,
};

use crate::arc::Arc;
use crate::flatten::FlattenError;
use crate::geom::{BBox, Point};

pub type Pline = Polyline<f64>;

/// Largest bulge magnitude a single segment may carry (half circle).
pub const MAX_BULGE: f64 = 1.0;

fn point(v: Vector2<f64>) -> Point {
    Point::new(v.x, v.y)
}

/// Append a vertex whose outgoing segment is the arc `from -> to` with `bulge`, splitting
/// arcs larger than a half circle into two. `from` is pushed (with the bulge), `to` is not.
pub fn add_bulge_segment(p: &mut Pline, from: Point, to: Point, bulge: f64) {
    if bulge.abs() <= MAX_BULGE {
        p.add(from.x, from.y, bulge);
        return;
    }
    match Arc::from_bulge(from, to, bulge) {
        Some(arc) => {
            let mid = arc.point_at(0.5);
            let half = (arc.sweep / 8.0).tan();
            p.add(from.x, from.y, half);
            p.add(mid.x, mid.y, half);
        }
        None => p.add(from.x, from.y, 0.0),
    }
}

/// Polyline from `(x, y, bulge)` triples (DXF LWPOLYLINE semantics). The last vertex's
/// bulge is the closing arc for closed polylines and is ignored for open ones.
pub fn from_bulge_vertices<I>(vertices: I, closed: bool) -> Pline
where
    I: IntoIterator<Item = (f64, f64, f64)>,
{
    let vs: Vec<(f64, f64, f64)> = vertices.into_iter().collect();
    let mut p = if closed {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    let n = vs.len();
    for (i, &(x, y, b)) in vs.iter().enumerate() {
        let next = if i + 1 < n {
            Some(vs[i + 1])
        } else if closed && n > 1 {
            Some(vs[0])
        } else {
            None
        };
        match next {
            Some((nx, ny, _)) => add_bulge_segment(&mut p, Point::new(x, y), Point::new(nx, ny), b),
            None => p.add(x, y, 0.0),
        }
    }
    p
}

/// Closed polyline from plain points (straight segments).
pub fn from_points<I: IntoIterator<Item = Point>>(points: I, closed: bool) -> Pline {
    from_bulge_vertices(points.into_iter().map(|p| (p.x, p.y, 0.0)), closed)
}

/// Full circle as two half-circle arcs, counter-clockwise.
pub fn circle(center: Point, radius: f64) -> Pline {
    let mut p = Pline::new_closed();
    p.add(center.x - radius, center.y, 1.0);
    p.add(center.x + radius, center.y, 1.0);
    p
}

/// Axis-aligned rectangle, counter-clockwise.
pub fn rect(min: Point, max: Point) -> Pline {
    from_points(
        [
            Point::new(min.x, min.y),
            Point::new(max.x, min.y),
            Point::new(max.x, max.y),
            Point::new(min.x, max.y),
        ],
        true,
    )
}

pub fn bounds(p: &Pline) -> Option<BBox> {
    p.extents().map(|e| BBox {
        min: Point::new(e.min_x, e.min_y),
        max: Point::new(e.max_x, e.max_y),
    })
}

/// Orientation of a closed polyline; open polylines return `None`.
pub fn is_ccw(p: &Pline) -> Option<bool> {
    match p.orientation() {
        PlineOrientation::CounterClockwise => Some(true),
        PlineOrientation::Clockwise => Some(false),
        PlineOrientation::Open => None,
    }
}

/// Make a closed polyline counter-clockwise (no-op when it already is, or when open).
pub fn ensure_ccw(mut p: Pline) -> Pline {
    if is_ccw(&p) == Some(false) {
        p.invert_direction_mut();
    }
    p
}

/// Flatten to points within `tolerance` (chordal deviation). For closed polylines the
/// first point is not repeated at the end.
pub fn flatten(p: &Pline, tolerance: f64) -> Result<Vec<Point>, FlattenError> {
    let mut out: Vec<Point> = Vec::with_capacity(p.vertex_count());
    for (v1, v2) in p.iter_segments() {
        let a = point(v1.pos());
        let b = point(v2.pos());
        match Arc::from_bulge(a, b, v1.bulge) {
            Some(arc) => {
                let pts = arc.flatten(tolerance)?;
                // Skip the arc's last point: it is the next vertex, pushed by the next segment
                // (or already present as the first point for a closed loop).
                out.extend(pts.iter().take(pts.len() - 1).copied());
            }
            None => out.push(a),
        }
    }
    if !p.is_closed()
        && let Some(last) = p.last()
    {
        out.push(point(last.pos()));
    }
    Ok(out)
}

/// Midpoint of a segment and the unit normal pointing to the left of the direction of travel.
pub fn seg_midpoint_and_left_normal(v1: PlineVertex<f64>, v2: PlineVertex<f64>) -> (Point, Point) {
    let mid = seg_midpoint(v1, v2);
    let t = seg_tangent_vector(v1, v2, mid);
    let len = t.x.hypot(t.y);
    let n = if len > 0.0 {
        Point::new(-t.y / len, t.x / len)
    } else {
        Point::new(0.0, 0.0)
    };
    (point(mid), n)
}

/// Rotate about the origin by `angle` (radians, counter-clockwise). Arcs are preserved.
pub fn rotated(p: &Pline, angle: f64) -> Pline {
    let (s, c) = angle.sin_cos();
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for v in p.iter_vertexes() {
        out.add(c * v.x - s * v.y, s * v.x + c * v.y, v.bulge);
    }
    out
}

/// Total path length.
pub fn length(p: &Pline) -> f64 {
    p.path_length()
}

/// Positions closer than this are the same vertex. Below any manufacturing relevance, above
/// cavalier's own fuzzy epsilon (so its debug assertions on coincident vertices hold).
pub const REPEAT_EPS: f64 = 1e-6;

/// Remove coincident consecutive vertices (including a closing duplicate of the first
/// vertex). Returns `None` when fewer than two vertices remain. Every polyline entering the
/// kernel must pass through here: a zero-length arc segment is undefined.
pub fn sanitize(p: &Pline) -> Option<Pline> {
    let mut q = p.remove_repeat_pos(REPEAT_EPS).unwrap_or_else(|| p.clone());
    if q.is_closed()
        && q.vertex_count() >= 2
        && q.at(0)
            .pos()
            .fuzzy_eq_eps(q.last().expect("non-empty").pos(), REPEAT_EPS)
    {
        q.remove_last();
    }
    (q.vertex_count() >= 2).then_some(q)
}

/// Outline of a stroked polyline with flat ends and mitred joins (bevelled where the mitre
/// would exceed twice the width): the exact area a square-ended path covers. `ext_begin`
/// and `ext_end` extend the ends along the path (half the width for square ends). Returns
/// `None` for zero width or a single point without extension.
pub fn stroke_polygon(
    pts: &[Point],
    width: f64,
    ext_begin: f64,
    ext_end: f64,
) -> Option<Vec<Point>> {
    let mut p: Vec<Point> = Vec::new();
    for &q in pts {
        if p.last()
            .is_none_or(|l: &Point| (l.x - q.x).abs() > 1e-9 || (l.y - q.y).abs() > 1e-9)
        {
            p.push(q);
        }
    }
    let hw = width / 2.0;
    if hw <= 0.0 || p.is_empty() {
        return None;
    }
    if p.len() == 1 {
        let c = p[0];
        if ext_begin <= 0.0 && ext_end <= 0.0 {
            return None;
        }
        return Some(vec![
            Point::new(c.x - ext_begin, c.y - hw),
            Point::new(c.x + ext_end, c.y - hw),
            Point::new(c.x + ext_end, c.y + hw),
            Point::new(c.x - ext_begin, c.y + hw),
        ]);
    }
    let n = p.len();
    let dir = |a: Point, b: Point| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l = dx.hypot(dy);
        (dx / l, dy / l)
    };
    let d0 = dir(p[0], p[1]);
    p[0] = Point::new(p[0].x - d0.0 * ext_begin, p[0].y - d0.1 * ext_begin);
    let dn = dir(p[n - 2], p[n - 1]);
    p[n - 1] = Point::new(p[n - 1].x + dn.0 * ext_end, p[n - 1].y + dn.1 * ext_end);
    let segs: Vec<(Point, (f64, f64))> = p.windows(2).map(|w| (w[0], dir(w[0], w[1]))).collect();
    let side = |sign: f64| -> Vec<Point> {
        let normal = |d: (f64, f64)| (-d.1 * hw * sign, d.0 * hw * sign);
        let mut out = Vec::new();
        let (s0, d0) = segs[0];
        let n0 = normal(d0);
        out.push(Point::new(s0.x + n0.0, s0.y + n0.1));
        for k in 1..segs.len() {
            let (_, dprev) = segs[k - 1];
            let (pk, dk) = segs[k];
            let np = normal(dprev);
            let nk = normal(dk);
            let a = Point::new(pk.x + np.0, pk.y + np.1);
            let b = Point::new(pk.x + nk.0, pk.y + nk.1);
            let cross = dprev.0 * dk.1 - dprev.1 * dk.0;
            if cross.abs() < 1e-9 {
                out.push(a);
                continue;
            }
            let t = ((b.x - a.x) * dk.1 - (b.y - a.y) * dk.0) / cross;
            let m = Point::new(a.x + dprev.0 * t, a.y + dprev.1 * t);
            if (m.x - pk.x).hypot(m.y - pk.y) <= hw * 4.0 {
                out.push(m);
            } else {
                out.push(a);
                out.push(b);
            }
        }
        let (_, dl) = segs[segs.len() - 1];
        let nl = normal(dl);
        let end = p[n - 1];
        out.push(Point::new(end.x + nl.0, end.y + nl.1));
        out
    };
    let mut left = side(1.0);
    let mut right = side(-1.0);
    right.reverse();
    left.append(&mut right);
    Some(left)
}

/// Append the arc `from -> to` around `center` to an open polyline whose last vertex is
/// `from`, splitting arcs over a half turn and expressing a full circle (`from == to`) as
/// two half circles. The end point is trusted for direction only: the radius comes from
/// `from`, and `to` is pushed as given.
pub fn add_center_arc(p: &mut Pline, from: Point, to: Point, center: Point, ccw: bool) {
    use std::f64::consts::{PI, TAU};
    let r = (from.x - center.x).hypot(from.y - center.y);
    if r <= 0.0 {
        p.add(to.x, to.y, 0.0);
        return;
    }
    let a0 = (from.y - center.y).atan2(from.x - center.x);
    let a1 = (to.y - center.y).atan2(to.x - center.x);
    let same = (from.x - to.x).abs() < 1e-9 && (from.y - to.y).abs() < 1e-9;
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 }.rem_euclid(TAU);
    if same {
        sweep = TAU;
    }
    let sign = if ccw { 1.0 } else { -1.0 };
    let last = p.vertex_count() - 1;
    let set_last_bulge = |p: &mut Pline, b: f64| {
        let mut v = p.at(last);
        v.bulge = b;
        p.set_vertex(last, v);
    };
    if (sweep - TAU).abs() < 1e-9 {
        let opposite = Point::new(2.0 * center.x - from.x, 2.0 * center.y - from.y);
        set_last_bulge(p, sign);
        p.add(opposite.x, opposite.y, sign);
        p.add(to.x, to.y, 0.0);
        return;
    }
    if sweep > PI + 1e-12 {
        let amid = a0 + sign * sweep / 2.0;
        let mid = Point::new(center.x + r * amid.cos(), center.y + r * amid.sin());
        let b = sign * (sweep / 8.0).tan();
        set_last_bulge(p, b);
        p.add(mid.x, mid.y, b);
        p.add(to.x, to.y, 0.0);
        return;
    }
    set_last_bulge(p, sign * (sweep / 4.0).tan());
    p.add(to.x, to.y, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    #[test]
    fn large_bulges_are_split() {
        // 270° arc from (1,0) to (0,-1) CCW around the origin: bulge = tan(270°/4) = tan(67.5°).
        let b = (3.0 * PI / 4.0 / 2.0).tan();
        assert!(b > 1.0);
        let p = from_bulge_vertices([(1.0, 0.0, b), (0.0, -1.0, 0.0)], false);
        assert_eq!(p.vertex_count(), 3);
        for (v1, _) in p.iter_segments() {
            assert!(v1.bulge.abs() <= MAX_BULGE + 1e-12);
        }
        let pts = flatten(&p, 1e-4).unwrap();
        for q in &pts {
            assert!((q.distance(Point::new(0.0, 0.0)) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn circle_area_and_orientation() {
        let c = circle(Point::new(3.0, 4.0), 2.0);
        assert!((c.area() - PI * 4.0).abs() < 1e-9);
        assert_eq!(is_ccw(&c), Some(true));
        let cw = {
            let mut p = c.clone();
            p.invert_direction_mut();
            p
        };
        assert_eq!(is_ccw(&cw), Some(false));
        assert_eq!(is_ccw(&ensure_ccw(cw)), Some(true));
    }

    #[test]
    fn flatten_closed_does_not_repeat_first_point() {
        let r = rect(Point::new(0.0, 0.0), Point::new(2.0, 1.0));
        let pts = flatten(&r, 1e-3).unwrap();
        assert_eq!(pts.len(), 4);
        let c = circle(Point::new(0.0, 0.0), 1.0);
        let pts = flatten(&c, 1e-3).unwrap();
        assert!(pts.len() > 8);
        assert!(pts[0].distance(*pts.last().unwrap()) > 1e-3);
    }

    #[test]
    fn sanitize_drops_repeats_and_closing_duplicate() {
        // Two half-circle arcs, each start vertex duplicated, plus a closing duplicate.
        let mut p = Pline::new_closed();
        p.add(0.0, 0.0, 1.0);
        p.add(0.0, 0.0, 1.0);
        p.add(2.0, 0.0, 1.0);
        p.add(2.0, 0.0, 1.0);
        p.add(0.0, 0.0, 0.0);
        let q = sanitize(&p).unwrap();
        assert_eq!(q.vertex_count(), 2);
        assert!(
            (q.area() - PI).abs() < 1e-9,
            "unit circle remains: {}",
            q.area()
        );
        let mut single = Pline::new();
        single.add(1.0, 1.0, 0.0);
        single.add(1.0, 1.0, 0.0);
        assert!(sanitize(&single).is_none());
    }

    #[test]
    fn left_normal_points_inside_ccw_rect() {
        let r = rect(Point::new(0.0, 0.0), Point::new(2.0, 2.0));
        let (mid, n) = seg_midpoint_and_left_normal(r.at(0), r.at(1));
        assert!((mid.x - 1.0).abs() < 1e-12 && mid.y.abs() < 1e-12);
        assert!(n.y > 0.99); // left of +x travel is +y, i.e. into the rectangle
    }

    proptest! {
        #[test]
        fn flattened_circle_stays_on_circle(cx in -100.0..100.0f64, cy in -100.0..100.0f64,
                                            r in 0.01..50.0f64, tol in 1e-4..0.1f64) {
            let c = circle(Point::new(cx, cy), r);
            let pts = flatten(&c, tol).unwrap();
            for q in &pts {
                prop_assert!((q.distance(Point::new(cx, cy)) - r).abs() <= 1e-9 * (r + cx.abs() + cy.abs()));
            }
            // Shoelace cancellation grows with the distance from the origin.
            prop_assert!((c.area() - PI * r * r).abs() <= 1e-10 * (r * r + cx * cx + cy * cy));
        }

        #[test]
        fn bulge_vertices_roundtrip_length(verts in prop::collection::vec((-50.0..50.0f64, -50.0..50.0f64, -3.0..3.0f64), 2..8)) {
            // Splitting arcs must not change the path length.
            let p = from_bulge_vertices(verts.iter().copied(), false);
            let mut expected = 0.0;
            for w in verts.windows(2) {
                let (a, b) = (Point::new(w[0].0, w[0].1), Point::new(w[1].0, w[1].1));
                expected += match Arc::from_bulge(a, b, w[0].2) {
                    Some(arc) => arc.radius * arc.sweep.abs(),
                    None => a.distance(b),
                };
            }
            prop_assert!((length(&p) - expected).abs() <= 1e-7 * (1.0 + expected));
        }
    }
}
