//! Vector PDF reader (D22): `lopdf` supplies the object model (xref tables and streams,
//! filters, page tree); the content-stream interpreter here is ours and covers the
//! documented subset: path construction (`m l c v y h re`), painting (`S s f F f* B B* b b*
//! n`), clipping (`W W*`) applied to fills, graphics state (`q Q cm w d`), device colours
//! (`g rg k` and the `sc`/`scn` forms), form XObjects with their matrices, and optional
//! content (`BDC /OC … EMC`) as layers. Text, images, shadings and pattern paints are
//! reported, never turned into geometry.
//!
//! Layers: an optional-content group's name when the geometry sits inside one, otherwise
//! the paint colour as `#rrggbb`, which makes a colour-separated PDF (this app's own
//! export, for instance) come back as one layer per output group.

use std::collections::BTreeMap;

use cavalier_contours::polyline::PlineSource;
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::diag::Diagnostic;
use crate::flatten::flatten_cubic;
use crate::geom::Point;
use crate::model::SourceFormat;
use crate::pline::{self, Pline};
use crate::region::{LoopRule, Region};
use crate::source::{Primitive, ReadError, ReadOptions, SourceDocument, SourceLayer};
use crate::units::Unit;

const MM_PER_PT: f64 = 25.4 / 72.0;
/// Decompression bound per page (bomb protection).
const MAX_CONTENT_BYTES: usize = 256 << 20;
const MAX_FORM_DEPTH: usize = 16;

/// PDF transformation matrix `[a b c d e f]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mat {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Mat {
    const IDENTITY: Mat = Mat {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// `self` applied first, then `m` (PDF `cm`: CTM' = M × CTM).
    fn then(self, m: Mat) -> Mat {
        Mat {
            a: self.a * m.a + self.b * m.c,
            b: self.a * m.b + self.b * m.d,
            c: self.c * m.a + self.d * m.c,
            d: self.c * m.b + self.d * m.d,
            e: self.e * m.a + self.f * m.c + m.e,
            f: self.e * m.b + self.f * m.d + m.f,
        }
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn scale(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

#[derive(Clone)]
struct GState {
    ctm: Mat,
    line_width: f64,
    stroke_rgb: [u8; 3],
    fill_rgb: [u8; 3],
    fill_pattern: bool,
    stroke_pattern: bool,
    dashed: bool,
    clip: Option<Region>,
}

#[derive(Default)]
struct Counts {
    text: usize,
    images: usize,
    shadings: usize,
    pattern_paint: usize,
    dashed: usize,
    zero_width: usize,
    clipped_linework: usize,
    rotated_pages: usize,
    unknown_ops: std::collections::BTreeSet<String>,
    form_depth_exceeded: usize,
}

struct Interp<'a> {
    doc: &'a Document,
    tol_mm: f64,
    /// Device points → millimetres: subtract the media box origin, scale by UserUnit.
    origin_pt: (f64, f64),
    mm_per_pt: f64,
    layer_prefix: String,
    layers: BTreeMap<String, SourceLayer>,
    counts: Counts,
}

fn rgb_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(num).collect()
}

fn clamp8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn colour_from(values: &[f64]) -> Option<[u8; 3]> {
    match values.len() {
        1 => Some([clamp8(values[0]); 3]),
        3 => Some([clamp8(values[0]), clamp8(values[1]), clamp8(values[2])]),
        4 => {
            let (c, m, y, k) = (values[0], values[1], values[2], values[3]);
            Some([
                clamp8((1.0 - c) * (1.0 - k)),
                clamp8((1.0 - m) * (1.0 - k)),
                clamp8((1.0 - y) * (1.0 - k)),
            ])
        }
        _ => None,
    }
}

/// Resolve references.
fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

/// Inherited page attribute (MediaBox, Resources, UserUnit…).
fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Some(v) = dict_get(doc, d, key) {
            return Some(v);
        }
        match d.get(b"Parent").ok() {
            Some(Object::Reference(p)) => id = *p,
            _ => return None,
        }
    }
    None
}

