//! SVG export: one `<g>` per exporting output group (an Inkscape layer with the group's
//! name), stroked paths only, millimetre units with a 1:1 viewBox, round caps and joins.
//! Arcs are written as exact SVG arc commands.
//! The page is the paths' bounds plus the export margin; y is flipped from the model's
//! y-up frame to SVG's y-down frame. Reading the file back with the app's SVG reader gives
//! one layer per group.

use std::fmt::Write;

use super::ExportPath;
use crate::geom::BBox;
use crate::model::{GroupId, Rgb};

pub struct SvgOptions<'a> {
    pub margin_mm: f64,
    pub title: &'a str,
    /// Group id → display name, in output order.
    pub groups: &'a [(GroupId, String)],
}

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn rgb(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the paths; `None` when there is nothing to draw.
pub fn write(paths: &[ExportPath], opts: &SvgOptions<'_>) -> Option<String> {
    let bounds = paths
        .iter()
        .filter_map(ExportPath::bounds)
        .reduce(|a, b| a.union(b))?;
    let m = opts.margin_mm.max(0.0);
    let page = BBox {
        min: crate::geom::Point::new(bounds.min.x - m, bounds.min.y - m),
        max: crate::geom::Point::new(bounds.max.x + m, bounds.max.y + m),
    };
    let (w, h) = (page.width(), page.height());
    let mut out = String::new();
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" width="{}mm" height="{}mm" viewBox="0 0 {} {}" version="1.1">"#,
        num(w),
        num(h),
        num(w),
        num(h)
    );
    let _ = writeln!(out, "  <title>{}</title>", xml_escape(opts.title));
    let _ = writeln!(
        out,
        "  <desc>Laser Vector Prep export: 1:1 millimetres, strokes are importer hints, not process parameters. Path order within a group is the planned marking order.</desc>"
    );
    // Model (x, y) with y up → SVG (x - page.min.x, page.max.y - y).
    let tx = |x: f64| num(x - page.min.x);
    let ty = |y: f64| num(page.max.y - y);
    for (id, name) in opts.groups {
        let group_paths: Vec<&ExportPath> = paths.iter().filter(|p| &p.group == id).collect();
        let Some(first) = group_paths.first() else {
            continue;
        };
        let _ = writeln!(
            out,
            r#"  <g id="{}" inkscape:groupmode="layer" inkscape:label="{}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round">"#,
            xml_escape(&id.to_string()),
            xml_escape(name),
            rgb(first.color),
            num(first.width_mm)
        );
        for p in group_paths {
            let (start, segments) = p.segments();
            if segments.is_empty() {
                continue;
            }
            let mut d = format!("M{} {}", tx(start.x), ty(start.y));
            let last = segments.len() - 1;
            let mut from = start;
            for (i, (bulge, to)) in segments.iter().enumerate() {
                if *bulge == 0.0 {
                    if !(p.closed() && i == last) {
                        let _ = write!(d, " L{} {}", tx(to.x), ty(to.y));
                    }
                } else {
                    // |bulge| <= 1 (at most a half turn), so the large-arc flag is 0. The
                    // y flip turns counter-clockwise into SVG's negative-angle direction.
                    let chord = (to.x - from.x).hypot(to.y - from.y);
                    let r = chord * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
                    let sweep = if *bulge > 0.0 { 0 } else { 1 };
                    let _ = write!(
                        d,
                        " A{} {} 0 0 {} {} {}",
                        num(r),
                        num(r),
                        sweep,
                        tx(to.x),
                        ty(to.y)
                    );
                }
                from = *to;
            }
            if p.closed() {
                d.push_str(" Z");
            }
            let _ = writeln!(out, r#"    <path d="{d}"/>"#);
        }
        let _ = writeln!(out, "  </g>");
    }
    let _ = writeln!(out, "</svg>");
    Some(out)
}
