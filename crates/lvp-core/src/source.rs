//! Format-independent result of reading an input file, and the front door that detects a
//! file's format and dispatches to the right reader.
//!
//! Every reader produces the same thing: named layers of primitives in millimetres, plus
//! diagnostics. Readers never execute anything from the file and never fetch anything.

use crate::diag::Diagnostic;
use crate::dxf;
use crate::model::SourceFormat;
use crate::pline::Pline;
use crate::region::LoopRule;
use crate::units::Unit;

#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// Chordal tolerance for flattening curves the model cannot hold exactly (ellipses,
    /// splines, Béziers, non-uniformly scaled arcs).
    pub tolerance_mm: f64,
    /// Units applied when the file declares none.
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
    /// carried one (polyline widths, Gerber draws, GDSII paths); `None` is a zero-width
    /// centreline.
    Path { pline: Pline, width: Option<f64> },
    /// Filled area bounded by loops (hatches, flashes, regions, polygons).
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
    /// AutoCAD Color Index from a DXF layer table, if declared.
    pub color: Option<i16>,
    /// What the layer is, when the format says so (Gerber X2 `TF.FileFunction` such as
    /// `Copper,L1,Top`, IPC-2581 `layerFunction`). Matched by `MatchRule::Function`.
    pub function: Option<String>,
    pub primitives: Vec<Primitive>,
}

#[derive(Clone, Debug)]
pub struct SourceDocument {
    pub format: SourceFormat,
    /// Format version string when the file states one (DXF `$ACADVER`, PDF header…).
    pub version: Option<String>,
    /// Declared units; `None` when the file was unitless and `ReadOptions::assume_units`
    /// applied. Coordinates are always millimetres.
    pub units: Option<Unit>,
    /// Sorted by name. May include declared-but-empty layers.
    pub layers: Vec<SourceLayer>,
    pub diagnostics: Vec<Diagnostic>,
}

impl SourceDocument {
    pub fn layer(&self, name: &str) -> Option<&SourceLayer> {
        self.layers.iter().find(|l| l.name == name)
    }
}

/// A file could not be read at all. Partial problems are diagnostics on the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub format: Option<SourceFormat>,
    pub message: String,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.format {
            Some(fm) => write!(f, "{}: {}", format_name(fm), self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ReadError {}

pub fn format_name(f: SourceFormat) -> &'static str {
    match f {
        SourceFormat::Dxf => "DXF",
        SourceFormat::Gerber => "Gerber",
        SourceFormat::Svg => "SVG",
        SourceFormat::Pdf => "PDF",
        SourceFormat::Gds => "GDSII",
        SourceFormat::Ipc2581 => "IPC-2581",
    }
}

/// Extensions commonly used for each format, lower case, without the dot.
pub fn extensions(f: SourceFormat) -> &'static [&'static str] {
    match f {
        SourceFormat::Dxf => &["dxf"],
        SourceFormat::Gerber => &[
            "gbr", "ger", "gtl", "gbl", "gts", "gbs", "gto", "gbo", "gtp", "gbp", "gko", "gml",
            "gm1", "gm2", "gm3", "g1", "g2", "g3", "g4", "g5", "g6", "gp1", "gp2", "gpt", "gpb",
            "pho", "art",
        ],
        SourceFormat::Svg => &["svg"],
        SourceFormat::Pdf => &["pdf"],
        SourceFormat::Gds => &["gds", "gds2", "gdsii", "sf"],
        SourceFormat::Ipc2581 => &["xml", "cvg"],
    }
}

/// Every extension the app accepts, for file pickers.
pub fn all_extensions() -> Vec<&'static str> {
    [
        SourceFormat::Dxf,
        SourceFormat::Gerber,
        SourceFormat::Svg,
        SourceFormat::Pdf,
        SourceFormat::Gds,
        SourceFormat::Ipc2581,
    ]
    .into_iter()
    .flat_map(|f| extensions(f).iter().copied())
    .collect()
}

fn extension_of(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    Some(name[dot + 1..].to_ascii_lowercase())
}

