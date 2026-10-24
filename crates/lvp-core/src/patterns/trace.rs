//! Boundary tracing: offset rings around a region's outers and holes, with user-entered
//! kerf compensation and cutting order.

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};

use crate::model::OffsetSide;
use crate::pline::Pline;
use crate::region::Region;

#[derive(Clone, Copy, Debug)]
pub struct TraceParams {
    pub side: OffsetSide,
    pub offset_mm: f64,
    pub kerf_mm: f64,
    pub rings: u32,
    pub ring_spacing_mm: f64,
    pub reverse: bool,
    pub inner_first: bool,
}

/// Offsets for each ring, signed relative to the filled region (positive grows).
pub fn ring_offsets(p: &TraceParams) -> Vec<f64> {
    let dir = match p.side {
        OffsetSide::Outside => 1.0,
        // Additional rings on the boundary step inward (engraving outlines); `on` itself is 0.
        OffsetSide::On | OffsetSide::Inside => -1.0,
    };
    let base = p.kerf_mm / 2.0
        + match p.side {
            OffsetSide::On => 0.0,
            _ => dir * p.offset_mm.abs(),
        };
    (0..p.rings.max(1))
        .map(|k| base + dir * k as f64 * p.ring_spacing_mm)
        .collect()
}

pub fn boundary_trace(region: &Region, p: &TraceParams) -> Vec<Pline> {
    boundary_trace_ordered(region, p).0
}

/// Boundary trace plus the ordering constraints `(hole index, outer index)` that a path
/// optimiser must keep when `inner_first` is set: every hole before the island it belongs
/// to. Without `inner_first` there are no constraints.
pub fn boundary_trace_ordered(
    region: &Region,
    p: &TraceParams,
) -> (Vec<Pline>, Vec<(usize, usize)>) {
    let mut out: Vec<Pline> = Vec::new();
    let mut precedence = Vec::new();
    for d in ring_offsets(p) {
        let ring = region.offset(d);
        // Holes (clockwise) first when cutting: inner features before the outer contour.
        let (holes, outers): (Vec<&Pline>, Vec<&Pline>) =
            ring.loops.iter().partition(|l| l.area() < 0.0);
        let base = out.len();
        let ordered: Vec<&Pline> = if p.inner_first {
            holes
                .iter()
                .copied()
                .chain(outers.iter().copied())
                .collect()
        } else {
            outers
                .iter()
                .copied()
                .chain(holes.iter().copied())
                .collect()
        };
        if p.inner_first {
            // Each hole precedes the outer that contains it (the outer with the smallest
            // area among those containing one of its points).
            for (hi, hole) in holes.iter().enumerate() {
                let probe = hole.at(0);
                let probe = crate::geom::Point::new(probe.x, probe.y);
                let mut best: Option<(usize, f64)> = None;
                for (oi, outer) in outers.iter().enumerate() {
                    if crate::region::loop_contains(outer, probe) {
                        let a = outer.area();
                        if best.is_none_or(|(_, ba)| a < ba) {
                            best = Some((oi, a));
                        }
                    }
                }
                if let Some((oi, _)) = best {
                    precedence.push((base + hi, base + holes.len() + oi));
                }
            }
        }
        for l in ordered {
            let mut path = l.clone();
            if p.reverse {
                path.invert_direction_mut();
            }
            out.push(path);
        }
    }
    (out, precedence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::pline::{circle, rect};
    use crate::region::LoopRule;
    use std::f64::consts::PI;

    fn part() -> Region {
        Region::from_loops(
            [rect(Point::new(0.0, 0.0), Point::new(20.0, 10.0))],
            LoopRule::NonZero,
        )
        .difference(&Region::from_loops(
            [circle(Point::new(5.0, 5.0), 2.0)],
            LoopRule::NonZero,
        ))
    }

    fn params(side: OffsetSide, offset: f64, kerf: f64, rings: u32, spacing: f64) -> TraceParams {
        TraceParams {
            side,
            offset_mm: offset,
            kerf_mm: kerf,
            rings,
            ring_spacing_mm: spacing,
            reverse: false,
            inner_first: true,
        }
    }

    #[test]
    fn on_boundary_trace_emits_hole_before_outer() {
        let paths = boundary_trace(&part(), &params(OffsetSide::On, 0.0, 0.0, 1, 0.0));
        assert_eq!(paths.len(), 2);
        assert!(paths[0].area() < 0.0, "hole first");
        assert!((paths[0].area().abs() - PI * 4.0).abs() < 1e-9);
        assert!((paths[1].area() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn kerf_moves_outward_and_shrinks_holes() {
        let paths = boundary_trace(&part(), &params(OffsetSide::On, 0.0, 0.2, 1, 0.0));
        let hole = paths.iter().find(|p| p.area() < 0.0).unwrap();
        let outer = paths.iter().find(|p| p.area() > 0.0).unwrap();
        assert!(
            (hole.area().abs() - PI * 1.9 * 1.9).abs() < 1e-9,
            "hole radius 2 - 0.1"
        );
        assert!(
            outer.area() > 200.0 + 20.0 * 0.1 * 2.0,
            "outer grows by kerf/2"
        );
    }

    #[test]
    fn inside_outside_and_rings() {
        let inside = boundary_trace(&part(), &params(OffsetSide::Inside, 0.5, 0.0, 1, 0.0));
        let outer_in = inside.iter().find(|p| p.area() > 0.0).unwrap();
        assert!((outer_in.area() - 19.0 * 9.0).abs() < 1e-9);
        let outside = boundary_trace(&part(), &params(OffsetSide::Outside, 0.5, 0.0, 3, 0.25));
        assert_eq!(outside.len(), 6, "three rings of outer + hole");
        let outers: Vec<f64> = outside
            .iter()
            .filter(|p| p.area() > 0.0)
            .map(|p| p.area())
            .collect();
        assert!(
            outers[0] < outers[1] && outers[1] < outers[2],
            "rings grow outward"
        );
        let close = |a: &[f64], b: &[f64]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12)
        };
        assert!(close(
            &ring_offsets(&params(OffsetSide::Outside, 0.5, 0.2, 2, 0.3)),
            &[0.6, 0.9]
        ));
        assert!(close(
            &ring_offsets(&params(OffsetSide::Inside, 0.5, 0.0, 2, 0.3)),
            &[-0.5, -0.8]
        ));
    }

    #[test]
    fn reverse_flips_direction() {
        let mut p = params(OffsetSide::On, 0.0, 0.0, 1, 0.0);
        p.reverse = true;
        let paths = boundary_trace(&part(), &p);
        assert!(
            paths[0].area() > 0.0 && paths[1].area() < 0.0,
            "orientation flipped, order kept"
        );
    }
}
