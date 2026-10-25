//! Gerber RS-274X / X2 reader (our own, see D22).
//!
//! A Gerber file is one image: a sequence of dark and clear operations. The reader turns it
//! into one source layer named after the file. When the file never switches to clear
//! polarity the operations become primitives directly (flashes and regions as filled loops,
//! draws as stroked centrelines) and the evaluator unions them; when it does, operations are
//! composited here in source order and the layer gets the resulting region. Everything the
//! format can express about *what* a layer is (`TF.FileFunction`) is kept as the layer's
//! function for profile binding.

pub mod macros;

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use cavalier_contours::polyline::{PlineSource, PlineSourceMut};

use crate::diag::Diagnostic;
use crate::geom::Point;
use crate::model::SourceFormat;
use crate::pline::{self, Pline};
use crate::region::{self, LoopRule, Region};
use crate::source::{Budget, Primitive, ReadError, ReadOptions, SourceDocument, SourceLayer};
use crate::units::Unit;

use macros::MacroDef;

// ---------------------------------------------------------------------------------------
// Lexing

#[derive(Clone, Debug, PartialEq)]
enum Cmd {
    /// A plain `…*` word, whitespace removed (comments keep their text).
    Word(String),
    /// `%…%` block, split into its `*`-terminated statements.
    Extended(Vec<String>),
}

fn lex(text: &str) -> Result<Vec<Cmd>, String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '%' {
            chars.next();
            let start = i + 1;
            let mut end = None;
            for (j, d) in chars.by_ref() {
                if d == '%' {
                    end = Some(j);
                    break;
                }
            }
            let Some(end) = end else {
                return Err("unterminated % block".into());
            };
            let body = &text[start..end];
            let statements: Vec<String> = body
                .split('*')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            out.push(Cmd::Extended(statements));
            continue;
        }
        let start = i;
        let mut end = None;
        for (j, d) in chars.by_ref() {
            if d == '*' {
                end = Some(j);
                break;
            }
        }
        let Some(end) = end else {
            let tail = text[start..].trim();
            if tail.is_empty() {
                break;
            }
            return Err(format!(
                "unterminated word {:?}",
                tail.chars().take(20).collect::<String>()
            ));
        };
        let raw = &text[start..end];
        let word = if is_comment(raw) {
            raw.trim().to_string()
        } else {
            raw.chars().filter(|c| !c.is_whitespace()).collect()
        };
        if !word.is_empty() {
            out.push(Cmd::Word(word));
        }
    }
    Ok(out)
}

fn is_comment(word: &str) -> bool {
    let w = word.trim_start();
    if let Some(rest) = w.strip_prefix("G04") {
        return !rest.starts_with(|c: char| c.is_ascii_digit());
    }
    if let Some(rest) = w.strip_prefix("G4") {
        return !rest.starts_with(|c: char| c.is_ascii_digit());
    }
    false
}

// ---------------------------------------------------------------------------------------
// Coordinate format

#[derive(Clone, Copy, Debug)]
struct Format {
    x_dec: u32,
    y_dec: u32,
    x_int: u32,
    y_int: u32,
    /// `FSL…` (default): leading zeros may be omitted. `FST…`: trailing zeros omitted.
    trailing_omitted: bool,
    incremental: bool,
}

impl Format {
    fn parse(spec: &str) -> Result<Format, String> {
        // e.g. "LAX24Y24", "TIX34Y34", also tolerate "AX24Y24" / "LX24Y24".
        let mut s = spec;
        let mut trailing = false;
        let mut incremental = false;
        if let Some(r) = s.strip_prefix('L') {
            s = r;
        } else if let Some(r) = s.strip_prefix('T') {
            trailing = true;
            s = r;
        }
        if let Some(r) = s.strip_prefix('A') {
            s = r;
        } else if let Some(r) = s.strip_prefix('I') {
            incremental = true;
            s = r;
        }
        let digits = |c: char, s: &str| -> Result<(u32, u32), String> {
            let pos = s
                .find(c)
                .ok_or_else(|| format!("FS without {c} digits: {spec:?}"))?;
            let d: Vec<char> = s[pos + 1..].chars().take(2).collect();
            if d.len() != 2 || !d.iter().all(|c| c.is_ascii_digit()) {
                return Err(format!("bad FS digits for {c}: {spec:?}"));
            }
            Ok((d[0].to_digit(10).unwrap(), d[1].to_digit(10).unwrap()))
        };
        let (x_int, x_dec) = digits('X', s)?;
        let (y_int, y_dec) = digits('Y', s)?;
        if x_dec > 9 || y_dec > 9 {
            return Err(format!("FS decimals out of range: {spec:?}"));
        }
        Ok(Format {
            x_dec,
            y_dec,
            x_int,
            y_int,
            trailing_omitted: trailing,
            incremental,
        })
    }

    /// Parse one coordinate word value (digits with optional sign; a decimal point is
    /// accepted for files that ignore the format).
    fn value(&self, text: &str, axis_dec: u32, axis_int: u32) -> Result<f64, String> {
        let (neg, digits) = match text.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if digits.is_empty() {
            return Err(format!("empty coordinate {text:?}"));
        }
        let v = if digits.contains('.') {
            digits
                .parse::<f64>()
                .map_err(|_| format!("bad coordinate {text:?}"))?
        } else {
            if !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("bad coordinate {text:?}"));
            }
            let padded: String = if self.trailing_omitted {
                let total = (axis_int + axis_dec) as usize;
                let mut d = digits.to_string();
                while d.len() < total {
                    d.push('0');
                }
                d
            } else {
                digits.to_string()
            };
            let n: i128 = padded
                .parse()
                .map_err(|_| format!("coordinate {text:?} too long"))?;
            n as f64 / 10f64.powi(axis_dec as i32)
        };
        Ok(if neg { -v } else { v })
    }

    fn x(&self, t: &str) -> Result<f64, String> {
        self.value(t, self.x_dec, self.x_int)
    }

    fn y(&self, t: &str) -> Result<f64, String> {
        self.value(t, self.y_dec, self.y_int)
    }
}

// ---------------------------------------------------------------------------------------
// Image model

/// One drawing operation in millimetres, before polarity compositing.
#[derive(Clone, Debug)]
struct Item {
    dark: bool,
    shape: Shape,
}

#[derive(Clone, Debug)]
enum Shape {
    /// A resolved filled area of a flash or region: outer loops and the holes inside them.
    /// Kept apart so that re-reading with the even-odd rule reproduces the area exactly.
    Fill {
        outers: Vec<Pline>,
        holes: Vec<Pline>,
    },
    /// A draw with a circular aperture: centreline plus width.
    Stroke { pline: Pline, width: f64 },
}

impl Shape {
    fn fill_of(region: &Region, f: impl Fn(&Pline) -> Pline) -> Shape {
        Shape::Fill {
            outers: region.outers().map(&f).collect(),
            holes: region.holes().map(&f).collect(),
        }
    }
}

