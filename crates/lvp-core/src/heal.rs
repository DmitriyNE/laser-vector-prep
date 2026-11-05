//! Healing linework: open paths whose ends meet, within a tolerance, joined into longer
//! paths and closed loops (D40). CAD often draws one outline as separate lines and arcs in
//! either direction, and rounds their ends apart by a few nanometres (KiCad's Gerber
//! profiles); a region needs the closed loop they describe.
//!
//! An end is joined only to the one other end within the tolerance, and only when that end
//! has no other candidate either: junctions and doubled edges are left as they are and
//! counted. Gaps up to [`SNAP_MM`] close by moving both ends to their midpoint; wider gaps
//! (a tolerance the user raised) get a straight bridge, so arcs keep their shape.

use cavalier_contours::polyline::{PlineSource, PlineSourceMut, PlineVertex};
use cavalier_contours::static_aabb2d_index::{
    Control, NeighborPriorityQueue, StaticAABB2DIndex, StaticAABB2DIndexBuilder,
};

use crate::geom::Point;
use crate::pline::Pline;

/// Default tolerance of a heal layer, and the largest gap closed by moving the ends rather
/// than bridging them.
pub const SNAP_MM: f64 = 0.001;

/// What healing did, and what it left open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealReport {
    /// Gaps closed: pairs of ends joined, including the two ends of a path closed on itself.
    pub joints: usize,
    /// Widest gap closed (mm).
    pub largest_gap: f64,
    /// Closed loops made from open paths.
    pub closed: usize,
    /// The ends of the open paths left, in output order.
    pub loose_ends: Vec<Point>,
    /// Ends left loose because more than one other end lay within the tolerance (junctions).
    pub ambiguous: usize,
    /// Distance between the two nearest loose ends away from junctions (mm): the tolerance
    /// that would join them.
    pub nearest_loose: Option<f64>,
}

/// `paths` with the open ones joined where their ends meet within `tolerance` (mm). Closed
/// paths and single points pass through unchanged; every output path takes the place of the
/// first input path it holds.
pub fn heal(paths: Vec<Pline>, tolerance: f64) -> (Vec<Pline>, HealReport) {
    let mut report = HealReport::default();
    // Output paths with the input position they take and, for open ones made here, their
    // two ends.
    let mut out: Vec<(usize, Pline, Option<[usize; 2]>)> = Vec::new();
    // Open paths with their input positions.
    let mut open: Vec<(usize, Pline)> = Vec::new();
    for (i, p) in paths.into_iter().enumerate() {
        if p.is_closed() || p.vertex_count() < 2 {
            out.push((i, p, None));
        } else {
            open.push((i, p));
        }
    }
    // End 2k is the start of open path k, end 2k + 1 its end.
    let ends: Vec<Point> = open.iter().flat_map(|(_, p)| [first(p), last(p)]).collect();
    let (partner, junction) = pair_ends(&ends, tolerance);
    report.ambiguous = junction.iter().filter(|j| **j).count();
    let mut used = vec![false; open.len()];
    // Walk from path `e / 2`, entered at end `e`, on through partnered ends to the end it
    // leaves by; a walk that comes back to its first path is a cycle.
    let walk = |e: usize, used: &mut [bool], report: &mut HealReport| {
        let (start, path) = &open[e / 2];
        let mut at = *start;
        let mut acc = oriented(path, e % 2 == 0);
        used[e / 2] = true;
        let mut exit = e ^ 1;
        while let Some(next) = partner[exit] {
            if used[next / 2] {
                return (at, acc, exit, next == e);
            }
            let (i, path) = &open[next / 2];
            join(&mut acc, &oriented(path, next % 2 == 0), report);
            used[next / 2] = true;
            at = at.min(*i);
            exit = next ^ 1;
        }
        (at, acc, exit, false)
    };
    // Chains first, each from its loose start, then the cycles that remain.
    for e in 0..ends.len() {
        if !used[e / 2] && partner[e].is_none() {
            let (at, path, exit, _) = walk(e, &mut used, &mut report);
            out.push((at, path, Some([e, exit])));
        }
    }
    for k in 0..open.len() {
        if !used[k] {
            let (at, mut path, exit, cycle) = walk(2 * k, &mut used, &mut report);
            if cycle {
                close(&mut path, tolerance, &mut report);
            }
            let loose = (!path.is_closed()).then_some([2 * k, exit]);
            out.push((at, path, loose));
        }
    }
    out.sort_by_key(|(at, ..)| *at);
    // Loose ends at junctions are no gap a larger tolerance would close.
    let mut gaps = Vec::new();
    for e in out.iter().filter_map(|(.., loose)| *loose).flatten() {
        report.loose_ends.push(ends[e]);
        if !junction[e] {
            gaps.push(ends[e]);
        }
    }
    report.nearest_loose = nearest_pair(&gaps);
    (out.into_iter().map(|(_, p, _)| p).collect(), report)
}

