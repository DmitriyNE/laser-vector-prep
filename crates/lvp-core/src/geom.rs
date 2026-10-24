//! Basic planar primitives.

use rayon::prelude::*;

/// Numerical noise floor in millimetres (one picometre). Differences below this are
/// floating-point rounding, not geometry, and are snapped rather than reported.
pub const EPS_MM: f64 = 1e-9;

/// A point in document space, millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Axis-aligned bounding box, inclusive on both ends. Never empty: constructing one
/// from zero points yields `None` rather than a sentinel box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub min: Point,
    pub max: Point,
}

impl BBox {
    /// Degenerate (zero-area) box around a single point.
    pub fn at(p: Point) -> Self {
        Self { min: p, max: p }
    }

    pub fn of_points<I: IntoIterator<Item = Point>>(points: I) -> Option<Self> {
        let mut it = points.into_iter();
        let mut b = BBox::at(it.next()?);
        for p in it {
            b.include(p);
        }
        Some(b)
    }

    /// Parallel variant of [`BBox::of_points`] for large buffers. Needs a rayon pool to
    /// exist (on wasm: the thread pool must have been initialised by the host).
    pub fn of_points_par<I>(points: I) -> Option<Self>
    where
        I: IntoParallelIterator<Item = Point>,
    {
        points
            .into_par_iter()
            .map(BBox::at)
            .reduce_with(BBox::union)
    }

    pub fn include(&mut self, p: Point) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    pub fn union(self, other: BBox) -> BBox {
        let mut b = self;
        b.include(other.min);
        b.include(other.max);
        b
    }

    pub fn width(self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Grow (or, with a negative margin, shrink) by `margin` on every side.
    /// Returns `None` when shrinking would invert the box by more than [`EPS_MM`];
    /// callers must report that, not clamp it silently. Inversions within `EPS_MM`
    /// (rounding when shrinking a degenerate box) collapse to a point instead.
    pub fn expanded(self, margin: f64) -> Option<BBox> {
        let (min_x, max_x) = snap_interval(self.min.x - margin, self.max.x + margin)?;
        let (min_y, max_y) = snap_interval(self.min.y - margin, self.max.y + margin)?;
        Some(BBox {
            min: Point::new(min_x, min_y),
            max: Point::new(max_x, max_y),
        })
    }
}

/// Convex hull (Andrew's monotone chain), counter-clockwise, without the repeated first
/// point. Fewer than three distinct points yield what there is (0, 1 or 2 points).
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.iter().copied().filter(|p| p.is_finite()).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| (a.x - b.x).abs() <= EPS_MM && (a.y - b.y).abs() <= EPS_MM);
    if pts.len() < 3 {
        return pts;
    }
    let cross =
        |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    // Lower and upper chains are built separately so popping the upper chain can never eat
    // into the lower one.
    let chain = |iter: &mut dyn Iterator<Item = &Point>| {
        let mut out: Vec<Point> = Vec::new();
        for p in iter {
            while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], *p) <= 0.0 {
                out.pop();
            }
            out.push(*p);
        }
        out.pop(); // the last point is the first point of the other chain
        out
    };
    let mut hull = chain(&mut pts.iter());
    hull.extend(chain(&mut pts.iter().rev()));
    hull
}

/// Smallest circle enclosing all points (Welzl, iterative, deterministic order).
/// Returns `(centre, radius)`; `None` for no finite points.
pub fn min_enclosing_circle(points: &[Point]) -> Option<(Point, f64)> {
    let mut pts: Vec<Point> = points.iter().copied().filter(|p| p.is_finite()).collect();
    if pts.is_empty() {
        return None;
    }
    // Deterministic shuffle (xorshift) keeps the expected linear time without randomness.
    let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ pts.len() as u64;
    for i in (1..pts.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        pts.swap(i, (seed % (i as u64 + 1)) as usize);
    }
    let inside = |c: Point, r: f64, p: Point| p.distance(c) <= r * (1.0 + 1e-12) + EPS_MM;
    let circle2 = |a: Point, b: Point| {
        (
            Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
            a.distance(b) / 2.0,
        )
    };
    let circle3 = |a: Point, b: Point, c: Point| -> (Point, f64) {
        let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
        if d.abs() < 1e-18 {
            // Collinear: the two farthest apart define the circle.
            let (p, q) = [(a, b), (a, c), (b, c)]
                .into_iter()
                .max_by(|x, y| x.0.distance(x.1).total_cmp(&y.0.distance(y.1)))
                .unwrap();
            return circle2(p, q);
        }
        let (a2, b2, c2) = (
            a.x * a.x + a.y * a.y,
            b.x * b.x + b.y * b.y,
            c.x * c.x + c.y * c.y,
        );
        let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
        let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
        let centre = Point::new(ux, uy);
        (centre, centre.distance(a))
    };
    let (mut c, mut r) = (pts[0], 0.0);
    for i in 1..pts.len() {
        if inside(c, r, pts[i]) {
            continue;
        }
        (c, r) = (pts[i], 0.0);
        for j in 0..i {
            if inside(c, r, pts[j]) {
                continue;
            }
            (c, r) = circle2(pts[i], pts[j]);
            for k in 0..j {
                if !inside(c, r, pts[k]) {
                    (c, r) = circle3(pts[i], pts[j], pts[k]);
                }
            }
        }
    }
    Some((c, r))
}