struct PathBuilder {
    /// Subpaths in mm with their closed flag.
    subpaths: Vec<(Vec<Point>, bool)>,
    current: Vec<Point>,
    /// Current point in user space (for `v`/`y` control points), and subpath start.
    cur_user: (f64, f64),
    start_user: (f64, f64),
    pending_clip: Option<LoopRule>,
}

impl PathBuilder {
    fn new() -> Self {
        Self {
            subpaths: Vec::new(),
            current: Vec::new(),
            cur_user: (0.0, 0.0),
            start_user: (0.0, 0.0),
            pending_clip: None,
        }
    }

    fn flush(&mut self, closed: bool) {
        if self.current.len() >= 2 {
            let pts = std::mem::take(&mut self.current);
            self.subpaths.push((pts, closed));
        } else {
            self.current.clear();
        }
    }

    fn take(&mut self) -> Vec<(Vec<Point>, bool)> {
        self.flush(false);
        std::mem::take(&mut self.subpaths)
    }
}

impl Interp<'_> {
    fn to_mm(&self, ctm: &Mat, x: f64, y: f64) -> Point {
        let (dx, dy) = ctm.apply(x, y);
        Point::new(
            (dx - self.origin_pt.0) * self.mm_per_pt,
            (dy - self.origin_pt.1) * self.mm_per_pt,
        )
    }

    fn layer(&mut self, name: &str) -> &mut SourceLayer {
        let full = format!("{}{}", self.layer_prefix, name);
        self.layers
            .entry(full.clone())
            .or_insert_with(|| SourceLayer {
                name: full,
                ..Default::default()
            })
    }

    fn run(
        &mut self,
        content: &[u8],
        resources: Option<&Dictionary>,
        initial: GState,
        depth: usize,
    ) {
        let ops = match Content::decode(content) {
            Ok(c) => c.operations,
            Err(_) => return,
        };
        let mut stack: Vec<GState> = Vec::new();
        let mut gs = initial;
        let mut path = PathBuilder::new();
        let mut layer_stack: Vec<Option<String>> = Vec::new();
        let mut in_text = false;
        let tol_pt = (self.tol_mm / self.mm_per_pt).max(1e-6);
        for op in &ops {
            let a = &op.operands;
            match op.operator.as_str() {
                // ---- graphics state
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(s) = stack.pop() {
                        gs = s;
                    }
                }
                "cm" => {
                    let v = nums(a);
                    if v.len() == 6 {
                        let m = Mat {
                            a: v[0],
                            b: v[1],
                            c: v[2],
                            d: v[3],
                            e: v[4],
                            f: v[5],
                        };
                        gs.ctm = m.then(gs.ctm);
                    }
                }
                "w" => {
                    if let Some(w) = a.first().and_then(num) {
                        gs.line_width = w;
                    }
                }
                "d" => {
                    let dashed = matches!(a.first(), Some(Object::Array(arr)) if !arr.is_empty());
                    if dashed && !gs.dashed {
                        self.counts.dashed += 1;
                    }
                    gs.dashed = dashed;
                }
                "gs" | "J" | "j" | "M" | "ri" | "i" => {}
                // ---- colour
                "g" | "rg" | "k" | "sc" | "scn" => {
                    if a.iter().any(|o| matches!(o, Object::Name(_))) {
                        gs.fill_pattern = true;
                        self.counts.pattern_paint += 1;
                    } else if let Some(c) = colour_from(&nums(a)) {
                        gs.fill_rgb = c;
                        gs.fill_pattern = false;
                    }
                }
                "G" | "RG" | "K" | "SC" | "SCN" => {
                    if a.iter().any(|o| matches!(o, Object::Name(_))) {
                        gs.stroke_pattern = true;
                        self.counts.pattern_paint += 1;
                    } else if let Some(c) = colour_from(&nums(a)) {
                        gs.stroke_rgb = c;
                        gs.stroke_pattern = false;
                    }
                }
                "cs" | "CS" => {}
                // ---- path construction
                "m" => {
                    let v = nums(a);
                    if v.len() == 2 {
                        path.flush(false);
                        path.cur_user = (v[0], v[1]);
                        path.start_user = path.cur_user;
                        let p = self.to_mm(&gs.ctm, v[0], v[1]);
                        path.current.push(p);
                    }
                }
                "l" => {
                    let v = nums(a);
                    if v.len() == 2 {
                        path.cur_user = (v[0], v[1]);
                        let p = self.to_mm(&gs.ctm, v[0], v[1]);
                        path.current.push(p);
                    }
                }
                "c" | "v" | "y" => {
                    let v = nums(a);
                    let (p1, p2, p3) = match (op.operator.as_str(), v.len()) {
                        ("c", 6) => ((v[0], v[1]), (v[2], v[3]), (v[4], v[5])),
                        ("v", 4) => (path.cur_user, (v[0], v[1]), (v[2], v[3])),
                        ("y", 4) => ((v[0], v[1]), (v[2], v[3]), (v[2], v[3])),
                        _ => continue,
                    };
                    let p0 = path.cur_user;
                    // Flatten in device points so the tolerance is physical.
                    let dev = |p: (f64, f64)| {
                        let (x, y) = gs.ctm.apply(p.0, p.1);
                        Point::new(x, y)
                    };
                    let (origin, mmpp) = (self.origin_pt, self.mm_per_pt);
                    let mut pts = Vec::new();
                    flatten_cubic(dev(p0), dev(p1), dev(p2), dev(p3), tol_pt, &mut |q| {
                        pts.push(q)
                    });
                    for q in pts {
                        path.current
                            .push(Point::new((q.x - origin.0) * mmpp, (q.y - origin.1) * mmpp));
                    }
                    path.cur_user = p3;
                }
                "h" => {
                    path.flush(true);
                    path.cur_user = path.start_user;
                }
                "re" => {
                    let v = nums(a);
                    if v.len() == 4 {
                        path.flush(false);
                        let (x, y, w, h) = (v[0], v[1], v[2], v[3]);
                        let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                        let pts: Vec<Point> = corners
                            .iter()
                            .map(|(px, py)| self.to_mm(&gs.ctm, *px, *py))
                            .collect();
                        path.subpaths.push((pts, true));
                        path.cur_user = (x, y);
                        path.start_user = (x, y);
                    }
                }
                // ---- clipping
                "W" => path.pending_clip = Some(LoopRule::NonZero),
                "W*" => path.pending_clip = Some(LoopRule::EvenOdd),
                // ---- painting
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    let o = op.operator.as_str();
                    if matches!(o, "s" | "b" | "b*") {
                        path.flush(true);
                    }
                    let subpaths = path.take();
                    let fill_rule = match o {
                        "f" | "F" | "B" | "b" => Some(LoopRule::NonZero),
                        "f*" | "B*" | "b*" => Some(LoopRule::EvenOdd),
                        _ => None,
                    };
                    let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                    if !in_text && !subpaths.is_empty() {
                        let layer_name = layer_stack.iter().rev().flatten().next().cloned();
                        if let Some(rule) = fill_rule {
                            self.emit_fill(&subpaths, rule, &gs, layer_name.as_deref());
                        }
                        if stroke {
                            self.emit_stroke(&subpaths, &gs, layer_name.as_deref());
                        }
                    }
                    if let Some(rule) = path.pending_clip.take() {
                        let loops: Vec<Pline> = subpaths
                            .iter()
                            .filter(|(pts, _)| pts.len() >= 3)
                            .map(|(pts, _)| pline::from_points(pts.iter().copied(), true))
                            .collect();
                        let r = Region::from_loops(loops, rule);
                        gs.clip = Some(match gs.clip.take() {
                            Some(c) => c.intersection(&r),
                            None => r,
                        });
                    }
                }
                // ---- text
                "BT" => {
                    in_text = true;
                    self.counts.text += 1;
                }
                "ET" => in_text = false,
                "Tj" | "TJ" | "'" | "\"" | "Tf" | "Td" | "TD" | "Tm" | "T*" | "Tc" | "Tw"
                | "Tz" | "TL" | "Tr" | "Ts" => {}
                // ---- XObjects, images, shadings
                "Do" => {
                    if let Some(Object::Name(name)) = a.first() {
                        self.do_xobject(name, resources, &gs, depth);
                    }
                }
                "BI" | "ID" | "EI" => {
                    if op.operator == "BI" {
                        self.counts.images += 1;
                    }
                }
                "sh" => self.counts.shadings += 1,
                // ---- marked content (optional content = layers)
                "BDC" => {
                    let mut name = None;
                    if let (Some(Object::Name(tag)), Some(prop)) = (a.first(), a.get(1))
                        && tag.as_slice() == b"OC"
                    {
                        name = self.oc_name(prop, resources);
                    }
                    layer_stack.push(name);
                }
                "BMC" => layer_stack.push(None),
                "EMC" => {
                    layer_stack.pop();
                }
                "MP" | "DP" | "BX" | "EX" | "d0" | "d1" => {}
                other => {
                    self.counts.unknown_ops.insert(other.to_string());
                }
            }
        }
    }

    fn oc_name(&self, prop: &Object, resources: Option<&Dictionary>) -> Option<String> {
        let ocg = match prop {
            Object::Name(n) => {
                let props = dict_get(self.doc, resources?, b"Properties")?
                    .as_dict()
                    .ok()?;
                dict_get(self.doc, props, n)?
            }
            other => deref(self.doc, other),
        };
        let d = ocg.as_dict().ok()?;
        let name = dict_get(self.doc, d, b"Name")?;
        match name {
            Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
            _ => None,
        }
    }

    fn do_xobject(
        &mut self,
        name: &[u8],
        resources: Option<&Dictionary>,
        gs: &GState,
        depth: usize,
    ) {
        let Some(res) = resources else {
            return;
        };
        let Some(xobjects) = dict_get(self.doc, res, b"XObject").and_then(|o| o.as_dict().ok())
        else {
            return;
        };
        let Some(Object::Stream(stream)) = dict_get(self.doc, xobjects, name) else {
            return;
        };
        let subtype = dict_get(self.doc, &stream.dict, b"Subtype").and_then(|o| o.as_name().ok());
        match subtype {
            Some(b"Image") => self.counts.images += 1,
            Some(b"Form") => {
                if depth >= MAX_FORM_DEPTH {
                    self.counts.form_depth_exceeded += 1;
                    return;
                }
                let mut inner = gs.clone();
                if let Some(Object::Array(m)) = dict_get(self.doc, &stream.dict, b"Matrix") {
                    let v = nums(m);
                    if v.len() == 6 {
                        inner.ctm = Mat {
                            a: v[0],
                            b: v[1],
                            c: v[2],
                            d: v[3],
                            e: v[4],
                            f: v[5],
                        }
                        .then(gs.ctm);
                    }
                }
                if let Some(Object::Array(bb)) = dict_get(self.doc, &stream.dict, b"BBox") {
                    let v = nums(bb);
                    if v.len() == 4 {
                        let corners = [(v[0], v[1]), (v[2], v[1]), (v[2], v[3]), (v[0], v[3])];
                        let pts: Vec<Point> = corners
                            .iter()
                            .map(|(x, y)| self.to_mm(&inner.ctm, *x, *y))
                            .collect();
                        let bbox =
                            Region::from_loops([pline::from_points(pts, true)], LoopRule::NonZero);
                        inner.clip = Some(match inner.clip.take() {
                            Some(c) => c.intersection(&bbox),
                            None => bbox,
                        });
                    }
                }
                let form_res =
                    dict_get(self.doc, &stream.dict, b"Resources").and_then(|o| o.as_dict().ok());
                let Ok(content) = stream.decompressed_content_with_limit(MAX_CONTENT_BYTES) else {
                    return;
                };
                self.run(&content, form_res.or(resources), inner, depth + 1);
            }
            _ => {}
        }
    }

    fn emit_fill(
        &mut self,
        subpaths: &[(Vec<Point>, bool)],
        rule: LoopRule,
        gs: &GState,
        layer: Option<&str>,
    ) {
        let loops: Vec<Pline> = subpaths
            .iter()
            .filter(|(pts, _)| pts.len() >= 3)
            .filter_map(|(pts, _)| pline::sanitize(&pline::from_points(pts.iter().copied(), true)))
            .filter(|l| l.area().abs() > 1e-12)
            .collect();
        if loops.is_empty() {
            return;
        }
        let name = layer
            .map(str::to_string)
            .unwrap_or_else(|| rgb_hex(gs.fill_rgb));
        let prim = match &gs.clip {
            Some(c) => {
                let r = Region::from_loops(loops, rule).intersection(c);
                if r.is_empty() {
                    return;
                }
                Primitive::Hatch {
                    loops: r.outers().chain(r.holes()).cloned().collect(),
                    rule: LoopRule::EvenOdd,
                    solid: true,
                    pattern: String::new(),
                }
            }
            None => Primitive::Hatch {
                loops,
                rule,
                solid: true,
                pattern: String::new(),
            },
        };
        self.layer(&name).primitives.push(prim);
    }

    fn emit_stroke(&mut self, subpaths: &[(Vec<Point>, bool)], gs: &GState, layer: Option<&str>) {
        let width = gs.line_width * gs.ctm.scale() * self.mm_per_pt;
        let width = if width > 0.0 {
            Some(width)
        } else {
            self.counts.zero_width += 1;
            None
        };
        if gs.clip.is_some() {
            self.counts.clipped_linework += 1;
        }
        let name = layer
            .map(str::to_string)
            .unwrap_or_else(|| rgb_hex(gs.stroke_rgb));
        for (pts, closed) in subpaths {
            let Some(pl) = pline::sanitize(&pline::from_points(pts.iter().copied(), *closed))
            else {
                continue;
            };
            self.layer(&name)
                .primitives
                .push(Primitive::Path { pline: pl, width });
        }
    }
}

