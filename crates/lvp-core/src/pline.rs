//! Line-and-arc polylines: the canonical curve representation of the core.
//!
//! We use `cavalier_contours`' `Polyline<f64>` directly (vertices carry a DXF-style bulge
//! for the arc to the next vertex) and add the helpers the rest of the core needs. The one
//! kernel rule enforced here: every segment's `|bulge| <= 1` (at most a half circle), which
//! `cavalier_contours` requires; [`add_bulge_segment`] splits larger arcs.

use cavalier_contours::core::math::Vector2;
use cavalier_contours::polyline::{
    PlineOrientation, PlineSource, PlineSourceMut, PlineVertex, Polyline, seg_length, seg_midpoint,
    seg_split_at_point, seg_tangent_vector,
};

use crate::arc::Arc;
use crate::flatten::FlattenError;
use crate::geom::{BBox, Point};

pub type Pline = Polyline<f64>;

/// Largest bulge magnitude a single segment may carry (half circle).
pub const MAX_BULGE: f64 = 1.0;

fn point(v: Vector2<f64>) -> Point {
    Point::new(v.x, v.y)
}

/// Append a vertex whose outgoing segment is the arc `from -> to` with `bulge`, splitting
/// arcs larger than a half circle into two. `from` is pushed (with the bulge), `to` is not.
pub fn add_bulge_segment(p: &mut Pline, from: Point, to: Point, bulge: f64) {
    if bulge.abs() <= MAX_BULGE {
        p.add(from.x, from.y, bulge);
        return;
    }
    match Arc::from_bulge(from, to, bulge) {
        Some(arc) => {
            let mid = arc.point_at(0.5);
            let half = (arc.sweep / 8.0).tan();
            p.add(from.x, from.y, half);
            p.add(mid.x, mid.y, half);
        }
        None => p.add(from.x, from.y, 0.0),
    }
}

