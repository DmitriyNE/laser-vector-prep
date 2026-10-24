//! Planar regions with exact arcs, and boolean operations on them.
//!
//! A [`Region`] is a set of closed, mutually non-crossing loops: counter-clockwise loops
//! bound filled area, clockwise loops bound holes. `cavalier_contours` only offers pairwise
//! loop booleans, so this module implements one general n-loop boolean on top of its
//! intersection and winding-number primitives:
//!
//! 1. split every loop at its intersections with every other loop,
//! 2. classify each slice by probing just left and right of its midpoint with the winding
//!    numbers of both operands (a slice is a result boundary iff exactly one side is inside),
//! 3. orient kept slices with the interior on the left and stitch them into loops.
//!
//! Offsets go through cavalier's `Shape` offsetting, which keeps arcs exact and handles
//! holes. Numerical policy: probe distance and join tolerance scale with the geometry's
//! extent and never drop below `geom::EPS_MM`-sized values (see `tolerances`).

use std::collections::HashMap;

use cavalier_contours::core::math::Vector2;
use cavalier_contours::polyline::{
    PlineCreation, PlineSource, PlineSourceMut, PlineViewData, seg_closest_point, seg_length,
    seg_tangent_vector,
};
use cavalier_contours::shape_algorithms::{Shape, ShapeOffsetOptions};

use crate::arc::Arc;
use crate::flatten::FlattenError;
use crate::geom::{BBox, Point};
use crate::pline::{self, Pline};

/// How a bag of closed source loops (whose orientation carries no meaning) becomes a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopRule {
    /// Nesting parity: a loop inside a loop is a hole; overlapping loops XOR.
    EvenOdd,
    /// Everything enclosed by any loop is filled; nested loops do not make holes.
    NonZero,
    /// CAD convention: loops whose boundaries cross are merged (union); the resulting
    /// non-crossing loops then nest by parity, so a loop strictly inside another is a hole.
    MergeThenNest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Intersection,
    Difference,
    Xor,
}

#[derive(Clone, Debug, Default)]
pub struct Region {
    pub loops: Vec<Pline>,
}

/// Tolerances derived from the extent of the geometry being operated on.
#[derive(Clone, Copy, Debug)]
struct Tolerances {
    /// Distance from a slice midpoint to its inside/outside probe points.
    probe: f64,
    /// Points closer than this are the same point (slice ends, duplicate splits).
    join: f64,
}

impl Tolerances {
    fn for_extent(extent: f64) -> Self {
        let extent = extent.max(1.0);
        Self {
            probe: (extent * 1e-9).max(1e-7),
            join: (extent * 1e-8).max(1e-6),
        }
    }
}

fn v2(p: Point) -> Vector2<f64> {
    Vector2::new(p.x, p.y)
}

fn pt(v: Vector2<f64>) -> Point {
    Point::new(v.x, v.y)
}

/// Loops that can participate in a boolean: closed, at least two vertices, non-degenerate.
fn is_usable(p: &Pline) -> bool {
    p.is_closed() && p.vertex_count() >= 2 && p.area().abs() > 1e-18
}

struct Tagged {
    pline: Pline,
    operand: u8,
    bbox: BBox,
}

fn tag(plines: &[Pline], operand: u8) -> Vec<Tagged> {
    plines
        .iter()
        .filter(|p| is_usable(p))
        .filter_map(|p| {
            Some(Tagged {
                pline: p.clone(),
                operand,
                bbox: pline::bounds(p)?,
            })
        })
        .collect()
}

fn bbox_overlap(a: &BBox, b: &BBox, pad: f64) -> bool {
    a.min.x - pad <= b.max.x
        && b.min.x - pad <= a.max.x
        && a.min.y - pad <= b.max.y
        && b.min.y - pad <= a.max.y
}

/// Sum of winding numbers of the loops of one operand around `p`.
fn winding(loops: &[Tagged], operand: u8, p: Point, pad: f64) -> i32 {
    loops
        .iter()
        .filter(|l| l.operand == operand && bbox_overlap(&l.bbox, &BBox::at(p), pad))
        .map(|l| l.pline.winding_number(v2(p)))
        .sum()
}

