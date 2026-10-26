//! Pattern passes: turn a working layer's geometry into marking centrelines.
//!
//! Every pattern yields ordered polylines (open or closed) in mm. Output is geometry,
//! never a machine process; cosmetic PDF stroke width and assumed mark width are not
//! inputs here (except where a coverage audit asks for them explicitly).

pub mod gapfill;
pub mod hatch;
pub mod trace;

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};

use crate::diag::Diagnostic;
use crate::geom::Point;
use crate::model::{Pattern, RingOrder};
use crate::pline::Pline;
use crate::region::Region;

/// Geometry a working layer evaluates to.
#[derive(Clone, Debug)]
pub enum Geometry {
    Region(Region),
    Linework(Vec<Pline>),
}

impl Geometry {
    pub fn is_empty(&self) -> bool {
        match self {
            Geometry::Region(r) => r.is_empty(),
            Geometry::Linework(l) => l.is_empty(),
        }
    }

    /// Closed boundary loops (regions) or the paths themselves (linework).
    pub fn paths(&self) -> Vec<Pline> {
        match self {
            Geometry::Region(r) => r.loops.clone(),
            Geometry::Linework(l) => l.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PatternStats {
    pub rings: usize,
    pub gap_fills: usize,
}

#[derive(Clone, Debug, Default)]
pub struct PatternOutput {
    pub paths: Vec<Pline>,
    pub diagnostics: Vec<Diagnostic>,
    pub stats: PatternStats,
    /// Ordering constraints `(before, after)` as indices into `paths` that any path-order
    /// optimisation must keep (cutting: holes before the contour that contains them).
    pub precedence: Vec<(usize, usize)>,
    /// Open paths (indices into `paths`) whose direction any path order must keep.
    pub directed: Vec<usize>,
}

/// Upper bound on contour rings, as a guard against a pitch far below the geometry size.
const MAX_RINGS: usize = 100_000;

pub fn generate(pattern: &Pattern, geometry: &Geometry) -> PatternOutput {
    match pattern {
        Pattern::Passthrough {
            simplify_mm,
            reverse,
        } => passthrough(geometry, *simplify_mm, *reverse),
        Pattern::ContourFill {
            pitch_mm,
            first_offset_mm,
            gap_fill,
            end_spacing_mm,
            simplify_mm,
            ring_order,
            link,
        } => contour_fill(
            geometry,
            &ContourParams {
                pitch_mm: *pitch_mm,
                first_offset_mm: *first_offset_mm,
                gap_fill: *gap_fill,
                end_spacing_mm: *end_spacing_mm,
                simplify_mm: *simplify_mm,
                ring_order: *ring_order,
                link: *link,
            },
        ),
        Pattern::BoundaryTrace {
            side,
            offset_mm,
            kerf_mm,
            rings,
            ring_spacing_mm,
            reverse,
            inner_first,
            join,
        } => with_region(geometry, "boundary trace", |region| {
            let (paths, precedence) = trace::boundary_trace_ordered(
                region,
                &trace::TraceParams {
                    side: *side,
                    offset_mm: *offset_mm,
                    kerf_mm: *kerf_mm,
                    rings: *rings,
                    ring_spacing_mm: *ring_spacing_mm,
                    reverse: *reverse,
                    inner_first: *inner_first,
                    join: *join,
                },
            );
            PatternOutput {
                stats: PatternStats {
                    rings: paths.len(),
                    gap_fills: 0,
                },
                paths,
                precedence,
                ..Default::default()
            }
        }),
        Pattern::Hatch {
            pitch_mm,
            angle_deg,
            phase_mm,
            serpentine,
            inset_mm,
        } => with_region(geometry, "hatch", |region| {
            let area = if *inset_mm != 0.0 {
                region.offset(-inset_mm)
            } else {
                region.clone()
            };
            PatternOutput {
                paths: hatch::hatch(
                    &area,
                    &hatch::HatchParams {
                        pitch_mm: *pitch_mm,
                        angle_deg: *angle_deg,
                        phase_mm: *phase_mm,
                        serpentine: *serpentine,
                    },
                ),
                ..Default::default()
            }
        }),
        Pattern::CrossHatch {
            pitch_mm,
            angles_deg,
            phase_mm,
            serpentine,
            inset_mm,
        } => with_region(geometry, "crosshatch", |region| {
            let area = if *inset_mm != 0.0 {
                region.offset(-inset_mm)
            } else {
                region.clone()
            };
            let mut out = PatternOutput::default();
            for angle in angles_deg {
                out.paths.extend(hatch::hatch(
                    &area,
                    &hatch::HatchParams {
                        pitch_mm: *pitch_mm,
                        angle_deg: *angle,
                        phase_mm: *phase_mm,
                        serpentine: *serpentine,
                    },
                ));
            }
            out
        }),
        Pattern::BoundaryHatch {
            shells,
            shell_pitch_mm,
            inset_mm,
            pitch_mm,
            angles_deg,
            phase_mm,
            serpentine,
        } => with_region(geometry, "boundary + hatch", |region| {
            let mut out = PatternOutput::default();
            // Shells at pitch/2, 3 pitch/2, ... inward (centreline semantics, like contour fill).
            for k in 0..*shells {
                let ring = region.offset(-(shell_pitch_mm / 2.0 + k as f64 * shell_pitch_mm));
                out.stats.rings += ring.loops.len();
                out.paths.extend(ring.loops);
            }
            let interior = region.offset(-(*shells as f64 * shell_pitch_mm + inset_mm));
            for angle in angles_deg {
                out.paths.extend(hatch::hatch(
                    &interior,
                    &hatch::HatchParams {
                        pitch_mm: *pitch_mm,
                        angle_deg: *angle,
                        phase_mm: *phase_mm,
                        serpentine: *serpentine,
                    },
                ));
            }
            out
        }),
    }
}

/// Run a region-only pattern, or explain why linework will not do.
fn with_region(
    geometry: &Geometry,
    what: &str,
    f: impl FnOnce(&Region) -> PatternOutput,
) -> PatternOutput {
    match geometry {
        Geometry::Region(region) => f(region),
        Geometry::Linework(_) => PatternOutput {
            diagnostics: vec![Diagnostic::error(
                "pattern.needs_region",
                format!(
                    "{what} needs a region layer; this layer is linework (expand strokes or choose passthrough)"
                ),
            )],
            ..Default::default()
        },
    }
}

fn passthrough(geometry: &Geometry, simplify_mm: Option<f64>, reverse: bool) -> PatternOutput {
    let mut out = PatternOutput::default();
    for mut p in geometry.paths() {
        if let Some(eps) = simplify_mm
            && eps > 0.0
            && let Some(s) = p.remove_redundant(eps)
        {
            p = s;
        }
        if reverse {
            p.invert_direction_mut();
        }
        if p.vertex_count() >= 2 {
            out.paths.push(p);
        }
    }
    out
}

/// Contour fill settings with the defaults applied and the tolerance clamped.
#[derive(Clone, Copy)]
struct FillSettings {
    pitch: f64,
    first: f64,
    end_spacing: f64,
    simplify: Option<f64>,
    gap_fill: bool,
    order: RingOrder,
    link: bool,
}

struct IslandFill {
    paths: Vec<Pline>,
    /// Ordering constraints `(before, after)` between `paths`.
    precedence: Vec<(usize, usize)>,
    /// Paths whose direction matters.
    directed: Vec<usize>,
    rings: usize,
    gap_fills: usize,
    skipped: usize,
}

/// Rings of one island from depth `first` inward, then the gap-fill centrelines (the
/// other way round for an outward ring order).
fn fill_island(region: &Region, s: &FillSettings) -> IslandFill {
    let mut fill = IslandFill {
        paths: Vec::new(),
        precedence: Vec::new(),
        directed: Vec::new(),
        rings: 0,
        gap_fills: 0,
        skipped: 0,
    };
    // The area inside each ring (`region` shrunk by the ring's depth) and the indices of
    // its paths.
    let mut levels: Vec<Region> = Vec::new();
    let mut level_paths: Vec<Vec<usize>> = Vec::new();
    let mut depth = s.first;
    loop {
        if crate::cancel::cancelled() {
            return fill;
        }
        let ring = region.offset(-depth);
        if ring.is_empty() || fill.rings >= MAX_RINGS {
            break;
        }
        // At exactly half a feature's width the ring runs into the feature and back along
        // its centreline: that stretch is drawn once, as an open path.
        let mut here = Vec::new();
        for l in &ring.loops {
            let (loops, bridges) = crate::pline::split_retraced(l);
            for p in loops.into_iter().chain(bridges) {
                fill.rings += 1;
                here.push(fill.paths.len());
                fill.paths.push(match s.simplify {
                    Some(t) => crate::pline::simplify(&p, t),
                    None => p,
                });
            }
        }
        level_paths.push(here);
        if s.gap_fill {
            levels.push(ring);
        }
        depth += s.pitch;
    }
    // Gap-fill centrelines with the level of the ring outside them (-1: before the first).
    let mut gaps: Vec<(isize, usize)> = Vec::new();
    if s.gap_fill {
        let opts = gapfill::GapFillOptions {
            assumed_width_mm: s.pitch,
            flatten_tolerance_mm: (s.pitch * 0.01).max(1e-5),
            min_length_mm: s.pitch / 2.0,
        };
        for (level, core) in cores(region, &levels, s.first, s.pitch, s.end_spacing) {
            if crate::cancel::cancelled() {
                return fill;
            }
            let (lines, skipped) = gapfill::centrelines(&core, &opts);
            fill.gap_fills += lines.len();
            for line in lines {
                gaps.push((level, fill.paths.len()));
                fill.paths.push(line);
            }
            fill.skipped += skipped;
        }
    }
    let ordered = s.order != RingOrder::Any;
    let mut pairs = if ordered {
        ring_parents(&fill.paths, &level_paths, &gaps, s.pitch)
    } else {
        Vec::new()
    };
    if s.link {
        // Simplification moves a ring by up to its tolerance, so the nearest point of the
        // ring outside may be that much farther or nearer on both rings.
        let reach = s.pitch * (1.0 + 1e-9) + 2.0 * s.simplify.unwrap_or(0.0) + 1e-9;
        let chains = link_rings(&fill.paths, &level_paths, reach);
        let paths = std::mem::take(&mut fill.paths);
        (fill.paths, pairs, fill.directed) = join_chains(paths, &chains, &pairs, ordered);
    }
    if s.order == RingOrder::Outward {
        // Core first: generated in reverse (chains run backwards), every path before the
        // ring outside it.
        let n = fill.paths.len();
        for &d in &fill.directed {
            fill.paths[d].invert_direction_mut();
        }
        fill.paths.reverse();
        fill.directed = fill.directed.iter().map(|&d| n - 1 - d).collect();
        pairs = pairs
            .into_iter()
            .map(|(outer, inner)| (n - 1 - inner, n - 1 - outer))
            .collect();
    }
    fill.precedence = pairs;
    fill
}

/// Rings of one island joined into chains (see [`link_rings`]).
struct Chains {
    /// Ring indices (into the paths) of each chain, outermost first.
    chains: Vec<Vec<usize>>,
    /// Where each chained ring is entered and left: its segment and the point on it.
    seams: std::collections::HashMap<usize, (usize, Point)>,
    /// The ring outside each chain's first ring, if any (one another chain took).
    outside: Vec<Option<usize>>,
}

/// Join nested rings into chains. Working inside out: a ring's seam is where the ring
/// inside it connects (or its first vertex), and its connector runs from there to the
/// nearest point of the rings outside, which becomes that ring's seam. That point is a
/// pitch away (a ring lies at one depth, and a pitch less is reached straight on the way
/// out), so the connector stays in the band between the two rings and crosses nothing;
/// farther than `reach` means the ring outside is not there. A ring already taken by
/// another chain ends this one.
fn link_rings(paths: &[Pline], level_paths: &[Vec<usize>], reach: f64) -> Chains {
    let loops: Vec<Vec<usize>> = level_paths
        .iter()
        .map(|l| {
            l.iter()
                .copied()
                .filter(|&i| paths[i].is_closed())
                .collect()
        })
        .collect();
    let mut seams = std::collections::HashMap::new();
    let mut inner_of = std::collections::HashMap::new();
    let mut outside = std::collections::HashMap::new();
    for j in (1..loops.len()).rev() {
        let index = NearestPath::new(paths, &loops[j - 1], 2.0 * reach);
        for &c in &loops[j] {
            let (seg, q) = seams.get(&c).copied().unwrap_or_else(|| {
                let v = paths[c].at(0);
                (0, Point::new(v.x, v.y))
            });
            let Some((p, p_seg, at, d)) = index.nearest_point(q) else {
                continue;
            };
            outside.insert(c, p);
            if d <= reach && !inner_of.contains_key(&p) {
                inner_of.insert(p, c);
                seams.insert(p, (p_seg, at));
                seams.insert(c, (seg, q));
            }
        }
    }
    let inner: std::collections::HashSet<usize> = inner_of.values().copied().collect();
    let mut chains = Chains {
        chains: Vec::new(),
        seams,
        outside: Vec::new(),
    };
    for &head in loops.iter().flatten() {
        if inner.contains(&head) {
            continue;
        }
        let mut chain = vec![head];
        while let Some(&next) = inner_of.get(chain.last().unwrap()) {
            chain.push(next);
        }
        chains.outside.push(outside.get(&head).copied());
        chains.chains.push(chain);
    }
    chains
}

/// The paths with every chain of rings joined into one (a ring alone stays closed): the
/// chains first, then the other paths. Also the constraints carried over to them (a chain
/// after the chain holding the ring outside its first ring; a centreline or bridge after
/// the chain holding its ring) and the joined chains, whose direction then matters.
fn join_chains(
    paths: Vec<Pline>,
    chains: &Chains,
    pairs: &[(usize, usize)],
    ordered: bool,
) -> (Vec<Pline>, Vec<(usize, usize)>, Vec<usize>) {
    let mut index = vec![usize::MAX; paths.len()];
    let mut out = Vec::with_capacity(paths.len());
    let mut directed = Vec::new();
    for chain in &chains.chains {
        let k = out.len();
        if let [ring] = chain[..] {
            out.push(paths[ring].clone());
        } else {
            let mut path = Pline::new();
            for &r in chain {
                let (seg, at) = chains.seams[&r];
                crate::pline::append(&mut path, &crate::pline::open_at(&paths[r], seg, at));
            }
            out.push(path);
            if ordered {
                directed.push(k);
            }
        }
        for &r in chain {
            index[r] = k;
        }
    }
    for (i, p) in paths.into_iter().enumerate() {
        if index[i] == usize::MAX {
            index[i] = out.len();
            out.push(p);
        }
    }
    let mut constraints = std::collections::BTreeSet::new();
    if ordered {
        for (k, o) in chains.outside.iter().enumerate() {
            if let Some(o) = o {
                constraints.insert((index[*o], k));
            }
        }
        for &(outer, inner) in pairs {
            if index[inner] >= chains.chains.len() {
                constraints.insert((index[outer], index[inner]));
            }
        }
    }
    constraints.retain(|(a, b)| a != b);
    (out, constraints.into_iter().collect(), directed)
}

/// Ties every ring after the first level, and every gap-fill centreline, to the nearest path
/// of the ring level just outside it: `(outer, inner)` pairs of indices into `paths`.
fn ring_parents(
    paths: &[Pline],
    level_paths: &[Vec<usize>],
    gaps: &[(isize, usize)],
    pitch: f64,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (j, outer) in level_paths.iter().enumerate() {
        let inner = level_paths
            .get(j + 1)
            .into_iter()
            .flatten()
            .copied()
            .chain(gaps.iter().filter(|g| g.0 == j as isize).map(|g| g.1));
        let index = NearestPath::new(paths, outer, 2.0 * pitch);
        for c in inner {
            let v = paths[c].at(0);
            if let Some(p) = index.nearest(Point::new(v.x, v.y)) {
                pairs.push((p, c));
            }
        }
    }
    pairs
}

/// The segments of some paths bucketed by a grid, for finding the path nearest a point.
struct NearestPath<'a> {
    paths: &'a [Pline],
    members: &'a [usize],
    cell: f64,
    grid: std::collections::HashMap<(i64, i64), Vec<(usize, usize)>>,
}

impl<'a> NearestPath<'a> {
    fn new(paths: &'a [Pline], members: &'a [usize], cell: f64) -> NearestPath<'a> {
        let mut grid: std::collections::HashMap<(i64, i64), Vec<(usize, usize)>> =
            std::collections::HashMap::new();
        let key = |x: f64| (x / cell).floor() as i64;
        for &m in members {
            let p = &paths[m];
            let n = p.vertex_count();
            let segments = if p.is_closed() {
                n
            } else {
                n.saturating_sub(1)
            };
            for i in 0..segments {
                let b = cavalier_contours::polyline::seg_fast_approx_bounding_box(
                    p.at(i),
                    p.at((i + 1) % n),
                );
                for x in key(b.min_x)..=key(b.max_x) {
                    for y in key(b.min_y)..=key(b.max_y) {
                        grid.entry((x, y)).or_default().push((m, i));
                    }
                }
            }
        }
        NearestPath {
            paths,
            members,
            cell,
            grid,
        }
    }

    /// The point of segment `i` of member `m` nearest `q`, and its distance.
    fn closest(&self, (m, i): (usize, usize), q: Point) -> (Point, f64) {
        let p = &self.paths[m];
        let c = cavalier_contours::polyline::seg_closest_point(
            p.at(i),
            p.at((i + 1) % p.vertex_count()),
            cavalier_contours::core::math::Vector2::new(q.x, q.y),
            1e-9,
        );
        (Point::new(c.x, c.y), (c.x - q.x).hypot(c.y - q.y))
    }

    /// The member nearest `q`.
    fn nearest(&self, q: Point) -> Option<usize> {
        self.nearest_point(q).map(|n| n.0)
    }

    /// The member nearest `q`, the segment and point of it nearest `q`, and the distance:
    /// among the segments in the cells around `q` (all of them when none is there, which a
    /// ring a pitch away never causes).
    fn nearest_point(&self, q: Point) -> Option<(usize, usize, Point, f64)> {
        let (kx, ky) = (
            (q.x / self.cell).floor() as i64,
            (q.y / self.cell).floor() as i64,
        );
        let near = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dy| (kx + dx, ky + dy)))
            .filter_map(|k| self.grid.get(&k))
            .flatten()
            .copied();
        let best = |segments: &mut dyn Iterator<Item = (usize, usize)>| {
            segments
                .map(|s| {
                    let (at, d) = self.closest(s, q);
                    (s.0, s.1, at, d)
                })
                .min_by(|a, b| a.3.total_cmp(&b.3).then((a.0, a.1).cmp(&(b.0, b.1))))
        };
        best(&mut near.clone()).or_else(|| {
            best(&mut self.members.iter().flat_map(|&m| {
                let p = &self.paths[m];
                let segments = if p.is_closed() {
                    p.vertex_count()
                } else {
                    p.vertex_count().saturating_sub(1)
                };
                (0..segments).map(move |i| (m, i))
            }))
        })
    }
}

