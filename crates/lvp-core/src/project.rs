//! The project: everything the geometry worker owns for one editing session, exposed in
//! UI-friendly form. Sources and the active profile go in; JSON summaries, flat preview
//! buffers and export bytes come out. Nothing here persists anything.

use cavalier_contours::polyline::PlineSource;
use serde::Serialize;

use crate::diag::{Diagnostic, Severity};
use crate::dxf::read::{DxfDocument, DxfReadError, Primitive, ReadOptions, read_bytes};
use crate::eval::{BindingStatus, Evaluation, Evaluator, Sources};
use crate::export::{self, ExportError, GroupFile};
use crate::geom::BBox;
use crate::model::*;
use crate::patterns::Geometry;
use crate::pline::{self, Pline};
use crate::profile::{self, ProfileError};
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
    pub format: &'static str,
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
    pub diagnostics: Vec<DiagnosticJson>,
    pub has_errors: bool,
}

/// Flattened polylines for drawing: path `i` is `coords[2*starts[i] .. 2*starts[i+1]]`.
/// `starts` has one more entry than there are paths. Coordinates are mm, `f32` (preview is
/// never the authoritative geometry).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewBuffer {
    pub coords: Vec<f32>,
    pub starts: Vec<u32>,
    pub closed: Vec<u8>,
}

impl PreviewBuffer {
    pub fn from_plines<'a, I: IntoIterator<Item = &'a Pline>>(
        plines: I,
        tolerance_mm: f64,
    ) -> Self {
        let mut b = PreviewBuffer {
            starts: vec![0],
            ..Default::default()
        };
        for p in plines {
            let Ok(pts) = pline::flatten(p, tolerance_mm) else {
                continue;
            };
            if pts.len() < 2 {
                continue;
            }
            for q in &pts {
                b.coords.push(q.x as f32);
                b.coords.push(q.y as f32);
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
    last: Option<Evaluation>,
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

    pub fn add_dxf(&mut self, name: &str, bytes: &[u8]) -> Result<SourceSummary, DxfReadError> {
        let doc = read_bytes(bytes, &self.read_options)?;
        let idx = self.sources.add_dxf(name, doc);
        Ok(self.source_summary(idx).expect("just added"))
    }

    pub fn replace_dxf(
        &mut self,
        index: usize,
        bytes: &[u8],
    ) -> Result<SourceSummary, DxfReadError> {
        let doc = read_bytes(bytes, &self.read_options)?;
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
            format: "dxf",
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

    pub fn document(&self, index: usize) -> Option<&DxfDocument> {
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
        let ev = self.evaluator.evaluate(&self.profile, &self.sources);
        self.revision += 1;
        self.last = Some(ev);
        self.summary().expect("just evaluated")
    }

    pub fn evaluation(&self) -> Option<&Evaluation> {
        self.last.as_ref()
    }

    pub fn summary(&self) -> Option<EvaluationSummary> {
        let ev = self.last.as_ref()?;
        let bindings = self
            .profile
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
        let layers = self
            .profile
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
                let name = self
                    .profile
                    .layer(&pr.layer)
                    .and_then(|l| l.passes.iter().find(|p| &p.id == id))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                let exported = self.profile.group(&pr.group).is_some_and(|g| g.export);
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
                }
            })
            .collect();
        Some(EvaluationSummary {
            revision: self.revision,
            bounds_mm: bbox_json(all_bounds),
            bindings,
            unbound_layers: ev.unbound_layers.clone(),
            layers,
            passes,
            diagnostics: diags(&ev.diagnostics),
            has_errors: ev.has_errors(),
        })
    }

    pub fn preview_source(
        &self,
        file: usize,
        layer: &str,
        tolerance_mm: f64,
    ) -> Option<PreviewBuffer> {
        let l = self.sources.files.get(file)?.doc.layer(layer)?;
        let plines: Vec<&Pline> = l
            .primitives
            .iter()
            .flat_map(|p| match p {
                Primitive::Path { pline, .. } => vec![pline],
                Primitive::Hatch { loops, .. } => loops.iter().collect(),
            })
            .collect();
        Some(PreviewBuffer::from_plines(plines, tolerance_mm))
    }

    pub fn preview_layer(&self, id: &LayerId, tolerance_mm: f64) -> Option<PreviewBuffer> {
        let r = self.last.as_ref()?.layers.get(id)?;
        Some(match &r.geometry {
            Geometry::Region(reg) => PreviewBuffer::from_plines(reg.loops.iter(), tolerance_mm),
            Geometry::Linework(ps) => PreviewBuffer::from_plines(ps.iter(), tolerance_mm),
        })
    }

    pub fn preview_pass(&self, id: &PassId, tolerance_mm: f64) -> Option<PreviewBuffer> {
        let pr = self.last.as_ref()?.pass(id)?;
        Some(PreviewBuffer::from_plines(pr.paths.iter(), tolerance_mm))
    }

    /// Hypothetical straight beam-off jumps between consecutive paths of a pass, in planner
    /// order. Preview only; never exported.
    pub fn preview_jumps(&self, id: &PassId) -> Option<PreviewBuffer> {
        let pr = self.last.as_ref()?.pass(id)?;
        let mut b = PreviewBuffer {
            starts: vec![0],
            ..Default::default()
        };
        for w in pr.paths.windows(2) {
            let from = if w[0].is_closed() {
                w[0].at(0)
            } else {
                w[0].last()?
            };
            let to = w[1].at(0);
            b.coords
                .extend([from.x as f32, from.y as f32, to.x as f32, to.y as f32]);
            b.starts.push((b.coords.len() / 2) as u32);
            b.closed.push(0);
        }
        Some(b)
    }

    pub fn export_pdf(&self) -> Result<Vec<u8>, ExportError> {
        let ev = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_pdf(&self.profile, ev).map(|(bytes, _)| bytes)
    }

    pub fn export_pdf_per_group(&self) -> Result<Vec<GroupFile>, ExportError> {
        let ev = self.last.as_ref().ok_or(ExportError::NothingToExport)?;
        export::export_pdf_per_group(&self.profile, ev).map(|(files, _)| files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let s = p.add_dxf("board-islands.dxf", &board()).unwrap();
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
        assert_eq!(e.layers.len(), 2);
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

        let buf = p.preview_pass(&fill.id, 0.01).unwrap();
        assert_eq!(buf.path_count(), fill.paths);
        assert_eq!(buf.starts.len(), fill.paths + 1);
        assert!(buf.starts.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(*buf.starts.last().unwrap() as usize * 2, buf.coords.len());
        let jumps = p.preview_jumps(&fill.id).unwrap();
        assert_eq!(jumps.path_count(), fill.paths - 1);
        let layer = p.preview_layer(&LayerId::new("wl-copper"), 0.01).unwrap();
        assert_eq!(layer.path_count(), 11);
        let src = p.preview_source(0, "ROUTE_TOOL_PATH", 0.01).unwrap();
        assert_eq!(src.path_count(), 1);
        assert_eq!(src.closed, vec![1]);

        let pdf = p.export_pdf().unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert_eq!(p.export_pdf_per_group().unwrap().len(), 2);

        // Second evaluation is fully cached.
        let e2 = p.evaluate();
        assert_eq!(e2.revision, 2);
        assert!(e2.layers.iter().all(|l| l.cached) && e2.passes.iter().all(|p| p.cached));
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
        assert!(p.add_dxf("bad.dxf", b"not a dxf").is_err());
        assert!(p.source_summaries().is_empty());
    }
}