/// Split points on one loop, snapped onto their segments, sorted along the loop and
/// de-duplicated. Snapping matters: intersection points of near-tangent micro-arcs can sit
/// off their segment by more than the join tolerance, which would make the slice invalid.
fn sorted_splits(
    p: &Pline,
    splits: &mut [(usize, Vector2<f64>)],
    join: f64,
) -> Vec<(usize, Vector2<f64>)> {
    for sp in splits.iter_mut() {
        let i = sp.0;
        let j = if i + 1 < p.vertex_count() { i + 1 } else { 0 };
        sp.1 = seg_closest_point(p.at(i), p.at(j), sp.1, join);
    }
    // Chord distance from the segment start is monotonic along a segment (|bulge| <= 1).
    splits.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            let da = pt(a.1).distance(pt(p.at(a.0).pos()));
            let db = pt(b.1).distance(pt(p.at(b.0).pos()));
            da.total_cmp(&db)
        })
    });
    let mut out: Vec<(usize, Vector2<f64>)> = Vec::with_capacity(splits.len());
    for s in splits.iter() {
        if out.last().is_none_or(|l| pt(l.1).distance(pt(s.1)) > join) {
            out.push(*s);
        }
    }
    if out.len() > 1 && pt(out[0].1).distance(pt(out[out.len() - 1].1)) <= join {
        out.pop();
    }
    out
}

/// Midpoint (by path length) of an open or closed polyline and the left unit normal there.
fn midpoint_and_left_normal(p: &Pline) -> Option<(Point, Point)> {
    let total: f64 = p.iter_segments().map(|(a, b)| seg_length(a, b)).sum();
    if total.is_nan() || total <= 0.0 {
        return None;
    }
    let mut acc = 0.0;
    for (a, b) in p.iter_segments() {
        let len = seg_length(a, b);
        if acc + len >= total / 2.0 || acc + len >= total {
            return Some(pline::seg_midpoint_and_left_normal(a, b));
        }
        acc += len;
    }
    None
}

fn start_point(p: &Pline) -> Point {
    pt(p.at(0).pos())
}

fn end_point(p: &Pline) -> Point {
    pt(p.last().expect("non-empty").pos())
}

fn start_tangent(p: &Pline) -> Point {
    let (a, b) = (p.at(0), p.at(1));
    pt(seg_tangent_vector(a, b, a.pos()))
}

fn end_tangent(p: &Pline) -> Point {
    let n = p.vertex_count();
    let (a, b) = (p.at(n - 2), p.at(n - 1));
    pt(seg_tangent_vector(a, b, b.pos()))
}

/// Signed turn angle from direction `a` to direction `b`, positive = left.
fn turn(a: Point, b: Point) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
}

/// Stitch oriented open slices (plus already-closed whole loops) into closed loops.
fn stitch(slices: Vec<Pline>, join: f64) -> Vec<Pline> {
    let (closed, open): (Vec<Pline>, Vec<Pline>) = slices.into_iter().partition(|s| s.is_closed());
    let mut result = closed;
    if open.is_empty() {
        return result;
    }
    let cell = |p: Point| ((p.x / join).floor() as i64, (p.y / join).floor() as i64);
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in open.iter().enumerate() {
        by_start.entry(cell(start_point(s))).or_default().push(i);
    }
    let mut used = vec![false; open.len()];
    let candidates = |p: Point, used: &[bool]| -> Vec<usize> {
        let (cx, cy) = cell(p);
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = by_start.get(&(cx + dx, cy + dy)) {
                    for &i in ids {
                        if !used[i] && start_point(&open[i]).distance(p) <= join {
                            out.push(i);
                        }
                    }
                }
            }
        }
        out
    };

    for first in 0..open.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut chain = vec![first];
        let origin = start_point(&open[first]);
        let mut cur = first;
        let mut closed_ok = false;
        loop {
            let tail = end_point(&open[cur]);
            if tail.distance(origin) <= join {
                closed_ok = true;
                break;
            }
            let cands = candidates(tail, &used);
            let Some(&next) = (if cands.len() <= 1 {
                cands.first()
            } else {
                // Sharpest left turn keeps the interior on the left and splits pinch points.
                let t_in = end_tangent(&open[cur]);
                cands.iter().max_by(|&&a, &&b| {
                    turn(t_in, start_tangent(&open[a]))
                        .total_cmp(&turn(t_in, start_tangent(&open[b])))
                })
            }) else {
                break;
            };
            used[next] = true;
            chain.push(next);
            cur = next;
        }
        if !closed_ok {
            // A boundary slice set must always close; a dangling chain means the
            // classification or the intersections were inconsistent. Loud in debug builds,
            // dropped in release (the result is then incomplete rather than corrupt).
            debug_assert!(
                false,
                "boolean stitching left a dangling chain of {} slice(s) starting at {:?}",
                chain.len(),
                origin
            );
            continue;
        }
        let mut lp = Pline::new_closed();
        for &i in &chain {
            let s = &open[i];
            let n = s.vertex_count();
            for k in 0..n - 1 {
                let v = s.at(k);
                lp.add(v.x, v.y, v.bulge);
            }
        }
        if lp.vertex_count() >= 2 && lp.area().abs() > 1e-18 {
            result.push(lp);
        }
    }
    result
}

