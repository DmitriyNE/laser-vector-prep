//! Parallel hatch: equally spaced scanlines clipped exactly to a region and its holes.
//!
//! Lines lie at `phase + k * pitch` in a frame rotated by `angle` about the document
//! origin, so two layers with the same parameters share one lattice. Every row is split
//! into separate segments wherever it leaves the filled area: nothing bridges a hole.
//!
//! Linked rows: where a segment ends on the boundary and the next boundary point along the
//! loop is the start of a segment in the next row, a stretch of the boundary at most
//! `link_mm` long joins the two into one path (beam-on, as a zigzag fill draws it). The
//! link runs on the boundary, so it never crosses a hole or leaves the region; it stops at
//! every other row end, so it never runs past one.

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
    /// Join consecutive rows along the boundary where the link is at most this long.
    pub link_mm: Option<f64>,
}

/// Scanlines hitting a vertex exactly would be counted twice; nudge them off by this much.
const VERTEX_NUDGE: f64 = 1e-7;

/// A frame rotated by an angle about the document origin.
#[derive(Clone, Copy)]
pub(crate) struct Frame {
    sin: f64,
    cos: f64,
}

impl Frame {
    pub(crate) fn new(angle_rad: f64) -> Frame {
        let (sin, cos) = angle_rad.sin_cos();
        Frame { sin, cos }
    }

    /// From frame coordinates back to the document.
    pub(crate) fn back(&self, x: f64, y: f64) -> Point {
        Point::new(self.cos * x - self.sin * y, self.sin * x + self.cos * y)
    }
}

/// Where a row crosses the boundary: x in the rotated frame, the loop it crosses and the
/// arc length from that loop's first vertex to the crossing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Crossing {
    pub x: f64,
    pub lp: usize,
    pub at: f64,
}

/// One scanline: its lattice index, its y in the rotated frame, and its crossings in order
/// of x; consecutive pairs bound the filled spans.
pub(crate) struct Row {
    pub k: i64,
    pub y: f64,
    pub crossings: Vec<Crossing>,
}

impl Row {
    /// The filled spans of the row, left to right, without zero-length ones.
    pub(crate) fn spans(&self) -> impl Iterator<Item = (Crossing, Crossing)> + '_ {
        let (pairs, _) = self.crossings.as_chunks::<2>();
        pairs
            .iter()
            .map(|[a, b]| (*a, *b))
            .filter(|(a, b)| b.x - a.x > 1e-9)
    }
}