fn first(p: &Pline) -> Point {
    let v = p.at(0);
    Point::new(v.x, v.y)
}

fn last(p: &Pline) -> Point {
    let v = p.at(p.vertex_count() - 1);
    Point::new(v.x, v.y)
}

fn oriented(p: &Pline, forward: bool) -> Pline {
    let mut q = p.clone();
    if !forward {
        q.invert_direction_mut();
    }
    q
}

/// For every end, the one other end it joins: the only other end within `tolerance`, when
/// that end has no other candidate either (the other end of its own path closes it on
/// itself). Second, whether an end without a partner had candidates (a junction).
fn pair_ends(ends: &[Point], tolerance: f64) -> (Vec<Option<usize>>, Vec<bool>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Near {
        None,
        One(usize),
        Many,
    }
    let Some(index) = point_index(ends) else {
        return (vec![None; ends.len()], vec![false; ends.len()]);
    };
    let tol2 = tolerance * tolerance;
    let mut queue = NeighborPriorityQueue::new();
    let near: Vec<Near> = (0..ends.len())
        .map(|e| {
            let mut found = Near::None;
            let mut visit = |i: usize, d2: f64| {
                if d2 > tol2 {
                    return Control::Break(());
                }
                if i == e {
                    return Control::Continue;
                }
                if found != Near::None {
                    found = Near::Many;
                    return Control::Break(());
                }
                found = Near::One(i);
                Control::Continue
            };
            index.visit_neighbors_with_queue(ends[e].x, ends[e].y, &mut visit, &mut queue);
            found
        })
        .collect();
    (0..ends.len())
        .map(|e| match near[e] {
            Near::One(f) if near[f] == Near::One(e) => (Some(f), false),
            Near::None => (None, false),
            _ => (None, true),
        })
        .unzip()
}

fn point_index(points: &[Point]) -> Option<StaticAABB2DIndex<f64>> {
    let mut b = StaticAABB2DIndexBuilder::new(points.len());
    for p in points {
        b.add(p.x, p.y, p.x, p.y);
    }
    b.build().ok()
}

/// Append `next` to `acc` across the gap between them: snapped at the midpoint up to
/// [`SNAP_MM`], bridged by a straight segment beyond.
fn join(acc: &mut Pline, next: &Pline, report: &mut HealReport) {
    let (a, b) = (last(acc), first(next));
    let gap = a.distance(b);
    let n = acc.vertex_count();
    // An open path's last bulge describes no segment and may hold anything.
    let (at, bulge, from) = if gap <= SNAP_MM {
        (
            Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
            next.at(0).bulge,
            1,
        )
    } else {
        (a, 0.0, 0)
    };
    acc.set_vertex(n - 1, PlineVertex::new(at.x, at.y, bulge));
    for v in next.iter_vertexes().skip(from) {
        acc.add_vertex(v);
    }
    report.joints += 1;
    report.largest_gap = report.largest_gap.max(gap);
}

/// Close `p`, whose ends meet within the tolerance, unless the loop would enclose nothing
/// (a piece drawn there and back): that stays an open path.
fn close(p: &mut Pline, tolerance: f64, report: &mut HealReport) {
    let (a, b) = (first(p), last(p));
    let gap = a.distance(b);
    let mut q = p.clone();
    let n = q.vertex_count();
    if gap <= SNAP_MM {
        let v = q.at(0);
        q.set_vertex(
            0,
            PlineVertex::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, v.bulge),
        );
        q.remove_last();
    } else {
        q.set_vertex(n - 1, PlineVertex::new(b.x, b.y, 0.0));
    }
    q.set_is_closed(true);
    if q.vertex_count() < 2 || q.area().abs() <= tolerance * tolerance {
        return;
    }
    *p = q;
    report.joints += 1;
    report.largest_gap = report.largest_gap.max(gap);
    report.closed += 1;
}

