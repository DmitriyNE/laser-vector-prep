//! Path-order planning within one output group (spec §7).
//!
//! The only assumption about the machine is the one the spec allows: a beam-off jump is a
//! straight line from the end of one emitted path to the start of the next. Everything here
//! is geometry under that assumption; the controller may still reorder, and nothing is a
//! dose. A plan never changes mark geometry: it reorders paths, reverses open paths and
//! rotates the start vertex (seam) of closed loops.
//!
//! Pipeline: deterministic nearest-entry construction that honours precedence constraints
//! (holes before their outer for cutting) → local improvement (or-opt and 2-opt moves on
//! the group's objective) → orientation and seam choice by dynamic programming.
//!
//! Objective: total jump length, plus in `bleedAware` mode a bleed penalty: the squared
//! coverage of jump footprints (capsules of the configured width) accumulated on a raster
//! over the protected regions. Squaring penalises stacking several jumps over the same spot
//! more than spreading them out, which is what "avoid hotspots" means geometrically.

use std::collections::{BTreeMap, HashMap};

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};
use serde::Serialize;

use crate::geom::{BBox, Point};
use crate::model::{OptimizerMode, OptimizerSettings, PassId};
use crate::pline::Pline;
use crate::region::Region;

/// One path of the group with its planned orientation and seam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathRef {
    pub pass: PassId,
    pub index: usize,
    /// Open paths only: traverse from the generated end to the generated start.
    pub reversed: bool,
    /// Closed loops only: vertex index to start (and end) at.
    pub seam: usize,
}

/// Geometric statistics of a sequence under the straight-jump assumption.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub paths: usize,
    pub jumps: usize,
    pub jump_length_mm: f64,
    pub longest_jump_mm: f64,
    /// Jump-jump intersections (a rough measure of travel tangling).
    pub crossings: usize,
    /// Protected area touched by at least one jump footprint (mm²); `None` without a
    /// protected region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_mm2: Option<f64>,
    /// Σ coverage² over protected cells × cell area (mm²·jumps²): the bleed penalty's basis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_integral: Option<f64>,
    /// Most jump footprints stacked on one protected cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_coverage: Option<u32>,
    /// The objective the planner minimised (mm).
    pub objective: f64,
}

/// Jump footprint coverage raster for the preview heatmap.
#[derive(Clone, Debug, PartialEq)]
pub struct Heatmap {
    pub origin: Point,
    pub cell_mm: f64,
    pub width: usize,
    pub height: usize,
    /// Row-major from `origin` (x right, y up), number of jump footprints covering the cell.
    pub counts: Vec<u16>,
    /// 1 where the cell centre lies in a protected region.
    pub mask: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupPlan {
    pub mode: OptimizerMode,
    pub order: Vec<PathRef>,
    pub before: Stats,
    pub after: Stats,
    pub heatmap: Option<Heatmap>,
}

/// Paths of one pass as planner input.
pub struct PassPaths<'a> {
    pub id: PassId,
    pub paths: &'a [Pline],
    pub precedence: &'a [(usize, usize)],
}

/// Largest raster the footprint model will allocate (cells); the cell size grows past it.
const MAX_CELLS: usize = 4_000_000;
/// Nearest-neighbour candidates kept per path for local moves.
const NEIGHBOURS: usize = 8;

struct Node {
    pass: usize,
    index: usize,
    closed: bool,
    /// Vertices (open: all; closed: all, as seam candidates).
    verts: Vec<Point>,
}

impl Node {
    fn start(&self, reversed: bool, seam: usize) -> Point {
        if self.closed {
            self.verts[seam]
        } else if reversed {
            *self.verts.last().unwrap()
        } else {
            self.verts[0]
        }
    }

    fn end(&self, reversed: bool, seam: usize) -> Point {
        if self.closed {
            self.verts[seam]
        } else if reversed {
            self.verts[0]
        } else {
            *self.verts.last().unwrap()
        }
    }

    /// Entry points a jump can arrive at: both ends of an open path, every vertex of a loop.
    fn entries(&self) -> Vec<(Point, bool, usize)> {
        if self.closed {
            self.verts
                .iter()
                .enumerate()
                .map(|(i, p)| (*p, false, i))
                .collect()
        } else {
            vec![
                (self.verts[0], false, 0),
                (*self.verts.last().unwrap(), true, 0),
            ]
        }
    }

    fn nearest_entry(&self, from: Point) -> (f64, bool, usize) {
        let mut best = (f64::INFINITY, false, 0usize);
        for (p, rev, seam) in self.entries() {
            let d = dist(from, p);
            if d < best.0 {
                best = (d, rev, seam);
            }
        }
        best
    }
}

fn dist(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn nodes_of(passes: &[PassPaths<'_>]) -> (Vec<Node>, Vec<(usize, usize)>) {
    let mut nodes = Vec::new();
    let mut precedence = Vec::new();
    for (pi, pp) in passes.iter().enumerate() {
        let base = nodes.len();
        for (i, p) in pp.paths.iter().enumerate() {
            let verts: Vec<Point> = p.iter_vertexes().map(|v| Point::new(v.x, v.y)).collect();
            if verts.is_empty() {
                continue;
            }
            nodes.push(Node {
                pass: pi,
                index: i,
                closed: p.is_closed(),
                verts,
            });
        }
        // Paths are pushed in order, so pass-local indices map directly when none is empty;
        // map through a lookup to stay correct otherwise.
        let lookup: HashMap<usize, usize> = nodes[base..]
            .iter()
            .enumerate()
            .map(|(k, n)| (n.index, base + k))
            .collect();
        for &(a, b) in pp.precedence {
            if let (Some(&na), Some(&nb)) = (lookup.get(&a), lookup.get(&b)) {
                precedence.push((na, nb));
            }
        }
    }
    (nodes, precedence)
}

// ---------------------------------------------------------------------------------------
// Spatial index over entry points

struct EntryGrid {
    origin: Point,
    cell: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<(usize, Point)>>, // node id, entry point
}

impl EntryGrid {
    fn new(bounds: BBox, n_points: usize) -> EntryGrid {
        let w = bounds.width().max(1e-6);
        let h = bounds.height().max(1e-6);
        let target_cells = (n_points / 4).clamp(1, 1 << 20) as f64;
        // Cell from the larger extent so a line of points gives a 1 x k grid, not 1 x huge.
        let side = target_cells.sqrt().ceil().max(1.0);
        let cell = (w.max(h) / side).max(1e-6);
        let cols = (w / cell).ceil() as usize + 1;
        let rows = (h / cell).ceil() as usize + 1;
        EntryGrid {
            origin: bounds.min,
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
        }
    }

    fn cell_of(&self, p: Point) -> (usize, usize) {
        let cx = (((p.x - self.origin.x) / self.cell).floor().max(0.0) as usize).min(self.cols - 1);
        let cy = (((p.y - self.origin.y) / self.cell).floor().max(0.0) as usize).min(self.rows - 1);
        (cx, cy)
    }

    fn insert(&mut self, node: usize, p: Point) {
        let (cx, cy) = self.cell_of(p);
        self.cells[cy * self.cols + cx].push((node, p));
    }

    fn remove_node(&mut self, node: usize, points: &[Point]) {
        for p in points {
            let (cx, cy) = self.cell_of(*p);
            self.cells[cy * self.cols + cx].retain(|(n, _)| *n != node);
        }
    }

    /// Visit the cells at Chebyshev distance exactly `ring` from `(cx, cy)`.
    fn ring_cells(&self, cx: usize, cy: usize, ring: usize, mut f: impl FnMut(&[(usize, Point)])) {
        let (x0, x1) = (cx as i64 - ring as i64, cx as i64 + ring as i64);
        let (y0, y1) = (cy as i64 - ring as i64, cy as i64 + ring as i64);
        for y in y0.max(0)..=y1.min(self.rows as i64 - 1) {
            let edge_row = y == y0 || y == y1;
            let xs: Vec<i64> = if edge_row {
                (x0.max(0)..=x1.min(self.cols as i64 - 1)).collect()
            } else {
                [x0, x1]
                    .into_iter()
                    .filter(|x| *x >= 0 && *x < self.cols as i64)
                    .collect()
            };
            for x in xs {
                f(&self.cells[y as usize * self.cols + x as usize]);
            }
        }
    }

    /// Up to `k` nearest distinct nodes to `from` (by their closest entry point).
    fn k_nearest(&self, from: Point, k: usize, exclude: Option<usize>) -> Vec<(usize, f64)> {
        let (cx, cy) = self.cell_of(from);
        let max_ring = self.cols.max(self.rows);
        let mut best: HashMap<usize, f64> = HashMap::new();
        for ring in 0..=max_ring {
            if best.len() >= k {
                let mut ds: Vec<f64> = best.values().copied().collect();
                ds.sort_by(|a, b| a.partial_cmp(b).unwrap());
                if ds[k - 1] <= (ring as f64 - 1.0).max(0.0) * self.cell {
                    break;
                }
            }
            self.ring_cells(cx, cy, ring, |cell| {
                for (n, p) in cell {
                    if Some(*n) == exclude {
                        continue;
                    }
                    let d = dist(from, *p);
                    let e = best.entry(*n).or_insert(d);
                    if d < *e {
                        *e = d;
                    }
                }
            });
        }
        let mut out: Vec<(usize, f64)> = best.into_iter().collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0)));
        out.truncate(k);
        out
    }

    /// Nearest entry point of any inserted node to `from`.
    fn nearest(&self, from: Point) -> Option<(usize, Point, f64)> {
        let (cx, cy) = self.cell_of(from);
        let max_ring = self.cols.max(self.rows);
        let mut best: Option<(usize, Point, f64)> = None;
        for ring in 0..=max_ring {
            if let Some((_, _, d)) = best
                && d <= (ring as f64 - 1.0).max(0.0) * self.cell
            {
                break;
            }
            self.ring_cells(cx, cy, ring, |cell| {
                for (n, p) in cell {
                    let d = dist(from, *p);
                    if best.is_none_or(|(_, _, bd)| d < bd) {
                        best = Some((*n, *p, d));
                    }
                }
            });
        }
        best
    }
}

