//! Centreline finish for cores too narrow for another contour ring.
//!
//! After the rings are generated, the area they sweep (each ring stroked with the pitch as
//! the assumed mark width) is subtracted from the region. What remains are slivers less than
//! a pitch wide; their medial axis, from a Voronoi diagram of the sliver boundary
//! (`boostvoronoi`, integer and robust), becomes open centreline paths. Short paths are
//! legitimate output here and are kept unless shorter than `min_length_mm`.

use std::collections::{HashMap, HashSet};

use boostvoronoi::diagram::Diagram;
use boostvoronoi::prelude::*;

use crate::geom::Point;
use crate::pline::{self, Pline};
use crate::region::{self, Region};

#[derive(Clone, Copy, Debug)]
pub struct GapFillOptions {
    /// Assumed swept width of a mark, used only to find uncovered area (geometry surrogate,
    /// not optical dose). Contour fill passes the pitch.
    pub assumed_width_mm: f64,
    /// Chordal tolerance for flattening the sliver boundaries before the Voronoi diagram.
    pub flatten_tolerance_mm: f64,
    /// Centrelines shorter than this are dropped.
    pub min_length_mm: f64,
}

/// Area of `region` not swept by `paths` stroked with `width`.
pub fn uncovered(region: &Region, paths: &[Pline], width: f64) -> Region {
    if paths.is_empty() {
        return region.clone();
    }
    region.difference(&region::stroke_many(paths, width))
}

/// Medial-axis centrelines of every island of `slivers`, plus the number of islands that
/// had to be skipped because their boundary was too degenerate for a Voronoi diagram.
pub fn centrelines(slivers: &Region, opts: &GapFillOptions) -> (Vec<Pline>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for island in slivers.islands() {
        if island.area() <= 0.0 {
            continue;
        }
        match medial_axis(&island, opts) {
            Some(mut paths) => out.append(&mut paths),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

/// Build the diagram, turning a library panic (debug assertions on degenerate input) into
/// `None` on the host. On wasm (panic = abort) the input sanitising above has to suffice.
/// Such a panic is expected, so it is not printed: the panic hook (installed once, as
/// swapping the process-wide hook from parallel threads could leave the quiet one in place)
/// stays silent on a thread while it builds a diagram.
#[cfg(not(target_arch = "wasm32"))]
fn build_diagram(segments: &[[i32; 4]]) -> Option<Diagram> {
    use std::cell::Cell;
    thread_local! {
        static QUIET: Cell<bool> = const { Cell::new(false) };
    }
    static HOOK: std::sync::Once = std::sync::Once::new();
    if !segments_valid(segments) {
        return None;
    }
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !QUIET.with(Cell::get) {
                prev(info);
            }
        }));
    });
    QUIET.with(|q| q.set(true));
    let result = std::panic::catch_unwind(|| {
        Builder::<i32>::default()
            .with_segments(segments.iter())
            .ok()?
            .build()
            .ok()
    });
    QUIET.with(|q| q.set(false));
    result.ok().flatten()
}

#[cfg(target_arch = "wasm32")]
fn build_diagram(segments: &[[i32; 4]]) -> Option<Diagram> {
    if !segments_valid(segments) {
        return None;
    }
    Builder::<i32>::default()
        .with_segments(segments.iter())
        .ok()?
        .build()
        .ok()
}

