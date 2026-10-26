//! IPC-2581 reader (our own, D22) on `roxmltree`: `Ecad/CadData` layers and steps, the
//! standard and user primitive dictionaries, line descriptors, feature sets with positive /
//! negative polarity composited in source order, the board profile, and `layerFunction` +
//! `side` as the layer function for profile binding. Components, nets, stack-up and
//! fabrication specifications are not geometry and are ignored.
//!
//! Each `<Layer>` with features becomes one source layer; the profile becomes `Profile`.
//! Several steps prefix the layer names with the step name.

use std::collections::{BTreeMap, HashMap};

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};
use usvg::roxmltree::{Document, Node};

use crate::diag::Diagnostic;
use crate::flatten::flatten_cubic;
use crate::geom::Point;
use crate::model::SourceFormat;
use crate::pline::{self, Pline};
use crate::region::{self, LoopRule, Region};
use crate::source::{Budget, Primitive, ReadError, ReadOptions, SourceDocument, SourceLayer};
use crate::units::Unit;

fn unit_scale(name: Option<&str>) -> Option<(f64, Option<Unit>)> {
    match name.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        Some("MILLIMETER") | Some("MM") => Some((1.0, Some(Unit::Millimetre))),
        Some("INCH") | Some("IN") => Some((25.4, Some(Unit::Inch))),
        Some("MICRON") | Some("UM") => Some((0.001, Some(Unit::Micrometre))),
        Some("MIL") => Some((0.0254, Some(Unit::Mil))),
        _ => None,
    }
}

fn attr_f(n: Node<'_, '_>, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|v| v.trim().parse::<f64>().ok())
}

fn attr_bool(n: Node<'_, '_>, name: &str) -> bool {
    n.attribute(name)
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
}

/// Rigid transform: mirror about the y axis (x → −x), rotate (degrees, CCW), translate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Xform {
    mirror: bool,
    rotation_deg: f64,
    dx: f64,
    dy: f64,
}

impl Xform {
    const IDENTITY: Xform = Xform {
        mirror: false,
        rotation_deg: 0.0,
        dx: 0.0,
        dy: 0.0,
    };

    fn from_node(n: Node<'_, '_>, scale: f64) -> Xform {
        Xform {
            mirror: attr_bool(n, "mirror"),
            rotation_deg: attr_f(n, "rotation").unwrap_or(0.0),
            dx: attr_f(n, "xOffset").unwrap_or(0.0) * scale,
            dy: attr_f(n, "yOffset").unwrap_or(0.0) * scale,
        }
    }

    fn apply_pline(&self, p: &Pline) -> Pline {
        if *self == Self::IDENTITY {
            return p.clone();
        }
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        let mut out = if p.is_closed() {
            Pline::new_closed()
        } else {
            Pline::new()
        };
        for v in p.iter_vertexes() {
            let x = if self.mirror { -v.x } else { v.x };
            let y = v.y;
            out.add(
                c * x - s * y + self.dx,
                s * x + c * y + self.dy,
                if self.mirror { -v.bulge } else { v.bulge },
            );
        }
        if self.mirror {
            out = reverse(&out);
        }
        out
    }

    fn translated(&self, x: f64, y: f64) -> Xform {
        // Translation applied after the rotation: just add to the offset.
        Xform {
            dx: self.dx + x,
            dy: self.dy + y,
            ..*self
        }
    }
}

fn reverse(p: &Pline) -> Pline {
    let n = p.vertex_count();
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for i in (0..n).rev() {
        let v = p.at(i);
        let b = if p.is_closed() {
            -p.at((i + n - 1) % n).bulge
        } else if i == 0 {
            0.0
        } else {
            -p.at(i - 1).bulge
        };
        out.add(v.x, v.y, b);
    }
    out
}

#[derive(Clone, Debug)]
struct LineDesc {
    width: f64,
    /// ROUND, SQUARE or NONE.
    end: String,
}

impl LineDesc {
    fn from_node(n: Node<'_, '_>, scale: f64) -> LineDesc {
        LineDesc {
            width: attr_f(n, "lineWidth").unwrap_or(0.0) * scale,
            end: n
                .attribute("lineEnd")
                .unwrap_or("ROUND")
                .to_ascii_uppercase(),
        }
    }
}

#[derive(Default)]
struct Counts {
    unsupported: BTreeMap<String, usize>,
    components: usize,
    mirrored: usize,
}

struct Reader<'a> {
    scale: f64,
    tol_mm: f64,
    standard: HashMap<String, Region>,
    user: HashMap<String, Region>,
    lines: HashMap<String, LineDesc>,
    counts: Counts,
    diags: Vec<Diagnostic>,
    budget: Budget,
    _doc: &'a Document<'a>,
}

/// A feature read from a set, before polarity compositing.
enum Shape {
    Fill(Region),
    Stroke { pline: Pline, width: f64 },
    Centreline(Pline),
}

