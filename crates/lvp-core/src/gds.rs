//! GDSII stream reader (our own, D22). Records are `length, type, data type, data`;
//! structures (cells) hold BOUNDARY, PATH, BOX, SREF, AREF, TEXT and NODE elements. Every
//! cell nobody references is a top cell and is flattened with the reference transforms
//! (reflection about x, then magnification, then rotation, then translation). Layers are
//! `layer/datatype`, prefixed with the cell name when there are several top cells. TEXT
//! and NODE are counted, never geometry.
//!
//! PATH elements with flush, square or custom ends become exact polygons (mitred joins);
//! round-ended paths stay centrelines with a width, which is what stroke expansion
//! produces anyway.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::diag::Diagnostic;
use crate::geom::Point;
use crate::model::SourceFormat;
use crate::pline;
use crate::region::LoopRule;
use crate::source::{Primitive, ReadError, ReadOptions, SourceDocument, SourceLayer};
use crate::units::Unit;

const MAX_DEPTH: usize = 64;
const MAX_INSTANCES: usize = 5_000_000;

mod rec {
    pub const HEADER: u8 = 0x00;
    pub const BGNLIB: u8 = 0x01;
    pub const LIBNAME: u8 = 0x02;
    pub const UNITS: u8 = 0x03;
    pub const ENDLIB: u8 = 0x04;
    pub const BGNSTR: u8 = 0x05;
    pub const STRNAME: u8 = 0x06;
    pub const ENDSTR: u8 = 0x07;
    pub const BOUNDARY: u8 = 0x08;
    pub const PATH: u8 = 0x09;
    pub const SREF: u8 = 0x0A;
    pub const AREF: u8 = 0x0B;
    pub const TEXT: u8 = 0x0C;
    pub const LAYER: u8 = 0x0D;
    pub const DATATYPE: u8 = 0x0E;
    pub const WIDTH: u8 = 0x0F;
    pub const XY: u8 = 0x10;
    pub const ENDEL: u8 = 0x11;
    pub const SNAME: u8 = 0x12;
    pub const COLROW: u8 = 0x13;
    pub const NODE: u8 = 0x15;
    pub const TEXTTYPE: u8 = 0x16;
    pub const PRESENTATION: u8 = 0x17;
    pub const STRING: u8 = 0x19;
    pub const STRANS: u8 = 0x1A;
    pub const MAG: u8 = 0x1B;
    pub const ANGLE: u8 = 0x1C;
    pub const PATHTYPE: u8 = 0x21;
    pub const ELFLAGS: u8 = 0x26;
    pub const NODETYPE: u8 = 0x2A;
    pub const PROPATTR: u8 = 0x2B;
    pub const PROPVALUE: u8 = 0x2C;
    pub const BOX: u8 = 0x2D;
    pub const BOXTYPE: u8 = 0x2E;
    pub const PLEX: u8 = 0x2F;
    pub const BGNEXTN: u8 = 0x30;
    pub const ENDEXTN: u8 = 0x31;
}

struct Record<'a> {
    kind: u8,
    data: &'a [u8],
}

fn records(bytes: &[u8]) -> Result<Vec<Record<'_>>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if len == 0 {
            // Padding at the end of a tape block.
            break;
        }
        if len < 4 || i + len > bytes.len() {
            return Err(format!("corrupt record length {len} at byte {i}"));
        }
        out.push(Record {
            kind: bytes[i + 2],
            data: &bytes[i + 4..i + len],
        });
        i += len;
    }
    Ok(out)
}

fn i16s(d: &[u8]) -> Vec<i16> {
    d.as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_be_bytes(*c))
        .collect()
}

fn i32s(d: &[u8]) -> Vec<i32> {
    d.as_chunks::<4>()
        .0
        .iter()
        .map(|c| i32::from_be_bytes(*c))
        .collect()
}

/// GDSII 8-byte real: sign, 7-bit excess-64 exponent of 16, 56-bit fraction.
pub fn real8(d: &[u8]) -> f64 {
    if d.len() < 8 {
        return 0.0;
    }
    let sign = if d[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (d[0] & 0x7f) as i32 - 64;
    let mut mant: u64 = 0;
    for b in &d[1..8] {
        mant = (mant << 8) | *b as u64;
    }
    sign * (mant as f64 / 2f64.powi(56)) * 16f64.powi(exp)
}

fn ascii(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).to_string()
}

