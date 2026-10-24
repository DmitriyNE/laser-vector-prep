//! Parallel hatch: equally spaced scanlines clipped exactly to a region and its holes.
//!
//! Lines lie at `phase + k * pitch` in a frame rotated by `angle` about the document
//! origin, so two layers with the same parameters share one lattice. Every row is split
//! into separate segments wherever it leaves the filled area: nothing bridges a hole.

use cavalier_contours::polyline::PlineSource;

use crate::arc::Arc;
use crate::geom::Point;
use crate::pline::{self, Pline};
use crate::region::Region;

#[derive(Clone, Copy, Debug)]
pub struct HatchParams {
    pub pitch_mm: f64,
    pub angle_deg: f64,
    pub phase_mm: f64,
    pub serpentine: bool,
}

/// Scanlines hitting a vertex exactly would be counted twice; nudge them off by this much.
const VERTEX_NUDGE: f64 = 1e-7;

pub fn hatch(region: &Region, p: &HatchParams) -> Vec<Pline> {
    if region.is_empty() || p.pitch_mm.is_nan() || p.pitch_mm <= 0.0 {
        return Vec::new();
    }
    let angle = p.angle_deg.to_radians();
    let rotated: Vec<Pline> = region
        .loops
        .iter()
        .map(|l| pline::rotated(l, -angle))
        .collect();
    let Some(bounds) = rotated
        .iter()
        .filter_map(pline::bounds)
        .reduce(|a, b| a.union(b))
    else {
        return Vec::new();
    };
    let mut vertex_ys: Vec<f64> = rotated
        .iter()
        .flat_map(|l| l.iter_vertexes().map(|v| v.y))
        .collect();
    vertex_ys.sort_by(f64::total_cmp);

    let k0 = ((bounds.min.y - p.phase_mm) / p.pitch_mm).ceil() as i64;
    let k1 = ((bounds.max.y - p.phase_mm) / p.pitch_mm).floor() as i64;
    let mut rows: Vec<(f64, Vec<f64>)> = Vec::new();
    for k in k0..=k1 {
        let mut y = p.phase_mm + k as f64 * p.pitch_mm;
        // Avoid exact vertex hits (binary search in the sorted vertex ys).
        let i = vertex_ys.partition_point(|v| *v < y - VERTEX_NUDGE);
        if vertex_ys
            .get(i)
            .is_some_and(|v| (v - y).abs() <= VERTEX_NUDGE)
        {
            y += 2.0 * VERTEX_NUDGE;
        }
        let mut xs: Vec<f64> = Vec::new();
        for l in &rotated {
            let Some(b) = pline::bounds(l) else { continue };
            if y < b.min.y || y > b.max.y {
                continue;
            }
            for (v1, v2) in l.iter_segments() {
                crossings(v1.x, v1.y, v2.x, v2.y, v1.bulge, y, &mut xs);
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(f64::total_cmp);
        rows.push((y, xs));
    }

    let (s, c) = angle.sin_cos();
    let back = |x: f64, y: f64| Point::new(c * x - s * y, s * x + c * y);
    let mut out = Vec::new();
    for (row, (y, xs)) in rows.into_iter().enumerate() {
        let reverse = p.serpentine && row % 2 == 1;
        let (pairs, _) = xs.as_chunks::<2>();
        let mut segs: Vec<(f64, f64)> = pairs
            .iter()
            .map(|[a, b]| (*a, *b))
            .filter(|(a, b)| b - a > 1e-9)
            .collect();
        if reverse {
            segs.reverse();
        }
        for (a, b) in segs {
            let (from, to) = if reverse { (b, a) } else { (a, b) };
            out.push(pline::from_points([back(from, y), back(to, y)], false));
        }
    }
    out
}

/// x coordinates where the horizontal line at `y` crosses the segment `(x1,y1)->(x2,y2)`
/// with `bulge`. Half-open at the upper endpoint so shared vertices count once.
fn crossings(x1: f64, y1: f64, x2: f64, y2: f64, bulge: f64, y: f64, xs: &mut Vec<f64>) {
    match Arc::from_bulge(Point::new(x1, y1), Point::new(x2, y2), bulge) {
        None => {
            if y1 == y2 {
                return; // horizontal: no crossing (its endpoints are counted by neighbours)
            }
            let (lo, hi) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
            if y >= lo && y < hi {
                xs.push(x1 + (y - y1) * (x2 - x1) / (y2 - y1));
            }
        }
        Some(arc) => {
            let dy = y - arc.center.y;
            if dy.abs() > arc.radius {
                return;
            }
            let dx = (arc.radius * arc.radius - dy * dy).max(0.0).sqrt();
            for x in [arc.center.x - dx, arc.center.x + dx] {
                let ang = dy.atan2(x - arc.center.x);
                if !arc.contains_angle(ang) {
                    continue;
                }
                // Half-open rule at the arc endpoints: count the lower endpoint, not the upper.
                let at_start = (x - x1).abs() < 1e-9 && (y - y1).abs() < 1e-9;
                let at_end = (x - x2).abs() < 1e-9 && (y - y2).abs() < 1e-9;
                if (at_start && y1 >= y2) || (at_end && y2 >= y1) {
                    continue;
                }
                // Tangent at the extremum (dx == 0) touches, does not cross.
                if dx < 1e-12 {
                    continue;
                }
                xs.push(x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pline::{circle, rect};
    use crate::region::LoopRule;
    use proptest::prelude::*;

    fn square(s: f64) -> Region {
        Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(s, s))],
            LoopRule::NonZero,
        )
    }

    fn params(pitch: f64, angle: f64, phase: f64, serp: bool) -> HatchParams {
        HatchParams {
            pitch_mm: pitch,
            angle_deg: angle,
            phase_mm: phase,
            serpentine: serp,
        }
    }

    #[test]
    fn horizontal_hatch_of_a_square() {
        let lines = hatch(&square(10.0), &params(1.0, 0.0, 0.5, true));
        assert_eq!(lines.len(), 10);
        let total: f64 = lines.iter().map(pline::length).sum();
        assert!((total - 100.0).abs() < 1e-9);
        // Serpentine: rows alternate direction.
        assert!(lines[0].at(0).x < lines[0].at(1).x);
        assert!(lines[1].at(0).x > lines[1].at(1).x);
        // Rows sit on the global lattice phase + k * pitch.
        for l in &lines {
            let y = l.at(0).y;
            assert!(((y - 0.5) / 1.0 - ((y - 0.5) / 1.0).round()).abs() < 1e-9);
        }
    }

    #[test]
    fn holes_split_rows_and_nothing_bridges_them() {
        let outer = square(20.0);
        let hole = Region::from_loops([circle(Point::new(10.0, 10.0), 4.0)], LoopRule::NonZero);
        let region = outer.difference(&hole);
        let lines = hatch(&region, &params(0.5, 0.0, 0.25, false));
        assert!(lines.len() > 40);
        for l in &lines {
            let (a, b) = (l.at(0), l.at(1));
            for t in [0.1, 0.5, 0.9] {
                let p = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
                assert!(region.contains(p), "segment leaves the region at {p:?}");
            }
        }
        // Rows crossing the hole have two segments; a row at y = 10 crosses the circle.
        let mid: Vec<&Pline> = lines
            .iter()
            .filter(|l| (l.at(0).y - 10.25).abs() < 1e-9)
            .collect();
        assert_eq!(mid.len(), 2);
    }

    #[test]
    fn angle_and_phase_are_document_coordinates() {
        let v = hatch(&square(10.0), &params(1.0, 90.0, 0.5, false));
        assert_eq!(v.len(), 10);
        for l in &v {
            assert!((l.at(0).x - l.at(1).x).abs() < 1e-9, "vertical lines");
        }
        let d = hatch(&square(10.0), &params(1.0, 45.0, 0.0, false));
        assert!(d.len() >= 13 && d.len() <= 15, "{} diagonal lines", d.len());
        // The same lattice for two different regions: line positions coincide.
        let a = hatch(&square(10.0), &params(0.7, 30.0, 0.1, false));
        let b = hatch(
            &Region::from_loops(
                [rect(Point::new(3.0, 3.0), Point::new(8.0, 8.0))],
                LoopRule::NonZero,
            ),
            &params(0.7, 30.0, 0.1, false),
        );
        let (s, c) = (30f64.to_radians().sin(), 30f64.to_radians().cos());
        let lane = |l: &Pline| ((-s * l.at(0).x + c * l.at(0).y - 0.1) / 0.7).round() as i64;
        let lanes_a: std::collections::HashSet<i64> = a.iter().map(lane).collect();
        for l in &b {
            assert!(lanes_a.contains(&lane(l)));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn every_segment_lies_inside(x in -5.0..5.0f64, y in -5.0..5.0f64, w in 1.0..8.0f64, h in 1.0..8.0f64,
                                     r in 0.3..2.0f64, angle in 0.0..180.0f64, pitch in 0.2..0.9f64, phase in 0.0..1.0f64) {
            let region = Region::from_loops([rect(Point::new(x, y), Point::new(x + w, y + h))], LoopRule::NonZero)
                .difference(&Region::from_loops([circle(Point::new(x + w / 2.0, y + h / 2.0), r.min(w.min(h) / 2.5))], LoopRule::NonZero));
            let lines = hatch(&region, &params(pitch, angle, phase, true));
            let total: f64 = lines.iter().map(pline::length).sum();
            // Length is bounded by area / pitch plus one extra row.
            prop_assert!(total <= region.area() / pitch + w.max(h) * 2.0 + 1e-6, "total {total} area {}", region.area());
            for l in &lines {
                let (a, b) = (l.at(0), l.at(1));
                for t in [0.05, 0.5, 0.95] {
                    let p = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
                    prop_assert!(region.contains(p), "segment leaves the region at {:?}", p);
                }
            }
        }
    }
}
