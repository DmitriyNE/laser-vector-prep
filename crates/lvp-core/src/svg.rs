//! SVG reader on top of `usvg` (D22): usvg resolves CSS, `use`/`symbol`, nested
//! transforms, the viewBox and turns every shape into a path; this module turns that tree
//! into source layers in millimetres with y up.
//!
//! Layers: each top-level group becomes a layer named by its Inkscape label or id; loose
//! top-level elements go to a layer named after the file. Filled paths become filled loops
//! (with the SVG fill rule), stroked paths become centrelines with their stroke width, paths
//! with neither are centrelines without width. Clip paths are applied exactly to fills;
//! text, images, masks, filters and foreign objects never silently become geometry.

use std::collections::HashMap;

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};
use usvg::roxmltree;
use usvg::tiny_skia_path::{self, PathSegment};

use crate::diag::Diagnostic;
use crate::geom::Point;
use crate::model::SourceFormat;
use crate::pline::{self, Pline};
use crate::region::{LoopRule, Region};
use crate::source::{Primitive, ReadError, ReadOptions, SourceDocument, SourceLayer};
use crate::units::Unit;

/// CSS pixels per inch; usvg converts every length to user units (px) at this density.
const PX_PER_INCH: f64 = 96.0;
const MM_PER_PX: f64 = 25.4 / PX_PER_INCH;
const INKSCAPE_NS: &str = "http://www.inkscape.org/namespaces/inkscape";

/// What the raw XML says that usvg would quietly drop or flatten.
#[derive(Default)]
struct Scan {
    labels: HashMap<String, String>,
    text: usize,
    image: usize,
    filter: usize,
    mask: usize,
    foreign: usize,
    gradients_or_patterns: usize,
    version: Option<String>,
    units: Option<Unit>,
    unitless: bool,
}

fn scan(doc: &roxmltree::Document<'_>) -> Scan {
    let mut s = Scan::default();
    let root = doc.root_element();
    s.version = root.attribute("version").map(str::to_string);
    if let Some(w) = root.attribute("width") {
        let unit =
            w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
        s.units = match unit.trim() {
            "mm" => Some(Unit::Millimetre),
            "cm" => Some(Unit::Centimetre),
            "in" => Some(Unit::Inch),
            "" | "px" => {
                s.unitless = true;
                None
            }
            _ => None,
        };
    } else {
        s.unitless = true;
    }
    for n in doc.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "text" => s.text += 1,
            "image" => s.image += 1,
            "filter" => s.filter += 1,
            "mask" => s.mask += 1,
            "foreignObject" => s.foreign += 1,
            "linearGradient" | "radialGradient" | "pattern" => s.gradients_or_patterns += 1,
            _ => {}
        }
        if let Some(id) = n.attribute("id")
            && let Some(label) = n.attribute((INKSCAPE_NS, "label"))
        {
            s.labels.insert(id.to_string(), label.to_string());
        }
    }
    s
}

/// Flatten a tiny-skia path (already in absolute px coordinates) into polylines in mm.
/// `height_px` flips y so the drawing is y-up; `tol_px` bounds the Bézier chord error.
fn flatten_path(path: &tiny_skia_path::Path, height_px: f64, tol_px: f64) -> Vec<(Pline, bool)> {
    let to_mm = |p: tiny_skia_path::Point| {
        Point::new(p.x as f64 * MM_PER_PX, (height_px - p.y as f64) * MM_PER_PX)
    };
    let mut out: Vec<(Pline, bool)> = Vec::new();
    let mut pts: Vec<Point> = Vec::new();
    let mut last = tiny_skia_path::Point::zero();
    let mut closed = false;
    let flush = |pts: &mut Vec<Point>, closed: bool, out: &mut Vec<(Pline, bool)>| {
        if pts.len() >= 2 {
            let p = pline::from_points(pts.drain(..), closed);
            if let Some(p) = pline::sanitize(&p) {
                out.push((p, closed));
            }
        }
        pts.clear();
    };
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                flush(&mut pts, closed, &mut out);
                closed = false;
                pts.push(to_mm(p));
                last = p;
            }
            PathSegment::LineTo(p) => {
                pts.push(to_mm(p));
                last = p;
            }
            PathSegment::QuadTo(c, p) => {
                // Elevate to a cubic.
                let c1 = tiny_skia_path::Point::from_xy(
                    last.x + 2.0 / 3.0 * (c.x - last.x),
                    last.y + 2.0 / 3.0 * (c.y - last.y),
                );
                let c2 = tiny_skia_path::Point::from_xy(
                    p.x + 2.0 / 3.0 * (c.x - p.x),
                    p.y + 2.0 / 3.0 * (c.y - p.y),
                );
                cubic(last, c1, c2, p, tol_px, &mut |q| pts.push(to_mm(q)), 0);
                last = p;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                cubic(last, c1, c2, p, tol_px, &mut |q| pts.push(to_mm(q)), 0);
                last = p;
            }
            PathSegment::Close => {
                flush(&mut pts, true, &mut out);
                closed = false;
            }
        }
    }
    flush(&mut pts, closed, &mut out);
    out
}

