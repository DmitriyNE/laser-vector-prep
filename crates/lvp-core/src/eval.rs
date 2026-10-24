//! Evaluation of a profile against imported sources: binding resolution, the working-layer
//! DAG (dependency order, cycle rejection), geometry composition, pattern passes, and a
//! fingerprint cache so an upstream change recomputes only what depends on it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use cavalier_contours::polyline::PlineSource;

use crate::diag::{Diagnostic, Severity};
use crate::dxf::read::{DxfDocument, Primitive};
use crate::model::*;
use crate::patterns::{self, Geometry};
use crate::pline::Pline;
use crate::profile;
use crate::region::{self, Region};

/// One imported file.
#[derive(Clone, Debug)]
pub struct LoadedSource {
    pub name: String,
    pub format: SourceFormat,
    pub doc: DxfDocument,
    /// Changes whenever the file's content changes; part of every dependent fingerprint.
    pub revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub files: Vec<LoadedSource>,
    next_revision: u64,
}

impl Sources {
    pub fn add_dxf(&mut self, name: impl Into<String>, doc: DxfDocument) -> usize {
        self.next_revision += 1;
        self.files.push(LoadedSource {
            name: name.into(),
            format: SourceFormat::Dxf,
            doc,
            revision: self.next_revision,
        });
        self.files.len() - 1
    }

