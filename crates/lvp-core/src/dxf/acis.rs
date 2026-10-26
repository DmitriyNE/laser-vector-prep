//! Planar REGION entities from the ACIS data a DXF file embeds (SAT text in group codes 1
//! and 3, DXF R13 to R2010): every face becomes a filled area bounded by its loops, with
//! straight edges as lines, circular ones as exact arcs and elliptic ones flattened.
//!
//! The text is lightly enciphered: every printable character `c` is stored as `159 - c`.
//! Records end with `#`; a field `$n` refers to record `n`. The position of a field within
//! a record shifts between ACIS versions (7.0 added a history reference and an integer
//! after the attribute), so references are found by the type of record they point to,
//! and a sense flag is the token after the reference it qualifies (`0`/`1` before 7.0,
//! `forward`/`reversed` after).

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};

use crate::geom::Point;
use crate::pline::{self, Pline};

/// Undo the DXF cipher of ACIS text.
pub fn decipher(line: &str) -> String {
    line.chars()
        .map(|c| match c as u32 {
            33..=126 => char::from_u32(159 - c as u32).unwrap_or(c),
            _ => c,
        })
        .collect()
}

struct Record {
    name: String,
    tokens: Vec<String>,
}

impl Record {
    /// Indices of the records this one refers to, with their token positions.
    fn refs(&self) -> impl Iterator<Item = (usize, i64)> + '_ {
        self.tokens
            .iter()
            .enumerate()
            .filter_map(|(k, t)| Some((k, t.strip_prefix('$')?.parse().ok()?)))
    }

    fn numbers(&self) -> Vec<f64> {
        self.tokens
            .iter()
            .filter(|t| !t.starts_with('$'))
            .filter_map(|t| t.parse().ok())
            .collect()
    }
}

struct Sat {
    version: u32,
    records: Vec<Record>,
}

impl Sat {
    fn parse(text: &str) -> Result<Sat, String> {
        let mut lines = text.lines();
        let header = lines.next().ok_or("no ACIS header")?;
        let version: u32 = header
            .split_whitespace()
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or("no ACIS version")?;
        let mut body = String::new();
        let mut in_header = version >= 400;
        for line in lines {
            // From 4.0 on, a product line (`@n` counted strings) and a units line follow.
            if in_header {
                let t = line.trim_start();
                if t.starts_with('@') || t.split_whitespace().all(|w| w.parse::<f64>().is_ok()) {
                    continue;
                }
                in_header = false;
            }
            body.push_str(line);
            body.push(' ');
        }
        let mut records = Vec::new();
        for chunk in body.split('#') {
            let mut tokens: Vec<String> = chunk.split_whitespace().map(String::from).collect();
            // An optional record number `-n` comes first.
            if tokens.len() > 1
                && tokens[0].starts_with('-')
                && tokens[0][1..].bytes().all(|b| b.is_ascii_digit())
            {
                tokens.remove(0);
            }
            let Some(name) = tokens.first().cloned() else {
                continue;
            };
            if name.starts_with("End-of") {
                break;
            }
            tokens.remove(0);
            records.push(Record { name, tokens });
        }
        Ok(Sat { version, records })
    }

    fn get(&self, i: i64) -> Option<&Record> {
        usize::try_from(i).ok().and_then(|i| self.records.get(i))
    }

    /// The first reference of `r` to a record whose name satisfies `kind`, with its token
    /// position.
    fn find(&self, r: &Record, kind: impl Fn(&str) -> bool) -> Option<(usize, i64)> {
        r.refs()
            .find(|&(_, i)| self.get(i).is_some_and(|t| kind(&t.name)))
    }

    /// Whether the token after position `k` of `r` marks the reversed sense.
    fn reversed_after(r: &Record, k: usize) -> bool {
        matches!(
            r.tokens.get(k + 1).map(String::as_str),
            Some("1" | "reversed")
        )
    }