/// The general boolean. `inside(w_a, w_b)` decides membership from the winding numbers
/// of the two operands.
fn resolve(a: &[Pline], b: &[Pline], inside: &dyn Fn(i32, i32) -> bool) -> Vec<Pline> {
    let loops: Vec<Tagged> = tag(a, 0).into_iter().chain(tag(b, 1)).collect();
    if loops.is_empty() {
        return Vec::new();
    }
    let extent = loops
        .iter()
        .map(|l| l.bbox)
        .reduce(|x, y| x.union(y))
        .map(|bb| {
            bb.width()
                .max(bb.height())
                .max(bb.max.x.abs())
                .max(bb.max.y.abs())
        })
        .unwrap_or(1.0);
    let tol = Tolerances::for_extent(extent);

    // 1. Intersections -> split points per loop.
    let mut splits: Vec<Vec<(usize, Vector2<f64>)>> = vec![Vec::new(); loops.len()];
    for i in 0..loops.len() {
        for j in (i + 1)..loops.len() {
            if !bbox_overlap(&loops[i].bbox, &loops[j].bbox, tol.join) {
                continue;
            }
            let ix = loops[i].pline.find_intersects(&loops[j].pline);
            for bi in &ix.basic_intersects {
                splits[i].push((bi.start_index1, bi.point));
                splits[j].push((bi.start_index2, bi.point));
            }
            for ov in &ix.overlapping_intersects {
                for p in [ov.point1, ov.point2] {
                    splits[i].push((ov.start_index1, p));
                    splits[j].push((ov.start_index2, p));
                }
            }
        }
    }

    // 2. Slices.
    let mut slices: Vec<Pline> = Vec::new();
    for (i, l) in loops.iter().enumerate() {
        let pts = sorted_splits(&l.pline, &mut splits[i], tol.join);
        if pts.len() < 2 {
            slices.push(l.pline.clone());
            continue;
        }
        for k in 0..pts.len() {
            let (si, sp) = pts[k];
            let (ei, ep) = pts[(k + 1) % pts.len()];
            if let Some(vd) = PlineViewData::from_slice_points(&l.pline, sp, si, ep, ei, tol.join) {
                let s = Pline::create_from(&vd.view(&l.pline));
                if s.vertex_count() >= 2 && s.path_length() > 2.0 * tol.join {
                    slices.push(s);
                }
            }
        }
    }

    // 3. Classify and orient; drop coincident duplicates.
    let mut seen: HashMap<(i64, i64, i64, i64, i64, i64), ()> = HashMap::new();
    let q = |p: Point| {
        (
            (p.x / tol.join).round() as i64,
            (p.y / tol.join).round() as i64,
        )
    };
    let mut kept: Vec<Pline> = Vec::new();
    for mut s in slices {
        let Some((mid, n)) = midpoint_and_left_normal(&s) else {
            continue;
        };
        let left = Point::new(mid.x + n.x * tol.probe, mid.y + n.y * tol.probe);
        let right = Point::new(mid.x - n.x * tol.probe, mid.y - n.y * tol.probe);
        let pad = tol.probe * 2.0;
        let in_left = inside(winding(&loops, 0, left, pad), winding(&loops, 1, left, pad));
        let in_right = inside(
            winding(&loops, 0, right, pad),
            winding(&loops, 1, right, pad),
        );
        if in_left == in_right {
            continue;
        }
        if in_right {
            s.invert_direction_mut();
        }
        let (a, b, m) = (q(start_point(&s)), q(end_point(&s)), q(mid));
        let key = if s.is_closed() {
            (m.0, m.1, m.0, m.1, m.0, m.1)
        } else {
            (a.0, a.1, b.0, b.1, m.0, m.1)
        };
        if seen.insert(key, ()).is_none() {
            kept.push(s);
        }
    }

    // 4. Stitch.
    stitch(kept, tol.join)
}

