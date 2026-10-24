//! Aperture macros (`%AM…%`): parsing of primitive statements with variables and
//! arithmetic, and evaluation into a filled region in millimetres.
//!
//! Primitives are composited in source order: exposure *on* unions, exposure *off*
//! subtracts. Rotations are about the macro origin, as the format specifies.

use std::f64::consts::{PI, TAU};

use crate::geom::Point;
use crate::pline::{self, Pline};
use crate::region::{LoopRule, Region};

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Var(usize),
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
}

impl Expr {
    pub fn eval(&self, vars: &[f64]) -> f64 {
        match self {
            Expr::Num(v) => *v,
            // Undefined variables are 0 per the specification.
            Expr::Var(i) => vars.get(*i).copied().unwrap_or(0.0),
            Expr::Neg(e) => -e.eval(vars),
            Expr::Add(a, b) => a.eval(vars) + b.eval(vars),
            Expr::Sub(a, b) => a.eval(vars) - b.eval(vars),
            Expr::Mul(a, b) => a.eval(vars) * b.eval(vars),
            Expr::Div(a, b) => {
                let d = b.eval(vars);
                if d == 0.0 { 0.0 } else { a.eval(vars) / d }
            }
        }
    }
}

/// Recursive-descent parser for macro arithmetic: `+ - x X /`, parentheses, unary minus,
/// numbers and `$n` variables.
pub fn parse_expr(text: &str) -> Result<Expr, String> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut p = ExprParser { chars, pos: 0 };
    let e = p.sum()?;
    if p.pos != p.chars.len() {
        return Err(format!(
            "unexpected {:?} in expression {text:?}",
            p.chars[p.pos]
        ));
    }
    Ok(e)
}

struct ExprParser {
    chars: Vec<char>,
    pos: usize,
}

