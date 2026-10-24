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
    /// Scratch: last stamp id per cell, to avoid counting a cell twice for one jump.
    stamp: Vec<u32>,
    stamp_id: u32,
    footprint_r: f64,
    /// Cell offsets that can lie within the footprint of a point: the stencil walked at
    /// every step along a jump (exact distance test afterwards).
    disk: Vec<(i64, i64)>,
}

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
        // Step points sit anywhere in a cell and cell centres up to r away must be seen:
        // radius r plus one cell diagonal covers both offsets.
        let reach = (r + cell * std::f64::consts::SQRT_2) / cell;
        let rc = reach.ceil() as i64;
        let mut disk = Vec::new();
        for dy in -rc..=rc {
            for dx in -rc..=rc {
                if ((dx * dx + dy * dy) as f64).sqrt() <= reach + 0.01 {
                    disk.push((dx, dy));
                }
            }
        }
        let mut raster = Raster {
            origin: b.min,
            cell,
            width,
            height,
            counts: vec![0; width * height],
            mask: vec![0; width * height],
            stamp: vec![0; width * height],
            stamp_id: 0,
            footprint_r: r,
            disk,
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

    /// Visit every cell of the capsule around `a -> b`, once, with the current count. The
    /// callback returns the new count for the cell.
    fn stamp_capsule(&mut self, a: Point, b: Point, mut f: impl FnMut(u16, bool) -> u16) {
        self.stamp_id = self.stamp_id.wrapping_add(1);
        if self.stamp_id == 0 {
            self.stamp.fill(0);
            self.stamp_id = 1;
        }
        let r = self.footprint_r;
        let len = dist(a, b);
        let steps = (len / self.cell).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let c = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
            let cx = ((c.x - self.origin.x) / self.cell).floor() as i64;
            let cy = ((c.y - self.origin.y) / self.cell).floor() as i64;
            for k in 0..self.disk.len() {
                let (dx, dy) = self.disk[k];
                let (ix, iy) = (cx + dx, cy + dy);
                if ix < 0 || iy < 0 || ix >= self.width as i64 || iy >= self.height as i64 {
                    continue;
                }
                let idx = iy as usize * self.width + ix as usize;
                if self.stamp[idx] == self.stamp_id {
                    continue;
                }
                let p = self.centre(ix as usize, iy as usize);
                if seg_dist(p, a, b) > r {
                    continue;
                }
                self.stamp[idx] = self.stamp_id;
                let m = self.mask[idx] != 0;
                self.counts[idx] = f(self.counts[idx], m);
            }
        }
    }

    /// Protected cells a jump's footprint would cover, without changing anything.
    fn probe(&mut self, a: Point, b: Point) -> f64 {
        let mut n = 0.0;
        self.stamp_capsule(a, b, |c, m| {
            if m {
                n += 1.0;
            }
            c
        });
        n
    }

    /// Add a jump; returns the increase of Σ coverage² over protected cells (in cells).
    fn add(&mut self, a: Point, b: Point) -> f64 {
        let mut delta = 0.0;
        self.stamp_capsule(a, b, |c, m| {
            if m {
                delta += 2.0 * c as f64 + 1.0;
            }
            c.saturating_add(1)
        });
        delta
    }

    /// Remove a jump previously added; returns the (negative) change of Σ coverage².
    fn remove(&mut self, a: Point, b: Point) -> f64 {
        let mut delta = 0.0;
        self.stamp_capsule(a, b, |c, m| {
            if m && c > 0 {
                delta += -2.0 * c as f64 + 1.0;
            }
            c.saturating_sub(1)
        });
        delta
    }

    fn clear(&mut self) {
        self.counts.fill(0);
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

    fn respects(&self, precedence: &[(usize, usize)]) -> bool {
        if precedence.is_empty() {
            return true;
        }
        let mut pos = vec![0usize; self.reversed.len()];
        for (k, &n) in self.order.iter().enumerate() {
            pos[n] = k;
        }
        precedence.iter().all(|&(a, b)| pos[a] < pos[b])
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
        let pick = if first {
            first = false;
            // The first generated path that is available.
            (0..n).find(|&i| remaining_preds[i] == 0)
        } else {
            grid.nearest(current).map(|(i, _, _)| i)
        };
        let Some(i) = pick else {
            // Cyclic constraints: append the rest in generated order.
            for j in 0..n {
                if !seq.order.contains(&j) {
                    seq.order.push(j);
                }
            }
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
fn jump_cost(obj: &mut Objective<'_>, a: Point, b: Point) -> f64 {
    let len = dist(a, b);
    if obj.bleed_weight > 0.0
        && let Some(r) = &mut obj.raster
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
fn polish(seq: &mut Seq, obj: &mut Objective<'_>) {
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
    for k in 1..m {
        for (ps, &(prev_rev, prev_seam)) in states[k - 1].clone().iter().enumerate() {
            if !cost[k - 1][ps].is_finite() {
                continue;
            }
            let from = node(k - 1).end(prev_rev, prev_seam);
            for (si, &(rev, seam)) in states[k].clone().iter().enumerate() {
                let c = cost[k - 1][ps] + jump_cost(obj, from, node(k).start(rev, seam));
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
        if let Some(r) = &mut self.raster {
            r.clear();
            let mut sq = 0.0;
            for (a, b) in seq.jumps(self.nodes, self.start, self.end) {
                sq += r.add(a, b);
            }
            len + self.bleed_weight * sq * self.bleed_scale()
        } else {
            len
        }
    }

    fn uses_raster(&self) -> bool {
        self.bleed_weight > 0.0 && self.raster.is_some()
    }
}

type Jump = (Point, Point);

/// Local search with or-opt (relocate one path, re-choosing its orientation or seam) and
/// 2-opt (reverse a run) moves, candidates limited to spatial neighbours, evaluated by the
/// delta of the affected jumps only. Precedence is checked before a move is applied.
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
    let neighbours: Vec<Vec<usize>> = (0..nodes.len())
        .map(|i| {
            grid.k_nearest(centres[i], NEIGHBOURS, Some(i))
                .into_iter()
                .map(|(n, _)| n)
                .collect()
        })
        .collect();
    let has_precedence = !precedence.is_empty();
    // Make the raster reflect the current sequence.
    let _ = obj.total(seq);
    let scale = obj.bleed_scale() * obj.bleed_weight;

    let mut pos: Vec<usize> = vec![0; nodes.len()];
    for sweep in 0..max_sweeps {
        let mut improved = false;
        for (k, &n) in seq.order.iter().enumerate() {
            pos[n] = k;
        }
        // Points before/after a position in the current sequence.
        let (start_pt, end_pt) = (obj.start, obj.end);
        let end_at = |seq: &Seq, k: i64| -> Option<Point> {
            if k < 0 {
                start_pt
            } else {
                Some(seq.end(nodes, k as usize))
            }
        };
        let start_at = |seq: &Seq, k: usize| -> Option<Point> {
            if k >= m {
                end_pt
            } else {
                Some(seq.start(nodes, k))
            }
        };
        // Coverage probe (protected cells of a jump's footprint), or 0 without a raster.
        let probe = |obj: &mut Objective<'_>, j: &Jump| -> f64 {
            match (&mut obj.raster, obj.bleed_weight > 0.0) {
                (Some(r), true) => r.probe(j.0, j.1),
                _ => 0.0,
            }
        };
        let jl = |j: &Jump| dist(j.0, j.1);
        let uses_raster = obj.uses_raster();

        // ---- or-opt: move the node at position i to just after position a.
        'outer: for i in 0..m {
            let n = seq.order[i];
            let nd = &nodes[n];
            let prev_i = end_at(seq, i as i64 - 1);
            let next_i = start_at(seq, i + 1);
            let cur_s = seq.start(nodes, i);
            let cur_e = seq.end(nodes, i);
            // Jumps around i that disappear when it moves, and the one that joins the gap.
            let removed_i: Vec<Jump> = [(prev_i, Some(cur_s)), (Some(cur_e), next_i)]
                .into_iter()
                .filter_map(|(x, y)| Some((x?, y?)))
                .collect();
            let joined: Option<Jump> = prev_i.zip(next_i);
            let len_removed_i: f64 = removed_i.iter().map(jl).sum();
            let cov_removed_i: f64 = removed_i.iter().map(|j| probe(obj, j)).sum();
            let len_joined = joined.as_ref().map_or(0.0, jl);
            let cov_joined = joined.as_ref().map_or(0.0, |j| probe(obj, j));
            for &nb in &neighbours[n] {
                let j = pos[nb] as i64;
                for a in [j, j - 1] {
                    if a == i as i64 || a == i as i64 - 1 || a < -1 || a >= m as i64 {
                        continue;
                    }
                    let a_end = end_at(seq, a);
                    let after_a = if (a + 1) as usize == i {
                        start_at(seq, i + 1)
                    } else {
                        start_at(seq, (a + 1) as usize)
                    };
                    let removed_a: Option<Jump> = a_end.zip(after_a);
                    let removed_len = len_removed_i + removed_a.as_ref().map_or(0.0, jl);
                    let removed_cov =
                        cov_removed_i + removed_a.as_ref().map_or(0.0, |j| probe(obj, j));
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
                            + into.as_ref().map_or(0.0, jl)
                            + out_of.as_ref().map_or(0.0, jl)
                            - removed_len;
                        // The bleed term can gain at most what the removed jumps covered.
                        if len_delta - scale * (removed_cov - cov_joined).max(0.0) >= -1e-9 {
                            continue;
                        }
                        let mut delta = len_delta;
                        if uses_raster {
                            let est = cov_joined
                                + into.as_ref().map_or(0.0, |j| probe(obj, j))
                                + out_of.as_ref().map_or(0.0, |j| probe(obj, j))
                                - removed_cov;
                            delta += scale * est;
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
                    let removed: Vec<Jump> = removed_i.iter().copied().chain(removed_a).collect();
                    // Build the candidate order and check constraints.
                    let mut order = seq.order.clone();
                    let node = order.remove(i);
                    let mut at = (a + 1) as usize;
                    if a >= 0 && a as usize >= i {
                        at -= 1;
                    }
                    order.insert(at.min(order.len()), node);
                    let cand = Seq {
                        order,
                        reversed: seq.reversed.clone(),
                        seam: seq.seam.clone(),
                    };
                    if has_precedence && !cand.respects(precedence) {
                        continue;
                    }
                    // Exact bleed delta on the raster; revert if it does not pay off.
                    if uses_raster {
                        let r = obj.raster.as_mut().unwrap();
                        let mut exact = 0.0;
                        for (x, y) in &removed {
                            exact += r.remove(*x, *y);
                        }
                        for (x, y) in &added {
                            exact += r.add(*x, *y);
                        }
                        let len_delta: f64 =
                            added.iter().map(jl).sum::<f64>() - removed.iter().map(jl).sum::<f64>();
                        if len_delta + scale * exact >= -1e-9 {
                            for (x, y) in &added {
                                r.remove(*x, *y);
                            }
                            for (x, y) in &removed {
                                r.add(*x, *y);
                            }
                            continue;
                        }
                    }
                    *seq = cand;
                    seq.reversed[n] = rev;
                    seq.seam[n] = seam;
                    improved = true;
                    break 'outer;
                }
            }
        }
        if improved {
            continue;
        }
        // ---- 2-opt: reverse the run (i+1 ..= j); i = -1 reverses a prefix, which is how
        // the first path can move when no start point is assumed.
        'outer2: for i in -1i64..(m as i64 - 1) {
            let first = (i + 1) as usize;
            let n_next = seq.order[first];
            let a_end = end_at(seq, i);
            let run_first_s = seq.start(nodes, first);
            let in_jump: Option<Jump> = a_end.map(|p| (p, run_first_s));
            let in_len = in_jump.as_ref().map_or(0.0, jl);
            let in_cov = in_jump.as_ref().map_or(0.0, |j| probe(obj, j));
            for &nb in &neighbours[n_next] {
                let j = pos[nb];
                if j <= first {
                    continue;
                }
                let b_start = start_at(seq, j + 1);
                let run_last_e = seq.end(nodes, j);
                let out_jump: Option<Jump> = b_start.map(|p| (run_last_e, p));
                // Reversed run: enter at the old end of node j, leave from the old start of
                // the first node.
                let new_in: Option<Jump> = a_end.map(|p| (p, run_last_e));
                let new_out: Option<Jump> = b_start.map(|p| (run_first_s, p));
                let removed_len = in_len + out_jump.as_ref().map_or(0.0, jl);
                let len_delta = new_in.as_ref().map_or(0.0, jl) + new_out.as_ref().map_or(0.0, jl)
                    - removed_len;
                let removed_cov = in_cov + out_jump.as_ref().map_or(0.0, |jp| probe(obj, jp));
                if len_delta - scale * removed_cov >= -1e-9 {
                    continue;
                }
                let mut delta = len_delta;
                if uses_raster {
                    let est = new_in.as_ref().map_or(0.0, |jp| probe(obj, jp))
                        + new_out.as_ref().map_or(0.0, |jp| probe(obj, jp))
                        - removed_cov;
                    delta += scale * est;
                }
                if delta >= -1e-9 {
                    continue;
                }
                let mut cand = seq.clone();
                cand.order[first..=j].reverse();
                for &nn in &cand.order[first..=j] {
                    if !nodes[nn].closed {
                        cand.reversed[nn] = !cand.reversed[nn];
                    }
                }
                if has_precedence && !cand.respects(precedence) {
                    continue;
                }
                let removed: Vec<Jump> = [in_jump, out_jump].into_iter().flatten().collect();
                let added: Vec<Jump> = [new_in, new_out].into_iter().flatten().collect();
                if uses_raster {
                    let r = obj.raster.as_mut().unwrap();
                    let mut exact = 0.0;
                    for (x, y) in &removed {
                        exact += r.remove(*x, *y);
                    }
                    for (x, y) in &added {
                        exact += r.add(*x, *y);
                    }
                    let len_delta: f64 =
                        added.iter().map(jl).sum::<f64>() - removed.iter().map(jl).sum::<f64>();
                    if len_delta + scale * exact >= -1e-9 {
                        for (x, y) in &added {
                            r.remove(*x, *y);
                        }
                        for (x, y) in &removed {
                            r.add(*x, *y);
                        }
                        continue;
                    }
                }
                *seq = cand;
                improved = true;
                break 'outer2;
            }
        }
        if !improved {
            break;
        }
        let _ = sweep;
    }
}

fn stats_of(seq: &Seq, obj: &mut Objective<'_>, crossings_cap: usize) -> Stats {
    let nodes = obj.nodes;
    let jumps = seq.jumps(nodes, obj.start, obj.end);
    let objective = obj.total(seq);
    let lengths: Vec<f64> = jumps.iter().map(|(a, b)| dist(*a, *b)).collect();
    let mut crossings = 0usize;
    if jumps.len() <= crossings_cap {
        for i in 0..jumps.len() {
            for j in i + 2..jumps.len() {
                if segments_cross(jumps[i].0, jumps[i].1, jumps[j].0, jumps[j].1) {
                    crossings += 1;
                }
            }
        }
    }
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
            polish(&mut s, &mut obj);
            improve(&mut s, &mut obj, &precedence, settings.max_sweeps);
            polish(&mut s, &mut obj);
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
}