impl Region {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }

    /// Build a region from source loops under `rule`. Loop orientation in the input is
    /// ignored (CAD files carry none); open or degenerate loops are dropped.
    pub fn from_loops<I: IntoIterator<Item = Pline>>(loops: I, rule: LoopRule) -> Region {
        let ccw: Vec<Pline> = loops
            .into_iter()
            .filter(is_usable)
            .map(pline::ensure_ccw)
            .collect();
        let loops = match rule {
            LoopRule::EvenOdd => resolve(&ccw, &[], &|a, _| a % 2 != 0),
            LoopRule::NonZero => resolve(&ccw, &[], &|a, _| a != 0),
            LoopRule::MergeThenNest => merge_then_nest(ccw),
        };
        Region { loops }
    }

    pub fn boolean(&self, other: &Region, op: BoolOp) -> Region {
        let f: &dyn Fn(i32, i32) -> bool = match op {
            BoolOp::Union => &|a, b| a > 0 || b > 0,
            BoolOp::Intersection => &|a, b| a > 0 && b > 0,
            BoolOp::Difference => &|a, b| a > 0 && b <= 0,
            BoolOp::Xor => &|a, b| (a > 0) != (b > 0),
        };
        Region {
            loops: resolve(&self.loops, &other.loops, f),
        }
    }

    pub fn union(&self, other: &Region) -> Region {
        self.boolean(other, BoolOp::Union)
    }

    pub fn intersection(&self, other: &Region) -> Region {
        self.boolean(other, BoolOp::Intersection)
    }

    pub fn difference(&self, other: &Region) -> Region {
        self.boolean(other, BoolOp::Difference)
    }

    pub fn xor(&self, other: &Region) -> Region {
        self.boolean(other, BoolOp::Xor)
    }

    /// Offset the filled area by `delta` (positive grows, negative shrinks). Reflex corners
    /// get exact round arcs; collapsed parts vanish.
    pub fn offset(&self, delta: f64) -> Region {
        if self.loops.is_empty() || delta == 0.0 {
            return self.clone();
        }
        let shape = Shape::from_plines(self.loops.iter().cloned());
        let off = shape.parallel_offset(-delta, &ShapeOffsetOptions::default());
        Region {
            loops: off
                .ccw_plines
                .into_iter()
                .chain(off.cw_plines)
                .map(|ip| ip.polyline)
                .filter(is_usable)
                .collect(),
        }
    }

    /// Signed area: positive loops minus holes.
    pub fn area(&self) -> f64 {
        self.loops.iter().map(|l| l.area()).sum()
    }

    pub fn bounds(&self) -> Option<BBox> {
        self.loops
            .iter()
            .filter_map(pline::bounds)
            .reduce(|a, b| a.union(b))
    }

    pub fn outers(&self) -> impl Iterator<Item = &Pline> {
        self.loops.iter().filter(|l| l.area() > 0.0)
    }

    pub fn holes(&self) -> impl Iterator<Item = &Pline> {
        self.loops.iter().filter(|l| l.area() < 0.0)
    }

    /// Split into connected islands: each outer loop with the holes directly inside it.
    pub fn islands(&self) -> Vec<Region> {
        let outers: Vec<&Pline> = self.outers().collect();
        let mut groups: Vec<Vec<Pline>> = outers.iter().map(|o| vec![(*o).clone()]).collect();
        for h in self.holes() {
            let probe = h.at(0).pos();
            // Smallest outer containing the hole.
            let owner = outers
                .iter()
                .enumerate()
                .filter(|(_, o)| o.winding_number(probe) != 0)
                .min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area()))
                .map(|(i, _)| i);
            if let Some(i) = owner {
                groups[i].push(h.clone());
            }
        }
        groups.into_iter().map(|loops| Region { loops }).collect()
    }

    /// Flatten every loop within `tolerance`.
    pub fn flatten(&self, tolerance: f64) -> Result<Vec<Vec<Point>>, FlattenError> {
        self.loops
            .iter()
            .map(|l| pline::flatten(l, tolerance))
            .collect()
    }

    /// Whether `p` lies in the filled area (holes excluded).
    pub fn contains(&self, p: Point) -> bool {
        self.loops
            .iter()
            .map(|l| l.winding_number(v2(p)))
            .sum::<i32>()
            > 0
    }
}

/// `LoopRule::MergeThenNest`: union crossing clusters, then nest by containment parity.
fn merge_then_nest(ccw: Vec<Pline>) -> Vec<Pline> {
    if ccw.is_empty() {
        return Vec::new();
    }
    let boxes: Vec<BBox> = ccw.iter().filter_map(pline::bounds).collect();
    let extent = boxes
        .iter()
        .copied()
        .reduce(|a, b| a.union(b))
        .map(|bb| bb.width().max(bb.height()))
        .unwrap_or(1.0);
    let tol = Tolerances::for_extent(extent);

    // Union-find over loops whose boundaries cross or overlap.
    let mut parent: Vec<usize> = (0..ccw.len()).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..ccw.len() {
        for j in (i + 1)..ccw.len() {
            if !bbox_overlap(&boxes[i], &boxes[j], tol.join) {
                continue;
            }
            let ix = ccw[i].find_intersects(&ccw[j]);
            if !ix.basic_intersects.is_empty() || !ix.overlapping_intersects.is_empty() {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut clusters: HashMap<usize, Vec<Pline>> = HashMap::new();
    for (i, l) in ccw.into_iter().enumerate() {
        let r = find(&mut parent, i);
        clusters.entry(r).or_default().push(l);
    }
    let mut merged: Vec<Pline> = Vec::new();
    let mut keys: Vec<usize> = clusters.keys().copied().collect();
    keys.sort_unstable(); // deterministic output order
    for k in keys {
        let cluster = clusters.remove(&k).unwrap();
        if cluster.len() == 1 {
            merged.extend(cluster);
        } else {
            merged.extend(resolve(&cluster, &[], &|a, _| a != 0));
        }
    }

    // Nest: depth parity decides outer (even) vs hole (odd). Loops are non-crossing now.
    let n = merged.len();
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let probe = merged[i].at(0).pos();
        for (j, other) in merged.iter().enumerate() {
            if i != j && other.winding_number(probe) != 0 {
                depth[i] += 1;
            }
        }
    }
    merged
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let want_ccw = depth[i].is_multiple_of(2);
            let is_ccw = l.area() > 0.0;
            if want_ccw == is_ccw {
                l
            } else {
                let mut r = l;
                r.invert_direction_mut();
                r
            }
        })
        .collect()
}

