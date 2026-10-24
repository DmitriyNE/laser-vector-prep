//! Exporters. PDF is the mandatory format; SVG and DXF are the secondary outputs (spec §8).
//!
//! Export carries exactly what enabled passes produced for exporting output groups:
//! no reference layers, no domain outlines, no preview-only jump paths.

pub mod dxf;
pub mod pdf;
pub mod svg;

use cavalier_contours::polyline::PlineSource;

use crate::diag::Diagnostic;
use crate::eval::Evaluation;
use crate::geom::{BBox, Point};
use crate::model::{GroupId, PassId, Profile, Rgb};
use crate::order;
use crate::pline::{self, Pline};

/// One stroked trajectory ready for an exporter: exact lines and arcs in document mm, in
/// the planned direction. Each writer decides how arcs are expressed (DXF and SVG exactly,
/// PDF as cubic Béziers within the export tolerance).
#[derive(Clone, Debug)]
pub struct ExportPath {
    pub pline: Pline,
    pub color: Rgb,
    /// Cosmetic stroke width in mm (importer hint, not a process parameter).
    pub width_mm: f64,
    pub group: GroupId,
}

impl ExportPath {
    pub fn closed(&self) -> bool {
        self.pline.is_closed()
    }

    /// Exact bounds (arc extremes included).
    pub fn bounds(&self) -> Option<BBox> {
        pline::bounds(&self.pline)
    }