impl Reader<'_> {
    fn unsupported(&mut self, what: &str) {
        *self.counts.unsupported.entry(what.to_string()).or_insert(0) += 1;
    }

    // ---- polygons --------------------------------------------------------------------

    /// `<Polygon>` / `<Cutout>` / `<Polyline>` children: PolyBegin, PolyStepSegment,
    /// PolyStepCurve. Returns an open polyline (the caller closes it where appropriate).
    fn poly(&mut self, n: Node<'_, '_>, scale: f64) -> Option<Pline> {
        let mut p = Pline::new();
        let mut last: Option<Point> = None;
        for c in n
            .children()
            .filter(|c| c.is_element() && c.tag_name().name().starts_with("Poly"))
        {
            let x = attr_f(c, "x")? * scale;
            let y = attr_f(c, "y")? * scale;
            let pt = Point::new(x, y);
            match c.tag_name().name() {
                "PolyBegin" | "PolyStepSegment" => {
                    p.add(x, y, 0.0);
                    last = Some(pt);
                }
                "PolyStepCurve" => {
                    let cx = attr_f(c, "centerX")? * scale;
                    let cy = attr_f(c, "centerY")? * scale;
                    let cw = attr_bool(c, "clockwise");
                    match last {
                        Some(from) => {
                            pline::add_center_arc(&mut p, from, pt, Point::new(cx, cy), !cw)
                        }
                        None => p.add(x, y, 0.0),
                    }
                    last = Some(pt);
                }
                _ => {}
            }
        }
        (p.vertex_count() >= 2).then_some(p)
    }

    fn closed(&mut self, n: Node<'_, '_>, scale: f64) -> Option<Pline> {
        let mut p = self.poly(n, scale)?;
        p.set_is_closed(true);
        pline::sanitize(&p)
    }

    /// `<Contour>`: a Polygon with Cutouts.
    fn contour(&mut self, n: Node<'_, '_>, scale: f64) -> Region {
        let mut outer: Vec<Pline> = Vec::new();
        let mut cut: Vec<Pline> = Vec::new();
        for c in n.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "Polygon" => outer.extend(self.closed(c, scale)),
                "Cutout" => cut.extend(self.closed(c, scale)),
                _ => {}
            }
        }
        let r = Region::from_loops(outer, LoopRule::NonZero);
        if cut.is_empty() {
            r
        } else {
            r.difference(&Region::from_loops(cut, LoopRule::NonZero))
        }
    }

    // ---- primitives -------------------------------------------------------------------

    fn standard_primitive(&mut self, n: Node<'_, '_>, scale: f64) -> Option<Region> {
        let o = Point::new(0.0, 0.0);
        let s = scale;
        let loops_region = |l: Pline| Region::from_loops([l], LoopRule::NonZero);
        let rect = |w: f64, h: f64| {
            pline::rect(Point::new(-w / 2.0, -h / 2.0), Point::new(w / 2.0, h / 2.0))
        };
        let shape = match n.tag_name().name() {
            "Circle" => loops_region(pline::circle(o, attr_f(n, "diameter")? * s / 2.0)),
            "RectCenter" => loops_region(rect(attr_f(n, "width")? * s, attr_f(n, "height")? * s)),
            "RectCorner" => loops_region(pline::rect(
                Point::new(attr_f(n, "lowerLeftX")? * s, attr_f(n, "lowerLeftY")? * s),
                Point::new(attr_f(n, "upperRightX")? * s, attr_f(n, "upperRightY")? * s),
            )),
            "RectRound" => {
                let (w, h, r) = (
                    attr_f(n, "width")? * s,
                    attr_f(n, "height")? * s,
                    attr_f(n, "radius")? * s,
                );
                let r = r.min(w / 2.0).min(h / 2.0);
                let corners =
                    ["lowerLeft", "lowerRight", "upperRight", "upperLeft"].map(|c| attr_bool(n, c));
                loops_region(rounded_rect(w, h, r, corners))
            }
            "RectCham" => {
                let (w, h, c) = (
                    attr_f(n, "width")? * s,
                    attr_f(n, "height")? * s,
                    attr_f(n, "chamfer")? * s,
                );
                let corners =
                    ["lowerLeft", "lowerRight", "upperRight", "upperLeft"].map(|k| attr_bool(n, k));
                loops_region(chamfered_rect(w, h, c.min(w / 2.0).min(h / 2.0), corners))
            }
            "Oval" => {
                let (w, h) = (attr_f(n, "width")? * s, attr_f(n, "height")? * s);
                loops_region(obround(w, h))
            }
            "Donut" => {
                let (od, id) = (
                    attr_f(n, "outerDiameter")? * s,
                    attr_f(n, "innerDiameter")? * s,
                );
                let shape = n.attribute("shape").unwrap_or("ROUND").to_ascii_uppercase();
                let (outer, inner) = match shape.as_str() {
                    "SQUARE" => (rect(od, od), rect(id, id)),
                    "HEXAGON" => (regular(o, od, 6), regular(o, id, 6)),
                    "OCTAGON" => (regular(o, od, 8), regular(o, id, 8)),
                    _ => (pline::circle(o, od / 2.0), pline::circle(o, id / 2.0)),
                };
                Region::from_loops([outer, inner], LoopRule::EvenOdd)
            }
            "Octagon" => {
                let w = attr_f(n, "width").or_else(|| attr_f(n, "diameter"))? * s;
                loops_region(regular(o, w / (22.5f64.to_radians().cos()), 8))
            }
            "Hexagon" => {
                let w = attr_f(n, "width").or_else(|| attr_f(n, "diameter"))? * s;
                loops_region(regular(o, w / (30f64.to_radians().cos()), 6))
            }
            "Diamond" => {
                let (w, h) = (attr_f(n, "width")? * s, attr_f(n, "height")? * s);
                loops_region(pline::from_points(
                    [
                        Point::new(0.0, -h / 2.0),
                        Point::new(w / 2.0, 0.0),
                        Point::new(0.0, h / 2.0),
                        Point::new(-w / 2.0, 0.0),
                    ],
                    true,
                ))
            }
            "Triangle" => {
                let (b, h) = (attr_f(n, "base")? * s, attr_f(n, "height")? * s);
                loops_region(pline::from_points(
                    [
                        Point::new(-b / 2.0, -h / 2.0),
                        Point::new(b / 2.0, -h / 2.0),
                        Point::new(0.0, h / 2.0),
                    ],
                    true,
                ))
            }
            "Ellipse" => {
                let (w, h) = (attr_f(n, "width")? * s, attr_f(n, "height")? * s);
                loops_region(ellipse(w, h, self.tol_mm))
            }
            "Thermal" => {
                let (od, id) = (
                    attr_f(n, "outerDiameter")? * s,
                    attr_f(n, "innerDiameter")? * s,
                );
                let gap = attr_f(n, "gap").unwrap_or(0.0) * s;
                let spokes = attr_f(n, "spokeCount").unwrap_or(4.0).max(0.0) as usize;
                let start = attr_f(n, "spokeStartAngle").unwrap_or(45.0);
                let shape = n.attribute("shape").unwrap_or("ROUND").to_ascii_uppercase();
                let (outer, inner) = match shape.as_str() {
                    "SQUARE" => (rect(od, od), rect(id, id)),
                    "HEXAGON" => (regular(o, od, 6), regular(o, id, 6)),
                    "OCTAGON" => (regular(o, od, 8), regular(o, id, 8)),
                    _ => (pline::circle(o, od / 2.0), pline::circle(o, id / 2.0)),
                };
                let mut r = Region::from_loops([outer, inner], LoopRule::EvenOdd);
                if gap > 0.0 && spokes > 0 {
                    let mut bars = Vec::new();
                    for k in 0..spokes {
                        let a = (start + 180.0 * k as f64 / spokes as f64).to_radians();
                        let bar =
                            pline::rect(Point::new(-od, -gap / 2.0), Point::new(od, gap / 2.0));
                        bars.push(pline::rotated(&bar, a));
                    }
                    r = r.difference(&Region::from_loops(bars, LoopRule::MergeThenNest));
                }
                r
            }
            "Contour" => self.contour(n, s),
            "Polygon" => loops_region(self.closed(n, s)?),
            other => {
                self.unsupported(other);
                return None;
            }
        };
        Some(shape)
    }

    fn dictionaries(&mut self, root: Node<'_, '_>) {
        for dict in root.descendants().filter(|n| n.is_element()) {
            match dict.tag_name().name() {
                "DictionaryStandard" => {
                    let scale = unit_scale(dict.attribute("units"))
                        .map(|u| u.0)
                        .unwrap_or(self.scale);
                    for e in dict
                        .children()
                        .filter(|c| c.is_element() && c.tag_name().name() == "EntryStandard")
                    {
                        let Some(id) = e.attribute("id") else {
                            continue;
                        };
                        let mut region: Option<Region> = None;
                        for prim in e.children().filter(|c| c.is_element()) {
                            if let Some(r) = self.standard_primitive(prim, scale) {
                                region = Some(match region {
                                    Some(acc) => acc.union(&r),
                                    None => r,
                                });
                            }
                        }
                        if let Some(r) = region {
                            self.standard.insert(id.to_string(), r);
                        }
                    }
                }
                "DictionaryUser" => {
                    let scale = unit_scale(dict.attribute("units"))
                        .map(|u| u.0)
                        .unwrap_or(self.scale);
                    for e in dict
                        .children()
                        .filter(|c| c.is_element() && c.tag_name().name() == "EntryUser")
                    {
                        let Some(id) = e.attribute("id") else {
                            continue;
                        };
                        // The entry's features (UserSpecial groups, or the same elements
                        // directly), united into one area.
                        let mut shapes = Vec::new();
                        self.features(e, scale, Xform::IDENTITY, &mut shapes, true);
                        let region = Region::union_all(shapes.into_iter().filter_map(
                            |(_, shape)| match shape {
                                Shape::Fill(r) => Some(r),
                                Shape::Stroke { pline, width } => {
                                    Some(region::stroke(&pline, width))
                                }
                                Shape::Centreline(_) => None,
                            },
                        ));
                        if !region.is_empty() {
                            self.user.insert(id.to_string(), region);
                        }
                    }
                }
                "DictionaryLineDesc" => {
                    let scale = unit_scale(dict.attribute("units"))
                        .map(|u| u.0)
                        .unwrap_or(self.scale);
                    for e in dict
                        .children()
                        .filter(|c| c.is_element() && c.tag_name().name() == "EntryLineDesc")
                    {
                        let Some(id) = e.attribute("id") else {
                            continue;
                        };
                        if let Some(ld) = e
                            .children()
                            .find(|c| c.is_element() && c.tag_name().name() == "LineDesc")
                        {
                            self.lines
                                .insert(id.to_string(), LineDesc::from_node(ld, scale));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ---- features ---------------------------------------------------------------------

    /// A line's description: a reference into the dictionary, or written inline (as
    /// Altium does for every line and arc).
    fn line_desc(&self, n: Node<'_, '_>, scale: f64) -> Option<LineDesc> {
        n.children()
            .filter(|c| c.is_element())
            .find_map(|c| match c.tag_name().name() {
                "LineDescRef" => self.lines.get(c.attribute("id")?).cloned(),
                "LineDesc" => Some(LineDesc::from_node(c, scale)),
                _ => None,
            })
    }

    /// Line, Arc or Polyline with a line descriptor.
    fn linework(&mut self, n: Node<'_, '_>, scale: f64, xf: &Xform) -> Option<Shape> {
        let mut p = Pline::new();
        match n.tag_name().name() {
            "Line" => {
                p.add(
                    attr_f(n, "startX")? * scale,
                    attr_f(n, "startY")? * scale,
                    0.0,
                );
                p.add(attr_f(n, "endX")? * scale, attr_f(n, "endY")? * scale, 0.0);
            }
            "Arc" => {
                let from = Point::new(attr_f(n, "startX")? * scale, attr_f(n, "startY")? * scale);
                let to = Point::new(attr_f(n, "endX")? * scale, attr_f(n, "endY")? * scale);
                let c = Point::new(attr_f(n, "centerX")? * scale, attr_f(n, "centerY")? * scale);
                p.add(from.x, from.y, 0.0);
                pline::add_center_arc(&mut p, from, to, c, !attr_bool(n, "clockwise"));
            }
            "Polyline" => {
                p = self.poly(n, scale)?;
            }
            _ => return None,
        }
        let p = pline::sanitize(&xf.apply_pline(&p))?;
        let desc = self.line_desc(n, scale);
        match desc {
            Some(ld) if ld.width > 0.0 => {
                if ld.end == "ROUND" {
                    Some(Shape::Stroke {
                        pline: p,
                        width: ld.width,
                    })
                } else {
                    // Flat or square ends: exact polygon of the flattened centreline.
                    let pts = pline::flatten(&p, self.tol_mm).ok()?;
                    let ext = if ld.end == "SQUARE" {
                        ld.width / 2.0
                    } else {
                        0.0
                    };
                    let poly = pline::stroke_polygon(&pts, ld.width, ext, ext)?;
                    Some(Shape::Fill(Region::from_loops(
                        [pline::from_points(poly, true)],
                        LoopRule::NonZero,
                    )))
                }
            }
            _ => Some(Shape::Centreline(p)),
        }
    }

    fn flash(&mut self, region: &Region, xf: &Xform) -> Shape {
        let loops: Vec<Pline> = region
            .outers()
            .chain(region.holes())
            .map(|l| xf.apply_pline(l))
            .collect();
        Shape::Fill(Region::from_loops(loops, LoopRule::EvenOdd))
    }

    /// Features of one `<Set>` (or `<Features>` block) in document order.
    /// Keep a feature if the import budget allows it (see [`Budget`]).
    fn emit(&mut self, out: &mut Vec<(bool, Shape)>, dark: bool, shape: Shape) {
        let vertices: usize = match &shape {
            Shape::Fill(r) => r.loops.iter().map(|l| l.vertex_count()).sum(),
            Shape::Stroke { pline, .. } | Shape::Centreline(pline) => pline.vertex_count(),
        };
        if self.budget.primitive(vertices) {
            out.push((dark, shape));
        }
    }

    fn features(
        &mut self,
        n: Node<'_, '_>,
        scale: f64,
        xf: Xform,
        out: &mut Vec<(bool, Shape)>,
        dark: bool,
    ) {
        let mut xf = xf;
        for c in n.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "Xform" => {
                    let local = Xform::from_node(c, scale);
                    if local.mirror {
                        self.counts.mirrored += 1;
                    }
                    xf = Xform {
                        mirror: xf.mirror ^ local.mirror,
                        rotation_deg: xf.rotation_deg + local.rotation_deg,
                        dx: xf.dx + local.dx,
                        dy: xf.dy + local.dy,
                    };
                }
                "Location" => {
                    xf = xf.translated(
                        attr_f(c, "x").unwrap_or(0.0) * scale,
                        attr_f(c, "y").unwrap_or(0.0) * scale,
                    );
                }
                "Features" => {
                    self.features(c, scale, xf, out, dark);
                }
                "Pad" | "Fiducial" => {
                    let mut local = xf;
                    for k in c.children().filter(|k| k.is_element()) {
                        match k.tag_name().name() {
                            "Xform" => {
                                let x = Xform::from_node(k, scale);
                                if x.mirror {
                                    self.counts.mirrored += 1;
                                }
                                local = Xform {
                                    mirror: local.mirror ^ x.mirror,
                                    rotation_deg: local.rotation_deg + x.rotation_deg,
                                    dx: local.dx + x.dx,
                                    dy: local.dy + x.dy,
                                };
                            }
                            "Location" => {
                                local = local.translated(
                                    attr_f(k, "x").unwrap_or(0.0) * scale,
                                    attr_f(k, "y").unwrap_or(0.0) * scale,
                                );
                            }
                            _ => {}
                        }
                    }
                    if let Some(shape) = self.primitive_ref(c, scale, &local) {
                        self.emit(out, dark, shape);
                    }
                }
                "Hole" => {
                    let d = attr_f(c, "diameter").unwrap_or(0.0) * scale;
                    let x = attr_f(c, "x").unwrap_or(0.0) * scale;
                    let y = attr_f(c, "y").unwrap_or(0.0) * scale;
                    if d > 0.0 {
                        let circle = pline::circle(Point::new(x, y), d / 2.0);
                        let shape = Shape::Fill(Region::from_loops(
                            [xf.apply_pline(&circle)],
                            LoopRule::NonZero,
                        ));
                        self.emit(out, dark, shape);
                    }
                }
                "Line" | "Arc" | "Polyline" => {
                    if let Some(shape) = self.linework(c, scale, &xf) {
                        self.emit(out, dark, shape);
                    }
                }
                "Contour" => {
                    let r = self.contour(c, scale);
                    if !r.is_empty() {
                        {
                            let shape = self.flash(&r, &xf);
                            self.emit(out, dark, shape);
                        }
                    }
                }
                "Polygon" => {
                    if let Some(l) = self.closed(c, scale) {
                        let r = Region::from_loops([l], LoopRule::NonZero);
                        {
                            let shape = self.flash(&r, &xf);
                            self.emit(out, dark, shape);
                        }
                    }
                }
                "StandardPrimitiveRef" | "UserPrimitiveRef" => {
                    if let Some(shape) = self.primitive_ref_node(c, &xf) {
                        self.emit(out, dark, shape);
                    }
                }
                // A group of simple features: Altium writes a layer's traces, arcs and
                // outlines this way, not only user dictionary entries.
                "UserSpecial" => self.features(c, scale, xf, out, dark),
                "Text" => self.unsupported("Text"),
                "Marking" => self.unsupported("Marking"),
                other if other.ends_with("Ref") || other == "Color" || other == "Property" => {}
                // A standard primitive placed directly, positioned by the Xform and
                // Location before it (unknown elements are counted as unsupported).
                _ => {
                    if let Some(r) = self.standard_primitive(c, scale) {
                        let shape = self.flash(&r, &xf);
                        self.emit(out, dark, shape);
                    }
                }
            }
        }
    }

    fn primitive_ref(&mut self, pad: Node<'_, '_>, _scale: f64, xf: &Xform) -> Option<Shape> {
        let r = pad
            .children()
            .find(|k| k.is_element() && k.tag_name().name().ends_with("PrimitiveRef"))?;
        self.primitive_ref_node(r, xf)
    }

    fn primitive_ref_node(&mut self, r: Node<'_, '_>, xf: &Xform) -> Option<Shape> {
        let id = r.attribute("id")?;
        let region = match r.tag_name().name() {
            "StandardPrimitiveRef" => self.standard.get(id).cloned(),
            "UserPrimitiveRef" => self.user.get(id).cloned(),
            _ => None,
        };
        match region {
            Some(reg) => Some(self.flash(&reg, xf)),
            None => {
                self.diags.push(Diagnostic::warning(
                    "ipc.primitive_missing",
                    format!("primitive {id:?} is not in the dictionaries"),
                ));
                None
            }
        }
    }
}

fn regular(c: Point, diameter: f64, n: usize) -> Pline {
    crate::gerber::macros::regular_polygon(c, diameter, n, if n == 8 { 22.5 } else { 30.0 })
}

fn obround(w: f64, h: f64) -> Pline {
    if (w - h).abs() < 1e-12 {
        return pline::circle(Point::new(0.0, 0.0), w / 2.0);
    }
    let mut p = Pline::new_closed();
    if w > h {
        let r = h / 2.0;
        let dx = w / 2.0 - r;
        p.add(-dx, -r, 0.0);
        p.add(dx, -r, 1.0);
        p.add(dx, r, 0.0);
        p.add(-dx, r, 1.0);
    } else {
        let r = w / 2.0;
        let dy = h / 2.0 - r;
        p.add(r, -dy, 0.0);
        p.add(r, dy, 1.0);
        p.add(-r, dy, 0.0);
        p.add(-r, -dy, 1.0);
    }
    p
}

/// Rectangle with rounded corners where `corners` (lower-left, lower-right, upper-right,
/// upper-left) is set; counter-clockwise.
fn rounded_rect(w: f64, h: f64, r: f64, corners: [bool; 4]) -> Pline {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let q = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
    let mut p = Pline::new_closed();
    // Walk: bottom edge left→right, right edge up, top edge right→left, left edge down.
    let corner = |p: &mut Pline, cx: f64, cy: f64, sx: f64, sy: f64, round: bool| {
        // Corner at (cx, cy) with the two tangent points along (sx, 0) and (0, sy) directions
        // of approach; the arc goes from the incoming edge point to the outgoing one.
        if round && r > 0.0 {
            p.add(cx - sx * r, cy, q);
            p.add(cx, cy - sy * r, 0.0);
        } else {
            p.add(cx, cy, 0.0);
        }
    };
    // lower-right corner: coming along +x on the bottom edge, leaving along +y.
    corner(&mut p, hw, -hh, 1.0, -1.0, corners[1]);
    // upper-right: coming along +y, leaving along -x.
    if corners[2] && r > 0.0 {
        p.add(hw, hh - r, q);
        p.add(hw - r, hh, 0.0);
    } else {
        p.add(hw, hh, 0.0);
    }
    // upper-left: coming along -x, leaving along -y.
    if corners[3] && r > 0.0 {
        p.add(-hw + r, hh, q);
        p.add(-hw, hh - r, 0.0);
    } else {
        p.add(-hw, hh, 0.0);
    }
    // lower-left: coming along -y, leaving along +x.
    if corners[0] && r > 0.0 {
        p.add(-hw, -hh + r, q);
        p.add(-hw + r, -hh, 0.0);
    } else {
        p.add(-hw, -hh, 0.0);
    }
    p
}

fn chamfered_rect(w: f64, h: f64, c: f64, corners: [bool; 4]) -> Pline {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let mut pts = Vec::new();
    // lower-left → lower-right → upper-right → upper-left
    if corners[0] && c > 0.0 {
        pts.push(Point::new(-hw + c, -hh));
    } else {
        pts.push(Point::new(-hw, -hh));
    }
    if corners[1] && c > 0.0 {
        pts.push(Point::new(hw - c, -hh));
        pts.push(Point::new(hw, -hh + c));
    } else {
        pts.push(Point::new(hw, -hh));
    }
    if corners[2] && c > 0.0 {
        pts.push(Point::new(hw, hh - c));
        pts.push(Point::new(hw - c, hh));
    } else {
        pts.push(Point::new(hw, hh));
    }
    if corners[3] && c > 0.0 {
        pts.push(Point::new(-hw + c, hh));
        pts.push(Point::new(-hw, hh - c));
    } else {
        pts.push(Point::new(-hw, hh));
    }
    if corners[0] && c > 0.0 {
        pts.push(Point::new(-hw, -hh + c));
    }
    pline::from_points(pts, true)
}

fn ellipse(w: f64, h: f64, tol: f64) -> Pline {
    // Four Bézier quarters with kappa, flattened to the tolerance.
    let (a, b) = (w / 2.0, h / 2.0);
    let k = 0.552_284_75;
    let quarters = [
        (
            Point::new(a, 0.0),
            Point::new(a, b * k),
            Point::new(a * k, b),
            Point::new(0.0, b),
        ),
        (
            Point::new(0.0, b),
            Point::new(-a * k, b),
            Point::new(-a, b * k),
            Point::new(-a, 0.0),
        ),
        (
            Point::new(-a, 0.0),
            Point::new(-a, -b * k),
            Point::new(-a * k, -b),
            Point::new(0.0, -b),
        ),
        (
            Point::new(0.0, -b),
            Point::new(a * k, -b),
            Point::new(a, -b * k),
            Point::new(a, 0.0),
        ),
    ];
    let mut pts = vec![Point::new(a, 0.0)];
    for (p0, p1, p2, p3) in quarters {
        flatten_cubic(p0, p1, p2, p3, tol, &mut |q| pts.push(q));
    }
    pts.pop();
    pline::from_points(pts, true)
}

/// Shapes of one layer into primitives, compositing in source order when negative sets
/// exist (as the Gerber reader does). The flag reports a boolean that could not be
/// resolved completely (see `Region::incomplete`).
fn composite(items: Vec<(bool, Shape)>) -> (Vec<Primitive>, bool) {
    let to_prim = |s: Shape| -> Primitive {
        match s {
            Shape::Fill(r) => Primitive::Hatch {
                loops: r.outers().chain(r.holes()).cloned().collect(),
                rule: LoopRule::EvenOdd,
                solid: true,
                pattern: String::new(),
            },
            Shape::Stroke { pline, width } => Primitive::Path {
                pline,
                width: Some(width),
            },
            Shape::Centreline(pline) => Primitive::Path { pline, width: None },
        }
    };
    if items.iter().all(|(dark, _)| *dark) {
        let incomplete = items
            .iter()
            .any(|(_, s)| matches!(s, Shape::Fill(r) if r.incomplete));
        return (
            items.into_iter().map(|(_, s)| to_prim(s)).collect(),
            incomplete,
        );
    }
    let mut acc = Region::empty();
    let mut centrelines = Vec::new();
    for (dark, shape) in items {
        let r = match shape {
            Shape::Fill(r) => r,
            Shape::Stroke { pline, width } => region::stroke(&pline, width),
            Shape::Centreline(p) => {
                centrelines.push(Primitive::Path {
                    pline: p,
                    width: None,
                });
                continue;
            }
        };
        acc = if dark {
            if acc.is_empty() { r } else { acc.union(&r) }
        } else if acc.is_empty() {
            acc
        } else {
            acc.difference(&r)
        };
    }
    let incomplete = acc.incomplete;
    let mut out = Vec::new();
    if !acc.is_empty() {
        out.push(to_prim(Shape::Fill(acc)));
    }
    out.extend(centrelines);
    (out, incomplete)
}

fn incomplete_boolean(layer: &str) -> Diagnostic {
    Diagnostic::warning(
        "ipc.incomplete_boolean",
        "part of the layer could not be composited and was left out: boundaries within nanometres of each other could not be intersected consistently",
    )
    .with_context(layer)
}

pub fn read_bytes(
    name: &str,
    bytes: &[u8],
    opts: &ReadOptions,
) -> Result<SourceDocument, ReadError> {
    let err = |message: String| ReadError {
        format: Some(SourceFormat::Ipc2581),
        message,
    };
    let text = std::str::from_utf8(bytes).map_err(|_| err(format!("{name}: not UTF-8")))?;
    let doc =
        Document::parse(text).map_err(|e| err(format!("{name}: not well-formed XML: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "IPC-2581" {
        return Err(err(format!(
            "{name}: root element is <{}>, not <IPC-2581>",
            root.tag_name().name()
        )));
    }
    let version = root.attribute("revision").map(str::to_string);
    let header_units = root
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "CadHeader")
        .and_then(|n| unit_scale(n.attribute("units")));
    let mut diags = Vec::new();
    let (scale, units) = header_units.unwrap_or_else(|| {
        diags.push(Diagnostic::warning(
            "ipc.units_missing",
            "CadHeader declares no units; assuming millimetres",
        ));
        (1.0, Some(Unit::Millimetre))
    });
    let mut reader = Reader {
        scale,
        tol_mm: opts.tolerance_mm,
        standard: HashMap::new(),
        user: HashMap::new(),
        lines: HashMap::new(),
        counts: Counts::default(),
        budget: Budget::new(opts.max_work),
        diags: Vec::new(),
        _doc: &doc,
    };
    reader.dictionaries(root);

    // Layer declarations: name → function string.
    let mut functions: HashMap<String, String> = HashMap::new();
    for l in root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Layer")
    {
        if let Some(n) = l.attribute("name") {
            let mut f = l.attribute("layerFunction").unwrap_or("").to_string();
            if let Some(side) = l.attribute("side") {
                if !f.is_empty() {
                    f.push(',');
                }
                f.push_str(side);
            }
            functions.insert(n.to_string(), f);
        }
    }
    let steps: Vec<Node> = root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Step")
        .collect();
    let multi = steps.len() > 1;
    let mut layers: BTreeMap<String, SourceLayer> = BTreeMap::new();
    let ensure_layer =
        |layers: &mut BTreeMap<String, SourceLayer>, name: String, function: Option<String>| {
            layers.entry(name.clone()).or_insert_with(|| SourceLayer {
                name,
                function,
                ..Default::default()
            });
        };
    for step in &steps {
        let prefix = if multi {
            format!("{} ", step.attribute("name").unwrap_or("step"))
        } else {
            String::new()
        };
        reader.counts.components += step
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "Component")
            .count();
        // Board profile.
        if let Some(profile) = step
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "Profile")
        {
            let mut items = Vec::new();
            for c in profile.children().filter(|c| c.is_element()) {
                match c.tag_name().name() {
                    "Polygon" => {
                        if let Some(l) = reader.closed(c, scale) {
                            items.push((
                                true,
                                Shape::Fill(Region::from_loops([l], LoopRule::NonZero)),
                            ));
                        }
                    }
                    "Contour" => {
                        let r = reader.contour(c, scale);
                        items.push((true, Shape::Fill(r)));
                    }
                    _ => {}
                }
            }
            let name = format!("{prefix}Profile");
            ensure_layer(&mut layers, name.clone(), Some("PROFILE".into()));
            let (prims, incomplete) = composite(items);
            if incomplete {
                diags.push(incomplete_boolean(&name));
            }
            layers.get_mut(&name).unwrap().primitives.extend(prims);
        }
        for lf in step
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "LayerFeature")
        {
            let Some(layer_ref) = lf.attribute("layerRef") else {
                continue;
            };
            let mut items: Vec<(bool, Shape)> = Vec::new();
            for set in lf.children().filter(|c| c.is_element()) {
                match set.tag_name().name() {
                    "Set" => {
                        let dark = !set
                            .attribute("polarity")
                            .is_some_and(|p| p.eq_ignore_ascii_case("NEGATIVE"));
                        reader.features(set, scale, Xform::IDENTITY, &mut items, dark);
                    }
                    _ => reader.features(lf, scale, Xform::IDENTITY, &mut items, true),
                }
            }
            if reader.budget.exhausted() {
                return Err(reader.budget.error(SourceFormat::Ipc2581));
            }
            if items.is_empty() {
                continue;
            }
            let name = format!("{prefix}{layer_ref}");
            let function = functions.get(layer_ref).cloned().filter(|f| !f.is_empty());
            ensure_layer(&mut layers, name.clone(), function);
            let (prims, incomplete) = composite(items);
            if incomplete {
                diags.push(incomplete_boolean(&name));
            }
            layers.get_mut(&name).unwrap().primitives.extend(prims);
        }
    }
    diags.extend(reader.diags);
    if reader.counts.components > 0 {
        diags.push(Diagnostic::info(
            "ipc.components",
            format!(
                "{} component placement(s) ignored (not geometry)",
                reader.counts.components
            ),
        ));
    }
    if reader.counts.mirrored > 0 {
        diags.push(Diagnostic::info(
            "ipc.mirror",
            "Xform mirror is read as a mirror about the y axis before rotation",
        ));
    }
    for (what, n) in &reader.counts.unsupported {
        diags.push(Diagnostic::warning(
            "ipc.unsupported",
            format!("{n} <{what}> element(s) are not supported and were skipped"),
        ));
    }
    if steps.is_empty() {
        diags.push(Diagnostic::warning(
            "ipc.no_step",
            "the file has no <Step>; nothing to read",
        ));
    }
    if multi {
        diags.push(Diagnostic::info(
            "ipc.steps",
            format!(
                "{} steps; layer names are prefixed with the step name",
                steps.len()
            ),
        ));
    }
    let layers: Vec<SourceLayer> = layers.into_values().collect();
    if layers.is_empty() {
        diags.push(Diagnostic::warning(
            "ipc.empty",
            "no layer features or profile found",
        ));
    }
    Ok(SourceDocument {
        format: SourceFormat::Ipc2581,
        version,
        units,
        layers,
        diagnostics: diags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn read(xml: &str) -> SourceDocument {
        read_bytes("t.xml", xml.as_bytes(), &ReadOptions::default()).unwrap()
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

    const HEAD: &str = r#"<?xml version="1.0"?>
<IPC-2581 revision="C" xmlns="http://webstds.ipc.org/2581">
 <Content>
  <DictionaryStandard units="MILLIMETER">
   <EntryStandard id="c1"><Circle diameter="1.0"/></EntryStandard>
   <EntryStandard id="r2"><RectCenter width="2.0" height="1.0"/></EntryStandard>
   <EntryStandard id="o"><Oval width="3" height="1"/></EntryStandard>
   <EntryStandard id="d"><Donut shape="ROUND" outerDiameter="2" innerDiameter="1"/></EntryStandard>
  </DictionaryStandard>
  <DictionaryUser units="MILLIMETER">
   <EntryUser id="tri"><UserSpecial><Polygon><PolyBegin x="0" y="0"/><PolyStepSegment x="2" y="0"/><PolyStepSegment x="0" y="2"/><PolyStepSegment x="0" y="0"/></Polygon></UserSpecial></EntryUser>
  </DictionaryUser>
  <DictionaryLineDesc units="MILLIMETER">
   <EntryLineDesc id="round"><LineDesc lineEnd="ROUND" lineWidth="0.2"/></EntryLineDesc>
   <EntryLineDesc id="square"><LineDesc lineEnd="SQUARE" lineWidth="0.4"/></EntryLineDesc>
   <EntryLineDesc id="none"><LineDesc lineEnd="NONE" lineWidth="0.4"/></EntryLineDesc>
  </DictionaryLineDesc>
 </Content>
 <Ecad name="board">
  <CadHeader units="MILLIMETER"/>
  <CadData>
   <Layer name="TOP" layerFunction="CONDUCTOR" side="TOP" polarity="POSITIVE"/>
   <Layer name="SM_TOP" layerFunction="SOLDERMASK" side="TOP" polarity="POSITIVE"/>
   <Step name="s1">
    <Profile><Polygon><PolyBegin x="0" y="0"/><PolyStepSegment x="30" y="0"/><PolyStepCurve x="30" y="20" centerX="30" centerY="10" clockwise="false"/><PolyStepSegment x="0" y="20"/><PolyStepSegment x="0" y="0"/></Polygon></Profile>
"#;

    #[test]
    fn imports_past_the_work_limit_are_refused() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/ipc2581/board.xml"
        ));
        assert!(read_bytes("t.xml", bytes, &ReadOptions::default()).is_ok());
        let err = read_bytes(
            "t.xml",
            bytes,
            &ReadOptions {
                max_work: 50,
                ..ReadOptions::default()
            },
        )
        .unwrap_err();
        assert!(err.message.contains("import refused"), "{}", err.message);
    }

    #[test]
    fn pads_lines_and_profile_read_as_layers_with_functions() {
        let xml = format!(
            r#"{HEAD}
    <LayerFeature layerRef="TOP">
     <Set polarity="POSITIVE">
      <Pad><Location x="5" y="5"/><StandardPrimitiveRef id="c1"/></Pad>
      <Pad><Xform rotation="90"/><Location x="10" y="5"/><StandardPrimitiveRef id="r2"/></Pad>
      <Pad><Location x="15" y="5"/><StandardPrimitiveRef id="o"/></Pad>
      <Pad><Location x="20" y="5"/><StandardPrimitiveRef id="d"/></Pad>
      <Pad><Location x="24" y="4"/><UserPrimitiveRef id="tri"/></Pad>
      <Features>
       <Line startX="2" startY="12" endX="12" endY="12"><LineDescRef id="round"/></Line>
       <Line startX="2" startY="15" endX="12" endY="15"><LineDescRef id="square"/></Line>
       <Polyline><PolyBegin x="14" y="12"/><PolyStepSegment x="20" y="12"/><PolyStepSegment x="20" y="16"/><LineDescRef id="none"/></Polyline>
       <Arc startX="26" startY="12" endX="26" endY="16" centerX="26" centerY="14" clockwise="false"><LineDescRef id="round"/></Arc>
      </Features>
     </Set>
    </LayerFeature>
   </Step>
  </CadData>
 </Ecad>
</IPC-2581>"#
        );
        let doc = read(&xml);
        assert_eq!(doc.format, SourceFormat::Ipc2581);
        assert_eq!(doc.units, Some(Unit::Millimetre));
        assert_eq!(doc.version.as_deref(), Some("C"));
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let names: Vec<(&str, Option<&str>)> = doc
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.function.as_deref()))
            .collect();
        assert_eq!(
            names,
            vec![("Profile", Some("PROFILE")), ("TOP", Some("CONDUCTOR,TOP"))]
        );

        // Profile: 30 x 20 with a semicircular right end (r = 10): 30*20 - 10*20 + pi*50... the
        // arc bulges outwards from x = 30: area = 600 + pi * 100 / 2.
        let profile = region_of(&doc.layers[0]);
        assert!(
            (profile.area() - (600.0 + PI * 50.0)).abs() < 1e-6,
            "{}",
            profile.area()
        );

        let top = &doc.layers[1];
        let fills = region_of(top);
        // circle 1 (pi/4) + rect 2x1 (2) + oval (2 + pi/4) + donut (pi*(1-0.25)) + triangle (2)
        // + square-ended line (10.4 x 0.4) + flat-ended polyline (10 x 0.4 mitred: 6.4*0.4+0.4*3.6? → use the exact polygon: 6+0.2 wide... computed below)
        let expected_pads = PI / 4.0 + 2.0 + (2.0 + PI / 4.0) + PI * 0.75 + 2.0;
        let square_line = 10.4 * 0.4;
        // Polyline (14,12)→(20,12)→(20,16), width 0.4, flat ends, mitred corner:
        // horizontal leg 6.2 x 0.4 (mitre fills to x = 20.2) + vertical leg 0.4 x 3.8.
        let polyline = 6.2 * 0.4 + 0.4 * 3.8;
        assert!(
            (fills.area() - (expected_pads + square_line + polyline)).abs() < 1e-6,
            "{}",
            fills.area()
        );
        // The rotated rectangle is 1 wide and 2 tall now.
        let strokes: Vec<(f64, bool)> = top
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Path { pline, width } => Some((width.unwrap(), pline.is_closed())),
                _ => None,
            })
            .collect();
        assert_eq!(
            strokes.len(),
            2,
            "round-ended line and arc stay centrelines"
        );
        assert!(
            strokes
                .iter()
                .all(|(w, closed)| (*w - 0.2).abs() < 1e-9 && !closed)
        );
        let arc_len: f64 = top
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Path { pline, .. }
                    if pline.vertex_count() == 2 && pline.at(0).bulge != 0.0 =>
                {
                    Some(pline::length(pline))
                }
                _ => None,
            })
            .sum();
        assert!(
            (arc_len - PI * 2.0).abs() < 1e-9,
            "half circle of r = 2: {arc_len}"
        );
    }

    #[test]
    fn negative_sets_are_composited_and_unknowns_are_diagnosed() {
        let xml = format!(
            r#"{HEAD}
    <LayerFeature layerRef="SM_TOP">
     <Set polarity="POSITIVE"><Features><Contour><Polygon><PolyBegin x="0" y="0"/><PolyStepSegment x="10" y="0"/><PolyStepSegment x="10" y="10"/><PolyStepSegment x="0" y="10"/><PolyStepSegment x="0" y="0"/></Polygon><Cutout><PolyBegin x="1" y="1"/><PolyStepSegment x="2" y="1"/><PolyStepSegment x="2" y="2"/><PolyStepSegment x="1" y="2"/><PolyStepSegment x="1" y="1"/></Cutout></Contour></Features></Set>
     <Set polarity="NEGATIVE"><Pad><Location x="5" y="5"/><StandardPrimitiveRef id="d"/></Pad></Set>
     <Set polarity="POSITIVE"><Features><Text>x</Text></Features><Pad><Location x="9" y="9"/><StandardPrimitiveRef id="nope"/></Pad></Set>
    </LayerFeature>
    <Component refDes="R1" packageRef="p"/>
   </Step>
  </CadData>
 </Ecad>
</IPC-2581>"#
        );
        let doc = read(&xml);
        let sm = doc.layers.iter().find(|l| l.name == "SM_TOP").unwrap();
        assert_eq!(sm.function.as_deref(), Some("SOLDERMASK,TOP"));
        assert_eq!(sm.primitives.len(), 1, "composited into one region");
        let r = region_of(sm);
        // 100 - 1 (cutout) - donut area (pi * 0.75).
        assert!((r.area() - (99.0 - PI * 0.75)).abs() < 1e-6, "{}", r.area());
        assert_eq!(r.holes().count(), 2);
        let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"ipc.unsupported"));
        assert!(codes.contains(&"ipc.primitive_missing"));
        assert!(codes.contains(&"ipc.components"));
    }

    #[test]
    fn features_grouped_in_user_specials_and_placed_primitives_are_read() {
        // As Altium writes a layer: traces and arcs grouped in UserSpecial straight inside
        // Features, and a standard primitive placed by the Xform and Location before it.
        let xml = format!(
            r#"{HEAD}
    <LayerFeature layerRef="TOP">
     <Set net="N1"><Features><UserSpecial>
      <Line startX="0" startY="0" endX="10" endY="0"><LineDesc lineEnd="ROUND" lineWidth="0.2"/></Line>
      <Arc startX="10" startY="0" endX="10" endY="0" centerX="11" centerY="0" clockwise="false"><LineDesc lineEnd="ROUND" lineWidth="0.2"/></Arc>
      <Features><Xform rotation="90"/><Location x="5" y="5"/><RectCenter width="2" height="1"/></Features>
     </UserSpecial></Features></Set>
     <Set net="N2"><Features><Xform rotation="90"/><Location x="20" y="5"/><RectCenter width="2" height="1"/></Features></Set>
    </LayerFeature>
   </Step>
  </CadData>
 </Ecad>
</IPC-2581>"#
        );
        let doc = read(&xml);
        assert!(
            !doc.diagnostics.iter().any(|d| d.code == "ipc.unsupported"),
            "{:?}",
            doc.diagnostics
        );
        let top = doc.layers.iter().find(|l| l.name == "TOP").unwrap();
        let r = region_of(top);
        // Two rectangles, turned upright, at (5, 5) and (20, 5).
        for x in [5.0, 20.0] {
            assert!(r.contains(Point::new(x, 5.9)) && !r.contains(Point::new(x + 0.6, 5.0)));
        }
        // The line and the full circle round (11, 0), both 0.2 wide strokes.
        let strokes: Vec<f64> = top
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Path {
                    pline,
                    width: Some(w),
                } if (*w - 0.2).abs() < 1e-12 => Some(pline::length(pline)),
                _ => None,
            })
            .collect();
        assert_eq!(strokes.len(), 2, "{:?}", top.primitives);
        assert!(strokes.iter().any(|l| (l - 10.0).abs() < 1e-9));
        assert!(strokes.iter().any(|l| (l - 2.0 * PI).abs() < 1e-9));
    }

    #[test]
    fn malformed_input_is_refused() {
        assert!(read_bytes("x.xml", b"<svg/>", &ReadOptions::default()).is_err());
        assert!(read_bytes("x.xml", b"<IPC-2581", &ReadOptions::default()).is_err());
        let doc = read(
            r#"<IPC-2581 revision="B"><Ecad><CadHeader units="INCH"/><CadData/></Ecad></IPC-2581>"#,
        );
        assert_eq!(doc.units, Some(Unit::Inch));
        assert!(doc.diagnostics.iter().any(|d| d.code == "ipc.no_step"));
    }
}