// ---------------------------------------------------------------------------------------
// Footprint raster

struct Raster {
    origin: Point,
    cell: f64,
    width: usize,
    height: usize,
    counts: Vec<u16>,
    mask: Vec<u8>,
    footprint_r: f64,
}

/// Rows of the raster stamped together when the whole sequence is re-stamped in parallel.
const BAND_ROWS: usize = 32;

impl Raster {
    fn new(bounds: BBox, cell_mm: f64, footprint_mm: f64, protected: Option<&Region>) -> Raster {
        let r = footprint_mm.max(1e-6) / 2.0;
        let b = bounds.expanded(r + cell_mm).unwrap_or(bounds);
        let mut cell = cell_mm.max(1e-4);
        let mut width = (b.width() / cell).ceil() as usize + 1;
        let mut height = (b.height() / cell).ceil() as usize + 1;
        while width * height > MAX_CELLS {
            cell *= 2.0;
            width = (b.width() / cell).ceil() as usize + 1;
            height = (b.height() / cell).ceil() as usize + 1;
        }
        let mut raster = Raster {
            origin: b.min,
            cell,
            width,
            height,
            counts: vec![0; width * height],
            mask: vec![0; width * height],
            footprint_r: r,
        };
        if let Some(region) = protected {
            raster.fill_mask(region);
        }
        raster
    }

    fn centre(&self, ix: usize, iy: usize) -> Point {
        Point::new(
            self.origin.x + (ix as f64 + 0.5) * self.cell,
            self.origin.y + (iy as f64 + 0.5) * self.cell,
        )
    }

    /// Even-odd scanline fill of the region's loops (resolved loops alternate outer/hole).
    fn fill_mask(&mut self, region: &Region) {
        let Ok(loops) = region.flatten(self.cell / 2.0) else {
            return;
        };
        let mut edges: Vec<(Point, Point)> = Vec::new();
        for l in &loops {
            for i in 0..l.len() {
                let a = l[i];
                let b = l[(i + 1) % l.len()];
                if a.y != b.y {
                    edges.push((a, b));
                }
            }
        }
        if edges.is_empty() {
            return;
        }
        // Bucket edges by the rows they span.
        let mut by_row: Vec<Vec<usize>> = vec![Vec::new(); self.height];
        for (ei, (a, b)) in edges.iter().enumerate() {
            let (ymin, ymax) = (a.y.min(b.y), a.y.max(b.y));
            let r0 = (((ymin - self.origin.y) / self.cell) - 0.5).ceil().max(0.0) as usize;
            let r1 = (((ymax - self.origin.y) / self.cell) - 0.5)
                .floor()
                .max(-1.0) as i64;
            for row in r0 as i64..=r1.min(self.height as i64 - 1) {
                by_row[row as usize].push(ei);
            }
        }
        let mut xs: Vec<f64> = Vec::new();
        for iy in 0..self.height {
            let y = self.centre(0, iy).y;
            xs.clear();
            for &ei in &by_row[iy] {
                let (a, b) = edges[ei];
                // Half-open rule on y to count each vertex crossing once.
                if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
                    let t = (y - a.y) / (b.y - a.y);
                    xs.push(a.x + t * (b.x - a.x));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in xs.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                let x0 = (((pair[0] - self.origin.x) / self.cell) - 0.5)
                    .ceil()
                    .max(0.0) as usize;
                let x1 = (((pair[1] - self.origin.x) / self.cell) - 0.5).floor();
                if x1 < 0.0 {
                    continue;
                }
                let x1 = (x1 as usize).min(self.width - 1);
                for ix in x0..=x1 {
                    self.mask[iy * self.width + ix] = 1;
                }
            }
        }
    }

    fn has_mask(&self) -> bool {
        self.mask.iter().any(|m| *m != 0)
    }

    /// Visit, once each, the cells of rows `rows` whose centre lies within the footprint
    /// of the jump `a -> b` (a capsule). The capsule is convex, so each row is scanned
    /// over an interval of x that contains it, with a cell of slack for rounding, and the
    /// exact distance test decides.
    fn cells_in_rows(
        &self,
        a: Point,
        b: Point,
        rows: std::ops::Range<usize>,
        mut f: impl FnMut(usize),
    ) {
        let r = self.footprint_r;
        let slack = r + self.cell;
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        // Cell index ranges, along one axis, of the centres within [lo, hi].
        let first = |lo: f64, origin: f64| ((lo - origin) / self.cell - 0.5).ceil();
        let last = |hi: f64, origin: f64| ((hi - origin) / self.cell - 0.5).floor();
        let y0 = first(a.y.min(b.y) - slack, self.origin.y).max(rows.start as f64);
        let y1 = last(a.y.max(b.y) + slack, self.origin.y).min(rows.end as f64 - 1.0);
        if y0 > y1 {
            return;
        }
        for iy in y0 as usize..=y1 as usize {
            let y = self.origin.y + (iy as f64 + 0.5) * self.cell;
            // Points of the jump within `slack` of this row in y.
            let (t0, t1) = if dy == 0.0 {
                if (y - a.y).abs() > slack {
                    continue;
                }
                (0.0, 1.0)
            } else {
                let (u, v) = ((y - slack - a.y) / dy, (y + slack - a.y) / dy);
                (u.min(v).max(0.0), u.max(v).min(1.0))
            };
            if t0 > t1 {
                continue;
            }
            let (xa, xb) = (a.x + t0 * dx, a.x + t1 * dx);
            let x0 = first(xa.min(xb) - slack, self.origin.x).max(0.0);
            let x1 = last(xa.max(xb) + slack, self.origin.x).min(self.width as f64 - 1.0);
            if x0 > x1 {
                continue;
            }
            for ix in x0 as usize..=x1 as usize {
                if seg_dist(self.centre(ix, iy), a, b) <= r {
                    f(iy * self.width + ix);
                }
            }
        }
    }

