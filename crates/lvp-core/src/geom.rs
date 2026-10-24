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
        fn over_shrinking_is_rejected(pts in prop::collection::vec(point(), 1..20),
                                      extra in (10.0 * EPS_MM)..100.0f64) {
            let b = BBox::of_points(pts.iter().copied()).unwrap();
            let half_min_side = b.width().min(b.height()) / 2.0;
            prop_assert_eq!(b.expanded(-(half_min_side + extra)), None);
        }
    }
}