    /// Replace a file's document (same slot, new revision).
    pub fn replace(&mut self, index: usize, doc: DxfDocument) {
        self.next_revision += 1;
        if let Some(f) = self.files.get_mut(index) {
            f.doc = doc;
            f.revision = self.next_revision;
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.files.len() {
            self.files.remove(index);
        }
    }

    /// All `(file name, layer name)` pairs.
    pub fn layer_names(&self) -> Vec<(String, String)> {
        self.files
            .iter()
            .flat_map(|f| {
                f.doc
                    .layers
                    .iter()
                    .map(move |l| (f.name.clone(), l.name.clone()))
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingStatus {
    /// Exactly one `(file index, layer name)`.
    Matched(usize, String),
    Missing,
    /// Several candidates; needs an explicit file mapping.
    Ambiguous(Vec<(usize, String)>),
}

/// Resolve every binding; also report source layers no binding claims.
pub fn resolve_bindings(
    profile: &Profile,
    sources: &Sources,
) -> (BTreeMap<SourceId, BindingStatus>, Vec<(String, String)>) {
    let mut out = BTreeMap::new();
    let mut claimed: HashSet<(usize, String)> = HashSet::new();
    for b in &profile.source_bindings {
        let mut status = BindingStatus::Missing;
        for rule in &b.rules {
            let mut hits: Vec<(usize, String)> = Vec::new();
            for (i, f) in sources.files.iter().enumerate() {
                for l in &f.doc.layers {
                    let hit = match rule {
                        MatchRule::LayerName { format, name } => {
                            format.is_none_or(|fm| fm == f.format) && &l.name == name
                        }
                        MatchRule::FileLayer { file, name } => &f.name == file && &l.name == name,
                    };
                    if hit {
                        hits.push((i, l.name.clone()));
                    }
                }
            }
            match hits.len() {
                0 => continue,
                1 => {
                    let (i, name) = hits.remove(0);
                    claimed.insert((i, name.clone()));
                    status = BindingStatus::Matched(i, name);
                }
                _ => status = BindingStatus::Ambiguous(hits),
            }
            break;
        }
        out.insert(b.id.clone(), status);
    }
    let mut unbound = Vec::new();
    for (i, f) in sources.files.iter().enumerate() {
        for l in &f.doc.layers {
            if !l.primitives.is_empty() && !claimed.contains(&(i, l.name.clone())) {
                unbound.push((f.name.clone(), l.name.clone()));
            }
        }
    }
    (out, unbound)
}

#[derive(Clone, Debug)]
pub struct LayerResult {
    pub geometry: Geometry,
    pub diagnostics: Vec<Diagnostic>,
    pub fingerprint: u64,
    pub cached: bool,
}

#[derive(Clone, Debug)]
pub struct PassResult {
    pub layer: LayerId,
    pub group: GroupId,
    pub paths: Vec<Pline>,
    pub diagnostics: Vec<Diagnostic>,
    pub cached: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Evaluation {
    /// Dependency order of the layers that evaluated (cyclic ones are absent).
    pub order: Vec<LayerId>,
    pub layers: BTreeMap<LayerId, LayerResult>,
    /// In profile order (layer order, then pass order).
    pub passes: Vec<(PassId, PassResult)>,
    pub bindings: BTreeMap<SourceId, BindingStatus>,
    /// Imported layers with geometry that no binding uses.
    pub unbound_layers: Vec<(String, String)>,
    /// Profile-level diagnostics (validation, cycles, bindings).
    pub diagnostics: Vec<Diagnostic>,
}

impl Evaluation {
    pub fn pass(&self, id: &PassId) -> Option<&PassResult> {
        self.passes.iter().find(|(p, _)| p == id).map(|(_, r)| r)
    }

    /// Every diagnostic from every level, with layer/pass context.
    pub fn all_diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = self.diagnostics.clone();
        for (id, l) in &self.layers {
            out.extend(
                l.diagnostics
                    .iter()
                    .cloned()
                    .map(|d| d.with_context(format!("layer {id}"))),
            );
        }
        for (id, p) in &self.passes {
            out.extend(
                p.diagnostics
                    .iter()
                    .cloned()
                    .map(|d| d.with_context(format!("pass {id}"))),
            );
        }
        out
    }

    pub fn has_errors(&self) -> bool {
        self.all_diagnostics()
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Holds caches between evaluations of the same project.
#[derive(Default)]
pub struct Evaluator {
    layer_cache: HashMap<LayerId, (u64, Geometry, Vec<Diagnostic>)>,
    pass_cache: HashMap<PassId, (u64, Vec<Pline>, Vec<Diagnostic>)>,
}

/// Kahn's algorithm over layer→layer edges; returns `(order, cyclic)`.
fn topo_order(profile: &Profile) -> (Vec<LayerId>, Vec<LayerId>) {
    let ids: Vec<&LayerId> = profile.working_layers.iter().map(|l| &l.id).collect();
    let known: HashSet<&LayerId> = ids.iter().copied().collect();
    let mut indegree: HashMap<&LayerId, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut dependents: HashMap<&LayerId, Vec<&LayerId>> = HashMap::new();
    for l in &profile.working_layers {
        for op in &l.inputs {
            if let Operand::Layer(dep) = op
                && known.contains(dep)
            {
                *indegree.get_mut(&l.id).unwrap() += 1;
                dependents.entry(dep).or_default().push(&l.id);
            }
        }
    }
    let mut ready: Vec<&LayerId> = ids.iter().copied().filter(|id| indegree[id] == 0).collect();
    let mut order = Vec::new();
    while let Some(id) = ready.first().copied() {
        ready.remove(0);
        order.push(id.clone());
        if let Some(ds) = dependents.get(id) {
            for d in ds {
                let e = indegree.get_mut(d).unwrap();
                *e -= 1;
                if *e == 0 {
                    // Keep profile order among newly ready nodes for determinism.
                    let pos = ids.iter().position(|x| x == d).unwrap();
                    let insert_at = ready
                        .iter()
                        .position(|r| ids.iter().position(|x| x == r).unwrap() > pos)
                        .unwrap_or(ready.len());
                    ready.insert(insert_at, d);
                }
            }
        }
    }
    let cyclic = ids
        .iter()
        .filter(|id| !order.contains(id))
        .map(|id| (*id).clone())
        .collect();
    (order, cyclic)
}

fn hash_json<T: serde::Serialize>(h: &mut DefaultHasher, v: &T) {
    serde_json::to_string(v).unwrap_or_default().hash(h);
}

impl Evaluator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.layer_cache.clear();
        self.pass_cache.clear();
    }

    pub fn evaluate(&mut self, profile: &Profile, sources: &Sources) -> Evaluation {
        let mut ev = Evaluation {
            diagnostics: profile::validate(profile),
            ..Default::default()
        };
        let (bindings, unbound) = resolve_bindings(profile, sources);
        for b in &profile.source_bindings {
            match &bindings[&b.id] {
                BindingStatus::Missing if b.required => ev.diagnostics.push(
                    Diagnostic::error(
                        "eval.binding_missing",
                        format!("required source {:?} matched no imported layer", b.name),
                    )
                    .with_context(b.id.to_string()),
                ),
                BindingStatus::Missing => ev.diagnostics.push(
                    Diagnostic::info(
                        "eval.binding_missing",
                        format!("optional source {:?} matched no imported layer", b.name),
                    )
                    .with_context(b.id.to_string()),
                ),
                BindingStatus::Ambiguous(c) => ev.diagnostics.push(
                    Diagnostic::error(
                        "eval.binding_ambiguous",
                        format!(
                            "source {:?} matches {} layers; map it to one file explicitly",
                            b.name,
                            c.len()
                        ),
                    )
                    .with_context(b.id.to_string()),
                ),
                BindingStatus::Matched(..) => {}
            }
        }
        let (order, cyclic) = topo_order(profile);
        for id in &cyclic {
            ev.diagnostics.push(
                Diagnostic::error("eval.cycle", "working layer is part of a dependency cycle")
                    .with_context(id.to_string()),
            );
        }

        for id in &order {
            let layer = profile.layer(id).expect("ordered layer exists");
            let fp = self.layer_fingerprint(layer, sources, &bindings, &ev.layers);
            if let Some((cfp, geom, diags)) = self.layer_cache.get(id)
                && *cfp == fp
            {
                ev.layers.insert(
                    id.clone(),
                    LayerResult {
                        geometry: geom.clone(),
                        diagnostics: diags.clone(),
                        fingerprint: fp,
                        cached: true,
                    },
                );
                continue;
            }
            let (geometry, diagnostics) =
                compute_layer(layer, profile, sources, &bindings, &ev.layers);
            self.layer_cache
                .insert(id.clone(), (fp, geometry.clone(), diagnostics.clone()));
            ev.layers.insert(
                id.clone(),
                LayerResult {
                    geometry,
                    diagnostics,
                    fingerprint: fp,
                    cached: false,
                },
            );
        }
        ev.order = order;

        for layer in profile
            .working_layers
            .iter()
            .filter(|l| l.enabled && !l.reference_only)
        {
            let Some(lr) = ev.layers.get(&layer.id) else {
                continue;
            };
            for pass in layer.passes.iter().filter(|p| p.enabled) {
                let mut h = DefaultHasher::new();
                hash_json(&mut h, &pass.pattern);
                lr.fingerprint.hash(&mut h);
                let fp = h.finish();
                let (paths, diagnostics, cached) = match self.pass_cache.get(&pass.id) {
                    Some((cfp, paths, diags)) if *cfp == fp => (paths.clone(), diags.clone(), true),
                    _ => {
                        let out = patterns::generate(&pass.pattern, &lr.geometry);
                        self.pass_cache.insert(
                            pass.id.clone(),
                            (fp, out.paths.clone(), out.diagnostics.clone()),
                        );
                        (out.paths, out.diagnostics, false)
                    }
                };
                ev.passes.push((
                    pass.id.clone(),
                    PassResult {
                        layer: layer.id.clone(),
                        group: pass.group.clone(),
                        paths,
                        diagnostics,
                        cached,
                    },
                ));
            }
        }
        ev.bindings = bindings;
        ev.unbound_layers = unbound;
        ev
    }

    fn layer_fingerprint(
        &self,
        layer: &WorkingLayer,
        sources: &Sources,
        bindings: &BTreeMap<SourceId, BindingStatus>,
        done: &BTreeMap<LayerId, LayerResult>,
    ) -> u64 {
        let mut h = DefaultHasher::new();
        let config = WorkingLayer {
            passes: Vec::new(),
            ..layer.clone()
        };
        hash_json(&mut h, &config);
        for op in &layer.inputs {
            match op {
                Operand::Source(sid) => match bindings.get(sid) {
                    Some(BindingStatus::Matched(i, name)) => {
                        sources.files[*i].revision.hash(&mut h);
                        name.hash(&mut h);
                    }
                    other => format!("{other:?}").hash(&mut h),
                },
                Operand::Layer(lid) => match done.get(lid) {
                    Some(r) => r.fingerprint.hash(&mut h),
                    None => "unresolved".hash(&mut h),
                },
            }
        }
        h.finish()
    }
}

/// Geometry of one imported layer under a working layer's interpretation.
fn source_geometry(
    kind: GeometryKind,
    interp: &Interpretation,
    prims: &[Primitive],
    diags: &mut Vec<Diagnostic>,
) -> Geometry {
    match kind {
        GeometryKind::Linework => {
            let mut paths = Vec::new();
            for p in prims {
                match p {
                    Primitive::Path { pline, .. } => paths.push(pline.clone()),
                    Primitive::Hatch { loops, .. } => paths.extend(loops.iter().cloned()),
                }
            }
            Geometry::Linework(paths)
        }
        GeometryKind::Region => {
            let mut closed_loops: Vec<Pline> = Vec::new();
            let mut parts: Vec<Region> = Vec::new();
            let mut open_without_width = 0usize;
            for p in prims {
                match p {
                    Primitive::Path { pline, width } => {
                        let w = match interp.stroke_width {
                            StrokeWidth::Fixed { mm } => Some(mm),
                            StrokeWidth::Original => *width,
                            StrokeWidth::None => None,
                        };
                        match (pline.is_closed(), w) {
                            (_, Some(w)) if w > 0.0 => parts.push(region::stroke(pline, w)),
                            (true, _) => closed_loops.push(pline.clone()),
                            (false, _) => open_without_width += 1,
                        }
                    }
                    Primitive::Hatch { loops, rule, .. } => {
                        parts.push(Region::from_loops(loops.iter().cloned(), *rule))
                    }
                }
            }
            if open_without_width > 0 {
                diags.push(Diagnostic::error(
                    "eval.open_linework_in_region",
                    format!("{open_without_width} open path(s) cannot bound a region; set a stroke width or use a linework layer"),
                ));
            }
            let mut region = Region::from_loops(closed_loops, interp.loop_rule.into());
            for part in parts {
                region = region.union(&part);
            }
            Geometry::Region(region)
        }
    }
}

fn to_region(g: &Geometry, interp: &Interpretation, diags: &mut Vec<Diagnostic>) -> Region {
    match g {
        Geometry::Region(r) => r.clone(),
        Geometry::Linework(paths) => {
            let mut closed = Vec::new();
            let mut parts = Vec::new();
            let mut open = 0usize;
            for p in paths {
                match (p.is_closed(), interp.stroke_width) {
                    (_, StrokeWidth::Fixed { mm }) if mm > 0.0 => parts.push(region::stroke(p, mm)),
                    (true, _) => closed.push(p.clone()),
                    (false, _) => open += 1,
                }
            }
            if open > 0 {
                diags.push(Diagnostic::error(
                    "eval.open_linework_in_region",
                    format!("{open} open path(s) from a linework input cannot bound a region; set a fixed stroke width"),
                ));
            }
            let mut r = Region::from_loops(closed, interp.loop_rule.into());
            for part in parts {
                r = r.union(&part);
            }
            r
        }
    }
}

fn compute_layer(
    layer: &WorkingLayer,
    profile: &Profile,
    sources: &Sources,
    bindings: &BTreeMap<SourceId, BindingStatus>,
    done: &BTreeMap<LayerId, LayerResult>,
) -> (Geometry, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut inputs: Vec<Geometry> = Vec::new();
    for op in &layer.inputs {
        match op {
            Operand::Source(sid) => match bindings.get(sid) {
                Some(BindingStatus::Matched(i, name)) => {
                    let prims = sources.files[*i]
                        .doc
                        .layer(name)
                        .map(|l| l.primitives.as_slice())
                        .unwrap_or(&[]);
                    inputs.push(source_geometry(
                        layer.kind,
                        &layer.interpretation,
                        prims,
                        &mut diags,
                    ));
                }
                _ => {
                    let required = profile.binding(sid).is_some_and(|b| b.required);
                    if required {
                        diags.push(Diagnostic::error(
                            "eval.input_unavailable",
                            format!("source {sid} is not bound; layer output is blocked"),
                        ));
                        return (empty_geometry(layer.kind), diags);
                    }
                    inputs.push(empty_geometry(layer.kind));
                }
            },
            Operand::Layer(lid) => match done.get(lid) {
                Some(r) => inputs.push(r.geometry.clone()),
                None => {
                    diags.push(Diagnostic::error(
                        "eval.input_unavailable",
                        format!("input layer {lid} did not evaluate (cycle or missing)"),
                    ));
                    return (empty_geometry(layer.kind), diags);
                }
            },
        }
    }
    let geometry = match layer.kind {
        GeometryKind::Linework => {
            Geometry::Linework(inputs.iter().flat_map(|g| g.paths()).collect())
        }
        GeometryKind::Region => {
            let regions: Vec<Region> = inputs
                .iter()
                .map(|g| to_region(g, &layer.interpretation, &mut diags))
                .collect();
            let mut it = regions.into_iter();
            let Some(first) = it.next() else {
                return (Geometry::Region(Region::empty()), diags);
            };
            let result = match layer.op {
                LayerOp::Union => it.fold(first, |acc, r| acc.union(&r)),
                LayerOp::Intersection => it.fold(first, |acc, r| acc.intersection(&r)),
                LayerOp::Xor => it.fold(first, |acc, r| acc.xor(&r)),
                LayerOp::Subtraction => {
                    let rest = it.fold(Region::empty(), |acc, r| acc.union(&r));
                    first.difference(&rest)
                }
            };
            Geometry::Region(result)
        }
    };
    if geometry.is_empty() {
        diags.push(Diagnostic::warning(
            "eval.empty_layer",
            "layer evaluates to no geometry",
        ));
    }
    (geometry, diags)
}

fn empty_geometry(kind: GeometryKind) -> Geometry {
    match kind {
        GeometryKind::Region => Geometry::Region(Region::empty()),
        GeometryKind::Linework => Geometry::Linework(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dxf::read::{ReadOptions, read_str};
    use crate::dxf::write::{Document, Entity};
    use crate::geom::Point;
    use crate::profile::pcb_positive_ldi;
    use crate::units::Unit;

    fn board() -> DxfDocument {
        let mut d = Document::new(Unit::Millimetre);
        d.layer("TOP_LAYER", 5).layer("ROUTE_TOOL_PATH", 1);
        d.add(
            "TOP_LAYER",
            Entity::rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0)),
        );
        d.add(
            "TOP_LAYER",
            Entity::rect(Point::new(5.0, 5.0), Point::new(15.0, 15.0)),
        );
        d.add(
            "TOP_LAYER",
            Entity::Circle {
                center: Point::new(30.0, 5.0),
                radius: 2.0,
            },
        );
        d.add(
            "TOP_LAYER",
            Entity::Circle {
                center: Point::new(30.0, 5.0),
                radius: 1.0,
            },
        );
        d.add(
            "ROUTE_TOOL_PATH",
            Entity::rect(Point::new(-2.0, -2.0), Point::new(40.0, 20.0)),
        );
        read_str(&d.to_dxf(), &ReadOptions::default()).unwrap()
    }

    #[test]
    fn preset_evaluates_fixture_like_board() {
        let mut sources = Sources::default();
        sources.add_dxf("board.dxf", board());
        let profile = pcb_positive_ldi();
        let mut ev = Evaluator::new();
        let e = ev.evaluate(&profile, &sources);
        assert!(!e.has_errors(), "{:?}", e.all_diagnostics());
        assert_eq!(
            e.bindings[&SourceId::new("src-copper")],
            BindingStatus::Matched(0, "TOP_LAYER".into())
        );
        let copper = &e.layers[&LayerId::new("wl-copper")];
        let Geometry::Region(r) = &copper.geometry else {
            panic!()
        };
        // Two overlapping squares merged (175) plus an annulus (pi * (4 - 1)).
        assert!(
            (r.area() - (175.0 + std::f64::consts::PI * 3.0)).abs() < 1e-6,
            "area {}",
            r.area()
        );
        assert_eq!(r.holes().count(), 1);
        let fill = e.pass(&PassId::new("pass-copper-fill")).unwrap();
        assert!(!fill.paths.is_empty());
        // Rings are closed; gap-fill centrelines (open) are legitimate too.
        assert!(fill.paths.iter().filter(|p| p.is_closed()).count() > 10);
        let outline = e.pass(&PassId::new("pass-outline")).unwrap();
        assert_eq!(outline.paths.len(), 1);
        assert!(e.unbound_layers.is_empty());
    }

    #[test]
    fn cache_invalidates_only_dependents() {
        let mut sources = Sources::default();
        sources.add_dxf("board.dxf", board());
        let mut profile = pcb_positive_ldi();
        let mut ev = Evaluator::new();
        let first = ev.evaluate(&profile, &sources);
        assert!(first.layers.values().all(|l| !l.cached));
        let second = ev.evaluate(&profile, &sources);
        assert!(second.layers.values().all(|l| l.cached));
        assert!(second.passes.iter().all(|(_, p)| p.cached));

        // Changing a pass parameter recomputes that pass only.
        if let Pattern::ContourFill { pitch_mm, .. } =
            &mut profile.working_layers[0].passes[0].pattern
        {
            *pitch_mm = 0.5;
        }
        let third = ev.evaluate(&profile, &sources);
        assert!(third.layers.values().all(|l| l.cached));
        assert!(!third.pass(&PassId::new("pass-copper-fill")).unwrap().cached);
        assert!(third.pass(&PassId::new("pass-outline")).unwrap().cached);

        // Replacing the source file invalidates layers depending on it.
        sources.replace(0, board());
        let fourth = ev.evaluate(&profile, &sources);
        assert!(fourth.layers.values().all(|l| !l.cached));

        // Reordering layers in the profile changes nothing.
        profile.working_layers.swap(0, 1);
        let fifth = ev.evaluate(&profile, &sources);
        assert!(fifth.layers.values().all(|l| l.cached));
    }

    #[test]
    fn cycles_are_rejected_not_guessed() {
        let mut profile = pcb_positive_ldi();
        let a = LayerId::new("wl-a");
        let b = LayerId::new("wl-b");
        let mk = |id: &LayerId, dep: &LayerId| WorkingLayer {
            id: id.clone(),
            name: id.to_string(),
            kind: GeometryKind::Region,
            inputs: vec![Operand::Layer(dep.clone())],
            op: LayerOp::Union,
            interpretation: Interpretation::default(),
            passes: vec![],
            enabled: true,
            reference_only: false,
        };
        profile.working_layers.push(mk(&a, &b));
        profile.working_layers.push(mk(&b, &a));
        let mut sources = Sources::default();
        sources.add_dxf("board.dxf", board());
        let e = Evaluator::new().evaluate(&profile, &sources);
        let cyc: Vec<&Diagnostic> = e
            .diagnostics
            .iter()
            .filter(|d| d.code == "eval.cycle")
            .collect();
        assert_eq!(cyc.len(), 2);
        assert!(!e.layers.contains_key(&a) && !e.layers.contains_key(&b));
        assert!(
            e.layers.contains_key(&LayerId::new("wl-copper")),
            "healthy layers still evaluate"
        );
    }

    #[test]
    fn missing_required_binding_blocks_output() {
        let mut d = Document::new(Unit::Millimetre);
        d.layer("OTHER", 7);
        d.add(
            "OTHER",
            Entity::rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0)),
        );
        let mut sources = Sources::default();
        sources.add_dxf(
            "x.dxf",
            read_str(&d.to_dxf(), &ReadOptions::default()).unwrap(),
        );
        let e = Evaluator::new().evaluate(&pcb_positive_ldi(), &sources);
        assert!(
            e.diagnostics
                .iter()
                .any(|d| d.code == "eval.binding_missing" && d.severity == Severity::Error)
        );
        assert!(
            e.pass(&PassId::new("pass-copper-fill"))
                .unwrap()
                .paths
                .is_empty()
        );
        assert_eq!(
            e.unbound_layers,
            vec![("x.dxf".to_string(), "OTHER".to_string())]
        );
    }

    #[test]
    fn ambiguous_bindings_need_explicit_mapping() {
        let mut sources = Sources::default();
        sources.add_dxf("a.dxf", board());
        sources.add_dxf("b.dxf", board());
        let mut profile = pcb_positive_ldi();
        let e = Evaluator::new().evaluate(&profile, &sources);
        assert!(
            e.diagnostics
                .iter()
                .any(|d| d.code == "eval.binding_ambiguous")
        );
        profile.source_bindings[0].rules.insert(
            0,
            MatchRule::FileLayer {
                file: "b.dxf".into(),
                name: "TOP_LAYER".into(),
            },
        );
        let e = Evaluator::new().evaluate(&profile, &sources);
        assert_eq!(
            e.bindings[&SourceId::new("src-copper")],
            BindingStatus::Matched(1, "TOP_LAYER".into())
        );
    }

    #[test]
    fn open_linework_never_becomes_a_region_implicitly() {
        let mut d = Document::new(Unit::Millimetre);
        d.layer("TOP_LAYER", 5);
        d.add(
            "TOP_LAYER",
            Entity::Line {
                start: Point::new(0.0, 0.0),
                end: Point::new(10.0, 0.0),
            },
        );
        let mut sources = Sources::default();
        sources.add_dxf(
            "x.dxf",
            read_str(&d.to_dxf(), &ReadOptions::default()).unwrap(),
        );
        let mut profile = pcb_positive_ldi();
        let e = Evaluator::new().evaluate(&profile, &sources);
        let copper = &e.layers[&LayerId::new("wl-copper")];
        assert!(
            copper
                .diagnostics
                .iter()
                .any(|d| d.code == "eval.open_linework_in_region")
        );
        assert!(copper.geometry.is_empty());
        // An explicit stroke width turns it into a region.
        profile.working_layers[0].interpretation.stroke_width = StrokeWidth::Fixed { mm: 1.0 };
        let e = Evaluator::new().evaluate(&profile, &sources);
        let Geometry::Region(r) = &e.layers[&LayerId::new("wl-copper")].geometry else {
            panic!()
        };
        assert!((r.area() - (10.0 + std::f64::consts::PI * 0.25)).abs() < 1e-9);
    }

    #[test]
    fn subtraction_and_intersection_follow_dependencies_not_order() {
        let mut d = Document::new(Unit::Millimetre);
        d.layer("A", 1).layer("B", 2);
        d.add(
            "A",
            Entity::rect(Point::new(0.0, 0.0), Point::new(10.0, 10.0)),
        );
        d.add(
            "B",
            Entity::rect(Point::new(5.0, 0.0), Point::new(15.0, 10.0)),
        );
        let mut sources = Sources::default();
        sources.add_dxf(
            "x.dxf",
            read_str(&d.to_dxf(), &ReadOptions::default()).unwrap(),
        );
        let mut profile = Profile::empty("t", "t");
        for (id, name) in [("src-a", "A"), ("src-b", "B")] {
            profile.source_bindings.push(SourceBinding {
                id: SourceId::new(id),
                name: name.into(),
                required: true,
                rules: vec![MatchRule::LayerName {
                    format: None,
                    name: name.into(),
                }],
            });
        }
        let region = |id: &str, inputs: Vec<Operand>, op: LayerOp| WorkingLayer {
            id: LayerId::new(id),
            name: id.into(),
            kind: GeometryKind::Region,
            inputs,
            op,
            interpretation: Interpretation::default(),
            passes: vec![],
            enabled: true,
            reference_only: false,
        };
        // Dependent layer listed before its input on purpose.
        profile.working_layers.push(region(
            "wl-diff",
            vec![
                Operand::Layer(LayerId::new("wl-a")),
                Operand::Source(SourceId::new("src-b")),
            ],
            LayerOp::Subtraction,
        ));
        profile.working_layers.push(region(
            "wl-a",
            vec![Operand::Source(SourceId::new("src-a"))],
            LayerOp::Union,
        ));
        profile.working_layers.push(region(
            "wl-int",
            vec![
                Operand::Source(SourceId::new("src-a")),
                Operand::Source(SourceId::new("src-b")),
            ],
            LayerOp::Intersection,
        ));
        let e = Evaluator::new().evaluate(&profile, &sources);
        assert_eq!(
            e.order,
            vec![
                LayerId::new("wl-a"),
                LayerId::new("wl-diff"),
                LayerId::new("wl-int")
            ]
        );
        let Geometry::Region(diff) = &e.layers[&LayerId::new("wl-diff")].geometry else {
            panic!()
        };
        let Geometry::Region(int) = &e.layers[&LayerId::new("wl-int")].geometry else {
            panic!()
        };
        assert!((diff.area() - 50.0).abs() < 1e-9);
        assert!((int.area() - 50.0).abs() < 1e-9);
    }
}