#[derive(Clone, Debug)]
enum ApertureShape {
    /// Resolved geometry in mm around the aperture origin; `circle_diameter` is set for a
    /// plain round aperture without hole, the only kind that draws exact strokes.
    Fixed {
        region: Region,
        circle_diameter: Option<f64>,
        extent: f64,
    },
    /// Aperture block (`%AB`): operations replayed at each flash.
    Block(Vec<Item>),
}

#[derive(Clone, Debug)]
struct Aperture {
    shape: ApertureShape,
}

/// Mirror / rotation / scale applied to flashed apertures (`LM`, `LR`, `LS`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ApertureTransform {
    mirror_x: bool,
    mirror_y: bool,
    rotation_deg: f64,
    scale: f64,
}

impl Default for ApertureTransform {
    fn default() -> Self {
        Self {
            mirror_x: false,
            mirror_y: false,
            rotation_deg: 0.0,
            scale: 1.0,
        }
    }
}

impl ApertureTransform {
    fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

fn map_pline(p: &Pline, f: impl Fn(Point) -> Point, flip: bool) -> Pline {
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    for v in p.iter_vertexes() {
        let q = f(Point::new(v.x, v.y));
        out.add(q.x, q.y, if flip { -v.bulge } else { v.bulge });
    }
    if flip {
        // Mirroring reverses orientation; keep outers CCW by reversing vertex order.
        out = reverse_pline(&out);
    }
    out
}

/// Reverse a polyline's direction, keeping arcs on the same side.
fn reverse_pline(p: &Pline) -> Pline {
    let n = p.vertex_count();
    let mut out = if p.is_closed() {
        Pline::new_closed()
    } else {
        Pline::new()
    };
    if p.is_closed() {
        for i in (0..n).rev() {
            let v = p.at(i);
            let prev = p.at((i + n - 1) % n);
            out.add(v.x, v.y, -prev.bulge);
        }
    } else {
        for i in (0..n).rev() {
            let v = p.at(i);
            let b = if i == 0 { 0.0 } else { -p.at(i - 1).bulge };
            out.add(v.x, v.y, b);
        }
    }
    out
}

fn translate_pline(p: &Pline, dx: f64, dy: f64) -> Pline {
    map_pline(p, |q| Point::new(q.x + dx, q.y + dy), false)
}

fn transform_pline(p: &Pline, t: &ApertureTransform, at: Point) -> Pline {
    if t.is_identity() {
        return translate_pline(p, at.x, at.y);
    }
    let (s, c) = t.rotation_deg.to_radians().sin_cos();
    let flip = t.mirror_x ^ t.mirror_y;
    map_pline(
        p,
        |q| {
            let mut x = q.x * t.scale;
            let mut y = q.y * t.scale;
            if t.mirror_x {
                x = -x;
            }
            if t.mirror_y {
                y = -y;
            }
            Point::new(c * x - s * y + at.x, s * x + c * y + at.y)
        },
        flip,
    )
}

fn transform_shape(shape: &Shape, t: &ApertureTransform, at: Point) -> Shape {
    match shape {
        Shape::Fill { outers, holes } => Shape::Fill {
            outers: outers.iter().map(|l| transform_pline(l, t, at)).collect(),
            holes: holes.iter().map(|l| transform_pline(l, t, at)).collect(),
        },
        Shape::Stroke { pline, width } => Shape::Stroke {
            pline: transform_pline(pline, t, at),
            width: width * t.scale,
        },
    }
}

fn translate_shape(shape: &Shape, dx: f64, dy: f64) -> Shape {
    match shape {
        Shape::Fill { outers, holes } => Shape::Fill {
            outers: outers.iter().map(|l| translate_pline(l, dx, dy)).collect(),
            holes: holes.iter().map(|l| translate_pline(l, dx, dy)).collect(),
        },
        Shape::Stroke { pline, width } => Shape::Stroke {
            pline: translate_pline(pline, dx, dy),
            width: *width,
        },
    }
}

/// A resolved region as a hatch primitive. Even-odd is exact for resolved loops (outer,
/// hole, island-in-hole alternate) and independent of loop orientation.
fn region_primitive(outers: Vec<Pline>, holes: Vec<Pline>) -> Primitive {
    Primitive::Hatch {
        loops: outers.into_iter().chain(holes).collect(),
        rule: LoopRule::EvenOdd,
        solid: true,
        pattern: String::new(),
    }
}

// ---------------------------------------------------------------------------------------
// Reader state machine

#[derive(Clone, Copy, Debug, PartialEq)]
enum Interpolation {
    Linear,
    Clockwise,
    CounterClockwise,
}

struct StepRepeat {
    x: u32,
    y: u32,
    i: f64,
    j: f64,
    items: Vec<Item>,
}

struct Reader<'a> {
    opts: &'a ReadOptions,
    diags: Vec<Diagnostic>,
    format: Option<Format>,
    /// Millimetres per file unit; `None` until `MO` (or the assumed unit) is known.
    scale: Option<f64>,
    units_declared: Option<Unit>,
    macros: BTreeMap<String, MacroDef>,
    apertures: BTreeMap<u32, Aperture>,
    current_aperture: Option<u32>,
    interpolation: Interpolation,
    multi_quadrant: bool,
    dark: bool,
    transform: ApertureTransform,
    current: Point,
    /// Last D operation for coordinate words without one (deprecated form).
    last_op: Option<u32>,
    /// Open draw chain with its width.
    chain: Option<(Pline, f64)>,
    region_mode: bool,
    region_contour: Option<Pline>,
    items: Vec<Item>,
    step_repeat: Option<StepRepeat>,
    /// Open aperture blocks, innermost last.
    blocks: Vec<(u32, Vec<Item>)>,
    function: Option<String>,
    attributes_seen: bool,
    ended: bool,
    warned: std::collections::BTreeSet<&'static str>,
    budget: Budget,
}

/// Import work of one shape (see [`Budget`]).
fn shape_cost(shape: &Shape) -> u64 {
    match shape {
        Shape::Fill { outers, holes } => outers
            .iter()
            .chain(holes)
            .map(|l| Budget::PRIMITIVE + l.vertex_count() as u64)
            .sum(),
        Shape::Stroke { pline, .. } => Budget::PRIMITIVE + pline.vertex_count() as u64,
    }
}

impl<'a> Reader<'a> {
    fn new(opts: &'a ReadOptions) -> Self {
        Self {
            opts,
            diags: Vec::new(),
            format: None,
            scale: None,
            units_declared: None,
            macros: BTreeMap::new(),
            apertures: BTreeMap::new(),
            current_aperture: None,
            interpolation: Interpolation::Linear,
            multi_quadrant: true,
            dark: true,
            transform: ApertureTransform::default(),
            current: Point::new(0.0, 0.0),
            last_op: None,
            chain: None,
            region_mode: false,
            region_contour: None,
            items: Vec::new(),
            step_repeat: None,
            blocks: Vec::new(),
            function: None,
            attributes_seen: false,
            ended: false,
            warned: Default::default(),
            budget: Budget::new(opts.max_work),
        }
    }

