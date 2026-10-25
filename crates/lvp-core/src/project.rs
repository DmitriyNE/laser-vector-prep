//! The project: everything the geometry worker owns for one editing session, exposed in
//! UI-friendly form. Sources and the active profile go in; JSON summaries, flat preview
//! buffers and export bytes come out. Nothing here persists anything.

use cavalier_contours::polyline::PlineSource;
use serde::Serialize;

use crate::diag::{Diagnostic, Severity};
use crate::eval::{BindingStatus, Evaluation, Evaluator, Sources};
use crate::export::{self, ExportError, GroupFile};
use crate::geom::BBox;
use crate::model::*;
use crate::order::{self, Heatmap, Stats};
use crate::patterns::Geometry;
use crate::pline::{self, Pline};
use crate::profile::{self, ProfileError};
use crate::source::{Primitive, ReadError, ReadOptions, SourceDocument, read_any};
use crate::units::Unit;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticJson {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    pub context: Option<String>,
}

impl From<&Diagnostic> for DiagnosticJson {
    fn from(d: &Diagnostic) -> Self {
        Self {
            severity: match d.severity {
                Severity::Info => "info",
                Severity::Warning => "warning",
                Severity::Error => "error",
            },
            code: d.code,
            message: d.message.clone(),
            context: d.context.clone(),
        }
    }
}

fn diags(ds: &[Diagnostic]) -> Vec<DiagnosticJson> {
    ds.iter().map(DiagnosticJson::from).collect()
}