/// Decide a file's format from its content first and its extension second. Content wins
/// because extensions lie (`.xml` may be anything, `.gm1` is a convention).
pub fn detect_format(name: &str, bytes: &[u8]) -> Option<SourceFormat> {
    let head: &[u8] = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if trimmed.starts_with("%PDF-") {
        return Some(SourceFormat::Pdf);
    }
    if bytes.len() >= 6 && bytes[0] == 0 && bytes[1] == 6 && bytes[2] == 0 && bytes[3] == 2 {
        // HEADER record: length 6, type 0x00, data type 0x02.
        return Some(SourceFormat::Gds);
    }
    if trimmed.starts_with("<?xml") || trimmed.starts_with('<') {
        if text.contains("<IPC-2581") {
            return Some(SourceFormat::Ipc2581);
        }
        if text.contains("<svg") {
            return Some(SourceFormat::Svg);
        }
    }
    if trimmed.starts_with("AutoCAD Binary DXF") {
        return Some(SourceFormat::Dxf);
    }
    let gerber_marks = ["%FS", "%MO", "%AD", "%TF", "%LP", "G04", "%AM", "%SR"];
    if gerber_marks.iter().any(|m| text.contains(m)) && !text.contains("SECTION") {
        return Some(SourceFormat::Gerber);
    }
    let looks_like_dxf =
        text.contains("SECTION") || trimmed.starts_with("999") || trimmed.starts_with('0');
    if looks_like_dxf && (text.contains("SECTION") || extension_of(name).as_deref() == Some("dxf"))
    {
        return Some(SourceFormat::Dxf);
    }
    let ext = extension_of(name)?;
    [
        SourceFormat::Dxf,
        SourceFormat::Gerber,
        SourceFormat::Svg,
        SourceFormat::Pdf,
        SourceFormat::Gds,
        SourceFormat::Ipc2581,
    ]
    .into_iter()
    .find(|f| extensions(*f).contains(&ext.as_str()))
}

/// Read any supported file. `name` is used for format detection and for diagnostics only.
pub fn read_any(name: &str, bytes: &[u8], opts: &ReadOptions) -> Result<SourceDocument, ReadError> {
    let Some(format) = detect_format(name, bytes) else {
        return Err(ReadError {
            format: None,
            message: format!(
                "{name}: unrecognised file format (supported: DXF, Gerber RS-274X, SVG, PDF, GDSII, IPC-2581)"
            ),
        });
    };
    match format {
        SourceFormat::Dxf => dxf::read::read_bytes(bytes, opts).map_err(|e| ReadError {
            format: Some(format),
            message: e.to_string(),
        }),
        SourceFormat::Gerber => crate::gerber::read_bytes(name, bytes, opts),
        other => Err(ReadError {
            format: Some(other),
            message: format!("{} reading is not implemented yet", format_name(other)),
        }),
    }
}

/// Case-insensitive glob with `*` (any run) and `?` (one character); no character classes,
/// no escapes, so a pattern can never be expensive: this is linear in the text for each
/// `*` segment (classic two-pointer matching with backtracking to the last `*`).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_semantics() {
        assert!(glob_match("*.gtl", "board.GTL"));
        assert!(glob_match("*-F_Cu.gbr", "proj-F_Cu.gbr"));
        assert!(!glob_match("*.gtl", "board.gbl"));
        assert!(glob_match("Copper,L?,Top", "copper,l1,top"));
        assert!(glob_match("Profile*", "Profile,NP"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("?", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn detection_prefers_content_over_extension() {
        assert_eq!(
            detect_format("x.txt", b"%PDF-1.4\n"),
            Some(SourceFormat::Pdf)
        );
        assert_eq!(
            detect_format("x.gm1", b"G04 hi*\n%FSLAX24Y24*%\n%MOMM*%\n"),
            Some(SourceFormat::Gerber)
        );
        assert_eq!(
            detect_format("x.dxf", b"  0\nSECTION\n  2\nHEADER\n"),
            Some(SourceFormat::Dxf)
        );
        assert_eq!(
            detect_format(
                "x.svg",
                b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
            ),
            Some(SourceFormat::Svg)
        );
        assert_eq!(
            detect_format(
                "x.xml",
                b"<?xml version=\"1.0\"?>\n<IPC-2581 revision=\"C\">"
            ),
            Some(SourceFormat::Ipc2581)
        );
        assert_eq!(
            detect_format("x.gds", &[0, 6, 0, 2, 0, 7, 0, 0x1c, 1, 2]),
            Some(SourceFormat::Gds)
        );
        assert_eq!(detect_format("x.GTL", b""), Some(SourceFormat::Gerber));
        assert_eq!(detect_format("x.bin", b"\x01\x02\x03"), None);
    }

    #[test]
    fn unknown_formats_are_refused_with_a_message() {
        let e = read_any("blob.bin", b"\x01\x02", &ReadOptions::default()).unwrap_err();
        assert!(e.message.contains("unrecognised"));
        let e = read_any("x.svg", b"<svg/>", &ReadOptions::default()).unwrap_err();
        assert_eq!(e.format, Some(SourceFormat::Svg));
        let d = read_any(
            "x.gtl",
            b"%FSLAX24Y24*%%MOMM*%%ADD10C,1*%D10*X0Y0D03*M02*",
            &ReadOptions::default(),
        )
        .unwrap();
        assert_eq!(d.format, SourceFormat::Gerber);
        assert_eq!(d.layers[0].name, "x.gtl");
    }
}