    fn warn_once(&mut self, code: &'static str, message: impl Into<String>) {
        if self.warned.insert(code) {
            self.diags.push(Diagnostic::warning(code, message));
        }
    }

    fn scale(&mut self) -> f64 {
        if let Some(s) = self.scale {
            return s;
        }
        let unit = self.opts.assume_units;
        self.diags.push(Diagnostic::warning(
            "gerber.no_units",
            format!("file declares no units (MO); assuming {unit:?}"),
        ));
        let s = unit.to_mm(1.0);
        self.scale = Some(s);
        s
    }

    fn format(&self) -> Result<Format, String> {
        self.format
            .ok_or_else(|| "coordinate data before the FS format specification".to_string())
    }

    // ---- output sinks --------------------------------------------------------------

    fn push_item(&mut self, item: Item) {
        if !self.budget.spend(shape_cost(&item.shape)) {
            return;
        }
        if let Some((_, items)) = self.blocks.last_mut() {
            items.push(item);
        } else if let Some(sr) = &mut self.step_repeat {
            sr.items.push(item);
        } else {
            self.items.push(item);
        }
    }

    fn emit_fill(&mut self, shape: Shape) {
        if let Shape::Fill { outers, .. } = &shape
            && outers.is_empty()
        {
            return;
        }
        let dark = self.dark;
        self.push_item(Item { dark, shape });
    }

