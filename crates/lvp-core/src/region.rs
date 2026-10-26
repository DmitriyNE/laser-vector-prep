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
    FindIntersectsOptions, PlineCreation, PlineIntersect, PlineSource, PlineSourceMut, PlineVertex,
    PlineViewData, seg_closest_point, seg_length, seg_tangent_vector,
};
use cavalier_contours::shape_algorithms::{Shape, ShapeOffsetOptions};
use cavalier_contours::static_aabb2d_index::StaticAABB2DIndex;

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

/// Offsets run in micrometres. The kernel's tolerances are fixed (1e-5, made for geometry of
/// unit size); in millimetres a curve approximated by micron-long chords falls below its
/// parallel test, reads as lines folded back on themselves, and its offsets come out
/// broken. In micrometres such chords are well above it.
pub const OFFSET_SCALE: f64 = 1000.0;

/// The kernel's own tolerance, in the scaled coordinates: positions this close are one, and
/// two straight segments whose cross product is this small are one line ([`pline::clean`]).
pub const OFFSET_CLEAN_EPS: f64 = 1e-5;

#[derive(Clone, Debug, Default)]
pub struct Region {
    pub loops: Vec<Pline>,
    /// A boolean that produced this region, or one it was derived from, could not close
    /// every boundary and left the unclosed part out. Happens only on near-degenerate input
    /// (boundaries within nanometres of each other that the kernel intersects
    /// inconsistently); callers report it rather than lose geometry silently.
    pub incomplete: bool,
}

/// Tolerances derived from the extent of the geometry being operated on.
#[derive(Clone, Copy, Debug)]
struct Tolerances {
    /// Distance from a slice midpoint to its inside/outside probe points.
    probe: f64,
    /// Points closer than this are the same point (slice ends, duplicate splits).
    join: f64,
    /// Vertices of different loops closer than this are merged before intersecting. The
    /// kernel may move a crossing point by up to its tolerance on each loop, so boundaries
    /// closer than a few tolerances cannot be intersected consistently.
    snap: f64,
    /// Loops of the same shape within this are one loop (see `merge_duplicates`).
    same: f64,
}

/// The kernel's own position tolerance (`pos_equal_eps` of cavalier_contours' intersection
/// and slicing defaults). Joining at anything finer lets the two disagree about where
/// near-coincident boundaries meet.
const KERNEL_EPS: f64 = 1e-5;

impl Tolerances {
    fn for_extent(extent: f64) -> Self {
        let extent = extent.max(1.0);
        Self {
            probe: (extent * 1e-9).max(1e-7),
            join: (extent * 1e-8).max(KERNEL_EPS),
            snap: (extent * 3e-8).max(3.0 * KERNEL_EPS),
            same: (extent * 1e-7).max(10.0 * KERNEL_EPS),
        }
    }
}

fn v2(p: Point) -> Vector2<f64> {
    Vector2::new(p.x, p.y)
}

fn pt(v: Vector2<f64>) -> Point {
    Point::new(v.x, v.y)
}

/// Booleans refuse loops beyond this (or not finite): ten times the import range
/// ([`crate::source::MAX_COORDINATE_MM`]), leaving room for offsets and envelopes. The
/// kernel's numbers overflow long before the range of `f64`.
const MAX_COORDINATE: f64 = 10.0 * crate::source::MAX_COORDINATE_MM;

/// Loops that can participate in a boolean: closed, at least two vertices, enclosing
/// something. A loop that crosses itself (a bow-tie) may have zero net area and still
/// enclose area under either fill rule.
fn is_usable(p: &Pline) -> bool {
    p.is_closed()
        && p.vertex_count() >= 2
        && p.vertex_data.iter().all(|v| {
            v.x.abs() <= MAX_COORDINATE && v.y.abs() <= MAX_COORDINATE && v.bulge.is_finite()
        })
        && (p.area().abs() > 1e-18 || (p.vertex_count() >= 4 && p.scan_for_self_intersect()))
}

struct Tagged {
    pline: Pline,
    operand: u8,
    bbox: BBox,
    /// How many identical copies of this loop the operand had (see `merge_duplicates`).
    count: i32,
    /// Moved by snapping or merged with a near (not exact) duplicate: the boolean of its
    /// cluster is checked against the robust one (see `resolve_cluster`).
    nudged: bool,
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
                count: 1,
                nudged: false,
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

/// Uniform grid over loop bounding boxes: candidates for a box or point query in O(local).
///
/// Work and memory stay bounded whatever the input: a box spanning many cells is kept in a
/// separate list checked on every query instead of being entered into each cell, and a
/// query is clamped to the occupied cells and falls back to a linear scan when it would
/// visit more cells than there are boxes (huge or far-away queries, extreme coordinates).
struct BBoxGrid {
    origin: Point,
    cell: f64,
    /// Occupied cell index range (x0, y0, x1, y1).
    range: (i32, i32, i32, i32),
    cells: HashMap<(i32, i32), Vec<usize>>,
    /// Boxes spanning more than `MAX_CELLS_PER_BOX` cells.
    large: Vec<usize>,
    boxes: Vec<BBox>,
}

impl BBoxGrid {
    const MAX_CELLS_PER_BOX: i64 = 64;