    /// The numbers of a geometry record, without the integer 7.0 put after the attribute.
    fn geometry(&self, r: &Record) -> Vec<f64> {
        let mut n = r.numbers();
        if self.version >= 700 && !n.is_empty() {
            n.remove(0);
        }
        n
    }

    fn point_of_vertex(&self, v: i64) -> Option<Point> {
        let vertex = self.get(v)?;
        let (_, p) = self.find(vertex, |n| n == "point")?;
        let n = self.get(p)?.numbers();
        let k = n.len().checked_sub(3)?;
        Some(Point::new(n[k], n[k + 1]))
    }
}

/// Why a region could not be read: shown in the diagnostic.
pub type Failure = String;

/// The faces of one REGION's ACIS data (deciphered SAT text), each as its loops, in the
/// coordinates of the entity. Elliptic edges are flattened within `tolerance`.
pub fn region_faces(sat: &str, tolerance: f64) -> Result<Vec<Vec<Pline>>, Failure> {
    let sat = Sat::parse(sat)?;
    let transform = sat
        .records
        .iter()
        .find(|r| r.name == "body")
        .and_then(|b| sat.find(b, |n| n == "transform"))
        .and_then(|(_, t)| sat.get(t))
        .map(|t| sat.geometry(t));
    let mut faces = Vec::new();
    for face in sat.records.iter().filter(|r| r.name == "face") {
        if let Some((_, s)) = sat.find(face, |n| n.ends_with("-surface"))
            && let Some(surface) = sat.get(s)
        {
            if surface.name != "plane-surface" {
                return Err(format!("a face lies on a {}", surface.name));
            }
            let n = sat.geometry(surface);
            if n.len() >= 6 && n[5].abs() < 1.0 - 1e-9 {
                return Err("a face is not parallel to the XY plane".into());
            }
        }
        let mut loops = Vec::new();
        let mut next = sat.find(face, |n| n == "loop").map(|(_, l)| l);
        let mut guard = 0;
        while let Some(l) = next {
            guard += 1;
            if guard > sat.records.len() {
                return Err("the loops of a face run in a circle".into());
            }
            let lp = sat.get(l).ok_or("a loop is missing")?;
            loops.push(loop_pline(&sat, lp, tolerance)?);
            next = sat.find(lp, |n| n == "loop").map(|(_, n)| n);
        }
        if !loops.is_empty() {
            faces.push(loops);
        }
    }
    if let Some(m) = transform {
        apply(&mut faces, &m)?;
    }
    Ok(faces)
}

