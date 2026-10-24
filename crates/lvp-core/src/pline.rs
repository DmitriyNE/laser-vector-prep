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

/// Total path length.
pub fn length(p: &Pline) -> f64 {
    p.path_length()
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
            prop_assert!((c.area() - PI * r * r).abs() <= 1e-9 * r * r);
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