/// The parts of `region` its rings leave unswept: the cores gap fill draws centrelines in.
///
/// Ring `j` lies at depth `d_j = first + j * pitch` and `levels[j]` is the area inside it
/// (`region` shrunk by `d_j`). Each ring sweeps half a pitch to either side, except that the
/// last ring of any part is credited only `end_spacing` beyond itself. So the points deeper
/// than `d_j + end_spacing` are missed unless ring `j + 1` passes within half a pitch, that
/// is, unless they lie within `pitch - end_spacing` of `levels[j + 1]`. Where the region is
/// wide that distance reaches back to the swept band; in necks and fingers that run out of
/// rings before the rest of the island, and in sharp corners, it does not, and what is left
/// is a core at that depth. The innermost level has no next ring: its core is everything
/// deeper than `d_last + end_spacing`; parts too thin for the first ring are cores before it.
///
/// A round sweep also misses a sliver at every convex corner, `(tan(φ/2) - φ/2) r²` for a
/// turn of `φ`. Corners of 60° or wider (a mitre at most twice the sweep's reach) count as
/// swept, as if the next ring's sweep were mitred there: a centreline in such a sliver
/// would be shorter than about a pitch and is not worth a stroke. Sharper corners are cores.
/// (The audit, which strokes the paths themselves, still reports these corner slivers.)
fn cores(
    region: &Region,
    levels: &[Region],
    first: f64,
    pitch: f64,
    end_spacing: f64,
) -> Vec<(isize, Region)> {
    // Grows the next level slightly more than it must, so that its boundary passes outside
    // the coinciding stretches of the deeper boundary instead of onto them.
    let margin = pitch * 1e-3;
    let mut out = Vec::new();
    for j in -1..levels.len() as isize {
        let deeper = if j < 0 {
            (first - end_spacing).max(0.0)
        } else {
            first + j as f64 * pitch + end_spacing
        };
        let next = levels.get((j + 1) as usize);
        let reach = first + (j + 1) as f64 * pitch - deeper;
        if next.is_some() && reach <= 0.0 {
            continue; // the next ring is no deeper than where its core would start
        }
        let kites = next.map(|next| Kites::new(next, reach + margin));
        // Cores only form where something collapses between the two rings: an edge or arc
        // vanishes, a loop splits or disappears. Inner offsets of a boundary whose reflex
        // corners are already rounded add no vertices otherwise, so two rings with the
        // same number of loops and of vertices have no core between them, except in a
        // corner sharper than the kites take.
        if j >= 0
            && let (Some(next), Some(kites)) = (next, &kites)
            && !kites.sharp
            && same_structure(&levels[j as usize], next)
        {
            continue;
        }
        let inner = region.offset(-deeper);
        if inner.is_empty() {
            break; // every later level is deeper still
        }
        let core = match (next, kites) {
            (Some(next), Some(kites)) => {
                let missed = inner.difference(&next.offset(reach + margin));
                Region {
                    incomplete: missed.incomplete,
                    loops: missed
                        .islands()
                        .into_iter()
                        .filter(|island| !kites.covers(island))
                        .flat_map(|island| island.loops)
                        .collect(),
                }
            }
            _ => inner,
        };
        if !core.is_empty() {
            out.push((j, core));
        }
    }
    out
}