/// One loop of coedges as a closed polyline.
fn loop_pline(sat: &Sat, lp: &Record, tolerance: f64) -> Result<Pline, Failure> {
    let (_, first) = sat
        .find(lp, |n| n == "coedge")
        .ok_or("a loop has no edges")?;
    let mut p = Pline::new();
    let mut c = first;
    for _ in 0..=sat.records.len() {
        let coedge = sat.get(c).ok_or("a coedge is missing")?;
        let (k, e) = sat
            .find(coedge, |n| n == "edge")
            .ok_or("a coedge has no edge")?;
        let coedge_reversed = Sat::reversed_after(coedge, k);
        let edge = sat.get(e).ok_or("an edge is missing")?;
        let vertices: Vec<i64> = edge
            .refs()
            .filter(|&(_, i)| sat.get(i).is_some_and(|r| r.name == "vertex"))
            .map(|(_, i)| i)
            .collect();
        let [start, end] = vertices[..] else {
            return Err("an edge without two vertices".into());
        };
        let (from, to) = if coedge_reversed {
            (end, start)
        } else {
            (start, end)
        };
        let a = sat.point_of_vertex(from).ok_or("a vertex has no point")?;
        let b = sat.point_of_vertex(to).ok_or("a vertex has no point")?;
        let (kc, cu) = sat
            .find(edge, |n| n.ends_with("-curve"))
            .ok_or("an edge has no curve")?;
        let curve = sat.get(cu).ok_or("a curve is missing")?;
        if p.vertex_count() == 0 {
            p.add(a.x, a.y, 0.0);
        }
        match curve.name.as_str() {
            "straight-curve" => p.add(b.x, b.y, 0.0),
            "ellipse-curve" => {
                let g = sat.geometry(curve);
                if g.len() < 10 {
                    return Err("an ellipse curve is incomplete".into());
                }
                let centre = Point::new(g[0], g[1]);
                let normal_up = g[5] > 0.0;
                let major = Point::new(g[6], g[7]);
                let ratio = g[9];
                // The curve runs counter-clockwise about its normal; the edge and the
                // coedge may each reverse that.
                let along = !(Sat::reversed_after(edge, kc) ^ coedge_reversed);
                let ccw = along == normal_up;
                if (ratio - 1.0).abs() < 1e-9 {
                    pline::add_center_arc(&mut p, a, b, centre, ccw);
                } else {
                    ellipse(
                        &mut p, centre, major, ratio, normal_up, a, b, ccw, tolerance,
                    );
                }
            }
            other => return Err(format!("an edge follows a {other}")),
        }
        let (_, next) = sat
            .find(coedge, |n| n == "coedge")
            .ok_or("a loop is not closed")?;
        if next == first {
            let mut closed = Pline::new_closed();
            let n = p.vertex_count();
            for i in 0..n.saturating_sub(1) {
                let v = p.at(i);
                closed.add(v.x, v.y, v.bulge);
            }
            return pline::sanitize(&closed).ok_or_else(|| "a loop is degenerate".into());
        }
        c = next;
    }
    Err("a loop does not come back to its first edge".into())
}

/// Flatten the elliptic arc from `a` to `b` round `centre` (major axis `major`, minor/major
/// `ratio`, normal up or down) in the given turning direction, appending to `p`.
#[allow(clippy::too_many_arguments)]
fn ellipse(
    p: &mut Pline,
    centre: Point,
    major: Point,
    ratio: f64,
    normal_up: bool,
    a: Point,
    b: Point,
    ccw: bool,
    tolerance: f64,
) {
    // Minor axis: the normal turned onto the major axis, scaled.
    let s = if normal_up { 1.0 } else { -1.0 };
    let minor = Point::new(-major.y * ratio * s, major.x * ratio * s);
    let at = |t: f64| {
        Point::new(
            centre.x + major.x * t.cos() + minor.x * t.sin(),
            centre.y + major.y * t.cos() + minor.y * t.sin(),
        )
    };
    let param = |q: Point| {
        let (dx, dy) = (q.x - centre.x, q.y - centre.y);
        let (mm, nn) = (
            major.x * major.x + major.y * major.y,
            minor.x * minor.x + minor.y * minor.y,
        );
        ((dx * minor.x + dy * minor.y) / nn).atan2((dx * major.x + dy * major.y) / mm)
    };
    // Increasing parameter turns counter-clockwise about the normal.
    let forward = ccw == normal_up;
    let (t0, mut t1) = (param(a), param(b));
    let tau = std::f64::consts::TAU;
    if forward {
        while t1 <= t0 + 1e-12 {
            t1 += tau;
        }
    } else {
        while t1 >= t0 - 1e-12 {
            t1 -= tau;
        }
    }
    let mut pts = Vec::new();
    super::read::flatten_curve(&at, t0, t1, tolerance, &mut pts);
    for q in pts.into_iter().skip(1) {
        p.add(q.x, q.y, 0.0);
    }
}