/// PDF text strings: UTF-16BE with BOM, or PDFDocEncoding (treated as Latin-1 here).
fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

pub fn read_bytes(
    name: &str,
    bytes: &[u8],
    opts: &ReadOptions,
) -> Result<SourceDocument, ReadError> {
    let err = |message: String| ReadError {
        format: Some(SourceFormat::Pdf),
        message,
    };
    let doc = Document::load_mem(bytes).map_err(|e| err(format!("{name}: {e}")))?;
    if doc.is_encrypted() {
        return Err(err(
            "encrypted PDFs are not supported; export without a password".into(),
        ));
    }
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err(err("the PDF has no pages".into()));
    }
    let multi = pages.len() > 1;
    let mut layers: BTreeMap<String, SourceLayer> = BTreeMap::new();
    let mut counts = Counts::default();
    let mut diags = Vec::new();
    for (n, page_id) in &pages {
        let media = inherited(&doc, *page_id, b"MediaBox")
            .and_then(|o| o.as_array().ok())
            .map(|a| nums(a))
            .filter(|v| v.len() == 4)
            .unwrap_or_else(|| vec![0.0, 0.0, 612.0, 792.0]);
        let user_unit = inherited(&doc, *page_id, b"UserUnit")
            .and_then(num)
            .unwrap_or(1.0);
        if let Some(r) = inherited(&doc, *page_id, b"Rotate").and_then(num)
            && r.rem_euclid(360.0) != 0.0
        {
            counts.rotated_pages += 1;
        }
        let resources = inherited(&doc, *page_id, b"Resources").and_then(|o| o.as_dict().ok());
        let content = match doc.get_page_content_with_limit(*page_id, MAX_CONTENT_BYTES) {
            Ok(c) => c,
            Err(e) => {
                diags.push(Diagnostic::error(
                    "pdf.content",
                    format!("page {n}: cannot read content: {e}"),
                ));
                continue;
            }
        };
        let mut interp = Interp {
            doc: &doc,
            tol_mm: opts.tolerance_mm,
            origin_pt: (media[0].min(media[2]), media[1].min(media[3])),
            mm_per_pt: MM_PER_PT * user_unit,
            layer_prefix: if multi {
                format!("p{n} ")
            } else {
                String::new()
            },
            layers: std::mem::take(&mut layers),
            counts: std::mem::take(&mut counts),
        };
        let initial = GState {
            ctm: Mat::IDENTITY,
            line_width: 1.0,
            stroke_rgb: [0, 0, 0],
            fill_rgb: [0, 0, 0],
            fill_pattern: false,
            stroke_pattern: false,
            dashed: false,
            clip: None,
        };
        interp.run(&content, resources, initial, 0);
        layers = interp.layers;
        counts = interp.counts;
    }
    if multi {
        diags.push(Diagnostic::info(
            "pdf.pages",
            format!(
                "{} pages; layer names are prefixed with the page number",
                pages.len()
            ),
        ));
    }
    if counts.text > 0 {
        diags.push(Diagnostic::error(
            "pdf.text",
            format!(
                "{} text object(s) are not converted to geometry; outline text before export",
                counts.text
            ),
        ));
    }
    if counts.images > 0 {
        diags.push(Diagnostic::warning(
            "pdf.image",
            format!("{} image(s) ignored", counts.images),
        ));
    }
    if counts.shadings > 0 {
        diags.push(Diagnostic::warning(
            "pdf.shading",
            format!("{} shading(s) ignored", counts.shadings),
        ));
    }
    if counts.pattern_paint > 0 {
        diags.push(Diagnostic::warning(
            "pdf.pattern_paint",
            format!(
                "{} pattern or shading paint(s) read as solid geometry",
                counts.pattern_paint
            ),
        ));
    }
    if counts.clipped_linework > 0 {
        diags.push(Diagnostic::warning(
            "pdf.clip_linework",
            format!(
                "clipping was applied to fills but not to {} stroked path(s)",
                counts.clipped_linework
            ),
        ));
    }
    if counts.rotated_pages > 0 {
        diags.push(Diagnostic::warning(
            "pdf.rotate",
            "page rotation (/Rotate) is ignored; geometry is read unrotated",
        ));
    }
    if counts.dashed > 0 {
        diags.push(Diagnostic::info(
            "pdf.dash",
            format!(
                "{} dash pattern(s) ignored; strokes are read solid",
                counts.dashed
            ),
        ));
    }
    if counts.zero_width > 0 {
        diags.push(Diagnostic::info(
            "pdf.zero_width",
            format!(
                "{} zero-width stroke(s) read as centrelines without width",
                counts.zero_width
            ),
        ));
    }
    if counts.form_depth_exceeded > 0 {
        diags.push(Diagnostic::warning(
            "pdf.form_depth",
            "form XObjects nested deeper than 16 levels were skipped",
        ));
    }
    if !counts.unknown_ops.is_empty() {
        diags.push(Diagnostic::info(
            "pdf.unknown_operator",
            format!(
                "unknown content operators ignored: {}",
                counts
                    .unknown_ops
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ));
    }
    let layers: Vec<SourceLayer> = layers.into_values().collect();
    if layers.is_empty() {
        diags.push(Diagnostic::warning(
            "pdf.empty",
            "the PDF contains no vector geometry",
        ));
    }
    Ok(SourceDocument {
        format: SourceFormat::Pdf,
        version: Some(doc.version.clone()),
        units: Some(Unit::Point),
        layers,
        diagnostics: diags,
    })
}