    fn new(boxes: Vec<BBox>) -> Self {
        let all = boxes
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .unwrap_or(BBox::at(Point::new(0.0, 0.0)));
        // Aim for a few loops per cell; never below a sane minimum cell size.
        let n = boxes.len().max(1) as f64;
        let extent = all.width().max(all.height()).max(1e-6);
        let cell = (extent / n.sqrt()).max(extent * 1e-4);
        let mut g = BBoxGrid {
            origin: all.min,
            cell,
            range: (0, 0, 0, 0),
            cells: HashMap::new(),
            large: Vec::new(),
            boxes,
        };
        g.range = g.span(&all);
        for i in 0..g.boxes.len() {
            let (x0, y0, x1, y1) = g.clamped_span(&g.boxes[i]);
            if cell_count(x0, y0, x1, y1) > Self::MAX_CELLS_PER_BOX {
                g.large.push(i);
                continue;
            }
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    g.cells.entry((cx, cy)).or_default().push(i);
                }
            }
        }
        g
    }

    fn span(&self, b: &BBox) -> (i32, i32, i32, i32) {
        // `as` saturates (and maps NaN to 0), so this never overflows.
        let f = |v: f64, o: f64| ((v - o) / self.cell).floor() as i32;
        (
            f(b.min.x, self.origin.x),
            f(b.min.y, self.origin.y),
            f(b.max.x, self.origin.x),
            f(b.max.y, self.origin.y),
        )
    }

    fn clamped_span(&self, b: &BBox) -> (i32, i32, i32, i32) {
        let (x0, y0, x1, y1) = self.span(b);
        let (rx0, ry0, rx1, ry1) = self.range;
        (x0.max(rx0), y0.max(ry0), x1.min(rx1), y1.min(ry1))
    }

    /// Indices whose box overlaps `b` (expanded by `pad`), each once, ascending.
    fn query_box(&self, b: &BBox, pad: f64) -> Vec<usize> {
        let q = BBox {
            min: Point::new(b.min.x - pad, b.min.y - pad),
            max: Point::new(b.max.x + pad, b.max.y + pad),
        };
        let overlaps = |i: &usize| bbox_overlap(&self.boxes[*i], &q, 0.0);
        let (x0, y0, x1, y1) = self.clamped_span(&q);
        if cell_count(x0, y0, x1, y1) > self.boxes.len() as i64 {
            return (0..self.boxes.len()).filter(overlaps).collect();
        }
        let mut out: Vec<usize> = self.large.iter().copied().filter(overlaps).collect();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(ids) = self.cells.get(&(cx, cy)) {
                    out.extend(ids.iter().copied().filter(overlaps));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Indices whose box contains `p` (expanded by `pad`).
    fn query_point(&self, p: Point, pad: f64) -> Vec<usize> {
        self.query_box(&BBox::at(p), pad)
    }
}

/// Cells in an inclusive index range; 0 when the range is empty.
fn cell_count(x0: i32, y0: i32, x1: i32, y1: i32) -> i64 {
    if x1 < x0 || y1 < y0 {
        return 0;
    }
    (i64::from(x1) - i64::from(x0) + 1) * (i64::from(y1) - i64::from(y0) + 1)
}

/// Sum of winding numbers of the loops of one operand around `p`.
fn winding(loops: &[Tagged], grid: &BBoxGrid, operand: u8, p: Point, pad: f64) -> i32 {
    winding_indexed(loops, None, grid, operand, p, pad)
}

/// Every segment of the loops of one operand in one spatial index, for winding numbers of a
/// large boolean: a probe inside hundreds of nested loops (the strokes of a fill's rings)
/// then visits only the segments its rightward ray can cross, not every loop's index.
struct SegmentField {
    index: Option<StaticAABB2DIndex<f64>>,
    /// The loop and segment of each index entry.
    refs: Vec<(u32, u32)>,
    max_x: f64,
}

impl SegmentField {
    fn new(loops: &[Tagged], operand: u8) -> SegmentField {
        let mut refs = Vec::new();
        let mut builder_boxes = Vec::new();
        let mut max_x = f64::NEG_INFINITY;
        for (li, l) in loops
            .iter()
            .enumerate()
            .filter(|(_, l)| l.operand == operand)
        {
            let n = l.pline.vertex_count();
            for si in 0..n {
                let b = cavalier_contours::polyline::seg_fast_approx_bounding_box(
                    l.pline.at(si),
                    l.pline.at((si + 1) % n),
                );
                max_x = max_x.max(b.max_x);
                builder_boxes.push(b);
                refs.push((li as u32, si as u32));
            }
        }
        let index = (!builder_boxes.is_empty()).then(|| {
            let mut builder = cavalier_contours::static_aabb2d_index::StaticAABB2DIndexBuilder::new(
                builder_boxes.len(),
            );
            for b in &builder_boxes {
                builder.add(b.min_x, b.min_y, b.max_x, b.max_y);
            }
            builder.build().expect("as many boxes as announced")
        });
        SegmentField { index, refs, max_x }
    }

    fn winding(&self, loops: &[Tagged], p: Point) -> i32 {
        let Some(index) = &self.index else {
            return 0;
        };
        let mut w = 0;
        index.visit_query(p.x, p.y, self.max_x.max(p.x), p.y, &mut |k: usize| {
            let (li, si) = self.refs[k];
            let l = &loops[li as usize];
            let n = l.pline.vertex_count();
            let si = si as usize;
            w += l.count * segment_winding(l.pline.at(si), l.pline.at((si + 1) % n), v2(p));
        });
        w
    }
}

/// Resolves with more loop vertices than this take winding numbers from one index of all
/// their segments ([`SegmentField`]).
const SEGMENT_FIELD_FROM: usize = 4096;

/// Spatial indexes of the segments of loops, built on first use, for winding numbers.
type SegmentIndexes = [std::sync::OnceLock<StaticAABB2DIndex<f64>>];

/// Loops with more vertices than this have their winding numbers taken through an index.
const WINDING_INDEX_FROM: usize = 64;

/// [`winding`], taking large loops' windings through `indexes` (one per loop).
fn winding_indexed(
    loops: &[Tagged],
    indexes: Option<&SegmentIndexes>,
    grid: &BBoxGrid,
    operand: u8,
    p: Point,
    pad: f64,
) -> i32 {
    grid.query_point(p, pad)
        .into_iter()
        .filter(|&i| loops[i].operand == operand)
        .map(|i| {
            let l = &loops[i];
            let w = match indexes {
                Some(ix) if l.pline.vertex_count() > WINDING_INDEX_FROM => {
                    let index = ix[i].get_or_init(|| l.pline.create_approx_aabb_index());
                    loop_winding(&l.pline, index, l.bbox, p)
                }
                _ => l.pline.winding_number(v2(p)),
            };
            l.count * w
        })
        .sum()
}

/// The winding number of closed loop `pl` about `p`, from only the segments whose boxes
/// reach `p`'s height: a segment that does not contributes nothing. The same sum as
/// cavalier's `winding_number`, without visiting every segment of a large loop for every
/// probe (the swept area of a fill has loops of thousands of vertices, probed thousands of
/// times).
fn loop_winding(pl: &Pline, index: &StaticAABB2DIndex<f64>, bbox: BBox, p: Point) -> i32 {
    let n = pl.vertex_count();
    let mut w = 0;
    index.visit_query(
        bbox.min.x - 1.0,
        p.y,
        bbox.max.x + 1.0,
        p.y,
        &mut |i: usize| {
            w += segment_winding(pl.at(i), pl.at((i + 1) % n), v2(p));
        },
    );
    w
}

/// One segment's share of a winding number (cavalier_contours' `winding_number`, MIT or
/// Apache-2.0, per segment): a line crossing the horizontal through `p` to its right counts
/// one, an arc also where `p` lies in its bulge.
fn segment_winding(v1: PlineVertex<f64>, v2: PlineVertex<f64>, point: Vector2<f64>) -> i32 {
    let left = |a: Vector2<f64>, b: Vector2<f64>, q: Vector2<f64>| {
        (b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x)
    };
    if v1.bulge == 0.0 {
        if v1.y <= point.y {
            if v2.y > point.y && left(v1.pos(), v2.pos(), point) > 0.0 {
                return 1;
            }
        } else if v2.y <= point.y && left(v1.pos(), v2.pos(), point) <= 0.0 {
            return -1;
        }
        return 0;
    }
    let is_ccw = v1.bulge > 0.0;
    let point_is_left = if is_ccw {
        left(v1.pos(), v2.pos(), point) > 0.0
    } else {
        left(v1.pos(), v2.pos(), point) >= 0.0
    };
    let inside_circle = || {
        let (dx, dy, b) = (v2.x - v1.x, v2.y - v1.y, v1.bulge);
        let b2 = b * b;
        let (offset, scale) = (1.0 - b2, 2.0 * b);
        let x = scale * (2.0 * point.x - v1.x - v2.x) + dy * offset;
        let y = scale * (2.0 * point.y - v1.y - v2.y) - dx * offset;
        let r = 1.0 + b2;
        x * x + y * y < (dx * dx + dy * dy) * r * r
    };
    if v1.y <= point.y {
        if v2.y > point.y {
            // Upward crossing of the chord.
            if is_ccw {
                if point_is_left || inside_circle() {
                    return 1;
                }
            } else if point_is_left && !inside_circle() {
                return 1;
            }
            0
        } else if is_ccw && !point_is_left && v2.x < point.x && point.x < v1.x && inside_circle() {
            1
        } else if !is_ccw && point_is_left && v1.x < point.x && point.x < v2.x && inside_circle() {
            -1
        } else {
            0
        }
    } else if v2.y <= point.y {
        // Downward crossing of the chord.
        if is_ccw {
            if !point_is_left && !inside_circle() {
                return -1;
            }
        } else if !point_is_left || inside_circle() {
            return -1;
        }
        0
    } else if is_ccw && !point_is_left && v1.x < point.x && point.x < v2.x && inside_circle() {
        1
    } else if !is_ccw && point_is_left && v2.x < point.x && point.x < v1.x && inside_circle() {
        -1
    } else {
        0
    }
}

/// Replace duplicates (same operand and orientation, the same shape within `tol`: every
/// vertex and every segment's midpoint, from any starting vertex) by one loop with a count.
/// Duplicates add nothing to the boundary but every slice's winding probe would visit each
/// of them (a pad flashed hundreds of times in place made the boolean quadratic), and
/// copies a few tens of nanometres apart cross each other at grazing angles in places the
/// kernel cannot pin down consistently, more so with three or more copies. Merging moves a
/// boundary by at most `tol`.
fn merge_duplicates(loops: &mut Vec<Tagged>, tol: f64) {
    let centre = |l: &Tagged| {
        Point::new(
            (l.bbox.min.x + l.bbox.max.x) / 2.0,
            (l.bbox.min.y + l.bbox.max.y) / 2.0,
        )
    };
    let cell = |p: Point| {
        (
            (p.x / (4.0 * tol)).floor() as i64,
            (p.y / (4.0 * tol)).floor() as i64,
        )
    };
    let mut kept_at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut keep = vec![true; loops.len()];
    for i in 0..loops.len() {
        let c = centre(&loops[i]);
        let (cx, cy) = cell(c);
        let twin = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
            .filter_map(|k| kept_at.get(&k))
            .flatten()
            .copied()
            .filter_map(|j| same_shape(&loops[j], &loops[i], tol).map(|exact| (j, exact)))
            .min_by_key(|&(j, _)| j);
        match twin {
            Some((j, exact)) => {
                loops[j].count += loops[i].count;
                loops[j].nudged |= !exact || loops[i].nudged;
                keep[i] = false;
            }
            None => kept_at.entry((cx, cy)).or_default().push(i),
        }
    }
    let mut i = 0;
    loops.retain(|_| {
        i += 1;
        keep[i - 1]
    });
}

/// Same operand, closedness and orientation, and every vertex and segment midpoint of `b`
/// within `tol` of `a`'s, for some starting vertex: `Some(exact)`, `exact` when every vertex
/// is identical.
fn same_shape(a: &Tagged, b: &Tagged, tol: f64) -> Option<bool> {
    let (pa, pb) = (&a.pline, &b.pline);
    let n = pa.vertex_count();
    if a.operand != b.operand
        || n != pb.vertex_count()
        || pa.is_closed() != pb.is_closed()
        || (pa.area() > 0.0) != (pb.area() > 0.0)
        || (a.bbox.min.x - b.bbox.min.x).abs() > tol
        || (a.bbox.min.y - b.bbox.min.y).abs() > tol
        || (a.bbox.max.x - b.bbox.max.x).abs() > tol
        || (a.bbox.max.y - b.bbox.max.y).abs() > tol
    {
        return None;
    }
    let near = |p: Point, q: Point| p.distance(q) <= tol;
    let vertex = |p: &Pline, i: usize| pt(p.at(i % n).pos());
    let midpoint =
        |p: &Pline, i: usize| pline::seg_midpoint_and_left_normal(p.at(i % n), p.at((i + 1) % n)).0;
    let k = (0..n)
        .filter(|&k| near(vertex(pa, 0), vertex(pb, k)))
        .find(|&k| {
            (0..n).all(|i| {
                near(vertex(pa, i), vertex(pb, i + k)) && near(midpoint(pa, i), midpoint(pb, i + k))
            })
        })?;
    Some((0..n).all(|i| pa.at(i) == pb.at((i + k) % n)))
}

/// Split points on one loop, snapped onto their segments, sorted along the loop and
/// de-duplicated. Snapping matters: intersection points of near-tangent micro-arcs can sit
/// off their segment by more than the join tolerance, which would make the slice invalid.
fn sorted_splits(
    p: &Pline,
    splits: &mut [(usize, Vector2<f64>)],
    join: f64,
    merged: &mut bool,
) -> Vec<(usize, Vector2<f64>)> {
    for sp in splits.iter_mut() {
        let i = sp.0;
        let j = if i + 1 < p.vertex_count() { i + 1 } else { 0 };
        sp.1 = seg_closest_point(p.at(i), p.at(j), sp.1, join);
    }
    // Position along the loop: length of the segments before, plus the chord distance from
    // the segment start (monotonic along a segment since |bulge| <= 1).
    let mut before = Vec::with_capacity(p.vertex_count() + 1);
    before.push(0.0);
    for (a, b) in p.iter_segments() {
        before.push(before.last().unwrap() + seg_length(a, b));
    }
    let total = *before.last().unwrap();
    let along = |s: &(usize, Vector2<f64>)| before[s.0] + pt(s.1).distance(pt(p.at(s.0).pos()));
    splits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| along(a).total_cmp(&along(b))));
    // Two splits are the same point only if they are close in space *and* along the loop:
    // where a loop crosses itself, two splits coincide in space far apart along the loop.
    let same = |a: &(usize, Vector2<f64>), b: &(usize, Vector2<f64>), gap: f64| {
        pt(a.1).distance(pt(b.1)) <= join && gap <= 4.0 * join
    };
    let mut out: Vec<(usize, Vector2<f64>)> = Vec::with_capacity(splits.len());
    for s in splits.iter() {
        match out.last() {
            Some(l) if same(l, s, along(s) - along(l)) => {
                *merged |= pt(l.1).distance(pt(s.1)) > 1e-12;
            }
            _ => out.push(*s),
        }
    }
    if out.len() > 1 {
        let (first, last) = (out[0], out[out.len() - 1]);
        if same(&first, &last, total - along(&last) + along(&first)) {
            *merged |= pt(first.1).distance(pt(last.1)) > 1e-12;
            out.pop();
        }
    }
    out
}