/// Distance between the two nearest of `points`.
fn nearest_pair(points: &[Point]) -> Option<f64> {
    let index = point_index(points)?;
    let mut queue = NeighborPriorityQueue::new();
    let mut best = f64::INFINITY;
    for (k, p) in points.iter().enumerate() {
        let mut visit = |i: usize, d2: f64| {
            if d2 >= best * best {
                return Control::Break(());
            }
            if i != k {
                best = d2.sqrt();
                return Control::Break(());
            }
            Control::Continue
        };
        index.visit_neighbors_with_queue(p.x, p.y, &mut visit, &mut queue);
    }
    best.is_finite().then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn path(pts: &[(f64, f64, f64)]) -> Pline {
        let mut p = Pline::new();
        for &(x, y, b) in pts {
            p.add(x, y, b);
        }
        p
    }

    /// The heater's profile as KiCad 10 writes it: eight draws, arcs all clockwise, two
    /// joints 9 and 9.5 nm apart.
    fn heater_profile() -> Vec<Pline> {
        vec![
            path(&[(7.698113, 24.310266, -0.327), (7.000009, 25.263610, 0.0)]),
            path(&[(7.0, 25.263610, 0.0), (7.0, 35.5, 0.0)]),
            path(&[(6.0, 36.5, -0.41421356), (7.0, 35.5, 0.0)]),
            path(&[(6.0, 36.5, 0.0), (-6.0, 36.5, 0.0)]),
            path(&[(-7.0, 35.5, -0.41421356), (-6.0, 36.5, 0.0)]),
            path(&[(-7.0, 35.5, 0.0), (-7.0, 25.263610, 0.0)]),
            path(&[(-7.0, 25.263610, -0.327), (-7.698110, 24.310257, 0.0)]),
            path(&[
                (7.698113, 24.310266, -0.8573),
                (0.0, -25.5, -0.8573),
                (-7.698113, 24.310266, 0.0),
            ]),
        ]
    }

    #[test]
    fn pieces_in_any_order_and_direction_close_across_nanometre_gaps() {
        let (out, r) = heal(heater_profile(), SNAP_MM);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].is_closed());
        assert_eq!(out[0].vertex_count(), 9);
        assert_eq!((r.joints, r.closed, r.ambiguous), (8, 1, 0));
        assert!((r.largest_gap - 9.487e-6).abs() < 1e-8, "{}", r.largest_gap);
        assert!(r.loose_ends.is_empty() && r.nearest_loose.is_none());
    }

    #[test]
    fn gaps_beyond_the_tolerance_stay_open_and_report_the_nearest_ends() {
        let (out, r) = heal(heater_profile(), 5e-6);
        // Two chains, broken at the two 9-ish nm joints.
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|p| !p.is_closed()));
        assert_eq!(r.loose_ends.len(), 4);
        assert!((r.nearest_loose.unwrap() - 9e-6).abs() < 1e-9);
    }

    #[test]
    fn exact_tolerance_still_joins_pieces_that_share_their_ends() {
        let square = vec![
            path(&[(0.0, 0.0, 0.0), (2.0, 0.0, 0.0)]),
            path(&[(2.0, 2.0, 0.0), (0.0, 2.0, 0.0)]),
            path(&[(2.0, 2.0, 0.0), (2.0, 0.0, 0.0)]),
            path(&[(0.0, 2.0, 0.0), (0.0, 0.0, 0.0)]),
        ];
        let (out, r) = heal(square, crate::pline::REPEAT_EPS);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_closed() && (out[0].area().abs() - 4.0).abs() < 1e-12);
        assert_eq!(r.largest_gap, 0.0);
    }

    #[test]
    fn junctions_are_left_alone_and_counted() {
        // A T: three lines meeting at (1, 0); and a chain of two lines, which joins.
        let (out, r) = heal(
            vec![
                path(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)]),
                path(&[(1.0, 0.0, 0.0), (2.0, 0.0, 0.0)]),
                path(&[(1.0, 0.0, 0.0), (1.0, 1.0, 0.0)]),
                path(&[(5.0, 0.0, 0.0), (6.0, 0.0, 0.0)]),
                path(&[(7.0, 1.0, 0.0), (6.0, 0.0, 0.0)]),
            ],
            SNAP_MM,
        );
        assert_eq!(out.len(), 4);
        assert_eq!(r.ambiguous, 3);
        assert_eq!(r.joints, 1);
        // The three ends at the junction coincide, but are no gap to close.
        assert_eq!(r.loose_ends.len(), 8);
        assert_eq!(r.nearest_loose, Some(2f64.sqrt()));
        let chain = out.iter().find(|p| p.vertex_count() == 3).unwrap();
        assert_eq!(
            (first(chain), last(chain)),
            (Point::new(5.0, 0.0), Point::new(7.0, 1.0))
        );
    }

    #[test]
    fn a_raised_tolerance_bridges_wider_gaps_with_straight_segments() {
        // A half-disc whose diameter stops 0.05 mm short of the arc at both ends.
        let pieces = vec![
            path(&[(1.0, 0.0, 1.0), (-1.0, 0.0, 0.0)]),
            path(&[(-0.95, 0.0, 0.0), (0.95, 0.0, 0.0)]),
        ];
        // The trailing bulges of open paths describe no segment: the bridges stay straight.
        let pieces: Vec<Pline> = pieces
            .into_iter()
            .map(|mut p| {
                let n = p.vertex_count();
                let v = p.at(n - 1);
                p.set_vertex(n - 1, PlineVertex::new(v.x, v.y, 0.7));
                p
            })
            .collect();
        let (out, r) = heal(pieces.clone(), 0.01);
        assert_eq!(out.len(), 2);
        assert_eq!(r.loose_ends.len(), 4);
        let (out, r) = heal(pieces, 0.1);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_closed());
        // The arc keeps its shape: the area is the exact half disc.
        assert!((out[0].area().abs() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(out[0].vertex_count(), 4);
        assert!((r.largest_gap - 0.05).abs() < 1e-12);
    }

    #[test]
    fn a_path_closes_on_itself_but_one_drawn_there_and_back_stays_open() {
        let (out, r) = heal(
            vec![path(&[
                (0.0, 0.0, 0.0),
                (1.0, 0.0, 0.0),
                (1.0, 1.0, 0.0),
                (0.0, 0.0004, 0.0),
            ])],
            SNAP_MM,
        );
        assert!(out[0].is_closed() && out[0].vertex_count() == 3);
        assert_eq!((r.joints, r.closed), (1, 1));
        let (out, r) = heal(
            vec![
                path(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0)]),
                path(&[(1.0, 0.0, 0.0), (0.0, 0.0, 0.0)]),
            ],
            SNAP_MM,
        );
        assert_eq!(out.len(), 1);
        assert!(!out[0].is_closed());
        assert_eq!((r.closed, r.loose_ends.len()), (0, 2));
    }

    #[test]
    fn closed_paths_and_points_pass_through() {
        let mut ring = path(&[(0.0, 0.0, 1.0), (1.0, 0.0, 1.0)]);
        ring.set_is_closed(true);
        let dot = path(&[(3.0, 3.0, 0.0)]);
        let (out, r) = heal(vec![ring.clone(), dot.clone()], SNAP_MM);
        assert_eq!(out.len(), 2);
        assert!(out[0].is_closed() && out[0].vertex_data == ring.vertex_data);
        assert!(!out[1].is_closed() && out[1].vertex_data == dot.vertex_data);
        assert_eq!(r, HealReport::default());
    }

    proptest! {
        /// A polygon of lines and arcs cut into one piece per segment, the pieces shuffled,
        /// some reversed, and every joint pulled apart by less than the tolerance, heals
        /// back into one closed loop of the same area.
        #[test]
        fn shuffled_pieces_of_a_loop_heal_back_into_it(
            n in 3usize..12,
            radii in prop::collection::vec(1.0f64..10.0, 12),
            bulges in prop::collection::vec(-0.3f64..0.3, 12),
            jitter in prop::collection::vec((0.0f64..0.45, 0.0f64..std::f64::consts::TAU), 24),
            flips in prop::collection::vec(any::<bool>(), 12),
            order in Just((0..12).collect::<Vec<usize>>()).prop_shuffle(),
        ) {
            let tol = SNAP_MM;
            let pts: Vec<Point> = (0..n)
                .map(|i| {
                    let t = std::f64::consts::TAU * i as f64 / n as f64;
                    Point::new(radii[i] * t.cos(), radii[i] * t.sin())
                })
                .collect();
            let mut original = Pline::new_closed();
            for i in 0..n {
                original.add(pts[i].x, pts[i].y, bulges[i]);
            }
            let moved = |p: Point, k: usize| {
                let (r, a) = jitter[k];
                Point::new(p.x + r * tol * a.cos(), p.y + r * tol * a.sin())
            };
            let mut pieces: Vec<Pline> = (0..n)
                .map(|i| {
                    let (a, b) = (moved(pts[i], 2 * i), moved(pts[(i + 1) % n], 2 * i + 1));
                    let p = path(&[(a.x, a.y, bulges[i]), (b.x, b.y, 0.0)]);
                    oriented(&p, !flips[i])
                })
                .collect();
            let order: Vec<usize> = order.into_iter().filter(|&k| k < n).collect();
            pieces = order.iter().map(|&k| pieces[k].clone()).collect();
            let (out, r) = heal(pieces, tol);
            prop_assert_eq!(out.len(), 1);
            prop_assert!(out[0].is_closed());
            prop_assert_eq!(out[0].vertex_count(), n);
            prop_assert_eq!((r.joints, r.closed, r.ambiguous), (n, 1, 0));
            prop_assert!(r.largest_gap < 0.9 * tol);
            // Each vertex moves at most 0.45 µm; the area changes accordingly little.
            let perimeter = crate::pline::length(&original);
            prop_assert!(
                (out[0].area().abs() - original.area().abs()).abs() <= perimeter * 0.45 * tol,
                "{} vs {}", out[0].area(), original.area()
            );
        }
    }
}