/// The scanlines at `phase + k * pitch` across the region in the frame rotated by `angle`.
pub(crate) fn rows(region: &Region, angle_rad: f64, pitch: f64, phase: f64) -> Vec<Row> {
    if region.is_empty() || pitch.is_nan() || pitch <= 0.0 {
        return Vec::new();
    }
    let rotated: Vec<Pline> = region
        .loops
        .iter()
        .map(|l| pline::rotated(l, -angle_rad))
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
    // Arc length from each loop's first vertex to the start of each of its segments.
    let starts: Vec<Vec<f64>> = rotated
        .iter()
        .map(|l| {
            let mut acc = 0.0;
            l.iter_segments()
                .map(|(v1, v2)| {
                    let s = acc;
                    acc += cavalier_contours::polyline::seg_length(v1, v2);
                    s
                })
                .collect()
        })
        .collect();
    let boxes: Vec<_> = rotated.iter().map(pline::bounds).collect();

    let k0 = ((bounds.min.y - phase) / pitch).ceil() as i64;
    let k1 = ((bounds.max.y - phase) / pitch).floor() as i64;
    let mut rows = Vec::new();
    for k in k0..=k1 {
        let mut y = phase + k as f64 * pitch;
        // Avoid exact vertex hits (binary search in the sorted vertex ys).
        let i = vertex_ys.partition_point(|v| *v < y - VERTEX_NUDGE);
        if vertex_ys
            .get(i)
            .is_some_and(|v| (v - y).abs() <= VERTEX_NUDGE)
        {
            y += 2.0 * VERTEX_NUDGE;
        }
        let mut crossings: Vec<Crossing> = Vec::new();
        for (lp, l) in rotated.iter().enumerate() {
            let Some(b) = boxes[lp] else { continue };
            if y < b.min.y || y > b.max.y {
                continue;
            }
            for (si, (v1, v2)) in l.iter_segments().enumerate() {
                crossings_of(v1.x, v1.y, v2.x, v2.y, v1.bulge, y, &mut |x, along| {
                    crossings.push(Crossing {
                        x,
                        lp,
                        at: starts[lp][si] + along,
                    })
                });
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_by(|a, b| a.x.total_cmp(&b.x));
        rows.push(Row { k, y, crossings });
    }
    rows
}

pub fn hatch(region: &Region, p: &HatchParams) -> Vec<Pline> {
    let angle = p.angle_deg.to_radians();
    let rows = rows(region, angle, p.pitch_mm, p.phase_mm);
    let frame = Frame::new(angle);
    if let Some(max) = p.link_mm.filter(|m| *m > 0.0) {
        return linked(region, &rows, frame, max);
    }
    let mut out = Vec::new();
    for (row, r) in rows.iter().enumerate() {
        let reverse = p.serpentine && row % 2 == 1;
        let mut segs: Vec<(f64, f64)> = r.spans().map(|(a, b)| (a.x, b.x)).collect();
        if reverse {
            segs.reverse();
        }
        for (a, b) in segs {
            let (from, to) = if reverse { (b, a) } else { (a, b) };
            out.push(pline::from_points(
                [frame.back(from, r.y), frame.back(to, r.y)],
                false,
            ));
        }
    }
    out
}

/// Rows joined along the boundary into zigzag paths (see the module notes).
fn linked(region: &Region, rows: &[Row], frame: Frame, max: f64) -> Vec<Pline> {
    // Spans in row order: (row, start crossing, end crossing).
    let spans: Vec<(usize, Crossing, Crossing)> = rows
        .iter()
        .enumerate()
        .flat_map(|(r, row)| row.spans().map(move |(a, b)| (r, a, b)))
        .collect();
    // Every span end on each loop, in order along the loop: (arc length, span, which end).
    let mut on_loop: Vec<Vec<(f64, usize, usize)>> = vec![Vec::new(); region.loops.len()];
    for (s, (_, a, b)) in spans.iter().enumerate() {
        on_loop[a.lp].push((a.at, s, 0));
        on_loop[b.lp].push((b.at, s, 1));
    }
    for ends in &mut on_loop {
        ends.sort_by(|x, y| x.0.total_cmp(&y.0));
    }
    // Where each span end sits in its loop's list.
    let mut slot = vec![[0usize; 2]; spans.len()];
    for ends in &on_loop {
        for (i, &(_, s, e)) in ends.iter().enumerate() {
            slot[s][e] = i;
        }
    }
    let lengths: Vec<f64> = region.loops.iter().map(pline::length).collect();
    let crossing = |s: usize, e: usize| if e == 0 { spans[s].1 } else { spans[s].2 };
    let point = |s: usize, e: usize| frame.back(crossing(s, e).x, rows[spans[s].0].y);

    let mut used = vec![false; spans.len()];
    let mut out = Vec::new();
    for first in 0..spans.len() {
        if used[first] {
            continue;
        }
        let (mut s, mut entry) = (first, 0usize);
        let mut path = Pline::new();
        loop {
            used[s] = true;
            let exit = 1 - entry;
            pline::append(
                &mut path,
                &pline::from_points([point(s, entry), point(s, exit)], false),
            );
            // The boundary neighbours of the exit, either way along its loop.
            let c = crossing(s, exit);
            let ends = &on_loop[c.lp];
            let n = ends.len();
            let len = lengths[c.lp];
            let here = slot[s][exit];
            let next_row = rows[spans[s].0].k + 1;
            let mut best: Option<(f64, usize, usize, bool)> = None;
            for forward in [true, false] {
                let (at, t, te) = ends[if forward {
                    (here + 1) % n
                } else {
                    (here + n - 1) % n
                }];
                if t == s || used[t] || rows[spans[t].0].k != next_row {
                    continue;
                }
                let run = if forward {
                    (at - c.at).rem_euclid(len)
                } else {
                    (c.at - at).rem_euclid(len)
                };
                if run <= max && best.is_none_or(|b| run < b.0) {
                    best = Some((run, t, te, forward));
                }
            }
            let Some((_, t, te, forward)) = best else {
                break;
            };
            let to = crossing(t, te).at;
            let link = if forward {
                pline::sub_path(&region.loops[c.lp], c.at, to)
            } else {
                pline::sub_path(&region.loops[c.lp], to, c.at).map(|mut l| {
                    cavalier_contours::polyline::PlineSourceMut::invert_direction_mut(&mut l);
                    l
                })
            };
            if let Some(link) = link {
                pline::append(&mut path, &link);
            }
            (s, entry) = (t, te);
        }
        out.push(path);
    }
    out
}

/// Points of a lattice clipped to the region: rows `pitch_y` apart at `phase + k * pitch_y`,
/// points `pitch` apart along them at `phase + m * pitch`, every odd row shifted by `shift`.
/// In document coordinates, rows alternating direction.
pub(crate) fn lattice(
    region: &Region,
    angle_rad: f64,
    pitch: f64,
    pitch_y: f64,
    phase: f64,
    shift: f64,
) -> Vec<Point> {
    let frame = Frame::new(angle_rad);
    let mut out = Vec::new();
    for (n, row) in rows(region, angle_rad, pitch_y, phase).iter().enumerate() {
        let offset = phase + if row.k.rem_euclid(2) == 1 { shift } else { 0.0 };
        let mut here = Vec::new();
        for (a, b) in row.spans() {
            let m0 = ((a.x - offset) / pitch).ceil() as i64;
            let m1 = ((b.x - offset) / pitch).floor() as i64;
            for m in m0..=m1 {
                here.push(frame.back(offset + m as f64 * pitch, row.y));
            }
        }
        if n % 2 == 1 {
            here.reverse();
        }
        out.extend(here);
    }
    out
}

/// Each crossing of the horizontal line at `y` with the segment `(x1,y1)->(x2,y2)` of
/// `bulge`: its x and its arc length from the segment start. Half-open at the upper
/// endpoint so shared vertices count once.
fn crossings_of(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    bulge: f64,
    y: f64,
    found: &mut impl FnMut(f64, f64),
) {
    match Arc::from_bulge(Point::new(x1, y1), Point::new(x2, y2), bulge) {
        None => {
            if y1 == y2 {
                return; // horizontal: no crossing (its endpoints are counted by neighbours)
            }
            let (lo, hi) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
            if y >= lo && y < hi {
                let t = (y - y1) / (y2 - y1);
                found(x1 + t * (x2 - x1), t * (x2 - x1).hypot(y2 - y1));
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
                let turned = if arc.sweep >= 0.0 {
                    (ang - arc.start_angle).rem_euclid(std::f64::consts::TAU)
                } else {
                    (arc.start_angle - ang).rem_euclid(std::f64::consts::TAU)
                };
                // Past either end within rounding: clamp to the nearer one.
                let sweep = arc.sweep.abs();
                let turned = if turned <= sweep {
                    turned
                } else if turned - sweep < std::f64::consts::TAU - turned {
                    sweep
                } else {
                    0.0
                };
                found(x, turned * arc.radius);
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
            link_mm: None,
        }
    }

    fn link_params(pitch: f64, angle: f64, link: f64) -> HatchParams {
        HatchParams {
            link_mm: Some(link),
            ..params(pitch, angle, 0.25, true)
        }
    }

    #[test]
    fn linked_rows_of_a_square_zigzag_along_its_sides() {
        // Ten rows 1 mm apart joined by nine 1 mm stretches of the square's sides.
        let lines = hatch(&square(10.0), &link_params(1.0, 0.0, 1.5));
        assert_eq!(lines.len(), 1);
        assert!((pline::length(&lines[0]) - (100.0 + 9.0)).abs() < 1e-9);
        // At 45° the sides between rows are √2 mm: too long for a 1.2 mm limit.
        let plain = hatch(&square(10.0), &params(1.0, 45.0, 0.25, true));
        assert_eq!(
            hatch(&square(10.0), &link_params(1.0, 45.0, 1.2)).len(),
            plain.len()
        );
        let diagonal = hatch(&square(10.0), &link_params(1.0, 45.0, 1.5));
        assert!(diagonal.len() <= 2, "{} paths", diagonal.len());
    }

    #[test]
    fn links_follow_the_boundary_and_never_cross_a_hole() {
        let region = square(20.0).difference(&Region::from_loops(
            [circle(Point::new(10.0, 10.0), 4.0)],
            LoopRule::NonZero,
        ));
        let plain = hatch(&region, &params(0.5, 0.0, 0.25, true));
        let lines = hatch(&region, &link_params(0.5, 0.0, 2.0));
        assert!(
            lines.len() < plain.len() / 4,
            "{} of {} paths",
            lines.len(),
            plain.len()
        );
        // Every link is on the boundary: the paths stay in the region and run along it.
        let grown = region.offset(1e-6);
        for l in &lines {
            for (a, b) in l.iter_segments() {
                let m = cavalier_contours::polyline::seg_midpoint(a, b);
                assert!(grown.contains(Point::new(m.x, m.y)), "outside at {m:?}");
            }
        }
        let total = |ls: &[Pline]| ls.iter().map(pline::length).sum::<f64>();
        let joins = plain.len() - lines.len();
        assert!(total(&lines) > total(&plain));
        assert!(total(&lines) <= total(&plain) + joins as f64 * 2.0 + 1e-9);
    }

    #[test]
    fn lattice_points_sit_on_the_global_grid() {
        // A 10 mm square, pitch 1, phase 0.5: 10 x 10 points at half-integers.
        let pts = lattice(&square(10.0), 0.0, 1.0, 1.0, 0.5, 0.0);
        assert_eq!(pts.len(), 100);
        for p in &pts {
            assert!(((p.x - 0.5) - (p.x - 0.5).round()).abs() < 1e-9);
            assert!(((p.y - 0.5) - (p.y - 0.5).round()).abs() < 1e-9);
        }
        // Hex rows: 0.866 apart, odd rows shifted half a pitch.
        let hex = lattice(&square(10.0), 0.0, 1.0, 3f64.sqrt() / 2.0, 0.0, 0.5);
        let rows: std::collections::BTreeSet<i64> = hex
            .iter()
            .map(|p| (p.y / (3f64.sqrt() / 2.0)).round() as i64)
            .collect();
        assert_eq!(rows.len(), 12);
        let grown = square(10.0).offset(1e-6);
        assert!(hex.iter().all(|p| grown.contains(*p)));
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
            // Linked: the same rows, plus links on the boundary.
            let joined = hatch(&region, &HatchParams { link_mm: Some(3.0 * pitch), ..params(pitch, angle, phase, true) });
            let joined_total: f64 = joined.iter().map(pline::length).sum();
            prop_assert!(joined.len() <= lines.len());
            prop_assert!(joined_total >= total - 1e-9);
            prop_assert!(joined_total <= total + (lines.len() - joined.len()) as f64 * 3.0 * pitch + 1e-6);
            let grown = region.offset(1e-6);
            for l in &joined {
                for (a, b) in l.iter_segments() {
                    let m = cavalier_contours::polyline::seg_midpoint(a, b);
                    prop_assert!(grown.contains(Point::new(m.x, m.y)), "link outside at {:?}", m);
                }
            }
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