/// The Voronoi builder requires segments that meet only at shared endpoints. Checked with
/// exact integer arithmetic; slivers are small, so the quadratic scan is cheap.
fn segments_valid(segs: &[[i32; 4]]) -> bool {
    fn orient(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i64 {
        ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).signum()
    }
    fn on_segment(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> bool {
        orient(a, b, p) == 0
            && p[0] >= a[0].min(b[0])
            && p[0] <= a[0].max(b[0])
            && p[1] >= a[1].min(b[1])
            && p[1] <= a[1].max(b[1])
    }
    let pt = |s: &[i32; 4], i: usize| -> [i64; 2] { [s[2 * i] as i64, s[2 * i + 1] as i64] };
    for i in 0..segs.len() {
        let (a, b) = (pt(&segs[i], 0), pt(&segs[i], 1));
        if a == b {
            return false;
        }
        for other in &segs[i + 1..] {
            let (c, d) = (pt(other, 0), pt(other, 1));
            let shared: Vec<[i64; 2]> = [a, b]
                .iter()
                .filter(|p| **p == c || **p == d)
                .copied()
                .collect();
            match shared.len() {
                2 => return false, // duplicate segment
                1 => {
                    // Adjacent: must not overlap beyond the shared endpoint.
                    let s0 = shared[0];
                    let (oa, oc) = (if a == s0 { b } else { a }, if c == s0 { d } else { c });
                    if orient(s0, oa, oc) == 0
                        && (oa[0] - s0[0]) * (oc[0] - s0[0]) + (oa[1] - s0[1]) * (oc[1] - s0[1]) > 0
                    {
                        return false;
                    }
                }
                _ => {
                    let (o1, o2, o3, o4) = (
                        orient(a, b, c),
                        orient(a, b, d),
                        orient(c, d, a),
                        orient(c, d, b),
                    );
                    if o1 != o2 && o3 != o4 {
                        return false; // proper crossing
                    }
                    if on_segment(a, b, c)
                        || on_segment(a, b, d)
                        || on_segment(c, d, a)
                        || on_segment(c, d, b)
                    {
                        return false; // touching or collinear overlap
                    }
                }
            }
        }
    }
    true
}

/// Voronoi-based medial axis of one island. `None` when the island is thinner than the
/// snapping grid or its diagram cannot be built.
fn medial_axis(island: &Region, opts: &GapFillOptions) -> Option<Vec<Pline>> {
    let bounds = island.bounds()?;
    let extent = bounds.width().max(bounds.height()).max(1e-9);
    // Finest integer grid that keeps every coordinate well inside i32 (at most 1 nm): a
    // coarse grid would fold a thin sliver's two sides onto each other.
    let scale = (1.0e6_f64).min(1.0e8 / extent);
    let origin = bounds.min;
    let to_int = |p: Point| {
        [
            ((p.x - origin.x) * scale).round() as i32,
            ((p.y - origin.y) * scale).round() as i32,
        ]
    };
    let to_mm = |x: f64, y: f64| Point::new(x / scale + origin.x, y / scale + origin.y);

    // Islands thinner than a few grid cells on average are below any meaningful resolution.
    let perimeter: f64 = island.loops.iter().map(pline::length).sum();
    if perimeter <= 0.0 || island.area() / (perimeter / 2.0) < 4.0 / scale {
        return None;
    }

    // Boundary segments (flattened, snapped, de-duplicated).
    let mut segments: Vec<[i32; 4]> = Vec::new();
    for lp in &island.loops {
        let pts = pline::flatten(lp, opts.flatten_tolerance_mm).ok()?;
        let mut ints: Vec<[i32; 2]> = Vec::with_capacity(pts.len());
        for p in pts {
            let q = to_int(p);
            if ints.last() != Some(&q) {
                ints.push(q);
            }
        }
        while ints.len() > 1 && ints.first() == ints.last() {
            ints.pop();
        }
        if ints.len() < 3 {
            continue;
        }
        let n = ints.len();
        for i in 0..n {
            segments.push([
                ints[i][0],
                ints[i][1],
                ints[(i + 1) % n][0],
                ints[(i + 1) % n][1],
            ]);
        }
    }
    if segments.len() < 3 {
        return None;
    }
    let diagram = build_diagram(&segments)?;

    // Endpoint of a segment as a point, for adjacency tests.
    let seg_points = |i: usize| {
        (
            [segments[i][0], segments[i][1]],
            [segments[i][2], segments[i][3]],
        )
    };
    let shares_endpoint = |i: usize, j: usize| {
        let (a0, a1) = seg_points(i);
        let (b0, b1) = seg_points(j);
        a0 == b0 || a0 == b1 || a1 == b0 || a1 == b1
    };
    // Site of a cell: its segment index and, for endpoint cells, the endpoint.
    let cell_site = |cell_id| -> Option<(usize, Option<[i32; 2]>)> {
        let cell = diagram.cell(cell_id).ok()?;
        let idx = cell.source_index().usize();
        let pt = match cell.source_category() {
            SourceCategory::SegmentStart => Some(seg_points(idx).0),
            SourceCategory::SegmentEnd => Some(seg_points(idx).1),
            _ => None,
        };
        Some((idx, pt))
    };
    let is_axis_edge = |ca: (usize, Option<[i32; 2]>), cb: (usize, Option<[i32; 2]>)| -> bool {
        match (ca.1, cb.1) {
            // Two endpoint sites: axis only if the points differ and are not joined by a segment.
            (Some(p), Some(q)) => p != q && !shares_endpoint(ca.0, cb.0),
            // Endpoint vs segment: axis unless the point is an endpoint of that segment.
            (Some(p), None) => seg_points(cb.0).0 != p && seg_points(cb.0).1 != p,
            (None, Some(q)) => seg_points(ca.0).0 != q && seg_points(ca.0).1 != q,
            // Two segments: axis unless they are the same or adjacent.
            (None, None) => ca.0 != cb.0 && !shares_endpoint(ca.0, cb.0),
        }
    };

    // Collect axis chords (vertex0 -> vertex1), one per twin pair, inside the island.
    // Diagram vertices are keyed by their exact coordinates (identical for shared vertices).
    let node = |x: f64, y: f64| (x.to_bits(), y.to_bits());
    let mut seen: HashSet<(Node, Node)> = HashSet::new();
    let mut chords: Vec<Chord> = Vec::new();
    for edge in diagram.edges() {
        if !edge.is_primary() {
            continue;
        }
        let (Some(v0), Some(v1)) = (
            edge.vertex0(),
            diagram.edge_get_vertex1(edge.id()).ok().flatten(),
        ) else {
            continue;
        };
        let Ok(twin) = edge.twin() else { continue };
        let (Ok(ca), Ok(cb)) = (edge.cell(), diagram.edge(twin).and_then(|t| t.cell())) else {
            continue;
        };
        let (Some(sa), Some(sb)) = (cell_site(ca), cell_site(cb)) else {
            continue;
        };
        if !is_axis_edge(sa, sb) {
            continue;
        }
        let a = diagram.vertex(v0).ok()?;
        let b = diagram.vertex(v1).ok()?;
        let (na, nb) = (node(a.x(), a.y()), node(b.x(), b.y()));
        let key = if na <= nb { (na, nb) } else { (nb, na) };
        if !seen.insert(key) {
            continue; // the twin edge
        }
        let (pa, pb) = (to_mm(a.x(), a.y()), to_mm(b.x(), b.y()));
        let mid = Point::new((pa.x + pb.x) / 2.0, (pa.y + pb.y) / 2.0);
        if !island.contains(mid) {
            continue;
        }
        // Between a corner and an edge the axis is a parabola: follow it.
        let via = match (edge.is_curved(), sa, sb) {
            (true, (_, Some(q)), (seg, None)) | (true, (seg, None), (_, Some(q))) => {
                let (s0, s1) = seg_points(seg);
                let mm = |p: [i32; 2]| to_mm(p[0] as f64, p[1] as f64);
                parabola_points(mm(q), mm(s0), mm(s1), pa, pb, opts.flatten_tolerance_mm)
            }
            _ => Vec::new(),
        };
        chords.push(Chord {
            from: na,
            to: nb,
            p_from: pa,
            p_to: pb,
            via,
        });
    }
    Some(chain(chords, opts.min_length_mm))
}

/// Points of the medial axis between corner `focus` and the edge on the line through `a`
/// and `b` (a parabola), strictly between its points `from` and `to`, so that straight
/// segments through them stay within `tol` of it.
fn parabola_points(
    focus: Point,
    a: Point,
    b: Point,
    from: Point,
    to: Point,
    tol: f64,
) -> Vec<Point> {
    let len = (b.x - a.x).hypot(b.y - a.y);
    if len <= 0.0 || tol <= 0.0 {
        return Vec::new();
    }
    let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    // Frame: x along the line, y the distance from it (the focus has y != 0).
    let local = |p: Point| {
        let (px, py) = (p.x - a.x, p.y - a.y);
        (px * ux + py * uy, -px * uy + py * ux)
    };
    let (fx, fy) = local(focus);
    if fy.abs() < 1e-12 {
        return Vec::new();
    }
    let y = |x: f64| ((x - fx) * (x - fx) + fy * fy) / (2.0 * fy);
    let back = |x: f64| {
        let yy = y(x);
        Point::new(a.x + x * ux - yy * uy, a.y + x * uy + yy * ux)
    };
    fn split(x0: f64, x1: f64, depth: u32, y: &dyn Fn(f64) -> f64, tol: f64, out: &mut Vec<f64>) {
        let xm = (x0 + x1) / 2.0;
        if depth >= 16 || (y(xm) - (y(x0) + y(x1)) / 2.0).abs() <= tol {
            return;
        }
        split(x0, xm, depth + 1, y, tol, out);
        out.push(xm);
        split(xm, x1, depth + 1, y, tol, out);
    }
    let mut xs = Vec::new();
    split(local(from).0, local(to).0, 0, &y, tol, &mut xs);
    xs.into_iter().map(back).collect()
}

type Node = (u64, u64);

#[derive(Clone)]
struct Chord {
    from: Node,
    to: Node,
    p_from: Point,
    p_to: Point,
    /// Points in between, from `p_from` on (where the axis is curved).
    via: Vec<Point>,
}

/// Join chords sharing vertices into open polylines; junctions (degree != 2) end chains.
fn chain(chords: Vec<Chord>, min_length: f64) -> Vec<Pline> {
    let mut degree: HashMap<Node, usize> = HashMap::new();
    let mut by_node: HashMap<Node, Vec<usize>> = HashMap::new();
    for (i, c) in chords.iter().enumerate() {
        *degree.entry(c.from).or_default() += 1;
        *degree.entry(c.to).or_default() += 1;
        by_node.entry(c.from).or_default().push(i);
        by_node.entry(c.to).or_default().push(i);
    }
    let mut used = vec![false; chords.len()];
    let mut out = Vec::new();
    // Start chains at non-degree-2 nodes first, then pick up pure loops.
    let mut starts: Vec<usize> = (0..chords.len())
        .filter(|&i| degree[&chords[i].from] != 2 || degree[&chords[i].to] != 2)
        .collect();
    starts.extend(0..chords.len());
    for start in starts {
        if used[start] {
            continue;
        }
        let mut c = chords[start].clone();
        // Begin at a non-degree-2 node when there is one.
        if degree[&c.from] == 2 && degree[&c.to] != 2 {
            std::mem::swap(&mut c.from, &mut c.to);
            std::mem::swap(&mut c.p_from, &mut c.p_to);
            c.via.reverse();
        }
        used[start] = true;
        let mut pts = vec![c.p_from];
        pts.extend(&c.via);
        pts.push(c.p_to);
        let origin = c.from;
        let mut cur = c.to;
        while degree[&cur] == 2 && cur != origin {
            let Some(&next) = by_node[&cur].iter().find(|&&i| !used[i]) else {
                break;
            };
            used[next] = true;
            let n = &chords[next];
            if n.from == cur {
                pts.extend(&n.via);
                pts.push(n.p_to);
                cur = n.to;
            } else {
                pts.extend(n.via.iter().rev());
                pts.push(n.p_from);
                cur = n.from;
            }
        }
        // A chain that comes back to where it began is a closed centreline (the core of a
        // ring around a hole), and the readers would close it on import anyway.
        let closed = cur == origin && pts.len() > 3;
        if closed {
            pts.pop();
        }
        let p = pline::from_points(pts, closed);
        if pline::length(&p) >= min_length {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pline::rect;
    use crate::region::LoopRule;
    use cavalier_contours::polyline::PlineSource;

    #[test]
    fn thin_rectangle_gets_one_centreline() {
        let r = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(10.0, 0.25))],
            LoopRule::NonZero,
        );
        let ring = r.offset(-0.05);
        assert_eq!(ring.loops.len(), 1);
        let gaps = uncovered(&r, &ring.loops, 0.1);
        assert!(
            gaps.area() > 0.3 && gaps.area() < 0.6,
            "gap area {}",
            gaps.area()
        );
        let (lines, skipped) = centrelines(
            &gaps,
            &GapFillOptions {
                assumed_width_mm: 0.1,
                flatten_tolerance_mm: 0.001,
                min_length_mm: 0.05,
            },
        );
        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let l = &lines[0];
        assert!(!l.is_closed());
        let len = pline::length(l);
        assert!(len > 9.5 && len < 10.0, "length {len}");
        for v in l.iter_vertexes() {
            assert!((v.y - 0.125).abs() < 2e-3, "centreline y {}", v.y);
        }
    }

    #[test]
    fn parabolic_axis_points_are_equidistant_and_close_enough() {
        // Corner (1, 2) against the x axis: the parabola y = ((x - 1)^2 + 4) / 4.
        let focus = Point::new(1.0, 2.0);
        let (a, b) = (Point::new(-5.0, 0.0), Point::new(5.0, 0.0));
        let y = |x: f64| ((x - 1.0) * (x - 1.0) + 4.0) / 4.0;
        let (from, to) = (Point::new(-2.0, y(-2.0)), Point::new(3.0, y(3.0)));
        let tol = 1e-3;
        let pts = parabola_points(focus, a, b, from, to, tol);
        assert!(pts.len() > 4);
        let mut all = vec![from];
        all.extend(&pts);
        all.push(to);
        for p in &pts {
            assert!(
                (p.distance(focus) - p.y).abs() < 1e-9,
                "{p:?} is not on the axis"
            );
        }
        for w in all.windows(2) {
            assert!(w[0].x < w[1].x, "in order");
            let mx = (w[0].x + w[1].x) / 2.0;
            assert!(((w[0].y + w[1].y) / 2.0 - y(mx)).abs() <= tol + 1e-12);
        }
    }

    #[test]
    fn a_thin_ring_gets_a_closed_centreline() {
        // A square annulus 0.2 mm wide: its medial axis is one loop, emitted closed.
        let ring = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(4.0, 4.0))],
            LoopRule::NonZero,
        )
        .difference(&Region::from_loops(
            [rect(Point::new(0.2, 0.2), Point::new(3.8, 3.8))],
            LoopRule::NonZero,
        ));
        let (lines, skipped) = centrelines(
            &ring,
            &GapFillOptions {
                assumed_width_mm: 0.2,
                flatten_tolerance_mm: 0.001,
                min_length_mm: 0.1,
            },
        );
        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].is_closed());
        // The axis bends around the inner corners, so it is a little shorter than the
        // square through the middle of the band.
        let len = pline::length(&lines[0]);
        assert!(
            len > 0.97 * 4.0 * 3.8 && len <= 4.0 * 3.8 + 1e-9,
            "length {len}"
        );
    }

    #[test]
    fn cross_shape_yields_branching_centrelines() {
        let h = Region::from_loops(
            [rect(Point::new(-5.0, -0.1), Point::new(5.0, 0.1))],
            LoopRule::NonZero,
        );
        let v = Region::from_loops(
            [rect(Point::new(-0.1, -5.0), Point::new(0.1, 5.0))],
            LoopRule::NonZero,
        );
        let cross = h.union(&v);
        let (lines, _) = centrelines(
            &cross,
            &GapFillOptions {
                assumed_width_mm: 0.2,
                flatten_tolerance_mm: 0.001,
                min_length_mm: 0.5,
            },
        );
        assert!(
            lines.len() >= 2 && lines.len() <= 4,
            "{} centrelines",
            lines.len()
        );
        let total: f64 = lines.iter().map(pline::length).sum();
        assert!(total > 18.0 && total < 20.5, "total {total}");
    }
}