/// Round-capped, round-joined stroke of `width` around an open or closed path, as a region.
pub fn stroke(path: &Pline, width: f64) -> Region {
    stroke_many(std::slice::from_ref(path), width)
}

/// Union of the strokes of many paths (the swept area of a tool path set).
///
/// A closed path's stroke is the ring between the outward and inward offsets of its
/// filled loop; an open path's stroke is its two side offsets closed with semicircular
/// caps. Both come from cavalier's offsetting, so the pieces have no coincident or
/// tangent boundaries (which a union of per-segment capsules is full of). Paths whose
/// offsets fragment fall back to the capsule union.
pub fn stroke_many(paths: &[Pline], width: f64) -> Region {
    let r = width / 2.0;
    if r.is_nan() || r <= 0.0 {
        return Region::empty();
    }
    let clean: Vec<Pline> = paths.iter().filter_map(pline::sanitize).collect();
    let mut parts: Vec<Region> = Vec::new();
    for p in &clean {
        if p.is_closed() && p.area().abs() > 1e-18 {
            let base = Region::from_loops([p.clone()], LoopRule::NonZero);
            let outer = base.offset(r);
            let inner = base.offset(-r);
            parts.push(if inner.is_empty() {
                outer
            } else {
                outer.difference(&inner)
            });
        } else if let Some(region) = open_stroke(p, r) {
            parts.push(region);
        } else {
            parts.push(stroke_capsules(std::slice::from_ref(p), r));
        }
    }
    parts.into_iter().fold(Region::empty(), |acc, part| {
        if acc.is_empty() {
            part
        } else {
            acc.union(&part)
        }
    })
}

/// Stroke of an open path from its two side offsets plus end caps; `None` when an offset
/// fragments (self-intersecting or looped paths), where the capsule fallback is used.
fn open_stroke(p: &Pline, r: f64) -> Option<Region> {
    if p.vertex_count() < 2 || p.is_closed() {
        return None;
    }
    let left = p.parallel_offset(r);
    let right = p.parallel_offset(-r);
    if left.len() != 1 || right.len() != 1 {
        return None;
    }
    let right = pline::sanitize(&right[0])?;
    let mut left_back = pline::sanitize(&left[0])?;
    left_back.invert_direction_mut();
    let (t_start, t_end) = (start_tangent(p), end_tangent(p));
    let mut lp = Pline::new_closed();
    let n = right.vertex_count();
    for (i, v) in right.iter_vertexes().enumerate() {
        if i + 1 < n {
            lp.add(v.x, v.y, v.bulge);
        } else {
            // End cap around the path end, from the right side to the left side.
            let to = left_back.at(0).pos();
            lp.add(v.x, v.y, cap_bulge(Point::new(v.x, v.y), pt(to), t_end));
        }
    }
    let m = left_back.vertex_count();
    for (i, v) in left_back.iter_vertexes().enumerate() {
        if i + 1 < m {
            lp.add(v.x, v.y, v.bulge);
        } else {
            // Start cap around the path start, back to the right side's first vertex.
            let to = right.at(0).pos();
            lp.add(
                v.x,
                v.y,
                cap_bulge(
                    Point::new(v.x, v.y),
                    pt(to),
                    Point::new(-t_start.x, -t_start.y),
                ),
            );
        }
    }
    let region = Region::from_loops([lp], LoopRule::NonZero);
    (!region.is_empty()).then_some(region)
}

