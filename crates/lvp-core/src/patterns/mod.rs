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
use crate::model::Pattern;
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
        } => contour_fill(
            geometry,
            &ContourParams {
                pitch_mm: *pitch_mm,
                first_offset_mm: *first_offset_mm,
                gap_fill: *gap_fill,
                end_spacing_mm: *end_spacing_mm,
                simplify_mm: *simplify_mm,
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
                diagnostics: Vec::new(),
                precedence,
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

struct IslandFill {
    paths: Vec<Pline>,
    rings: usize,
    gap_fills: usize,
    skipped: usize,
}

/// Rings of one island from depth `first` inward, then the gap-fill centrelines.
fn fill_island(
    region: &Region,
    pitch_mm: f64,
    first: f64,
    end_spacing: f64,
    simplify: Option<f64>,
    gap_fill: bool,
) -> IslandFill {
    let mut fill = IslandFill {
        paths: Vec::new(),
        rings: 0,
        gap_fills: 0,
        skipped: 0,
    };
    let mut depth = first;
    loop {
        if crate::cancel::cancelled() {
            return fill;
        }
        let ring = region.offset(-depth);
        if ring.is_empty() || fill.rings >= MAX_RINGS {
            break;
        }
        fill.rings += ring.loops.len();
        match simplify {
            Some(t) => fill
                .paths
                .extend(ring.loops.iter().map(|l| crate::pline::simplify(l, t))),
            None => fill.paths.extend(ring.loops),
        }
        depth += pitch_mm;
    }
    if gap_fill {
        // Rings sit at depths first, first + pitch, ...; each sweeps ±pitch/2. The cores they
        // miss are the region deeper than the last ring plus the end spacing (pitch/2 by
        // default), which an offset gives directly (and exactly, for offset-based rings)
        // without stroking thousands of ring segments.
        let covered_depth = if fill.rings == 0 {
            (first - end_spacing).max(0.0)
        } else {
            depth - pitch_mm + end_spacing
        };
        let gaps = region.offset(-covered_depth);
        if !gaps.is_empty() {
            let (lines, skipped) = gapfill::centrelines(
                &gaps,
                &gapfill::GapFillOptions {
                    assumed_width_mm: pitch_mm,
                    flatten_tolerance_mm: (pitch_mm * 0.01).max(1e-5),
                    min_length_mm: pitch_mm / 2.0,
                },
            );
            fill.gap_fills = lines.len();
            fill.paths.extend(lines);
            fill.skipped = skipped;
        }
    }
    fill
}

/// Contour fill parameters (see [`Pattern::ContourFill`]).
#[derive(Clone, Copy, Debug)]
pub struct ContourParams {
    pub pitch_mm: f64,
    pub first_offset_mm: Option<f64>,
    pub gap_fill: bool,
    pub end_spacing_mm: Option<f64>,
    pub simplify_mm: Option<f64>,
}

fn contour_fill(geometry: &Geometry, params: &ContourParams) -> PatternOutput {
    let ContourParams {
        pitch_mm,
        first_offset_mm,
        gap_fill,
        end_spacing_mm,
        simplify_mm,
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
    let first = first_offset_mm.unwrap_or(pitch_mm / 2.0);
    // Islands are filled independently (in parallel where a thread pool exists): every
    // offset then only sees one island's loops, so the work grows linearly with their
    // number instead of offsetting the whole layer at every depth.
    let islands = region.islands();
    let count = islands.len().max(1) as f64;
    let filled = std::sync::atomic::AtomicUsize::new(0);
    let fills = crate::par::map(islands, |island| {
        let fill = fill_island(&island, pitch_mm, first, end_spacing, simplify, gap_fill);
        let n = filled.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        crate::cancel::step(n as f64 / count);
        fill
    });
    let mut skipped = 0;
    for f in fills {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::pline::{circle, rect};
    use crate::region::LoopRule;

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