#[derive(Clone, Debug)]
enum Element {
    Boundary {
        layer: i16,
        dtype: i16,
        pts: Vec<(i32, i32)>,
    },
    Path {
        layer: i16,
        dtype: i16,
        width: i32,
        pathtype: i16,
        bgnextn: i32,
        endextn: i32,
        pts: Vec<(i32, i32)>,
    },
    Sref {
        name: String,
        strans: u16,
        mag: f64,
        angle_deg: f64,
        origin: (i32, i32),
    },
    Aref {
        name: String,
        strans: u16,
        mag: f64,
        angle_deg: f64,
        cols: i16,
        rows: i16,
        pts: Vec<(i32, i32)>,
    },
    Text,
    Node,
}

#[derive(Default)]
struct Cell {
    elements: Vec<Element>,
}

/// Affine transform in database units: `p' = linear × p + t`.
#[derive(Clone, Copy, Debug)]
struct Xf {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl Xf {
    const IDENTITY: Xf = Xf {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Reference transform: reflect about x (y → −y), magnify, rotate, translate.
    fn reference(strans: u16, mag: f64, angle_deg: f64, origin: (f64, f64)) -> Xf {
        let reflect = strans & 0x8000 != 0;
        let (s, c) = angle_deg.to_radians().sin_cos();
        let m = if mag > 0.0 { mag } else { 1.0 };
        let ry = if reflect { -1.0 } else { 1.0 };
        // linear = R(angle) × diag(m, m) × diag(1, ry)
        Xf {
            a: c * m,
            b: -s * m * ry,
            c: s * m,
            d: c * m * ry,
            tx: origin.0,
            ty: origin.1,
        }
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.b * y + self.tx,
            self.c * x + self.d * y + self.ty,
        )
    }

    /// `self` after `inner`: apply `inner` first.
    fn compose(&self, inner: &Xf) -> Xf {
        Xf {
            a: self.a * inner.a + self.b * inner.c,
            b: self.a * inner.b + self.b * inner.d,
            c: self.c * inner.a + self.d * inner.c,
            d: self.c * inner.b + self.d * inner.d,
            tx: self.a * inner.tx + self.b * inner.ty + self.tx,
            ty: self.c * inner.tx + self.d * inner.ty + self.ty,
        }
    }