/// Same number of loops and of vertices.
fn same_structure(a: &Region, b: &Region) -> bool {
    let vertices = |r: &Region| r.loops.iter().map(|l| l.vertex_count()).sum::<usize>();
    a.loops.len() == b.loops.len() && vertices(a) == vertices(b)
}

/// The mitre kites of a region grown by `r`: for each convex corner whose mitre reaches at
/// most twice `r` (a turn of at most 120°), the quadrilateral between the corner, the ends
/// of the round join and the mitre tip (from the tangents, for arcs). Bucketed by a grid.
struct Kites {
    kites: Vec<[Point; 4]>,
    cell: f64,
    grid: std::collections::HashMap<(i64, i64), Vec<usize>>,
    /// Some convex corner turns by more than the limit allows (sharper than 60°).
    sharp: bool,
}

impl Kites {
    /// Largest mitre length, as a multiple of the grown distance.
    const LIMIT: f64 = 2.0;

    fn new(region: &Region, r: f64) -> Kites {
        let mut kites = Vec::new();
        let mut sharp = false;
        for l in &region.loops {
            let n = l.vertex_count();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let (p, c, q) = (l.at((i + n - 1) % n), l.at(i), l.at((i + 1) % n));
                let unit = |t: cavalier_contours::core::math::Vector2<f64>| {
                    let h = t.x.hypot(t.y);
                    (h > 0.0).then(|| (t.x / h, t.y / h))
                };
                let (Some(t_in), Some(t_out)) = (
                    unit(cavalier_contours::polyline::seg_tangent_vector(
                        p,
                        c,
                        c.pos(),
                    )),
                    unit(cavalier_contours::polyline::seg_tangent_vector(
                        c,
                        q,
                        c.pos(),
                    )),
                ) else {
                    continue;
                };
                // Filled area on the left: a convex corner turns left.
                if t_in.0 * t_out.1 - t_in.1 * t_out.0 <= 1e-12 {
                    continue;
                }
                let cos_half = ((1.0 + t_in.0 * t_out.0 + t_in.1 * t_out.1) / 2.0).sqrt();
                if cos_half * Self::LIMIT < 1.0 {
                    sharp = true;
                    continue;
                }
                let (n_in, n_out) = ((t_in.1, -t_in.0), (t_out.1, -t_out.0));
                let Some(u) = unit(cavalier_contours::core::math::Vector2::new(
                    n_in.0 + n_out.0,
                    n_in.1 + n_out.1,
                )) else {
                    continue;
                };
                let reach = r / cos_half;
                kites.push([
                    Point::new(c.x, c.y),
                    Point::new(c.x + r * n_in.0, c.y + r * n_in.1),
                    Point::new(c.x + reach * u.0, c.y + reach * u.1),
                    Point::new(c.x + r * n_out.0, c.y + r * n_out.1),
                ]);
            }
        }
        let cell = (Self::LIMIT * r).max(1e-9);
        let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
            std::collections::HashMap::new();
        for (k, kite) in kites.iter().enumerate() {
            let key = (
                (kite[0].x / cell).floor() as i64,
                (kite[0].y / cell).floor() as i64,
            );
            grid.entry(key).or_default().push(k);
        }
        Kites {
            kites,
            cell,
            grid,
            sharp,
        }
    }

    /// Whether `island` lies within one kite (its vertices and arc midpoints do).
    fn covers(&self, island: &Region) -> bool {
        let Some(first) = island.loops.first().map(|l| l.at(0)) else {
            return true;
        };
        let (kx, ky) = (
            (first.x / self.cell).floor() as i64,
            (first.y / self.cell).floor() as i64,
        );
        let points: Vec<Point> = island
            .loops
            .iter()
            .flat_map(|l| {
                l.iter_segments().flat_map(|(a, b)| {
                    let mid = cavalier_contours::polyline::seg_midpoint(a, b);
                    [Point::new(a.x, a.y), Point::new(mid.x, mid.y)]
                })
            })
            .collect();
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                self.grid.get(&(kx + dx, ky + dy)).is_some_and(|ks| {
                    ks.iter()
                        .any(|&k| points.iter().all(|p| in_convex(&self.kites[k], *p)))
                })
            })
        })
    }
}