    fn cells(&self, a: Point, b: Point, f: impl FnMut(usize)) {
        self.cells_in_rows(a, b, 0..self.height, f);
    }

    /// Protected cells a jump's footprint would cover, without changing anything.
    fn probe(&self, a: Point, b: Point) -> f64 {
        let mut n = 0.0;
        self.cells(a, b, |i| {
            if self.mask[i] != 0 {
                n += 1.0;
            }
        });
        n
    }

    /// Add a jump; returns the increase of Σ coverage² over protected cells (in cells).
    fn add(&mut self, a: Point, b: Point) -> f64 {
        let mut cells = Vec::new();
        self.cells(a, b, |i| cells.push(i));
        let mut delta = 0.0;
        for i in cells {
            let c = self.counts[i];
            if self.mask[i] != 0 {
                delta += 2.0 * c as f64 + 1.0;
            }
            self.counts[i] = c.saturating_add(1);
        }
        delta
    }

    /// Remove a jump previously added; returns the (negative) change of Σ coverage².
    fn remove(&mut self, a: Point, b: Point) -> f64 {
        let mut cells = Vec::new();
        self.cells(a, b, |i| cells.push(i));
        let mut delta = 0.0;
        for i in cells {
            let c = self.counts[i];
            if self.mask[i] != 0 && c > 0 {
                delta += -2.0 * c as f64 + 1.0;
            }
            self.counts[i] = c.saturating_sub(1);
        }
        delta
    }

    /// Replace the counts by the footprints of `jumps`; returns Σ coverage² over protected
    /// cells (in cells). Bands of rows are stamped in parallel; counts are integers, so the
    /// result does not depend on how the work is split.
    fn stamp_all(&mut self, jumps: &[Jump]) -> f64 {
        let bands: Vec<std::ops::Range<usize>> = (0..self.height)
            .step_by(BAND_ROWS)
            .map(|y| y..(y + BAND_ROWS).min(self.height))
            .collect();
        let raster = &*self;
        let counts = crate::par::map(bands, |rows| {
            let base = rows.start * raster.width;
            let mut counts = vec![0u16; rows.len() * raster.width];
            for &(a, b) in jumps {
                raster.cells_in_rows(a, b, rows.clone(), |i| {
                    let c = &mut counts[i - base];
                    *c = c.saturating_add(1);
                });
            }
            counts
        });
        self.counts = counts.concat();
        self.counts
            .iter()
            .zip(&self.mask)
            .filter(|(_, m)| **m != 0)
            .map(|(c, _)| f64::from(*c) * f64::from(*c))
            .sum()
    }

    fn cell_area(&self) -> f64 {
        self.cell * self.cell
    }

    fn protected_stats(&self) -> (f64, f64, u32) {
        let mut covered = 0usize;
        let mut sq = 0.0;
        let mut peak = 0u32;
        for (c, m) in self.counts.iter().zip(&self.mask) {
            if *m != 0 && *c > 0 {
                covered += 1;
                sq += (*c as f64) * (*c as f64);
                peak = peak.max(*c as u32);
            }
        }
        (
            covered as f64 * self.cell_area(),
            sq * self.cell_area(),
            peak,
        )
    }
}

fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    if l2 <= 0.0 {
        return dist(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0);
    dist(p, Point::new(a.x + t * dx, a.y + t * dy))
}

fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let orient =
        |p: Point, q: Point, r: Point| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    (o1 > 0.0) != (o2 > 0.0)
        && (o3 > 0.0) != (o4 > 0.0)
        && o1 != 0.0
        && o2 != 0.0
        && o3 != 0.0
        && o4 != 0.0
}

// ---------------------------------------------------------------------------------------
// Sequence state

/// A sequence of nodes with orientation/seam choices.
#[derive(Clone)]
struct Seq {
    order: Vec<usize>,
    reversed: Vec<bool>,
    seam: Vec<usize>,
}

impl Seq {
    fn start(&self, nodes: &[Node], k: usize) -> Point {
        let n = self.order[k];
        nodes[n].start(self.reversed[n], self.seam[n])
    }

    fn end(&self, nodes: &[Node], k: usize) -> Point {
        let n = self.order[k];
        nodes[n].end(self.reversed[n], self.seam[n])
    }

    /// All jumps of the sequence including the optional assumed start/end points.
    fn jumps(
        &self,
        nodes: &[Node],
        start: Option<Point>,
        end: Option<Point>,
    ) -> Vec<(Point, Point)> {
        let mut out = Vec::new();
        if self.order.is_empty() {
            return out;
        }
        if let Some(s) = start {
            out.push((s, self.start(nodes, 0)));
        }
        for k in 0..self.order.len() - 1 {
            out.push((self.end(nodes, k), self.start(nodes, k + 1)));
        }
        if let Some(e) = end {
            out.push((self.end(nodes, self.order.len() - 1), e));
        }
        out
    }

    fn jump_length(&self, nodes: &[Node], start: Option<Point>, end: Option<Point>) -> f64 {
        self.jumps(nodes, start, end)
            .iter()
            .map(|(a, b)| dist(*a, *b))
            .sum()
    }
}