/// Polyline from `(x, y, bulge)` triples (DXF LWPOLYLINE semantics). The last vertex's
/// bulge is the closing arc for closed polylines and is ignored for open ones.
pub fn from_bulge_vertices<I>(vertices: I, closed: bool) -> Pline
where
    I: IntoIterator<Item = (f64, f64, f64)>,
{
    let vs: Vec<(f64, f64, f64)> = vertices.into_iter().collect();
    let mut p = if closed {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    let n = vs.len();
    for (i, &(x, y, b)) in vs.iter().enumerate() {
        let next = if i + 1 < n {
            Some(vs[i + 1])
        } else if closed && n > 1 {
            Some(vs[0])
        } else {
            None
        };
        match next {
            Some((nx, ny, _)) => add_bulge_segment(&mut p, Point::new(x, y), Point::new(nx, ny), b),
            None => p.add(x, y, 0.0),
        }
    }
    p
}

/// Closed polyline from plain points (straight segments).
pub fn from_points<I: IntoIterator<Item = Point>>(points: I, closed: bool) -> Pline {
    from_bulge_vertices(points.into_iter().map(|p| (p.x, p.y, 0.0)), closed)
}

/// Full circle as two half-circle arcs, counter-clockwise.
pub fn circle(center: Point, radius: f64) -> Pline {
    let mut p = Pline::new_closed();
    p.add(center.x - radius, center.y, 1.0);
    p.add(center.x + radius, center.y, 1.0);
    p
}

/// Axis-aligned rectangle, counter-clockwise.
pub fn rect(min: Point, max: Point) -> Pline {
    from_points(
        [
            Point::new(min.x, min.y),
            Point::new(max.x, min.y),
            Point::new(max.x, max.y),
            Point::new(min.x, max.y),
        ],
        true,
    )
}

pub fn bounds(p: &Pline) -> Option<BBox> {
    p.extents().map(|e| BBox {
        min: Point::new(e.min_x, e.min_y),
        max: Point::new(e.max_x, e.max_y),
    })
}

/// Orientation of a closed polyline; open polylines return `None`.
pub fn is_ccw(p: &Pline) -> Option<bool> {
    match p.orientation() {
        PlineOrientation::CounterClockwise => Some(true),
        PlineOrientation::Clockwise => Some(false),
        PlineOrientation::Open => None,
    }
}

/// Make a closed polyline counter-clockwise (no-op when it already is, or when open).
pub fn ensure_ccw(mut p: Pline) -> Pline {
    if is_ccw(&p) == Some(false) {
        p.invert_direction_mut();
    }
    p
}

/// Flatten to points within `tolerance` (chordal deviation). For closed polylines the
/// first point is not repeated at the end.
pub fn flatten(p: &Pline, tolerance: f64) -> Result<Vec<Point>, FlattenError> {
    let mut out: Vec<Point> = Vec::with_capacity(p.vertex_count());
    for (v1, v2) in p.iter_segments() {
        let a = point(v1.pos());
        let b = point(v2.pos());
        match Arc::from_bulge(a, b, v1.bulge) {
            Some(arc) => {
                let pts = arc.flatten(tolerance)?;
                // Skip the arc's last point: it is the next vertex, pushed by the next segment
                // (or already present as the first point for a closed loop).
                out.extend(pts.iter().take(pts.len() - 1).copied());
            }
            None => out.push(a),
        }
    }
    if !p.is_closed()
        && let Some(last) = p.last()
    {
        out.push(point(last.pos()));
    }
    Ok(out)
}

/// Midpoint of a segment and the unit normal pointing to the left of the direction of travel.
pub fn seg_midpoint_and_left_normal(v1: PlineVertex<f64>, v2: PlineVertex<f64>) -> (Point, Point) {
    let mid = seg_midpoint(v1, v2);
    let t = seg_tangent_vector(v1, v2, mid);
    let len = t.x.hypot(t.y);
    let n = if len > 0.0 {
        Point::new(-t.y / len, t.x / len)
    } else {
        Point::new(0.0, 0.0)
    };
    (point(mid), n)
}

/// Rotate about the origin by `angle` (radians, counter-clockwise). Arcs are preserved.
pub fn rotated(p: &Pline, angle: f64) -> Pline {
    let (s, c) = angle.sin_cos();
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for v in p.iter_vertexes() {
        out.add(c * v.x - s * v.y, s * v.x + c * v.y, v.bulge);
    }
    out
}

/// Total path length.
pub fn length(p: &Pline) -> f64 {
    p.path_length()
}

/// Positions closer than this are the same vertex. Below any manufacturing relevance, above
/// cavalier's own fuzzy epsilon (so its debug assertions on coincident vertices hold).
pub const REPEAT_EPS: f64 = 1e-6;

/// Remove coincident consecutive vertices (including a closing duplicate of the first
/// vertex). Returns `None` when fewer than two vertices remain. Every polyline entering the
/// kernel must pass through here: a zero-length arc segment is undefined.
pub fn sanitize(p: &Pline) -> Option<Pline> {
    let mut q = p.remove_repeat_pos(REPEAT_EPS).unwrap_or_else(|| p.clone());
    if q.is_closed()
        && q.vertex_count() >= 2
        && q.at(0)
            .pos()
            .fuzzy_eq_eps(q.last().expect("non-empty").pos(), REPEAT_EPS)
    {
        q.remove_last();
    }
    (q.vertex_count() >= 2).then_some(q)
}

/// Closed loop `p` opened at `at`, a point on its segment `seg`: an open path from `at` round
/// the loop and back to `at`, the split arc kept exact.
pub fn open_at(p: &Pline, seg: usize, at: Point) -> Pline {
    let n = p.vertex_count();
    let split = seg_split_at_point(
        p.at(seg),
        p.at((seg + 1) % n),
        Vector2::new(at.x, at.y),
        REPEAT_EPS,
    );
    let mut out = Pline::new();
    out.add(at.x, at.y, split.split_vertex.bulge);
    for k in 1..=n {
        let v = p.at((seg + k) % n);
        let bulge = if k == n {
            split.updated_start.bulge
        } else {
            v.bulge
        };
        out.add(v.x, v.y, bulge);
    }
    out.add(at.x, at.y, 0.0);
    sanitize(&out).unwrap_or(out)
}

/// The part of `p` from arc length `from` to arc length `to` (both measured from its first
/// vertex) as an open path, arcs kept exact. On a closed loop `to < from` runs on past the
/// first vertex. `None` when that part is shorter than [`REPEAT_EPS`].
pub fn sub_path(p: &Pline, from: f64, to: f64) -> Option<Pline> {
    let n = p.vertex_count();
    let segments = if p.is_closed() {
        n
    } else {
        n.saturating_sub(1)
    };
    if segments == 0 {
        return None;
    }
    let lengths: Vec<f64> = (0..segments)
        .map(|i| seg_length(p.at(i), p.at((i + 1) % n)))
        .collect();
    let total: f64 = lengths.iter().sum();
    let (from, mut to) = (from.clamp(0.0, total), to.clamp(0.0, total));
    if to < from {
        if !p.is_closed() {
            return None;
        }
        to += total;
    }
    if to - from <= REPEAT_EPS {
        return None;
    }
    let mut out = Pline::new();
    let mut end = point(p.at(0).pos());
    // Over at most two laps of the segments, `start` the arc length where segment k begins.
    let mut start = 0.0;
    for k in 0..2 * segments {
        if start >= to {
            break;
        }
        let (i, len) = (k % segments, lengths[k % segments]);
        let (a, b) = (from.max(start), to.min(start + len));
        if b > a && len > 0.0 {
            let (v1, v2) = (p.at(i), p.at((i + 1) % n));
            let (t0, t1) = ((a - start) / len, (b - start) / len);
            match Arc::from_bulge(point(v1.pos()), point(v2.pos()), v1.bulge) {
                Some(arc) => {
                    let q = arc.point_at(t0);
                    out.add(q.x, q.y, (arc.sweep * (t1 - t0) / 4.0).tan());
                    end = arc.point_at(t1);
                }
                None => {
                    let at =
                        |t: f64| Point::new(v1.x + t * (v2.x - v1.x), v1.y + t * (v2.y - v1.y));
                    let q = at(t0);
                    out.add(q.x, q.y, 0.0);
                    end = at(t1);
                }
            }
        }
        start += len;
    }
    out.add(end.x, end.y, 0.0);
    sanitize(&out)
}

/// Append `piece` to `path`; where `piece` starts at the end of `path` the two share that
/// vertex, otherwise a straight segment joins them.
pub fn append(path: &mut Pline, piece: &Pline) {
    let n = piece.vertex_count();
    if n == 0 {
        return;
    }
    let mut from = 0;
    // (cavalier's `last` underflows on an empty polyline rather than returning `None`.)
    if path.vertex_count() > 0
        && let Some(last) = path.last()
        && last.pos().fuzzy_eq_eps(piece.at(0).pos(), REPEAT_EPS)
    {
        path.remove_last();
        path.add(last.x, last.y, piece.at(0).bulge);
        from = 1;
    }
    for i in from..n {
        let v = piece.at(i);
        path.add(v.x, v.y, v.bulge);
    }
}

/// Remove what the offset kernel would misread from a loop, without changing it by more
/// than a fraction of a micron: vertices within `eps` of the one before, and vertices
/// between two straight segments the kernel takes for parallel and continuing. Its parallel
/// test compares the (unnormalised) cross product of the two segments with `eps`, so
/// segments a few microns long meeting at several degrees count as one line folded back on
/// itself, and an offset of such a boundary comes out broken. Removing the middle vertex
/// moves the boundary by at most `eps` over the length of the new segment. Arcs are kept;
/// a loop is never reduced below three vertices. `None` when fewer than two remain.
pub fn clean(p: &Pline, eps: f64) -> Option<Pline> {
    let n = p.vertex_count();
    if n < 2 {
        return None;
    }
    // Whether the kernel takes a -> b -> c for one straight line.
    let continuing = |a: &PlineVertex<f64>, b: &PlineVertex<f64>, c: &PlineVertex<f64>| {
        if a.bulge != 0.0 || b.bulge != 0.0 {
            return false;
        }
        let (d1x, d1y) = (b.x - a.x, b.y - a.y);
        let (d2x, d2y) = (c.x - b.x, c.y - b.y);
        (d1x * d2y - d1y * d2x).abs() <= eps && d1x * d2x + d1y * d2y >= 0.0
    };
    // One pass with a stack: a vertex at the position of the one before replaces its bulge,
    // a vertex in the middle of a straight line is dropped.
    let floor = if p.is_closed() { 3 } else { 2 };
    let mut vs: Vec<PlineVertex<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let v = p.at(i);
        if let Some(last) = vs.last_mut()
            && last.pos().fuzzy_eq_eps(v.pos(), eps)
        {
            last.bulge = v.bulge;
            continue;
        }
        while vs.len() >= floor && continuing(&vs[vs.len() - 2], &vs[vs.len() - 1], &v) {
            vs.pop();
        }
        vs.push(v);
    }
    if p.is_closed() {
        // Across the seam: the last vertex repeating the first, then the same for lines.
        while vs.len() > 1 && vs[vs.len() - 1].pos().fuzzy_eq_eps(vs[0].pos(), eps) {
            vs.pop();
        }
        while vs.len() > 3 && continuing(&vs[vs.len() - 2], &vs[vs.len() - 1], &vs[0]) {
            vs.pop();
        }
        while vs.len() > 3 && continuing(&vs[vs.len() - 1], &vs[0], &vs[1]) {
            vs.remove(0);
        }
    }
    if vs.len() < 2 {
        return None;
    }
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for v in vs {
        out.add(v.x, v.y, v.bulge);
    }
    Some(out)
}