    /// Start point and the segments `(bulge, end point)` in drawing order, including the
    /// closing segment of a closed path.
    pub fn segments(&self) -> (Point, Vec<(f64, Point)>) {
        let p = &self.pline;
        let n = p.vertex_count();
        let at = |i: usize| {
            let v = p.at(i % n);
            Point::new(v.x, v.y)
        };
        let count = if p.is_closed() {
            n
        } else {
            n.saturating_sub(1)
        };
        (
            at(0),
            (0..count).map(|i| (p.at(i).bulge, at(i + 1))).collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportError {
    NothingToExport,
    Pdf(pdf::PdfError),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::NothingToExport => write!(
                f,
                "nothing to export: no enabled pass produced paths for an exporting group"
            ),
            ExportError::Pdf(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<pdf::PdfError> for ExportError {
    fn from(e: pdf::PdfError) -> Self {
        ExportError::Pdf(e)
    }
}

/// Gather exportable paths in planner order. Groups with `export: false` are skipped;
/// passes pointing at unknown groups are reported.
/// Flattened paths of every exporting group, group by group in profile order and within a
/// group in the planned order (spec §7): the PDF content stream carries that sequence.
pub fn collect_paths(profile: &Profile, ev: &Evaluation) -> (Vec<ExportPath>, Vec<Diagnostic>) {
    let mut out = Vec::new();
    let mut diags = Vec::new();
    for (pass_id, pr) in &ev.passes {
        if profile.group(&pr.group).is_none() {
            diags.push(Diagnostic::error(
                "export.unknown_group",
                format!("pass {pass_id} targets unknown group {}", pr.group),
            ));
        }
    }
    let paths_by_pass = ev.pass_paths();
    for group in &profile.output_groups {
        if !group.export {
            continue;
        }
        let width_mm = group
            .stroke_width_mm
            .unwrap_or(profile.export.stroke_width_mm);
        // Planned order when available, generated order otherwise.
        let ordered: Vec<(PassId, Pline)> = match ev.group_plan(&group.id) {
            Some(plan) => plan
                .order
                .iter()
                .filter_map(|r| {
                    paths_by_pass
                        .get(&r.pass)
                        .and_then(|v| v.get(r.index))
                        .map(|p| (r.pass.clone(), order::oriented(p, r)))
                })
                .collect(),
            None => ev
                .passes
                .iter()
                .filter(|(_, r)| r.group == group.id)
                .flat_map(|(id, r)| r.paths.iter().map(move |p| (id.clone(), p.clone())))
                .collect(),
        };
        for (_, p) in ordered {
            if p.vertex_count() >= 2 {
                out.push(ExportPath {
                    pline: p,
                    color: group.color,
                    width_mm,
                    group: group.id.clone(),
                });
            }
        }
    }
    (out, diags)
}

/// One colour-separated PDF with every exporting group.
pub fn export_pdf(
    profile: &Profile,
    ev: &Evaluation,
) -> Result<(Vec<u8>, Vec<Diagnostic>), ExportError> {
    let (paths, diags) = collect_paths(profile, ev);
    if paths.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    let opts = pdf::PdfOptions {
        margin_mm: profile.export.margin_mm,
        title: profile.name.clone(),
        keywords: group_summary(profile),
        layout: None,
        curve_tolerance_mm: profile.export.flatten_tolerance_mm,
    };
    let (bytes, _) = pdf::write(&paths, &opts)?;
    Ok((bytes, diags))
}

/// One exported file of a per-group export.
#[derive(Clone, Debug)]
pub struct GroupFile {
    pub group: GroupId,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// One PDF per exporting group, all on the identical page layout so they register.
pub fn export_pdf_per_group(
    profile: &Profile,
    ev: &Evaluation,
) -> Result<(Vec<GroupFile>, Vec<Diagnostic>), ExportError> {
    let (paths, diags) = collect_paths(profile, ev);
    if paths.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    let layout =
        pdf::Layout::fit(&paths, profile.export.margin_mm).ok_or(ExportError::NothingToExport)?;
    let mut files = Vec::new();
    for group in profile.output_groups.iter().filter(|g| g.export) {
        let subset: Vec<ExportPath> = paths
            .iter()
            .filter(|p| p.group == group.id)
            .cloned()
            .collect();
        if subset.is_empty() {
            continue;
        }
        let opts = pdf::PdfOptions {
            margin_mm: profile.export.margin_mm,
            title: format!("{} - {}", profile.name, group.name),
            keywords: group_summary(profile),
            layout: Some(layout),
            curve_tolerance_mm: profile.export.flatten_tolerance_mm,
        };
        let (bytes, _) = pdf::write(&subset, &opts)?;
        files.push(GroupFile {
            group: group.id.clone(),
            name: group.name.clone(),
            bytes,
        });
    }
    Ok((files, diags))
}

/// SVG with one Inkscape layer per exporting group; same page as the PDF.
pub fn export_svg(
    profile: &Profile,
    ev: &Evaluation,
) -> Result<(Vec<u8>, Vec<Diagnostic>), ExportError> {
    let (paths, diags) = collect_paths(profile, ev);
    if paths.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    let groups: Vec<(GroupId, String)> = profile
        .output_groups
        .iter()
        .filter(|g| g.export)
        .map(|g| (g.id.clone(), g.name.clone()))
        .collect();
    let text = svg::write(
        &paths,
        &svg::SvgOptions {
            margin_mm: profile.export.margin_mm,
            title: &profile.name,
            groups: &groups,
        },
    )
    .ok_or(ExportError::NothingToExport)?;
    Ok((text.into_bytes(), diags))
}

/// DXF with one layer per exporting group, arcs preserved, model coordinates (no page).
pub fn export_dxf(
    profile: &Profile,
    ev: &Evaluation,
) -> Result<(Vec<u8>, Vec<Diagnostic>), ExportError> {
    let mut diags = Vec::new();
    let paths_by_pass = ev.pass_paths();
    let mut groups = Vec::new();
    for group in profile.output_groups.iter().filter(|g| g.export) {
        let paths: Vec<Pline> = match ev.group_plan(&group.id) {
            Some(plan) => plan
                .order
                .iter()
                .filter_map(|r| {
                    paths_by_pass
                        .get(&r.pass)
                        .and_then(|v| v.get(r.index))
                        .map(|p| order::oriented(p, r))
                })
                .collect(),
            None => ev
                .passes
                .iter()
                .filter(|(_, r)| r.group == group.id)
                .flat_map(|(_, r)| r.paths.iter().cloned())
                .collect(),
        };
        if paths.is_empty() {
            continue;
        }
        groups.push(dxf::DxfGroup {
            name: group.name.clone(),
            color: group.color,
            paths,
        });
    }
    if groups.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    for g in &groups {
        if dxf::layer_name(&g.name) != g.name {
            diags.push(Diagnostic::info(
                "export.dxf_layer_name",
                format!(
                    "group {:?} is written as DXF layer {:?}",
                    g.name,
                    dxf::layer_name(&g.name)
                ),
            ));
        }
    }
    Ok((dxf::write(&groups).into_bytes(), diags))
}

/// Human-readable group list for PDF metadata. Names colours, never laser settings.
fn group_summary(profile: &Profile) -> String {
    profile
        .output_groups
        .iter()
        .filter(|g| g.export)
        .map(|g| format!("{} rgb({},{},{})", g.name, g.color.0, g.color.1, g.color.2))
        .collect::<Vec<_>>()
        .join("; ")
}