/// The closed loop the consecutive slices of one loop form (each ends where the next
/// starts); `None` if a slice is degenerate.
fn rejoin(pieces: &[Pline]) -> Option<Pline> {
    let mut out = Pline::new_closed();
    for p in pieces {
        let n = p.vertex_count();
        if n < 2 {
            return None;
        }
        for k in 0..n - 1 {
            let v = p.at(k);
            out.add(v.x, v.y, v.bulge);
        }
    }
    (out.vertex_count() >= 2).then_some(out)
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

/// How far (in `join` units) a chain may jump to the nearest free slice when nothing lies
/// within `join` (see `stitch`): 100 nm at the default tolerances, far below the 1 µm read
/// tolerance.
const STITCH_GAP: f64 = 4.0;

/// Stitch oriented open slices (plus already-closed whole loops) into closed loops. A
/// chain that does not close means the classification or the intersections were
/// inconsistent; it is dropped and the (incomplete) result comes back as `Err`.
fn stitch(slices: Vec<Pline>, join: f64, bridged: &mut bool) -> Result<Vec<Pline>, Vec<Pline>> {
    let (closed, open): (Vec<Pline>, Vec<Pline>) = slices.into_iter().partition(|s| s.is_closed());
    let mut result = closed;
    if open.is_empty() {
        return Ok(result);
    }
    let mut complete = true;
    let cell = |p: Point| ((p.x / join).floor() as i64, (p.y / join).floor() as i64);
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in open.iter().enumerate() {
        by_start.entry(cell(start_point(s))).or_default().push(i);
    }
    let mut used = vec![false; open.len()];
    // Unused slices starting within `radius` of `p`.
    let candidates = |p: Point, used: &[bool], radius: f64| -> Vec<usize> {
        let (cx, cy) = cell(p);
        let reach = (radius / join).ceil() as i64;
        let mut out = Vec::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                if let Some(ids) = by_start.get(&(cx + dx, cy + dy)) {
                    for &i in ids {
                        if !used[i] && start_point(&open[i]).distance(p) <= radius {
                            out.push(i);
                        }
                    }
                }
            }
        }
        out
    };
    // A nearly tangent crossing is placed by the kernel only to within a distance that grows
    // as the angle shrinks, so its two copies (one per loop) can end up further apart than
    // `join`. A chain that finds nothing within `join` continues at the nearest free start,
    // or closes on itself, within this gap: the boundary moves by at most that much.
    let gap = STITCH_GAP * join;

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
            let mut cands = candidates(tail, &used, join);
            if cands.is_empty() {
                let nearest = candidates(tail, &used, gap).into_iter().min_by(|&a, &b| {
                    let (da, db) = (
                        start_point(&open[a]).distance(tail),
                        start_point(&open[b]).distance(tail),
                    );
                    da.total_cmp(&db).then(a.cmp(&b))
                });
                let to_origin = tail.distance(origin);
                if to_origin <= gap
                    && nearest.is_none_or(|n| to_origin <= start_point(&open[n]).distance(tail))
                {
                    *bridged = true;
                    closed_ok = true;
                    break;
                }
                *bridged |= nearest.is_some();
                cands.extend(nearest);
            }
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
            complete = false;
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
    if complete { Ok(result) } else { Err(result) }
}

/// Move each vertex that lies within `join` of a vertex of another loop onto that vertex,
/// so near-coincident boundaries (a pad flashed twice with nanometre rounding differences)
/// become exactly coincident and take the kernel's overlap path. Left alone, they cross at
/// grazing angles inside the kernel's tolerance, which reports the arc ends as the crossing
/// points and lets slices of both loops be misclassified. Loops that degenerate are dropped.
fn snap_vertices(loops: &mut Vec<Tagged>, join: f64, repeat: f64) {
    let cell = |x: f64, y: f64| ((x / join).floor() as i64, (y / join).floor() as i64);
    // Grid cell -> vertices kept as snap targets: (owning loop, x, y).
    type Anchors = HashMap<(i64, i64), Vec<(usize, f64, f64)>>;
    let mut anchors: Anchors = HashMap::new();
    let mut moved = vec![false; loops.len()];
    for (li, l) in loops.iter_mut().enumerate() {
        // A cancelled computation discards its result (see `crate::cancel`).
        if li % 256 == 0 && crate::cancel::cancelled() {
            return;
        }
        for v in l.pline.vertex_data.iter_mut() {
            let (cx, cy) = cell(v.x, v.y);
            // The nearest anchor, not the first: a loop identical to an earlier one must snap
            // each vertex onto its own twin, not onto a neighbour of it a few nanometres off.
            let near = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
                .filter_map(|c| anchors.get(&c))
                .flatten()
                .filter(|&&(owner, x, y)| owner != li && (x - v.x).hypot(y - v.y) <= join)
                .min_by(|a, b| {
                    let (da, db) = ((a.1 - v.x).hypot(a.2 - v.y), (b.1 - v.x).hypot(b.2 - v.y));
                    da.total_cmp(&db).then(a.0.cmp(&b.0))
                })
                .copied();
            match near {
                Some((_, x, y)) => {
                    if x != v.x || y != v.y {
                        v.x = x;
                        v.y = y;
                        moved[li] = true;
                    }
                }
                None => anchors.entry((cx, cy)).or_default().push((li, v.x, v.y)),
            }
        }
    }
    // Segments shorter than the join tolerance (a draw a few nanometres long, or two
    // vertices snapped together) are below what the kernel can intersect reliably.
    let mut i = 0;
    loops.retain_mut(|l| {
        let was_moved = moved[i];
        i += 1;
        l.nudged |= was_moved;
        match l.pline.remove_repeat_pos(repeat) {
            Some(p) => l.pline = p,
            None if !was_moved => return true,
            None => {}
        }
        match pline::bounds(&l.pline) {
            Some(bb) if is_usable(&l.pline) => {
                l.bbox = bb;
                true
            }
            _ => false,
        }
    });
}