/// Capsule-per-segment stroke: exact arcs, but adjacent capsules share cap circles, so it
/// is only the fallback for paths whose offsets fragment.
fn stroke_capsules(paths: &[Pline], r: f64) -> Region {
    let mut capsules: Vec<Pline> = Vec::new();
    for (v1, v2) in paths.iter().flat_map(|p| p.iter_segments()) {
        let a = pt(v1.pos());
        let b = pt(v2.pos());
        if a.distance(b) <= pline::REPEAT_EPS {
            continue;
        }
        match Arc::from_bulge(a, b, v1.bulge) {
            Some(arc) if arc_capsule_is_well_formed(&arc, r) => capsules.push(arc_capsule(&arc, r)),
            Some(arc) => {
                if let Ok(pts) = arc.flatten(r * 0.05) {
                    for w in pts.windows(2) {
                        if w[0].distance(w[1]) > pline::REPEAT_EPS {
                            capsules.push(line_capsule(w[0], w[1], r));
                        }
                    }
                }
            }
            None => capsules.push(line_capsule(a, b, r)),
        }
    }
    Region::from_loops(capsules, LoopRule::NonZero)
}

/// The inner offset arc must keep a usable chord, or the capsule degenerates.
fn arc_capsule_is_well_formed(arc: &Arc, r: f64) -> bool {
    if arc.radius <= r {
        return false;
    }
    let chord = arc.start().distance(arc.end());
    let inner_chord = chord * (arc.radius - r) / arc.radius;
    inner_chord > 10.0 * pline::REPEAT_EPS
}

fn unit(p: Point) -> Point {
    let l = p.x.hypot(p.y);
    Point::new(p.x / l, p.y / l)
}

/// Bulge of the semicircle from `p` to `q` whose midpoint lies in direction `forward`.
/// A counter-clockwise arc (positive bulge) bulges to the right of its chord.
fn cap_bulge(p: Point, q: Point, forward: Point) -> f64 {
    let right = Point::new(q.y - p.y, -(q.x - p.x));
    if right.x * forward.x + right.y * forward.y > 0.0 {
        1.0
    } else {
        -1.0
    }
}

fn line_capsule(a: Point, b: Point, r: f64) -> Pline {
    let d = unit(Point::new(b.x - a.x, b.y - a.y));
    let n = Point::new(-d.y, d.x);
    let (ar, br) = (
        Point::new(a.x - n.x * r, a.y - n.y * r),
        Point::new(b.x - n.x * r, b.y - n.y * r),
    );
    let (al, bl) = (
        Point::new(a.x + n.x * r, a.y + n.y * r),
        Point::new(b.x + n.x * r, b.y + n.y * r),
    );
    let mut p = Pline::new_closed();
    p.add(ar.x, ar.y, 0.0);
    p.add(br.x, br.y, cap_bulge(br, bl, d));
    p.add(bl.x, bl.y, 0.0);
    p.add(al.x, al.y, cap_bulge(al, ar, Point::new(-d.x, -d.y)));
    p
}