    fn scale(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

struct Library {
    version: Option<String>,
    mm_per_db: f64,
    meters_per_user: f64,
    cells: BTreeMap<String, Cell>,
    unknown: BTreeSet<u8>,
}

fn parse(bytes: &[u8]) -> Result<Library, String> {
    let recs = records(bytes)?;
    if recs.is_empty() || recs[0].kind != rec::HEADER {
        return Err("not a GDSII stream (no HEADER record)".into());
    }
    let mut lib = Library {
        version: i16s(recs[0].data).first().map(|v| v.to_string()),
        mm_per_db: 1e-6,
        meters_per_user: 1e-6,
        cells: BTreeMap::new(),
        unknown: BTreeSet::new(),
    };
    let mut units_seen = false;
    let mut current_cell: Option<(String, Cell)> = None;
    let mut el: Option<Element> = None;
    let mut i = 1;
    while i < recs.len() {
        let r = &recs[i];
        i += 1;
        match r.kind {
            rec::BGNLIB | rec::LIBNAME | rec::ENDLIB | rec::BGNSTR => {}
            rec::UNITS => {
                if r.data.len() >= 16 {
                    let user_per_db = real8(&r.data[0..8]);
                    let meters_per_db = real8(&r.data[8..16]);
                    if meters_per_db > 0.0 && user_per_db > 0.0 {
                        lib.mm_per_db = meters_per_db * 1000.0;
                        lib.meters_per_user = meters_per_db / user_per_db;
                        units_seen = true;
                    }
                }
            }
            rec::STRNAME => current_cell = Some((ascii(r.data), Cell::default())),
            rec::ENDSTR => {
                if let Some((name, cell)) = current_cell.take() {
                    lib.cells.insert(name, cell);
                }
            }
            rec::BOUNDARY => {
                el = Some(Element::Boundary {
                    layer: 0,
                    dtype: 0,
                    pts: Vec::new(),
                })
            }
            rec::PATH => {
                el = Some(Element::Path {
                    layer: 0,
                    dtype: 0,
                    width: 0,
                    pathtype: 0,
                    bgnextn: 0,
                    endextn: 0,
                    pts: Vec::new(),
                })
            }
            rec::BOX => {
                el = Some(Element::Boundary {
                    layer: 0,
                    dtype: 0,
                    pts: Vec::new(),
                })
            }
            rec::SREF => {
                el = Some(Element::Sref {
                    name: String::new(),
                    strans: 0,
                    mag: 1.0,
                    angle_deg: 0.0,
                    origin: (0, 0),
                })
            }
            rec::AREF => {
                el = Some(Element::Aref {
                    name: String::new(),
                    strans: 0,
                    mag: 1.0,
                    angle_deg: 0.0,
                    cols: 1,
                    rows: 1,
                    pts: Vec::new(),
                })
            }
            rec::TEXT => el = Some(Element::Text),
            rec::NODE => el = Some(Element::Node),
            rec::ENDEL => {
                if let (Some(e), Some((_, cell))) = (el.take(), current_cell.as_mut()) {
                    cell.elements.push(e);
                }
            }
            rec::LAYER | rec::DATATYPE | rec::BOXTYPE => {
                let v = i16s(r.data).first().copied().unwrap_or(0);
                match (&mut el, r.kind) {
                    (Some(Element::Boundary { layer, .. }), rec::LAYER)
                    | (Some(Element::Path { layer, .. }), rec::LAYER) => *layer = v,
                    (Some(Element::Boundary { dtype, .. }), _)
                    | (Some(Element::Path { dtype, .. }), _) => *dtype = v,
                    _ => {}
                }
            }
            rec::WIDTH => {
                if let Some(Element::Path { width, .. }) = &mut el {
                    *width = i32s(r.data).first().copied().unwrap_or(0);
                }
            }
            rec::PATHTYPE => {
                if let Some(Element::Path { pathtype, .. }) = &mut el {
                    *pathtype = i16s(r.data).first().copied().unwrap_or(0);
                }
            }
            rec::BGNEXTN => {
                if let Some(Element::Path { bgnextn, .. }) = &mut el {
                    *bgnextn = i32s(r.data).first().copied().unwrap_or(0);
                }
            }
            rec::ENDEXTN => {
                if let Some(Element::Path { endextn, .. }) = &mut el {
                    *endextn = i32s(r.data).first().copied().unwrap_or(0);
                }
            }
            rec::XY => {
                let v = i32s(r.data);
                let points: Vec<(i32, i32)> =
                    v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect();
                match &mut el {
                    Some(Element::Boundary { pts, .. })
                    | Some(Element::Path { pts, .. })
                    | Some(Element::Aref { pts, .. }) => *pts = points,
                    Some(Element::Sref { origin, .. }) => {
                        if let Some(p) = points.first() {
                            *origin = *p;
                        }
                    }
                    _ => {}
                }
            }
            rec::SNAME => {
                let n = ascii(r.data);
                match &mut el {
                    Some(Element::Sref { name, .. }) | Some(Element::Aref { name, .. }) => {
                        *name = n
                    }
                    _ => {}
                }
            }
            rec::STRANS => {
                let v = r
                    .data
                    .first()
                    .map(|hi| u16::from_be_bytes([*hi, r.data.get(1).copied().unwrap_or(0)]))
                    .unwrap_or(0);
                match &mut el {
                    Some(Element::Sref { strans, .. }) | Some(Element::Aref { strans, .. }) => {
                        *strans = v
                    }
                    _ => {}
                }
            }
            rec::MAG => {
                let v = real8(r.data);
                match &mut el {
                    Some(Element::Sref { mag, .. }) | Some(Element::Aref { mag, .. }) => *mag = v,
                    _ => {}
                }
            }
            rec::ANGLE => {
                let v = real8(r.data);
                match &mut el {
                    Some(Element::Sref { angle_deg, .. })
                    | Some(Element::Aref { angle_deg, .. }) => *angle_deg = v,
                    _ => {}
                }
            }
            rec::COLROW => {
                let v = i16s(r.data);
                if let (Some(Element::Aref { cols, rows, .. }), 2) = (&mut el, v.len()) {
                    *cols = v[0];
                    *rows = v[1];
                }
            }
            rec::TEXTTYPE
            | rec::PRESENTATION
            | rec::STRING
            | rec::ELFLAGS
            | rec::PLEX
            | rec::NODETYPE
            | rec::PROPATTR
            | rec::PROPVALUE => {}
            other => {
                lib.unknown.insert(other);
            }
        }
    }
    if !units_seen {
        return Err("no UNITS record".into());
    }
    Ok(lib)
}

/// Polygon of a path with flush/square/custom ends and mitred joins (database units).
fn path_polygon(
    pts: &[(f64, f64)],
    width: f64,
    ext_begin: f64,
    ext_end: f64,
) -> Option<Vec<(f64, f64)>> {
    let points: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
    pline::stroke_polygon(&points, width, ext_begin, ext_end)
        .map(|v| v.into_iter().map(|p| (p.x, p.y)).collect())
}

struct Flattener<'a> {
    lib: &'a Library,
    multi_top: bool,
    layers: BTreeMap<String, SourceLayer>,
    text: usize,
    nodes: usize,
    missing: BTreeSet<String>,
    depth_exceeded: bool,
    instances: usize,
    tolerance_mm: f64,
}

impl Flattener<'_> {
    fn layer(&mut self, top: &str, layer: i16, dtype: i16) -> &mut SourceLayer {
        let name = if self.multi_top {
            format!("{top}:{layer}/{dtype}")
        } else {
            format!("{layer}/{dtype}")
        };
        self.layers
            .entry(name.clone())
            .or_insert_with(|| SourceLayer {
                name,
                ..Default::default()
            })
    }