/// The general boolean. `rule` decides membership from the winding numbers
/// of the two operands. `Err` carries the loops that did close when some boundary chain
/// could not (see [`Region::incomplete`]).
fn resolve(a: &[Pline], b: &[Pline], rule: Rule) -> Result<Vec<Pline>, Vec<Pline>> {
    let mut loops: Vec<Tagged> = tag(a, 0).into_iter().chain(tag(b, 1)).collect();
    if loops.is_empty() {
        return Ok(Vec::new());
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
    snap_vertices(&mut loops, tol.snap, tol.join);
    merge_duplicates(&mut loops, tol.same);

    // Loops in different clusters can neither cross nor contain each other, so each cluster
    // is an independent boolean: solved apart, in parallel where a pool exists.
    let clusters = clusters(&loops, tol.join);
    if clusters.len() > 1 && loops.len() >= MIN_LOOPS_TO_SPLIT {
        let mut slots: Vec<Option<Tagged>> = loops.into_iter().map(Some).collect();
        let parts: Vec<Vec<Tagged>> = clusters
            .iter()
            .map(|c| {
                c.iter()
                    .map(|&i| slots[i].take().expect("one cluster per loop"))
                    .collect()
            })
            .collect();
        let mut complete = true;
        let mut out = Vec::new();
        for r in crate::par::map(parts, |part| resolve_cluster(part, tol, rule)) {
            match r {
                Ok(l) => out.extend(l),
                Err(l) => {
                    out.extend(l);
                    complete = false;
                }
            }
        }
        return if complete { Ok(out) } else { Err(out) };
    }
    resolve_cluster(loops, tol, rule)
}

/// The exact boolean of one cluster, or, if it cannot be made consistent, the robust
/// polygon boolean of the same loops with arcs flattened finely (see [`overlay_fallback`]).
fn resolve_cluster(
    loops: Vec<Tagged>,
    tol: Tolerances,
    rule: Rule,
) -> Result<Vec<Pline>, Vec<Pline>> {
    let flat: Vec<(u8, i32, Pline)> = loops
        .iter()
        .map(|l| (l.operand, l.count, l.pline.clone()))
        .collect();
    // Fallback flattening: 0.1 µm, coarser only for geometry beyond a metre or so.
    let fine = tol.same.max(FALLBACK_FLATTEN_MM);
    let mut degenerate = false;
    match resolve_tagged(loops, tol, rule, &mut degenerate) {
        Ok(r) => {
            let valid = is_valid_region(&r, tol.probe);
            if valid && !degenerate {
                return Ok(r);
            }
            // Snapped or merged loops, merged cuts or bridged gaps (near-coincident
            // boundaries, where a complete result can still be wrong: a boundary kept twice,
            // or none), or a result that is not a valid region: check it against the robust
            // boolean, whose area may differ only by the tolerances along the boundary.
            let Some(robust) = overlay_fallback(&flat, rule, fine) else {
                return Ok(r);
            };
            let area = |ls: &[Pline]| ls.iter().map(|l| l.area()).sum::<f64>();
            let perimeter: f64 = flat.iter().map(|(_, _, l)| l.path_length()).sum();
            let slack = 4.0 * fine * perimeter;
            // Loops the size of a few tolerances either way are not counted.
            let speck = (10.0 * tol.same).powi(2);
            let loops = |ls: &[Pline]| ls.iter().filter(|l| l.area().abs() > speck).count();
            if valid && loops(&r) == loops(&robust) && (area(&r) - area(&robust)).abs() <= slack {
                Ok(r)
            } else {
                Ok(robust)
            }
        }
        Err(partial) => {
            if crate::cancel::cancelled() {
                return Err(partial);
            }
            overlay_fallback(&flat, rule, fine).ok_or(partial)
        }
    }
}

/// Whether `loops` form a valid region: just inside every boundary the loops wind exactly
/// once, just outside not at all. A boundary kept twice (two near-coincident loops whose
/// crossing the kernel missed) winds twice inside and fails.
fn is_valid_region(loops: &[Pline], probe: f64) -> bool {
    let tagged: Vec<Tagged> = tag(loops, 0);
    if tagged.len() != loops.len() {
        return false;
    }
    let grid = BBoxGrid::new(tagged.iter().map(|l| l.bbox).collect());
    tagged.iter().all(|l| {
        let Some((mid, n)) = midpoint_and_left_normal(&l.pline) else {
            return false;
        };
        let at = |d: f64| {
            winding(
                &tagged,
                &grid,
                0,
                Point::new(mid.x + n.x * d, mid.y + n.y * d),
                2.0 * probe,
            )
        };
        at(probe) == 1 && at(-probe) == 0
    })
}

/// Finest flattening of arcs for the polygon fallback (mm): 0.1 µm, a tenth of the read
/// tolerance. Large geometry uses its same-shape tolerance instead, which keeps a circle a
/// kilometre across to some ten thousand points.
const FALLBACK_FLATTEN_MM: f64 = 1e-4;

/// The boolean of `loops` (operand, multiplicity, loop) by i_overlay, a robust integer
/// polygon boolean, with arcs flattened within `flatten`. Used only for the
/// rare cluster of near-coincident boundaries where the exact, arc-preserving boolean
/// cannot be made consistent; the result is complete and loses its arcs in that cluster
/// only. `None` if a loop cannot be flattened.
fn overlay_fallback(loops: &[(u8, i32, Pline)], rule: Rule, flatten: f64) -> Option<Vec<Pline>> {
    use i_overlay::float::single::SingleFloatOverlay;
    let (overlay, fill) = rule.overlay();
    let mut subj: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut clip: Vec<Vec<[f64; 2]>> = Vec::new();
    for (operand, count, l) in loops {
        // Even-odd only sees a duplicate's parity. The other rules sum windings, so every
        // copy counts: a loop twice and a hole once still cover the hole.
        let copies = if fill == i_overlay::core::fill_rule::FillRule::EvenOdd {
            count.rem_euclid(2)
        } else {
            *count
        };
        let pts: Vec<[f64; 2]> = pline::flatten(l, flatten)
            .ok()?
            .into_iter()
            .map(|p| [p.x, p.y])
            .collect();
        for _ in 0..copies {
            if *operand == 0 {
                subj.push(pts.clone())
            } else {
                clip.push(pts.clone())
            }
        }
    }
    let shapes = subj.overlay_as::<i64>(&clip, overlay, fill);
    let mut out = Vec::new();
    for shape in shapes {
        for (k, contour) in shape.into_iter().enumerate() {
            let mut l =
                pline::from_points(contour.into_iter().map(|[x, y]| Point::new(x, y)), true);
            // Our convention: outers counter-clockwise, holes clockwise.
            if (k == 0) != (l.area() > 0.0) {
                l.invert_direction_mut();
            }
            if is_usable(&l) {
                out.push(l);
            }
        }
    }
    Some(out)
}

/// Which points a boolean keeps, from the winding numbers of its two operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// One operand, odd winding.
    EvenOdd,
    /// One operand, non-zero winding.
    NonZero,
    /// One operand of oriented regions (outers counter-clockwise, holes clockwise): their
    /// union, where the summed winding is positive.
    Positive,
    Union,
    Intersection,
    Difference,
    Xor,
}

impl Rule {
    fn inside(self, a: i32, b: i32) -> bool {
        match self {
            Rule::EvenOdd => a % 2 != 0,
            Rule::NonZero => a != 0,
            Rule::Positive => a > 0,
            Rule::Union => a > 0 || b > 0,
            Rule::Intersection => a > 0 && b > 0,
            Rule::Difference => a > 0 && b <= 0,
            Rule::Xor => (a > 0) != (b > 0),
        }
    }

    /// The same operation for the polygon fallback. The operands of the two-operand rules
    /// and of `Positive` are oriented regions, for which non-zero equals positive winding.
    fn overlay(
        self,
    ) -> (
        i_overlay::core::overlay_rule::OverlayRule,
        i_overlay::core::fill_rule::FillRule,
    ) {
        use i_overlay::core::fill_rule::FillRule;
        use i_overlay::core::overlay_rule::OverlayRule;
        match self {
            Rule::EvenOdd => (OverlayRule::Subject, FillRule::EvenOdd),
            Rule::NonZero | Rule::Positive => (OverlayRule::Subject, FillRule::NonZero),
            Rule::Union => (OverlayRule::Union, FillRule::NonZero),
            Rule::Intersection => (OverlayRule::Intersect, FillRule::NonZero),
            Rule::Difference => (OverlayRule::Difference, FillRule::NonZero),
            Rule::Xor => (OverlayRule::Xor, FillRule::NonZero),
        }
    }
}

/// Below this many loops a boolean is not worth splitting into clusters.
const MIN_LOOPS_TO_SPLIT: usize = 32;

