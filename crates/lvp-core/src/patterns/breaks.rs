//! Breaks in a pass's paths: dashes along every path (perforation), or tabs, evenly spaced
//! gaps in every closed path that keep a cut part attached. Geometry only: a gap is simply
//! where no path runs.

use cavalier_contours::polyline::PlineSource;

use crate::diag::Diagnostic;
use crate::model::Breaks;
use crate::pline::{self, Pline};

use super::PatternOutput;

/// Split every path of `out` as `breaks` asks. A constraint between two paths becomes one
/// between their pieces: the pieces of each constrained path keep their order along it,
/// and the last piece of the earlier path comes before the first of the later one. Pieces
/// of a path whose direction is fixed keep that direction.
pub fn apply(out: PatternOutput, breaks: &Breaks) -> PatternOutput {
    let mut paths = Vec::new();
    let mut pieces: Vec<std::ops::Range<usize>> = Vec::with_capacity(out.paths.len());
    let mut vanished = 0usize;
    for p in &out.paths {
        let start = paths.len();
        let parts = split(p, breaks);
        if parts.is_empty() {
            vanished += 1;
        }
        paths.extend(parts);
        pieces.push(start..paths.len());
    }
    let mut constrained = vec![false; out.paths.len()];
    let mut precedence = Vec::new();
    for &(a, b) in &out.precedence {
        constrained[a] = true;
        constrained[b] = true;
        if let (Some(last), Some(first)) = (pieces[a].clone().last(), pieces[b].clone().next()) {
            precedence.push((last, first));
        }
    }
    for (i, range) in pieces.iter().enumerate() {
        if constrained[i] {
            precedence.extend(range.clone().zip(range.clone().skip(1)));
        }
    }
    let directed = out
        .directed
        .iter()
        .flat_map(|&i| pieces[i].clone())
        .collect();
    let mut diagnostics = out.diagnostics;
    if vanished > 0 {
        diagnostics.push(Diagnostic::warning(
            "pattern.breaks_consumed",
            format!("{vanished} path(s) are shorter than their breaks and were left out"),
        ));
    }
    PatternOutput {
        paths,
        diagnostics,
        stats: out.stats,
        precedence,
        directed,
    }
}