    fn flush_chain(&mut self) {
        if let Some((mut pline, width)) = self.chain.take()
            && pline.vertex_count() >= 2
        {
            // A chain that returns to its start is a closed contour (board outlines are
            // drawn this way); drop the duplicate end vertex and close it.
            let n = pline.vertex_count();
            if n >= 4 {
                let (a, b) = (pline.at(0), pline.at(n - 1));
                if (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 {
                    pline.remove_last();
                    pline.set_is_closed(true);
                }
            }
            let dark = self.dark;
            self.push_item(Item {
                dark,
                shape: Shape::Stroke { pline, width },
            });
        }
    }

    fn close_region_contour(&mut self) {
        if let Some(mut contour) = self.region_contour.take() {
            contour.set_is_closed(true);
            match pline::sanitize(&contour) {
                Some(c) if c.vertex_count() >= 2 => self.emit_fill(Shape::Fill {
                    outers: vec![c],
                    holes: Vec::new(),
                }),
                _ => self.warn_once(
                    "gerber.region_degenerate",
                    "a region contour with fewer than three distinct points was dropped",
                ),
            }
        }
    }

    fn close_step_repeat(&mut self) {
        let Some(sr) = self.step_repeat.take() else {
            return;
        };
        let copies = u64::from(sr.x.max(1)) * u64::from(sr.y.max(1));
        let each: u64 = sr.items.iter().map(|it| shape_cost(&it.shape)).sum();
        if !self.budget.spend(copies.saturating_mul(each)) {
            return;
        }
        for ix in 0..sr.x.max(1) {
            for iy in 0..sr.y.max(1) {
                let (dx, dy) = (ix as f64 * sr.i, iy as f64 * sr.j);
                for item in &sr.items {
                    let shape = if dx == 0.0 && dy == 0.0 {
                        item.shape.clone()
                    } else {
                        translate_shape(&item.shape, dx, dy)
                    };
                    self.items.push(Item {
                        dark: item.dark,
                        shape,
                    });
                }
            }
        }
    }

    // ---- commands -------------------------------------------------------------------

    fn extended(&mut self, statements: &[String]) -> Result<(), String> {
        let Some(first) = statements.first() else {
            return Ok(());
        };
        // The code is the first two characters; split on a character boundary, the rest of
        // a malformed file may be anything.
        let split = first.char_indices().nth(2).map_or(first.len(), |(i, _)| i);
        let (code, rest) = first.split_at(split);
        match code {
            "FS" => {
                self.format = Some(Format::parse(rest)?);
                if self.format.is_some_and(|f| f.incremental) {
                    self.warn_once(
                        "gerber.deprecated",
                        "incremental coordinates (FSI) are deprecated; interpreted as specified",
                    );
                }
            }
            "MO" => {
                let unit = match rest {
                    "MM" => Unit::Millimetre,
                    "IN" => Unit::Inch,
                    other => return Err(format!("unknown unit {other:?} in MO")),
                };
                self.units_declared = Some(unit);
                self.scale = Some(unit.to_mm(1.0));
            }
            "AD" => self.aperture_define(rest)?,
            "AM" => {
                let def = macros::parse_macro(rest, &statements[1..])?;
                self.macros.insert(def.name.clone(), def);
            }
            "LP" => {
                self.flush_chain();
                self.dark = match rest {
                    "D" => true,
                    "C" => false,
                    other => return Err(format!("unknown polarity {other:?} in LP")),
                };
            }
            "LM" => {
                self.transform.mirror_x = rest.contains('X');
                self.transform.mirror_y = rest.contains('Y');
                if rest != "N" && !rest.contains('X') && !rest.contains('Y') {
                    return Err(format!("unknown mirror {rest:?} in LM"));
                }
            }
            "LR" => {
                self.transform.rotation_deg = rest
                    .parse()
                    .map_err(|_| format!("bad rotation {rest:?} in LR"))?;
            }
            "LS" => {
                let s: f64 = rest
                    .parse()
                    .map_err(|_| format!("bad scale {rest:?} in LS"))?;
                if s <= 0.0 {
                    return Err(format!("non-positive scale {rest:?} in LS"));
                }
                self.transform.scale = s;
            }
            "SR" => {
                self.flush_chain();
                self.close_step_repeat();
                if rest.is_empty() {
                    return Ok(());
                }
                let words = words(rest)?;
                let get = |c: char| words.iter().find(|(k, _)| *k == c).map(|(_, v)| v.as_str());
                let count = |c: char| -> Result<u32, String> {
                    get(c)
                        .ok_or_else(|| format!("SR without {c}"))?
                        .parse()
                        .map_err(|_| format!("bad SR {c} count"))
                };
                let scale = self.scale();
                let step = |c: char| -> Result<f64, String> {
                    Ok(get(c)
                        .map(|v| v.parse::<f64>().map_err(|_| format!("bad SR {c} step")))
                        .transpose()?
                        .unwrap_or(0.0)
                        * scale)
                };
                self.step_repeat = Some(StepRepeat {
                    x: count('X')?,
                    y: count('Y')?,
                    i: step('I')?,
                    j: step('J')?,
                    items: Vec::new(),
                });
            }
            "AB" => {
                self.flush_chain();
                if rest.is_empty() {
                    let Some((code, items)) = self.blocks.pop() else {
                        return Err("AB close without an open block".into());
                    };
                    self.apertures.insert(
                        code,
                        Aperture {
                            shape: ApertureShape::Block(items),
                        },
                    );
                } else {
                    let code: u32 = rest
                        .strip_prefix('D')
                        .and_then(|d| d.parse().ok())
                        .ok_or_else(|| format!("bad aperture block {rest:?}"))?;
                    self.blocks.push((code, Vec::new()));
                }
            }
            "TF" => {
                self.attributes_seen = true;
                let (name, value) = rest.split_once(',').unwrap_or((rest, ""));
                match name {
                    ".FileFunction" => self.function = Some(value.to_string()),
                    ".FilePolarity" if value.eq_ignore_ascii_case("Negative") => {
                        self.diags.push(Diagnostic::info(
                            "gerber.file_polarity_negative",
                            "TF.FilePolarity is Negative: the image shows where material is absent (e.g. solder mask openings)",
                        ));
                    }
                    _ => {}
                }
            }
            "TA" | "TO" | "TD" => self.attributes_seen = true,
            "IP" => {
                if rest == "NEG" {
                    self.diags.push(Diagnostic::warning(
                        "gerber.deprecated",
                        "IPNEG (negative image polarity) is deprecated and not applied; use LP",
                    ));
                }
            }
            "IN" | "LN" => {}
            "OF" | "SF" | "MI" | "AS" | "IR" => {
                self.warn_once(
                    "gerber.deprecated",
                    format!("deprecated image transform {code} is ignored"),
                );
            }
            other => self.warn_once(
                "gerber.unknown_command",
                format!("unknown extended command %{other}…% ignored"),
            ),
        }
        Ok(())
    }

    fn aperture_define(&mut self, rest: &str) -> Result<(), String> {
        // "D10C,0.5X0.2" | "D11THERMAL80" | "D12R,1X2"
        let rest = rest
            .strip_prefix('D')
            .ok_or_else(|| format!("bad aperture definition {rest:?}"))?;
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let code: u32 = rest[..digits_end]
            .parse()
            .map_err(|_| format!("bad aperture number in AD{rest}"))?;
        if code < 10 {
            return Err(format!("aperture number D{code} is reserved"));
        }
        let (template, mods) = match rest[digits_end..].split_once(',') {
            Some((t, m)) => (t, m),
            None => (&rest[digits_end..], ""),
        };
        let modifiers: Vec<f64> = if mods.is_empty() {
            Vec::new()
        } else {
            mods.split('X')
                .map(|m| {
                    m.trim()
                        .parse::<f64>()
                        .map_err(|_| format!("bad modifier {m:?} in AD D{code}"))
                })
                .collect::<Result<_, _>>()?
        };
        let s = self.scale();
        let hole = |idx: usize| {
            modifiers
                .get(idx)
                .copied()
                .filter(|h| *h > 0.0)
                .map(|h| h * s)
        };
        let with_hole = |outer: Pline, hole_d: Option<f64>| -> Region {
            match hole_d {
                Some(h) => Region::from_loops(
                    [outer, pline::circle(Point::new(0.0, 0.0), h / 2.0)],
                    LoopRule::EvenOdd,
                ),
                None => Region::from_loops([outer], LoopRule::NonZero),
            }
        };
        let origin = Point::new(0.0, 0.0);
        let (region, circle_diameter, extent) = match template {
            "C" => {
                let d = *modifiers.first().ok_or("C aperture needs a diameter")? * s;
                if d < 0.0 {
                    return Err(format!("negative diameter in AD D{code}"));
                }
                let h = hole(1);
                let region = if d > 0.0 {
                    with_hole(pline::circle(origin, d / 2.0), h)
                } else {
                    Region::empty()
                };
                (region, if h.is_none() { Some(d) } else { None }, d)
            }
            "R" | "O" => {
                if modifiers.len() < 2 {
                    return Err(format!("{template} aperture needs width and height"));
                }
                let (w, h) = (modifiers[0] * s, modifiers[1] * s);
                let outer = if template == "R" {
                    pline::rect(Point::new(-w / 2.0, -h / 2.0), Point::new(w / 2.0, h / 2.0))
                } else {
                    obround(w, h)
                };
                (with_hole(outer, hole(2)), None, w.max(h))
            }
            "P" => {
                if modifiers.len() < 2 {
                    return Err("P aperture needs a diameter and a vertex count".into());
                }
                let d = modifiers[0] * s;
                let n = modifiers[1].round();
                if !(3.0..=12.0).contains(&n) {
                    return Err(format!("P aperture with {n} vertices"));
                }
                let rot = modifiers.get(2).copied().unwrap_or(0.0);
                let poly = macros::regular_polygon(origin, d, n as usize, rot);
                (with_hole(poly, hole(3)), None, d)
            }
            name => {
                let def = self
                    .macros
                    .get(name)
                    .ok_or_else(|| format!("aperture D{code} uses unknown template {name:?}"))?
                    .clone();
                let (region, issues) = macros::evaluate(&def, &modifiers, s);
                for issue in issues {
                    self.diags.push(Diagnostic::error(
                        "gerber.macro_primitive",
                        format!("macro {name} (aperture D{code}): {}", issue.message),
                    ));
                }
                let extent = region
                    .bounds()
                    .map(|b| b.width().max(b.height()))
                    .unwrap_or(0.0);
                (region, None, extent)
            }
        };
        if region.incomplete {
            self.warn_once("gerber.incomplete_boolean", INCOMPLETE_BOOLEAN);
        }
        self.apertures.insert(
            code,
            Aperture {
                shape: ApertureShape::Fixed {
                    region,
                    circle_diameter,
                    extent,
                },
            },
        );
        Ok(())
    }

    fn word(&mut self, word: &str) -> Result<(), String> {
        if is_comment(word) {
            return Ok(());
        }
        let parts = words(word)?;
        let mut x = None;
        let mut y = None;
        let mut i = 0.0;
        let mut j = 0.0;
        let mut op: Option<u32> = None;
        for (k, v) in &parts {
            match k {
                'G' => self.g_code(v.parse().map_err(|_| format!("bad G code {v:?}"))?)?,
                'M' => {
                    self.flush_chain();
                    self.ended = true;
                    return Ok(());
                }
                'X' => x = Some(self.format()?.x(v)?),
                'Y' => y = Some(self.format()?.y(v)?),
                'I' => i = self.format()?.x(v)?,
                'J' => j = self.format()?.y(v)?,
                'D' => {
                    let d: u32 = v.parse().map_err(|_| format!("bad D code {v:?}"))?;
                    if d >= 10 {
                        self.select_aperture(d)?;
                    } else {
                        op = Some(d);
                    }
                }
                'N' => {} // sequence numbers (ancient)
                other => {
                    return Err(format!("unexpected {other} in {word:?}"));
                }
            }
        }
        let has_coords = x.is_some() || y.is_some() || i != 0.0 || j != 0.0;
        if op.is_none() && has_coords {
            match self.last_op {
                Some(prev) => {
                    self.warn_once(
                        "gerber.deprecated",
                        "coordinates without an operation code repeat the previous D code (deprecated)",
                    );
                    op = Some(prev);
                }
                None => return Err(format!("coordinates without an operation code in {word:?}")),
            }
        }
        let Some(op) = op else {
            return Ok(());
        };
        let scale = self.scale();
        let fmt = self.format()?;
        let target = {
            let base = if fmt.incremental {
                self.current
            } else {
                Point::new(0.0, 0.0)
            };
            let px = x
                .map(|v| v * scale)
                .map(|v| if fmt.incremental { base.x + v } else { v });
            let py = y
                .map(|v| v * scale)
                .map(|v| if fmt.incremental { base.y + v } else { v });
            Point::new(px.unwrap_or(self.current.x), py.unwrap_or(self.current.y))
        };
        let (i, j) = (i * scale, j * scale);
        match op {
            1 => self.draw(target, i, j)?,
            2 => self.mv(target),
            3 => self.flash(target)?,
            other => return Err(format!("unknown operation code D{other:02}")),
        }
        self.last_op = Some(op);
        Ok(())
    }

    fn g_code(&mut self, g: u32) -> Result<(), String> {
        match g {
            1 => self.interpolation = Interpolation::Linear,
            2 => self.interpolation = Interpolation::Clockwise,
            3 => self.interpolation = Interpolation::CounterClockwise,
            74 => self.multi_quadrant = false,
            75 => self.multi_quadrant = true,
            36 => {
                self.flush_chain();
                self.region_mode = true;
                self.region_contour = None;
            }
            37 => {
                self.close_region_contour();
                self.region_mode = false;
            }
            54 | 55 => self.warn_once(
                "gerber.deprecated",
                format!("G{g:02} is deprecated and ignored"),
            ),
            70 => {
                self.warn_once("gerber.deprecated", "G70 (inches) is deprecated; use MO");
                self.units_declared = Some(Unit::Inch);
                self.scale = Some(Unit::Inch.to_mm(1.0));
            }
            71 => {
                self.warn_once(
                    "gerber.deprecated",
                    "G71 (millimetres) is deprecated; use MO",
                );
                self.units_declared = Some(Unit::Millimetre);
                self.scale = Some(1.0);
            }
            90 => self.warn_once(
                "gerber.deprecated",
                "G90 is deprecated and ignored (absolute is the default)",
            ),
            91 => {
                self.warn_once(
                    "gerber.deprecated",
                    "G91 (incremental coordinates) is deprecated",
                );
                if let Some(f) = &mut self.format {
                    f.incremental = true;
                }
            }
            other => self.warn_once(
                "gerber.unknown_command",
                format!("unknown code G{other:02} ignored"),
            ),
        }
        Ok(())
    }

    fn select_aperture(&mut self, code: u32) -> Result<(), String> {
        if !self.apertures.contains_key(&code) {
            return Err(format!("aperture D{code} selected before it was defined"));
        }
        if self.current_aperture != Some(code) {
            self.flush_chain();
        }
        self.current_aperture = Some(code);
        Ok(())
    }

    fn mv(&mut self, target: Point) {
        self.flush_chain();
        if self.region_mode {
            self.close_region_contour();
        }
        self.current = target;
    }

    /// Bulge of the arc `from -> to` under the current interpolation mode and quadrant
    /// mode, with `(i, j)` the centre offset from `from`. Returns one or two
    /// `(bulge, via)` segments (a full circle needs two).
    fn arc_segments(
        &mut self,
        from: Point,
        to: Point,
        i: f64,
        j: f64,
    ) -> Result<Vec<(Point, f64)>, String> {
        let ccw = self.interpolation == Interpolation::CounterClockwise;
        let candidates: Vec<Point> = if self.multi_quadrant {
            vec![Point::new(from.x + i, from.y + j)]
        } else {
            // Single quadrant: offsets are unsigned; pick the centre that keeps the arc
            // within 90 degrees and on the circle.
            let mut c = Vec::new();
            for sx in [1.0, -1.0] {
                for sy in [1.0, -1.0] {
                    c.push(Point::new(from.x + sx * i, from.y + sy * j));
                }
            }
            c
        };
        let mut best: Option<(f64, Point, f64)> = None; // (radius mismatch, centre, sweep)
        for c in candidates {
            let r0 = (from.x - c.x).hypot(from.y - c.y);
            let r1 = (to.x - c.x).hypot(to.y - c.y);
            if r0 <= 0.0 {
                continue;
            }
            let a0 = (from.y - c.y).atan2(from.x - c.x);
            let a1 = (to.y - c.y).atan2(to.x - c.x);
            let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
            sweep = sweep.rem_euclid(TAU);
            let same_point = (from.x - to.x).abs() < 1e-9 && (from.y - to.y).abs() < 1e-9;
            if same_point {
                sweep = if self.multi_quadrant { TAU } else { 0.0 };
            }
            if !self.multi_quadrant && sweep > std::f64::consts::FRAC_PI_2 + 1e-6 {
                continue;
            }
            let mismatch = (r1 - r0).abs() / r0;
            if best.is_none_or(|(m, _, _)| mismatch < m) {
                best = Some((mismatch, c, sweep));
            }
        }
        let Some((mismatch, c, sweep)) = best else {
            return Err("arc with zero radius".into());
        };
        if mismatch > 1e-3 {
            self.warn_once(
                "gerber.arc_radius_mismatch",
                "an arc's end point is not on the circle defined by its centre offset; the centre offset was trusted",
            );
        }
        if sweep == 0.0 {
            return Ok(vec![(to, 0.0)]);
        }
        let sign = if ccw { 1.0 } else { -1.0 };
        if (sweep - TAU).abs() < 1e-9 {
            // Full circle: two half circles through the opposite point.
            let opposite = Point::new(2.0 * c.x - from.x, 2.0 * c.y - from.y);
            return Ok(vec![(opposite, sign), (to, sign)]);
        }
        if sweep > std::f64::consts::PI + 1e-12 {
            // More than a half circle: split at the midpoint so no bulge exceeds 1.
            let r = (from.x - c.x).hypot(from.y - c.y);
            let a0 = (from.y - c.y).atan2(from.x - c.x);
            let amid = a0 + sign * sweep / 2.0;
            let mid = Point::new(c.x + r * amid.cos(), c.y + r * amid.sin());
            let b = sign * (sweep / 8.0).tan();
            return Ok(vec![(mid, b), (to, b)]);
        }
        Ok(vec![(to, sign * (sweep / 4.0).tan())])
    }

    fn draw(&mut self, target: Point, i: f64, j: f64) -> Result<(), String> {
        let from = self.current;
        let segments = match self.interpolation {
            Interpolation::Linear => vec![(target, 0.0)],
            _ => self.arc_segments(from, target, i, j)?,
        };
        if self.region_mode {
            let contour = self.region_contour.get_or_insert_with(|| {
                let mut p = Pline::new();
                p.add(from.x, from.y, 0.0);
                p
            });
            for (p, bulge) in &segments {
                if let Some(last) = contour.vertex_count().checked_sub(1) {
                    let mut v = contour.at(last);
                    v.bulge = *bulge;
                    contour.set_vertex(last, v);
                }
                contour.add(p.x, p.y, 0.0);
            }
            self.current = target;
            return Ok(());
        }
        let code = self
            .current_aperture
            .ok_or("draw (D01) before any aperture was selected")?;
        let (circle_diameter, extent) = match &self
            .apertures
            .get(&code)
            .expect("selected apertures exist")
            .shape
        {
            ApertureShape::Fixed {
                circle_diameter,
                extent,
                ..
            } => (*circle_diameter, *extent),
            ApertureShape::Block(_) => {
                return Err(format!("aperture block D{code} cannot be used for a draw"));
            }
        };
        let width = match circle_diameter {
            Some(d) => d,
            None => {
                self.warn_once(
                    "gerber.non_circular_draw",
                    "draws with a non-circular aperture are approximated by a round stroke of the aperture's extent",
                );
                extent
            }
        };
        let zero_length = segments.iter().all(|(p, b)| {
            *b == 0.0 && (p.x - from.x).abs() < 1e-12 && (p.y - from.y).abs() < 1e-12
        });
        if zero_length {
            // A zero-length draw marks a dot the size of the aperture.
            self.flush_chain();
            if width > 0.0 {
                let dot = pline::circle(from, width / 2.0);
                self.emit_fill(Shape::Fill {
                    outers: vec![dot],
                    holes: Vec::new(),
                });
            }
            return Ok(());
        }
        if width <= 0.0 {
            self.warn_once(
                "gerber.zero_width_draw",
                "draws with a zero-size aperture produce no image and were dropped",
            );
            self.current = target;
            return Ok(());
        }
        let continue_chain = matches!(&self.chain, Some((p, w)) if *w == width && p.vertex_count() > 0 && {
            let last = p.at(p.vertex_count() - 1);
            (last.x - from.x).abs() < 1e-9 && (last.y - from.y).abs() < 1e-9
        });
        if !continue_chain {
            self.flush_chain();
            let mut p = Pline::new();
            p.add(from.x, from.y, 0.0);
            self.chain = Some((p, width));
        }
        let (p, _) = self.chain.as_mut().expect("chain exists");
        for (pt, bulge) in &segments {
            let last = p.vertex_count() - 1;
            let mut v = p.at(last);
            v.bulge = *bulge;
            p.set_vertex(last, v);
            p.add(pt.x, pt.y, 0.0);
        }
        self.current = target;
        Ok(())
    }

    fn flash(&mut self, at: Point) -> Result<(), String> {
        self.flush_chain();
        if self.region_mode {
            return Err("flash (D03) inside a region (G36) is not allowed".into());
        }
        let code = self
            .current_aperture
            .ok_or("flash (D03) before any aperture was selected")?;
        let aperture = self
            .apertures
            .get(&code)
            .expect("selected apertures exist")
            .clone();
        let t = self.transform;
        match &aperture.shape {
            ApertureShape::Fixed { region, .. } => {
                let shape = Shape::fill_of(region, |l| transform_pline(l, &t, at));
                self.emit_fill(shape);
            }
            ApertureShape::Block(items) => {
                let dark_now = self.dark;
                for item in items {
                    self.push_item(Item {
                        // Block polarities are relative to the polarity at flash time.
                        dark: item.dark == dark_now,
                        shape: transform_shape(&item.shape, &t, at),
                    });
                }
            }
        }
        self.current = at;
        Ok(())
    }

    fn finish(mut self, name: &str) -> Result<SourceDocument, ReadError> {
        self.flush_chain();
        if self.region_mode {
            self.diags.push(Diagnostic::warning(
                "gerber.region_not_closed",
                "file ended inside a region (G36 without G37); the open contour was closed",
            ));
            self.close_region_contour();
        }
        if !self.blocks.is_empty() {
            self.diags.push(Diagnostic::error(
                "gerber.block_not_closed",
                "file ended inside an aperture block (AB without a closing AB)",
            ));
        }
        self.close_step_repeat();
        if !self.ended {
            self.diags.push(Diagnostic::warning(
                "gerber.no_end",
                "file has no M02 end-of-file command",
            ));
        }
        if self.budget.exhausted() {
            return Err(self.budget.error(SourceFormat::Gerber));
        }
        let (primitives, incomplete) = composite(std::mem::take(&mut self.items));
        if incomplete {
            self.warn_once("gerber.incomplete_boolean", INCOMPLETE_BOOLEAN);
        }
        let units = self.units_declared;
        let function = self.function.take();
        Ok(SourceDocument {
            format: SourceFormat::Gerber,
            version: Some(
                if self.attributes_seen {
                    "X2"
                } else {
                    "RS-274X"
                }
                .to_string(),
            ),
            units,
            layers: vec![SourceLayer {
                name: name.to_string(),
                color: None,
                function,
                primitives,
            }],
            diagnostics: self.diags,
        })
    }
}

/// Split a word such as `G01X100Y-200D01` into `(letter, value)` pairs.
fn words(word: &str) -> Result<Vec<(char, String)>, String> {
    let mut out = Vec::new();
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_ascii_alphabetic() {
            return Err(format!("unexpected {c:?} in {word:?}"));
        }
        let mut value = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() || d == '-' || d == '+' || d == '.' {
                value.push(d);
                chars.next();
            } else {
                break;
            }
        }
        out.push((c.to_ascii_uppercase(), value));
    }
    Ok(out)
}