/// Apply a body transform (a 3×3 matrix by rows, a translation and a scale) to the faces.
/// Only a rotation and translation in the plane keep arcs exact; anything else is refused.
fn apply(faces: &mut [Vec<Pline>], m: &[f64]) -> Result<(), Failure> {
    if m.len() < 13 {
        return Err("a body transform is incomplete".into());
    }
    let scale = m[12];
    let (a, b, d, e) = (m[0] * scale, m[1] * scale, m[3] * scale, m[4] * scale);
    let (tx, ty) = (m[9], m[10]);
    let det = a * e - b * d;
    if (det.abs() - scale * scale).abs() > 1e-9 * scale * scale
        || m[2].abs() > 1e-12
        || m[5].abs() > 1e-12
    {
        return Err("a body transform leaves the XY plane or skews".into());
    }
    let mirror = det < 0.0;
    for p in faces.iter_mut().flatten() {
        let mut q = Pline::new_closed();
        for i in 0..p.vertex_count() {
            let v = p.at(i);
            // Row vectors: (x, y, z) times the matrix.
            q.add(
                v.x * a + v.y * d + tx,
                v.x * b + v.y * e + ty,
                if mirror { -v.bulge } else { v.bulge },
            );
        }
        *p = q;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encipher text as DXF stores it.
    fn encipher(s: &str) -> String {
        decipher(s)
    }

    /// An ACIS 1.6 region as AutoCAD R14 writes it: a 0.1 x 0.254 oval pad, two straight
    /// sides and two half circles (taken from a real REGION, renumbered).
    const OVAL: &str = "106 26 1 0
body $-1 $25 $-1 $-1 #
point $-1 0.44914566 -9.0338656 0 #
vertex $-1 $19 $1 #
point $-1 0.44914566 -9.2878656 0 #
vertex $-1 $13 $3 #
point $-1 0.55074566 -9.2878656 0 #
vertex $-1 $16 $5 #
point $-1 0.55074566 -9.0338656 0 #
vertex $-1 $19 $7 #
ellipse-curve $-1 0.44914566 -9.1608656 0 0 0 1 0 0.127 0 1 0 0 #
edge $-1 $2 $4 $11 $9 0 #
coedge $-1 $14 $20 $-1 $10 0 $21 $-1 #
straight-curve $-1 0.44914566 -9.2878656 0 0.1016 0 0 0 0 #
edge $-1 $4 $6 $14 $12 0 #
coedge $-1 $17 $11 $-1 $13 0 $21 $-1 #
ellipse-curve $-1 0.55074566 -9.1608656 0 0 0 1 0 -0.127 0 1 0 0 #
edge $-1 $6 $8 $17 $15 0 #
coedge $-1 $20 $14 $-1 $16 0 $21 $-1 #
straight-curve $-1 0.55074566 -9.0338656 0 -0.1016 0 0 0 0 #
edge $-1 $8 $2 $20 $18 0 #
coedge $-1 $11 $17 $-1 $19 0 $21 $-1 #
loop $-1 $-1 $11 $23 #
plane-surface $-1 0.49994566 -9.2243656 0 0 0 1 1 0 0 0 0 0 0 0 #
face $-1 $-1 $21 $24 $-1 $22 0 1 0 #
shell $-1 $-1 $-1 $23 $25 #
lump $-1 $-1 $24 $0 #
End-of-ACIS-data
";

    #[test]
    fn deciphering_is_its_own_inverse() {
        assert_eq!(decipher("noi mi n o"), "106 26 1 0");
        assert_eq!(decipher(&encipher(OVAL)), OVAL);
    }

    #[test]
    fn an_oval_pad_reads_as_one_loop_of_lines_and_half_circles() {
        let faces = region_faces(OVAL, 1e-4).unwrap();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].len(), 1);
        let lp = &faces[0][0];
        assert!(lp.is_closed());
        // 0.1016 x 0.254 between the arc centres' tangent points: a stadium.
        let (w, r) = (0.1016, 0.127);
        let expected = w * 2.0 * r + std::f64::consts::PI * r * r;
        assert!((lp.area().abs() - expected).abs() < 1e-9, "{}", lp.area());
        assert_eq!(lp.vertex_count(), 4);
        assert_eq!(
            lp.iter_vertexes().filter(|v| v.bulge.abs() > 0.99).count(),
            2
        );
    }

    #[test]
    fn a_square_with_a_round_hole_and_a_7_0_record_layout() {
        // ACIS 7.0: a history reference and an integer after every attribute, counted
        // strings, `forward`/`reversed`, a product line and a units line in the header.
        // A 10 mm square with a hole of radius 2 at (5, 5): one face, two loops.
        let sat = "700 0 1 0
@33 Open Design Alliance ACIS Builder @12 ACIS 7.0 NT @24 Mon Oct 26 12:00:00 2026
1 9.9999999999999995e-07 1e-10
body $-1 -1 $-1 $1 $-1 $-1 #
lump $-1 -1 $-1 $-1 $2 $0 #
shell $-1 -1 $-1 $-1 $-1 $3 $-1 $1 #
face $-1 -1 $-1 $-1 $4 $2 $-1 $5 forward single #
loop $-1 -1 $-1 $6 $7 $3 #
plane-surface $-1 -1 $-1 0 0 0 0 0 1 1 0 0 forward_v I I I I #
loop $-1 -1 $-1 $-1 $8 $3 #
coedge $-1 -1 $-1 $9 $10 $-1 $11 forward $4 $-1 #
coedge $-1 -1 $-1 $8 $8 $-1 $12 reversed $6 $-1 #
coedge $-1 -1 $-1 $13 $7 $-1 $14 forward $4 $-1 #
coedge $-1 -1 $-1 $7 $13 $-1 $15 forward $4 $-1 #
edge $-1 -1 $-1 $16 0 $17 10 $7 $18 forward @7 unknown #
edge $-1 -1 $-1 $19 0 $19 6.283185307179586 $8 $20 forward @7 unknown #
coedge $-1 -1 $-1 $10 $9 $-1 $21 forward $4 $-1 #
edge $-1 -1 $-1 $17 0 $22 10 $9 $23 forward @7 unknown #
edge $-1 -1 $-1 $24 0 $16 10 $10 $25 forward @7 unknown #
vertex $-1 -1 $-1 $11 $26 #
vertex $-1 -1 $-1 $14 $27 #
straight-curve $-1 -1 $-1 0 0 0 1 0 0 I I #
vertex $-1 -1 $-1 $12 $28 #
ellipse-curve $-1 -1 $-1 5 5 0 0 0 1 2 0 0 1 I I #
edge $-1 -1 $-1 $22 0 $24 10 $13 $29 forward @7 unknown #
vertex $-1 -1 $-1 $21 $30 #
straight-curve $-1 -1 $-1 10 0 0 0 1 0 I I #
vertex $-1 -1 $-1 $15 $31 #
straight-curve $-1 -1 $-1 0 10 0 0 -1 0 I I #
point $-1 -1 $-1 0 0 0 #
point $-1 -1 $-1 10 0 0 #
point $-1 -1 $-1 7 5 0 #
straight-curve $-1 -1 $-1 10 10 0 -1 0 0 I I #
point $-1 -1 $-1 10 10 0 #
point $-1 -1 $-1 0 10 0 #
End-of-ACIS-data
";
        let faces = region_faces(sat, 1e-4).unwrap();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].len(), 2);
        let areas: Vec<f64> = faces[0].iter().map(|l| l.area().abs()).collect();
        assert!(areas.iter().any(|a| (a - 100.0).abs() < 1e-9), "{areas:?}");
        assert!(
            areas
                .iter()
                .any(|a| (a - std::f64::consts::PI * 4.0).abs() < 1e-9),
            "{areas:?}"
        );
    }

    #[test]
    fn faces_off_the_plane_and_unknown_curves_are_refused() {
        let tilted = OVAL.replace(
            "plane-surface $-1 0.49994566 -9.2243656 0 0 0 1",
            "plane-surface $-1 0.49994566 -9.2243656 0 0 1 0",
        );
        assert!(region_faces(&tilted, 1e-4).is_err());
        let spline = OVAL.replace(
            "straight-curve $-1 0.44914566",
            "intcurve-curve $-1 0.44914566",
        );
        assert!(
            region_faces(&spline, 1e-4)
                .unwrap_err()
                .contains("intcurve")
        );
    }
}