/// Greedy nearest-entry construction honouring precedence.
fn construct(nodes: &[Node], precedence: &[(usize, usize)], start: Option<Point>) -> Seq {
    let n = nodes.len();
    let mut remaining_preds = vec![0usize; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in precedence {
        remaining_preds[b] += 1;
        successors[a].push(b);
    }
    let all_points = nodes.iter().flat_map(|nd| nd.verts.iter().copied());
    let bounds = BBox::of_points(all_points).unwrap_or(BBox {
        min: Point::new(0.0, 0.0),
        max: Point::new(1.0, 1.0),
    });
    let n_points: usize = nodes.iter().map(|nd| nd.verts.len()).sum();
    let mut grid = EntryGrid::new(bounds, n_points);
    let entry_points =
        |nd: &Node| -> Vec<Point> { nd.entries().into_iter().map(|(p, _, _)| p).collect() };
    for (i, nd) in nodes.iter().enumerate() {
        if remaining_preds[i] == 0 {
            for p in entry_points(nd) {
                grid.insert(i, p);
            }
        }
    }
    let mut seq = Seq {
        order: Vec::with_capacity(n),
        reversed: vec![false; n],
        seam: vec![0; n],
    };
    // Without an assumed start, begin at the first generated path (deterministic).
    let mut current = start.unwrap_or_else(|| {
        nodes
            .first()
            .map(|nd| nd.verts[0])
            .unwrap_or(Point::new(0.0, 0.0))
    });
    let mut first = start.is_none();
    while seq.order.len() < n {
        // Cancelled (the result is discarded) or cyclic constraints: the rest follows in
        // generated order, so the sequence stays a permutation.
        let stop = seq.order.len() % 1024 == 0 && crate::cancel::cancelled();
        let pick = if stop {
            None
        } else if first {
            first = false;
            // The first generated path that is available.
            (0..n).find(|&i| remaining_preds[i] == 0)
        } else {
            grid.nearest(current).map(|(i, _, _)| i)
        };
        let Some(i) = pick else {
            let mut placed = vec![false; n];
            for &j in &seq.order {
                placed[j] = true;
            }
            seq.order.extend((0..n).filter(|&j| !placed[j]));
            break;
        };
        let (_, rev, seam) = nodes[i].nearest_entry(current);
        seq.reversed[i] = rev;
        seq.seam[i] = seam;
        seq.order.push(i);
        grid.remove_node(i, &entry_points(&nodes[i]));
        current = nodes[i].end(rev, seam);
        for &s in &successors[i] {
            remaining_preds[s] -= 1;
            if remaining_preds[s] == 0 {
                for p in entry_points(&nodes[s]) {
                    grid.insert(s, p);
                }
            }
        }
    }
    seq
}

/// Per-jump cost used by the orientation/seam DP: length plus, in bleed-aware runs, the
/// footprint's protected coverage (single-jump proxy for the squared penalty).
fn jump_cost(obj: &Objective<'_>, a: Point, b: Point) -> f64 {
    let len = dist(a, b);
    if obj.bleed_weight > 0.0
        && let Some(r) = &obj.raster
    {
        let scale = r.cell_area() / (2.0 * r.footprint_r);
        return len + obj.bleed_weight * scale * r.probe(a, b);
    }
    len
}

fn nearest_vertex(nd: &Node, to: Point) -> usize {
    let mut best = (f64::INFINITY, 0usize);
    for (i, v) in nd.verts.iter().enumerate() {
        let d = dist(to, *v);
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

/// Orientation (open paths) and seam (closed loops) choice for a fixed order, by dynamic
/// programming over a few candidate states per path: both orientations of an open path;
/// for a loop, the seam nearest to each possible predecessor end and the seams nearest to
/// the successor's entry points.
fn polish(seq: &mut Seq, obj: &Objective<'_>) {
    let nodes = obj.nodes;
    let (start, end) = (obj.start, obj.end);
    let m = seq.order.len();
    if m == 0 {
        return;
    }
    let node = |k: usize| &nodes[seq.order[k]];
    // States: (reversed, seam) candidates per position.
    let mut states: Vec<Vec<(bool, usize)>> = Vec::with_capacity(m);
    for k in 0..m {
        let nd = node(k);
        if !nd.closed {
            states.push(vec![(false, 0), (true, 0)]);
            continue;
        }
        let mut seams: Vec<usize> = Vec::new();
        // Towards the predecessor's possible end points.
        if k == 0 {
            if let Some(sp) = start {
                seams.push(nearest_vertex(nd, sp));
            }
        } else {
            let prev = node(k - 1);
            for &(rev, seam) in &states[k - 1] {
                seams.push(nearest_vertex(nd, prev.end(rev, seam)));
            }
        }
        // Towards the successor's entry points.
        if k + 1 < m {
            let next = node(k + 1);
            if next.closed {
                seams.push(nearest_vertex(
                    nd,
                    next.verts[nearest_vertex(next, nd.verts[0])],
                ));
            } else {
                seams.push(nearest_vertex(nd, next.verts[0]));
                seams.push(nearest_vertex(nd, *next.verts.last().unwrap()));
            }
        } else if let Some(ep) = end {
            seams.push(nearest_vertex(nd, ep));
        }
        if seams.is_empty() {
            seams.push(0);
        }
        seams.sort_unstable();
        seams.dedup();
        states.push(seams.into_iter().map(|s| (false, s)).collect());
    }
    let mut cost: Vec<Vec<f64>> = states
        .iter()
        .map(|st| vec![f64::INFINITY; st.len()])
        .collect();
    let mut back: Vec<Vec<usize>> = states.iter().map(|st| vec![0usize; st.len()]).collect();
    for (si, &(rev, seam)) in states[0].iter().enumerate() {
        cost[0][si] = match start {
            Some(sp) => jump_cost(obj, sp, node(0).start(rev, seam)),
            None => 0.0,
        };
    }
    // The costs of the jumps between consecutive positions' states (most of the work, and
    // independent of each other) in parallel; row-major by (previous state, state).
    let edges: Vec<Vec<f64>> = crate::par::map((1..m).collect(), |k| {
        let mut c = Vec::with_capacity(states[k - 1].len() * states[k].len());
        for &(prev_rev, prev_seam) in &states[k - 1] {
            let from = node(k - 1).end(prev_rev, prev_seam);
            for &(rev, seam) in &states[k] {
                c.push(jump_cost(obj, from, node(k).start(rev, seam)));
            }
        }
        c
    });
    for k in 1..m {
        let width = states[k].len();
        for ps in 0..states[k - 1].len() {
            if !cost[k - 1][ps].is_finite() {
                continue;
            }
            for si in 0..width {
                let c = cost[k - 1][ps] + edges[k - 1][ps * width + si];
                if c < cost[k][si] {
                    cost[k][si] = c;
                    back[k][si] = ps;
                }
            }
        }
    }
    let mut best = 0usize;
    let mut best_cost = f64::INFINITY;
    for (si, &(rev, seam)) in states[m - 1].clone().iter().enumerate() {
        if !cost[m - 1][si].is_finite() {
            continue;
        }
        let c = cost[m - 1][si]
            + match end {
                Some(ep) => jump_cost(obj, node(m - 1).end(rev, seam), ep),
                None => 0.0,
            };
        if c < best_cost {
            best_cost = c;
            best = si;
        }
    }
    let mut si = best;
    for k in (0..m).rev() {
        let (rev, seam) = states[k][si];
        let n = seq.order[k];
        seq.reversed[n] = rev;
        seq.seam[n] = seam;
        si = back[k][si];
    }
}

/// Objective evaluation context. The raster, when present, always holds the footprints of
/// the current sequence's jumps.
struct Objective<'a> {
    nodes: &'a [Node],
    start: Option<Point>,
    end: Option<Point>,
    bleed_weight: f64,
    raster: Option<Raster>,
}

impl Objective<'_> {
    fn bleed_scale(&self) -> f64 {
        // Σ coverage² is in cells; cell area / footprint width turns it into a length so it
        // can be weighed against jump length.
        match &self.raster {
            Some(r) => r.cell_area() / (2.0 * r.footprint_r),
            None => 0.0,
        }
    }

    /// Re-stamp the raster for `seq` and return the full objective.
    fn total(&mut self, seq: &Seq) -> f64 {
        let len = seq.jump_length(self.nodes, self.start, self.end);
        let scale = self.bleed_scale();
        if let Some(r) = &mut self.raster {
            let sq = r.stamp_all(&seq.jumps(self.nodes, self.start, self.end));
            len + self.bleed_weight * sq * scale
        } else {
            len
        }
    }

    fn uses_raster(&self) -> bool {
        self.bleed_weight > 0.0 && self.raster.is_some()
    }
}

type Jump = (Point, Point);

fn jump_len(j: &Jump) -> f64 {
    dist(j.0, j.1)
}

/// A local move of [`improve`].
enum Move {
    /// The node at position `i` moves to just after position `a` (`-1`: to the front),
    /// with the given orientation and seam.
    Relocate {
        i: usize,
        a: i64,
        rev: bool,
        seam: usize,
    },
    /// The run of positions `first..=last` is reversed.
    Reverse { first: usize, last: usize },
}

impl Move {
    /// Where the move puts the node at position `k` of a sequence of `m` nodes.
    fn moved(&self, k: usize, m: usize) -> usize {
        match *self {
            Move::Relocate { i, a, .. } => {
                let at = Self::insertion(i, a, m);
                if k == i {
                    return at;
                }
                let k = if k > i { k - 1 } else { k };
                if k >= at { k + 1 } else { k }
            }
            Move::Reverse { first, last } => {
                if (first..=last).contains(&k) {
                    first + last - k
                } else {
                    k
                }
            }
        }
    }

    /// Index at which a relocated node is inserted once removed from position `i`.
    fn insertion(i: usize, a: i64, m: usize) -> usize {
        let mut at = (a + 1) as usize;
        if a >= 0 && a as usize >= i {
            at -= 1;
        }
        at.min(m - 1)
    }

    fn apply(&self, seq: &mut Seq, nodes: &[Node]) {
        match *self {
            Move::Relocate { i, a, rev, seam } => {
                let m = seq.order.len();
                let n = seq.order.remove(i);
                seq.order.insert(Self::insertion(i, a, m), n);
                seq.reversed[n] = rev;
                seq.seam[n] = seam;
            }
            Move::Reverse { first, last } => {
                seq.order[first..=last].reverse();
                for &n in &seq.order[first..=last] {
                    if !nodes[n].closed {
                        seq.reversed[n] = !seq.reversed[n];
                    }
                }
            }
        }
    }
}

/// A move that improves the estimated objective, with the jumps it removes and adds.
struct Candidate {
    mv: Move,
    removed: Vec<Jump>,
    added: Vec<Jump>,
}

/// What a scan for moves reads: the current sequence, unchanged while candidates are
/// collected, so positions can be scanned in parallel.
struct Scan<'a> {
    seq: &'a Seq,
    obj: &'a Objective<'a>,
    pos: &'a [usize],
    neighbours: &'a [Vec<usize>],
    precedence: &'a [(usize, usize)],
    /// Weight of a protected cell, in mm of jump length.
    scale: f64,
}

