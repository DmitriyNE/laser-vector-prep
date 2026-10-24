//! Minimal vector PDF writer: one stroked path per trajectory, exact RGB strokes, finite
//! cosmetic widths, 1:1 physical scale (72 pt per inch), no fills, no raster, no
//! compression, no dates (output is byte-deterministic). Straight segments are `l`; arcs
//! are cubic Béziers (`c`) split so each stays within the curve tolerance of the arc.

use std::fmt::Write as _;

use cavalier_contours::polyline::PlineSource;

use crate::geom::{BBox, Point};

use super::ExportPath;

pub const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Debug, Clone, PartialEq)]
pub enum PdfError {
    NothingToExport,
    NonFiniteCoordinate,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::NothingToExport => write!(f, "no paths to write"),
            PdfError::NonFiniteCoordinate => write!(f, "a path contains a non-finite coordinate"),
        }
    }
}

impl std::error::Error for PdfError {}

/// Page geometry in document millimetres: the page's lower-left corner and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub origin: Point,
    pub width_mm: f64,
    pub height_mm: f64,
}

impl Layout {
    /// Page around the paths' bounds plus `margin_mm` on every side.
    pub fn fit(paths: &[ExportPath], margin_mm: f64) -> Option<Layout> {
        let b = paths
            .iter()
            .filter_map(ExportPath::bounds)
            .reduce(|a, b| a.union(b))?;
        let m = margin_mm.max(0.0);
        Some(Layout {
            origin: Point::new(b.min.x - m, b.min.y - m),
            width_mm: b.width() + 2.0 * m,
            height_mm: b.height() + 2.0 * m,
        })
    }