/// Whether `p` lies in the convex quadrilateral `q` (counter-clockwise), within rounding.
fn in_convex(q: &[Point; 4], p: Point) -> bool {
    (0..4).all(|i| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        cross >= -1e-12 * (1.0 + (b.x - a.x).hypot(b.y - a.y))
    })
}

/// Contour fill parameters (see [`Pattern::ContourFill`]).
#[derive(Clone, Copy, Debug)]
pub struct ContourParams {
    pub pitch_mm: f64,
    pub first_offset_mm: Option<f64>,
    pub gap_fill: bool,
    pub end_spacing_mm: Option<f64>,
    pub simplify_mm: Option<f64>,
    pub ring_order: RingOrder,
    pub link: bool,
}

fn contour_fill(geometry: &Geometry, params: &ContourParams) -> PatternOutput {
    let ContourParams {
        pitch_mm,
        first_offset_mm,
        gap_fill,
        end_spacing_mm,
        simplify_mm,
        ring_order,
        link,
    } = *params;
    let mut out = PatternOutput::default();
    let Geometry::Region(region) = geometry else {
        out.diagnostics.push(Diagnostic::error(
            "pattern.needs_region",
            "contour fill needs a region layer; this layer is linework (expand strokes or choose passthrough)",
        ));
        return out;
    };
    if pitch_mm.is_nan() || pitch_mm <= 0.0 {
        out.diagnostics.push(Diagnostic::error(
            "pattern.pitch",
            "contour fill pitch must be positive",
        ));
        return out;
    }
    let end_spacing = end_spacing_mm.unwrap_or(pitch_mm / 2.0);
    if end_spacing.is_nan() || end_spacing < 0.0 {
        out.diagnostics.push(Diagnostic::error(
            "pattern.end_spacing",
            "contour fill end spacing must not be negative",
        ));
        return out;
    }
    // Simplification may move a ring by at most the tolerance; a quarter pitch keeps
    // neighbouring rings from touching.
    let simplify = simplify_mm.filter(|t| *t > 0.0).map(|t| {
        if t > pitch_mm / 4.0 {
            out.diagnostics.push(Diagnostic::warning(
                "pattern.simplify_clamped",
                format!(
                    "simplify tolerance {t} mm was clamped to a quarter of the pitch ({} mm)",
                    pitch_mm / 4.0
                ),
            ));
        }
        t.min(pitch_mm / 4.0)
    });
    let settings = FillSettings {
        pitch: pitch_mm,
        first: first_offset_mm.unwrap_or(pitch_mm / 2.0),
        end_spacing,
        simplify,
        gap_fill,
        order: ring_order,
        link,
    };
    // Islands are filled independently (in parallel where a thread pool exists): every
    // offset then only sees one island's loops, so the work grows linearly with their
    // number instead of offsetting the whole layer at every depth.
    let islands = region.islands();
    let count = islands.len().max(1) as f64;
    let filled = std::sync::atomic::AtomicUsize::new(0);
    let fills = crate::par::map(islands, |island| {
        let fill = fill_island(&island, &settings);
        let n = filled.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        crate::cancel::step(n as f64 / count);
        fill
    });
    let mut skipped = 0;
    for f in fills {
        let base = out.paths.len();
        out.precedence
            .extend(f.precedence.iter().map(|(a, b)| (base + a, base + b)));
        out.directed.extend(f.directed.iter().map(|d| base + d));
        out.paths.extend(f.paths);
        out.stats.rings += f.rings;
        out.stats.gap_fills += f.gap_fills;
        skipped += f.skipped;
    }
    if skipped > 0 {
        out.diagnostics.push(Diagnostic::warning(
            "pattern.gap_fill_skipped",
            format!("{skipped} uncovered sliver(s) were too thin or degenerate for a centreline"),
        ));
    }
    out
}