impl Scan<'_> {
    fn m(&self) -> usize {
        self.seq.order.len()
    }

    /// The point a jump into position `k + 1` comes from (the assumed start before the
    /// first path).
    fn end_at(&self, k: i64) -> Option<Point> {
        if k < 0 {
            self.obj.start
        } else {
            Some(self.seq.end(self.obj.nodes, k as usize))
        }
    }

    /// The point a jump out of position `k - 1` goes to (the assumed end after the last).
    fn start_at(&self, k: usize) -> Option<Point> {
        if k >= self.m() {
            self.obj.end
        } else {
            Some(self.seq.start(self.obj.nodes, k))
        }
    }

    /// Protected cells under a jump's footprint, or 0 without a bleed penalty.
    fn probe(&self, j: &Jump) -> f64 {
        match (&self.obj.raster, self.obj.bleed_weight > 0.0) {
            (Some(r), true) => r.probe(j.0, j.1),
            _ => 0.0,
        }
    }

    fn respects(&self, mv: &Move) -> bool {
        let m = self.m();
        self.precedence
            .iter()
            .all(|&(a, b)| mv.moved(self.pos[a], m) < mv.moved(self.pos[b], m))
    }

    /// Or-opt moves of the node at position `i` next to its spatial neighbours, in scan
    /// order, each with the orientation or seam that suits its new slot best.
    fn relocations(&self, i: usize) -> Vec<Candidate> {
        let (seq, nodes, m) = (self.seq, self.obj.nodes, self.m());
        let uses_raster = self.obj.uses_raster();
        let n = seq.order[i];
        let nd = &nodes[n];
        let prev_i = self.end_at(i as i64 - 1);
        let next_i = self.start_at(i + 1);
        let cur_s = seq.start(nodes, i);
        let cur_e = seq.end(nodes, i);
        // Jumps around i that disappear when it moves, and the one that joins the gap.
        let removed_i: Vec<Jump> = [(prev_i, Some(cur_s)), (Some(cur_e), next_i)]
            .into_iter()
            .filter_map(|(x, y)| Some((x?, y?)))
            .collect();
        let joined: Option<Jump> = prev_i.zip(next_i);
        let len_removed_i: f64 = removed_i.iter().map(jump_len).sum();
        let cov_removed_i: f64 = removed_i.iter().map(|j| self.probe(j)).sum();
        let len_joined = joined.as_ref().map_or(0.0, jump_len);
        let cov_joined = joined.as_ref().map_or(0.0, |j| self.probe(j));
        let mut out = Vec::new();
        for &nb in &self.neighbours[n] {
            let j = self.pos[nb] as i64;
            for a in [j, j - 1] {
                if a == i as i64 || a == i as i64 - 1 || a < -1 || a >= m as i64 {
                    continue;
                }
                let a_end = self.end_at(a);
                let after_a = if (a + 1) as usize == i {
                    self.start_at(i + 1)
                } else {
                    self.start_at((a + 1) as usize)
                };
                let removed_a: Option<Jump> = a_end.zip(after_a);
                let removed_len = len_removed_i + removed_a.as_ref().map_or(0.0, jump_len);
                let removed_cov = cov_removed_i + removed_a.as_ref().map_or(0.0, |j| self.probe(j));
                // Orientation / seam candidates of the moved node in its new slot.
                let candidates: Vec<(bool, usize)> = if nd.closed {
                    let mut v = Vec::new();
                    if let Some(p) = a_end {
                        v.push((false, nearest_vertex(nd, p)));
                    }
                    if let Some(p) = after_a {
                        v.push((false, nearest_vertex(nd, p)));
                    }
                    if v.is_empty() {
                        v.push((false, seq.seam[n]));
                    }
                    v
                } else {
                    vec![(false, 0), (true, 0)]
                };
                let mut best: Option<(f64, (bool, usize), Vec<Jump>)> = None;
                for (rev, seam) in candidates {
                    let ns = nd.start(rev, seam);
                    let ne = nd.end(rev, seam);
                    let into: Option<Jump> = a_end.map(|p| (p, ns));
                    let out_of: Option<Jump> = after_a.map(|p| (ne, p));
                    let len_delta = len_joined
                        + into.as_ref().map_or(0.0, jump_len)
                        + out_of.as_ref().map_or(0.0, jump_len)
                        - removed_len;
                    // The bleed term can gain at most what the removed jumps covered.
                    if len_delta - self.scale * (removed_cov - cov_joined).max(0.0) >= -1e-9 {
                        continue;
                    }
                    let mut delta = len_delta;
                    if uses_raster {
                        let est = cov_joined
                            + into.as_ref().map_or(0.0, |j| self.probe(j))
                            + out_of.as_ref().map_or(0.0, |j| self.probe(j))
                            - removed_cov;
                        delta += self.scale * est;
                    }
                    if delta < -1e-9 && best.as_ref().is_none_or(|b| delta < b.0) {
                        let added: Vec<Jump> =
                            [joined, into, out_of].into_iter().flatten().collect();
                        best = Some((delta, (rev, seam), added));
                    }
                }
                let Some((_, (rev, seam), added)) = best else {
                    continue;
                };
                let mv = Move::Relocate { i, a, rev, seam };
                if !self.precedence.is_empty() && !self.respects(&mv) {
                    continue;
                }
                let removed = removed_i.iter().copied().chain(removed_a).collect();
                out.push(Candidate { mv, removed, added });
            }
        }
        out
    }

    /// 2-opt moves reversing a run that starts at position `first` (`first == 0` reverses
    /// a prefix, which is how the first path can move when no start point is assumed) and
    /// ends at a spatial neighbour of its first node, in scan order.
    fn reversals(&self, first: usize) -> Vec<Candidate> {
        let (seq, nodes) = (self.seq, self.obj.nodes);
        let uses_raster = self.obj.uses_raster();
        let n_next = seq.order[first];
        let a_end = self.end_at(first as i64 - 1);
        let run_first_s = seq.start(nodes, first);
        let in_jump: Option<Jump> = a_end.map(|p| (p, run_first_s));
        let in_len = in_jump.as_ref().map_or(0.0, jump_len);
        let in_cov = in_jump.as_ref().map_or(0.0, |j| self.probe(j));
        let mut out = Vec::new();
        for &nb in &self.neighbours[n_next] {
            let j = self.pos[nb];
            if j <= first {
                continue;
            }
            let b_start = self.start_at(j + 1);
            let run_last_e = seq.end(nodes, j);
            let out_jump: Option<Jump> = b_start.map(|p| (run_last_e, p));
            // Reversed run: enter at the old end of node j, leave from the old start of
            // the first node.
            let new_in: Option<Jump> = a_end.map(|p| (p, run_last_e));
            let new_out: Option<Jump> = b_start.map(|p| (run_first_s, p));
            let removed_len = in_len + out_jump.as_ref().map_or(0.0, jump_len);
            let len_delta = new_in.as_ref().map_or(0.0, jump_len)
                + new_out.as_ref().map_or(0.0, jump_len)
                - removed_len;
            let removed_cov = in_cov + out_jump.as_ref().map_or(0.0, |jp| self.probe(jp));
            if len_delta - self.scale * removed_cov >= -1e-9 {
                continue;
            }
            let mut delta = len_delta;
            if uses_raster {
                let est = new_in.as_ref().map_or(0.0, |jp| self.probe(jp))
                    + new_out.as_ref().map_or(0.0, |jp| self.probe(jp))
                    - removed_cov;
                delta += self.scale * est;
            }
            if delta >= -1e-9 {
                continue;
            }
            let mv = Move::Reverse { first, last: j };
            if !self.precedence.is_empty() && !self.respects(&mv) {
                continue;
            }
            out.push(Candidate {
                mv,
                removed: [in_jump, out_jump].into_iter().flatten().collect(),
                added: [new_in, new_out].into_iter().flatten().collect(),
            });
        }
        out
    }
}