#[cfg(test)]
pub(crate) mod testpdf {
    //! Build small PDFs for tests from raw content streams.
    use lopdf::{Document, Object, Stream, dictionary};

    pub struct Page {
        pub media_box: [f64; 4],
        pub content: String,
        pub extra: Vec<(&'static str, Object)>,
        pub resources: Option<lopdf::Dictionary>,
    }

    pub fn build(pages: Vec<Page>, catalog_extra: Vec<(&'static str, Object)>) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for p in pages {
            let content_id = doc.add_object(Stream::new(dictionary! {}, p.content.into_bytes()));
            let mut d = dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "MediaBox" => Object::Array(p.media_box.iter().map(|v| Object::Real(*v as f32)).collect()),
            };
            if let Some(r) = p.resources {
                d.set("Resources", Object::Dictionary(r));
            }
            for (k, v) in p.extra {
                d.set(k, v);
            }
            kids.push(Object::Reference(doc.add_object(d)));
        }
        let count = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => Object::Array(kids), "Count" => count },
            ),
        );
        let mut cat = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
        for (k, v) in catalog_extra {
            cat.set(k, v);
        }
        let catalog_id = doc.add_object(cat);
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    pub fn page(media: [f64; 4], content: &str) -> Page {
        Page {
            media_box: media,
            content: content.to_string(),
            extra: Vec::new(),
            resources: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testpdf::{Page, build, page};
    use super::*;
    use lopdf::{Object, Stream, dictionary};

    fn read(bytes: &[u8]) -> SourceDocument {
        read_bytes("t.pdf", bytes, &ReadOptions::default()).unwrap()
    }

    fn region_of(l: &SourceLayer) -> Region {
        let mut r = Region::empty();
        for p in &l.primitives {
            if let Primitive::Hatch { loops, rule, .. } = p {
                let part = Region::from_loops(loops.iter().cloned(), *rule);
                r = if r.is_empty() { part } else { r.union(&part) };
            }
        }
        r
    }

    #[test]
    fn strokes_and_fills_land_in_colour_layers_in_millimetres() {
        // 72 pt = 25.4 mm. Blue 2 pt stroke, red 36 pt square, black even-odd ring.
        let bytes = build(
            vec![page(
                [0.0, 0.0, 288.0, 144.0],
                "0 0 1 RG 2 w 36 36 m 180 36 l S\n1 0 0 rg 72 72 36 36 re f\n0 g 200 20 m 272 20 l 272 92 l 200 92 l h 220 40 m 252 40 l 252 72 l 220 72 l h f*",
            )],
            vec![],
        );
        let doc = read(&bytes);
        assert_eq!(doc.format, SourceFormat::Pdf);
        assert_eq!(doc.units, Some(Unit::Point));
        assert_eq!(doc.version.as_deref(), Some("1.5"));
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["#000000", "#0000ff", "#ff0000"]);
        let Primitive::Path { pline, width } = &doc.layers[1].primitives[0] else {
            panic!()
        };
        assert!((width.unwrap() - 2.0 * MM_PER_PT).abs() < 1e-9);
        assert!((pline::length(pline) - 144.0 * MM_PER_PT).abs() < 1e-9);
        let red = region_of(&doc.layers[2]);
        assert!((red.area() - (36.0 * MM_PER_PT).powi(2)).abs() < 1e-9);
        let ring = region_of(&doc.layers[0]);
        assert!((ring.area() - (72.0 * 72.0 - 32.0 * 32.0) * MM_PER_PT * MM_PER_PT).abs() < 1e-9);
        assert_eq!(ring.holes().count(), 1);
    }

    #[test]
    fn transforms_beziers_clipping_and_forms() {
        // Form XObject: a unit square scaled by its Matrix; drawn twice with cm.
        let mut doc_pages = Vec::new();
        let form = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]),
                "Matrix" => Object::Array(vec![Object::Real(10.0), 0.into(), 0.into(), Object::Real(10.0), 0.into(), 0.into()]),
            },
            b"0 0 1 1 re f".to_vec(),
        );
        let mut doc = Document::with_version("1.5");
        let form_id = doc.add_object(form);
        let resources = dictionary! { "XObject" => dictionary! { "Sq" => form_id } };
        // Content: translate then draw the form; a quarter-circle Bézier stroke; a clipped
        // fill: rect 0..72 clipped to x >= 36.
        let content = format!(
            "q 1 0 0 1 20 20 cm /Sq Do Q q 1 0 0 1 50 20 cm /Sq Do Q\n\
             q 0.5 w 100 20 m 100 {k} {k2} 60 60 60 c S Q\n\
             q 36 100 100 44 re W n 0 100 72 44 re f Q",
            k = 20.0 + 0.552_284_75 * 40.0,
            k2 = 60.0 + 0.552_284_75 * 40.0
        );
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 200.into(), 150.into()]),
            "Resources" => resources,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
            ),
        );
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        doc_pages.push(bytes);

        let d = read(&doc_pages[0]);
        assert!(d.diagnostics.is_empty(), "{:?}", d.diagnostics);
        let black = d.layers.iter().find(|l| l.name == "#000000").unwrap();
        let fills = region_of(black);
        // Two 10 pt squares and the clipped rect (36 x 44 pt).
        let want = (2.0 * 100.0 + 36.0 * 44.0) * MM_PER_PT * MM_PER_PT;
        assert!(
            (fills.area() - want).abs() < 1e-6,
            "{} vs {want}",
            fills.area()
        );
        assert_eq!(fills.outers().count(), 3);
        let stroke = black
            .primitives
            .iter()
            .find_map(|p| match p {
                Primitive::Path { pline, width } => Some((pline, width.unwrap())),
                _ => None,
            })
            .unwrap();
        assert!((stroke.1 - 0.5 * MM_PER_PT).abs() < 1e-9);
        // Quarter circle of radius 40 pt: length 62.83 pt within the Bézier's 0.03 %.
        assert!(
            (pline::length(stroke.0) - 62.83 * MM_PER_PT).abs() < 0.05,
            "{}",
            pline::length(stroke.0)
        );
    }

    #[test]
    fn optional_content_groups_become_layers_and_oddities_are_diagnosed() {
        let mut doc = Document::with_version("1.6");
        let ocg = doc
            .add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Cut") });
        let img = doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
            vec![0u8],
        ));
        let resources = dictionary! {
            "Properties" => dictionary! { "oc1" => ocg },
            "XObject" => dictionary! { "Im" => img },
        };
        let content = "/OC /oc1 BDC 1 w 10 10 m 60 10 l S EMC\n0 0 1 rg 10 50 20 20 re f\nBT /F1 12 Tf 10 100 Td (hi) Tj ET\n/Im Do\n[3 2] 0 d 0 w 10 120 m 60 120 l S";
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Rotate" => 90,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 100.into(), 150.into()]),
            "Resources" => resources,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
            ),
        );
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let d = read(&bytes);
        let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["#000000", "#0000ff", "Cut"]);
        let codes: Vec<&str> = d.diagnostics.iter().map(|x| x.code).collect();
        for c in [
            "pdf.text",
            "pdf.image",
            "pdf.rotate",
            "pdf.dash",
            "pdf.zero_width",
        ] {
            assert!(codes.contains(&c), "missing {c} in {codes:?}");
        }
        assert!(
            d.diagnostics
                .iter()
                .any(|x| x.code == "pdf.text" && x.severity == crate::diag::Severity::Error)
        );
        // The zero-width dashed line is a centreline without width.
        assert!(matches!(
            d.layers[0].primitives[0],
            Primitive::Path { width: None, .. }
        ));
    }

    #[test]
    fn multi_page_and_user_unit_and_media_box_origin() {
        let bytes = build(
            vec![
                Page {
                    media_box: [100.0, 100.0, 172.0, 172.0],
                    content: "0 0 1 rg 100 100 72 72 re f".into(),
                    extra: vec![("UserUnit", Object::Real(2.0))],
                    resources: None,
                },
                page([0.0, 0.0, 72.0, 72.0], "0 0 1 rg 0 0 72 72 re f"),
            ],
            vec![],
        );
        let d = read(&bytes);
        let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["p1 #0000ff", "p2 #0000ff"]);
        // Page 1: media box origin at (100,100) maps to (0,0); UserUnit 2 doubles the scale.
        let r1 = region_of(&d.layers[0]);
        let b = r1.bounds().unwrap();
        assert!(b.min.x.abs() < 1e-9 && b.min.y.abs() < 1e-9, "{b:?}");
        assert!(
            (r1.area() - (25.4 * 2.0f64).powi(2)).abs() < 1e-6,
            "{}",
            r1.area()
        );
        assert!((region_of(&d.layers[1]).area() - 25.4f64.powi(2)).abs() < 1e-6);
        assert!(d.diagnostics.iter().any(|x| x.code == "pdf.pages"));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(read_bytes("x.pdf", b"%PDF-1.4 nothing", &ReadOptions::default()).is_err());
        assert!(read_bytes("x.pdf", b"hello", &ReadOptions::default()).is_err());
    }
}