fn arc_capsule(arc: &Arc, r: f64) -> Pline {
    let c = arc.center;
    let (a0, a1) = (arc.start_angle, arc.start_angle + arc.sweep);
    let at = |radius: f64, ang: f64| Point::new(c.x + radius * ang.cos(), c.y + radius * ang.sin());
    let (ro, ri) = (arc.radius + r, arc.radius - r);
    let (os, oe, is, ie) = (at(ro, a0), at(ro, a1), at(ri, a0), at(ri, a1));
    let b = (arc.sweep / 4.0).tan();
    // Forward tangents at the arc ends (direction of travel).
    let tangent = |ang: f64| {
        let s = arc.sweep.signum();
        Point::new(-ang.sin() * s, ang.cos() * s)
    };
    let (t0, t1) = (tangent(a0), tangent(a1));
    let mut p = Pline::new_closed();
    p.add(os.x, os.y, b); // outer arc forward
    p.add(oe.x, oe.y, cap_bulge(oe, ie, t1)); // end cap around the front
    p.add(ie.x, ie.y, -b); // inner arc backward
    p.add(is.x, is.y, cap_bulge(is, os, Point::new(-t0.x, -t0.y))); // start cap around the back
    pline::ensure_ccw(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pline::{circle, rect};
    use proptest::prelude::*;
    use std::f64::consts::PI;

    fn sq(x: f64, y: f64, s: f64) -> Pline {
        rect(Point::new(x, y), Point::new(x + s, y + s))
    }

    fn assert_near(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b}");
    }

    #[test]
    fn union_of_overlapping_squares() {
        let a = Region::from_loops([sq(0.0, 0.0, 10.0)], LoopRule::NonZero);
        let b = Region::from_loops([sq(5.0, 5.0, 10.0)], LoopRule::NonZero);
        let u = a.union(&b);
        assert_eq!(u.loops.len(), 1);
        assert_near(u.area(), 175.0, 1e-9);
        assert!(u.loops[0].area() > 0.0);
        let i = a.intersection(&b);
        assert_near(i.area(), 25.0, 1e-9);
        let d = a.difference(&b);
        assert_near(d.area(), 75.0, 1e-9);
        let x = a.xor(&b);
        assert_near(x.area(), 150.0, 1e-9);
        assert_eq!(x.loops.len(), 2);
    }

    #[test]
    fn difference_makes_a_hole() {
        let outer = Region::from_loops([sq(0.0, 0.0, 20.0)], LoopRule::NonZero);
        let inner = Region::from_loops([sq(4.0, 4.0, 12.0)], LoopRule::NonZero);
        let r = outer.difference(&inner);
        assert_eq!(r.loops.len(), 2);
        assert_near(r.area(), 400.0 - 144.0, 1e-9);
        assert_eq!(r.outers().count(), 1);
        assert_eq!(r.holes().count(), 1);
        assert!(r.contains(Point::new(1.0, 1.0)));
        assert!(!r.contains(Point::new(10.0, 10.0)));
        assert_eq!(r.islands().len(), 1);
        assert_eq!(r.islands()[0].loops.len(), 2);
    }

    #[test]
    fn even_odd_nesting_and_merge_then_nest() {
        // Annulus from two concentric circles, and two overlapping pads with a trace.
        let loops = vec![
            circle(Point::new(21.0, 5.65), 1.5),
            circle(Point::new(21.0, 5.65), 0.8),
            circle(Point::new(3.0, 2.8), 0.6),
            circle(Point::new(3.0, 8.5), 0.6),
            rect(Point::new(2.85, 3.0), Point::new(3.15, 8.3)),
        ];
        let r = Region::from_loops(loops.clone(), LoopRule::MergeThenNest);
        assert_eq!(r.outers().count(), 2, "annulus + merged pad group");
        assert_eq!(r.holes().count(), 1);
        let expected = PI * (1.5 * 1.5 - 0.8 * 0.8)
            + (2.0 * PI * 0.36 + 0.3 * 5.3 - 2.0 * overlap_circle_rect_area());
        assert_near(r.area(), expected, 1e-6);

        // Pure even-odd would XOR the pad/trace overlaps instead of merging them.
        let eo = Region::from_loops(loops, LoopRule::EvenOdd);
        assert!(eo.area() < r.area());
    }

    /// Area of the overlap between a pad circle (r 0.6) and the 0.3-wide trace rectangle
    /// crossing its centre, used by the merge test above: a circular segment-free band.
    fn overlap_circle_rect_area() -> f64 {
        // Rectangle x in [2.85, 3.15] fully spans the circle vertically on one side only;
        // integrate the chord length of the circle over the band around its centre.
        let r: f64 = 0.6;
        let half = 0.15;
        // Band from x=-0.15..0.15 of circle radius 0.6 intersected with y>=0.2 (trace starts
        // 0.2 above/below the pad centre): compute numerically for robustness.
        let n = 20000;
        let mut area = 0.0;
        for i in 0..n {
            let x = -half + (i as f64 + 0.5) / n as f64 * 2.0 * half;
            let y_top = (r * r - x * x).sqrt();
            area += (y_top - 0.2).max(0.0) * (2.0 * half / n as f64);
        }
        area
    }

    #[test]
    fn inward_offset_keeps_hole_with_round_reflex_corners() {
        let outer = Region::from_loops([sq(0.0, 0.0, 20.0)], LoopRule::NonZero);
        let inner = Region::from_loops([sq(4.0, 4.0, 12.0)], LoopRule::NonZero);
        let r = outer.difference(&inner).offset(-1.0);
        assert_eq!(r.outers().count(), 1);
        assert_eq!(r.holes().count(), 1);
        let hole_area = -r.holes().next().unwrap().area();
        assert_near(hole_area, 14.0 * 14.0 - (4.0 - PI), 1e-9);
        assert_near(r.outers().next().unwrap().area(), 18.0 * 18.0, 1e-9);
        // Offsetting past the band width empties the region.
        assert!(outer.difference(&inner).offset(-2.5).is_empty());
    }

    #[test]
    fn stroke_of_line_is_a_capsule() {
        let mut path = Pline::new();
        path.add(0.0, 0.0, 0.0);
        path.add(10.0, 0.0, 0.0);
        let s = stroke(&path, 1.0);
        assert_eq!(s.loops.len(), 1);
        assert_near(s.area(), 10.0 * 1.0 + PI * 0.25, 1e-9);
        assert!(s.contains(Point::new(5.0, 0.4)));
        assert!(!s.contains(Point::new(5.0, 0.6)));
        assert!(s.contains(Point::new(10.4, 0.0)));
    }

    #[test]
    fn stroke_of_arc_keeps_arcs() {
        let c = circle(Point::new(0.0, 0.0), 5.0);
        let s = stroke(&c, 1.0);
        assert_near(s.area(), PI * (5.5 * 5.5 - 4.5 * 4.5), 1e-6);
        assert_eq!(s.outers().count(), 1);
        assert_eq!(s.holes().count(), 1);
        assert!(
            s.loops
                .iter()
                .all(|l| l.iter_vertexes().any(|v| v.bulge != 0.0))
        );
    }

    #[test]
    fn zigzag_stroke_is_one_island() {
        let path = pline::from_points(
            [
                Point::new(14.0, 5.0),
                Point::new(15.0, 6.3),
                Point::new(16.0, 5.0),
                Point::new(17.0, 6.3),
                Point::new(18.0, 5.0),
            ],
            false,
        );
        let s = stroke(&path, 0.25);
        assert_eq!(s.outers().count(), 1);
        assert_eq!(s.holes().count(), 0);
    }

    fn shape() -> impl Strategy<Value = Pline> {
        prop_oneof![
            (-20.0..20.0f64, -20.0..20.0f64, 0.5..15.0f64, 0.5..15.0f64)
                .prop_map(|(x, y, w, h)| rect(Point::new(x, y), Point::new(x + w, y + h))),
            (-20.0..20.0f64, -20.0..20.0f64, 0.5..10.0f64)
                .prop_map(|(x, y, r)| circle(Point::new(x, y), r)),
        ]
    }

    fn region() -> impl Strategy<Value = Region> {
        prop::collection::vec(shape(), 1..4)
            .prop_map(|ls| Region::from_loops(ls, LoopRule::NonZero))
    }

    fn well_formed(r: &Region) -> Result<(), TestCaseError> {
        for l in &r.loops {
            prop_assert!(l.is_closed());
            prop_assert!(l.iter_vertexes().all(|v| v.bulge.abs() <= 1.0 + 1e-12));
        }
        // Every hole belongs to an island.
        let islands = r.islands();
        let assigned: usize = islands.iter().map(|i| i.loops.len()).sum();
        prop_assert_eq!(assigned, r.loops.len());
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn inclusion_exclusion(a in region(), b in region()) {
            let (aa, ab) = (a.area(), b.area());
            let u = a.union(&b).area();
            let i = a.intersection(&b).area();
            let d = a.difference(&b).area();
            let x = a.xor(&b).area();
            let tol = 1e-7 * (aa + ab + 1.0);
            prop_assert!((u + i - (aa + ab)).abs() <= tol, "union+intersection {} vs {}", u + i, aa + ab);
            prop_assert!((d - (aa - i)).abs() <= tol, "difference {} vs {}", d, aa - i);
            prop_assert!((x - (aa + ab - 2.0 * i)).abs() <= tol, "xor {} vs {}", x, aa + ab - 2.0 * i);
            prop_assert!(i >= -tol && i <= aa.min(ab) + tol);
            well_formed(&a.union(&b))?;
            well_formed(&a.difference(&b))?;
        }

        #[test]
        fn union_is_idempotent_and_absorbs_empty(a in region()) {
            let tol = 1e-7 * (a.area() + 1.0);
            prop_assert!((a.union(&a).area() - a.area()).abs() <= tol);
            prop_assert!((a.union(&Region::empty()).area() - a.area()).abs() <= tol);
            prop_assert!(a.intersection(&Region::empty()).is_empty());
            prop_assert!((a.difference(&Region::empty()).area() - a.area()).abs() <= tol);
        }

        #[test]
        fn offset_shrinks_and_closing_restores_convex(s in shape(), d in 0.05..1.0f64) {
            let r = Region::from_loops([s], LoopRule::NonZero);
            let shrunk = r.offset(-d);
            prop_assert!(shrunk.area() <= r.area() + 1e-9);
            let closed = r.offset(d).offset(-d);
            prop_assert!((closed.area() - r.area()).abs() <= 1e-7 * (r.area() + 1.0), "closing {} vs {}", closed.area(), r.area());
            well_formed(&shrunk)?;
        }

        #[test]
        fn merge_then_nest_never_exceeds_nonzero(ls in prop::collection::vec(shape(), 1..5)) {
            let nz = Region::from_loops(ls.clone(), LoopRule::NonZero).area();
            let mn = Region::from_loops(ls.clone(), LoopRule::MergeThenNest).area();
            let eo = Region::from_loops(ls, LoopRule::EvenOdd).area();
            prop_assert!(mn <= nz + 1e-9 * (nz + 1.0));
            prop_assert!(eo <= nz + 1e-9 * (nz + 1.0));
        }
    }
}