fn bbox_json(b: Option<BBox>) -> Option<[f64; 4]> {
    b.map(|b| [b.min.x, b.min.y, b.max.x, b.max.y])
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLayerSummary {
    pub name: String,
    pub color: Option<i16>,
    /// Declared layer function (Gerber X2, IPC-2581), if any.
    pub function: Option<String>,
    pub closed_paths: usize,
    pub open_paths: usize,
    pub hatches: usize,
    pub bounds_mm: Option<[f64; 4]>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub index: usize,
    pub name: String,
    pub format: SourceFormat,
    pub version: Option<String>,
    /// Declared units; `None` means the file was unitless and `assumed_units` applied.
    pub units: Option<String>,
    pub assumed_units: String,
    pub bounds_mm: Option<[f64; 4]>,
    pub layers: Vec<SourceLayerSummary>,
    pub diagnostics: Vec<DiagnosticJson>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingSummary {
    pub id: SourceId,
    pub name: String,
    pub required: bool,
    pub status: &'static str,
    pub file: Option<String>,
    pub layer: Option<String>,
    pub candidates: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSummary {
    pub id: LayerId,
    pub name: String,
    pub kind: GeometryKind,
    pub outers: usize,
    pub holes: usize,
    pub paths: usize,
    pub area_mm2: Option<f64>,
    pub bounds_mm: Option<[f64; 4]>,
    pub cached: bool,
    pub diagnostics: Vec<DiagnosticJson>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassSummary {
    pub id: PassId,
    pub name: String,
    pub layer: LayerId,
    pub group: GroupId,
    pub paths: usize,
    pub closed: usize,
    pub open: usize,
    pub length_mm: f64,
    pub cached: bool,
    pub diagnostics: Vec<DiagnosticJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<AuditSummary>,
}

/// Coverage audit of a pass (spec §10) against the group's assumed marking width. Areas in
/// mm²; a geometric surrogate, not a dose.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditSummary {
    pub width_mm: f64,
    pub region_mm2: f64,
    pub missed_mm2: f64,
    pub spill_mm2: f64,
    pub short_segments: usize,
}

/// Path-order plan of one output group: what the planner changed and what it predicts
/// under the straight-jump assumption. Nothing here is a dose or a machine setting.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub id: GroupId,
    pub name: String,
    pub mode: OptimizerMode,
    pub paths: usize,
    pub before: Stats,
    pub after: Stats,
    /// A footprint heatmap over protected regions exists for this group.
    pub heatmap: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSummary {
    pub revision: u64,
    pub bounds_mm: Option<[f64; 4]>,
    pub bindings: Vec<BindingSummary>,
    pub unbound_layers: Vec<(String, String)>,
    pub layers: Vec<LayerSummary>,
    pub passes: Vec<PassSummary>,
    pub groups: Vec<GroupSummary>,
    pub diagnostics: Vec<DiagnosticJson>,
    pub has_errors: bool,
}

/// Polylines for drawing, arcs unflattened: path `i` has vertices
/// `starts[i] .. starts[i+1]`, vertex `k` at `coords[2k], coords[2k+1]` with `bulges[k]` the
/// bulge (`tan(sweep / 4)`) of the segment leaving it (for a closed path, the last vertex's
/// bulge closes it). `starts` has one more entry than there are paths. Coordinates are mm
/// relative to `origin` (the buffer's lower-left corner), as `f32`: offsets within a
/// drawing stay precise to far below a micrometre wherever it sits, which absolute `f32`
/// coordinates do not (8 µm steps at 100 m). The preview is never the authoritative
/// geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewBuffer {
    pub origin: [f64; 2],
    pub coords: Vec<f32>,
    pub bulges: Vec<f32>,
    pub starts: Vec<u32>,
    pub closed: Vec<u8>,
}

/// Smallest x and y of the points (0, 0 when there are none).
fn lower_left(points: impl Iterator<Item = (f64, f64)>) -> [f64; 2] {
    points
        .fold(None, |acc: Option<[f64; 2]>, (x, y)| {
            Some(acc.map_or([x, y], |[ax, ay]| [ax.min(x), ay.min(y)]))
        })
        .unwrap_or([0.0, 0.0])
}

impl PreviewBuffer {
    pub fn from_plines<'a, I: IntoIterator<Item = &'a Pline>>(plines: I) -> Self {
        let plines: Vec<&Pline> = plines
            .into_iter()
            .filter(|p| p.vertex_count() >= 2)
            .collect();
        let origin = lower_left(
            plines
                .iter()
                .flat_map(|p| p.iter_vertexes().map(|v| (v.x, v.y))),
        );
        let mut b = PreviewBuffer {
            origin,
            starts: vec![0],
            ..Default::default()
        };
        for p in plines {
            let n = p.vertex_count();
            for (i, v) in p.iter_vertexes().enumerate() {
                b.coords.push((v.x - origin[0]) as f32);
                b.coords.push((v.y - origin[1]) as f32);
                // An open path's last vertex has no outgoing segment.
                let bulge = if !p.is_closed() && i + 1 == n {
                    0.0
                } else {
                    v.bulge
                };
                b.bulges.push(bulge as f32);
            }
            b.starts.push((b.coords.len() / 2) as u32);
            b.closed.push(u8::from(p.is_closed()));
        }
        b
    }

    pub fn path_count(&self) -> usize {
        self.closed.len()
    }
}

pub struct Project {
    pub sources: Sources,
    pub profile: Profile,
    read_options: ReadOptions,
    evaluator: Evaluator,
    /// The newest completed evaluation and the profile it was made from. A newer profile
    /// whose evaluation was cancelled does not replace it, so summaries, previews and
    /// exports always describe the same result.
    last: Option<(Evaluation, Profile)>,
    revision: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    /// Starts with the first built-in preset so a fresh session can import and export.
    pub fn new() -> Self {
        Self {
            sources: Sources::default(),
            profile: profile::builtin_presets().remove(0),
            read_options: ReadOptions::default(),
            evaluator: Evaluator::new(),
            last: None,
            revision: 0,
        }
    }

    pub fn set_assumed_units(&mut self, units: Unit) {
        self.read_options.assume_units = units;
    }

    /// Import a file of any supported format (detected from content and name).
    pub fn add_source(&mut self, name: &str, bytes: &[u8]) -> Result<SourceSummary, ReadError> {
        let doc = read_any(name, bytes, &self.read_options)?;
        let idx = self.sources.add(name, doc);
        Ok(self.source_summary(idx).expect("just added"))
    }

    pub fn replace_source(
        &mut self,
        index: usize,
        bytes: &[u8],
    ) -> Result<SourceSummary, ReadError> {
        let name = self
            .sources
            .files
            .get(index)
            .map(|f| f.name.clone())
            .ok_or_else(|| ReadError {
                format: None,
                message: format!("no source at index {index}"),
            })?;
        let doc = read_any(&name, bytes, &self.read_options)?;
        self.sources.replace(index, doc);
        Ok(self.source_summary(index).expect("exists"))
    }

    pub fn remove_source(&mut self, index: usize) {
        self.sources.remove(index);
    }

    pub fn source_summary(&self, index: usize) -> Option<SourceSummary> {
        let f = self.sources.files.get(index)?;
        let layers: Vec<SourceLayerSummary> = f
            .doc
            .layers
            .iter()
            .map(|l| {
                let mut s = SourceLayerSummary {
                    name: l.name.clone(),
                    color: l.color,
                    function: l.function.clone(),
                    closed_paths: 0,
                    open_paths: 0,
                    hatches: 0,
                    bounds_mm: None,
                };
                let mut bb: Option<BBox> = None;
                for p in &l.primitives {
                    let b = match p {
                        Primitive::Path { pline, .. } => {
                            if pline.is_closed() {
                                s.closed_paths += 1
                            } else {
                                s.open_paths += 1
                            }
                            pline::bounds(pline)
                        }
                        Primitive::Hatch { loops, .. } => {
                            s.hatches += 1;
                            loops
                                .iter()
                                .filter_map(pline::bounds)
                                .reduce(|a, b| a.union(b))
                        }
                    };
                    bb = match (bb, b) {
                        (Some(a), Some(b)) => Some(a.union(b)),
                        (a, b) => a.or(b),
                    };
                }
                s.bounds_mm = bbox_json(bb);
                s
            })
            .collect();
        let bounds = layers
            .iter()
            .filter_map(|l| l.bounds_mm)
            .map(|b| BBox {
                min: crate::geom::Point::new(b[0], b[1]),
                max: crate::geom::Point::new(b[2], b[3]),
            })
            .reduce(|a, b| a.union(b));
        Some(SourceSummary {
            index,
            name: f.name.clone(),
            format: f.format,
            version: f.doc.version.clone(),
            units: f.doc.units.map(|u| format!("{u:?}")),
            assumed_units: format!("{:?}", self.read_options.assume_units),
            bounds_mm: bbox_json(bounds),
            layers,
            diagnostics: diags(&f.doc.diagnostics),
        })
    }

    pub fn source_summaries(&self) -> Vec<SourceSummary> {
        (0..self.sources.files.len())
            .filter_map(|i| self.source_summary(i))
            .collect()
    }

    pub fn document(&self, index: usize) -> Option<&SourceDocument> {
        self.sources.files.get(index).map(|f| &f.doc)
    }

    /// Replace the active profile from JSON. On any error the current profile is kept.
    pub fn set_profile_json(&mut self, json: &str) -> Result<(), ProfileError> {
        self.profile = profile::parse_profile(json)?;
        Ok(())
    }

    pub fn set_profile(&mut self, profile: Profile) {
        self.profile = profile;
    }

    pub fn profile_json(&self) -> String {
        profile::to_json(&self.profile)
    }

    /// Re-evaluate with the current profile and sources. Cached layers and passes whose
    /// inputs did not change are reused.
    pub fn evaluate(&mut self) -> EvaluationSummary {
        self.try_evaluate()
            .expect("nothing is cancelled outside a cancellable computation")
    }

    /// [`Project::evaluate`] that the host may cancel ([`crate::cancel`]). A cancelled run
    /// leaves the previous evaluation in place, so previews and exports still match it.
    pub fn try_evaluate(&mut self) -> Result<EvaluationSummary, crate::cancel::Cancelled> {
        let ev = self.evaluator.try_evaluate(&self.profile, &self.sources)?;
        self.revision += 1;
        self.last = Some((ev, self.profile.clone()));
        Ok(self.summary().expect("just evaluated"))
    }

    pub fn evaluation(&self) -> Option<&Evaluation> {
        self.last.as_ref().map(|(ev, _)| ev)
    }

    pub fn summary(&self) -> Option<EvaluationSummary> {
        let (ev, profile) = self.last.as_ref()?;
        let bindings = profile
            .source_bindings
            .iter()
            .map(|b| {
                let (status, file, layer, candidates) = match ev.bindings.get(&b.id) {
                    Some(BindingStatus::Matched(i, l)) => (
                        "matched",
                        self.sources.files.get(*i).map(|f| f.name.clone()),
                        Some(l.clone()),
                        vec![],
                    ),
                    Some(BindingStatus::Ambiguous(c)) => (
                        "ambiguous",
                        None,
                        None,
                        c.iter()
                            .filter_map(|(i, l)| {
                                self.sources
                                    .files
                                    .get(*i)
                                    .map(|f| (f.name.clone(), l.clone()))
                            })
                            .collect(),
                    ),
                    _ => ("missing", None, None, vec![]),
                };
                BindingSummary {
                    id: b.id.clone(),
                    name: b.name.clone(),
                    required: b.required,
                    status,
                    file,
                    layer,
                    candidates,
                }
            })
            .collect();
        let layers = profile
            .working_layers
            .iter()
            .filter_map(|l| {
                let r = ev.layers.get(&l.id)?;
                let (outers, holes, paths, area, bounds) = match &r.geometry {
                    Geometry::Region(reg) => (
                        reg.outers().count(),
                        reg.holes().count(),
                        0,
                        Some(reg.area()),
                        reg.bounds(),
                    ),
                    Geometry::Linework(ps) => (
                        0,
                        0,
                        ps.len(),
                        None,
                        ps.iter()
                            .filter_map(pline::bounds)
                            .reduce(|a, b| a.union(b)),
                    ),
                };
                Some(LayerSummary {
                    id: l.id.clone(),
                    name: l.name.clone(),
                    kind: l.kind,
                    outers,
                    holes,
                    paths,
                    area_mm2: area,
                    bounds_mm: bbox_json(bounds),
                    cached: r.cached,
                    diagnostics: diags(&r.diagnostics),
                })
            })
            .collect();
        let mut all_bounds: Option<BBox> = None;
        let passes = ev
            .passes
            .iter()
            .map(|(id, pr)| {
                let name = profile
                    .layer(&pr.layer)
                    .and_then(|l| l.passes.iter().find(|p| &p.id == id))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                let exported = profile.group(&pr.group).is_some_and(|g| g.export);
                if exported
                    && let Some(b) = pr
                        .paths
                        .iter()
                        .filter_map(pline::bounds)
                        .reduce(|a, b| a.union(b))
                {
                    all_bounds = Some(all_bounds.map_or(b, |a| a.union(b)));
                }
                PassSummary {
                    id: id.clone(),
                    name,
                    layer: pr.layer.clone(),
                    group: pr.group.clone(),
                    paths: pr.paths.len(),
                    closed: pr.paths.iter().filter(|p| p.is_closed()).count(),
                    open: pr.paths.iter().filter(|p| !p.is_closed()).count(),
                    length_mm: pr.paths.iter().map(pline::length).sum(),
                    cached: pr.cached,
                    diagnostics: diags(&pr.diagnostics),
                    audit: pr.audit.as_ref().map(|a| AuditSummary {
                        width_mm: a.width_mm,
                        region_mm2: a.region_mm2,
                        missed_mm2: a.missed.area().max(0.0),
                        spill_mm2: a.spill.area().max(0.0),
                        short_segments: a.short_segments,
                    }),
                }
            })
            .collect();
        let groups = ev
            .groups
            .iter()
            .filter_map(|(id, plan)| {
                let g = profile.group(id)?;
                Some(GroupSummary {
                    id: id.clone(),
                    name: g.name.clone(),
                    mode: plan.mode,
                    paths: plan.order.len(),
                    before: plan.before.clone(),
                    after: plan.after.clone(),
                    heatmap: plan.heatmap.is_some(),
                })
            })
            .collect();
        Some(EvaluationSummary {
            revision: self.revision,
            bounds_mm: bbox_json(all_bounds),
            bindings,
            unbound_layers: ev.unbound_layers.clone(),
            layers,
            passes,
            groups,
            diagnostics: diags(&ev.diagnostics),
            has_errors: ev.has_errors(),
        })
    }

    pub fn preview_source(&self, file: usize, layer: &str) -> Option<PreviewBuffer> {
        let l = self.sources.files.get(file)?.doc.layer(layer)?;
        let plines: Vec<&Pline> = l
            .primitives
            .iter()
            .flat_map(|p| match p {
                Primitive::Path { pline, .. } => vec![pline],
                Primitive::Hatch { loops, .. } => loops.iter().collect(),
            })
            .collect();
        Some(PreviewBuffer::from_plines(plines))
    }

    pub fn preview_layer(&self, id: &LayerId) -> Option<PreviewBuffer> {
        let r = self.last.as_ref()?.0.layers.get(id)?;
        Some(match &r.geometry {
            Geometry::Region(reg) => PreviewBuffer::from_plines(reg.loops.iter()),
            Geometry::Linework(ps) => PreviewBuffer::from_plines(ps.iter()),
        })
    }

    /// Missed (`spill == false`) or spill area of a pass's coverage audit.
    pub fn preview_audit(&self, id: &PassId, spill: bool) -> Option<PreviewBuffer> {
        let a = self.last.as_ref()?.0.pass(id)?.audit.as_ref()?;
        let r = if spill { &a.spill } else { &a.missed };
        Some(PreviewBuffer::from_plines(r.loops.iter()))
    }

    pub fn preview_pass(&self, id: &PassId) -> Option<PreviewBuffer> {
        let pr = self.last.as_ref()?.0.pass(id)?;
        Some(PreviewBuffer::from_plines(pr.paths.iter()))
    }

    /// Hypothetical straight beam-off jumps of a group's planned order (preview only).
    pub fn preview_jumps(&self, group: &GroupId) -> Option<PreviewBuffer> {
        let (ev, profile) = self.last.as_ref()?;
        let plan = ev.group_plan(group)?;
        let paths = ev.pass_paths();
        let jumps = order::jumps(plan, &paths, &profile.optimizer);
        let origin = lower_left(jumps.iter().flat_map(|(a, b)| [(a.x, a.y), (b.x, b.y)]));
        let mut b = PreviewBuffer {
            origin,
            starts: vec![0],
            ..Default::default()
        };
        let local = |v: f64, o: f64| (v - o) as f32;
        for (from, to) in jumps {
            b.coords.extend([
                local(from.x, origin[0]),
                local(from.y, origin[1]),
                local(to.x, origin[0]),
                local(to.y, origin[1]),
            ]);
            b.bulges.extend([0.0, 0.0]);
            b.starts.push((b.coords.len() / 2) as u32);
            b.closed.push(0);
        }
        Some(b)
    }

    /// Jump footprint coverage over the protected regions, for the preview overlay.
    pub fn heatmap(&self, group: &GroupId) -> Option<&Heatmap> {
        self.last.as_ref()?.0.group_plan(group)?.heatmap.as_ref()
    }

    pub fn export_pdf(&self) -> Result<Vec<u8>, ExportError> {
        let (ev, profile) = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_pdf(profile, ev).map(|(bytes, _)| bytes)
    }

    pub fn export_svg(&self) -> Result<Vec<u8>, ExportError> {
        let (ev, profile) = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_svg(profile, ev).map(|(bytes, _)| bytes)
    }

    pub fn export_dxf(&self) -> Result<Vec<u8>, ExportError> {
        let (ev, profile) = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_dxf(profile, ev).map(|(bytes, _)| bytes)
    }

    pub fn export_pdf_per_group(&self) -> Result<Vec<GroupFile>, ExportError> {
        let (ev, profile) = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_pdf_per_group(profile, ev).map(|(files, _)| files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_buffers_stay_precise_far_from_the_origin() {
        // A 20 µm square 500 m from the origin: absolute f32 coordinates would be off by
        // up to 16 µm there; offsets from the buffer's own origin are not.
        let (x0, y0) = (500_000.123_456, -250_000.654_321);
        let sq = pline::from_points(
            [(0.0, 0.0), (0.02, 0.0), (0.02, 0.02), (0.0, 0.02)]
                .map(|(dx, dy)| crate::geom::Point::new(x0 + dx, y0 + dy)),
            true,
        );
        let b = PreviewBuffer::from_plines([&sq]);
        assert_eq!(b.origin, [x0, y0]);
        for (k, v) in sq.iter_vertexes().enumerate() {
            let x = b.origin[0] + f64::from(b.coords[2 * k]);
            let y = b.origin[1] + f64::from(b.coords[2 * k + 1]);
            assert!((x - v.x).abs() < 1e-9 && (y - v.y).abs() < 1e-9, "{x}, {y}");
        }
        assert_eq!(PreviewBuffer::from_plines([]).origin, [0.0, 0.0]);
    }

    fn board() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/dxf/board-islands.dxf"
        ))
        .unwrap()
    }

    fn coarse(p: &mut Project) {
        if let Pattern::ContourFill { pitch_mm, .. } =
            &mut p.profile.working_layers[0].passes[0].pattern
        {
            *pitch_mm = 0.2;
        }
    }

    #[test]
    fn import_evaluate_preview_export() {
        let mut p = Project::new();
        coarse(&mut p);
        let s = p.add_source("board-islands.dxf", &board()).unwrap();
        assert_eq!(s.units.as_deref(), Some("Millimetre"));
        // Layer table declares "0" too; it is empty.
        assert_eq!(s.layers.len(), 3);
        assert!(
            s.layers
                .iter()
                .any(|l| l.name == "0" && l.closed_paths + l.open_paths + l.hatches == 0)
        );
        let copper = s.layers.iter().find(|l| l.name == "TOP_LAYER").unwrap();
        assert_eq!(
            (copper.closed_paths, copper.open_paths, copper.hatches),
            (11, 2, 0)
        );

        let e = p.evaluate();
        assert_eq!(e.revision, 1);
        assert!(!e.has_errors, "{:?}", e.diagnostics);
        assert!(e.bindings.iter().all(|b| b.status == "matched"));
        // Copper, outline, and the artwork box and keep-clear background used for ordering.
        assert_eq!(e.layers.len(), 4);
        assert_eq!(e.layers[0].outers, 10, "ten copper islands");
        assert_eq!(e.layers[0].holes, 1);
        assert_eq!(e.passes.len(), 2);
        let fill = &e.passes[0];
        assert!(fill.paths > 10 && fill.length_mm > 50.0);
        let b = e.bounds_mm.unwrap();
        assert!(
            b[0].abs() < 1e-9 && b[1].abs() < 1e-9 && (b[2] - 25.0).abs() < 1e-9,
            "outline sets the bounds: {b:?}"
        );

        let buf = p.preview_pass(&fill.id).unwrap();
        assert_eq!(buf.path_count(), fill.paths);
        assert_eq!(buf.starts.len(), fill.paths + 1);
        assert!(buf.starts.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(*buf.starts.last().unwrap() as usize * 2, buf.coords.len());
        // Jumps are planned per group: the copper group holds only the fill pass here.
        let jumps = p.preview_jumps(&GroupId::new("grp-copper")).unwrap();
        assert_eq!(jumps.path_count(), fill.paths - 1);
        let summary = p.summary().unwrap();
        let copper = summary
            .groups
            .iter()
            .find(|g| g.id == GroupId::new("grp-copper"))
            .unwrap();
        assert_eq!(copper.paths, fill.paths);
        assert_eq!(copper.after.jumps, fill.paths - 1);
        assert_eq!(copper.mode, OptimizerMode::BleedAware);
        assert!(copper.heatmap, "the keep-clear background is protected");
        let layer = p.preview_layer(&LayerId::new("wl-copper")).unwrap();
        assert_eq!(layer.path_count(), 11);
        let src = p.preview_source(0, "ROUTE_TOOL_PATH").unwrap();
        assert_eq!(src.path_count(), 1);
        assert_eq!(src.closed, vec![1]);
        // Arcs travel unflattened: the rounded outline is 8 vertices, 4 of them bulged.
        assert_eq!(src.bulges.len() * 2, src.coords.len());
        assert_eq!(src.coords.len(), 16);
        assert_eq!(src.bulges.iter().filter(|b| **b != 0.0).count(), 4);

        let pdf = p.export_pdf().unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert_eq!(p.export_pdf_per_group().unwrap().len(), 2);

        // Second evaluation is fully cached.
        let e2 = p.evaluate();
        assert_eq!(e2.revision, 2);
        assert!(e2.layers.iter().all(|l| l.cached) && e2.passes.iter().all(|p| p.cached));
    }

    #[test]
    fn previews_of_empty_layers_and_passes_are_empty_buffers() {
        let mut p = Project::new();
        p.evaluate();
        let layer = p.preview_layer(&LayerId::new("wl-copper")).unwrap();
        assert_eq!(layer.path_count(), 0);
        assert_eq!(layer.starts, vec![0]);
        let pass = p.preview_pass(&PassId::new("pass-copper-fill")).unwrap();
        assert_eq!(pass.path_count(), 0);
        let jumps = p.preview_jumps(&GroupId::new("grp-copper")).unwrap();
        assert_eq!(jumps.path_count(), 0);
        assert!(p.heatmap(&GroupId::new("grp-copper")).is_none());
        assert!(p.preview_layer(&LayerId::new("nope")).is_none());
        assert!(p.preview_source(7, "x").is_none());
    }

    #[test]
    fn bad_profile_json_leaves_state_unchanged() {
        let mut p = Project::new();
        let before = p.profile_json();
        assert!(p.set_profile_json("{\"schemaVersion\": 42}").is_err());
        assert!(p.set_profile_json("nope").is_err());
        assert_eq!(p.profile_json(), before);
        assert!(p.set_profile_json(&before).is_ok());
    }

    #[test]
    fn export_without_evaluation_or_sources_fails_cleanly() {
        let mut p = Project::new();
        assert_eq!(p.export_pdf().unwrap_err(), ExportError::NothingToExport);
        let e = p.evaluate();
        assert!(e.has_errors, "required copper binding missing");
        assert!(
            e.bindings
                .iter()
                .any(|b| b.status == "missing" && b.required)
        );
        assert_eq!(p.export_pdf().unwrap_err(), ExportError::NothingToExport);
        assert!(p.add_source("bad.dxf", b"not a dxf").is_err());
        assert!(p.source_summaries().is_empty());
    }
}