/// The marked pieces of one path.
fn split(p: &Pline, breaks: &Breaks) -> Vec<Pline> {
    let len = pline::length(p);
    match *breaks {
        Breaks::Dashes {
            mark_mm,
            gap_mm,
            offset_mm,
        } => {
            if mark_mm.is_nan() || mark_mm <= 0.0 || !mark_mm.is_finite() {
                return Vec::new();
            }
            if gap_mm.is_nan() || gap_mm <= 0.0 || !gap_mm.is_finite() {
                return vec![p.clone()];
            }
            let period = mark_mm + gap_mm;
            // The pattern runs from the path's start, shifted on by the offset.
            let mut s = -offset_mm.rem_euclid(period);
            let mut out = Vec::new();
            while s < len {
                let (a, b) = (s.max(0.0), (s + mark_mm).min(len));
                if b > a
                    && let Some(piece) = pline::sub_path(p, a, b)
                {
                    out.push(piece);
                }
                s += period;
            }
            out
        }
        Breaks::Tabs { count, width_mm } => {
            if !p.is_closed()
                || count == 0
                || width_mm.is_nan()
                || width_mm <= 0.0
                || !width_mm.is_finite()
            {
                return vec![p.clone()];
            }
            let n = count as f64;
            if n * width_mm >= len {
                return Vec::new();
            }
            // Gaps centred at (i + 1/2) L / n from the seam; the marks run between them.
            let spacing = len / n;
            (0..count)
                .filter_map(|i| {
                    let from = (i as f64 + 0.5) * spacing + width_mm / 2.0;
                    let to = (i as f64 + 1.5) * spacing - width_mm / 2.0;
                    pline::sub_path(p, from.rem_euclid(len), to.rem_euclid(len))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::pline::{circle, from_points, rect};

    fn out(paths: Vec<Pline>) -> PatternOutput {
        PatternOutput {
            paths,
            ..Default::default()
        }
    }

    fn lengths(o: &PatternOutput) -> Vec<f64> {
        o.paths.iter().map(pline::length).collect()
    }

    #[test]
    fn dashes_mark_and_skip_from_the_start_of_every_path() {
        let line = from_points([Point::new(0.0, 0.0), Point::new(10.0, 0.0)], false);
        let dashes = |offset_mm| Breaks::Dashes {
            mark_mm: 1.0,
            gap_mm: 0.5,
            offset_mm,
        };
        let d = apply(out(vec![line.clone()]), &dashes(0.0));
        assert_eq!(lengths(&d), vec![1.0; 7]);
        assert_eq!(d.paths[1].at(0).x, 1.5);
        // Shifted by 0.75: a quarter mark first, then whole marks.
        let s = apply(out(vec![line]), &dashes(0.75));
        assert!((lengths(&s)[0] - 0.25).abs() < 1e-12);
        assert!((s.paths[1].at(0).x - 0.75).abs() < 1e-12);
        // Round a circle (arcs stay arcs): marked two thirds of the way round.
        let c = apply(out(vec![circle(Point::new(0.0, 0.0), 1.0)]), &dashes(0.0));
        let total: f64 = lengths(&c).iter().sum();
        let tau = std::f64::consts::TAU;
        assert!((total - (tau / 1.5).floor() - (tau % 1.5).min(1.0)).abs() < 1e-9);
    }

    #[test]
    fn tabs_leave_evenly_spaced_gaps_in_closed_paths_only() {
        // A 10 mm square from its corner: four tabs centred on the middles of the sides.
        let square = rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0));
        let line = from_points([Point::new(0.0, 0.0), Point::new(3.0, 0.0)], false);
        let tabs = Breaks::Tabs {
            count: 4,
            width_mm: 1.0,
        };
        let t = apply(out(vec![square.clone(), line]), &tabs);
        assert_eq!(lengths(&t), vec![9.0, 9.0, 9.0, 9.0, 3.0]);
        assert!(
            t.paths[0]
                .at(0)
                .pos()
                .fuzzy_eq(cavalier_contours::core::math::Vector2::new(5.5, 0.0))
        );
        assert!(t.diagnostics.is_empty());
        // Tabs wider than the path leave nothing of it, and say so.
        let wide = apply(
            out(vec![square]),
            &Breaks::Tabs {
                count: 4,
                width_mm: 10.0,
            },
        );
        assert!(wide.paths.is_empty());
        assert_eq!(wide.diagnostics[0].code, "pattern.breaks_consumed");
    }

    #[test]
    fn constraints_and_directions_carry_over_to_the_pieces() {
        // A hole before its outer contour: every piece of the hole before every piece of
        // the outer, through the pieces' own order.
        let outer = rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0));
        let hole = rect(Point::new(4.0, 4.0), Point::new(6.0, 6.0));
        let line = from_points([Point::new(20.0, 0.0), Point::new(30.0, 0.0)], false);
        let input = PatternOutput {
            paths: vec![outer, hole, line],
            precedence: vec![(1, 0)],
            directed: vec![2],
            ..Default::default()
        };
        let t = apply(
            input,
            &Breaks::Tabs {
                count: 2,
                width_mm: 0.5,
            },
        );
        // Outer: pieces 0, 1; hole: 2, 3; the line, whole: 4.
        assert_eq!(t.paths.len(), 5);
        let mut pairs = t.precedence.clone();
        pairs.sort();
        assert_eq!(pairs, vec![(0, 1), (2, 3), (3, 0)]);
        assert_eq!(t.directed, vec![4]);
        let d = apply(
            t,
            &Breaks::Dashes {
                mark_mm: 2.0,
                gap_mm: 1.0,
                offset_mm: 0.0,
            },
        );
        let line_pieces = d.directed.len();
        assert_eq!(line_pieces, 4);
    }
}