    fn point(&self, xf: &Xf, p: (i32, i32)) -> Point {
        let (x, y) = xf.apply(p.0 as f64, p.1 as f64);
        Point::new(x * self.lib.mm_per_db, y * self.lib.mm_per_db)
    }

    fn flatten(&mut self, top: &str, cell_name: &str, xf: Xf, depth: usize) {
        if depth > MAX_DEPTH {
            self.depth_exceeded = true;
            return;
        }
        let Some(cell) = self.lib.cells.get(cell_name) else {
            self.missing.insert(cell_name.to_string());
            return;
        };
        for el in &cell.elements {
            match el {
                Element::Boundary { layer, dtype, pts } => {
                    let mut points: Vec<Point> = pts.iter().map(|p| self.point(&xf, *p)).collect();
                    if points.len() > 1 && points.first() == points.last() {
                        points.pop();
                    }
                    if points.len() < 3 {
                        continue;
                    }
                    let Some(l) = pline::sanitize(&pline::from_points(points, true)) else {
                        continue;
                    };
                    self.layer(top, *layer, *dtype)
                        .primitives
                        .push(Primitive::Hatch {
                            loops: vec![l],
                            rule: LoopRule::EvenOdd,
                            solid: true,
                            pattern: String::new(),
                        });
                }
                Element::Path {
                    layer,
                    dtype,
                    width,
                    pathtype,
                    bgnextn,
                    endextn,
                    pts,
                } => {
                    let scale = xf.scale();
                    let w_mm = (*width as f64).abs() * scale * self.lib.mm_per_db;
                    if *pathtype == 1 {
                        // Round ends: a centreline with width, stroke expansion is exact.
                        let points: Vec<Point> = pts.iter().map(|p| self.point(&xf, *p)).collect();
                        let Some(l) = pline::sanitize(&pline::from_points(points, false)) else {
                            continue;
                        };
                        self.layer(top, *layer, *dtype)
                            .primitives
                            .push(Primitive::Path {
                                pline: l,
                                width: (w_mm > 0.0).then_some(w_mm),
                            });
                        continue;
                    }
                    let hw_db = (*width as f64).abs() / 2.0;
                    let (eb, ee) = match pathtype {
                        2 => (hw_db, hw_db),
                        4 => (*bgnextn as f64, *endextn as f64),
                        _ => (0.0, 0.0),
                    };
                    let raw: Vec<(f64, f64)> =
                        pts.iter().map(|p| (p.0 as f64, p.1 as f64)).collect();
                    match path_polygon(&raw, (*width as f64).abs(), eb, ee) {
                        Some(poly) => {
                            let points: Vec<Point> = poly
                                .iter()
                                .map(|&(x, y)| {
                                    let (tx, ty) = xf.apply(x, y);
                                    Point::new(tx * self.lib.mm_per_db, ty * self.lib.mm_per_db)
                                })
                                .collect();
                            if let Some(l) = pline::sanitize(&pline::from_points(points, true)) {
                                self.layer(top, *layer, *dtype)
                                    .primitives
                                    .push(Primitive::Hatch {
                                        loops: vec![l],
                                        rule: LoopRule::NonZero,
                                        solid: true,
                                        pattern: String::new(),
                                    });
                            }
                        }
                        None => {
                            // Zero width: a centreline.
                            let points: Vec<Point> =
                                pts.iter().map(|p| self.point(&xf, *p)).collect();
                            if let Some(l) = pline::sanitize(&pline::from_points(points, false)) {
                                self.layer(top, *layer, *dtype)
                                    .primitives
                                    .push(Primitive::Path {
                                        pline: l,
                                        width: None,
                                    });
                            }
                        }
                    }
                }
                Element::Sref {
                    name,
                    strans,
                    mag,
                    angle_deg,
                    origin,
                } => {
                    self.instances += 1;
                    if self.instances > MAX_INSTANCES {
                        return;
                    }
                    let local = Xf::reference(
                        *strans,
                        *mag,
                        *angle_deg,
                        (origin.0 as f64, origin.1 as f64),
                    );
                    self.flatten(top, name, xf.compose(&local), depth + 1);
                }
                Element::Aref {
                    name,
                    strans,
                    mag,
                    angle_deg,
                    cols,
                    rows,
                    pts,
                } => {
                    if pts.len() < 3 || *cols <= 0 || *rows <= 0 {
                        continue;
                    }
                    let o = (pts[0].0 as f64, pts[0].1 as f64);
                    let col_step = (
                        (pts[1].0 as f64 - o.0) / *cols as f64,
                        (pts[1].1 as f64 - o.1) / *cols as f64,
                    );
                    let row_step = (
                        (pts[2].0 as f64 - o.0) / *rows as f64,
                        (pts[2].1 as f64 - o.1) / *rows as f64,
                    );
                    for r in 0..*rows {
                        for c in 0..*cols {
                            self.instances += 1;
                            if self.instances > MAX_INSTANCES {
                                return;
                            }
                            let origin = (
                                o.0 + c as f64 * col_step.0 + r as f64 * row_step.0,
                                o.1 + c as f64 * col_step.1 + r as f64 * row_step.1,
                            );
                            let local = Xf::reference(*strans, *mag, *angle_deg, origin);
                            self.flatten(top, name, xf.compose(&local), depth + 1);
                        }
                    }
                }
                Element::Text => self.text += 1,
                Element::Node => self.nodes += 1,
            }
        }
        let _ = self.tolerance_mm;
    }
}

pub fn read_bytes(
    name: &str,
    bytes: &[u8],
    opts: &ReadOptions,
) -> Result<SourceDocument, ReadError> {
    let err = |message: String| ReadError {
        format: Some(SourceFormat::Gds),
        message,
    };
    let lib = parse(bytes).map_err(|m| err(format!("{name}: {m}")))?;
    if lib.cells.is_empty() {
        return Err(err("the library has no structures".into()));
    }
    // Top cells: never referenced.
    let mut referenced: HashSet<&str> = HashSet::new();
    for cell in lib.cells.values() {
        for el in &cell.elements {
            match el {
                Element::Sref { name, .. } | Element::Aref { name, .. } => {
                    referenced.insert(name.as_str());
                }
                _ => {}
            }
        }
    }
    let mut tops: Vec<&str> = lib
        .cells
        .keys()
        .map(String::as_str)
        .filter(|n| !referenced.contains(n))
        .collect();
    if tops.is_empty() {
        // Everything references everything (cycle): take them all.
        tops = lib.cells.keys().map(String::as_str).collect();
    }
    let mut fl = Flattener {
        lib: &lib,
        multi_top: tops.len() > 1,
        layers: BTreeMap::new(),
        text: 0,
        nodes: 0,
        missing: BTreeSet::new(),
        depth_exceeded: false,
        instances: 0,
        tolerance_mm: opts.tolerance_mm,
    };
    for top in &tops {
        fl.flatten(top, top, Xf::IDENTITY, 0);
    }
    let mut diags = Vec::new();
    if tops.len() > 1 {
        diags.push(Diagnostic::info(
            "gds.top_cells",
            format!(
                "{} top-level structures ({}); layers are prefixed with the structure name",
                tops.len(),
                tops.join(", ")
            ),
        ));
    }
    if fl.text > 0 {
        diags.push(Diagnostic::info(
            "gds.text",
            format!(
                "{} TEXT element(s) are annotations and were not read as geometry",
                fl.text
            ),
        ));
    }
    if fl.nodes > 0 {
        diags.push(Diagnostic::info(
            "gds.node",
            format!(
                "{} NODE element(s) ignored (electrical nets, no geometry)",
                fl.nodes
            ),
        ));
    }
    if !fl.missing.is_empty() {
        diags.push(Diagnostic::error(
            "gds.reference_missing",
            format!(
                "referenced structures are missing: {}",
                fl.missing.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    if fl.depth_exceeded {
        diags.push(Diagnostic::error(
            "gds.depth",
            "structure references nest deeper than 64 levels (cycle?); deeper content was skipped",
        ));
    }
    if fl.instances > MAX_INSTANCES {
        diags.push(Diagnostic::error(
            "gds.instances",
            "more than 5 million placed instances; the rest was skipped",
        ));
    }
    if !lib.unknown.is_empty() {
        diags.push(Diagnostic::info(
            "gds.unknown_record",
            format!(
                "unknown record types ignored: {}",
                lib.unknown
                    .iter()
                    .map(|k| format!("0x{k:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ));
    }
    let units = if (lib.meters_per_user - 1e-6).abs() < 1e-12 {
        Some(Unit::Micrometre)
    } else if (lib.meters_per_user - 1e-3).abs() < 1e-9 {
        Some(Unit::Millimetre)
    } else {
        diags.push(Diagnostic::info(
            "gds.units",
            format!(
                "user unit is {} m; coordinates were converted through the database unit",
                lib.meters_per_user
            ),
        ));
        None
    };
    let layers: Vec<SourceLayer> = fl.layers.into_values().collect();
    if layers.is_empty() {
        diags.push(Diagnostic::warning(
            "gds.empty",
            "no BOUNDARY, PATH or BOX geometry in the top structures",
        ));
    }
    Ok(SourceDocument {
        format: SourceFormat::Gds,
        version: lib.version,
        units,
        layers,
        diagnostics: diags,
    })
}

/// Minimal GDSII emitter used by tests (and mirrored in the fixture generator).
#[cfg(test)]
pub(crate) mod emit {
    pub fn encode_real8(v: f64) -> [u8; 8] {
        if v == 0.0 {
            return [0; 8];
        }
        let sign = if v < 0.0 { 0x80u8 } else { 0 };
        let mut m = v.abs();
        let mut e: i32 = 0;
        while m >= 1.0 {
            m /= 16.0;
            e += 1;
        }
        while m < 1.0 / 16.0 {
            m *= 16.0;
            e -= 1;
        }
        let mant = (m * 2f64.powi(56)).round() as u64;
        let mut out = [0u8; 8];
        out[0] = sign | ((e + 64) as u8 & 0x7f);
        for i in 0..7 {
            out[1 + i] = ((mant >> (8 * (6 - i))) & 0xff) as u8;
        }
        out
    }

    pub fn record(out: &mut Vec<u8>, kind: u8, dtype: u8, data: &[u8]) {
        let len = (data.len() + 4) as u16;
        out.extend_from_slice(&len.to_be_bytes());
        out.push(kind);
        out.push(dtype);
        out.extend_from_slice(data);
    }

    pub fn i16(out: &mut Vec<u8>, kind: u8, v: i16) {
        record(out, kind, 2, &v.to_be_bytes());
    }

    pub fn i32(out: &mut Vec<u8>, kind: u8, v: i32) {
        record(out, kind, 3, &v.to_be_bytes());
    }

    pub fn xy(out: &mut Vec<u8>, pts: &[(i32, i32)]) {
        let mut d = Vec::new();
        for (x, y) in pts {
            d.extend_from_slice(&x.to_be_bytes());
            d.extend_from_slice(&y.to_be_bytes());
        }
        record(out, super::rec::XY, 3, &d);
    }

    pub fn name(out: &mut Vec<u8>, kind: u8, s: &str) {
        let mut d = s.as_bytes().to_vec();
        if d.len() % 2 == 1 {
            d.push(0);
        }
        record(out, kind, 6, &d);
    }
}

#[cfg(test)]
mod tests {
    use super::emit::*;
    use super::*;
    use crate::region::Region;
    use cavalier_contours::polyline::PlineSource;

    const TIME: [u8; 24] = [0; 24];

    fn header(out: &mut Vec<u8>, user_per_db: f64, meters_per_db: f64) {
        i16(out, rec::HEADER, 600);
        record(out, rec::BGNLIB, 2, &TIME);
        name(out, rec::LIBNAME, "LIB");
        let mut u = Vec::new();
        u.extend_from_slice(&encode_real8(user_per_db));
        u.extend_from_slice(&encode_real8(meters_per_db));
        record(out, rec::UNITS, 5, &u);
    }

    fn begin_cell(out: &mut Vec<u8>, n: &str) {
        record(out, rec::BGNSTR, 2, &TIME);
        name(out, rec::STRNAME, n);
    }

    fn boundary(out: &mut Vec<u8>, layer: i16, dtype: i16, pts: &[(i32, i32)]) {
        record(out, rec::BOUNDARY, 0, &[]);
        i16(out, rec::LAYER, layer);
        i16(out, rec::DATATYPE, dtype);
        let mut closed = pts.to_vec();
        closed.push(pts[0]);
        xy(out, &closed);
        record(out, rec::ENDEL, 0, &[]);
    }

    fn path(out: &mut Vec<u8>, layer: i16, width: i32, pathtype: i16, pts: &[(i32, i32)]) {
        record(out, rec::PATH, 0, &[]);
        i16(out, rec::LAYER, layer);
        i16(out, rec::DATATYPE, 0);
        i16(out, rec::PATHTYPE, pathtype);
        i32(out, rec::WIDTH, width);
        xy(out, pts);
        record(out, rec::ENDEL, 0, &[]);
    }

    fn sref(
        out: &mut Vec<u8>,
        n: &str,
        strans: u16,
        mag: Option<f64>,
        angle: Option<f64>,
        at: (i32, i32),
    ) {
        record(out, rec::SREF, 0, &[]);
        name(out, rec::SNAME, n);
        if strans != 0 || mag.is_some() || angle.is_some() {
            record(out, rec::STRANS, 1, &strans.to_be_bytes());
        }
        if let Some(m) = mag {
            record(out, rec::MAG, 5, &encode_real8(m));
        }
        if let Some(a) = angle {
            record(out, rec::ANGLE, 5, &encode_real8(a));
        }
        xy(out, &[at]);
        record(out, rec::ENDEL, 0, &[]);
    }

    fn end_cell(out: &mut Vec<u8>) {
        record(out, rec::ENDSTR, 0, &[]);
    }

    fn end_lib(out: &mut Vec<u8>) {
        record(out, rec::ENDLIB, 0, &[]);
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
    fn real8_roundtrips() {
        for v in [
            0.0, 1.0, -1.0, 1e-9, 1e-3, 0.001, 123456.789, 2.5e-7, 90.0, 270.0,
        ] {
            let back = super::real8(&encode_real8(v));
            assert!(
                (back - v).abs() <= v.abs() * 1e-14 + 1e-300,
                "{v} -> {back}"
            );
        }
    }

    #[test]
    fn boundaries_paths_and_references_flatten_in_millimetres() {
        // 1 db unit = 1 nm, 1 user unit = 1 um.
        let mut g = Vec::new();
        header(&mut g, 1e-3, 1e-9);
        begin_cell(&mut g, "PAD");
        boundary(
            &mut g,
            1,
            0,
            &[(0, 0), (10_000, 0), (10_000, 10_000), (0, 10_000)],
        ); // 10 um square
        path(&mut g, 2, 2_000, 0, &[(0, 20_000), (30_000, 20_000)]); // flush 30 x 2 um
        end_cell(&mut g);
        begin_cell(&mut g, "TOP");
        sref(&mut g, "PAD", 0, None, None, (0, 0));
        sref(&mut g, "PAD", 0x8000, Some(2.0), Some(90.0), (100_000, 0)); // mirrored, 2x, rotated
        end_cell(&mut g);
        end_lib(&mut g);

        let doc = read_bytes("t.gds", &g, &ReadOptions::default()).unwrap();
        assert_eq!(doc.format, SourceFormat::Gds);
        assert_eq!(doc.units, Some(Unit::Micrometre));
        assert_eq!(doc.version.as_deref(), Some("600"));
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["1/0", "2/0"]);
        let l1 = region_of(&doc.layers[0]);
        // 10 um square (0.1 mm²... no: 0.01 mm × 0.01 mm = 1e-4 mm²) plus its 2x copy (4e-4).
        assert!((l1.area() - (1e-4 + 4e-4)).abs() < 1e-12, "{}", l1.area());
        assert_eq!(l1.outers().count(), 2);
        let l2 = region_of(&doc.layers[1]);
        // Flush path 30 x 2 um = 6e-5 mm², plus the 2x copy: 4 x 6e-5.
        assert!((l2.area() - (6e-5 + 2.4e-4)).abs() < 1e-12, "{}", l2.area());
        // The mirrored + rotated copy: mirror y, rotate 90 → the pad at (0..10,0..10) maps
        // to x in [0.1, 0.12] mm, y in [0, 0.02] mm.
        let b = l1.bounds().unwrap();
        assert!(
            (b.max.x - 0.12).abs() < 1e-9 && (b.max.y - 0.02).abs() < 1e-9,
            "{b:?}"
        );
    }

    #[test]
    fn path_end_styles() {
        let hw = 1.0;
        // Flush: exactly the rectangle between the end points.
        let p = path_polygon(&[(0.0, 0.0), (10.0, 0.0)], 2.0 * hw, 0.0, 0.0).unwrap();
        let poly = pline::from_points(p.iter().map(|&(x, y)| Point::new(x, y)), true);
        assert!((poly.area().abs() - 20.0).abs() < 1e-9);
        // Square ends extend by half the width at both ends.
        let p = path_polygon(&[(0.0, 0.0), (10.0, 0.0)], 2.0, hw, hw).unwrap();
        let poly = pline::from_points(p.iter().map(|&(x, y)| Point::new(x, y)), true);
        assert!((poly.area().abs() - 24.0).abs() < 1e-9);
        // A right-angle bend is mitred: the union of the two 10 x 2 legs (overlapping in a
        // 1 x 1 corner) plus the 1 x 1 mitre square: 20 + 20 - 1 + 1 = 40.
        let p = path_polygon(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 2.0, 0.0, 0.0).unwrap();
        let poly = pline::from_points(p.iter().map(|&(x, y)| Point::new(x, y)), true);
        let r = Region::from_loops([poly], LoopRule::NonZero);
        assert!((r.area() - 40.0).abs() < 1e-9, "{}", r.area());
        assert!((r.bounds().unwrap().max.x - 11.0).abs() < 1e-9);
    }

    #[test]
    fn arrays_text_and_missing_references() {
        let mut g = Vec::new();
        header(&mut g, 1e-3, 1e-9);
        begin_cell(&mut g, "DOT");
        boundary(
            &mut g,
            5,
            1,
            &[(0, 0), (1_000, 0), (1_000, 1_000), (0, 1_000)],
        );
        record(&mut g, rec::TEXT, 0, &[]);
        i16(&mut g, rec::LAYER, 5);
        i16(&mut g, rec::TEXTTYPE, 0);
        xy(&mut g, &[(0, 0)]);
        name(&mut g, rec::STRING, "hi");
        record(&mut g, rec::ENDEL, 0, &[]);
        end_cell(&mut g);
        begin_cell(&mut g, "TOP");
        // 3 x 2 array, 5 um pitch in x, 7 um in y.
        record(&mut g, rec::AREF, 0, &[]);
        name(&mut g, rec::SNAME, "DOT");
        record(&mut g, rec::COLROW, 2, &[0, 3, 0, 2]);
        xy(&mut g, &[(0, 0), (15_000, 0), (0, 14_000)]);
        record(&mut g, rec::ENDEL, 0, &[]);
        sref(&mut g, "GHOST", 0, None, None, (0, 0));
        end_cell(&mut g);
        end_lib(&mut g);
        let doc = read_bytes("t.gds", &g, &ReadOptions::default()).unwrap();
        let l = region_of(&doc.layers[0]);
        assert_eq!(doc.layers[0].name, "5/1");
        assert_eq!(l.outers().count(), 6);
        let b = l.bounds().unwrap();
        assert!(
            (b.max.x - 0.011).abs() < 1e-9 && (b.max.y - 0.008).abs() < 1e-9,
            "{b:?}"
        );
        let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"gds.text"));
        assert!(codes.contains(&"gds.reference_missing"));
    }

    #[test]
    fn garbage_is_refused() {
        assert!(
            read_bytes(
                "x.gds",
                b"\x00\x06\x00\x02\x02\x58",
                &ReadOptions::default()
            )
            .is_err()
        );
        assert!(read_bytes("x.gds", b"hello world", &ReadOptions::default()).is_err());
        let mut g = Vec::new();
        i16(&mut g, rec::HEADER, 600);
        record(&mut g, rec::ENDLIB, 0, &[]);
        assert!(
            read_bytes("x.gds", &g, &ReadOptions::default()).is_err(),
            "no units"
        );
    }
}
