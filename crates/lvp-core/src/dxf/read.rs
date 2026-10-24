//! DXF reader for the spec's entity subset (ASCII DXF, R12 and later).
//!
//! Produces per-layer primitives in millimetres: centreline paths (with an optional
//! constant width) and filled hatch regions. Everything outside the subset becomes a
//! diagnostic, never silent loss: ACIS solids are errors, text and dimensions are
//! informational, unknown entity types are warnings. Block references are expanded with
//! their transforms; arcs survive uniform scaling and mirroring, non-uniform scaling
//! flattens them (with a warning).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f64::consts::TAU;

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};

use crate::arc::Arc;
use crate::diag::{Diagnostic, Severity};
use crate::geom::Point;
use crate::pline::{self, Pline};
use crate::region::LoopRule;
use crate::units::Unit;

/// Maximum nesting of block references before we give up (guards against cycles).
const MAX_INSERT_DEPTH: usize = 32;

#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// Chordal tolerance for flattening ellipses, splines and non-uniformly scaled arcs.
    pub tolerance_mm: f64,
    /// Units applied when the file declares none (`$INSUNITS` 0 or missing).
    pub assume_units: Unit,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            tolerance_mm: 0.001,
            assume_units: Unit::Millimetre,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Primitive {
    /// Open or closed centreline. `width` is a constant stroke width in mm when the source
    /// entity carried one (LWPOLYLINE/POLYLINE widths); `None` is a zero-width centreline.
    Path { pline: Pline, width: Option<f64> },
    /// Filled area bounded by loops, from HATCH, SOLID or TRACE.
    Hatch {
        loops: Vec<Pline>,
        rule: LoopRule,
        solid: bool,
        pattern: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct SourceLayer {
    pub name: String,
    /// AutoCAD Color Index from the layer table, if the layer was declared.
    pub color: Option<i16>,
    pub primitives: Vec<Primitive>,
}

#[derive(Clone, Debug)]
pub struct DxfDocument {
    /// `$ACADVER`, e.g. `AC1015`.
    pub version: Option<String>,
    /// Declared drawing units; `None` when the file is unitless. Coordinates are always
    /// returned in millimetres (using `ReadOptions::assume_units` for unitless files).
    pub units: Option<Unit>,
    /// Sorted by name. Includes declared-but-empty layers.
    pub layers: Vec<SourceLayer>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DxfDocument {
    pub fn layer(&self, name: &str) -> Option<&SourceLayer> {
        self.layers.iter().find(|l| l.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DxfReadError {
    Malformed { line: usize, message: String },
    BinaryDxf,
    Empty,
}

impl std::fmt::Display for DxfReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DxfReadError::Malformed { line, message } => {
                write!(f, "malformed DXF at line {line}: {message}")
            }
            DxfReadError::BinaryDxf => write!(f, "binary DXF is not supported; save as ASCII DXF"),
            DxfReadError::Empty => write!(f, "file contains no DXF group codes"),
        }
    }
}

impl std::error::Error for DxfReadError {}

// ---------------------------------------------------------------------------------------
// Tokenizer and raw structure

#[derive(Clone, Debug)]
struct Pair {
    code: i32,
    value: String,
    line: usize,
}

fn tokenize(text: &str) -> Result<Vec<Pair>, DxfReadError> {
    if text.starts_with("AutoCAD Binary DXF") {
        return Err(DxfReadError::BinaryDxf);
    }
    let mut pairs = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((ln, code_line)) = lines.next() {
        let code_str = code_line.trim();
        if code_str.is_empty() {
            // Blank trailing lines are fine; a blank code line mid-file is not.
            if lines.peek().is_none() {
                break;
            }
            return Err(DxfReadError::Malformed {
                line: ln + 1,
                message: "empty group code".into(),
            });
        }
        let code: i32 = code_str.parse().map_err(|_| DxfReadError::Malformed {
            line: ln + 1,
            message: format!("group code is not an integer: {code_str:?}"),
        })?;
        let Some((_, value)) = lines.next() else {
            return Err(DxfReadError::Malformed {
                line: ln + 1,
                message: "group code without a value".into(),
            });
        };
        pairs.push(Pair {
            code,
            value: value.trim().to_string(),
            line: ln + 1,
        });
    }
    if pairs.is_empty() {
        return Err(DxfReadError::Empty);
    }
    Ok(pairs)
}

#[derive(Clone, Debug, Default)]
struct RawEntity {
    kind: String,
    pairs: Vec<Pair>,
    /// VERTEX entities of a POLYLINE.
    children: Vec<RawEntity>,
}

impl RawEntity {
    fn first(&self, code: i32) -> Option<&str> {
        self.pairs
            .iter()
            .find(|p| p.code == code)
            .map(|p| p.value.as_str())
    }

    fn f(&self, code: i32) -> Option<f64> {
        self.first(code).and_then(|v| v.parse().ok())
    }

    fn i(&self, code: i32) -> Option<i64> {
        self.first(code).and_then(|v| v.trim().parse().ok())
    }

    fn layer(&self) -> String {
        self.first(8).unwrap_or("0").to_string()
    }

    fn handle(&self) -> Option<&str> {
        self.first(5)
    }

    fn point(&self, xcode: i32) -> Option<Point> {
        Some(Point::new(self.f(xcode)?, self.f(xcode + 10)?))
    }
}

/// Split a run of pairs into entities at every group code 0, folding POLYLINE vertices.
fn group_entities(pairs: &[Pair]) -> Vec<RawEntity> {
    let mut out: Vec<RawEntity> = Vec::new();
    let mut cur: Option<RawEntity> = None;
    for p in pairs {
        if p.code == 0 {
            if let Some(e) = cur.take() {
                out.push(e);
            }
            cur = Some(RawEntity {
                kind: p.value.to_ascii_uppercase(),
                ..Default::default()
            });
        } else if let Some(e) = cur.as_mut() {
            e.pairs.push(p.clone());
        }
    }
    if let Some(e) = cur {
        out.push(e);
    }
    // Fold VERTEX ... SEQEND into the preceding POLYLINE.
    let mut folded: Vec<RawEntity> = Vec::with_capacity(out.len());
    for e in out {
        match (e.kind.as_str(), folded.last_mut()) {
            ("VERTEX", Some(prev)) if prev.kind == "POLYLINE" => prev.children.push(e),
            ("SEQEND", Some(prev)) if prev.kind == "POLYLINE" || prev.kind == "INSERT" => {}
            _ => folded.push(e),
        }
    }
    folded
}

struct Section<'a> {
    name: String,
    pairs: &'a [Pair],
}

fn sections(pairs: &[Pair]) -> Result<Vec<Section<'_>>, DxfReadError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let p = &pairs[i];
        if p.code == 0 && p.value.eq_ignore_ascii_case("SECTION") {
            let Some(name_pair) = pairs.get(i + 1).filter(|n| n.code == 2) else {
                return Err(DxfReadError::Malformed {
                    line: p.line,
                    message: "SECTION without a name".into(),
                });
            };
            let start = i + 2;
            let mut j = start;
            while j < pairs.len()
                && !(pairs[j].code == 0 && pairs[j].value.eq_ignore_ascii_case("ENDSEC"))
            {
                j += 1;
            }
            out.push(Section {
                name: name_pair.value.to_ascii_uppercase(),
                pairs: &pairs[start..j],
            });
            i = j + 1;
        } else if p.code == 0 && p.value.eq_ignore_ascii_case("EOF") {
            break;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

fn header_variables(pairs: &[Pair]) -> HashMap<String, Vec<Pair>> {
    let mut vars: HashMap<String, Vec<Pair>> = HashMap::new();
    let mut cur: Option<String> = None;
    for p in pairs {
        if p.code == 9 {
            cur = Some(p.value.to_ascii_uppercase());
            vars.entry(p.value.to_ascii_uppercase()).or_default();
        } else if let Some(name) = &cur {
            vars.get_mut(name).unwrap().push(p.clone());
        }
    }
    vars
}

struct Block {
    base: Point,
    entities: Vec<RawEntity>,
}

// ---------------------------------------------------------------------------------------
// Affine transforms

/// `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Xf {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Xf {
    const IDENTITY: Xf = Xf {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn translate(dx: f64, dy: f64) -> Xf {
        Xf {
            e: dx,
            f: dy,
            ..Xf::IDENTITY
        }
    }

    fn scale(sx: f64, sy: f64) -> Xf {
        Xf {
            a: sx,
            d: sy,
            ..Xf::IDENTITY
        }
    }

    fn rotate(rad: f64) -> Xf {
        let (s, c) = rad.sin_cos();
        Xf {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// `self` first, then `outer`.
    fn then(&self, outer: &Xf) -> Xf {
        Xf {
            a: outer.a * self.a + outer.c * self.b,
            b: outer.b * self.a + outer.d * self.b,
            c: outer.a * self.c + outer.c * self.d,
            d: outer.b * self.c + outer.d * self.d,
            e: outer.a * self.e + outer.c * self.f + outer.e,
            f: outer.b * self.e + outer.d * self.f + outer.f,
        }
    }

    fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Uniform scale factor (geometric mean of the singular values).
    fn scale_factor(&self) -> f64 {
        self.det().abs().sqrt()
    }

    /// Rotation + uniform scale + optional mirror: arcs stay arcs.
    fn is_similarity(&self) -> bool {
        let c1 = self.a * self.a + self.b * self.b;
        let c2 = self.c * self.c + self.d * self.d;
        let dot = self.a * self.c + self.b * self.d;
        let s = c1.max(c2).max(1e-300);
        (c1 - c2).abs() <= 1e-9 * s && dot.abs() <= 1e-9 * s
    }
}

fn transform_pline(p: &Pline, xf: &Xf, tolerance_mm: f64) -> Pline {
    if *xf == Xf::IDENTITY {
        return p.clone();
    }
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    if xf.is_similarity() {
        let flip = if xf.det() < 0.0 { -1.0 } else { 1.0 };
        for v in p.iter_vertexes() {
            let q = xf.apply(Point::new(v.x, v.y));
            out.add(q.x, q.y, v.bulge * flip);
        }
    } else {
        // Non-uniform scale turns arcs into ellipses: flatten first, in source units.
        let tol = tolerance_mm / xf.scale_factor().max(1e-12);
        if let Ok(pts) = pline::flatten(p, tol) {
            for q in pts {
                let t = xf.apply(q);
                out.add(t.x, t.y, 0.0);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Curve evaluation helpers

/// Adaptive flattening of a parametric curve on `[t0, t1]` within `tol`.
fn flatten_curve(f: &dyn Fn(f64) -> Point, t0: f64, t1: f64, tol: f64, out: &mut Vec<Point>) {
    fn deviation(p: Point, a: Point, b: Point) -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        if len2 <= 0.0 {
            return p.distance(a);
        }
        let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
        let t = t.clamp(0.0, 1.0);
        p.distance(Point::new(a.x + t * dx, a.y + t * dy))
    }
    #[allow(clippy::too_many_arguments)]
    fn rec(
        f: &dyn Fn(f64) -> Point,
        t0: f64,
        t1: f64,
        p0: Point,
        p1: Point,
        tol: f64,
        depth: u32,
        out: &mut Vec<Point>,
    ) {
        let tm = 0.5 * (t0 + t1);
        let pm = f(tm);
        let q1 = f(0.5 * (t0 + tm));
        let q3 = f(0.5 * (tm + t1));
        let ok = deviation(pm, p0, p1) <= tol
            && deviation(q1, p0, p1) <= tol
            && deviation(q3, p0, p1) <= tol;
        if ok || depth >= 18 {
            out.push(p1);
        } else {
            rec(f, t0, tm, p0, pm, tol, depth + 1, out);
            rec(f, tm, t1, pm, p1, tol, depth + 1, out);
        }
    }
    let p0 = f(t0);
    let p1 = f(t1);
    out.push(p0);
    // Always split once so closed curves (p0 == p1) are not accepted as a point.
    let tm = 0.5 * (t0 + t1);
    let pm = f(tm);
    rec(f, t0, tm, p0, pm, tol, 1, out);
    rec(f, tm, t1, pm, p1, tol, 1, out);
}

/// Point on a (possibly rational) B-spline by de Boor's algorithm.
fn nurbs_point(degree: usize, knots: &[f64], ctrl: &[(Point, f64)], u: f64) -> Point {
    let n = ctrl.len();
    let p = degree;
    let (lo, hi) = (knots[p], knots[n]);
    let u = u.clamp(lo, hi);
    // Knot span k with knots[k] <= u < knots[k+1], clamped to [p, n-1].
    let mut k = p;
    while k + 1 < n && u >= knots[k + 1] {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = (0..=p)
        .map(|j| {
            let (pt, w) = ctrl[k - p + j];
            (pt.x * w, pt.y * w, w)
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k - p + j;
            let denom = knots[i + p - r + 1] - knots[i];
            let alpha = if denom.abs() < 1e-300 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            d[j] = (
                (1.0 - alpha) * d[j - 1].0 + alpha * d[j].0,
                (1.0 - alpha) * d[j - 1].1 + alpha * d[j].1,
                (1.0 - alpha) * d[j - 1].2 + alpha * d[j].2,
            );
        }
    }
    let (x, y, w) = d[p];
    if w.abs() < 1e-300 {
        Point::new(x, y)
    } else {
        Point::new(x / w, y / w)
    }
}

// ---------------------------------------------------------------------------------------
// Conversion

struct Converter<'a> {
    opts: &'a ReadOptions,
    blocks: &'a HashMap<String, Block>,
    diagnostics: Vec<Diagnostic>,
    /// entity type -> (count, layers) for the unsupported summary
    unsupported: BTreeMap<String, (usize, BTreeSet<String>)>,
    paperspace_skipped: usize,
}

impl Converter<'_> {
    fn diag(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    fn unsupported(&mut self, raw: &RawEntity, layer: &str) {
        let e = self.unsupported.entry(raw.kind.clone()).or_default();
        e.0 += 1;
        e.1.insert(layer.to_string());
    }

    fn convert(
        &mut self,
        raw: &RawEntity,
        xf: &Xf,
        layer_override: Option<&str>,
        depth: usize,
        out: &mut Vec<(String, Primitive)>,
    ) {
        if raw.i(67) == Some(1) {
            self.paperspace_skipped += 1;
            return;
        }
        let own_layer = raw.layer();
        let layer = match layer_override {
            Some(l) if own_layer == "0" => l.to_string(),
            _ => own_layer,
        };
        // Entities with a flipped extrusion direction live in a mirrored OCS.
        let xf = if raw.f(230).is_some_and(|z| z < 0.0) {
            Xf::scale(-1.0, 1.0).then(xf)
        } else {
            *xf
        };
        let tol = self.opts.tolerance_mm;
        let push_path = |pline: Pline, width: Option<f64>, out: &mut Vec<(String, Primitive)>| {
            if pline.vertex_count() >= 2 {
                out.push((
                    layer.clone(),
                    Primitive::Path {
                        pline: transform_pline(&pline, &xf, tol),
                        width: width.map(|w| w * xf.scale_factor()),
                    },
                ));
            }
        };
        match raw.kind.as_str() {
            "LINE" => {
                if let (Some(a), Some(b)) = (raw.point(10), raw.point(11)) {
                    push_path(pline::from_points([a, b], false), None, out);
                } else {
                    self.malformed(raw, "LINE without both end points");
                }
            }
            "CIRCLE" => match (raw.point(10), raw.f(40)) {
                (Some(c), Some(r)) if r > 0.0 => push_path(pline::circle(c, r), None, out),
                _ => self.malformed(raw, "CIRCLE without centre or positive radius"),
            },
            "ARC" => match (raw.point(10), raw.f(40), raw.f(50), raw.f(51)) {
                (Some(c), Some(r), Some(a0), Some(a1)) if r > 0.0 => {
                    let arc = Arc::from_dxf_angles(c, r, a0, a1);
                    let mut p = Pline::new();
                    pline::add_bulge_segment(
                        &mut p,
                        arc.start(),
                        arc.end(),
                        (arc.sweep / 4.0).tan(),
                    );
                    let e = arc.end();
                    p.add(e.x, e.y, 0.0);
                    push_path(p, None, out);
                }
                _ => self.malformed(raw, "ARC without centre, radius or angles"),
            },
            "LWPOLYLINE" => {
                let (p, width) = self.lwpolyline(raw);
                push_path(p, width, out);
            }
            "POLYLINE" => {
                if let Some((p, width)) = self.polyline(raw, &layer) {
                    push_path(p, width, out);
                }
            }
            "ELLIPSE" => {
                if let Some(p) = self.ellipse(raw) {
                    push_path(p, None, out);
                }
            }
            "SPLINE" => {
                if let Some(p) = self.spline(raw) {
                    push_path(p, None, out);
                }
            }
            "SOLID" | "TRACE" => {
                // Filled quad (or triangle) with the DXF bow-tie corner order 1-2-4-3.
                let pts: Vec<Point> = [10, 11, 13, 12]
                    .iter()
                    .filter_map(|&c| raw.point(c))
                    .collect();
                if pts.len() >= 3 {
                    let lp = transform_pline(&pline::from_points(pts, true), &xf, tol);
                    out.push((
                        layer.clone(),
                        Primitive::Hatch {
                            loops: vec![lp],
                            rule: LoopRule::NonZero,
                            solid: true,
                            pattern: "SOLID".into(),
                        },
                    ));
                } else {
                    self.malformed(raw, "SOLID with fewer than three corners");
                }
            }
            "HATCH" => {
                if let Some((loops, rule, solid, pattern)) = self.hatch(raw) {
                    let loops: Vec<Pline> =
                        loops.iter().map(|l| transform_pline(l, &xf, tol)).collect();
                    out.push((
                        layer.clone(),
                        Primitive::Hatch {
                            loops,
                            rule,
                            solid,
                            pattern,
                        },
                    ));
                }
            }
            "INSERT" => self.insert(raw, &xf, &layer, depth, out),
            "REGION" | "BODY" | "3DSOLID" | "SURFACE" | "ACAD_PROXY_ENTITY" | "MESH" => {
                self.unsupported(raw, &layer);
            }
            _ => self.unsupported(raw, &layer),
        }
    }

    fn malformed(&mut self, raw: &RawEntity, what: &str) {
        let ctx = format!("{} {}", raw.kind, raw.handle().unwrap_or("?"));
        self.diag(Diagnostic::warning("dxf.malformed_entity", what).with_context(ctx));
    }

    fn lwpolyline(&mut self, raw: &RawEntity) -> (Pline, Option<f64>) {
        let mut verts: Vec<(f64, f64, f64)> = Vec::new();
        let mut cur: Option<(f64, f64, f64)> = None;
        let mut const_width: Option<f64> = None;
        let mut vertex_widths: Vec<f64> = Vec::new();
        let mut flags = 0i64;
        for p in &raw.pairs {
            let v = p.value.parse::<f64>().ok();
            match p.code {
                10 => {
                    if let Some(c) = cur.take() {
                        verts.push(c);
                    }
                    cur = Some((v.unwrap_or(0.0), 0.0, 0.0));
                }
                20 => {
                    if let Some(c) = cur.as_mut() {
                        c.1 = v.unwrap_or(0.0);
                    }
                }
                42 => {
                    if let Some(c) = cur.as_mut() {
                        c.2 = v.unwrap_or(0.0);
                    }
                }
                40 | 41 => vertex_widths.push(v.unwrap_or(0.0)),
                43 => const_width = v,
                70 => flags = p.value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        if let Some(c) = cur {
            verts.push(c);
        }
        let closed = flags & 1 != 0;
        let width = self.resolve_width(raw, const_width, &vertex_widths);
        (pline::from_bulge_vertices(verts, closed), width)
    }

    fn resolve_width(
        &mut self,
        raw: &RawEntity,
        const_width: Option<f64>,
        vertex_widths: &[f64],
    ) -> Option<f64> {
        if let Some(w) = const_width.filter(|w| *w > 0.0) {
            return Some(w);
        }
        let positive: Vec<f64> = vertex_widths.iter().copied().filter(|w| *w > 0.0).collect();
        if positive.is_empty() {
            return None;
        }
        let max = positive.iter().copied().fold(0.0, f64::max);
        let min = positive.iter().copied().fold(f64::INFINITY, f64::min);
        if (max - min) > 1e-9 * max.max(1.0) || positive.len() != vertex_widths.len() {
            self.diag(
                Diagnostic::warning(
                    "dxf.variable_width",
                    format!("polyline with varying width approximated by its maximum width {max} (source units)"),
                )
                .with_context(format!("{} {}", raw.kind, raw.handle().unwrap_or("?"))),
            );
        }
        Some(max)
    }

    fn polyline(&mut self, raw: &RawEntity, layer: &str) -> Option<(Pline, Option<f64>)> {
        let flags = raw.i(70).unwrap_or(0);
        if flags & (8 | 16 | 64) != 0 {
            self.unsupported(raw, layer);
            return None;
        }
        if flags & (2 | 4) != 0 {
            self.diag(
                Diagnostic::warning(
                    "dxf.polyline_fit",
                    "curve/spline-fit POLYLINE read as its plain vertices",
                )
                .with_context(format!("POLYLINE {}", raw.handle().unwrap_or("?"))),
            );
        }
        let default_w = (raw.f(40).unwrap_or(0.0), raw.f(41).unwrap_or(0.0));
        let mut verts = Vec::new();
        let mut widths = Vec::new();
        for v in &raw.children {
            let vflags = v.i(70).unwrap_or(0);
            // Spline frame control points (16) are not on the curve; fit vertices (8) are.
            if vflags & 16 != 0 {
                continue;
            }
            if let Some(p) = v.point(10) {
                verts.push((p.x, p.y, v.f(42).unwrap_or(0.0)));
                widths.push(v.f(40).unwrap_or(default_w.0));
                widths.push(v.f(41).unwrap_or(default_w.1));
            }
        }
        let closed = flags & 1 != 0;
        let width = self.resolve_width(raw, None, &widths);
        Some((pline::from_bulge_vertices(verts, closed), width))
    }

    fn ellipse(&mut self, raw: &RawEntity) -> Option<Pline> {
        let (Some(c), Some(major), Some(ratio)) = (raw.point(10), raw.point(11), raw.f(40)) else {
            self.malformed(raw, "ELLIPSE without centre, major axis or ratio");
            return None;
        };
        let t0 = raw.f(41).unwrap_or(0.0);
        let mut t1 = raw.f(42).unwrap_or(TAU);
        if t1 <= t0 {
            t1 += TAU;
        }
        let full = (t1 - t0 - TAU).abs() < 1e-9;
        let minor = Point::new(-major.y * ratio, major.x * ratio);
        let f = |t: f64| {
            Point::new(
                c.x + major.x * t.cos() + minor.x * t.sin(),
                c.y + major.y * t.cos() + minor.y * t.sin(),
            )
        };
        let mut pts = Vec::new();
        flatten_curve(&f, t0, t1, self.opts.tolerance_mm, &mut pts);
        if full {
            pts.pop();
        }
        Some(pline::from_points(pts, full))
    }

    fn spline(&mut self, raw: &RawEntity) -> Option<Pline> {
        let flags = raw.i(70).unwrap_or(0);
        let closed = flags & 1 != 0;
        let degree = raw.i(71).unwrap_or(3).max(1) as usize;
        let knots: Vec<f64> = raw
            .pairs
            .iter()
            .filter(|p| p.code == 40)
            .filter_map(|p| p.value.parse().ok())
            .collect();
        let weights: Vec<f64> = raw
            .pairs
            .iter()
            .filter(|p| p.code == 41)
            .filter_map(|p| p.value.parse().ok())
            .collect();
        let ctrl: Vec<Point> = collect_points(&raw.pairs, 10);
        let fit: Vec<Point> = collect_points(&raw.pairs, 11);
        let ctx = format!("SPLINE {}", raw.handle().unwrap_or("?"));

        let pts: Vec<Point> = if ctrl.len() > degree && knots.len() == ctrl.len() + degree + 1 {
            let ctrl_w: Vec<(Point, f64)> = ctrl
                .iter()
                .enumerate()
                .map(|(i, p)| (*p, weights.get(i).copied().unwrap_or(1.0)))
                .collect();
            let f = |u: f64| nurbs_point(degree, &knots, &ctrl_w, u);
            let (u0, u1) = (knots[degree], knots[ctrl.len()]);
            let mut pts = Vec::new();
            // Flatten knot span by knot span so interior knots (C0 corners) are vertices.
            let mut spans: Vec<f64> = knots[degree..=ctrl.len()].to_vec();
            spans.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
            if spans.len() < 2 {
                spans = vec![u0, u1];
            }
            for w in spans.windows(2) {
                let mut seg = Vec::new();
                flatten_curve(&f, w[0], w[1], self.opts.tolerance_mm, &mut seg);
                if !pts.is_empty() {
                    seg.remove(0);
                }
                pts.extend(seg);
            }
            pts
        } else if fit.len() >= 2 {
            self.diag(
                Diagnostic::warning(
                    "dxf.spline_fit_points",
                    "SPLINE has no usable control polygon; read as a polyline through its fit points",
                )
                .with_context(ctx),
            );
            fit
        } else {
            self.malformed(raw, "SPLINE without control or fit points");
            return None;
        };
        let mut pts = pts;
        if closed && pts.len() > 2 && pts[0].distance(pts[pts.len() - 1]) < 1e-9 {
            pts.pop();
        }
        Some(pline::from_points(pts, closed))
    }

    fn hatch(&mut self, raw: &RawEntity) -> Option<(Vec<Pline>, LoopRule, bool, String)> {
        let ctx = format!("HATCH {}", raw.handle().unwrap_or("?"));
        let pattern = raw.first(2).unwrap_or("").to_string();
        let solid = raw.i(70) == Some(1);
        let style = raw.i(75).unwrap_or(0);
        let mut cur = Cursor {
            pairs: &raw.pairs,
            i: 0,
        };
        let Some(n_paths) = cur.seek(91).map(|v| v as usize) else {
            self.diag(
                Diagnostic::warning("dxf.malformed_entity", "HATCH without boundary paths")
                    .with_context(ctx),
            );
            return None;
        };
        let mut loops: Vec<(Pline, bool)> = Vec::new();
        for _ in 0..n_paths {
            let Some(flags) = cur.expect(92).map(|v| v as i64) else {
                self.diag(
                    Diagnostic::warning(
                        "dxf.malformed_entity",
                        "HATCH boundary path without type flags",
                    )
                    .with_context(ctx),
                );
                return None;
            };
            let external = flags & (1 | 16) != 0;
            let lp = if flags & 2 != 0 {
                let has_bulge = cur.take_if(72).is_some_and(|v| v != 0.0);
                let _closed = cur.take_if(73);
                let n = cur.expect(93)? as usize;
                let mut verts = Vec::with_capacity(n);
                for _ in 0..n {
                    let x = cur.expect(10)?;
                    let y = cur.expect(20)?;
                    let b = if has_bulge {
                        cur.take_if(42).unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    verts.push((x, y, b));
                }
                pline::from_bulge_vertices(verts, true)
            } else {
                let n_edges = cur.expect(93)? as usize;
                let mut edges: Vec<Edge> = Vec::with_capacity(n_edges);
                for _ in 0..n_edges {
                    let ty = cur.expect(72)? as i64;
                    let edge = match ty {
                        1 => {
                            let a = Point::new(cur.expect(10)?, cur.expect(20)?);
                            let b = Point::new(cur.expect(11)?, cur.expect(21)?);
                            Edge::line(a, b)
                        }
                        2 => {
                            let c = Point::new(cur.expect(10)?, cur.expect(20)?);
                            let r = cur.expect(40)?;
                            let a0 = cur.expect(50)?;
                            let a1 = cur.expect(51)?;
                            let ccw = cur.take_if(73).is_none_or(|v| v != 0.0);
                            let arc = Arc::from_dxf_angles(c, r, a0, a1);
                            let mut e = Edge::arc(&arc);
                            if !ccw {
                                e = e.reversed();
                            }
                            e
                        }
                        3 => {
                            let c = Point::new(cur.expect(10)?, cur.expect(20)?);
                            let major = Point::new(cur.expect(11)?, cur.expect(21)?);
                            let ratio = cur.expect(40)?;
                            let a0 = cur.expect(50)?.to_radians();
                            let mut a1 = cur.expect(51)?.to_radians();
                            let ccw = cur.take_if(73).is_none_or(|v| v != 0.0);
                            if a1 <= a0 {
                                a1 += TAU;
                            }
                            let minor = Point::new(-major.y * ratio, major.x * ratio);
                            let f = |t: f64| {
                                Point::new(
                                    c.x + major.x * t.cos() + minor.x * t.sin(),
                                    c.y + major.y * t.cos() + minor.y * t.sin(),
                                )
                            };
                            let mut pts = Vec::new();
                            flatten_curve(&f, a0, a1, self.opts.tolerance_mm, &mut pts);
                            let mut e = Edge::polyline(pts);
                            if !ccw {
                                e = e.reversed();
                            }
                            e
                        }
                        4 => {
                            let degree = cur.expect(94)? as usize;
                            let _rational = cur.take_if(73);
                            let _periodic = cur.take_if(74);
                            let n_knots = cur.expect(95)? as usize;
                            let n_ctrl = cur.expect(96)? as usize;
                            let mut knots = Vec::with_capacity(n_knots);
                            for _ in 0..n_knots {
                                knots.push(cur.expect(40)?);
                            }
                            let mut ctrl = Vec::with_capacity(n_ctrl);
                            for _ in 0..n_ctrl {
                                let x = cur.expect(10)?;
                                let y = cur.expect(20)?;
                                let w = cur.take_if(42).unwrap_or(1.0);
                                ctrl.push((Point::new(x, y), w));
                            }
                            let n_fit = cur.take_if(97).unwrap_or(0.0) as usize;
                            let mut fit = Vec::with_capacity(n_fit);
                            for _ in 0..n_fit {
                                fit.push(Point::new(cur.expect(11)?, cur.expect(21)?));
                            }
                            cur.take_if(12);
                            cur.take_if(22);
                            cur.take_if(13);
                            cur.take_if(23);
                            let mut pts = Vec::new();
                            if ctrl.len() > degree && knots.len() == ctrl.len() + degree + 1 {
                                let f = |u: f64| nurbs_point(degree, &knots, &ctrl, u);
                                flatten_curve(
                                    &f,
                                    knots[degree],
                                    knots[ctrl.len()],
                                    self.opts.tolerance_mm,
                                    &mut pts,
                                );
                            } else if fit.len() >= 2 {
                                pts = fit;
                            } else {
                                self.diag(
                                    Diagnostic::warning(
                                        "dxf.malformed_entity",
                                        "HATCH spline edge without control or fit points",
                                    )
                                    .with_context(ctx.clone()),
                                );
                                return None;
                            }
                            Edge::polyline(pts)
                        }
                        other => {
                            self.diag(
                                Diagnostic::warning(
                                    "dxf.malformed_entity",
                                    format!("HATCH boundary edge of unknown type {other}"),
                                )
                                .with_context(ctx.clone()),
                            );
                            return None;
                        }
                    };
                    edges.push(edge);
                }
                self.stitch_edges(edges, &ctx)
            };
            if let Some(n) = cur.take_if(97) {
                for _ in 0..(n as usize) {
                    cur.take_if(330);
                }
            }
            loops.push((lp, external));
        }
        let (loops, rule) = match style {
            1 => (
                loops
                    .iter()
                    .filter(|(_, ext)| *ext)
                    .map(|(l, _)| l.clone())
                    .collect::<Vec<_>>(),
                LoopRule::NonZero,
            ),
            2 => (
                loops.into_iter().map(|(l, _)| l).collect(),
                LoopRule::NonZero,
            ),
            _ => (
                loops.into_iter().map(|(l, _)| l).collect(),
                LoopRule::EvenOdd,
            ),
        };
        Some((loops, rule, solid, pattern))
    }

    /// Orient hatch boundary edges head-to-tail into one closed loop.
    fn stitch_edges(&mut self, edges: Vec<Edge>, ctx: &str) -> Pline {
        let mut verts: Vec<(f64, f64, f64)> = Vec::new();
        let mut end: Option<Point> = None;
        let mut gap = false;
        let eps = 1e-6;
        for mut e in edges {
            if let Some(prev_end) = end
                && prev_end.distance(e.start()) > eps
            {
                if prev_end.distance(e.end()) <= eps {
                    e = e.reversed();
                } else {
                    gap = true;
                }
            }
            verts.extend(e.verts.iter().copied());
            end = Some(e.end());
        }
        if gap {
            self.diag(
                Diagnostic::warning(
                    "dxf.hatch_edge_gap",
                    "HATCH boundary edges do not connect; loop closed as read",
                )
                .with_context(ctx.to_string()),
            );
        }
        pline::from_bulge_vertices(verts, true)
    }

    fn insert(
        &mut self,
        raw: &RawEntity,
        xf: &Xf,
        layer: &str,
        depth: usize,
        out: &mut Vec<(String, Primitive)>,
    ) {
        let ctx = format!("INSERT {}", raw.handle().unwrap_or("?"));
        let Some(name) = raw.first(2) else {
            self.malformed(raw, "INSERT without a block name");
            return;
        };
        if depth >= MAX_INSERT_DEPTH {
            self.diag(
                Diagnostic::error(
                    "dxf.insert_depth",
                    format!("block references nested deeper than {MAX_INSERT_DEPTH} (cycle?)"),
                )
                .with_context(ctx),
            );
            return;
        }
        let Some(block) = self.blocks.get(name) else {
            self.diag(
                Diagnostic::error(
                    "dxf.block_missing",
                    format!("INSERT references missing block {name:?}"),
                )
                .with_context(ctx),
            );
            return;
        };
        let at = raw.point(10).unwrap_or(Point::new(0.0, 0.0));
        let sx = raw.f(41).unwrap_or(1.0);
        let sy = raw.f(42).unwrap_or(1.0);
        let rot = raw.f(50).unwrap_or(0.0).to_radians();
        if (sx.abs() - sy.abs()).abs() > 1e-9 * sx.abs().max(sy.abs()) {
            self.diag(
                Diagnostic::warning(
                    "dxf.nonuniform_scale",
                    format!("INSERT {name:?} scaled non-uniformly ({sx} x {sy}); arcs flattened"),
                )
                .with_context(ctx.clone()),
            );
        }
        let cols = raw.i(70).unwrap_or(1).max(1);
        let rows = raw.i(71).unwrap_or(1).max(1);
        let col_sp = raw.f(44).unwrap_or(0.0);
        let row_sp = raw.f(45).unwrap_or(0.0);
        for r in 0..rows {
            for c in 0..cols {
                let local = Xf::translate(-block.base.x, -block.base.y)
                    .then(&Xf::scale(sx, sy))
                    .then(&Xf::translate(c as f64 * col_sp, r as f64 * row_sp))
                    .then(&Xf::rotate(rot))
                    .then(&Xf::translate(at.x, at.y))
                    .then(xf);
                for e in &block.entities {
                    self.convert(e, &local, Some(layer), depth + 1, out);
                }
            }
        }
    }
}

/// A hatch boundary edge as bulge vertices from its start to (excluding) its end.
struct Edge {
    verts: Vec<(f64, f64, f64)>,
    end: Point,
}

impl Edge {
    fn line(a: Point, b: Point) -> Edge {
        Edge {
            verts: vec![(a.x, a.y, 0.0)],
            end: b,
        }
    }

    fn arc(arc: &Arc) -> Edge {
        let mut p = Pline::new();
        pline::add_bulge_segment(&mut p, arc.start(), arc.end(), (arc.sweep / 4.0).tan());
        Edge {
            verts: p.iter_vertexes().map(|v| (v.x, v.y, v.bulge)).collect(),
            end: arc.end(),
        }
    }

    fn polyline(mut pts: Vec<Point>) -> Edge {
        let end = pts.pop().unwrap_or(Point::new(0.0, 0.0));
        Edge {
            verts: pts.iter().map(|p| (p.x, p.y, 0.0)).collect(),
            end,
        }
    }

    fn start(&self) -> Point {
        self.verts
            .first()
            .map(|v| Point::new(v.0, v.1))
            .unwrap_or(self.end)
    }

    fn end(&self) -> Point {
        self.end
    }

    fn reversed(&self) -> Edge {
        // Points in reverse; each segment's bulge moves to its new start vertex, negated.
        let mut pts: Vec<Point> = self.verts.iter().map(|v| Point::new(v.0, v.1)).collect();
        pts.push(self.end);
        let bulges: Vec<f64> = self.verts.iter().map(|v| v.2).collect();
        pts.reverse();
        let n = pts.len();
        let mut verts = Vec::with_capacity(n - 1);
        for i in 0..n - 1 {
            // Segment i in the reversed list corresponds to original segment n-2-i.
            verts.push((pts[i].x, pts[i].y, -bulges[n - 2 - i]));
        }
        Edge {
            verts,
            end: pts[n - 1],
        }
    }
}

fn collect_points(pairs: &[Pair], xcode: i32) -> Vec<Point> {
    let mut pts = Vec::new();
    let mut x: Option<f64> = None;
    for p in pairs {
        if p.code == xcode {
            x = p.value.parse().ok();
        } else if p.code == xcode + 10
            && let Some(xv) = x.take()
            && let Ok(yv) = p.value.parse::<f64>()
        {
            pts.push(Point::new(xv, yv));
        }
    }
    pts
}

/// Sequential reader over an entity's pairs (HATCH boundary data is order-dependent).
struct Cursor<'a> {
    pairs: &'a [Pair],
    i: usize,
}

impl Cursor<'_> {
    fn seek(&mut self, code: i32) -> Option<f64> {
        while self.i < self.pairs.len() {
            let p = &self.pairs[self.i];
            self.i += 1;
            if p.code == code {
                return p.value.parse().ok();
            }
        }
        None
    }

    fn expect(&mut self, code: i32) -> Option<f64> {
        let p = self.pairs.get(self.i)?;
        if p.code != code {
            return None;
        }
        self.i += 1;
        p.value.parse().ok()
    }

    fn take_if(&mut self, code: i32) -> Option<f64> {
        let p = self.pairs.get(self.i)?;
        if p.code != code {
            return None;
        }
        self.i += 1;
        p.value.parse().ok()
    }
}

// ---------------------------------------------------------------------------------------
// Entry points

pub fn read_bytes(bytes: &[u8], opts: &ReadOptions) -> Result<DxfDocument, DxfReadError> {
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err(DxfReadError::BinaryDxf);
    }
    read_str(&String::from_utf8_lossy(bytes), opts)
}

pub fn read_str(text: &str, opts: &ReadOptions) -> Result<DxfDocument, DxfReadError> {
    let pairs = tokenize(text)?;
    let secs = sections(&pairs)?;
    let mut diagnostics = Vec::new();

    // Header: version and units.
    let header = secs
        .iter()
        .find(|s| s.name == "HEADER")
        .map(|s| header_variables(s.pairs))
        .unwrap_or_default();
    let version = header
        .get("$ACADVER")
        .and_then(|v| v.first())
        .map(|p| p.value.clone());
    let insunits = header
        .get("$INSUNITS")
        .and_then(|v| v.iter().find(|p| p.code == 70))
        .and_then(|p| p.value.parse::<i64>().ok());
    let units = match insunits {
        None => {
            diagnostics.push(Diagnostic::warning(
                "dxf.units_missing",
                format!(
                    "file declares no units ($INSUNITS); assuming {:?}",
                    opts.assume_units
                ),
            ));
            None
        }
        Some(code) => match Unit::from_dxf_insunits(code) {
            Ok(Some(u)) => Some(u),
            Ok(None) => {
                diagnostics.push(Diagnostic::warning(
                    "dxf.units_missing",
                    format!(
                        "file is unitless ($INSUNITS 0); assuming {:?}",
                        opts.assume_units
                    ),
                ));
                None
            }
            Err(code) => {
                diagnostics.push(Diagnostic::warning(
                    "dxf.units_unknown",
                    format!(
                        "unsupported $INSUNITS code {code}; assuming {:?}",
                        opts.assume_units
                    ),
                ));
                None
            }
        },
    };
    let scale = units.unwrap_or(opts.assume_units).mm_per_unit();

    // Layer table.
    let mut layer_colors: BTreeMap<String, Option<i16>> = BTreeMap::new();
    if let Some(tables) = secs.iter().find(|s| s.name == "TABLES") {
        for e in group_entities(tables.pairs) {
            if e.kind == "LAYER"
                && let Some(name) = e.first(2)
            {
                let color = e.i(62).map(|c| c.unsigned_abs() as i16);
                layer_colors.insert(name.to_string(), color);
            }
        }
    }

    // Blocks.
    let mut blocks: HashMap<String, Block> = HashMap::new();
    if let Some(bsec) = secs.iter().find(|s| s.name == "BLOCKS") {
        let ents = group_entities(bsec.pairs);
        let mut i = 0;
        while i < ents.len() {
            if ents[i].kind == "BLOCK" {
                let name = ents[i].first(2).unwrap_or("").to_string();
                let base = ents[i].point(10).unwrap_or(Point::new(0.0, 0.0));
                let mut j = i + 1;
                let mut entities = Vec::new();
                while j < ents.len() && ents[j].kind != "ENDBLK" {
                    entities.push(ents[j].clone());
                    j += 1;
                }
                blocks.insert(name, Block { base, entities });
                i = j + 1;
            } else {
                i += 1;
            }
        }
    }

    // Entities.
    let mut conv = Converter {
        opts,
        blocks: &blocks,
        diagnostics: Vec::new(),
        unsupported: BTreeMap::new(),
        paperspace_skipped: 0,
    };
    let mut placed: Vec<(String, Primitive)> = Vec::new();
    if let Some(esec) = secs.iter().find(|s| s.name == "ENTITIES") {
        for e in group_entities(esec.pairs) {
            conv.convert(&e, &Xf::IDENTITY, None, 0, &mut placed);
        }
    } else {
        diagnostics.push(Diagnostic::warning(
            "dxf.no_entities",
            "file has no ENTITIES section",
        ));
    }

    // Unsupported summary.
    for (kind, (count, layers)) in &conv.unsupported {
        let layers: Vec<&str> = layers.iter().map(|s| s.as_str()).collect();
        let (severity, code, note) = match kind.as_str() {
            "REGION" | "BODY" | "3DSOLID" | "SURFACE" | "ACAD_PROXY_ENTITY" => (
                Severity::Error,
                "dxf.acis",
                "ACIS/solid geometry cannot be read; important features may be missing. Explode or convert to polylines in CAD.",
            ),
            "TEXT" | "MTEXT" | "ATTRIB" | "ATTDEF" | "DIMENSION" | "LEADER" | "MLEADER"
            | "TOLERANCE" => (
                Severity::Info,
                "dxf.text_skipped",
                "text and annotation are not marking geometry; explode to outlines in CAD if they must be marked",
            ),
            "POINT" | "VIEWPORT" | "IMAGE" | "WIPEOUT" | "LIGHT" => (
                Severity::Info,
                "dxf.entity_skipped",
                "entity has no marking geometry",
            ),
            "POLYLINE" => (
                Severity::Warning,
                "dxf.polyline_mesh",
                "3D/mesh/polyface POLYLINE is not supported",
            ),
            _ => (
                Severity::Warning,
                "dxf.unsupported_entity",
                "entity type is not supported and was skipped",
            ),
        };
        conv.diagnostics.push(
            Diagnostic::new(severity, code, format!("{count} x {kind}: {note}"))
                .with_context(format!("layers {}", layers.join(", "))),
        );
    }
    if conv.paperspace_skipped > 0 {
        conv.diagnostics.push(Diagnostic::info(
            "dxf.paperspace_skipped",
            format!("{} paper-space entities skipped", conv.paperspace_skipped),
        ));
    }
    diagnostics.extend(conv.diagnostics);

    // Assemble layers, converting to millimetres.
    let unit_xf = Xf::scale(scale, scale);
    let mut by_name: BTreeMap<String, SourceLayer> = layer_colors
        .iter()
        .map(|(name, color)| {
            (
                name.clone(),
                SourceLayer {
                    name: name.clone(),
                    color: *color,
                    primitives: Vec::new(),
                },
            )
        })
        .collect();
    for (layer, prim) in placed {
        // Convert to mm and drop coincident vertices (a zero-length arc is undefined).
        let prim = match prim {
            Primitive::Path { pline, width } => {
                match pline::sanitize(&transform_pline(&pline, &unit_xf, opts.tolerance_mm)) {
                    Some(pline) => Primitive::Path {
                        pline,
                        width: width.map(|w| w * scale),
                    },
                    None => continue,
                }
            }
            Primitive::Hatch {
                loops,
                rule,
                solid,
                pattern,
            } => Primitive::Hatch {
                loops: loops
                    .iter()
                    .filter_map(|l| {
                        pline::sanitize(&transform_pline(l, &unit_xf, opts.tolerance_mm))
                    })
                    .collect(),
                rule,
                solid,
                pattern,
            },
        };
        by_name
            .entry(layer.clone())
            .or_insert_with(|| SourceLayer {
                name: layer,
                color: None,
                primitives: Vec::new(),
            })
            .primitives
            .push(prim);
    }

    Ok(DxfDocument {
        version,
        units,
        layers: by_name.into_values().collect(),
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostics;
    use crate::region::Region;
    use std::f64::consts::PI;

    /// Minimal DXF around an ENTITIES body; `header` adds `9/$NAME ...` pairs.
    fn dxf(header: &str, blocks: &str, entities: &str) -> String {
        format!(
            "  0\nSECTION\n  2\nHEADER\n{header}  0\nENDSEC\n  0\nSECTION\n  2\nBLOCKS\n{blocks}  0\nENDSEC\n  0\nSECTION\n  2\nENTITIES\n{entities}  0\nENDSEC\n  0\nEOF\n"
        )
    }

    fn mm() -> &'static str {
        "  9\n$INSUNITS\n 70\n4\n"
    }

    fn read(text: &str) -> DxfDocument {
        read_str(text, &ReadOptions::default()).unwrap()
    }

    fn paths<'a>(doc: &'a DxfDocument, layer: &str) -> Vec<(&'a Pline, Option<f64>)> {
        doc.layer(layer)
            .map(|l| {
                l.primitives
                    .iter()
                    .filter_map(|p| match p {
                        Primitive::Path { pline, width } => Some((pline, *width)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn basic_entities() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nLINE\n  8\nA\n 10\n0\n 20\n0\n 11\n10\n 21\n5\n  0\nCIRCLE\n  8\nA\n 10\n20\n 20\n0\n 40\n2\n  0\nARC\n  8\nB\n 10\n0\n 20\n0\n 40\n1\n 50\n0\n 51\n270\n",
        ));
        assert_eq!(doc.units, Some(Unit::Millimetre));
        assert!(!doc.diagnostics.has_errors());
        let a = paths(&doc, "A");
        assert_eq!(a.len(), 2);
        assert!(!a[0].0.is_closed() && a[1].0.is_closed());
        assert!((a[1].0.area() - PI * 4.0).abs() < 1e-9);
        let b = paths(&doc, "B");
        assert_eq!(b.len(), 1);
        let arc = b[0].0;
        assert!(!arc.is_closed());
        assert_eq!(
            arc.vertex_count(),
            3,
            "270 degree arc split into two segments"
        );
        assert!((pline::length(arc) - 1.5 * PI).abs() < 1e-9);
    }

    #[test]
    fn lwpolyline_with_bulge_and_width() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nLWPOLYLINE\n  8\nT\n 90\n2\n 70\n0\n 43\n0.3\n 10\n0\n 20\n0\n 42\n1\n 10\n2\n 20\n0\n",
        ));
        let t = paths(&doc, "T");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1, Some(0.3));
        assert!(!t[0].0.is_closed());
        assert!((pline::length(t[0].0) - PI).abs() < 1e-9);
    }

    #[test]
    fn heavy_polyline_and_mesh_rejection() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nPOLYLINE\n  8\nP\n 70\n1\n  0\nVERTEX\n  8\nP\n 10\n0\n 20\n0\n  0\nVERTEX\n  8\nP\n 10\n4\n 20\n0\n  0\nVERTEX\n  8\nP\n 10\n4\n 20\n3\n  0\nSEQEND\n  0\nPOLYLINE\n  8\nP\n 70\n64\n  0\nVERTEX\n 10\n0\n 20\n0\n  0\nSEQEND\n",
        ));
        let p = paths(&doc, "P");
        assert_eq!(p.len(), 1);
        assert!(p[0].0.is_closed());
        assert!((p[0].0.area().abs() - 6.0).abs() < 1e-9);
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.polyline_mesh")
        );
    }

    #[test]
    fn ellipse_full_and_partial() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nELLIPSE\n  8\nE\n 10\n0\n 20\n0\n 11\n4\n 21\n0\n 40\n0.5\n 41\n0\n 42\n6.283185307179586\n  0\nELLIPSE\n  8\nE\n 10\n0\n 20\n0\n 11\n4\n 21\n0\n 40\n0.5\n 41\n0\n 42\n1.5707963267948966\n",
        ));
        let e = paths(&doc, "E");
        assert!(e[0].0.is_closed());
        let b = pline::bounds(e[0].0).unwrap();
        assert!((b.width() - 8.0).abs() < 1e-6 && (b.height() - 4.0).abs() < 1e-6);
        // Inscribed polygon at 1 um chordal tolerance: area low by ~perimeter * tol.
        assert!(
            (e[0].0.area() - PI * 4.0 * 2.0).abs() < 0.03,
            "area {}",
            e[0].0.area()
        );
        assert!(!e[1].0.is_closed());
        let last = e[1].0.last().unwrap();
        assert!((last.x).abs() < 1e-6 && (last.y - 2.0).abs() < 1e-6);
    }

    #[test]
    fn spline_through_control_polygon() {
        // Clamped cubic Bezier as a NURBS: knots 0,0,0,0,1,1,1,1.
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nSPLINE\n  8\nS\n 70\n8\n 71\n3\n 72\n8\n 73\n4\n 40\n0\n 40\n0\n 40\n0\n 40\n0\n 40\n1\n 40\n1\n 40\n1\n 40\n1\n 10\n0\n 20\n0\n 10\n1\n 20\n2\n 10\n3\n 20\n2\n 10\n4\n 20\n0\n",
        ));
        let s = paths(&doc, "S");
        assert_eq!(s.len(), 1);
        let p = s[0].0;
        assert!(!p.is_closed());
        assert!(p.vertex_count() > 8);
        assert!(p.at(0).pos().x.abs() < 1e-9);
        let last = p.last().unwrap();
        assert!((last.x - 4.0).abs() < 1e-9 && last.y.abs() < 1e-9);
        // Bezier midpoint (t=0.5) is (2, 1.5).
        let mid = p
            .iter_vertexes()
            .map(|v| Point::new(v.x, v.y))
            .min_by(|a, b| (a.x - 2.0).abs().total_cmp(&(b.x - 2.0).abs()))
            .unwrap();
        assert!((mid.y - 1.5).abs() < 0.01);
    }

    #[test]
    fn hatch_polyline_and_edge_boundaries() {
        // Outer: polyline square 0..10 with no bulges. Inner: edge path circle-ish made of
        // two half arcs (r=2 around (5,5)) -> even-odd hole.
        let hatch = "  0\nHATCH\n  8\nH\n  2\nSOLID\n 70\n1\n 71\n0\n 91\n2\n 92\n3\n 72\n0\n 73\n1\n 93\n4\n 10\n0\n 20\n0\n 10\n10\n 20\n0\n 10\n10\n 20\n10\n 10\n0\n 20\n10\n 97\n0\n 92\n0\n 93\n2\n 72\n2\n 10\n5\n 20\n5\n 40\n2\n 50\n0\n 51\n180\n 73\n1\n 72\n2\n 10\n5\n 20\n5\n 40\n2\n 50\n180\n 51\n360\n 73\n1\n 97\n0\n 75\n0\n 76\n1\n 98\n0\n";
        let doc = read(&dxf(mm(), "", hatch));
        let h = &doc.layer("H").unwrap().primitives;
        assert_eq!(h.len(), 1);
        let Primitive::Hatch {
            loops, rule, solid, ..
        } = &h[0]
        else {
            panic!("expected hatch");
        };
        assert!(*solid);
        assert_eq!(*rule, LoopRule::EvenOdd);
        assert_eq!(loops.len(), 2);
        let r = Region::from_loops(loops.clone(), *rule);
        assert_eq!(r.holes().count(), 1);
        assert!(
            (r.area() - (100.0 - PI * 4.0)).abs() < 1e-9,
            "area {}",
            r.area()
        );
        assert!(
            !doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.hatch_edge_gap")
        );
    }

    #[test]
    fn inserts_expand_with_transforms_and_layer_inheritance() {
        let blocks = "  0\nBLOCK\n  8\n0\n  2\nPAD\n 70\n0\n 10\n0\n 20\n0\n  0\nCIRCLE\n  8\n0\n 10\n0\n 20\n0\n 40\n0.5\n  0\nLWPOLYLINE\n  8\nKEEP\n 90\n4\n 70\n1\n 10\n-1\n 20\n-1\n 10\n1\n 20\n-1\n 10\n1\n 20\n1\n 10\n-1\n 20\n1\n  0\nENDBLK\n";
        let ents = "  0\nINSERT\n  8\nPADS\n  2\nPAD\n 10\n10\n 20\n0\n 41\n2\n 42\n2\n 43\n2\n 50\n45\n  0\nINSERT\n  8\nPADS\n  2\nPAD\n 10\n0\n 20\n0\n 41\n1\n 42\n2\n  0\nINSERT\n  8\nPADS\n  2\nMISSING\n 10\n0\n 20\n0\n  0\nINSERT\n  8\nGRID\n  2\nPAD\n 10\n100\n 20\n100\n 70\n2\n 71\n3\n 44\n10\n 45\n10\n";
        let doc = read(&dxf(mm(), blocks, ents));
        // Layer 0 entities inherit PADS; KEEP keeps its own layer. Two plain inserts plus a
        // 2x3 MINSERT grid (on layer GRID) make 8 block instances in total.
        let pads = paths(&doc, "PADS");
        assert_eq!(pads.len(), 2, "two circles from the two plain inserts");
        assert_eq!(
            paths(&doc, "GRID").len(),
            6,
            "six circles from the 2x3 grid"
        );
        assert_eq!(
            paths(&doc, "KEEP").len(),
            8,
            "every instance's square keeps layer KEEP"
        );
        // First insert: uniform scale 2 and 45 degrees -> circle radius 1 with arcs intact.
        let c = pads
            .iter()
            .find(|(p, _)| p.is_closed() && p.iter_vertexes().any(|v| v.bulge != 0.0))
            .unwrap();
        assert!((c.0.area() - PI).abs() < 1e-9);
        // Non-uniform insert: circle flattened (no bulges) and a warning.
        assert!(
            pads.iter()
                .any(|(p, _)| p.iter_vertexes().all(|v| v.bulge == 0.0))
        );
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.nonuniform_scale")
        );
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.block_missing" && d.severity == Severity::Error)
        );
    }

    #[test]
    fn mirrored_ocs_flips_x() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nCIRCLE\n  8\nM\n 10\n5\n 20\n1\n 40\n1\n210\n0\n220\n0\n230\n-1\n",
        ));
        let m = paths(&doc, "M");
        let b = pline::bounds(m[0].0).unwrap();
        assert!((b.min.x + 6.0).abs() < 1e-9 && (b.max.x + 4.0).abs() < 1e-9);
        // Mirroring flips the winding; the reader keeps source direction (passthrough
        // needs it), so the area is -pi here.
        assert!((m[0].0.area().abs() - PI).abs() < 1e-9);
        assert!(m[0].0.area() < 0.0);
    }

    #[test]
    fn units_are_converted_and_diagnosed() {
        let inch = read(&dxf(
            "  9\n$INSUNITS\n 70\n1\n",
            "",
            "  0\nLINE\n  8\nA\n 10\n0\n 20\n0\n 11\n1\n 21\n0\n",
        ));
        assert_eq!(inch.units, Some(Unit::Inch));
        assert!((pline::length(paths(&inch, "A")[0].0) - 25.4).abs() < 1e-9);
        let none = read(&dxf(
            "",
            "",
            "  0\nLINE\n  8\nA\n 10\n0\n 20\n0\n 11\n1\n 21\n0\n",
        ));
        assert_eq!(none.units, None);
        assert!(
            none.diagnostics
                .iter()
                .any(|d| d.code == "dxf.units_missing")
        );
        let feet = read(&dxf("  9\n$INSUNITS\n 70\n2\n", "", ""));
        assert!(
            feet.diagnostics
                .iter()
                .any(|d| d.code == "dxf.units_unknown")
        );
    }

    #[test]
    fn unsupported_entities_are_reported_not_dropped_silently() {
        let doc = read(&dxf(
            mm(),
            "",
            "  0\nREGION\n  8\nR\n  0\nTEXT\n  8\nT\n 10\n0\n 20\n0\n  1\nhi\n  0\nSOLID\n  8\nS\n 10\n0\n 20\n0\n 11\n2\n 21\n0\n 12\n0\n 22\n1\n 13\n2\n 23\n1\n  0\nFROB\n  8\nX\n",
        ));
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.acis" && d.severity == Severity::Error)
        );
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.text_skipped" && d.severity == Severity::Info)
        );
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "dxf.unsupported_entity" && d.message.contains("FROB"))
        );
        let s = &doc.layer("S").unwrap().primitives;
        let Primitive::Hatch { loops, .. } = &s[0] else {
            panic!()
        };
        assert!((loops[0].area().abs() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn tokenizer_errors() {
        assert_eq!(
            read_str("", &ReadOptions::default()).unwrap_err(),
            DxfReadError::Empty
        );
        assert!(matches!(
            read_str("abc\nSECTION\n", &ReadOptions::default()).unwrap_err(),
            DxfReadError::Malformed { line: 1, .. }
        ));
        assert_eq!(
            read_bytes(b"AutoCAD Binary DXF\r\n\x1a\0", &ReadOptions::default()).unwrap_err(),
            DxfReadError::BinaryDxf
        );
    }
}