/// Split a closed loop wherever it runs back along itself: an offset taken at exactly half
/// a feature's width keeps the collapsed feature as a zero-width bridge, one segment there
/// and the same segment reversed on the way back (vertices within [`REPEAT_EPS`]). Returns
/// the closed loops either side of the bridges and each bridge once, as an open path
/// (consecutive bridge segments joined; one that closes on itself, a circle run round once
/// each way, as a loop). A loop without bridges comes back unchanged; an open path is
/// returned as it is, among the open paths.
pub fn split_retraced(p: &Pline) -> (Vec<Pline>, Vec<Pline>) {
    if !p.is_closed() {
        return (Vec::new(), vec![p.clone()]);
    }
    let mut loops = Vec::new();
    let mut bridges: Vec<(Point, Point, f64)> = Vec::new();
    let mut todo = vec![p.clone()];
    while let Some(lp) = todo.pop() {
        let n = lp.vertex_count();
        let Some((i, j)) = retraced_pair(&lp) else {
            loops.push(lp);
            continue;
        };
        let v = |k: usize| lp.at(k % n);
        bridges.push((point(v(i).pos()), point(v(i + 1).pos()), v(i).bulge));
        // The two loops either side: segments i+1..j-1 and j+1..i-1 (cyclically).
        for (from, to) in [(i + 1, j), (j + 1, i + n)] {
            if to <= from {
                continue;
            }
            let mut piece = Pline::new_closed();
            for k in from..to {
                piece.add(v(k).x, v(k).y, v(k).bulge);
            }
            // A single vertex is no loop (and a one-vertex circle is never produced).
            if piece.vertex_count() >= 2 {
                todo.push(piece);
            }
        }
    }
    (loops, join_segments(bridges))
}

/// A pair of segments `(i, j)`, `i < j`, of closed loop `p` where `j` is `i` reversed.
fn retraced_pair(p: &Pline) -> Option<(usize, usize)> {
    let n = p.vertex_count();
    if n < 2 {
        return None;
    }
    let cell = |q: Vector2<f64>| {
        (
            (q.x / REPEAT_EPS).round() as i64,
            (q.y / REPEAT_EPS).round() as i64,
        )
    };
    // Segments by the cell of their start.
    let mut starts: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::with_capacity(n);
    for k in 0..n {
        starts.entry(cell(p.at(k).pos())).or_default().push(k);
    }
    for i in 0..n {
        let (a, b) = (p.at(i), p.at((i + 1) % n));
        let (cx, cy) = cell(b.pos());
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in starts.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    if j <= i {
                        continue;
                    }
                    let (c, d) = (p.at(j), p.at((j + 1) % n));
                    if c.pos().fuzzy_eq_eps(b.pos(), REPEAT_EPS)
                        && d.pos().fuzzy_eq_eps(a.pos(), REPEAT_EPS)
                        && (c.bulge + a.bulge).abs() <= 1e-9
                    {
                        return Some((i, j));
                    }
                }
            }
        }
    }
    None
}