/// Obround (stadium) of `w` by `h` around the origin, counter-clockwise.
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

const INCOMPLETE_BOOLEAN: &str = "part of the image could not be composited and was left out: boundaries within nanometres of each other could not be intersected consistently";

/// Turn the operation list into primitives. Dark-only images pass through as individual
/// primitives (the evaluator unions them); images with clear operations are composited in
/// order and become one region.
fn composite(items: Vec<Item>) -> (Vec<Primitive>, bool) {
    if items.iter().all(|it| it.dark) {
        let prims = items
            .into_iter()
            .map(|it| match it.shape {
                Shape::Fill { outers, holes } => region_primitive(outers, holes),
                Shape::Stroke { pline, width } => Primitive::Path {
                    pline,
                    width: Some(width),
                },
            })
            .collect();
        return (prims, false);
    }
    // Union of one polarity run. Fills without holes are merged in one pass; fills with
    // holes and strokes are unioned on top (a hole must not swallow another flash).
    let run_region = |run: &[&Item]| -> Region {
        let mut simple = Vec::new();
        let mut parts: Vec<Region> = Vec::new();
        let mut strokes: BTreeMap<u64, Vec<Pline>> = BTreeMap::new();
        for it in run {
            match &it.shape {
                Shape::Fill { outers, holes } if holes.is_empty() => {
                    simple.extend(outers.iter().cloned())
                }
                Shape::Fill { outers, holes } => parts.push(Region::from_loops(
                    outers.iter().chain(holes).cloned(),
                    LoopRule::EvenOdd,
                )),
                Shape::Stroke { pline, width } => strokes
                    .entry(width.to_bits())
                    .or_default()
                    .push(pline.clone()),
            }
        }
        let mut r = Region::from_loops(simple, LoopRule::MergeThenNest);
        for (bits, plines) in strokes {
            parts.push(region::stroke_many(&plines, f64::from_bits(bits)));
        }
        for part in parts {
            r = if r.is_empty() { part } else { r.union(&part) };
        }
        r
    };
    let mut acc = Region::empty();
    let mut idx = 0;
    while idx < items.len() {
        let dark = items[idx].dark;
        let mut end = idx;
        while end < items.len() && items[end].dark == dark {
            end += 1;
        }
        let run: Vec<&Item> = items[idx..end].iter().collect();
        let r = run_region(&run);
        acc = if dark {
            if acc.is_empty() { r } else { acc.union(&r) }
        } else if acc.is_empty() {
            acc
        } else {
            acc.difference(&r)
        };
        idx = end;
    }
    if acc.is_empty() {
        return (Vec::new(), acc.incomplete);
    }
    let prim = region_primitive(
        acc.outers().cloned().collect(),
        acc.holes().cloned().collect(),
    );
    (vec![prim], acc.incomplete)
}