impl ExprParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut e = self.product()?;
        while let Some(c) = self.peek() {
            match c {
                '+' => {
                    self.pos += 1;
                    e = Expr::Add(Box::new(e), Box::new(self.product()?));
                }
                '-' => {
                    self.pos += 1;
                    e = Expr::Sub(Box::new(e), Box::new(self.product()?));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn product(&mut self) -> Result<Expr, String> {
        let mut e = self.unary()?;
        while let Some(c) = self.peek() {
            match c {
                'x' | 'X' => {
                    self.pos += 1;
                    e = Expr::Mul(Box::new(e), Box::new(self.unary()?));
                }
                '/' => {
                    self.pos += 1;
                    e = Expr::Div(Box::new(e), Box::new(self.unary()?));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some('+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let e = self.sum()?;
                if self.peek() != Some(')') {
                    return Err("missing ')' in macro expression".into());
                }
                self.pos += 1;
                Ok(e)
            }
            Some('$') => {
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
                let s: String = self.chars[start..self.pos].iter().collect();
                s.parse::<usize>()
                    .map(Expr::Var)
                    .map_err(|_| format!("bad variable reference ${s}"))
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.pos += 1;
                }
                let s: String = self.chars[start..self.pos].iter().collect();
                s.parse::<f64>()
                    .map(Expr::Num)
                    .map_err(|_| format!("bad number {s:?} in macro expression"))
            }
            Some(c) => Err(format!("unexpected {c:?} in macro expression")),
            None => Err("unexpected end of macro expression".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// `$n = expr`
    Assign(usize, Expr),
    /// `code, param, param, …` (comments are dropped at parse time)
    Primitive { code: u32, params: Vec<Expr> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroDef {
    pub name: String,
    pub statements: Vec<Statement>,
}

/// Parse the statements of an `%AM` block (the name statement already removed).
pub fn parse_macro(name: &str, statements: &[String]) -> Result<MacroDef, String> {
    let mut out = Vec::new();
    for st in statements {
        let st = st.trim();
        if st.is_empty() {
            continue;
        }
        if let Some(rest) = st.strip_prefix('$') {
            let (var, expr) = rest
                .split_once('=')
                .ok_or_else(|| format!("macro {name}: bad assignment {st:?}"))?;
            let idx: usize = var
                .trim()
                .parse()
                .map_err(|_| format!("macro {name}: bad variable in {st:?}"))?;
            out.push(Statement::Assign(idx, parse_expr(expr)?));
            continue;
        }
        // Comment primitive: `0 <text>` (no comma after the code).
        if st == "0" || st.starts_with("0 ") || st.starts_with("0\t") {
            continue;
        }
        let mut parts = st.split(',');
        let code_str = parts.next().unwrap_or("").trim();
        let code: u32 = code_str
            .parse()
            .map_err(|_| format!("macro {name}: bad primitive code {code_str:?}"))?;
        let params = parts.map(parse_expr).collect::<Result<Vec<_>, _>>()?;
        out.push(Statement::Primitive { code, params });
    }
    Ok(MacroDef {
        name: name.to_string(),
        statements: out,
    })
}

/// Problems while evaluating a macro into geometry; the aperture still gets whatever
/// primitives did evaluate.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroIssue {
    pub code: u32,
    pub message: String,
}

/// Evaluate a macro with `modifiers` as `$1..$n` into a region. `scale` converts file units
/// to millimetres.
pub fn evaluate(def: &MacroDef, modifiers: &[f64], scale: f64) -> (Region, Vec<MacroIssue>) {
    // Variables are 1-based; index 0 is unused.
    let mut vars: Vec<f64> = std::iter::once(0.0)
        .chain(modifiers.iter().copied())
        .collect();
    let mut region = Region::empty();
    let mut issues = Vec::new();
    for st in &def.statements {
        match st {
            Statement::Assign(idx, expr) => {
                let v = expr.eval(&vars);
                if vars.len() <= *idx {
                    vars.resize(idx + 1, 0.0);
                }
                vars[*idx] = v;
            }
            Statement::Primitive { code, params } => {
                let p: Vec<f64> = params.iter().map(|e| e.eval(&vars)).collect();
                match primitive(*code, &p, scale) {
                    Ok((on, shape)) => {
                        region = if on {
                            if region.is_empty() {
                                shape
                            } else {
                                region.union(&shape)
                            }
                        } else if region.is_empty() {
                            region
                        } else {
                            region.difference(&shape)
                        };
                    }
                    Err(message) => issues.push(MacroIssue {
                        code: *code,
                        message,
                    }),
                }
            }
        }
    }
    (region, issues)
}

fn need(p: &[f64], n: usize, code: u32) -> Result<(), String> {
    if p.len() < n {
        Err(format!(
            "primitive {code} needs {n} parameters, got {}",
            p.len()
        ))
    } else {
        Ok(())
    }
}

fn rotate_loops(loops: Vec<Pline>, deg: f64) -> Vec<Pline> {
    if deg == 0.0 {
        return loops;
    }
    let a = deg.to_radians();
    loops.iter().map(|l| pline::rotated(l, a)).collect()
}

fn rect_from_center(cx: f64, cy: f64, w: f64, h: f64) -> Pline {
    pline::rect(
        Point::new(cx - w / 2.0, cy - h / 2.0),
        Point::new(cx + w / 2.0, cy + h / 2.0),
    )
}

/// Rectangle of `width` around the segment `a -> b`.
fn thick_segment(a: Point, b: Point, width: f64) -> Option<Pline> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    if len <= 0.0 || width <= 0.0 {
        return None;
    }
    let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
    Some(pline::from_points(
        [
            Point::new(a.x + nx, a.y + ny),
            Point::new(a.x - nx, a.y - ny),
            Point::new(b.x - nx, b.y - ny),
            Point::new(b.x + nx, b.y + ny),
        ],
        true,
    ))
}

/// Regular polygon with `n` vertices on a circle of `diameter` around `c`, first vertex on
/// the +x axis (before the macro rotation).
pub fn regular_polygon(c: Point, diameter: f64, n: usize, start_deg: f64) -> Pline {
    let r = diameter / 2.0;
    let a0 = start_deg.to_radians();
    pline::from_points(
        (0..n).map(|i| {
            let a = a0 + TAU * i as f64 / n as f64;
            Point::new(c.x + r * a.cos(), c.y + r * a.sin())
        }),
        true,
    )
}

/// One primitive as `(exposure on, region)`; all lengths scaled to mm.
fn primitive(code: u32, p: &[f64], s: f64) -> Result<(bool, Region), String> {
    let on = |v: f64| v != 0.0;
    let loops_region = |loops: Vec<Pline>, rule: LoopRule| Region::from_loops(loops, rule);
    match code {
        1 => {
            need(p, 4, code)?;
            let rot = p.get(4).copied().unwrap_or(0.0);
            let c = pline::circle(Point::new(p[2] * s, p[3] * s), p[1] * s / 2.0);
            Ok((
                on(p[0]),
                loops_region(rotate_loops(vec![c], rot), LoopRule::NonZero),
            ))
        }
        2 | 20 => {
            need(p, 7, code)?;
            let seg = thick_segment(
                Point::new(p[2] * s, p[3] * s),
                Point::new(p[4] * s, p[5] * s),
                p[1] * s,
            );
            Ok((
                on(p[0]),
                loops_region(
                    rotate_loops(seg.into_iter().collect(), p[6]),
                    LoopRule::NonZero,
                ),
            ))
        }
        21 => {
            need(p, 6, code)?;
            let r = rect_from_center(p[3] * s, p[4] * s, p[1] * s, p[2] * s);
            Ok((
                on(p[0]),
                loops_region(rotate_loops(vec![r], p[5]), LoopRule::NonZero),
            ))
        }
        22 => {
            need(p, 6, code)?;
            let r = pline::rect(
                Point::new(p[3] * s, p[4] * s),
                Point::new((p[3] + p[1]) * s, (p[4] + p[2]) * s),
            );
            Ok((
                on(p[0]),
                loops_region(rotate_loops(vec![r], p[5]), LoopRule::NonZero),
            ))
        }
        4 => {
            need(p, 2, code)?;
            let n = p[1].round();
            if !(3.0..=5000.0).contains(&n) {
                return Err(format!("outline with {n} vertices"));
            }
            let n = n as usize;
            // n + 1 coordinate pairs (the last repeats the first) plus the rotation.
            need(p, 2 + 2 * (n + 1) + 1, code)?;
            let pts: Vec<Point> = (0..n)
                .map(|i| Point::new(p[2 + 2 * i] * s, p[3 + 2 * i] * s))
                .collect();
            let rot = p[2 + 2 * (n + 1)];
            let poly = pline::from_points(pts, true);
            Ok((
                on(p[0]),
                loops_region(rotate_loops(vec![poly], rot), LoopRule::NonZero),
            ))
        }
        5 => {
            need(p, 6, code)?;
            let n = p[1].round();
            if !(3.0..=12.0).contains(&n) {
                return Err(format!("polygon with {n} vertices"));
            }
            let poly = regular_polygon(Point::new(p[2] * s, p[3] * s), p[4] * s, n as usize, 0.0);
            Ok((
                on(p[0]),
                loops_region(rotate_loops(vec![poly], p[5]), LoopRule::NonZero),
            ))
        }
        6 => {
            // Moiré: cx, cy, outer diameter, ring thickness, gap, max rings, crosshair
            // thickness, crosshair length, rotation. Always exposure on.
            need(p, 9, code)?;
            let c = Point::new(p[0] * s, p[1] * s);
            let (mut outer, thick, gap) = (p[2] * s / 2.0, p[3] * s, p[4] * s);
            let max_rings = p[5].clamp(0.0, 100.0) as usize;
            let mut region = Region::empty();
            for _ in 0..max_rings {
                if outer <= 0.0 {
                    break;
                }
                let inner = outer - thick;
                let ring = if inner > 0.0 {
                    Region::from_loops(
                        [pline::circle(c, outer), pline::circle(c, inner)],
                        LoopRule::EvenOdd,
                    )
                } else {
                    Region::from_loops([pline::circle(c, outer)], LoopRule::NonZero)
                };
                region = if region.is_empty() {
                    ring
                } else {
                    region.union(&ring)
                };
                outer = inner - gap;
            }
            let (ct, cl) = (p[6] * s, p[7] * s);
            if ct > 0.0 && cl > 0.0 {
                let h = rect_from_center(c.x, c.y, cl, ct);
                let v = rect_from_center(c.x, c.y, ct, cl);
                let cross = Region::from_loops([h, v], LoopRule::MergeThenNest);
                region = if region.is_empty() {
                    cross
                } else {
                    region.union(&cross)
                };
            }
            let loops: Vec<Pline> = region.outers().chain(region.holes()).cloned().collect();
            Ok((
                true,
                loops_region(rotate_loops(loops, p[8]), LoopRule::NonZero),
            ))
        }
        7 => {
            // Thermal: cx, cy, outer diameter, inner diameter, gap thickness, rotation.
            need(p, 6, code)?;
            let c = Point::new(p[0] * s, p[1] * s);
            let (ro, ri, gap) = (p[2] * s / 2.0, p[3] * s / 2.0, p[4] * s);
            if ro <= 0.0 || ri >= ro || gap <= 0.0 {
                return Err("thermal with inconsistent diameters or gap".into());
            }
            let annulus = if ri > 0.0 {
                Region::from_loops(
                    [pline::circle(c, ro), pline::circle(c, ri)],
                    LoopRule::EvenOdd,
                )
            } else {
                Region::from_loops([pline::circle(c, ro)], LoopRule::NonZero)
            };
            let bars = Region::from_loops(
                [
                    rect_from_center(c.x, c.y, ro * 2.0 + gap, gap),
                    rect_from_center(c.x, c.y, gap, ro * 2.0 + gap),
                ],
                LoopRule::MergeThenNest,
            );
            let thermal = annulus.difference(&bars);
            let loops: Vec<Pline> = thermal.outers().chain(thermal.holes()).cloned().collect();
            Ok((
                true,
                loops_region(rotate_loops(loops, p[5]), LoopRule::NonZero),
            ))
        }
        other => Err(format!("unknown macro primitive code {other}")),
    }
}

/// Angle helper exposed for tests.
pub fn deg(rad: f64) -> f64 {
    rad * 180.0 / PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_follow_precedence_and_variables() {
        let e = parse_expr("1+2x3").unwrap();
        assert_eq!(e.eval(&[]), 7.0);
        let e = parse_expr("(1+2)x3").unwrap();
        assert_eq!(e.eval(&[]), 9.0);
        let e = parse_expr("-$1/2").unwrap();
        assert_eq!(e.eval(&[0.0, 5.0]), -2.5);
        let e = parse_expr("$2").unwrap();
        assert_eq!(e.eval(&[0.0, 5.0]), 0.0, "undefined variables read as 0");
        assert!(parse_expr("1+").is_err());
        assert!(parse_expr("(1").is_err());
    }

    #[test]
    fn thermal_has_four_pads_and_vector_line_is_a_rectangle() {
        let def = parse_macro("THERMAL80", &["7,0,0,0.800,0.550,0.125,45".to_string()]).unwrap();
        let (r, issues) = evaluate(&def, &[], 1.0);
        assert!(issues.is_empty());
        assert_eq!(r.outers().count(), 4, "four separate pads");
        let annulus = PI * (0.4f64.powi(2) - 0.275f64.powi(2));
        assert!(
            r.area() < annulus && r.area() > annulus * 0.5,
            "{}",
            r.area()
        );

        let def = parse_macro("LINE", &["20,1,0.9,0,0.45,12,0.45,0".to_string()]).unwrap();
        let (r, _) = evaluate(&def, &[], 1.0);
        assert!((r.area() - 12.0 * 0.9).abs() < 1e-9);

        // Variables from the aperture definition and exposure-off subtraction.
        let def = parse_macro(
            "DONUT",
            &[
                "$3=$1/2".to_string(),
                "1,1,$1,0,0".to_string(),
                "1,0,$2,0,0".to_string(),
            ],
        )
        .unwrap();
        let (r, _) = evaluate(&def, &[2.0, 1.0], 1.0);
        assert!((r.area() - PI * (1.0 - 0.25)).abs() < 1e-6);
        assert_eq!(r.holes().count(), 1);
    }

    #[test]
    fn comment_primitives_are_ignored() {
        let def = parse_macro(
            "C",
            &[
                "0 a comment, with a comma".to_string(),
                "1,1,2,0,0".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(def.statements.len(), 1);
    }

    #[test]
    fn unknown_primitives_are_reported_and_skipped() {
        let def = parse_macro("X", &["99,1,2".to_string(), "1,1,1,0,0".to_string()]).unwrap();
        let (r, issues) = evaluate(&def, &[], 1.0);
        assert_eq!(issues.len(), 1);
        assert!((r.area() - PI / 4.0).abs() < 1e-9);
    }
}