/// Join single segments `(from, to, bulge)` that meet end to end into polylines, only
/// through points where exactly two of them meet; a chain that comes back to its start is
/// closed.
fn join_segments(segments: Vec<(Point, Point, f64)>) -> Vec<Pline> {
    let key = |p: Point| {
        (
            (p.x / REPEAT_EPS).round() as i64,
            (p.y / REPEAT_EPS).round() as i64,
        )
    };
    let mut at: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (k, s) in segments.iter().enumerate() {
        at.entry(key(s.0)).or_default().push(k);
        at.entry(key(s.1)).or_default().push(k);
    }
    let other = |k: usize, p: Point| -> Option<usize> {
        let ks = at.get(&key(p))?;
        (ks.len() == 2).then(|| if ks[0] == k { ks[1] } else { ks[0] })
    };
    let mut used = vec![false; segments.len()];
    let mut out = Vec::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        // Walk back to the beginning of the chain, then forward along it.
        let (mut k, mut head) = (start, segments[start].0);
        let mut steps = 0;
        while let Some(prev) = other(k, head) {
            if prev == start || steps > segments.len() {
                break;
            }
            let s = segments[prev];
            head = if key(s.1) == key(head) { s.0 } else { s.1 };
            k = prev;
            steps += 1;
        }
        let mut vertices = Vec::new();
        let mut at_point = head;
        loop {
            used[k] = true;
            let s = segments[k];
            let (to, bulge) = if key(s.0) == key(at_point) {
                (s.1, s.2)
            } else {
                (s.0, -s.2)
            };
            vertices.push((at_point, bulge));
            at_point = to;
            match other(k, to) {
                Some(next) if !used[next] => k = next,
                _ => break,
            }
        }
        // A bridge can close on itself (a circle run round once each way): a loop.
        let closed = vertices.len() >= 2 && key(at_point) == key(head);
        let mut chain = if closed {
            Pline::new_closed()
        } else {
            vertices.push((at_point, 0.0));
            Pline::new()
        };
        for (q, bulge) in vertices {
            chain.add(q.x, q.y, bulge);
        }
        out.push(chain);
    }
    out
}

/// Outline of a stroked polyline with flat ends and mitred joins (bevelled where the mitre
/// would exceed twice the width): the exact area a square-ended path covers. `ext_begin`
/// and `ext_end` extend the ends along the path (half the width for square ends). Returns
/// `None` for zero width or a single point without extension.
pub fn stroke_polygon(
    pts: &[Point],
    width: f64,
    ext_begin: f64,
    ext_end: f64,
) -> Option<Vec<Point>> {
    let mut p: Vec<Point> = Vec::new();
    for &q in pts {
        if p.last()
            .is_none_or(|l: &Point| (l.x - q.x).abs() > 1e-9 || (l.y - q.y).abs() > 1e-9)
        {
            p.push(q);
        }
    }
    let hw = width / 2.0;
    if hw <= 0.0 || p.is_empty() {
        return None;
    }
    if p.len() == 1 {
        let c = p[0];
        if ext_begin <= 0.0 && ext_end <= 0.0 {
            return None;
        }
        return Some(vec![
            Point::new(c.x - ext_begin, c.y - hw),
            Point::new(c.x + ext_end, c.y - hw),
            Point::new(c.x + ext_end, c.y + hw),
            Point::new(c.x - ext_begin, c.y + hw),
        ]);
    }
    let n = p.len();
    let dir = |a: Point, b: Point| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l = dx.hypot(dy);
        (dx / l, dy / l)
    };
    let d0 = dir(p[0], p[1]);
    p[0] = Point::new(p[0].x - d0.0 * ext_begin, p[0].y - d0.1 * ext_begin);
    let dn = dir(p[n - 2], p[n - 1]);
    p[n - 1] = Point::new(p[n - 1].x + dn.0 * ext_end, p[n - 1].y + dn.1 * ext_end);
    let segs: Vec<(Point, (f64, f64))> = p.windows(2).map(|w| (w[0], dir(w[0], w[1]))).collect();
    let side = |sign: f64| -> Vec<Point> {
        let normal = |d: (f64, f64)| (-d.1 * hw * sign, d.0 * hw * sign);
        let mut out = Vec::new();
        let (s0, d0) = segs[0];
        let n0 = normal(d0);
        out.push(Point::new(s0.x + n0.0, s0.y + n0.1));
        for k in 1..segs.len() {
            let (_, dprev) = segs[k - 1];
            let (pk, dk) = segs[k];
            let np = normal(dprev);
            let nk = normal(dk);
            let a = Point::new(pk.x + np.0, pk.y + np.1);
            let b = Point::new(pk.x + nk.0, pk.y + nk.1);
            let cross = dprev.0 * dk.1 - dprev.1 * dk.0;
            if cross.abs() < 1e-9 {
                out.push(a);
                continue;
            }
            let t = ((b.x - a.x) * dk.1 - (b.y - a.y) * dk.0) / cross;
            let m = Point::new(a.x + dprev.0 * t, a.y + dprev.1 * t);
            if (m.x - pk.x).hypot(m.y - pk.y) <= hw * 4.0 {
                out.push(m);
            } else {
                out.push(a);
                out.push(b);
            }
        }
        let (_, dl) = segs[segs.len() - 1];
        let nl = normal(dl);
        let end = p[n - 1];
        out.push(Point::new(end.x + nl.0, end.y + nl.1));
        out
    };
    let mut left = side(1.0);
    let mut right = side(-1.0);
    right.reverse();
    left.append(&mut right);
    Some(left)
}