/// Area of `region` left unswept by `paths` when each is assumed `width` wide: the "missed
/// area" audit of the spec. A geometric surrogate, not an exposure model.
pub fn missed_area(region: &Region, paths: &[Pline], width: f64) -> f64 {
    gapfill::uncovered(region, paths, width).area().max(0.0)
}

/// Coverage audit of a fill (spec §10): its paths swept at an assumed effective marking
/// width, compared with the region they are meant to fill. A geometric surrogate set by the
/// user, not an exposure or cutting model.
#[derive(Clone, Debug)]
pub struct Audit {
    pub width_mm: f64,
    pub region_mm2: f64,
    /// Part of the region no path sweeps.
    pub missed: Region,
    /// Swept area outside the region.
    pub spill: Region,
    /// Paths shorter than the width: marks that are dots rather than strokes.
    pub short_segments: usize,
}

pub fn audit(region: &Region, paths: &[Pline], width: f64) -> Audit {
    let swept = crate::region::stroke_many(paths, width);
    Audit {
        width_mm: width,
        region_mm2: region.area(),
        missed: region.difference(&swept),
        spill: swept.difference(region),
        short_segments: paths
            .iter()
            .filter(|p| crate::pline::length(p) < width)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pline::{circle, rect};
    use crate::region::LoopRule;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    fn rect_region(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        Region::from_loops(
            [rect(Point::new(x0, y0), Point::new(x1, y1))],
            LoopRule::NonZero,
        )
    }

    fn contour(pitch: f64, gap_fill: bool, region: &Region) -> PatternOutput {
        ordered_contour(pitch, gap_fill, RingOrder::Any, region)
    }

    fn ordered_contour(
        pitch: f64,
        gap_fill: bool,
        ring_order: RingOrder,
        region: &Region,
    ) -> PatternOutput {
        generate(
            &Pattern::ContourFill {
                pitch_mm: pitch,
                first_offset_mm: None,
                gap_fill,
                end_spacing_mm: None,
                simplify_mm: None,
                ring_order,
                link: false,
            },
            &Geometry::Region(region.clone()),
        )
    }

    /// Distance from the start of `inner` to the nearest point of `outer`.
    fn gap_between(inner: &Pline, outer: &Pline) -> f64 {
        let v = inner.at(0);
        let c = outer
            .closest_point(cavalier_contours::core::math::Vector2::new(v.x, v.y), 1e-9)
            .unwrap();
        c.distance
    }

    #[test]
    fn ring_order_ties_every_ring_to_the_ring_outside_it() {
        // A square with a round hole and a thin finger (a neck core at depth): every path
        // but the first rings has the ring just outside it as its partner, a pitch away
        // (a gap-fill centreline at most a pitch).
        let (finger_shape, _) = finger(0.45);
        let shape = finger_shape.difference(&Region::from_loops(
            [circle(Point::new(2.0, 2.0), 0.6)],
            LoopRule::NonZero,
        ));
        let any = ordered_contour(0.1, true, RingOrder::Any, &shape);
        assert!(any.precedence.is_empty());
        let inward = ordered_contour(0.1, true, RingOrder::Inward, &shape);
        let outward = ordered_contour(0.1, true, RingOrder::Outward, &shape);
        let n = inward.paths.len();
        assert_eq!(n, any.paths.len());
        assert!(inward.stats.gap_fills > 0);
        // Two first rings (outer and hole) have no partner; everything else has one.
        assert_eq!(inward.precedence.len(), n - 2);
        let mut seen = vec![false; n];
        for &(outer, inner) in &inward.precedence {
            assert!(!seen[inner], "path {inner} has two partners");
            seen[inner] = true;
            // The generated order already keeps the constraint.
            assert!(outer < inner);
            let d = gap_between(&inward.paths[inner], &inward.paths[outer]);
            if inward.paths[inner].is_closed() {
                assert!(
                    (d - 0.1).abs() < 1e-6,
                    "ring {inner} is {d} from its partner"
                );
            } else {
                assert!(
                    d <= 0.1 + 1e-6,
                    "centreline {inner} is {d} from its partner"
                );
            }
        }
        // Outward: the same paths in reverse, each before the ring outside it.
        assert_eq!(outward.paths.len(), n);
        for (k, p) in outward.paths.iter().enumerate() {
            assert_eq!(p.vertex_data, inward.paths[n - 1 - k].vertex_data);
        }
        let flipped: std::collections::HashSet<(usize, usize)> = inward
            .precedence
            .iter()
            .map(|&(outer, inner)| (n - 1 - inner, n - 1 - outer))
            .collect();
        assert_eq!(
            outward
                .precedence
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>(),
            flipped
        );
        for &(a, b) in &outward.precedence {
            assert!(a < b);
        }
    }

    /// Two 4 mm squares joined by an 8 mm neck `w` wide, and the neck's middle 4 mm.
    fn dumbbell(w: f64) -> (Region, Region) {
        let (lo, hi) = (2.0 - w / 2.0, 2.0 + w / 2.0);
        let shape = rect_region(0.0, 0.0, 4.0, 4.0)
            .union(&rect_region(10.0, 0.0, 14.0, 4.0))
            .union(&rect_region(3.0, lo, 11.0, hi));
        (shape, rect_region(5.0, lo, 9.0, hi))
    }

    /// A 4 mm square with a 5 mm finger `w` wide, and the finger's middle 3 mm.
    fn finger(w: f64) -> (Region, Region) {
        let (lo, hi) = (2.0 - w / 2.0, 2.0 + w / 2.0);
        let shape = rect_region(0.0, 0.0, 4.0, 4.0).union(&rect_region(3.0, lo, 9.0, hi));
        (shape, rect_region(5.0, lo, 8.0, hi))
    }

    #[test]
    fn gap_fill_reaches_necks_that_run_out_of_rings_before_the_island() {
        // DEF-15. At pitch 0.1 the rings at depths 0.05 and 0.15 run through the 0.45 mm
        // neck and the third does not; the 0.05 mm core between them stayed unmarked, as
        // gap fill only looked past the squares' last ring.
        let (shape, neck) = dumbbell(0.45);
        let plain = contour(0.1, false, &shape);
        let filled = contour(0.1, true, &shape);
        assert!((missed_area(&neck, &plain.paths, 0.1) - 0.2).abs() < 1e-3);
        let missed = missed_area(&neck, &filled.paths, 0.1);
        assert!(missed < 1e-6, "neck missed {missed}");
    }

    #[test]
    fn a_ring_at_exactly_half_a_width_marks_the_centreline_once() {
        // A 0.3 mm neck at pitch 0.1: the ring at depth 0.15 collapses onto the neck's
        // centreline, so it is two square rings and one open path along the neck.
        let (shape, neck) = dumbbell(0.3);
        let out = contour(0.1, false, &shape);
        for p in out.paths.iter().filter(|p| p.is_closed()) {
            assert!(
                crate::pline::split_retraced(p).1.is_empty(),
                "a ring retraces itself"
            );
        }
        let open: Vec<&Pline> = out.paths.iter().filter(|p| !p.is_closed()).collect();
        assert_eq!(open.len(), 1);
        let b = crate::pline::bounds(open[0]).unwrap();
        assert!((b.min.y - 2.0).abs() < 1e-9 && (b.max.y - 2.0).abs() < 1e-9);
        assert!(b.min.x <= 4.0 + 1e-9 && b.max.x >= 10.0 - 1e-9, "{b:?}");
        // The audit sweeps it like any other path: nothing in the neck is missed.
        let missed = missed_area(&neck, &out.paths, 0.1);
        assert!(missed < 1e-9, "neck missed {missed}");
    }

    #[test]
    fn gap_fill_leaves_right_angles_to_the_rings_and_fills_sharp_corners() {
        // A 3.06 x 5 mm rectangle: the corners turn by 90°, so the only gap fill is the
        // 0.06 mm core past the last ring.
        let block = rect_region(0.0, 0.0, 3.06, 5.0);
        let out = contour(0.1, true, &block);
        assert_eq!(out.stats.gap_fills, 1);
        // A 20° spike: every ring leaves a sliver ahead of its tip, too long to sweep.
        let spike = Region::from_loops(
            [crate::pline::from_points(
                [
                    Point::new(0.0, 0.0),
                    Point::new(10.0, -1.76),
                    Point::new(10.0, 1.76),
                ],
                true,
            )],
            LoopRule::NonZero,
        );
        let out = contour(0.1, true, &spike);
        let near_tip = out.paths[out.stats.rings..]
            .iter()
            .filter(|p| crate::pline::bounds(p).unwrap().max.x < 5.0)
            .count();
        assert!(near_tip >= 3, "{near_tip} centrelines near the tip");
    }

    fn linked_contour(pitch: f64, ring_order: RingOrder, region: &Region) -> PatternOutput {
        generate(
            &Pattern::ContourFill {
                pitch_mm: pitch,
                first_offset_mm: None,
                gap_fill: true,
                end_spacing_mm: None,
                simplify_mm: None,
                ring_order,
                link: true,
            },
            &Geometry::Region(region.clone()),
        )
    }

    #[test]
    fn linked_rings_of_a_square_are_one_path() {
        // Ten nested rings, 0.2 mm apart: one path through all of them, nine connectors
        // a pitch long, entering each ring where it left the one before.
        let square = rect_region(0.0, 0.0, 4.0, 4.0);
        let plain = contour(0.2, false, &square);
        let rings: f64 = plain.paths.iter().map(crate::pline::length).sum();
        let out = linked_contour(0.2, RingOrder::Inward, &square);
        let chain = &out.paths[0];
        assert!(!chain.is_closed());
        assert_eq!(out.directed, vec![0]);
        assert_eq!(out.stats.rings, 10);
        assert!((crate::pline::length(chain) - (rings + 9.0 * 0.2)).abs() < 1e-9);
        // Inward: it starts on the outer ring and ends on the innermost.
        assert!(
            square.offset(-0.1).loops[0]
                .closest_point(chain.at(0).pos(), 1e-9)
                .unwrap()
                .distance
                < 1e-9
        );
        let end = chain.last().unwrap().pos();
        assert!(
            square.offset(-1.9).loops[0]
                .closest_point(end, 1e-9)
                .unwrap()
                .distance
                < 1e-9
        );
        // Outward: the same path backwards; any order: not directed.
        let outward = linked_contour(0.2, RingOrder::Outward, &square);
        assert!(outward.paths[outward.directed[0]].at(0).pos().fuzzy_eq(end));
        assert!(
            linked_contour(0.2, RingOrder::Any, &square)
                .directed
                .is_empty()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn linked_rings_stay_inside_and_add_a_pitch_per_join(
            x in -5.0..5.0f64, y in -5.0..5.0f64, w in 1.0..6.0f64, h in 1.0..6.0f64,
            hole in 0.1..0.4f64, pitch in 0.08..0.3f64, outward in any::<bool>(),
        ) {
            let r = hole * w.min(h);
            let region = rect_region(x, y, x + w, y + h).difference(&Region::from_loops(
                [circle(Point::new(x + w / 3.0, y + h / 2.0), r)],
                LoopRule::NonZero,
            ));
            let order = if outward { RingOrder::Outward } else { RingOrder::Inward };
            let plain = ordered_contour(pitch, true, order, &region);
            let out = linked_contour(pitch, order, &region);
            let length = |ps: &[Pline]| ps.iter().map(crate::pline::length).sum::<f64>();
            let joins = plain.paths.len() - out.paths.len();
            prop_assert!(
                (length(&out.paths) - length(&plain.paths) - joins as f64 * pitch).abs() < 1e-6,
                "{} joins: {} vs {}", joins, length(&out.paths), length(&plain.paths)
            );
            let grown = region.offset(1e-6);
            for p in &out.paths {
                for (a, b) in p.iter_segments() {
                    let m = cavalier_contours::polyline::seg_midpoint(a, b);
                    prop_assert!(grown.contains(Point::new(m.x, m.y)), "segment outside at {:?}", m);
                }
            }
            for &(a, b) in &out.precedence {
                prop_assert!(a < b, "generated order breaks {:?}", (a, b));
            }
            for &d in &out.directed {
                prop_assert!(!out.paths[d].is_closed());
            }
        }

        #[test]
        fn necks_and_fingers_are_swept_end_to_end(
            w in 0.15..1.5f64,
            pitch in 0.04..0.2f64,
            is_finger in any::<bool>(),
        ) {
            let (shape, middle) = if is_finger { finger(w) } else { dumbbell(w) };
            let out = contour(pitch, true, &shape);
            let missed = missed_area(&middle, &out.paths, pitch);
            prop_assert!(missed <= 1e-6 * middle.area().max(1.0), "missed {} of {}", missed, middle.area());
        }
    }

    #[test]
    fn passthrough_keeps_paths_and_reverses() {
        let mut open = Pline::new();
        open.add(0.0, 0.0, 0.0);
        open.add(1.0, 0.0, 0.0);
        open.add(2.0, 0.0, 0.0);
        let g = Geometry::Linework(vec![open]);
        let out = generate(
            &Pattern::Passthrough {
                simplify_mm: Some(1e-6),
                reverse: true,
            },
            &g,
        );
        assert_eq!(out.paths.len(), 1);
        assert_eq!(
            out.paths[0].vertex_count(),
            2,
            "collinear middle vertex removed"
        );
        assert_eq!(out.paths[0].at(0).x, 2.0, "reversed");
    }

    #[test]
    fn contour_fill_rings_shrink_and_keep_holes() {
        let outer = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(20.0, 20.0))],
            LoopRule::NonZero,
        );
        let hole = Region::from_loops([circle(Point::new(10.0, 10.0), 3.0)], LoopRule::NonZero);
        let region = outer.difference(&hole);
        let out = generate(
            &Pattern::ContourFill {
                pitch_mm: 1.0,
                first_offset_mm: None,
                gap_fill: false,
                end_spacing_mm: None,
                simplify_mm: None,
                ring_order: RingOrder::Any,
                link: false,
            },
            &Geometry::Region(region.clone()),
        );
        assert!(out.diagnostics.is_empty());
        // Depths 0.5, 1.5, 2.5 give an outer and a hole loop each (6); at 3.5 the grown hole
        // is tangent to the shrunk outer; at 4.5 four corner pieces remain; at 5.5 nothing.
        assert!(out.stats.rings >= 10, "rings {}", out.stats.rings);
        assert_eq!(
            out.paths.iter().filter(|p| p.area().abs() > 100.0).count(),
            3,
            "three outer squares"
        );
        for p in &out.paths {
            assert!(p.is_closed());
        }
        // The first outer ring is the 19x19 square, the first hole ring a circle of r 3.5.
        let areas: Vec<f64> = out.paths.iter().map(|p| p.area()).collect();
        assert!(areas.iter().any(|a| (a - 361.0).abs() < 1e-9));
        assert!(
            areas
                .iter()
                .any(|a| (a + std::f64::consts::PI * 3.5 * 3.5).abs() < 1e-9)
        );
    }

    #[test]
    fn islands_fill_identically_in_parallel_and_in_sequence() {
        // A grid of islands with holes: the per-island parallel fill must give exactly the
        // paths (and order) of the sequential one, as runs must be deterministic.
        let loops = (0..4).flat_map(|i| {
            (0..3).flat_map(move |j| {
                let (x, y) = (i as f64 * 12.0, j as f64 * 12.0);
                [
                    rect(Point::new(x, y), Point::new(x + 10.0, y + 8.0)),
                    circle(Point::new(x + 5.0, y + 4.0), 1.5),
                ]
            })
        });
        let region = Region::from_loops(loops, LoopRule::EvenOdd);
        assert_eq!(region.islands().len(), 12);
        let pattern = Pattern::ContourFill {
            pitch_mm: 0.25,
            first_offset_mm: None,
            gap_fill: true,
            end_spacing_mm: None,
            simplify_mm: None,
            ring_order: RingOrder::Any,
            link: false,
        };
        let geometry = Geometry::Region(region);
        let was = crate::par::enabled();
        crate::par::enable(false);
        let sequential = generate(&pattern, &geometry);
        crate::par::enable(true);
        let parallel = generate(&pattern, &geometry);
        crate::par::enable(was);
        let bits = |o: &PatternOutput| -> Vec<Vec<[u64; 3]>> {
            o.paths
                .iter()
                .map(|p| {
                    p.vertex_data
                        .iter()
                        .map(|v| [v.x.to_bits(), v.y.to_bits(), v.bulge.to_bits()])
                        .collect()
                })
                .collect()
        };
        assert!(!sequential.paths.is_empty());
        assert_eq!(bits(&sequential), bits(&parallel));
        assert_eq!(sequential.stats.rings, parallel.stats.rings);
    }

    #[test]
    fn audit_reports_missed_area_spill_and_short_segments() {
        // A 10 x 4 mm bar filled with rings 0.2 mm apart, the first 0.1 mm inside.
        let bar = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(10.0, 4.0))],
            LoopRule::NonZero,
        );
        let fill = generate(
            &Pattern::ContourFill {
                pitch_mm: 0.2,
                first_offset_mm: None,
                gap_fill: true,
                end_spacing_mm: None,
                simplify_mm: None,
                ring_order: RingOrder::Any,
                link: false,
            },
            &Geometry::Region(bar.clone()),
        );
        // Swept exactly at the pitch: the rings tile the bar and nothing spills. Each ring's
        // stroke rounds its outer corners while the ring outside it has sharp inner ones,
        // so every ring leaves (1 - pi/4) r^2 unswept at each of the four corners.
        let exact = audit(&bar, &fill.paths, 0.2);
        assert!((exact.region_mm2 - 40.0).abs() < 1e-9);
        let corners = fill.stats.rings as f64 * 4.0 * (1.0 - PI / 4.0) * 0.1 * 0.1;
        assert!(
            (exact.missed.area() - corners).abs() < 1e-3,
            "missed {}",
            exact.missed.area()
        );
        assert!(exact.spill.area() < 1e-6, "spill {}", exact.spill.area());
        // Wider than the pitch: the outer ring reaches 0.05 mm beyond the 28 mm perimeter.
        let wide = audit(&bar, &fill.paths, 0.3);
        assert!(
            (wide.spill.area() - 28.0 * 0.05).abs() < 0.02,
            "spill {}",
            wide.spill.area()
        );
        // Half the pitch: about half the bar stays unswept.
        let narrow = audit(&bar, &fill.paths, 0.1);
        assert!(
            (narrow.missed.area() / 40.0 - 0.5).abs() < 0.05,
            "missed {}",
            narrow.missed.area()
        );
        // Paths shorter than the width count as short segments.
        let dot = crate::pline::from_points([Point::new(1.0, 1.0), Point::new(1.05, 1.0)], false);
        assert_eq!(audit(&bar, &[dot], 0.1).short_segments, 1);
        assert_eq!(exact.short_segments, 0);
    }

    #[test]
    fn gap_fill_reduces_missed_area_without_touching_rings() {
        // A 20 mm bar 0.45 mm wide: one ring at 0.05, nothing at 0.15 -> a core gap remains.
        let bar = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(20.0, 0.45))],
            LoopRule::NonZero,
        );
        let pattern = |gap| Pattern::ContourFill {
            pitch_mm: 0.1,
            first_offset_mm: None,
            gap_fill: gap,
            end_spacing_mm: None,
            simplify_mm: None,
            ring_order: RingOrder::Any,
            link: false,
        };
        let plain = generate(&pattern(false), &Geometry::Region(bar.clone()));
        let filled = generate(&pattern(true), &Geometry::Region(bar.clone()));
        assert!(filled.stats.gap_fills >= 1);
        assert_eq!(
            filled.paths.len(),
            plain.paths.len() + filled.stats.gap_fills
        );
        for (a, b) in plain.paths.iter().zip(filled.paths.iter()) {
            assert_eq!(a.vertex_count(), b.vertex_count(), "rings unchanged");
        }
        let missed_plain = missed_area(&bar, &plain.paths, 0.1);
        let missed_filled = missed_area(&bar, &filled.paths, 0.1);
        assert!(missed_plain > 0.5, "plain misses {missed_plain}");
        assert!(
            missed_filled < 0.1 * missed_plain,
            "filled misses {missed_filled} vs {missed_plain}"
        );
    }

    #[test]
    fn crosshatch_is_independent_passes_and_boundary_hatch_stays_inside() {
        let region = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0))],
            LoopRule::NonZero,
        )
        .difference(&Region::from_loops(
            [circle(Point::new(5.0, 5.0), 2.0)],
            LoopRule::NonZero,
        ));
        let one = |angle: f64| {
            generate(
                &Pattern::Hatch {
                    pitch_mm: 0.5,
                    angle_deg: angle,
                    phase_mm: 0.0,
                    serpentine: true,
                    inset_mm: 0.0,
                },
                &Geometry::Region(region.clone()),
            )
            .paths
            .len()
        };
        let cross = generate(
            &Pattern::CrossHatch {
                pitch_mm: 0.5,
                angles_deg: vec![0.0, 90.0],
                phase_mm: 0.0,
                serpentine: true,
                inset_mm: 0.0,
            },
            &Geometry::Region(region.clone()),
        );
        assert_eq!(cross.paths.len(), one(0.0) + one(90.0));

        let bh = generate(
            &Pattern::BoundaryHatch {
                shells: 2,
                shell_pitch_mm: 0.3,
                inset_mm: 0.1,
                pitch_mm: 0.5,
                angles_deg: vec![45.0],
                phase_mm: 0.0,
                serpentine: true,
            },
            &Geometry::Region(region.clone()),
        );
        assert_eq!(
            bh.stats.rings, 4,
            "two shells, each an outer and a hole loop"
        );
        let grown = region.offset(1e-6);
        for p in &bh.paths {
            for v in p.iter_vertexes() {
                assert!(
                    grown.contains(Point::new(v.x, v.y)),
                    "vertex outside the target region"
                );
            }
        }
    }

    #[test]
    fn contour_fill_rejects_linework() {
        let out = generate(
            &Pattern::ContourFill {
                pitch_mm: 1.0,
                first_offset_mm: None,
                gap_fill: false,
                end_spacing_mm: None,
                simplify_mm: None,
                ring_order: RingOrder::Any,
                link: false,
            },
            &Geometry::Linework(vec![]),
        );
        assert_eq!(out.diagnostics[0].code, "pattern.needs_region");
    }

    #[test]
    fn end_spacing_controls_where_gap_fill_starts_and_simplify_thins_rings() {
        // A 10 x 3.3 mm strip at pitch 1: rings at depths 0.5 and 1.5; the core deeper than
        // 1.5 + end spacing is 0.3 mm wide with the default 0.5 end spacing (gap fill), and
        // empty with an end spacing of 0.7 (no gap fill).
        let strip = Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(10.0, 4.3))],
            LoopRule::NonZero,
        );
        let fill = |end: Option<f64>| {
            generate(
                &Pattern::ContourFill {
                    pitch_mm: 1.0,
                    first_offset_mm: None,
                    gap_fill: true,
                    end_spacing_mm: end,
                    simplify_mm: None,
                    ring_order: RingOrder::Any,
                    link: false,
                },
                &Geometry::Region(strip.clone()),
            )
        };
        let default = fill(None);
        assert_eq!(default.stats.rings, 2);
        assert_eq!(default.stats.gap_fills, 1, "{:?}", default.diagnostics);
        let wide = fill(Some(0.7));
        assert_eq!(wide.stats.rings, 2);
        assert_eq!(wide.stats.gap_fills, 0);
        let neg = fill(Some(-1.0));
        assert!(
            neg.diagnostics
                .iter()
                .any(|d| d.code == "pattern.end_spacing")
        );

        // Simplify: a polygon with a 2 um wiggle on one edge keeps far fewer ring vertices;
        // the tolerance is clamped to a quarter pitch with a warning.
        let mut pts = vec![Point::new(0.0, 0.0)];
        for k in 1..200 {
            pts.push(Point::new(
                k as f64 * 0.05,
                if k % 2 == 0 { 0.002 } else { -0.002 },
            ));
        }
        pts.extend([
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ]);
        let wiggly = Region::from_loops([crate::pline::from_points(pts, true)], LoopRule::NonZero);
        let rings = |simplify: Option<f64>| {
            generate(
                &Pattern::ContourFill {
                    pitch_mm: 1.0,
                    first_offset_mm: None,
                    gap_fill: false,
                    end_spacing_mm: None,
                    simplify_mm: simplify,
                    ring_order: RingOrder::Any,
                    link: false,
                },
                &Geometry::Region(wiggly.clone()),
            )
        };
        let plain = rings(None);
        let simple = rings(Some(0.01));
        let count = |o: &PatternOutput| o.paths.iter().map(|p| p.vertex_count()).sum::<usize>();
        assert!(
            count(&simple) * 3 < count(&plain),
            "{} vs {}",
            count(&simple),
            count(&plain)
        );
        assert_eq!(simple.paths.len(), plain.paths.len());
        let clamped = rings(Some(1.0));
        assert!(
            clamped
                .diagnostics
                .iter()
                .any(|d| d.code == "pattern.simplify_clamped")
        );
    }
}
