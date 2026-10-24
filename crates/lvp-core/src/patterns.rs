//! Pattern passes: turn a working layer's geometry into marking centrelines.
//!
//! Every pattern yields ordered polylines (open or closed) in mm. Output is geometry,
//! never a machine process; cosmetic PDF stroke width and assumed mark width are not
//! inputs here (except where a coverage audit asks for them explicitly).

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
        } => contour_fill(geometry, *pitch_mm, *first_offset_mm, *gap_fill),
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

fn contour_fill(
    geometry: &Geometry,
    pitch_mm: f64,
    first_offset_mm: Option<f64>,
    gap_fill: bool,
) -> PatternOutput {
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
    let first = first_offset_mm.unwrap_or(pitch_mm / 2.0);
    let mut depth = first;
    let mut rings = 0usize;
    loop {
        let ring = region.offset(-depth);
        if ring.is_empty() || rings >= MAX_RINGS {
            break;
        }
        rings += ring.loops.len();
        out.paths.extend(ring.loops);
        depth += pitch_mm;
    }
    out.stats.rings = rings;
    if gap_fill {
        // Centreline finish for cores narrower than a pitch arrives with the medial-axis
        // step; until then the request is recorded, not silently dropped.
        out.diagnostics.push(Diagnostic::info(
            "pattern.gap_fill_pending",
            "gap fill is not implemented yet; rings only",
        ));
    }
    out
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
    fn contour_fill_rejects_linework() {
        let out = generate(
            &Pattern::ContourFill {
                pitch_mm: 1.0,
                first_offset_mm: None,
                gap_fill: false,
            },
            &Geometry::Linework(vec![]),
        );
        assert_eq!(out.diagnostics[0].code, "pattern.needs_region");
    }
}