/// Append the arc `from -> to` around `center` to an open polyline whose last vertex is
/// `from`, splitting arcs over a half turn and expressing a full circle (`from == to`) as
/// two half circles. The end point is trusted for direction only: the radius comes from
/// `from`, and `to` is pushed as given.
pub fn add_center_arc(p: &mut Pline, from: Point, to: Point, center: Point, ccw: bool) {
    use std::f64::consts::{PI, TAU};
    let r = (from.x - center.x).hypot(from.y - center.y);
    if r <= 0.0 {
        p.add(to.x, to.y, 0.0);
        return;
    }
    let a0 = (from.y - center.y).atan2(from.x - center.x);
    let a1 = (to.y - center.y).atan2(to.x - center.x);
    let same = (from.x - to.x).abs() < 1e-9 && (from.y - to.y).abs() < 1e-9;
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 }.rem_euclid(TAU);
    if same {
        sweep = TAU;
    }
    let sign = if ccw { 1.0 } else { -1.0 };
    let last = p.vertex_count() - 1;
    let set_last_bulge = |p: &mut Pline, b: f64| {
        let mut v = p.at(last);
        v.bulge = b;
        p.set_vertex(last, v);
    };
    if (sweep - TAU).abs() < 1e-9 {
        let opposite = Point::new(2.0 * center.x - from.x, 2.0 * center.y - from.y);
        set_last_bulge(p, sign);
        p.add(opposite.x, opposite.y, sign);
        p.add(to.x, to.y, 0.0);
        return;
    }
    if sweep > PI + 1e-12 {
        let amid = a0 + sign * sweep / 2.0;
        let mid = Point::new(center.x + r * amid.cos(), center.y + r * amid.sin());
        let b = sign * (sweep / 8.0).tan();
        set_last_bulge(p, b);
        p.add(mid.x, mid.y, b);
        p.add(to.x, to.y, 0.0);
        return;
    }
    set_last_bulge(p, sign * (sweep / 4.0).tan());
    p.add(to.x, to.y, 0.0);
}

/// Simplify a polyline to within `tolerance`: arcs whose sagitta is at most half the
/// tolerance count as their chord, then Douglas-Peucker with the other half runs over the
/// straight runs. Larger arcs and the vertices that bound them are kept exactly. Closed
/// polylines without kept arcs use vertex 0 and the vertex farthest from it as anchors.
pub fn simplify(p: &Pline, tolerance: f64) -> Pline {
    let n = p.vertex_count();
    if tolerance <= 0.0 || n < 3 {
        return p.clone();
    }
    let half = tolerance / 2.0;
    let mut flat = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for i in 0..n {
        let v = p.at(i);
        let w = p.at((i + 1) % n);
        let chord = (w.x - v.x).hypot(w.y - v.y);
        let sagitta = v.bulge.abs() * chord / 2.0;
        let bulge = if v.bulge != 0.0 && sagitta <= half {
            0.0
        } else {
            v.bulge
        };
        flat.add(v.x, v.y, bulge);
    }
    let p = &flat;
    let tolerance = half;
    let pt = |i: usize| {
        let v = p.at(i % n);
        Point::new(v.x, v.y)
    };
    let closed = p.is_closed();
    let seg_count = if closed { n } else { n - 1 };
    // Anchors: vertices that start or end an arc segment, the ends of an open polyline.
    let mut keep = vec![false; n];
    for i in 0..seg_count {
        if p.at(i).bulge != 0.0 {
            keep[i] = true;
            keep[(i + 1) % n] = true;
        }
    }
    if !closed {
        keep[0] = true;
        keep[n - 1] = true;
    }
    if !keep.iter().any(|k| *k) {
        keep[0] = true;
        let far = (1..n)
            .max_by(|&a, &b| {
                let (pa, pb, o) = (pt(a), pt(b), pt(0));
                (pa.x - o.x)
                    .hypot(pa.y - o.y)
                    .partial_cmp(&(pb.x - o.x).hypot(pb.y - o.y))
                    .unwrap()
            })
            .unwrap_or(n / 2);
        keep[far] = true;
    }
    fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l2 = dx * dx + dy * dy;
        if l2 <= 0.0 {
            return (p.x - a.x).hypot(p.y - a.y);
        }
        let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0);
        (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
    }
    // Douglas-Peucker on each straight run between consecutive anchors (indices may wrap).
    let anchors: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    let runs: Vec<(usize, usize)> = if closed {
        (0..anchors.len())
            .map(|k| (anchors[k], anchors[(k + 1) % anchors.len()]))
            .collect()
    } else {
        anchors.windows(2).map(|w| (w[0], w[1])).collect()
    };
    for (start, end) in runs {
        let len = if end > start {
            end - start
        } else {
            end + n - start
        };
        if len < 2 || p.at(start).bulge != 0.0 {
            continue;
        }
        let mut stack = vec![(0usize, len)];
        while let Some((lo, hi)) = stack.pop() {
            if hi <= lo + 1 {
                continue;
            }
            let (a, b) = (pt(start + lo), pt(start + hi));
            let (mut best, mut at) = (0.0, lo);
            for k in lo + 1..hi {
                let dk = seg_dist(pt(start + k), a, b);
                if dk > best {
                    best = dk;
                    at = k;
                }
            }
            if best > tolerance {
                keep[(start + at) % n] = true;
                stack.push((lo, at));
                stack.push((at, hi));
            }
        }
    }
    let mut out = if closed {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for (i, _) in keep.iter().enumerate().filter(|(_, k)| **k) {
        let v = p.at(i);
        out.add(v.x, v.y, v.bulge);
    }
    out
}