/// Adaptive subdivision of a cubic Bézier; emits every point after the start.
fn cubic(
    p0: tiny_skia_path::Point,
    p1: tiny_skia_path::Point,
    p2: tiny_skia_path::Point,
    p3: tiny_skia_path::Point,
    tol: f64,
    emit: &mut dyn FnMut(tiny_skia_path::Point),
    depth: u32,
) {
    let d = |a: tiny_skia_path::Point, b: tiny_skia_path::Point, c: tiny_skia_path::Point| -> f64 {
        // Distance of c from the chord a-b.
        let (ax, ay, bx, by, cx, cy) = (
            a.x as f64, a.y as f64, b.x as f64, b.y as f64, c.x as f64, c.y as f64,
        );
        let (dx, dy) = (bx - ax, by - ay);
        let l = dx.hypot(dy);
        if l < 1e-12 {
            return (cx - ax).hypot(cy - ay);
        }
        ((cx - ax) * dy - (cy - ay) * dx).abs() / l
    };
    let flat = d(p0, p3, p1).max(d(p0, p3, p2)) <= tol;
    if flat || depth >= 16 {
        emit(p3);
        return;
    }
    let mid = |a: tiny_skia_path::Point, b: tiny_skia_path::Point| {
        tiny_skia_path::Point::from_xy((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
    };
    let p01 = mid(p0, p1);
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let m = mid(p012, p123);
    cubic(p0, p01, p012, m, tol, emit, depth + 1);
    cubic(m, p123, p23, p3, tol, emit, depth + 1);
}

/// Uniform scale factor of a transform (geometric mean of the axis scales), for widths.
fn scale_of(t: tiny_skia_path::Transform) -> f64 {
    let (sx, sy) = t.get_scale();
    (sx as f64 * sy as f64).abs().sqrt()
}

struct Walker<'a> {
    tol_px: f64,
    height_px: f64,
    diags: Vec<Diagnostic>,
    counts: Counts,
    layers: Vec<SourceLayer>,
    scan: &'a Scan,
}

#[derive(Default)]
struct Counts {
    non_solid_paint: usize,
    round_caps_substituted: usize,
    unpainted: usize,
    clipped_linework: usize,
    masked: usize,
    filtered: usize,
}

impl Walker<'_> {
    fn layer_name(&self, g: &usvg::Group, fallback: &str) -> String {
        let id = g.id();
        if id.is_empty() {
            return fallback.to_string();
        }
        self.scan
            .labels
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    /// Region of a clip path in mm (its children's fills), in the coordinate system given
    /// by `parent`: the referencing group's absolute transform.
    fn clip_region(&mut self, clip: &usvg::ClipPath, parent: tiny_skia_path::Transform) -> Region {
        let base = parent.pre_concat(clip.transform());
        let mut region = Region::empty();
        let mut stack: Vec<&usvg::Node> = clip.root().children().iter().collect();
        while let Some(node) = stack.pop() {
            match node {
                usvg::Node::Group(g) => stack.extend(g.children().iter()),
                usvg::Node::Path(p) => {
                    let Some(data) = p
                        .data()
                        .clone()
                        .transform(base.pre_concat(p.abs_transform()))
                    else {
                        continue;
                    };
                    let rule = p
                        .fill()
                        .map(|f| f.rule())
                        .unwrap_or(usvg::FillRule::NonZero);
                    let loops: Vec<Pline> = flatten_path(&data, self.height_px, self.tol_px)
                        .into_iter()
                        .map(|(mut l, _)| {
                            l.set_is_closed(true);
                            l
                        })
                        .collect();
                    let r = Region::from_loops(loops, fill_rule(rule));
                    region = if region.is_empty() {
                        r
                    } else {
                        region.union(&r)
                    };
                }
                _ => {}
            }
        }
        if let Some(nested) = clip.clip_path() {
            let inner = self.clip_region(nested, parent);
            region = region.intersection(&inner);
        }
        region
    }

    fn walk_group(&mut self, g: &usvg::Group, layer: usize, clip: Option<Region>) {
        if g.mask().is_some() {
            self.counts.masked += 1;
        }
        if !g.filters().is_empty() {
            self.counts.filtered += 1;
        }
        let clip = match g.clip_path() {
            Some(c) => {
                let r = self.clip_region(c, g.abs_transform());
                Some(match clip {
                    Some(outer) => outer.intersection(&r),
                    None => r,
                })
            }
            None => clip,
        };
        for node in g.children() {
            match node {
                usvg::Node::Group(child) => self.walk_group(child, layer, clip.clone()),
                usvg::Node::Path(p) => self.path(p, layer, clip.as_ref()),
                usvg::Node::Image(_) | usvg::Node::Text(_) => {}
            }
        }
    }

    fn path(&mut self, p: &usvg::Path, layer: usize, clip: Option<&Region>) {
        if !p.is_visible() {
            return;
        }
        let Some(data) = p.data().clone().transform(p.abs_transform()) else {
            return;
        };
        let parts = flatten_path(&data, self.height_px, self.tol_px);
        if parts.is_empty() {
            return;
        }
        let mut emitted = false;
        // Fills: only subpaths with an area. `line` and two-point paths carry SVG's default
        // black fill but enclose nothing.
        let fill_loops: Vec<Pline> = parts
            .iter()
            .filter_map(|(l, _)| {
                let mut l = l.clone();
                l.set_is_closed(true);
                (l.vertex_count() >= 3 && l.area().abs() > 1e-12).then_some(l)
            })
            .collect();
        if let (Some(fill), false) = (p.fill(), fill_loops.is_empty()) {
            if !matches!(fill.paint(), usvg::Paint::Color(_)) {
                self.counts.non_solid_paint += 1;
            }
            let loops = fill_loops;
            let rule = fill_rule(fill.rule());
            let prim = match clip {
                Some(c) => {
                    let r = Region::from_loops(loops, rule).intersection(c);
                    if r.is_empty() {
                        None
                    } else {
                        Some(Primitive::Hatch {
                            loops: r.outers().chain(r.holes()).cloned().collect(),
                            rule: LoopRule::EvenOdd,
                            solid: true,
                            pattern: String::new(),
                        })
                    }
                }
                None => Some(Primitive::Hatch {
                    loops,
                    rule,
                    solid: true,
                    pattern: String::new(),
                }),
            };
            if let Some(prim) = prim {
                self.layers[layer].primitives.push(prim);
                emitted = true;
            }
        }
        if let Some(stroke) = p.stroke() {
            let width = stroke.width().get() as f64 * scale_of(p.abs_transform()) * MM_PER_PX;
            if width > 0.0 {
                if !matches!(stroke.paint(), usvg::Paint::Color(_)) {
                    self.counts.non_solid_paint += 1;
                }
                if stroke.linecap() != usvg::LineCap::Round
                    || stroke.linejoin() != usvg::LineJoin::Round
                {
                    self.counts.round_caps_substituted += 1;
                }
                if clip.is_some() {
                    self.counts.clipped_linework += 1;
                }
                for (l, _) in &parts {
                    self.layers[layer].primitives.push(Primitive::Path {
                        pline: l.clone(),
                        width: Some(width),
                    });
                }
                emitted = true;
            }
        }
        if !emitted {
            // Neither fill nor stroke: keep the centreline, it is often a cut or guide line.
            self.counts.unpainted += 1;
            if clip.is_some() {
                self.counts.clipped_linework += 1;
            }
            for (l, _) in &parts {
                self.layers[layer].primitives.push(Primitive::Path {
                    pline: l.clone(),
                    width: None,
                });
            }
        }
    }
}

fn fill_rule(r: usvg::FillRule) -> LoopRule {
    match r {
        usvg::FillRule::NonZero => LoopRule::NonZero,
        usvg::FillRule::EvenOdd => LoopRule::EvenOdd,
    }
}

pub fn read_bytes(
    name: &str,
    bytes: &[u8],
    opts: &ReadOptions,
) -> Result<SourceDocument, ReadError> {
    let err = |message: String| ReadError {
        format: Some(SourceFormat::Svg),
        message,
    };
    let text = std::str::from_utf8(bytes).map_err(|_| err("file is not UTF-8".into()))?;
    let xml = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| err(format!("not well-formed XML: {e}")))?;
    let scanned = scan(&xml);
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_xmltree(&xml, &options).map_err(|e| err(format!("{e}")))?;
    let height_px = tree.size().height() as f64;
    let tol_px = (opts.tolerance_mm / MM_PER_PX).max(1e-4);

    let mut walker = Walker {
        tol_px,
        height_px,
        diags: Vec::new(),
        counts: Counts::default(),
        layers: Vec::new(),
        scan: &scanned,
    };
    // Named top-level groups are layers; loose elements go to a layer named after the
    // file. usvg wraps the document in anonymous groups for the viewBox transform, so the
    // search descends through id-less groups until it meets named ones or geometry.
    let loose_name = name.to_string();
    let mut loose_layer: Option<usize> = None;
    fn collect(
        walker: &mut Walker<'_>,
        g: &usvg::Group,
        loose_name: &str,
        loose_layer: &mut Option<usize>,
    ) {
        for node in g.children() {
            match node {
                usvg::Node::Group(child) if !child.id().is_empty() => {
                    let lname = walker.layer_name(child, loose_name);
                    let idx = match walker.layers.iter().position(|l| l.name == lname) {
                        Some(i) => i,
                        None => {
                            walker.layers.push(SourceLayer {
                                name: lname,
                                ..Default::default()
                            });
                            walker.layers.len() - 1
                        }
                    };
                    walker.walk_group(child, idx, None);
                }
                usvg::Node::Group(child)
                    if child.clip_path().is_none() && child.mask().is_none() =>
                {
                    collect(walker, child, loose_name, loose_layer);
                }
                other => {
                    let idx = *loose_layer.get_or_insert_with(|| {
                        walker.layers.push(SourceLayer {
                            name: loose_name.to_string(),
                            ..Default::default()
                        });
                        walker.layers.len() - 1
                    });
                    match other {
                        usvg::Node::Group(child) => walker.walk_group(child, idx, None),
                        usvg::Node::Path(p) => walker.path(p, idx, None),
                        _ => {}
                    }
                }
            }
        }
    }
    collect(&mut walker, tree.root(), &loose_name, &mut loose_layer);
    let mut diags = std::mem::take(&mut walker.diags);
    let c = &walker.counts;
    if scanned.text > 0 {
        diags.push(Diagnostic::error(
            "svg.text",
            format!("{} text element(s) are not converted to geometry; convert text to paths in the editor", scanned.text),
        ));
    }
    if scanned.image > 0 {
        diags.push(Diagnostic::warning(
            "svg.image",
            format!("{} raster image(s) ignored", scanned.image),
        ));
    }
    if scanned.mask > 0 || c.masked > 0 {
        diags.push(Diagnostic::warning(
            "svg.mask",
            "masks are not applied; masked geometry is read unmasked",
        ));
    }
    if scanned.filter > 0 || c.filtered > 0 {
        diags.push(Diagnostic::warning(
            "svg.filter",
            "filters are ignored; geometry is read unfiltered",
        ));
    }
    if scanned.foreign > 0 {
        diags.push(Diagnostic::warning(
            "svg.foreign_object",
            format!("{} foreignObject element(s) ignored", scanned.foreign),
        ));
    }
    if c.clipped_linework > 0 {
        diags.push(Diagnostic::warning(
            "svg.clip_linework",
            format!(
                "clip paths were applied to fills but not to {} stroked or unpainted path(s)",
                c.clipped_linework
            ),
        ));
    }
    if c.non_solid_paint > 0 {
        diags.push(Diagnostic::info(
            "svg.paint",
            format!(
                "{} gradient or pattern paint(s) read as solid",
                c.non_solid_paint
            ),
        ));
    }
    if c.round_caps_substituted > 0 {
        diags.push(Diagnostic::info(
            "svg.caps",
            format!("{} stroke(s) with non-round caps or joins; stroke expansion uses round caps and joins", c.round_caps_substituted),
        ));
    }
    if c.unpainted > 0 {
        diags.push(Diagnostic::info(
            "svg.unpainted",
            format!(
                "{} path(s) without fill or stroke read as centrelines",
                c.unpainted
            ),
        ));
    }
    if scanned.unitless {
        diags.push(Diagnostic::info(
            "svg.px_units",
            "the root size has no unit; 1 user unit = 1 CSS px = 25.4/96 mm",
        ));
    }
    let mut layers = walker.layers;
    layers.sort_by(|a, b| a.name.cmp(&b.name));
    if layers.is_empty() {
        diags.push(Diagnostic::warning(
            "svg.empty",
            "the file contains no drawable geometry",
        ));
    }
    Ok(SourceDocument {
        format: SourceFormat::Svg,
        version: scanned.version,
        units: scanned.units,
        layers,
        diagnostics: diags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn read(svg: &str) -> SourceDocument {
        read_bytes("t.svg", svg.as_bytes(), &ReadOptions::default()).unwrap()
    }

    fn region_of(layer: &SourceLayer) -> Region {
        let mut r = Region::empty();
        for p in &layer.primitives {
            if let Primitive::Hatch { loops, rule, .. } = p {
                let part = Region::from_loops(loops.iter().cloned(), *rule);
                r = if r.is_empty() { part } else { r.union(&part) };
            }
        }
        r
    }

    #[test]
    fn mm_document_with_layers_fills_and_strokes() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape"
            width="50mm" height="20mm" viewBox="0 0 50 20" version="1.1">
          <g id="layer1" inkscape:label="Cut" inkscape:groupmode="layer">
            <rect x="5" y="5" width="10" height="10" fill="none" stroke="#000" stroke-width="0.2"/>
          </g>
          <g id="Fill">
            <circle cx="30" cy="10" r="5" fill="#f00"/>
            <path d="M 40 5 L 48 5 L 48 15 L 40 15 Z M 42 7 L 46 7 L 46 13 L 42 13 Z" fill="#00f" fill-rule="evenodd"/>
          </g>
          <line x1="0" y1="0" x2="10" y2="20"/>
        </svg>"##;
        let doc = read(svg);
        assert_eq!(doc.format, SourceFormat::Svg);
        assert_eq!(doc.units, Some(Unit::Millimetre));
        assert_eq!(doc.version.as_deref(), Some("1.1"));
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["Cut", "Fill", "t.svg"]);

        // The viewBox maps 1 user unit to 1 mm; y is flipped so the rect's bottom is at 5.
        let cut = &doc.layers[0];
        assert_eq!(cut.primitives.len(), 1);
        let Primitive::Path { pline, width } = &cut.primitives[0] else {
            panic!()
        };
        assert!(pline.is_closed());
        // usvg lengths are f32; widths reproduce to about 1e-7.
        assert!((width.unwrap() - 0.2).abs() < 1e-6, "{width:?}");
        let b = pline::bounds(pline).unwrap();
        assert!(
            (b.min.x - 5.0).abs() < 1e-5
                && (b.min.y - 5.0).abs() < 1e-5
                && (b.max.y - 15.0).abs() < 1e-5,
            "{b:?}"
        );

        let fill = region_of(&doc.layers[1]);
        // Bézier circle (kappa approximation, +0.03 % area) plus an even-odd ring: 8 x 10 - 4 x 6 = 56.
        assert!(
            (fill.area() - (PI * 25.0 + 56.0)).abs() < 0.05,
            "{}",
            fill.area()
        );
        assert_eq!(fill.holes().count(), 1);

        // The loose line is an unpainted centreline; it is not an error, just noted.
        let loose = &doc.layers[2];
        assert!(matches!(
            loose.primitives[0],
            Primitive::Path { width: None, .. }
        ));
        assert!(doc.diagnostics.iter().any(|d| d.code == "svg.unpainted"));
        assert!(
            !doc.diagnostics
                .iter()
                .any(|d| d.severity == crate::diag::Severity::Error)
        );
    }

    #[test]
    fn transforms_use_and_px_units() {
        // 96 px = 1 in = 25.4 mm. A 48 px square scaled 2x by a group transform and reused.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="192" height="192">
          <defs><rect id="sq" width="24" height="24" fill="black"/></defs>
          <g id="A" transform="scale(2)"><use xlink:href="#sq" x="0" y="0"/></g>
          <g id="B" transform="translate(96 96) rotate(45)"><use xlink:href="#sq"/></g>
        </svg>"##;
        let doc = read(svg);
        assert!(doc.units.is_none());
        assert!(doc.diagnostics.iter().any(|d| d.code == "svg.px_units"));
        let a = region_of(&doc.layers[0]);
        let side = 48.0 * MM_PER_PX;
        assert!((a.area() - side * side).abs() < 1e-6, "{}", a.area());
        // usvg transforms are f32: a 45 degree rotation reproduces the area to ~1e-5.
        let b = region_of(&doc.layers[1]);
        let side_b = 24.0 * MM_PER_PX;
        assert!((b.area() - side_b * side_b).abs() < 1e-3, "{}", b.area());
        // Rotated by 45 degrees: the bounding box is a diamond, wider than the side.
        let bb = b.bounds().unwrap();
        assert!((bb.width() - side_b * 2f64.sqrt()).abs() < 1e-3, "{bb:?}");
    }

    #[test]
    fn beziers_are_flattened_within_tolerance_and_clips_apply_to_fills() {
        // A circle drawn with four cubic Béziers (the usual approximation), clipped to its
        // right half by a clipPath rectangle.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm" viewBox="0 0 20 20">
          <defs><clipPath id="half"><rect x="10" y="0" width="10" height="20"/></clipPath></defs>
          <g id="G" clip-path="url(#half)">
            <path fill="#000" d="M 15 10 C 15 12.761 12.761 15 10 15 C 7.239 15 5 12.761 5 10 C 5 7.239 7.239 5 10 5 C 12.761 5 15 7.239 15 10 Z"/>
          </g>
        </svg>"##;
        let doc = read(svg);
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let g = region_of(&doc.layers[0]);
        // Half of a radius-5 circle: 39.27 mm², Bézier approximation error ~0.03 %.
        assert!((g.area() - PI * 25.0 / 2.0).abs() < 0.05, "{}", g.area());
        let b = g.bounds().unwrap();
        assert!(
            (b.min.x - 10.0).abs() < 1e-6 && (b.max.x - 15.0).abs() < 1e-3,
            "{b:?}"
        );
    }

    #[test]
    fn text_images_masks_and_filters_are_diagnosed_not_dropped_silently() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
          <defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter><mask id="m"><rect width="10" height="10" fill="white"/></mask></defs>
          <text x="1" y="5">hello</text>
          <image x="0" y="0" width="2" height="2" href="data:image/png;base64,iVBORw0KGgo="/>
          <rect id="r" x="2" y="2" width="4" height="4" fill="#000" filter="url(#f)" mask="url(#m)"/>
        </svg>"##;
        let doc = read(svg);
        let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"svg.text"));
        assert!(codes.contains(&"svg.image"));
        assert!(codes.contains(&"svg.mask"));
        assert!(codes.contains(&"svg.filter"));
        assert!(
            doc.diagnostics
                .iter()
                .any(|d| d.code == "svg.text" && d.severity == crate::diag::Severity::Error)
        );
        // The rect itself is still read (unmasked, unfiltered).
        assert!((region_of(&doc.layers[0]).area() - 16.0).abs() < 1e-6);
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(read_bytes("x.svg", b"<svg", &ReadOptions::default()).is_err());
        assert!(read_bytes("x.svg", b"<not-svg/>", &ReadOptions::default()).is_err());
        assert!(read_bytes("x.svg", &[0xff, 0xfe], &ReadOptions::default()).is_err());
    }
}