/// Apply `c` if it pays off. With a bleed penalty the candidate's estimate treated its
/// jumps' footprints as independent: the exact change is taken on the raster, and the
/// raster restored if the move does not pay.
fn try_apply(seq: &mut Seq, obj: &mut Objective<'_>, c: &Candidate, scale: f64) -> bool {
    if obj.uses_raster() {
        let r = obj.raster.as_mut().expect("uses_raster");
        let mut exact = 0.0;
        for (x, y) in &c.removed {
            exact += r.remove(*x, *y);
        }
        for (x, y) in &c.added {
            exact += r.add(*x, *y);
        }
        let len_delta: f64 =
            c.added.iter().map(jump_len).sum::<f64>() - c.removed.iter().map(jump_len).sum::<f64>();
        if len_delta + scale * exact >= -1e-9 {
            for (x, y) in &c.added {
                r.remove(*x, *y);
            }
            for (x, y) in &c.removed {
                r.add(*x, *y);
            }
            return false;
        }
    }
    c.mv.apply(seq, obj.nodes);
    true
}

/// Local search with or-opt (relocate one path, re-choosing its orientation or seam) and
/// 2-opt (reverse a run) moves, candidates limited to spatial neighbours, evaluated by the
/// delta of the affected jumps only. Precedence is checked before a move is applied.
///
/// Each sweep applies the first move, in scan order, that pays off. Positions are scanned
/// in parallel blocks against the unchanged sequence and the candidates offered in scan
/// order, so the result is that of a sequential scan whatever the number of threads.
fn improve(seq: &mut Seq, obj: &mut Objective<'_>, precedence: &[(usize, usize)], max_sweeps: u32) {
    let m = seq.order.len();
    if m < 3 {
        return;
    }
    let nodes = obj.nodes;
    let centres: Vec<Point> = nodes.iter().map(|n| n.verts[0]).collect();
    let bounds = BBox::of_points(centres.iter().copied()).unwrap();
    let mut grid = EntryGrid::new(bounds, nodes.len());
    for (i, c) in centres.iter().enumerate() {
        grid.insert(i, *c);
    }
    let neighbours: Vec<Vec<usize>> = crate::par::map((0..nodes.len()).collect(), |i| {
        grid.k_nearest(centres[i], NEIGHBOURS, Some(i))
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    });
    // Make the raster reflect the current sequence.
    let _ = obj.total(seq);
    let scale = obj.bleed_scale() * obj.bleed_weight;
    // A sequential scan stops at the first move; a parallel one also evaluates the rest of
    // its block, which is the price of the parallelism.
    let threads = crate::par::threads();
    let block = if threads > 1 { 64 * threads } else { 1 };

    let mut pos: Vec<usize> = vec![0; nodes.len()];
    for _ in 0..max_sweeps {
        if crate::cancel::cancelled() {
            return;
        }
        for (k, &n) in seq.order.iter().enumerate() {
            pos[n] = k;
        }
        // Or-opt over every position, then (if none paid) 2-opt over every run start.
        let mut improved = false;
        for reverse in [false, true] {
            let positions = if reverse { m - 1 } else { m };
            let mut from = 0;
            while from < positions && !improved {
                let to = (from + block).min(positions);
                let found = {
                    let scan = Scan {
                        seq,
                        obj,
                        pos: &pos,
                        neighbours: &neighbours,
                        precedence,
                        scale,
                    };
                    crate::par::map((from..to).collect(), |k| {
                        if reverse {
                            scan.reversals(k)
                        } else {
                            scan.relocations(k)
                        }
                    })
                };
                improved = found
                    .iter()
                    .flatten()
                    .any(|c| try_apply(seq, obj, c, scale));
                from = to;
            }
            if improved {
                break;
            }
        }
        if !improved {
            break;
        }
    }
}

fn stats_of(seq: &Seq, obj: &mut Objective<'_>, crossings_cap: usize) -> Stats {
    let nodes = obj.nodes;
    let jumps = seq.jumps(nodes, obj.start, obj.end);
    let objective = obj.total(seq);
    let lengths: Vec<f64> = jumps.iter().map(|(a, b)| dist(*a, *b)).collect();
    let crossings = if jumps.len() <= crossings_cap {
        crate::par::map((0..jumps.len()).collect(), |i| {
            (i + 2..jumps.len())
                .filter(|&j| segments_cross(jumps[i].0, jumps[i].1, jumps[j].0, jumps[j].1))
                .count()
        })
        .into_iter()
        .sum()
    } else {
        0
    };
    let (covered, overlap, peak) = match &obj.raster {
        Some(r) if r.has_mask() => {
            let (c, o, p) = r.protected_stats();
            (Some(c), Some(o), Some(p))
        }
        _ => (None, None, None),
    };
    Stats {
        paths: seq.order.len(),
        jumps: jumps.len(),
        jump_length_mm: lengths.iter().sum(),
        longest_jump_mm: lengths.iter().cloned().fold(0.0, f64::max),
        crossings,
        covered_mm2: covered,
        overlap_integral: overlap,
        peak_coverage: peak,
        objective,
    }
}