/// Orders `lo <= hi`, tolerating an inversion up to `EPS_MM` by collapsing to the midpoint.
fn snap_interval(lo: f64, hi: f64) -> Option<(f64, f64)> {
    if lo <= hi {
        Some((lo, hi))
    } else if lo - hi <= EPS_MM {
        let mid = 0.5 * (lo + hi);
        Some((mid, mid))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn finite_mm() -> impl Strategy<Value = f64> {
        -1.0e4..1.0e4f64
    }

    fn point() -> impl Strategy<Value = Point> {
        (finite_mm(), finite_mm()).prop_map(|(x, y)| Point::new(x, y))
    }

    #[test]
    fn empty_input_has_no_bbox() {
        assert_eq!(BBox::of_points(std::iter::empty()), None);
    }

    proptest! {
        #[test]
        fn bbox_contains_every_input_point(pts in prop::collection::vec(point(), 1..50)) {
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            for p in &pts {
                prop_assert!(b.contains(*p));
            }
            prop_assert!(b.width() >= 0.0 && b.height() >= 0.0);
        }

        #[test]
        fn parallel_bbox_matches_sequential(pts in prop::collection::vec(point(), 0..2000)) {
            let seq = BBox::of_points(pts.iter().copied());
            let par = BBox::of_points_par(pts.clone());
            prop_assert_eq!(seq, par);
        }

        #[test]
        fn bbox_is_tight(pts in prop::collection::vec(point(), 1..50)) {
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            // Every face of the box is touched by at least one point.
            prop_assert!(pts.iter().any(|p| p.x == b.min.x));
            prop_assert!(pts.iter().any(|p| p.x == b.max.x));
            prop_assert!(pts.iter().any(|p| p.y == b.min.y));
            prop_assert!(pts.iter().any(|p| p.y == b.max.y));
        }

        #[test]
        fn union_contains_both(a in prop::collection::vec(point(), 1..20),
                               b in prop::collection::vec(point(), 1..20)) {
            let ba = BBox::of_points(a.iter().copied()).unwrap();
            let bb = BBox::of_points(b.iter().copied()).unwrap();
            let u = ba.union(bb);
            prop_assert_eq!(u, bb.union(ba));
            for p in a.iter().chain(b.iter()) {
                prop_assert!(u.contains(*p));
            }
        }

        #[test]
        fn expand_then_shrink_roundtrips(pts in prop::collection::vec(point(), 1..20),
                                         m in 0.0..100.0f64) {
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            let back = b.expanded(m).unwrap().expanded(-m).unwrap();
            let eps = 1e-9 * (1.0 + m);
            prop_assert!((back.min.x - b.min.x).abs() <= eps);
            prop_assert!((back.min.y - b.min.y).abs() <= eps);
            prop_assert!((back.max.x - b.max.x).abs() <= eps);
            prop_assert!((back.max.y - b.max.y).abs() <= eps);
        }

        #[test]
        fn hull_contains_all_points_and_is_convex(pts in prop::collection::vec(point(), 3..40)) {
            let hull = convex_hull(&pts);
            prop_assert!(!hull.is_empty());
            if hull.len() >= 3 {
                let n = hull.len();
                for i in 0..n {
                    let (a, b, c) = (hull[i], hull[(i + 1) % n], hull[(i + 2) % n]);
                    let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
                    prop_assert!(cross > -1e-6, "not convex / not ccw at {i}");
                }
                for p in &pts {
                    for i in 0..n {
                        let (a, b) = (hull[i], hull[(i + 1) % n]);
                        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                        prop_assert!(cross >= -1e-6 * (1.0 + a.distance(b)), "point outside hull edge {i}");
                    }
                }
            }
        }

        #[test]
        fn enclosing_circle_contains_all_and_touches_two(pts in prop::collection::vec(point(), 1..40)) {
            let (c, r) = min_enclosing_circle(&pts).unwrap();
            let tol = 1e-9 * (1.0 + r);
            for p in &pts {
                prop_assert!(p.distance(c) <= r + tol, "point outside circle by {}", p.distance(c) - r);
            }
            // Minimal: at least two points on the boundary (or one point with r == 0).
            let on = pts.iter().filter(|p| (p.distance(c) - r).abs() <= 1e-7 * (1.0 + r)).count();
            prop_assert!(on >= 2 || (pts.len() == 1 && r == 0.0) || pts.windows(2).all(|w| w[0] == w[1]));
            // Not larger than the bounding box's circumscribed circle.
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            prop_assert!(r <= b.width().hypot(b.height()) / 2.0 + tol);
        }

        #[test]
        fn over_shrinking_is_rejected(pts in prop::collection::vec(point(), 1..20),
                                      extra in (10.0 * EPS_MM)..100.0f64) {
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            let half_min_side = b.width().min(b.height()) / 2.0;
            prop_assert_eq!(b.expanded(-(half_min_side + extra)), None);
        }
    }
}