/// Whether an open point sequence ends where it starts (at least three distinct points):
/// such a path is a loop and is closed by the readers, as Gerber draw chains are.
pub fn returns_to_start(pts: &[(Point, f64)]) -> bool {
    pts.len() >= 3 && {
        let (a, b) = (pts[0].0, pts[pts.len() - 1].0);
        (a.x - b.x).abs() < REPEAT_EPS && (a.y - b.y).abs() < REPEAT_EPS
    }
}

/// Polyline from `(point, bulge of the segment that starts there)` pairs. Repeated points
/// are dropped (keeping the bulge of the segment that actually moves), and a closed
/// polyline whose last point repeats the first loses that duplicate. Returns `None` when
/// fewer than two distinct points remain.
pub fn from_bulged_points(pts: &[(Point, f64)], closed: bool) -> Option<Pline> {
    let mut v: Vec<(Point, f64)> = Vec::with_capacity(pts.len());
    for &(p, b) in pts {
        match v.last_mut() {
            Some(last)
                if (last.0.x - p.x).abs() < REPEAT_EPS && (last.0.y - p.y).abs() < REPEAT_EPS =>
            {
                // Same position: the later bulge describes the segment that leaves it.
                last.1 = b;
            }
            _ => v.push((p, b)),
        }
    }
    if closed
        && v.len() >= 2
        && (v[0].0.x - v[v.len() - 1].0.x).abs() < REPEAT_EPS
        && (v[0].0.y - v[v.len() - 1].0.y).abs() < REPEAT_EPS
    {
        v.pop();
    }
    if v.len() < 2 {
        return None;
    }
    let mut out = if closed {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    let n = v.len();
    for (i, &(p, b)) in v.iter().enumerate() {
        let next = if i + 1 < n {
            Some(v[i + 1].0)
        } else if closed {
            Some(v[0].0)
        } else {
            None
        };
        match next {
            Some(q) if b != 0.0 => add_bulge_segment(&mut out, p, q, b),
            _ => out.add(p.x, p.y, 0.0),
        }
    }
    sanitize(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cavalier_contours::polyline::seg_closest_point;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    #[test]
    fn large_bulges_are_split() {
        // 270° arc from (1,0) to (0,-1) CCW around the origin: bulge = tan(270°/4) = tan(67.5°).
        let b = (3.0 * PI / 4.0 / 2.0).tan();
        assert!(b > 1.0);
        let p = from_bulge_vertices([(1.0, 0.0, b), (0.0, -1.0, 0.0)], false);
        assert_eq!(p.vertex_count(), 3);
        for (v1, _) in p.iter_segments() {
            assert!(v1.bulge.abs() <= MAX_BULGE + 1e-12);
        }
        let pts = flatten(&p, 1e-4).unwrap();
        for q in &pts {
            assert!((q.distance(Point::new(0.0, 0.0)) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn circle_area_and_orientation() {
        let c = circle(Point::new(3.0, 4.0), 2.0);
        assert!((c.area() - PI * 4.0).abs() < 1e-9);
        assert_eq!(is_ccw(&c), Some(true));
        let cw = {
            let mut p = c.clone();
            p.invert_direction_mut();
            p
        };
        assert_eq!(is_ccw(&cw), Some(false));
        assert_eq!(is_ccw(&ensure_ccw(cw)), Some(true));
    }

    #[test]
    fn flatten_closed_does_not_repeat_first_point() {
        let r = rect(Point::new(0.0, 0.0), Point::new(2.0, 1.0));
        let pts = flatten(&r, 1e-3).unwrap();
        assert_eq!(pts.len(), 4);
        let c = circle(Point::new(0.0, 0.0), 1.0);
        let pts = flatten(&c, 1e-3).unwrap();
        assert!(pts.len() > 8);
        assert!(pts[0].distance(*pts.last().unwrap()) > 1e-3);
    }

    #[test]
    fn sanitize_drops_repeats_and_closing_duplicate() {
        // Two half-circle arcs, each start vertex duplicated, plus a closing duplicate.
        let mut p = Pline::new_closed();
        p.add(0.0, 0.0, 1.0);
        p.add(0.0, 0.0, 1.0);
        p.add(2.0, 0.0, 1.0);
        p.add(2.0, 0.0, 1.0);
        p.add(0.0, 0.0, 0.0);
        let q = sanitize(&p).unwrap();
        assert_eq!(q.vertex_count(), 2);
        assert!(
            (q.area() - PI).abs() < 1e-9,
            "unit circle remains: {}",
            q.area()
        );
        let mut single = Pline::new();
        single.add(1.0, 1.0, 0.0);
        single.add(1.0, 1.0, 0.0);
        assert!(sanitize(&single).is_none());
    }

    #[test]
    fn left_normal_points_inside_ccw_rect() {
        let r = rect(Point::new(0.0, 0.0), Point::new(2.0, 2.0));
        let (mid, n) = seg_midpoint_and_left_normal(r.at(0), r.at(1));
        assert!((mid.x - 1.0).abs() < 1e-12 && mid.y.abs() < 1e-12);
        assert!(n.y > 0.99); // left of +x travel is +y, i.e. into the rectangle
    }

    /// A loop around square A, out along `bridge` to square B, around B, and back along the
    /// bridge (reversed): what an offset at exactly half a trace's width produces.
    fn bridged(bridge: &[(f64, f64, f64)]) -> Pline {
        let (s, e) = (bridge[0], bridge[bridge.len() - 1]);
        let mut p = Pline::new_closed();
        // Square A below the bridge start, counter-clockwise, ending at the bridge start.
        p.add(s.0, s.1, 0.0);
        p.add(s.0 - 1.0, s.1, 0.0);
        p.add(s.0 - 1.0, s.1 - 2.0, 0.0);
        p.add(s.0 + 1.0, s.1 - 2.0, 0.0);
        p.add(s.0 + 1.0, s.1, 0.0);
        // Out along the bridge.
        p.add(s.0, s.1, bridge[0].2);
        for w in &bridge[1..bridge.len() - 1] {
            p.add(w.0, w.1, w.2);
        }
        // Square B above the bridge end.
        p.add(e.0, e.1, 0.0);
        p.add(e.0 + 1.0, e.1, 0.0);
        p.add(e.0 + 1.0, e.1 + 2.0, 0.0);
        p.add(e.0 - 1.0, e.1 + 2.0, 0.0);
        p.add(e.0 - 1.0, e.1, 0.0);
        // Back along the bridge.
        p.add(e.0, e.1, -bridge[bridge.len() - 2].2);
        for k in (1..bridge.len() - 1).rev() {
            p.add(bridge[k].0, bridge[k].1, -bridge[k - 1].2);
        }
        p
    }

    #[test]
    fn retraced_bridges_split_into_loops_and_one_open_path() {
        let p = bridged(&[
            (0.0, 0.0, 0.0),
            (0.0, 3.0, 0.3),
            (0.5, 4.0, 0.0),
            (0.5, 6.0, 0.0),
        ]);
        let (loops, open) = split_retraced(&p);
        assert_eq!(loops.len(), 2);
        assert_eq!(open.len(), 1, "the bridge comes out once, as one path");
        for l in &loops {
            assert!(l.is_closed());
            assert!((l.area() - 4.0).abs() < 1e-9, "area {}", l.area());
        }
        assert_eq!(open[0].vertex_count(), 4);
        let total: f64 = loops.iter().map(length).sum::<f64>() + 2.0 * length(&open[0]);
        assert!((total - length(&p)).abs() < 1e-9);
        // A loop without a bridge is left alone; an open path is passed through.
        let square = rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0));
        let (loops, open) = split_retraced(&square);
        assert!(open.is_empty());
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].vertex_data, square.vertex_data);
        let line = from_points([Point::new(0.0, 0.0), Point::new(1.0, 0.0)], false);
        let (loops, open) = split_retraced(&line);
        assert!(loops.is_empty());
        assert_eq!(open[0].vertex_data, line.vertex_data);
        // A spike: out and straight back.
        let spike = from_points(
            [
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 0.5),
                (3.0, 0.5),
                (1.0, 0.5),
                (1.0, 1.0),
                (0.0, 1.0),
            ]
            .map(|(x, y)| Point::new(x, y)),
            true,
        );
        let (loops, open) = split_retraced(&spike);
        assert_eq!((loops.len(), open.len()), (1, 1));
        assert!((loops[0].area() - 1.0).abs() < 1e-12);
        assert!((length(&open[0]) - 2.0).abs() < 1e-12);
        // A circle run round once each way (a ring at exactly half an annulus's width):
        // the circle once, closed.
        let mut twice = Pline::new_closed();
        for (x, b) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            twice.add(x, 0.0, b);
        }
        let (loops, bridges) = split_retraced(&twice);
        assert!(loops.is_empty());
        assert_eq!(bridges.len(), 1);
        assert!(bridges[0].is_closed());
        assert!((length(&bridges[0]) - 2.0 * PI).abs() < 1e-12);
    }

    #[test]
    fn loops_open_at_any_point_and_paths_cut_by_arc_length() {
        let square = rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0));
        let opened = open_at(&square, 0, Point::new(0.5, 0.0));
        assert!(!opened.is_closed());
        assert_eq!(opened.vertex_count(), 6);
        assert!((length(&opened) - 4.0).abs() < 1e-12);
        assert_eq!((opened.at(0).x, opened.at(5).x), (0.5, 0.5));
        // At a vertex: no zero-length piece.
        assert_eq!(open_at(&square, 1, Point::new(1.0, 0.0)).vertex_count(), 5);
        let c = circle(Point::new(0.0, 0.0), 1.0);
        let at = Point::new(0.0, 1.0);
        let seg = (0..c.vertex_count())
            .find(|&i| {
                (seg_closest_point(
                    c.at(i),
                    c.at((i + 1) % c.vertex_count()),
                    Vector2::new(0.0, 1.0),
                    1e-9,
                ) - Vector2::new(0.0, 1.0))
                .length()
                    < 1e-9
            })
            .unwrap();
        let opened = open_at(&c, seg, at);
        assert!((length(&opened) - 2.0 * PI).abs() < 1e-9);
        assert!(opened.at(0).pos().fuzzy_eq(Vector2::new(0.0, 1.0)));

        // Cut by arc length: along an edge and round a corner, and past the start of a loop.
        let piece = sub_path(&square, 0.5, 1.5).unwrap();
        assert!((length(&piece) - 1.0).abs() < 1e-12);
        assert!(piece.at(0).pos().fuzzy_eq(Vector2::new(0.5, 0.0)));
        assert!(piece.last().unwrap().pos().fuzzy_eq(Vector2::new(1.0, 0.5)));
        let wrapped = sub_path(&square, 3.5, 0.5).unwrap();
        assert!((length(&wrapped) - 1.0).abs() < 1e-12);
        assert!(wrapped.at(0).pos().fuzzy_eq(Vector2::new(0.0, 0.5)));
        let half = sub_path(&c, 0.0, PI).unwrap();
        assert!((length(&half) - PI).abs() < 1e-9);
        let line = from_points([Point::new(0.0, 0.0), Point::new(2.0, 0.0)], false);
        assert!(
            sub_path(&line, 1.5, 0.5).is_none(),
            "an open path does not wrap"
        );
        assert!(sub_path(&line, 1.0, 1.0).is_none());

        // Appending shares the meeting vertex, or joins with a straight segment.
        let mut path = from_points([Point::new(0.0, 0.0), Point::new(1.0, 0.0)], false);
        append(
            &mut path,
            &from_points([Point::new(1.0, 0.0), Point::new(1.0, 1.0)], false),
        );
        assert_eq!(path.vertex_count(), 3);
        append(
            &mut path,
            &from_points([Point::new(2.0, 1.0), Point::new(3.0, 1.0)], false),
        );
        assert_eq!(path.vertex_count(), 5);
        assert!((length(&path) - 4.0).abs() < 1e-12);
    }

    proptest! {
        #[test]
        fn cutting_a_loop_by_arc_length_measures_up(
            verts in prop::collection::vec((-5.0..5.0f64, -5.0..5.0f64, -0.8..0.8f64), 3..8),
            a in 0.0..1.0f64,
            b in 0.0..1.0f64,
        ) {
            let p = from_bulge_vertices(verts.iter().copied(), true);
            let total = length(&p);
            prop_assume!(total > 1e-3);
            let (from, to) = (a * total, b * total);
            let expected = (to - from).rem_euclid(total);
            match sub_path(&p, from, to) {
                Some(piece) => prop_assert!((length(&piece) - expected).abs() < 1e-7 * (1.0 + total), "{} vs {}", length(&piece), expected),
                None => prop_assert!(expected <= REPEAT_EPS * 2.0 || total - expected <= REPEAT_EPS * 2.0),
            }
        }

        #[test]
        fn splitting_retraced_loops_keeps_area_and_length(
            steps in prop::collection::vec((-1.0..1.0f64, 0.2..2.0f64, -0.5..0.5f64), 1..6)
        ) {
            // A bridge of random straight and arc steps, heading up.
            let mut bridge = vec![(0.0, 0.0, 0.0)];
            for (dx, dy, b) in &steps {
                let last = *bridge.last().unwrap();
                bridge.last_mut().unwrap().2 = *b;
                bridge.push((last.0 + dx, last.1 + dy, 0.0));
            }
            let p = bridged(&bridge);
            let (loops, open) = split_retraced(&p);
            prop_assert_eq!(loops.len(), 2);
            prop_assert_eq!(open.len(), 1);
            let area: f64 = loops.iter().map(|l| l.area()).sum();
            prop_assert!((area - p.area()).abs() < 1e-9);
            let total: f64 = loops.iter().map(length).sum::<f64>() + 2.0 * length(&open[0]);
            prop_assert!((total - length(&p)).abs() < 1e-9);
        }

        #[test]
        fn flattened_circle_stays_on_circle(cx in -100.0..100.0f64, cy in -100.0..100.0f64,
                                            r in 0.01..50.0f64, tol in 1e-4..0.1f64) {
            let c = circle(Point::new(cx, cy), r);
            let pts = flatten(&c, tol).unwrap();
            for q in &pts {
                prop_assert!((q.distance(Point::new(cx, cy)) - r).abs() <= 1e-9 * (r + cx.abs() + cy.abs()));
            }
            // Shoelace cancellation grows with the distance from the origin.
            prop_assert!((c.area() - PI * r * r).abs() <= 1e-10 * (r * r + cx * cx + cy * cy));
        }

        #[test]
        fn bulge_vertices_roundtrip_length(verts in prop::collection::vec((-50.0..50.0f64, -50.0..50.0f64, -3.0..3.0f64), 2..8)) {
            // Splitting arcs must not change the path length.
            let p = from_bulge_vertices(verts.iter().copied(), false);
            let mut expected = 0.0;
            for w in verts.windows(2) {
                let (a, b) = (Point::new(w[0].0, w[0].1), Point::new(w[1].0, w[1].1));
                expected += match Arc::from_bulge(a, b, w[0].2) {
                    Some(arc) => arc.radius * arc.sweep.abs(),
                    None => a.distance(b),
                };
            }
            prop_assert!((length(&p) - expected).abs() <= 1e-7 * (1.0 + expected));
        }
    }

    #[test]
    fn simplify_drops_near_collinear_vertices_and_keeps_arcs() {
        // A 10 x 10 square whose bottom edge wiggles by 1 um; simplified at 10 um it is
        // the plain square again.
        let mut pts = vec![Point::new(0.0, 0.0)];
        for k in 1..100 {
            pts.push(Point::new(
                k as f64 * 0.1,
                if k % 2 == 0 { 0.001 } else { -0.001 },
            ));
        }
        pts.extend([
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ]);
        let p = from_points(pts, true);
        let s = simplify(&p, 0.01);
        assert_eq!(s.vertex_count(), 4, "{}", s.vertex_count());
        assert!((s.area() - 100.0).abs() < 1e-9);
        // Within tolerance of the original everywhere: the wiggle is smaller than it.
        assert_eq!(simplify(&p, 0.0001).vertex_count(), p.vertex_count());
        // Arcs survive untouched.
        let c = circle(Point::new(0.0, 0.0), 1.0);
        let sc = simplify(&c, 0.5);
        assert_eq!(sc.vertex_count(), 2);
        assert!((sc.area() - std::f64::consts::PI).abs() < 1e-12);
    }
}