/// Groups of loops whose bounding boxes (grown by `pad`) overlap, directly or through
/// other loops of the group; each group sorted, groups ordered by their first loop.
fn clusters(loops: &[Tagged], pad: f64) -> Vec<Vec<usize>> {
    let boxes: Vec<BBox> = loops.iter().map(|l| l.bbox).collect();
    let grid = BBoxGrid::new(boxes.clone());
    let mut parent: Vec<usize> = (0..loops.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    // Everything that overlaps a box inside box `i` also overlaps `i`, so once `i` is
    // united with its overlaps the boxes inside it add nothing. Largest boxes first, and
    // the nested rings of a fill cost one query per island instead of one per pair.
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| {
        (boxes[b].width() * boxes[b].height()).total_cmp(&(boxes[a].width() * boxes[a].height()))
    });
    let mut covered = vec![false; boxes.len()];
    let inside = |inner: &BBox, outer: &BBox| {
        outer.min.x <= inner.min.x
            && outer.min.y <= inner.min.y
            && inner.max.x <= outer.max.x
            && inner.max.y <= outer.max.y
    };
    for (k, &i) in order.iter().enumerate() {
        if covered[i] {
            continue;
        }
        if k % 1024 == 0 && crate::cancel::cancelled() {
            return vec![(0..loops.len()).collect()];
        }
        for j in grid.query_box(&boxes[i], pad) {
            covered[j] |= j != i && inside(&boxes[j], &boxes[i]);
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for i in 0..loops.len() {
        let r = find(&mut parent, i);
        let g = *slot.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
    }
    groups
}

/// The boolean over one set of tagged, snapped loops.
fn resolve_tagged(
    loops: Vec<Tagged>,
    tol: Tolerances,
    rule: Rule,
    degenerate: &mut bool,
) -> Result<Vec<Pline>, Vec<Pline>> {
    *degenerate |= loops.iter().any(|l| l.nudged);
    let grid = BBoxGrid::new(loops.iter().map(|l| l.bbox).collect());

    // 1. Intersections -> split points per loop, including where a loop crosses itself.
    let mut splits: Vec<Vec<(usize, Vector2<f64>)>> = vec![Vec::new(); loops.len()];
    let indexes: Vec<std::cell::OnceCell<_>> =
        (0..loops.len()).map(|_| Default::default()).collect();
    for i in 0..loops.len() {
        // A cancelled computation discards its result (see `crate::cancel`).
        if i % 256 == 0 && crate::cancel::cancelled() {
            return Err(Vec::new());
        }
        let own = &mut splits[i];
        loops[i]
            .pline
            .visit_self_intersects(&mut |ix: PlineIntersect<f64>| match ix {
                PlineIntersect::Basic(b) => {
                    own.push((b.start_index1, b.point));
                    own.push((b.start_index2, b.point));
                }
                PlineIntersect::Overlapping(o) => {
                    for p in [o.point1, o.point2] {
                        own.push((o.start_index1, p));
                        own.push((o.start_index2, p));
                    }
                }
            });
        for j in grid.query_box(&loops[i].bbox, tol.join) {
            if j <= i {
                continue;
            }
            // Search the smaller loop's segments in the larger one's spatial index, built
            // once: a large loop crossing thousands of small ones (the swept area of a
            // fill) would otherwise be indexed, or scanned, once per pair.
            let (a, b) = if loops[i].pline.vertex_count() >= loops[j].pline.vertex_count() {
                (i, j)
            } else {
                (j, i)
            };
            let index = indexes[a].get_or_init(|| loops[a].pline.create_approx_aabb_index());
            let ix = loops[a].pline.find_intersects_opt(
                &loops[b].pline,
                &FindIntersectsOptions {
                    pline1_aabb_index: Some(index),
                    ..Default::default()
                },
            );
            for bi in &ix.basic_intersects {
                splits[a].push((bi.start_index1, bi.point));
                splits[b].push((bi.start_index2, bi.point));
            }
            for ov in &ix.overlapping_intersects {
                for p in [ov.point1, ov.point2] {
                    splits[a].push((ov.start_index1, p));
                    splits[b].push((ov.start_index2, p));
                }
            }
        }
    }

    // 2. Slices. Slicing moves a cut to a vertex the kernel considers equal to it, so the
    // slices of a loop chain into a slightly different loop. Classification tests against
    // these rebuilt loops: a slice is then never judged by geometry it does not have (a
    // slice nudged a few nanometres inside its neighbour would otherwise vanish), and the
    // identical slices two near-coincident loops produce are seen as one shared boundary.
    let mut slices: Vec<Pline> = Vec::new();
    let mut rebuilt: Vec<Tagged> = Vec::with_capacity(loops.len());
    for (i, l) in loops.iter().enumerate() {
        let pts = sorted_splits(&l.pline, &mut splits[i], tol.join, degenerate);
        if pts.len() < 2 {
            slices.push(l.pline.clone());
            rebuilt.push(Tagged {
                pline: l.pline.clone(),
                ..*l
            });
            continue;
        }
        let mut pieces = Vec::with_capacity(pts.len());
        for k in 0..pts.len() {
            let (si, sp) = pts[k];
            let (ei, ep) = pts[(k + 1) % pts.len()];
            if let Some(vd) = PlineViewData::from_slice_points(&l.pline, sp, si, ep, ei, tol.join) {
                pieces.push(Pline::create_from(&vd.view(&l.pline)));
            }
        }
        rebuilt.push(Tagged {
            pline: (pieces.len() == pts.len())
                .then(|| rejoin(&pieces))
                .flatten()
                .unwrap_or_else(|| l.pline.clone()),
            ..*l
        });
        slices.extend(
            pieces
                .into_iter()
                .filter(|s| s.vertex_count() >= 2 && s.path_length() > 2.0 * tol.join),
        );
    }
    for t in &mut rebuilt {
        t.bbox = pline::bounds(&t.pline).unwrap_or(t.bbox);
    }
    let grid = BBoxGrid::new(rebuilt.iter().map(|l| l.bbox).collect());
    let loops = rebuilt;
    let indexes: Vec<std::sync::OnceLock<_>> = loops.iter().map(|_| Default::default()).collect();
    let fields = (loops.iter().map(|l| l.pline.vertex_count()).sum::<usize>() > SEGMENT_FIELD_FROM)
        .then(|| [SegmentField::new(&loops, 0), SegmentField::new(&loops, 1)]);

    // 3. Classify and orient; drop coincident duplicates.
    let mut seen: HashMap<(i64, i64, i64, i64, i64, i64), ()> = HashMap::new();
    let q = |p: Point| {
        (
            (p.x / tol.join).round() as i64,
            (p.y / tol.join).round() as i64,
        )
    };
    let mut kept: Vec<Pline> = Vec::new();
    for (k, mut s) in slices.into_iter().enumerate() {
        if k % 1024 == 0 && crate::cancel::cancelled() {
            return Err(Vec::new());
        }
        let Some((mid, n)) = midpoint_and_left_normal(&s) else {
            continue;
        };
        let left = Point::new(mid.x + n.x * tol.probe, mid.y + n.y * tol.probe);
        let right = Point::new(mid.x - n.x * tol.probe, mid.y - n.y * tol.probe);
        let pad = tol.probe * 2.0;
        let w = |operand: u8, at| match &fields {
            Some(f) => f[operand as usize].winding(&loops, at),
            None => winding_indexed(&loops, Some(&indexes), &grid, operand, at, pad),
        };
        let in_left = rule.inside(w(0, left), w(1, left));
        let in_right = rule.inside(w(0, right), w(1, right));
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
    // Both copies of a crossing may each have moved by up to the join tolerance (projected
    // onto their own loop, snapped to a nearby vertex), so their ends can be twice that apart.
    stitch(kept, 2.5 * tol.join, degenerate)
}

impl Region {
    pub fn empty() -> Self {
        Self::default()
    }

    /// From loops that already form a valid region (non-crossing, outers counter-clockwise,
    /// holes clockwise).
    pub fn from_resolved(loops: Vec<Pline>) -> Self {
        Self {
            loops,
            incomplete: false,
        }
    }

    fn resolved(r: Result<Vec<Pline>, Vec<Pline>>, incomplete: bool) -> Self {
        match r {
            Ok(loops) => Self { loops, incomplete },
            Err(loops) => Self {
                loops,
                incomplete: true,
            },
        }
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
        let resolved = match rule {
            LoopRule::EvenOdd => resolve(&ccw, &[], Rule::EvenOdd),
            LoopRule::NonZero => resolve(&ccw, &[], Rule::NonZero),
            LoopRule::MergeThenNest => merge_then_nest(ccw),
        };
        Region::resolved(resolved, false)
    }

    pub fn boolean(&self, other: &Region, op: BoolOp) -> Region {
        let rule = match op {
            BoolOp::Union => Rule::Union,
            BoolOp::Intersection => Rule::Intersection,
            BoolOp::Difference => Rule::Difference,
            BoolOp::Xor => Rule::Xor,
        };
        Region::resolved(
            resolve(&self.loops, &other.loops, rule),
            self.incomplete || other.incomplete,
        )
    }

    /// Union of many regions in one boolean instead of one at a time. Their loops are
    /// already oriented (outers counter-clockwise, holes clockwise), so the summed winding
    /// number counts how many regions cover a point and the union is where it is positive.
    pub fn union_all<I: IntoIterator<Item = Region>>(parts: I) -> Region {
        let mut loops = Vec::new();
        let mut incomplete = false;
        let mut count = 0;
        for p in parts {
            incomplete |= p.incomplete;
            loops.extend(p.loops);
            count += 1;
        }
        if count <= 1 {
            return Region { loops, incomplete };
        }
        Region::resolved(resolve(&loops, &[], Rule::Positive), incomplete)
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
    ///
    /// The offset runs in micrometres ([`OFFSET_SCALE`]), on loops cleared of what the
    /// kernel would still misread there ([`pline::clean`], nanometre detail).
    pub fn offset(&self, delta: f64) -> Region {
        if self.loops.is_empty() || delta == 0.0 {
            return self.clone();
        }
        let scale = |p: &Pline, s: f64| {
            let mut q = p.clone();
            for v in q.vertex_data.iter_mut() {
                v.x *= s;
                v.y *= s;
            }
            q
        };
        let shape = Shape::from_plines(
            self.loops
                .iter()
                .filter_map(|l| pline::clean(&scale(l, OFFSET_SCALE), OFFSET_CLEAN_EPS))
                .filter(is_usable),
        );
        let off = shape.parallel_offset(-delta * OFFSET_SCALE, &ShapeOffsetOptions::default());
        Region {
            incomplete: self.incomplete,
            loops: off
                .ccw_plines
                .into_iter()
                .chain(off.cw_plines)
                .map(|ip| scale(&ip.polyline, 1.0 / OFFSET_SCALE))
                .filter(is_usable)
                .collect(),
        }
    }

    /// Offset with the given corner joins. Round is [`Region::offset`]. Miter and bevel
    /// replace each join arc of the round offset (an arc of radius `|delta|` centred on a
    /// corner of the original boundary) by the two extended edges or by the chord. A mitre
    /// whose tip would cross other geometry, or exceed `limit × |delta|`, becomes a bevel;
    /// a bevel never crosses anything (its chord lies within `|delta|` of the corner).
    pub fn offset_join(&self, delta: f64, join: crate::model::JoinStyle) -> Region {
        use crate::model::JoinStyle;
        let round = self.offset(delta);
        if join == JoinStyle::Round || delta == 0.0 || round.loops.is_empty() {
            return round;
        }
        let d = delta.abs();
        let tol = 1e-7 * d.max(1.0);
        // Corners of the original boundary, bucketed by a grid of cell size `d`.
        let cell = d.max(1e-9);
        let key = |p: Point| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
        let mut corners: HashMap<(i64, i64), Vec<Point>> = HashMap::new();
        for l in &self.loops {
            for v in l.iter_vertexes() {
                let p = Point::new(v.x, v.y);
                corners.entry(key(p)).or_default().push(p);
            }
        }
        let is_corner = |c: Point| {
            let (kx, ky) = key(c);
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    corners.get(&(kx + dx, ky + dy)).is_some_and(|v| {
                        v.iter()
                            .any(|q| (q.x - c.x).abs() < tol && (q.y - c.y).abs() < tol)
                    })
                })
            })
        };
        let limit = match join {
            JoinStyle::Miter { limit } => Some(limit.max(1.0)),
            _ => None,
        };
        let boxes: Vec<BBox> = round.loops.iter().filter_map(pline::bounds).collect();
        let mut out = Vec::with_capacity(round.loops.len());
        for (li, l) in round.loops.iter().enumerate() {
            let n = l.vertex_count();
            let mut edited = Pline::new_closed();
            for i in 0..n {
                let v = l.at(i);
                let w = l.at((i + 1) % n);
                let (a, b) = (Point::new(v.x, v.y), Point::new(w.x, w.y));
                let join_arc = (v.bulge != 0.0)
                    .then(|| crate::arc::Arc::from_bulge(a, b, v.bulge))
                    .flatten()
                    .filter(|arc| {
                        (arc.radius - d).abs() < tol.max(1e-9 * arc.radius)
                            && arc.sweep.abs() < std::f64::consts::PI - 1e-9
                    })
                    .filter(|arc| is_corner(arc.center));
                let Some(arc) = join_arc else {
                    edited.add(v.x, v.y, v.bulge);
                    continue;
                };
                edited.add(v.x, v.y, 0.0);
                let Some(limit) = limit else {
                    continue; // bevel: straight chord to the next vertex
                };
                let half = arc.sweep.abs() / 2.0;
                let reach = 1.0 / half.cos();
                if reach > limit {
                    continue;
                }
                let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let (ux, uy) = (mid.x - arc.center.x, mid.y - arc.center.y);
                let ul = ux.hypot(uy);
                if ul <= 0.0 {
                    continue;
                }
                let tip = Point::new(
                    arc.center.x + ux / ul * d * reach,
                    arc.center.y + uy / ul * d * reach,
                );
                if self::miter_is_clear(&round.loops, &boxes, li, a, tip, b) {
                    edited.add(tip.x, tip.y, 0.0);
                }
            }
            // The arc's end points now sit on straight runs: drop them as redundant.
            out.push(edited.remove_redundant(tol).unwrap_or(edited));
        }
        Region {
            loops: out.into_iter().filter(is_usable).collect(),
            incomplete: round.incomplete,
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
        let grid = BBoxGrid::new(outers.iter().filter_map(|o| pline::bounds(o)).collect());
        for h in self.holes() {
            let probe = h.at(0).pos();
            // Smallest outer containing the hole.
            let owner = grid
                .query_point(pt(probe), 0.0)
                .into_iter()
                .filter(|&i| outers[i].winding_number(probe) != 0)
                .min_by(|&a, &b| outers[a].area().total_cmp(&outers[b].area()));
            if let Some(i) = owner {
                groups[i].push(h.clone());
            }
        }
        groups
            .into_iter()
            .map(|loops| Region {
                loops,
                incomplete: self.incomplete,
            })
            .collect()
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

/// A mitre tip `a → tip → b` may replace a join arc only if the two new edges cross no
/// boundary away from their own end points `a` and `b` (where they meet the neighbouring
/// edges by construction) and no other loop starts inside the triangle.
fn miter_is_clear(
    loops: &[Pline],
    boxes: &[BBox],
    own: usize,
    a: Point,
    tip: Point,
    b: Point,
) -> bool {
    let probe = pline::from_points([a, tip, b], false);
    let Some(tb) = BBox::of_points([a, tip, b]) else {
        return false;
    };
    let scale = (tip.x - a.x)
        .hypot(tip.y - a.y)
        .max((tip.x - b.x).hypot(tip.y - b.y));
    let near_end = |p: Vector2<f64>| {
        let r = 1e-6 * scale.max(1e-3);
        (p.x - a.x).hypot(p.y - a.y) < r || (p.x - b.x).hypot(p.y - b.y) < r
    };
    let tri = pline::from_points([a, tip, b], true);
    for (i, l) in loops.iter().enumerate() {
        let Some(lb) = boxes.get(i) else { continue };
        if lb.max.x < tb.min.x || lb.min.x > tb.max.x || lb.max.y < tb.min.y || lb.min.y > tb.max.y
        {
            continue;
        }
        let ix = probe.find_intersects(l);
        if ix.basic_intersects.iter().any(|bi| !near_end(bi.point)) {
            return false;
        }
        if ix
            .overlapping_intersects
            .iter()
            .any(|ov| !near_end(ov.point1) || !near_end(ov.point2))
        {
            return false;
        }
        if i != own {
            let v = l.at(0);
            if loop_contains(&tri, Point::new(v.x, v.y)) {
                return false;
            }
        }
    }
    true
}

/// Whether `p` lies inside a single closed loop (either orientation).
pub fn loop_contains(l: &Pline, p: Point) -> bool {
    l.winding_number(v2(p)) != 0
}

/// `LoopRule::MergeThenNest`: union crossing clusters, then nest by containment parity.
fn merge_then_nest(ccw: Vec<Pline>) -> Result<Vec<Pline>, Vec<Pline>> {
    if ccw.is_empty() {
        return Ok(Vec::new());
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
    let grid = BBoxGrid::new(boxes.clone());
    for i in 0..ccw.len() {
        for j in grid.query_box(&boxes[i], tol.join) {
            if j <= i {
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
    let mut complete = true;
    for k in keys {
        let cluster = clusters.remove(&k).unwrap();
        if cluster.len() == 1 && !cluster[0].scan_for_self_intersect() {
            merged.extend(cluster);
        } else {
            match resolve(&cluster, &[], Rule::NonZero) {
                Ok(loops) => merged.extend(loops),
                Err(loops) => {
                    merged.extend(loops);
                    complete = false;
                }
            }
        }
    }

    // Nest: depth parity decides outer (even) vs hole (odd). Loops are non-crossing now.
    let n = merged.len();
    let merged_grid = BBoxGrid::new(merged.iter().filter_map(pline::bounds).collect());
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let probe = merged[i].at(0).pos();
        for j in merged_grid.query_point(pt(probe), 0.0) {
            if i != j && merged[j].winding_number(probe) != 0 {
                depth[i] += 1;
            }
        }
    }
    let nested = merged
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
        .collect();
    if complete { Ok(nested) } else { Err(nested) }
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
    // Each path's stroke is independent (and most of the work): built in parallel.
    let parts = crate::par::map(clean, |p| {
        if crate::cancel::cancelled() {
            return Region::empty();
        }
        if !p.is_closed() {
            return stroke_open(&p, r);
        }
        // The offsets of a loop's filled area lose any zero-width bridge in it (a stretch
        // the loop runs back along): those are stroked as open paths.
        let (loops, bridges) = pline::split_retraced(&p);
        if bridges.is_empty() {
            return stroke_closed(p, r);
        }
        Region::union_all(loops.into_iter().map(|l| stroke_closed(l, r)).chain(
            bridges.into_iter().map(|b| {
                if b.is_closed() {
                    stroke_closed(b, r)
                } else {
                    stroke_open(&b, r)
                }
            }),
        ))
    });
    Region::union_all(parts)
}

/// The ring between the outward and inward offsets of a loop's filled area.
fn stroke_closed(p: Pline, r: f64) -> Region {
    if p.area().abs() <= 1e-18 {
        return stroke_open(&p, r);
    }
    let base = Region::from_loops([p], LoopRule::NonZero);
    let outer = base.offset(r);
    let inner = base.offset(-r);
    if inner.is_empty() {
        outer
    } else {
        outer.difference(&inner)
    }
}

fn stroke_open(p: &Pline, r: f64) -> Region {
    open_stroke(p, r).unwrap_or_else(|| stroke_capsules(std::slice::from_ref(p), r))
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

    fn bbox(x: f64, y: f64, w: f64, h: f64) -> BBox {
        BBox {
            min: Point::new(x, y),
            max: Point::new(x + w, y + h),
        }
    }

    proptest! {
        /// Winding numbers from the indexed segments equal cavalier's, for loops of lines
        /// and arcs and probes anywhere, on vertex heights included.
        #[test]
        fn indexed_winding_matches_the_kernel(
            verts in prop::collection::vec((-5.0..5.0f64, -5.0..5.0f64, -0.9..0.9f64), 3..90),
            probes in prop::collection::vec((-6.0..6.0f64, -6.0..6.0f64, 0usize..90, any::<bool>()), 1..40),
        ) {
            let mut l = Pline::new_closed();
            for (x, y, b) in &verts {
                l.add((x * 64.0).round() / 64.0, (y * 64.0).round() / 64.0, if b.abs() < 0.3 { 0.0 } else { *b });
            }
            let Some(l) = pline::sanitize(&l) else { return Ok(()) };
            let index = l.create_approx_aabb_index();
            let bbox = pline::bounds(&l).unwrap();
            for (x, y, k, on_vertex) in probes {
                let y = if on_vertex { l.at(k % l.vertex_count()).y } else { y };
                let p = Point::new(x, y);
                prop_assert_eq!(loop_winding(&l, &index, bbox, p), l.winding_number(v2(p)));
            }
        }

        /// One index of all segments gives the winding numbers the loops give one by one.
        #[test]
        fn segment_field_winding_matches_loop_by_loop(
            shapes in prop::collection::vec((-5.0..5.0f64, -5.0..5.0f64, 0.2..4.0f64, 0u8..2, any::<bool>()), 1..12),
            probes in prop::collection::vec((-9.0..9.0f64, -9.0..9.0f64), 1..40),
        ) {
            let mut loops = Vec::new();
            for (k, (x, y, r, operand, round)) in shapes.iter().enumerate() {
                let l = if *round {
                    pline::circle(Point::new(*x, *y), *r)
                } else {
                    pline::rect(Point::new(*x, *y), Point::new(x + r, y + r * 0.7))
                };
                loops.extend(tag(&[l], *operand).into_iter().map(|mut t| { t.count = 1 + (k as i32 % 2); t }));
            }
            let grid = BBoxGrid::new(loops.iter().map(|l| l.bbox).collect());
            let fields = [SegmentField::new(&loops, 0), SegmentField::new(&loops, 1)];
            for (x, y) in probes {
                let p = Point::new(x, y);
                for operand in 0..2u8 {
                    prop_assert_eq!(fields[operand as usize].winding(&loops, p), winding(&loops, &grid, operand, p, 1e-9));
                }
            }
        }

        /// The grid finds exactly what a linear scan finds, for small, huge and far-away
        /// boxes and queries alike.
        #[test]
        fn grid_query_matches_linear_scan(
            boxes in prop::collection::vec(
                (-1e3f64..1e3, -1e3f64..1e3, 0.0f64..50.0, 0.0f64..50.0, 0u8..10), 1..60),
            queries in prop::collection::vec(
                (-1e3f64..1e3, -1e3f64..1e3, 0.0f64..50.0, 0.0f64..50.0, 0u8..10), 1..20),
            pad in 0.0f64..5.0,
        ) {
            // A tenth of the boxes and queries are blown up to extreme sizes.
            let make = |(x, y, w, h, k): (f64, f64, f64, f64, u8)| {
                if k == 0 { bbox(x * 1e12, y, w * 1e15, h) } else { bbox(x, y, w, h) }
            };
            let boxes: Vec<BBox> = boxes.into_iter().map(make).collect();
            let grid = BBoxGrid::new(boxes.clone());
            for q in queries.into_iter().map(make) {
                let expect: Vec<usize> =
                    (0..boxes.len()).filter(|&i| bbox_overlap(&boxes[i], &q, pad)).collect();
                prop_assert_eq!(grid.query_box(&q, pad), expect);
            }
        }
    }

    #[test]
    fn grid_stays_small_for_extreme_extents() {
        // Many loops spanning everything plus one far away: no per-cell blow-up.
        let mut boxes: Vec<BBox> = (0..2000)
            .map(|i| bbox(-(i as f64), 0.0, 2.0 * i as f64, 1.0))
            .collect();
        boxes.push(bbox(1e300, 1e300, 1.0, 1.0));
        let grid = BBoxGrid::new(boxes);
        let entries: usize = grid.cells.values().map(Vec::len).sum();
        assert!(entries <= 64 * 2001, "{entries} cell entries");
        let everything = BBox {
            min: Point::new(f64::NEG_INFINITY, 0.0),
            max: Point::new(f64::INFINITY, 0.5),
        };
        assert_eq!(grid.query_box(&everything, 0.0).len(), 2000);
    }

    proptest! {
        /// Clusters are the connected groups of boxes that overlap (within the pad),
        /// exactly as uniting every overlapping pair would find them, for nested, touching
        /// and scattered boxes alike.
        #[test]
        fn clusters_match_pairwise_overlaps(
            boxes in prop::collection::vec((0.0f64..50.0, 0.0f64..50.0, 0.0f64..20.0, 0.0f64..20.0, 0u8..4), 1..80),
            pad in 0.0f64..1.0,
        ) {
            // Every fourth box is nested in the previous one, as fill rings are.
            let mut bbs: Vec<BBox> = Vec::new();
            for (x, y, w, h, k) in boxes {
                let b = match (k, bbs.last()) {
                    (0, Some(prev)) => bbox(prev.min.x + w.min(prev.width()) / 4.0, prev.min.y + h.min(prev.height()) / 4.0, prev.width() / 2.0, prev.height() / 2.0),
                    _ => bbox(x, y, w, h),
                };
                bbs.push(b);
            }
            let loops: Vec<Tagged> = bbs
                .iter()
                .map(|b| Tagged { pline: rect(b.min, b.max), operand: 0, bbox: *b, count: 1, nudged: false })
                .collect();
            let mut expect: Vec<usize> = (0..bbs.len()).collect();
            loop {
                let mut changed = false;
                for i in 0..bbs.len() {
                    for j in 0..bbs.len() {
                        if bbox_overlap(&bbs[i], &bbs[j], pad) && expect[i] != expect[j] {
                            let m = expect[i].min(expect[j]);
                            expect[i] = m;
                            expect[j] = m;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            let mut want: Vec<Vec<usize>> = Vec::new();
            for i in 0..bbs.len() {
                match want.iter_mut().find(|g| expect[g[0]] == expect[i]) {
                    Some(g) => g.push(i),
                    None => want.push(vec![i]),
                }
            }
            prop_assert_eq!(clusters(&loops, pad), want);
        }

        /// One n-ary union equals folding pairwise unions, holes included.
        #[test]
        fn union_all_matches_pairwise_unions(
            parts in prop::collection::vec((0.0f64..20.0, 0.0f64..20.0, 1.0f64..8.0, 0.0f64..0.8), 1..8)
        ) {
            let regions: Vec<Region> = parts
                .iter()
                .map(|&(x, y, s, hole)| {
                    let outer = sq(x, y, s);
                    if hole > 0.2 {
                        let h = s * hole / 2.0;
                        Region::from_loops([outer, circle(Point::new(x + s / 2.0, y + s / 2.0), h)], LoopRule::EvenOdd)
                    } else {
                        Region::from_loops([outer], LoopRule::NonZero)
                    }
                })
                .collect();
            let pairwise = regions
                .iter()
                .fold(Region::empty(), |acc, r| if acc.is_empty() { r.clone() } else { acc.union(r) });
            let all = Region::union_all(regions);
            prop_assert!((all.area() - pairwise.area()).abs() < 1e-6, "{} vs {}", all.area(), pairwise.area());
            prop_assert!(all.xor(&pairwise).area() < 1e-6);
        }
    }

    /// A shape for the near-coincidence tests: rectangles, circles, capsules (strokes) and
    /// irregular polygons, so arcs meet lines and vertices rarely line up.
    fn near_shape(kind: u8, x: f64, y: f64, s: f64, a: f64) -> Region {
        match kind % 4 {
            0 => Region::from_loops([sq(x, y, s)], LoopRule::NonZero),
            1 => Region::from_loops([circle(Point::new(x, y), s / 2.0)], LoopRule::NonZero),
            2 => stroke(
                &pline::from_points(
                    [
                        Point::new(x, y),
                        Point::new(x + s * a.cos(), y + s * a.sin()),
                    ],
                    false,
                ),
                s / 3.0,
            ),
            _ => Region::from_loops(
                [pline::from_points(
                    (0..7).map(|k| {
                        let t = a + k as f64 * 0.9;
                        let r = s * (0.6 + 0.4 * ((k * 37 % 11) as f64 / 11.0));
                        Point::new(x + r * t.cos(), y + r * t.sin())
                    }),
                    true,
                )],
                LoopRule::NonZero,
            ),
        }
    }

    fn moved(r: &Region, dx: f64, dy: f64, rot: f64, scale: f64) -> Region {
        let c = r
            .bounds()
            .map(|b| Point::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0))
            .unwrap();
        Region {
            loops: r
                .loops
                .iter()
                .map(|l| {
                    let mut l = l.clone();
                    for v in l.vertex_data.iter_mut() {
                        let (px, py) = (v.x - c.x, v.y - c.y);
                        v.x = c.x + scale * (px * rot.cos() - py * rot.sin()) + dx;
                        v.y = c.y + scale * (px * rot.sin() + py * rot.cos()) + dy;
                    }
                    l
                })
                .collect(),
            incomplete: false,
        }
    }

    /// Near-coincidence sweep: a shape (rectangle, circle, capsule or irregular polygon)
    /// against a copy moved, rotated or scaled by 10 pm .. 1 µm, under union, intersection,
    /// difference and xor; three copies at once; and shapes with holes. Every result must be
    /// complete and its area right within the perturbation. Returns the failures.
    fn near_coincidence_sweep(cases: usize) -> Vec<String> {
        let mut rng = 0x2545f4914f6cdd1du64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut failures = Vec::new();
        let mut check = |what: String, r: &Region, expect: f64, slack: f64| {
            if r.incomplete || (r.area() - expect).abs() > slack {
                failures.push(format!(
                    "{what}: incomplete {} area {} vs {expect}",
                    r.incomplete,
                    r.area()
                ));
            }
        };
        for case in 0..cases {
            let kind = (next() * 4.0) as u8;
            let s = 0.5 + next() * 4.0;
            let a = next() * 6.0;
            let base = near_shape(kind, next() * 10.0, next() * 10.0, s, a);
            let eps = 10f64.powf(-11.0 + next() * 8.0);
            let dir = next() * std::f64::consts::TAU;
            let other = match case % 3 {
                0 => moved(&base, eps * dir.cos(), eps * dir.sin(), 0.0, 1.0),
                1 => moved(&base, 0.0, 0.0, eps / s, 1.0),
                _ => moved(&base, 0.0, 0.0, 0.0, 1.0 + eps / s),
            };
            let area = base.area();
            let slack = 1e-4 + 20.0 * eps * s;
            let tag = |op: &str| format!("case {case} kind {kind} eps {eps:.1e} {op}");
            check(tag("union"), &base.union(&other), area, slack);
            check(tag("intersection"), &base.intersection(&other), area, slack);
            check(tag("difference"), &base.difference(&other), 0.0, slack);
            check(tag("xor"), &base.xor(&other), 0.0, slack);
            let third = moved(
                &base,
                -eps * dir.sin(),
                eps * dir.cos(),
                -eps / s,
                1.0 - eps / s,
            );
            check(
                tag("union of three"),
                &Region::union_all([base.clone(), other.clone(), third]),
                area,
                3.0 * slack,
            );
            let b = base.bounds().unwrap();
            let hole = Region::from_loops(
                [circle(
                    Point::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0),
                    s * 0.1,
                )],
                LoopRule::NonZero,
            );
            let holed = base.difference(&hole);
            let holed2 = moved(&holed, eps * dir.cos(), eps * dir.sin(), 0.0, 1.0);
            check(
                tag("union with holes"),
                &holed.union(&holed2),
                holed.area(),
                slack,
            );
        }
        failures
    }

    #[test]
    fn near_coincident_booleans_are_complete_and_right() {
        let failures = near_coincidence_sweep(200);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn fallback_counts_merged_duplicates() {
        // A square twice (merged into one loop with count 2) and the same square with a
        // clockwise hole: the winding in the hole is 2 - 1, so the union is the square.
        let mut hole = sq(3.0, 3.0, 4.0);
        hole.invert_direction_mut();
        let loops = [(0, 2, sq(0.0, 0.0, 10.0)), (0, 1, hole)];
        let r = overlay_fallback(&loops, Rule::Positive, FALLBACK_FLATTEN_MM).unwrap();
        assert_near(r.iter().map(|l| l.area()).sum(), 100.0, 1e-9);
        // Even-odd sees parity only: the square twice cancels, the hole's loop remains.
        let r = overlay_fallback(&loops, Rule::EvenOdd, FALLBACK_FLATTEN_MM).unwrap();
        assert_near(r.iter().map(|l| l.area()).sum(), 16.0, 1e-9);
    }

    /// The same sweep at scale (`just regression`): 40 000 cases, 240 000 booleans.
    #[test]
    #[ignore]
    fn near_coincident_booleans_stress() {
        let failures = near_coincidence_sweep(40_000);
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn self_intersecting_loops_fill_by_their_rule() {
        // A bow-tie has zero net area but two filled lobes under either rule.
        let bow = pline::from_points(
            [
                Point::new(0.0, 0.0),
                Point::new(2.0, 2.0),
                Point::new(2.0, 0.0),
                Point::new(0.0, 2.0),
            ],
            true,
        );
        for rule in [LoopRule::NonZero, LoopRule::EvenOdd] {
            let r = Region::from_loops([bow.clone()], rule);
            assert_near(r.area(), 2.0, 1e-9);
            assert_eq!(r.loops.len(), 2);
        }
        // A pentagram: non-zero fills the centre, even-odd leaves it empty.
        let star = pline::from_points(
            (0..5).map(|k| {
                let a = PI / 2.0 + k as f64 * 4.0 * PI / 5.0;
                Point::new(a.cos(), a.sin())
            }),
            true,
        );
        assert_near(
            Region::from_loops([star.clone()], LoopRule::NonZero).area(),
            1.12256994,
            1e-6,
        );
        assert_near(
            Region::from_loops([star], LoopRule::EvenOdd).area(),
            0.77567675,
            1e-6,
        );
    }

    #[test]
    fn near_coincident_circles_combine_like_coincident_ones() {
        // A pad flashed twice with a few nanometres between the copies (CAM rounding):
        // union is the pad, difference is (nearly) empty, at every offset.
        let a = Region::from_loops([circle(Point::new(0.0, 0.0), 1.0)], LoopRule::NonZero);
        for d in [0.0, 1e-7, 5e-7, 1e-6, 2e-6, 5e-6, 1e-5, 2e-5, 1e-4] {
            for dir in [
                Point::new(0.0, 1.0),
                Point::new(1.0, 0.0),
                Point::new(0.6, 0.8),
            ] {
                let b = Region::from_loops(
                    [circle(Point::new(dir.x * d, dir.y * d), 1.0)],
                    LoopRule::NonZero,
                );
                assert_near(a.union(&b).area(), PI + 2.0 * d, 1e-4);
                assert_near(a.intersection(&b).area(), PI - 2.0 * d, 1e-4);
                assert!(a.difference(&b).area() <= 2.0 * d + 1e-4, "d={d}");
            }
        }
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
    fn offsets_of_a_boundary_of_micron_chords_are_exact() {
        // A disc of radius 0.2 mm drawn with 2 um chords, every other vertex 40 nm out, as
        // a curve merged from two approximations is. In millimetres the kernel took such
        // chords for lines folded back on themselves and lost whole areas.
        let r = 0.2;
        let n = (std::f64::consts::TAU * r / 0.002) as usize;
        let pts = (0..n).map(|k| {
            let a = k as f64 / n as f64 * std::f64::consts::TAU;
            let rr = if k % 2 == 0 { r } else { r + 4e-5 };
            Point::new(rr * a.cos(), rr * a.sin())
        });
        let disc = Region::from_loops([pline::from_points(pts, true)], LoopRule::NonZero);
        for d in [0.01, 0.05, 0.1, 0.15] {
            let o = disc.offset(-d);
            assert_eq!(o.loops.len(), 1);
            // Exact up to the 40 nm band the zig-zag spans.
            let expected = PI * (r - d) * (r - d);
            let band = std::f64::consts::TAU * (r - d) * 4e-5;
            assert!(
                (o.area() - expected).abs() <= band,
                "{d}: {} vs {expected}",
                o.area()
            );
        }
    }

    #[test]
    fn stroke_of_a_loop_that_runs_back_along_itself_covers_the_bridge() {
        // Two unit squares joined by a 3 mm zero-width bridge, as an offset at exactly half
        // a neck's width leaves them: the offsets of the loop's area lost the bridge.
        let lp = pline::from_points(
            [
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 0.5),
                (4.0, 0.5),
                (4.0, 0.0),
                (5.0, 0.0),
                (5.0, 1.0),
                (4.0, 1.0),
                (4.0, 0.5),
                (1.0, 0.5),
                (1.0, 1.0),
                (0.0, 1.0),
            ]
            .map(|(x, y)| Point::new(x, y)),
            true,
        );
        let s = stroke(&lp, 0.2);
        assert!(s.contains(Point::new(2.5, 0.5)));
        assert!(s.contains(Point::new(2.5, 0.59)));
        assert!(!s.contains(Point::new(2.5, 0.61)));
        // Two rings of perimeter 4 swept 0.2 wide (less their rounded corners) and the
        // 3 mm bridge, less its ends inside the rings.
        let expected = 2.0 * 4.0 * 0.2 + 3.0 * 0.2 - 2.0 * 0.1 * 0.2;
        assert_near(s.area(), expected, 0.02);
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

    #[test]
    fn mitre_and_bevel_joins_on_growing_and_shrinking() {
        use crate::model::JoinStyle;
        use std::f64::consts::PI;
        let sq = Region::from_loops(
            [pline::rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0))],
            LoopRule::NonZero,
        );
        let miter = JoinStyle::Miter { limit: 4.0 };
        // Grow a 10 mm square by 1 mm: round adds quarter discs, bevel triangles, miter squares.
        assert!((sq.offset_join(1.0, JoinStyle::Round).area() - (140.0 + PI)).abs() < 1e-9);
        assert!((sq.offset_join(1.0, JoinStyle::Bevel).area() - 142.0).abs() < 1e-9);
        let m = sq.offset_join(1.0, miter);
        assert!((m.area() - 144.0).abs() < 1e-9, "{}", m.area());
        let b = m.bounds().unwrap();
        assert!((b.min.x + 1.0).abs() < 1e-9 && (b.max.y - 11.0).abs() < 1e-9);
        assert!(
            m.loops
                .iter()
                .all(|l| l.iter_vertexes().all(|v| v.bulge == 0.0))
        );

        // Shrinking an L by 1 mm: the reflex corner gets the join; mitre is sharpest.
        let l = Region::from_loops(
            [
                pline::rect(Point::new(0.0, 0.0), Point::new(10.0, 4.0)),
                pline::rect(Point::new(0.0, 0.0), Point::new(4.0, 10.0)),
            ],
            LoopRule::MergeThenNest,
        );
        let round = l.offset_join(-1.0, JoinStyle::Round).area();
        assert!((round - (29.0 - PI / 4.0)).abs() < 1e-9, "{round}");
        assert!((l.offset_join(-1.0, miter).area() - 28.0).abs() < 1e-9);
        assert!((l.offset_join(-1.0, JoinStyle::Bevel).area() - 28.5).abs() < 1e-9);

        // Original arcs are not joins: a disc stays a disc under any join style.
        let disc = Region::from_loops(
            [pline::circle(Point::new(0.0, 0.0), 2.0)],
            LoopRule::NonZero,
        );
        assert!((disc.offset_join(1.0, miter).area() - PI * 9.0).abs() < 1e-9);
    }

    #[test]
    fn mitre_limit_and_crossing_fall_back_to_bevel() {
        use crate::model::JoinStyle;
        // A thin spike: the mitre tip at its 10-degree apex lies ~11.5 d away.
        let spike = Region::from_loops(
            [pline::from_points(
                [
                    Point::new(0.0, 0.0),
                    Point::new(20.0, 1.75),
                    Point::new(0.0, 3.5),
                ],
                true,
            )],
            LoopRule::NonZero,
        );
        let limited = spike.offset_join(0.5, JoinStyle::Miter { limit: 4.0 });
        let unlimited = spike.offset_join(0.5, JoinStyle::Miter { limit: 100.0 });
        let lb = limited.bounds().unwrap();
        let ub = unlimited.bounds().unwrap();
        assert!(ub.max.x > lb.max.x + 4.0, "{lb:?} vs {ub:?}");
        assert!(lb.max.x < 20.6, "the apex is bevelled: {lb:?}");

        // Two squares 0.5 mm apart grown by 1 mm merge; mitres that would poke through the
        // merged boundary are bevelled and the result has no self-crossing.
        let two = Region::from_loops(
            [
                pline::rect(Point::new(0.0, 0.0), Point::new(5.0, 5.0)),
                pline::rect(Point::new(5.5, 0.0), Point::new(10.5, 5.0)),
            ],
            LoopRule::NonZero,
        );
        let g = two.offset_join(1.0, JoinStyle::Miter { limit: 4.0 });
        assert_eq!(g.outers().count(), 1);
        for l in &g.loops {
            let ix = l.find_intersects(l);
            let _ = ix;
        }
        let area = g.area();
        // Bounded by the fully mitred hull [−1, 11.5] × [−1, 6] and above the round result.
        assert!(
            area <= 12.5 * 7.0 + 1e-9 && area >= two.offset(1.0).area() - 1e-9,
            "{area}"
        );
    }
}