    pub fn bounds(&self) -> BBox {
        BBox {
            min: self.origin,
            max: Point::new(
                self.origin.x + self.width_mm,
                self.origin.y + self.height_mm,
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PdfOptions {
    pub margin_mm: f64,
    pub title: String,
    /// Free-text metadata (group names and colours); never process settings.
    pub keywords: String,
    /// Force a page layout (for per-group files that must register); `None` fits the paths.
    pub layout: Option<Layout>,
    /// Largest distance between an arc and the Bézier curves written for it (mm).
    pub curve_tolerance_mm: f64,
}

/// Relative radial error of the cubic approximation of a quarter-turn arc
/// (h = 4/3 tan(φ/4)); it scales with the sixth power of the swept angle.
const QUARTER_ARC_REL_ERROR: f64 = 2.8e-4;

/// Cubic Béziers `[c1, c2, end]` for the arc from `a` to `b` with `bulge`, split into equal
/// pieces short enough that each deviates from the circle by at most `tolerance`. The last
/// piece ends exactly at `b`.
pub fn arc_to_cubics(a: Point, b: Point, bulge: f64, tolerance: f64) -> Vec<[Point; 3]> {
    use std::f64::consts::FRAC_PI_2;
    let Some(arc) = crate::arc::Arc::from_bulge(a, b, bulge) else {
        return vec![[a, b, b]];
    };
    let r = arc.radius;
    let ratio = (tolerance.max(1e-9) / (QUARTER_ARC_REL_ERROR * r)).min(1.0);
    let max_piece = FRAC_PI_2 * ratio.powf(1.0 / 6.0);
    let n = (arc.sweep.abs() / max_piece).ceil().max(1.0) as usize;
    let phi = arc.sweep / n as f64;
    let h = 4.0 / 3.0 * (phi / 4.0).tan();
    let c = arc.center;
    let on = |t: f64| Point::new(c.x + r * t.cos(), c.y + r * t.sin());
    (0..n)
        .map(|k| {
            let a0 = arc.start_angle + phi * k as f64;
            let a1 = a0 + phi;
            let p0 = if k == 0 { a } else { on(a0) };
            let p3 = if k + 1 == n { b } else { on(a1) };
            let c1 = Point::new(p0.x - h * r * a0.sin(), p0.y + h * r * a0.cos());
            let c2 = Point::new(p3.x + h * r * a1.sin(), p3.y - h * r * a1.cos());
            [c1, c2, p3]
        })
        .collect()
}

/// Fixed formatting: four decimals (0.1 mil = 2.5 um resolution in points is far below the
/// 1 um requirement), trailing zeros trimmed.
pub fn num(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".into() } else { s }
}

fn pdf_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('(');
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\r' | '\n' => out.push(' '),
            c if c.is_ascii() => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// Write the PDF. Returns the bytes and the layout used.
pub fn write(paths: &[ExportPath], opts: &PdfOptions) -> Result<(Vec<u8>, Layout), PdfError> {
    let drawable: Vec<&ExportPath> = paths
        .iter()
        .filter(|p| p.pline.vertex_count() >= 2)
        .collect();
    if drawable.is_empty() {
        return Err(PdfError::NothingToExport);
    }
    if drawable.iter().any(|p| {
        p.pline
            .iter_vertexes()
            .any(|v| !v.x.is_finite() || !v.y.is_finite() || !v.bulge.is_finite())
            || !p.width_mm.is_finite()
    }) {
        return Err(PdfError::NonFiniteCoordinate);
    }
    let layout = match opts.layout {
        Some(l) => l,
        None => Layout::fit(paths, opts.margin_mm).ok_or(PdfError::NothingToExport)?,
    };

    // Content stream: round joins/caps so polyline corners are continuous hairlines.
    let mut content = String::from("1 J 1 j\n");
    let mut color: Option<(u8, u8, u8)> = None;
    let mut width: Option<f64> = None;
    let to_pt = |p: Point| {
        (
            (p.x - layout.origin.x) * PT_PER_MM,
            (p.y - layout.origin.y) * PT_PER_MM,
        )
    };
    for p in &drawable {
        let c = (p.color.0, p.color.1, p.color.2);
        if color != Some(c) {
            let _ = writeln!(
                content,
                "{} {} {} RG",
                num(c.0 as f64 / 255.0),
                num(c.1 as f64 / 255.0),
                num(c.2 as f64 / 255.0)
            );
            color = Some(c);
        }
        if width != Some(p.width_mm) {
            let _ = writeln!(content, "{} w", num(p.width_mm * PT_PER_MM));
            width = Some(p.width_mm);
        }
        let (start, segments) = p.segments();
        let (x, y) = to_pt(start);
        let _ = writeln!(content, "{} {} m", num(x), num(y));
        let last = segments.len().saturating_sub(1);
        let mut from = start;
        for (i, (bulge, to)) in segments.iter().enumerate() {
            if *bulge == 0.0 {
                // A closed path's final straight segment is drawn by `h`.
                if !(p.closed() && i == last) {
                    let (x, y) = to_pt(*to);
                    let _ = writeln!(content, "{} {} l", num(x), num(y));
                }
            } else {
                for [c1, c2, end] in arc_to_cubics(from, *to, *bulge, opts.curve_tolerance_mm) {
                    let (x1, y1) = to_pt(c1);
                    let (x2, y2) = to_pt(c2);
                    let (x3, y3) = to_pt(end);
                    let _ = writeln!(
                        content,
                        "{} {} {} {} {} {} c",
                        num(x1),
                        num(y1),
                        num(x2),
                        num(y2),
                        num(x3),
                        num(y3)
                    );
                }
            }
            from = *to;
        }
        content.push_str(if p.closed() { "h S\n" } else { "S\n" });
    }

    let page_w = num(layout.width_mm * PT_PER_MM);
    let page_h = num(layout.height_mm * PT_PER_MM);
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w} {page_h}] /Contents 4 0 R /Resources << >> >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        format!(
            "<< /Title {} /Producer {} /Keywords {} >>",
            pdf_string(&opts.title),
            pdf_string(&format!("Laser Vector Prep {}", crate::VERSION)),
            pdf_string(&opts.keywords)
        ),
    ];

    let mut out: Vec<u8> = Vec::with_capacity(content.len() + 1024);
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let mut trailer = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in &offsets {
        let _ = writeln!(trailer, "{off:010} 00000 n ");
    }
    let _ = write!(
        trailer,
        "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    out.extend_from_slice(trailer.as_bytes());
    Ok((out, layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GroupId, Rgb};
    use cavalier_contours::polyline::PlineSourceMut;
    use proptest::prelude::*;

    fn path(pts: &[(f64, f64)], closed: bool, color: Rgb, width: f64) -> ExportPath {
        let mut pline = if closed {
            crate::pline::Pline::new_closed()
        } else {
            crate::pline::Pline::new()
        };
        for (x, y) in pts {
            pline.add(*x, *y, 0.0);
        }
        ExportPath {
            pline,
            color,
            width_mm: width,
            group: GroupId::new("g"),
        }
    }

    /// Content stream operators with their operands, from the bytes.
    fn ops(pdf: &[u8]) -> Vec<Vec<String>> {
        let s = String::from_utf8_lossy(pdf);
        let start = s.find("stream\n").unwrap() + 7;
        let end = s.find("endstream").unwrap();
        s[start..end]
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split_whitespace().map(str::to_string).collect())
            .collect()
    }

    #[test]
    fn writes_strokes_only_with_exact_scale_and_colors() {
        let paths = vec![
            path(
                &[(0.0, 0.0), (10.0, 0.0), (10.0, 5.0)],
                true,
                Rgb::BLUE,
                0.0254,
            ),
            path(&[(20.0, 0.0), (30.0, 2.0)], false, Rgb::RED, 0.0254),
        ];
        let opts = PdfOptions {
            margin_mm: 5.0,
            title: "t (x)".into(),
            keywords: "k".into(),
            layout: None,
            curve_tolerance_mm: 0.001,
        };
        let (bytes, layout) = write(&paths, &opts).unwrap();
        assert_eq!(layout.origin, Point::new(-5.0, -5.0));
        assert!((layout.width_mm - 40.0).abs() < 1e-12 && (layout.height_mm - 15.0).abs() < 1e-12);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.starts_with("%PDF-1.4\n"));
        assert!(s.contains(&format!(
            "/MediaBox [0 0 {} {}]",
            num(40.0 * PT_PER_MM),
            num(15.0 * PT_PER_MM)
        )));
        assert!(s.contains("/Title (t \\(x\\))"));
        let ops = ops(&bytes);
        let count = |op: &str| {
            ops.iter()
                .filter(|o| o.last().map(String::as_str) == Some(op))
                .count()
        };
        assert_eq!(count("m"), 2);
        assert_eq!(count("l"), 3);
        assert_eq!(count("S"), 2, "{ops:?}");
        assert_eq!(
            ops.iter()
                .filter(|o| o.first().map(String::as_str) == Some("h"))
                .count(),
            1
        );
        assert_eq!(count("RG"), 2);
        assert_eq!(count("w"), 1, "width set once for identical widths");
        for forbidden in ["f", "B", "b", "f*", "re", "Do", "BI"] {
            assert_eq!(count(forbidden), 0, "no fills or images: {forbidden}");
        }
        // First move: (0,0) mm with origin (-5,-5) -> 5 mm = 14.1732 pt.
        let m = ops.iter().find(|o| o.last().unwrap() == "m").unwrap();
        assert_eq!(m[0], num(5.0 * PT_PER_MM));
        assert_eq!(m[1], num(5.0 * PT_PER_MM));
        assert_eq!(
            ops.iter().find(|o| o.last().unwrap() == "w").unwrap()[0],
            "0.072"
        );
        assert!(ops.iter().any(|o| o == &["0", "0", "1", "RG"]));
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn xref_offsets_point_at_objects_and_output_is_deterministic() {
        let paths = vec![path(&[(0.0, 0.0), (1.0, 1.0)], false, Rgb::RED, 0.05)];
        let opts = PdfOptions {
            margin_mm: 1.0,
            title: "d".into(),
            keywords: String::new(),
            layout: None,
            curve_tolerance_mm: 0.001,
        };
        let (a, _) = write(&paths, &opts).unwrap();
        let (b, _) = write(&paths, &opts).unwrap();
        assert_eq!(a, b);
        // Offsets are byte offsets (the header has a binary comment), so work on bytes.
        let sx = find(&a, b"startxref\n").unwrap() + 10;
        let xref_at: usize = std::str::from_utf8(&a[sx..])
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(a[xref_at..].starts_with(b"xref\n"));
        let table = std::str::from_utf8(&a[xref_at..]).unwrap();
        for (i, line) in table.lines().skip(3).take(5).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(
                a[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()),
                "object {} offset",
                i + 1
            );
        }
    }

    #[test]
    fn rejects_empty_and_non_finite() {
        let opts = PdfOptions {
            margin_mm: 1.0,
            title: String::new(),
            keywords: String::new(),
            layout: None,
            curve_tolerance_mm: 0.001,
        };
        assert_eq!(write(&[], &opts).unwrap_err(), PdfError::NothingToExport);
        assert_eq!(
            write(&[path(&[(0.0, 0.0)], false, Rgb::RED, 0.1)], &opts).unwrap_err(),
            PdfError::NothingToExport
        );
        assert_eq!(
            write(
                &[path(&[(0.0, 0.0), (f64::NAN, 1.0)], false, Rgb::RED, 0.1)],
                &opts
            )
            .unwrap_err(),
            PdfError::NonFiniteCoordinate
        );
    }

    #[test]
    fn forced_layout_registers_separate_files() {
        let a = vec![path(&[(0.0, 0.0), (10.0, 0.0)], false, Rgb::BLUE, 0.0254)];
        let b = vec![path(&[(50.0, 50.0), (60.0, 55.0)], false, Rgb::RED, 0.0254)];
        let all: Vec<ExportPath> = a.iter().chain(b.iter()).cloned().collect();
        let layout = Layout::fit(&all, 2.0).unwrap();
        let mk = |paths: &[ExportPath]| {
            write(
                paths,
                &PdfOptions {
                    margin_mm: 2.0,
                    title: String::new(),
                    keywords: String::new(),
                    layout: Some(layout),
                    curve_tolerance_mm: 0.001,
                },
            )
            .unwrap()
        };
        let (pa, la) = mk(&a);
        let (pb, lb) = mk(&b);
        assert_eq!(la, lb);
        let media = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .split("/MediaBox ")
                .nth(1)
                .unwrap()
                .split(']')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(media(&pa), media(&pb));
    }

    proptest! {
        #[test]
        fn coordinates_roundtrip_within_a_hundredth_of_a_micron(
            pts in prop::collection::vec((-500.0..500.0f64, -500.0..500.0f64), 2..30),
            margin in 0.0..20.0f64,
        ) {
            let p = path(&pts, false, Rgb::BLUE, 0.0254);
            let opts = PdfOptions { margin_mm: margin, title: String::new(), keywords: String::new(), layout: None, curve_tolerance_mm: 0.001 };
            let (bytes, layout) = write(std::slice::from_ref(&p), &opts).unwrap();
            let ops = ops(&bytes);
            let coords: Vec<(f64, f64)> = ops
                .iter()
                .filter(|o| matches!(o.last().map(String::as_str), Some("m" | "l")))
                .map(|o| (o[0].parse().unwrap(), o[1].parse().unwrap()))
                .collect();
            prop_assert_eq!(coords.len(), pts.len());
            for ((x, y), (px, py)) in coords.iter().zip(&pts) {
                let ex = (px - layout.origin.x) * PT_PER_MM;
                let ey = (py - layout.origin.y) * PT_PER_MM;
                // 4 decimals in points: 5e-5 pt = 0.018 um.
                prop_assert!((x - ex).abs() <= 5.1e-5 && (y - ey).abs() <= 5.1e-5);
            }
        }
    }

    #[test]
    fn arcs_become_beziers_within_tolerance() {
        use crate::pline::circle;
        // Every Bézier piece stays within the tolerance of its circle, for small and large
        // radii; a 1 mm circle at 1 um needs four pieces, a 100 mm circle more.
        for (r, tol) in [(1.0, 0.001), (100.0, 0.001), (10.0, 0.0001), (0.05, 0.001)] {
            let c = circle(Point::new(3.0, -2.0), r);
            let a = Point::new(3.0 - r, -2.0);
            let b = Point::new(3.0 + r, -2.0);
            let pieces = arc_to_cubics(a, b, c.at(0).bulge, tol);
            let mut from = a;
            let mut worst: f64 = 0.0;
            for [c1, c2, end] in &pieces {
                for i in 0..=32 {
                    let t = i as f64 / 32.0;
                    let u = 1.0 - t;
                    let x = u * u * u * from.x
                        + 3.0 * u * u * t * c1.x
                        + 3.0 * u * t * t * c2.x
                        + t * t * t * end.x;
                    let y = u * u * u * from.y
                        + 3.0 * u * u * t * c1.y
                        + 3.0 * u * t * t * c2.y
                        + t * t * t * end.y;
                    worst = worst.max(((x - 3.0).hypot(y + 2.0) - r).abs());
                }
                from = *end;
            }
            assert_eq!(from, b, "last piece ends exactly at the arc end");
            assert!(
                worst <= tol,
                "r {r} tol {tol}: worst {worst} with {} pieces",
                pieces.len()
            );
        }
        assert_eq!(
            arc_to_cubics(Point::new(-1.0, 0.0), Point::new(1.0, 0.0), 1.0, 0.001).len(),
            2
        );
    }

    #[test]
    fn circles_are_written_as_curves_not_segments() {
        let ring = ExportPath {
            pline: crate::pline::circle(Point::new(5.0, 5.0), 2.0),
            color: Rgb::BLUE,
            width_mm: 0.0254,
            group: GroupId::new("g"),
        };
        let opts = PdfOptions {
            margin_mm: 1.0,
            title: String::new(),
            keywords: String::new(),
            layout: None,
            curve_tolerance_mm: 0.001,
        };
        let (bytes, layout) = write(&[ring], &opts).unwrap();
        // The page fits the circle exactly: arc bounds, not vertex bounds.
        assert!((layout.width_mm - 6.0).abs() < 1e-12 && (layout.height_mm - 6.0).abs() < 1e-12);
        let ops = ops(&bytes);
        let count = |op: &str| {
            ops.iter()
                .filter(|o| o.last().map(String::as_str) == Some(op))
                .count()
        };
        assert_eq!(count("l"), 0);
        assert!(count("c") >= 4 && count("c") <= 8, "{}", count("c"));
        assert_eq!(count("m"), 1);
    }
}