/// Read a Gerber file. `name` becomes the layer name (one layer per file).
pub fn read_str(name: &str, text: &str, opts: &ReadOptions) -> Result<SourceDocument, ReadError> {
    let err = |message: String| ReadError {
        format: Some(SourceFormat::Gerber),
        message,
    };
    let cmds = lex(text).map_err(err)?;
    if cmds.is_empty() {
        return Err(err("file contains no Gerber commands".into()));
    }
    let mut reader = Reader::new(opts);
    for (n, cmd) in cmds.iter().enumerate() {
        if reader.ended {
            break;
        }
        let result = match cmd {
            Cmd::Word(w) => reader.word(w),
            Cmd::Extended(st) => reader.extended(st),
        };
        if let Err(message) = result {
            return Err(err(format!("command {}: {message}", n + 1)));
        }
        if reader.budget.exhausted() {
            return Err(reader.budget.error(SourceFormat::Gerber));
        }
    }
    reader.finish(name)
}

pub fn read_bytes(
    name: &str,
    bytes: &[u8],
    opts: &ReadOptions,
) -> Result<SourceDocument, ReadError> {
    read_str(name, &String::from_utf8_lossy(bytes), opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn read(text: &str) -> SourceDocument {
        read_str("test.gbr", text, &ReadOptions::default()).unwrap()
    }

    fn layer_region(doc: &SourceDocument) -> Region {
        let mut r = Region::empty();
        for p in &doc.layers[0].primitives {
            let part = match p {
                Primitive::Hatch { loops, rule, .. } => {
                    Region::from_loops(loops.iter().cloned(), *rule)
                }
                Primitive::Path { pline, width } => region::stroke(pline, width.unwrap()),
            };
            r = if r.is_empty() { part } else { r.union(&part) };
        }
        r
    }

    const HEAD: &str = "%FSLAX24Y24*%\n%MOMM*%\n";

    #[test]
    fn huge_step_and_repeat_is_refused() {
        let text = "%FSLAX46Y46*%%MOMM*%%ADD10C,1*%%SRX3000000Y5I2J2*%D10*X0Y0D03*%SR*%M02*";
        let t = std::time::Instant::now();
        let err = read_str("x.gbr", text, &ReadOptions::default()).unwrap_err();
        assert!(err.message.contains("import refused"), "{}", err.message);
        assert!(t.elapsed().as_secs() < 5);
    }

    #[test]
    fn format_leading_and_trailing_zero_omission() {
        let f = Format::parse("LAX24Y24").unwrap();
        assert_eq!(f.x("12345").unwrap(), 1.2345);
        assert_eq!(f.x("-5").unwrap(), -0.0005);
        let f = Format::parse("TAX24Y24").unwrap();
        assert_eq!(f.x("12").unwrap(), 12.0);
        assert_eq!(f.x("123456").unwrap(), 12.3456);
        assert!(Format::parse("LAX2Y24").is_err());
        let f = Format::parse("LAX36Y36").unwrap();
        assert_eq!(f.x("1.5").unwrap(), 1.5, "decimal points are tolerated");
    }

    #[test]
    fn flashes_draws_and_regions_in_dark_polarity_stay_separate_primitives() {
        let g = format!(
            "{HEAD}%ADD10C,1.0*%\n%ADD11R,2X1*%\n%ADD12O,2X1*%\n%ADD13P,2X4X45*%\n%ADD14C,0.2*%\n\
             D10*X0Y0D03*\nD11*X50000Y0D03*\nD12*X100000Y0D03*\nD13*X150000Y0D03*\n\
             D14*X0Y50000D02*X100000Y50000D01*X100000Y80000D01*\n\
             G36*X0Y100000D02*X20000Y100000D01*X20000Y120000D01*X0Y120000D01*X0Y100000D01*G37*\nM02*"
        );
        let doc = read(&g);
        assert_eq!(doc.format, SourceFormat::Gerber);
        assert_eq!(doc.units, Some(Unit::Millimetre));
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let prims = &doc.layers[0].primitives;
        assert_eq!(prims.len(), 6, "4 flashes + 1 draw chain + 1 region");
        let areas: Vec<f64> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Hatch { loops, rule, .. } => {
                    Some(Region::from_loops(loops.iter().cloned(), *rule).area())
                }
                _ => None,
            })
            .collect();
        assert!((areas[0] - PI / 4.0).abs() < 1e-9, "circle");
        assert!((areas[1] - 2.0).abs() < 1e-9, "rectangle");
        assert!(
            (areas[2] - (1.0 + PI / 4.0)).abs() < 1e-9,
            "obround = 1x1 square + circle"
        );
        // Square of diameter 2: diagonal 2, area 2.
        assert!((areas[3] - 2.0).abs() < 1e-9, "polygon {}", areas[3]);
        assert!((areas[4] - 4.0).abs() < 1e-9, "region");
        let Primitive::Path { pline, width } = &prims[4] else {
            panic!("draw chain should be a path");
        };
        assert_eq!(*width, Some(0.2));
        assert_eq!(pline.vertex_count(), 3);
        assert!(!pline.is_closed());
    }

    #[test]
    fn clear_polarity_is_composited_in_order() {
        // A 10 x 10 dark square, a clear 2 mm circle in it, then a dark 1 mm circle inside
        // the clearing: area = 100 - pi + pi/4.
        let g = format!(
            "{HEAD}%ADD10C,2.0*%\n%ADD11C,1.0*%\n\
             G36*X0Y0D02*X100000Y0D01*X100000Y100000D01*X0Y100000D01*X0Y0D01*G37*\n\
             %LPC*%\nD10*X50000Y50000D03*\n%LPD*%\nD11*X50000Y50000D03*\nM02*"
        );
        let doc = read(&g);
        let prims = &doc.layers[0].primitives;
        assert_eq!(prims.len(), 1, "composited into one region");
        let r = layer_region(&doc);
        assert!(
            (r.area() - (100.0 - PI + PI / 4.0)).abs() < 1e-6,
            "{}",
            r.area()
        );
        assert_eq!(r.outers().count(), 2);
        assert_eq!(r.holes().count(), 1);
    }

    #[test]
    fn arcs_multi_and_single_quadrant() {
        // Multi-quadrant CCW half circle from (0,0) to (10,0) around (5,0).
        let g = format!("{HEAD}%ADD10C,0.1*%\nD10*G75*X0Y0D02*G03X100000Y0I50000J0D01*\nM02*");
        let doc = read(&g);
        let Primitive::Path { pline, .. } = &doc.layers[0].primitives[0] else {
            panic!()
        };
        assert_eq!(pline.vertex_count(), 2);
        assert!(
            (pline.at(0).bulge - 1.0).abs() < 1e-9,
            "{}",
            pline.at(0).bulge
        );
        assert!((pline::length(pline) - PI * 5.0).abs() < 1e-9);

        // Single quadrant: unsigned offsets, quarter circle from (10,0) to (0,10) around 0.
        let g =
            format!("{HEAD}%ADD10C,0.1*%\nD10*G74*X100000Y0D02*G03X0Y100000I100000J0D01*\nM02*");
        let doc = read(&g);
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let Primitive::Path { pline, .. } = &doc.layers[0].primitives[0] else {
            panic!()
        };
        assert!(
            (pline::length(pline) - PI * 5.0).abs() < 1e-9,
            "{}",
            pline::length(pline)
        );
        assert!((pline.at(0).bulge - (PI / 8.0).tan()).abs() < 1e-9);

        // Full circle in multi-quadrant mode (start == end).
        let g =
            format!("{HEAD}%ADD10C,0.1*%\nD10*G75*X100000Y0D02*G02X100000Y0I-100000J0D01*\nM02*");
        let doc = read(&g);
        let Primitive::Path { pline, .. } = &doc.layers[0].primitives[0] else {
            panic!()
        };
        assert!((pline::length(pline) - 2.0 * PI * 10.0).abs() < 1e-9);
    }

    #[test]
    fn step_repeat_and_aperture_blocks_replicate_items() {
        let g = format!(
            "{HEAD}%ADD10C,1.0*%\n%SRX3Y2I20.0J10.0*%\nD10*X0Y0D03*\n%SR*%\n\
             %ABD20*%\nD10*X0Y0D03*X30000Y0D03*\n%AB*%\nD20*X500000Y500000D03*\nM02*"
        );
        let doc = read(&g);
        let prims = &doc.layers[0].primitives;
        assert_eq!(prims.len(), 6 + 2);
        let r = layer_region(&doc);
        assert!((r.area() - 8.0 * PI / 4.0).abs() < 1e-6);
        let b = r.bounds().unwrap();
        assert!(
            (b.max.x - 53.5).abs() < 1e-9 && (b.max.y - 50.5).abs() < 1e-9,
            "{b:?}"
        );
    }

    #[test]
    fn macro_apertures_and_transforms() {
        let g = format!(
            "{HEAD}%AMBOX*21,1,$1,$2,0,0,0*1,0,$3,0,0*%\n%ADD10BOX,4X2X1*%\n\
             D10*X0Y0D03*\n%LR90*%\nX100000Y0D03*\n%LR0*%\n%LS2*%\nX200000Y0D03*\nM02*"
        );
        let doc = read(&g);
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        let prims = &doc.layers[0].primitives;
        assert_eq!(prims.len(), 3);
        let region_of = |p: &Primitive| match p {
            Primitive::Hatch { loops, rule, .. } => {
                Region::from_loops(loops.iter().cloned(), *rule)
            }
            _ => panic!(),
        };
        let a = region_of(&prims[0]);
        assert!((a.area() - (8.0 - PI / 4.0)).abs() < 1e-9);
        assert_eq!(a.holes().count(), 1);
        let b = region_of(&prims[1]).bounds().unwrap();
        assert!(
            (b.width() - 2.0).abs() < 1e-9 && (b.height() - 4.0).abs() < 1e-9,
            "rotated {b:?}"
        );
        let c = region_of(&prims[2]);
        assert!(
            (c.area() - 4.0 * (8.0 - PI / 4.0)).abs() < 1e-9,
            "scaled {}",
            c.area()
        );
    }

    #[test]
    fn x2_attributes_and_diagnostics() {
        let g = format!(
            "%TF.GenerationSoftware,Test*%\n%TF.FileFunction,Copper,L1,Top*%\n%TF.FilePolarity,Positive*%\n{HEAD}%ADD10C,1*%\nD10*X0Y0D03*\nM02*"
        );
        let doc = read(&g);
        assert_eq!(doc.version.as_deref(), Some("X2"));
        assert_eq!(doc.layers[0].function.as_deref(), Some("Copper,L1,Top"));
        assert_eq!(doc.layers[0].name, "test.gbr");

        // Missing units and missing end are diagnosed; a draw with a rectangle warns.
        let doc = read("%FSLAX24Y24*%\n%ADD10R,1X1*%\nD10*X0Y0D02*X10000Y0D01*");
        let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"gerber.no_units"));
        assert!(codes.contains(&"gerber.no_end"));
        assert!(codes.contains(&"gerber.non_circular_draw"));

        // Hard errors: coordinates before FS, undefined aperture.
        assert!(read_str("x", "X100Y100D02*", &ReadOptions::default()).is_err());
        assert!(read_str("x", "%FSLAX24Y24*%%MOMM*%D99*", &ReadOptions::default()).is_err());
        assert!(read_str("x", "", &ReadOptions::default()).is_err());
    }

    #[test]
    fn inch_files_are_converted() {
        let g = "%FSLAX24Y24*%\n%MOIN*%\n%ADD10C,0.1*%\nD10*X10000Y0D03*\nM02*";
        let doc = read(g);
        assert_eq!(doc.units, Some(Unit::Inch));
        let r = layer_region(&doc);
        let b = r.bounds().unwrap();
        assert!((b.min.x - (25.4 - 1.27)).abs() < 1e-9, "{b:?}");
        assert!((r.area() - PI * 1.27 * 1.27).abs() < 1e-6);
    }
}