/// Plan one output group. `protected` is the union of the protected working layers'
/// regions, if any.
pub fn plan(
    passes: &[PassPaths<'_>],
    mode: OptimizerMode,
    settings: &OptimizerSettings,
    protected: Option<&Region>,
) -> GroupPlan {
    let (nodes, precedence) = nodes_of(passes);
    let start = settings.start_mm.map(|p| Point::new(p[0], p[1]));
    let end = settings.end_mm.map(|p| Point::new(p[0], p[1]));
    let empty = Stats::default();
    if nodes.is_empty() {
        return GroupPlan {
            mode,
            order: Vec::new(),
            before: empty.clone(),
            after: empty,
            heatmap: None,
        };
    }
    let bounds = BBox::of_points(
        nodes
            .iter()
            .flat_map(|n| n.verts.iter().copied())
            .chain(start)
            .chain(end),
    )
    .unwrap();
    let want_raster = protected.is_some() || mode == OptimizerMode::BleedAware;
    let raster = want_raster
        .then(|| Raster::new(bounds, settings.cell_mm, settings.footprint_mm, protected));
    let mut obj = Objective {
        nodes: &nodes,
        start,
        end,
        bleed_weight: if mode == OptimizerMode::BleedAware {
            settings.bleed_weight
        } else {
            0.0
        },
        raster,
    };
    let crossings_cap = 20_000;

    // As generated: identity order, generated orientation, seam at vertex 0.
    let generated = Seq {
        order: (0..nodes.len()).collect(),
        reversed: vec![false; nodes.len()],
        seam: vec![0; nodes.len()],
    };
    let before = stats_of(&generated, &mut obj, crossings_cap);

    let seq = match mode {
        OptimizerMode::AsGenerated => generated,
        OptimizerMode::MinimumJump | OptimizerMode::BleedAware => {
            let mut s = construct(&nodes, &precedence, start);
            polish(&mut s, &obj);
            improve(&mut s, &mut obj, &precedence, settings.max_sweeps);
            polish(&mut s, &obj);
            s
        }
    };
    let after = stats_of(&seq, &mut obj, crossings_cap);
    let heatmap = obj.raster.take().filter(|r| r.has_mask()).map(|r| Heatmap {
        origin: r.origin,
        cell_mm: r.cell,
        width: r.width,
        height: r.height,
        counts: r.counts,
        mask: r.mask,
    });
    let order = seq
        .order
        .iter()
        .map(|&n| PathRef {
            pass: passes[nodes[n].pass].id.clone(),
            index: nodes[n].index,
            reversed: !nodes[n].closed && seq.reversed[n],
            seam: if nodes[n].closed { seq.seam[n] } else { 0 },
        })
        .collect();
    GroupPlan {
        mode,
        order,
        before,
        after,
        heatmap,
    }
}

/// Apply a `PathRef`'s orientation and seam to the geometry: exact, no new vertices.
pub fn oriented(path: &Pline, r: &PathRef) -> Pline {
    if path.is_closed() {
        let n = path.vertex_count();
        if n == 0 || r.seam.is_multiple_of(n) {
            return path.clone();
        }
        let s = r.seam % n;
        let mut out = Pline::new_closed();
        for k in 0..n {
            let v = path.at((s + k) % n);
            out.add(v.x, v.y, v.bulge);
        }
        out
    } else if r.reversed {
        let mut p = path.clone();
        p.invert_direction_mut();
        p
    } else {
        path.clone()
    }
}

/// Jumps of a plan as point pairs (for previews), including assumed start/end points.
pub fn jumps(
    plan: &GroupPlan,
    paths: &BTreeMap<PassId, Vec<Pline>>,
    settings: &OptimizerSettings,
) -> Vec<(Point, Point)> {
    let mut ends: Vec<(Point, Point)> = Vec::new();
    for r in &plan.order {
        let Some(p) = paths.get(&r.pass).and_then(|v| v.get(r.index)) else {
            continue;
        };
        let o = oriented(p, r);
        let first = o.at(0);
        let last = if o.is_closed() {
            first
        } else {
            o.last().unwrap_or(first)
        };
        ends.push((Point::new(first.x, first.y), Point::new(last.x, last.y)));
    }
    let mut out = Vec::new();
    if let (Some(s), Some(first)) = (settings.start_mm, ends.first()) {
        out.push((Point::new(s[0], s[1]), first.0));
    }
    for w in ends.windows(2) {
        out.push((w[0].1, w[1].0));
    }
    if let (Some(e), Some(last)) = (settings.end_mm, ends.last()) {
        out.push((last.1, Point::new(e[0], e[1])));
    }
    out
}

/// Fingerprint of the geometry a plan depends on.
pub fn fingerprint(
    passes: &[PassPaths<'_>],
    mode: OptimizerMode,
    settings: &OptimizerSettings,
    protected_fp: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    for pp in passes {
        pp.id.hash(&mut h);
        pp.paths.len().hash(&mut h);
        for p in pp.paths {
            p.vertex_count().hash(&mut h);
            for v in p.iter_vertexes() {
                v.x.to_bits().hash(&mut h);
                v.y.to_bits().hash(&mut h);
                v.bulge.to_bits().hash(&mut h);
            }
            p.is_closed().hash(&mut h);
        }
        pp.precedence.hash(&mut h);
    }
    serde_json::to_string(&(mode, settings))
        .unwrap_or_default()
        .hash(&mut h);
    protected_fp.hash(&mut h);
    h.finish()
}

/// Flattened helper for tests and callers that want the planned geometry in order.
pub fn planned_paths(plan: &GroupPlan, paths: &BTreeMap<PassId, Vec<Pline>>) -> Vec<Pline> {
    plan.order
        .iter()
        .filter_map(|r| {
            paths
                .get(&r.pass)
                .and_then(|v| v.get(r.index))
                .map(|p| oriented(p, r))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pline::{circle, from_points, rect};
    use crate::region::LoopRule;

    fn settings(mode: OptimizerMode) -> OptimizerSettings {
        OptimizerSettings {
            mode,
            footprint_mm: 0.2,
            cell_mm: 0.05,
            ..OptimizerSettings::default()
        }
    }

    fn open(a: (f64, f64), b: (f64, f64)) -> Pline {
        from_points([Point::new(a.0, a.1), Point::new(b.0, b.1)], false)
    }

    #[test]
    fn minimum_jump_visits_a_row_of_segments_in_order_and_reverses_where_cheaper() {
        // Hatch-like rows generated in a scrambled order, all left to right.
        let rows = [3.0, 0.0, 4.0, 1.0, 2.0];
        let paths: Vec<Pline> = rows.iter().map(|&y| open((0.0, y), (10.0, y))).collect();
        let passes = [PassPaths {
            id: PassId::new("p"),
            paths: &paths,
            precedence: &[],
        }];
        let s = settings(OptimizerMode::MinimumJump);
        let plan = plan(&passes, OptimizerMode::MinimumJump, &s, None);
        assert_eq!(plan.order.len(), 5);
        // Generated order: every jump goes back to x = 0 and across rows.
        assert!(
            plan.before.jump_length_mm > 40.0,
            "{}",
            plan.before.jump_length_mm
        );
        // Planned: a serpentine visiting rows 3,4 or 3,2,... with 1 mm jumps only.
        assert!(
            (plan.after.jump_length_mm - 4.0).abs() < 1e-9,
            "{}",
            plan.after.jump_length_mm
        );
        let reversed: usize = plan.order.iter().filter(|r| r.reversed).count();
        assert_eq!(reversed, 2, "alternate rows run right to left");
        assert_eq!(plan.after.crossings, 0);
        assert!(plan.after.objective <= plan.before.objective);
    }

    #[test]
    fn seams_rotate_closed_loops_without_changing_geometry() {
        let a = rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0));
        let b = rect(Point::new(5.0, 0.0), Point::new(6.0, 1.0));
        let paths = vec![a.clone(), b.clone()];
        let passes = [PassPaths {
            id: PassId::new("p"),
            paths: &paths,
            precedence: &[],
        }];
        let s = settings(OptimizerMode::MinimumJump);
        let plan = plan(&passes, OptimizerMode::MinimumJump, &s, None);
        // Jump from a's vertex nearest to b (1,0) or (1,1) to b's nearest (5,·): 4 mm.
        assert!(
            (plan.after.jump_length_mm - 4.0).abs() < 1e-9,
            "{}",
            plan.after.jump_length_mm
        );
        let mut map = BTreeMap::new();
        map.insert(PassId::new("p"), paths.clone());
        for (orig, planned) in paths.iter().zip(planned_paths(&plan, &map)) {
            assert_eq!(planned.vertex_count(), orig.vertex_count());
            assert!((planned.area() - orig.area()).abs() < 1e-12);
            assert!(planned.is_closed());
        }
    }

    #[test]
    fn precedence_keeps_holes_before_their_outer() {
        // Outer square far right, its hole in the middle, a lone square left: the greedy
        // start is the lone square; the hole must still come before the outer.
        let lone = rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0));
        let outer = rect(Point::new(10.0, 0.0), Point::new(20.0, 10.0));
        let hole = rect(Point::new(14.0, 4.0), Point::new(16.0, 6.0));
        let paths = vec![outer.clone(), hole.clone(), lone.clone()];
        let precedence = [(1usize, 0usize)];
        let passes = [PassPaths {
            id: PassId::new("cut"),
            paths: &paths,
            precedence: &precedence,
        }];
        let mut s = settings(OptimizerMode::MinimumJump);
        s.start_mm = Some([0.0, 0.0]);
        let plan = plan(&passes, OptimizerMode::MinimumJump, &s, None);
        let pos = |i: usize| plan.order.iter().position(|r| r.index == i).unwrap();
        assert!(pos(1) < pos(0), "hole before outer: {:?}", plan.order);
        // From (0,0) the lone square is nearest; then the outer's corner (10,0) would be
        // nearer than the hole, but the constraint forbids it.
        assert_eq!(plan.order[0].index, 2);
        assert_eq!(plan.order[1].index, 1);
    }

    #[test]
    fn bleed_aware_routes_jumps_around_a_protected_region() {
        // Two vertical segments; the shortest connection (along y = 0) crosses a protected
        // square, the slightly longer one (along the top) does not. Minimum jump takes the
        // short one; bleed-aware pays 0.2 mm of travel to avoid the square.
        let left = open((0.0, 0.0), (0.0, 10.0));
        let right = open((10.0, 0.0), (10.0, 12.0));
        let paths = vec![left, right];
        let passes = [PassPaths {
            id: PassId::new("p"),
            paths: &paths,
            precedence: &[],
        }];
        let protected = Region::from_loops(
            [rect(Point::new(4.0, -1.0), Point::new(6.0, 1.0))],
            LoopRule::NonZero,
        );
        let s_min = settings(OptimizerMode::MinimumJump);
        let min = plan(
            &passes,
            OptimizerMode::MinimumJump,
            &s_min,
            Some(&protected),
        );
        assert!(
            (min.after.jump_length_mm - 10.0).abs() < 1e-9,
            "{}",
            min.after.jump_length_mm
        );
        assert!(
            min.after.covered_mm2.unwrap() > 0.3,
            "{:?}",
            min.after.covered_mm2
        );

        let mut s_bleed = settings(OptimizerMode::BleedAware);
        s_bleed.bleed_weight = 50.0;
        let bleed = plan(
            &passes,
            OptimizerMode::BleedAware,
            &s_bleed,
            Some(&protected),
        );
        assert!(
            (bleed.after.jump_length_mm - (100.0f64 + 4.0).sqrt()).abs() < 1e-9,
            "{}",
            bleed.after.jump_length_mm
        );
        assert_eq!(bleed.after.covered_mm2, Some(0.0), "{:?}", bleed.after);
        assert!(
            bleed.after.objective
                < min.after.objective + 50.0 * min.after.overlap_integral.unwrap() / 0.2
        );
        assert!(bleed.heatmap.is_some());
        let hm = bleed.heatmap.unwrap();
        assert!(hm.mask.iter().filter(|m| **m != 0).count() > 0);
    }

    #[test]
    fn as_generated_keeps_everything_but_still_reports() {
        let paths = vec![
            circle(Point::new(0.0, 0.0), 1.0),
            open((5.0, 0.0), (6.0, 0.0)),
        ];
        let passes = [PassPaths {
            id: PassId::new("p"),
            paths: &paths,
            precedence: &[],
        }];
        let s = settings(OptimizerMode::AsGenerated);
        let plan = plan(&passes, OptimizerMode::AsGenerated, &s, None);
        assert_eq!(
            plan.order.iter().map(|r| r.index).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(plan.order.iter().all(|r| !r.reversed && r.seam == 0));
        assert_eq!(plan.before, plan.after);
        assert_eq!(plan.after.jumps, 1);
        assert!((plan.after.jump_length_mm - 6.0).abs() < 1e-9); // circle starts at (-1, 0)
    }

    #[test]
    fn assumed_start_and_end_points_add_jumps_only_when_given() {
        let paths = vec![open((0.0, 0.0), (1.0, 0.0))];
        let passes = [PassPaths {
            id: PassId::new("p"),
            paths: &paths,
            precedence: &[],
        }];
        let mut s = settings(OptimizerMode::MinimumJump);
        let none = plan(&passes, OptimizerMode::MinimumJump, &s, None);
        assert_eq!(none.after.jumps, 0);
        s.start_mm = Some([0.0, 1.0]);
        s.end_mm = Some([0.0, 1.0]);
        let both = plan(&passes, OptimizerMode::MinimumJump, &s, None);
        assert_eq!(both.after.jumps, 2);
        // Start at (0,1): nearest end is (0,0); run to (1,0); back to (0,1): 1 + sqrt 2.
        assert!((both.after.jump_length_mm - (1.0 + 2f64.sqrt())).abs() < 1e-9);
    }

    #[test]
    fn raster_mask_and_capsules_are_sane() {
        let region = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(2.0, 1.0))],
            LoopRule::NonZero,
        );
        let bounds = BBox {
            min: Point::new(-1.0, -1.0),
            max: Point::new(3.0, 2.0),
        };
        let mut r = Raster::new(bounds, 0.1, 0.2, Some(&region));
        let masked = r.mask.iter().filter(|m| **m != 0).count() as f64 * r.cell_area();
        assert!((masked - 2.0).abs() < 0.1, "{masked}");
        r.add(Point::new(-1.0, 0.5), Point::new(3.0, 0.5));
        let (covered, sq, peak) = r.protected_stats();
        // A 0.2 mm wide band across the 2 mm rectangle: about 0.4 mm².
        assert!((covered - 0.4).abs() < 0.1, "{covered}");
        assert_eq!(peak, 1);
        assert!((sq - covered).abs() < 1e-9);
        r.add(Point::new(-1.0, 0.5), Point::new(3.0, 0.5));
        let (_, sq2, peak2) = r.protected_stats();
        assert_eq!(peak2, 2);
        assert!(
            (sq2 - 4.0 * sq).abs() < 1e-9,
            "squared coverage quadruples when two jumps stack"
        );
    }

    proptest::proptest! {
        /// The row scan visits exactly the cells whose centre lies within the footprint,
        /// once each, for jumps of any length and direction, also partly off the raster;
        /// stamping in bands gives the counts and squared sum that adding jump by jump does.
        #[test]
        fn capsule_cells_match_a_brute_force_scan(
            jumps in proptest::collection::vec((-1.0f64..6.0, -1.0f64..5.0, -1.0f64..6.0, -1.0f64..5.0), 1..12),
            cell in 0.02f64..0.3,
            footprint in 0.0f64..0.8,
        ) {
            let bounds = BBox { min: Point::new(0.0, 0.0), max: Point::new(5.0, 4.0) };
            let protected = Region::from_loops(
                [rect(Point::new(1.0, 1.0), Point::new(4.0, 3.0))],
                LoopRule::NonZero,
            );
            let mut r = Raster::new(bounds, cell, footprint, Some(&protected));
            let jumps: Vec<Jump> = jumps
                .into_iter()
                .map(|(ax, ay, bx, by)| (Point::new(ax, ay), Point::new(bx, by)))
                .collect();
            let mut sq = 0.0;
            for &(a, b) in &jumps {
                let mut seen = Vec::new();
                r.cells(a, b, |i| seen.push(i));
                let mut expect = Vec::new();
                for iy in 0..r.height {
                    for ix in 0..r.width {
                        if seg_dist(r.centre(ix, iy), a, b) <= r.footprint_r {
                            expect.push(iy * r.width + ix);
                        }
                    }
                }
                seen.sort_unstable();
                proptest::prop_assert_eq!(seen, expect);
                sq += r.add(a, b);
            }
            let added = r.counts.clone();
            let stamped = r.stamp_all(&jumps);
            proptest::prop_assert_eq!(&r.counts, &added);
            proptest::prop_assert_eq!(stamped, sq);
        }

        /// `Move::moved` predicts where `Move::apply` puts every node.
        #[test]
        fn moves_predict_their_positions(m in 3usize..30, x in 0usize..1000, y in 0usize..1000, reverse: bool) {
            let nodes: Vec<Node> = (0..m)
                .map(|k| Node { pass: 0, index: k, closed: k % 2 == 0, verts: vec![Point::new(k as f64, 0.0), Point::new(k as f64, 1.0)] })
                .collect();
            let mut seq = Seq { order: (0..m).rev().collect(), reversed: vec![false; m], seam: vec![0; m] };
            let mv = if reverse {
                let first = x % (m - 1);
                Move::Reverse { first, last: first + 1 + y % (m - 1 - first) }
            } else {
                let i = x % m;
                let a = (y % (m + 1)) as i64 - 1;
                proptest::prop_assume!(a != i as i64 && a != i as i64 - 1);
                Move::Relocate { i, a, rev: true, seam: 1 }
            };
            let before = seq.order.clone();
            mv.apply(&mut seq, &nodes);
            for (k, n) in before.iter().enumerate() {
                proptest::prop_assert_eq!(seq.order[mv.moved(k, m)], *n);
            }
        }
    }
}
