//! Evaluation of a profile against imported sources: binding resolution, the working-layer
//! DAG (dependency order, cycle rejection), geometry composition, pattern passes, and a
//! fingerprint cache so an upstream change recomputes only what depends on it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use cavalier_contours::polyline::PlineSource;

use crate::cancel::{self, Cancelled};
use crate::diag::{Diagnostic, Severity};
use crate::geom::{self, BBox, Point};
use crate::model::*;
use crate::order::{self, GroupPlan, PassPaths};
use crate::patterns::{self, Geometry};
use crate::pline::{self, Pline};
use crate::profile;
use crate::region::{self, LoopRule, Region};
use crate::source::{Primitive, SourceDocument, glob_match};

/// One imported file.
#[derive(Clone, Debug)]
pub struct LoadedSource {
    pub name: String,
    pub format: SourceFormat,
    pub doc: SourceDocument,
    /// Changes whenever the file's content changes; part of every dependent fingerprint.
    pub revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub files: Vec<LoadedSource>,
    next_revision: u64,
}

impl Sources {
    pub fn add(&mut self, name: impl Into<String>, doc: SourceDocument) -> usize {
        self.next_revision += 1;
        self.files.push(LoadedSource {
            name: name.into(),
            format: doc.format,
            doc,
            revision: self.next_revision,
        });
        self.files.len() - 1
    }

    /// Replace a file's document (same slot, new revision).
    pub fn replace(&mut self, index: usize, doc: SourceDocument) {
        self.next_revision += 1;
        if let Some(f) = self.files.get_mut(index) {
            f.format = doc.format;
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
                        MatchRule::FileName { format, pattern } => {
                            format.is_none_or(|fm| fm == f.format)
                                && !l.primitives.is_empty()
                                && glob_match(pattern, &f.name)
                        }
                        MatchRule::Function { pattern } => l
                            .function
                            .as_deref()
                            .is_some_and(|fun| glob_match(pattern, fun)),
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
    /// Ordering constraints `(before, after)` over `paths` (cutting: holes first).
    pub precedence: Vec<(usize, usize)>,
    pub cached: bool,
    /// Coverage audit, when the pass fills a region and its group sets an audit width.
    pub audit: Option<patterns::Audit>,
}

/// Cached pattern output of one pass: fingerprint, paths, diagnostics, precedence.
type CachedPass = (u64, Vec<Pline>, Vec<Diagnostic>, Vec<(usize, usize)>);

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
    /// Path-order plan per output group, in profile order (spec §7).
    pub groups: Vec<(GroupId, GroupPlan)>,
}

impl Evaluation {
    pub fn pass(&self, id: &PassId) -> Option<&PassResult> {
        self.passes.iter().find(|(p, _)| p == id).map(|(_, r)| r)
    }

    pub fn group_plan(&self, id: &GroupId) -> Option<&GroupPlan> {
        self.groups.iter().find(|(g, _)| g == id).map(|(_, p)| p)
    }

    /// Paths of every pass, for resolving a plan's references.
    pub fn pass_paths(&self) -> BTreeMap<PassId, Vec<Pline>> {
        self.passes
            .iter()
            .map(|(id, r)| (id.clone(), r.paths.clone()))
            .collect()
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
    pass_cache: HashMap<PassId, CachedPass>,
    plan_cache: HashMap<GroupId, (u64, GroupPlan)>,
    audit_cache: HashMap<PassId, (u64, patterns::Audit)>,
}

/// Kahn's algorithm over layer→layer edges; returns `(order, cyclic)`.
fn topo_order(profile: &Profile) -> (Vec<LayerId>, Vec<LayerId>) {
    let ids: Vec<&LayerId> = profile.working_layers.iter().map(|l| &l.id).collect();
    let known: HashSet<&LayerId> = ids.iter().copied().collect();
    let mut indegree: HashMap<&LayerId, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut dependents: HashMap<&LayerId, Vec<&LayerId>> = HashMap::new();
    for l in &profile.working_layers {
        for op in l.inputs.iter().chain(&l.operands) {
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
        self.plan_cache.clear();
        self.audit_cache.clear();
    }

    /// Evaluate outside a cancellable computation (tests, tools): never cancelled.
    pub fn evaluate(&mut self, profile: &Profile, sources: &Sources) -> Evaluation {
        self.try_evaluate(profile, sources)
            .expect("nothing is cancelled outside a cancellable computation")
    }

    /// Evaluate, stopping early if the host cancels ([`crate::cancel`]). Results computed
    /// before the stop stay cached; a partial one is never cached.
    pub fn try_evaluate(
        &mut self,
        profile: &Profile,
        sources: &Sources,
    ) -> Result<Evaluation, Cancelled> {
        // Cancelled while still queued: do nothing, even if everything is cached.
        cancel::check()?;
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
        let passes = profile
            .working_layers
            .iter()
            .filter(|l| l.enabled && !l.reference_only)
            .map(|l| l.passes.iter().filter(|p| p.enabled).count())
            .sum::<usize>();
        cancel::steps((order.len() + passes + profile.output_groups.len()) as u32);

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
                cancel::next_step();
                continue;
            }
            cancel::check()?;
            let (geometry, diagnostics) =
                compute_layer(layer, profile, sources, &bindings, &ev.layers);
            cancel::check()?;
            cancel::next_step();
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
                let (paths, diagnostics, precedence, cached) = match self.pass_cache.get(&pass.id) {
                    Some((cfp, paths, diags, prec)) if *cfp == fp => {
                        (paths.clone(), diags.clone(), prec.clone(), true)
                    }
                    _ => {
                        cancel::check()?;
                        let out = patterns::generate(&pass.pattern, &lr.geometry);
                        cancel::check()?;
                        self.pass_cache.insert(
                            pass.id.clone(),
                            (
                                fp,
                                out.paths.clone(),
                                out.diagnostics.clone(),
                                out.precedence.clone(),
                            ),
                        );
                        (out.paths, out.diagnostics, out.precedence, false)
                    }
                };
                let audit_width = profile
                    .group(&pass.group)
                    .and_then(|g| g.audit_width_mm)
                    .filter(|w| *w > 0.0 && w.is_finite());
                let audit = match (&lr.geometry, audit_width) {
                    (Geometry::Region(region), Some(w)) => {
                        let mut h = DefaultHasher::new();
                        fp.hash(&mut h);
                        w.to_bits().hash(&mut h);
                        let afp = h.finish();
                        match self.audit_cache.get(&pass.id) {
                            Some((cfp, a)) if *cfp == afp => Some(a.clone()),
                            _ => {
                                cancel::check()?;
                                let a = patterns::audit(region, &paths, w);
                                cancel::check()?;
                                self.audit_cache.insert(pass.id.clone(), (afp, a.clone()));
                                Some(a)
                            }
                        }
                    }
                    _ => None,
                };
                cancel::next_step();
                ev.passes.push((
                    pass.id.clone(),
                    PassResult {
                        layer: layer.id.clone(),
                        group: pass.group.clone(),
                        paths,
                        diagnostics,
                        precedence,
                        cached,
                        audit,
                    },
                ));
            }
        }
        ev.bindings = bindings;
        ev.unbound_layers = unbound;
        self.plan_groups(profile, &mut ev)?;
        Ok(ev)
    }

    /// Plan the path order of every output group (spec §7), cached per group on the
    /// geometry of its passes, the settings and the protected regions.
    fn plan_groups(&mut self, profile: &Profile, ev: &mut Evaluation) -> Result<(), Cancelled> {
        let settings = &profile.optimizer;
        // Protected region: union of the configured layers' regions, with a fingerprint.
        let mut protected: Option<Region> = None;
        let mut protected_fp = DefaultHasher::new();
        for id in &settings.protected {
            match ev.layers.get(id) {
                Some(lr) => {
                    lr.fingerprint.hash(&mut protected_fp);
                    if let Geometry::Region(r) = &lr.geometry
                        && !r.is_empty()
                    {
                        protected = Some(match protected.take() {
                            Some(acc) => acc.union(r),
                            None => r.clone(),
                        });
                    }
                }
                None => {
                    ev.diagnostics.push(Diagnostic::warning(
                        "order.protected_unknown",
                        format!("protected layer {id} does not exist"),
                    ));
                }
            }
        }
        let protected_fp = protected_fp.finish();
        let mut plans = Vec::new();
        for group in &profile.output_groups {
            let passes: Vec<PassPaths<'_>> = ev
                .passes
                .iter()
                .filter(|(_, r)| r.group == group.id)
                .map(|(id, r)| PassPaths {
                    id: id.clone(),
                    paths: &r.paths,
                    precedence: &r.precedence,
                })
                .collect();
            let mode = group.order_mode.unwrap_or(settings.mode);
            let fp = order::fingerprint(&passes, mode, settings, protected_fp);
            let plan = match self.plan_cache.get(&group.id) {
                Some((cfp, plan)) if *cfp == fp => plan.clone(),
                _ => {
                    cancel::check()?;
                    let plan = order::plan(&passes, mode, settings, protected.as_ref());
                    cancel::check()?;
                    self.plan_cache.insert(group.id.clone(), (fp, plan.clone()));
                    plan
                }
            };
            cancel::next_step();
            plans.push((group.id.clone(), plan));
        }
        ev.groups = plans;
        Ok(())
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
        for op in layer.inputs.iter().chain(&layer.operands) {
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
        // An envelope's reference set is read like a region layer's inputs, except that open
        // widthless linework is fine for the point-based providers: keep it as linework.
        GeometryKind::Envelope => {
            let has_open_widthless = prims
                .iter()
                .any(|p| matches!(p, Primitive::Path { pline, width: None } if !pline.is_closed()))
                && !matches!(interp.stroke_width, StrokeWidth::Fixed { .. });
            if has_open_widthless {
                source_geometry(GeometryKind::Linework, interp, prims, diags)
            } else {
                source_geometry(GeometryKind::Region, interp, prims, diags)
            }
        }
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
    // Resolve one operand; `None` means the layer is blocked.
    let resolve = |op: &Operand, diags: &mut Vec<Diagnostic>| -> Option<Geometry> {
        match op {
            Operand::Source(sid) => match bindings.get(sid) {
                Some(BindingStatus::Matched(i, name)) => {
                    let prims = sources.files[*i]
                        .doc
                        .layer(name)
                        .map(|l| l.primitives.as_slice())
                        .unwrap_or(&[]);
                    Some(source_geometry(
                        layer.kind,
                        &layer.interpretation,
                        prims,
                        diags,
                    ))
                }
                _ => {
                    if profile.binding(sid).is_some_and(|b| b.required) {
                        diags.push(Diagnostic::error(
                            "eval.input_unavailable",
                            format!("source {sid} is not bound; layer output is blocked"),
                        ));
                        None
                    } else {
                        Some(empty_geometry(layer.kind))
                    }
                }
            },
            Operand::Layer(lid) => match done.get(lid) {
                Some(r) => Some(r.geometry.clone()),
                None => {
                    diags.push(Diagnostic::error(
                        "eval.input_unavailable",
                        format!("input layer {lid} did not evaluate (cycle or missing)"),
                    ));
                    None
                }
            },
        }
    };
    let mut group_a: Vec<Geometry> = Vec::new();
    let mut group_b: Vec<Geometry> = Vec::new();
    for op in &layer.inputs {
        match resolve(op, &mut diags) {
            Some(g) => group_a.push(g),
            None => return (empty_geometry(layer.kind), diags),
        }
    }
    for op in &layer.operands {
        match resolve(op, &mut diags) {
            Some(g) => group_b.push(g),
            None => return (empty_geometry(layer.kind), diags),
        }
    }
    let geometry = match layer.kind {
        GeometryKind::Envelope => {
            let Some(provider) = &layer.envelope else {
                diags.push(Diagnostic::error(
                    "eval.envelope_provider",
                    "envelope layer has no provider",
                ));
                return (Geometry::Region(Region::empty()), diags);
            };
            Geometry::Region(envelope(
                provider,
                &group_a,
                &layer.interpretation,
                &mut diags,
            ))
        }
        // Linework only merges; validation rejects other ops.
        GeometryKind::Linework => Geometry::Linework(
            group_a
                .iter()
                .chain(&group_b)
                .flat_map(|g| g.paths())
                .collect(),
        ),
        GeometryKind::Region => {
            let union_of = |gs: &[Geometry], diags: &mut Vec<Diagnostic>| {
                gs.iter()
                    .map(|g| to_region(g, &layer.interpretation, diags))
                    .fold(Region::empty(), |acc, r| {
                        if acc.is_empty() { r } else { acc.union(&r) }
                    })
            };
            let a = union_of(&group_a, &mut diags);
            let b = union_of(&group_b, &mut diags);
            if layer.op == LayerOp::Subtraction && !b.is_empty() {
                // Spec: subject geometry outside the domain must be diagnosed, not guessed away.
                let outside = b.difference(&a);
                if !outside.is_empty() {
                    diags.push(Diagnostic::warning(
                        "eval.subject_outside_domain",
                        format!("{:.4} mm² of group B lies outside group A and is not part of the result", outside.area()),
                    ));
                }
            }
            Geometry::Region(match layer.op {
                LayerOp::Union => a.union(&b),
                LayerOp::Intersection => a.intersection(&b),
                LayerOp::Subtraction => a.difference(&b),
                LayerOp::Xor => a.xor(&b),
            })
        }
    };
    if geometry.is_empty() {
        diags.push(Diagnostic::warning(
            "eval.empty_layer",
            "layer evaluates to no geometry",
        ));
    }
    // Report an inconsistent boolean where it happens, not on every layer downstream.
    let inherited = layer.inputs.iter().chain(&layer.operands).any(|op| {
        matches!(op, Operand::Layer(lid)
            if matches!(done.get(lid), Some(LayerResult { geometry: Geometry::Region(r), .. }) if r.incomplete))
    });
    if matches!(&geometry, Geometry::Region(r) if r.incomplete) && !inherited {
        diags.push(Diagnostic::warning(
            "eval.incomplete_boolean",
            "part of this region's boundary could not be resolved and was left out: the input has boundaries within nanometres of each other that the geometry kernel cannot intersect consistently",
        ));
    }
    (geometry, diags)
}

/// Flattening tolerance for point-based envelope providers (box, circle, hull).
const ENVELOPE_POINT_TOLERANCE_MM: f64 = 0.001;

/// Every vertex of the reference geometry, curves flattened.
fn reference_points(refs: &[Geometry]) -> Vec<Point> {
    let mut pts = Vec::new();
    for g in refs {
        for p in g.paths() {
            if let Ok(flat) = pline::flatten(&p, ENVELOPE_POINT_TOLERANCE_MM) {
                pts.extend(flat);
            }
        }
    }
    pts
}

/// Bounded domain from a reference set. Empty references yield an empty region and an
/// error: an unbounded domain is never guessed.
fn envelope(
    provider: &EnvelopeProvider,
    refs: &[Geometry],
    interp: &Interpretation,
    diags: &mut Vec<Diagnostic>,
) -> Region {
    if refs.iter().all(Geometry::is_empty) {
        diags.push(Diagnostic::error(
            "eval.envelope_unbounded",
            "envelope has no reference geometry; a negative region without a bounded domain is invalid",
        ));
        return Region::empty();
    }
    let union_ref = |diags: &mut Vec<Diagnostic>| {
        refs.iter()
            .map(|g| to_region(g, interp, diags))
            .fold(Region::empty(), |acc, r| {
                if acc.is_empty() { r } else { acc.union(&r) }
            })
    };
    match provider {
        EnvelopeProvider::NamedBoundary => union_ref(diags),
        EnvelopeProvider::BoundingBox { margin_mm } => {
            let Some(b) = BBox::of_points(reference_points(refs)) else {
                return Region::empty();
            };
            match b.expanded(*margin_mm) {
                Some(bb) => Region::from_loops([pline::rect(bb.min, bb.max)], LoopRule::NonZero),
                None => {
                    diags.push(Diagnostic::error(
                        "eval.envelope_collapsed",
                        format!("a margin of {margin_mm} mm collapses the bounding box"),
                    ));
                    Region::empty()
                }
            }
        }
        EnvelopeProvider::BoundingCircle { margin_mm } => {
            let Some((c, r)) = geom::min_enclosing_circle(&reference_points(refs)) else {
                return Region::empty();
            };
            let radius = r + margin_mm;
            if radius <= 0.0 {
                diags.push(Diagnostic::error(
                    "eval.envelope_collapsed",
                    format!("a margin of {margin_mm} mm collapses the bounding circle"),
                ));
                return Region::empty();
            }
            Region::from_loops([pline::circle(c, radius)], LoopRule::NonZero)
        }
        EnvelopeProvider::Buffer { distance_mm, join } => {
            let base = union_ref(diags);
            if *distance_mm <= 0.0 {
                diags.push(Diagnostic::warning(
                    "eval.envelope_distance",
                    "buffer distance is not positive; the envelope adds no exterior band",
                ));
            }
            base.offset_join(*distance_mm, *join)
        }
        EnvelopeProvider::ConvexHull { distance_mm, join } => {
            let hull = geom::convex_hull(&reference_points(refs));
            if hull.len() < 3 {
                diags.push(Diagnostic::error(
                    "eval.envelope_collapsed",
                    "reference geometry is collinear; its convex hull has no area",
                ));
                return Region::empty();
            }
            let base = Region::from_loops([pline::from_points(hull, true)], LoopRule::NonZero);
            if *distance_mm == 0.0 {
                base
            } else {
                base.offset_join(*distance_mm, *join)
            }
        }
    }
}

fn empty_geometry(kind: GeometryKind) -> Geometry {
    match kind {
        GeometryKind::Region | GeometryKind::Envelope => Geometry::Region(Region::empty()),
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

    fn board() -> SourceDocument {
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
        sources.add("board.dxf", board());
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
    fn file_name_and_function_rules_match_single_layer_files() {
        use crate::source::{SourceDocument, SourceLayer};
        let gerber = |name: &str, function: Option<&str>| SourceDocument {
            format: SourceFormat::Gerber,
            version: None,
            units: Some(Unit::Millimetre),
            layers: vec![SourceLayer {
                name: name.into(),
                color: None,
                function: function.map(str::to_string),
                primitives: vec![Primitive::Hatch {
                    loops: vec![pline::rect(Point::new(0.0, 0.0), Point::new(1.0, 1.0))],
                    rule: LoopRule::NonZero,
                    solid: true,
                    pattern: String::new(),
                }],
            }],
            diagnostics: Vec::new(),
        };
        let mut sources = Sources::default();
        sources.add(
            "board-F_Cu.gbr",
            gerber("board-F_Cu.gbr", Some("Copper,L1,Top")),
        );
        sources.add("board.GKO", gerber("board.GKO", None));
        sources.add("board.dxf", board());

        // The presets' copper slot hits the X2 function; the outline slot falls through to
        // the `*.gko` glob. The DXF layer-name rules come first but do not match these files
        // alone.
        let profile = pcb_positive_ldi();
        let (bindings, unbound) = resolve_bindings(&profile, &sources);
        // The DXF file also has TOP_LAYER and ROUTE_TOOL_PATH, so the first rule wins there.
        assert_eq!(
            bindings[&SourceId::new("src-copper")],
            BindingStatus::Matched(2, "TOP_LAYER".into())
        );
        assert_eq!(
            bindings[&SourceId::new("src-outline")],
            BindingStatus::Matched(2, "ROUTE_TOOL_PATH".into())
        );
        assert_eq!(unbound.len(), 2);

        sources.remove(2);
        let (bindings, unbound) = resolve_bindings(&profile, &sources);
        assert_eq!(
            bindings[&SourceId::new("src-copper")],
            BindingStatus::Matched(0, "board-F_Cu.gbr".into())
        );
        assert_eq!(
            bindings[&SourceId::new("src-outline")],
            BindingStatus::Matched(1, "board.GKO".into())
        );
        assert!(unbound.is_empty());

        // A function glob is independent of the file name; a file glob respects the format.
        let mut p = Profile::empty("t", "t");
        p.source_bindings = vec![
            SourceBinding {
                id: SourceId::new("a"),
                name: "a".into(),
                required: true,
                rules: vec![MatchRule::Function {
                    pattern: "copper,*,top".into(),
                }],
            },
            SourceBinding {
                id: SourceId::new("b"),
                name: "b".into(),
                required: true,
                rules: vec![MatchRule::FileName {
                    format: Some(SourceFormat::Dxf),
                    pattern: "*.gko".into(),
                }],
            },
        ];
        let (bindings, _) = resolve_bindings(&p, &sources);
        assert_eq!(
            bindings[&SourceId::new("a")],
            BindingStatus::Matched(0, "board-F_Cu.gbr".into())
        );
        assert_eq!(bindings[&SourceId::new("b")], BindingStatus::Missing);
    }

    #[test]
    fn cache_invalidates_only_dependents() {
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
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
            operands: Vec::new(),
            op: LayerOp::Union,
            envelope: None,
            interpretation: Interpretation::default(),
            passes: vec![],
            enabled: true,
            reference_only: false,
        };
        profile.working_layers.push(mk(&a, &b));
        profile.working_layers.push(mk(&b, &a));
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
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
        sources.add(
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
        sources.add("a.dxf", board());
        sources.add("b.dxf", board());
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
        sources.add(
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
        sources.add(
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
        let region =
            |id: &str, inputs: Vec<Operand>, operands: Vec<Operand>, op: LayerOp| WorkingLayer {
                id: LayerId::new(id),
                name: id.into(),
                kind: GeometryKind::Region,
                inputs,
                operands,
                op,
                envelope: None,
                interpretation: Interpretation::default(),
                passes: vec![],
                enabled: true,
                reference_only: false,
            };
        // Dependent layer listed before its input on purpose; operand groups, not click
        // order, decide what is subtracted from what.
        profile.working_layers.push(region(
            "wl-diff",
            vec![Operand::Layer(LayerId::new("wl-a"))],
            vec![Operand::Source(SourceId::new("src-b"))],
            LayerOp::Subtraction,
        ));
        profile.working_layers.push(region(
            "wl-a",
            vec![Operand::Source(SourceId::new("src-a"))],
            vec![],
            LayerOp::Union,
        ));
        profile.working_layers.push(region(
            "wl-int",
            vec![Operand::Source(SourceId::new("src-a"))],
            vec![Operand::Source(SourceId::new("src-b"))],
            LayerOp::Intersection,
        ));
        // Group A is a union: (a ∪ b) − b = the part of a outside b.
        profile.working_layers.push(region(
            "wl-grouped",
            vec![
                Operand::Source(SourceId::new("src-b")),
                Operand::Source(SourceId::new("src-a")),
            ],
            vec![Operand::Source(SourceId::new("src-b"))],
            LayerOp::Subtraction,
        ));
        let e = Evaluator::new().evaluate(&profile, &sources);
        assert_eq!(
            e.order,
            vec![
                LayerId::new("wl-a"),
                LayerId::new("wl-diff"),
                LayerId::new("wl-int"),
                LayerId::new("wl-grouped")
            ]
        );
        let Geometry::Region(diff) = &e.layers[&LayerId::new("wl-diff")].geometry else {
            panic!()
        };
        let Geometry::Region(int) = &e.layers[&LayerId::new("wl-int")].geometry else {
            panic!()
        };
        let Geometry::Region(grouped) = &e.layers[&LayerId::new("wl-grouped")].geometry else {
            panic!()
        };
        assert!((diff.area() - 50.0).abs() < 1e-9);
        assert!((int.area() - 50.0).abs() < 1e-9);
        assert!(
            (grouped.area() - 50.0).abs() < 1e-9,
            "grouped {}",
            grouped.area()
        );
    }

    fn envelope_layer(id: &str, provider: EnvelopeProvider, refs: Vec<Operand>) -> WorkingLayer {
        WorkingLayer {
            id: LayerId::new(id),
            name: id.into(),
            kind: GeometryKind::Envelope,
            inputs: refs,
            operands: vec![],
            op: LayerOp::Union,
            envelope: Some(provider),
            interpretation: Interpretation::default(),
            passes: vec![],
            enabled: true,
            reference_only: true,
        }
    }

    fn negative(id: &str, domain: &str, subject: &str) -> WorkingLayer {
        WorkingLayer {
            id: LayerId::new(id),
            name: id.into(),
            kind: GeometryKind::Region,
            inputs: vec![Operand::Layer(LayerId::new(domain))],
            operands: vec![Operand::Layer(LayerId::new(subject))],
            op: LayerOp::Subtraction,
            envelope: None,
            interpretation: Interpretation::default(),
            passes: vec![],
            enabled: true,
            reference_only: false,
        }
    }

    #[test]
    fn same_subject_inverts_against_all_five_domain_providers() {
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
        let mut profile = pcb_positive_ldi();
        let copper = Operand::Layer(LayerId::new("wl-copper"));
        let providers = [
            ("box", EnvelopeProvider::BoundingBox { margin_mm: 0.5 }),
            (
                "circle",
                EnvelopeProvider::BoundingCircle { margin_mm: 0.5 },
            ),
            (
                "buffer",
                EnvelopeProvider::Buffer {
                    distance_mm: 0.5,
                    join: JoinStyle::Round,
                },
            ),
            (
                "hull",
                EnvelopeProvider::ConvexHull {
                    distance_mm: 0.5,
                    join: JoinStyle::Round,
                },
            ),
        ];
        for (name, provider) in providers {
            profile.working_layers.push(envelope_layer(
                &format!("env-{name}"),
                provider,
                vec![copper.clone()],
            ));
            profile.working_layers.push(negative(
                &format!("neg-{name}"),
                &format!("env-{name}"),
                "wl-copper",
            ));
        }
        profile.working_layers.push(envelope_layer(
            "env-named",
            EnvelopeProvider::NamedBoundary,
            vec![Operand::Source(SourceId::new("src-outline"))],
        ));
        profile
            .working_layers
            .push(negative("neg-named", "env-named", "wl-copper"));

        let e = Evaluator::new().evaluate(&profile, &sources);
        assert!(!e.has_errors(), "{:?}", e.all_diagnostics());
        let Geometry::Region(copper_region) = &e.layers[&LayerId::new("wl-copper")].geometry else {
            panic!()
        };
        let copper_area = copper_region.area();
        let copper_islands = copper_region.outers().count();
        let copper_bounds = copper_region.bounds().unwrap();
        for name in ["box", "circle", "buffer", "hull", "named"] {
            let Geometry::Region(env) = &e.layers[&LayerId::new(format!("env-{name}"))].geometry
            else {
                panic!()
            };
            let Geometry::Region(neg) = &e.layers[&LayerId::new(format!("neg-{name}"))].geometry
            else {
                panic!()
            };
            assert!(!env.is_empty(), "{name}: envelope empty");
            let eb = env.bounds().unwrap();
            assert!(
                eb.min.x <= copper_bounds.min.x + 1e-9 && eb.max.x >= copper_bounds.max.x - 1e-9,
                "{name}: x extent"
            );
            assert!(
                (neg.area() - (env.area() - copper_area)).abs() < 1e-6,
                "{name}: area {} vs {}",
                neg.area(),
                env.area() - copper_area
            );
            // One outer frame plus one island per copper hole (the disc inside the annulus),
            // and at least one hole per copper island.
            if name != "buffer" {
                assert_eq!(
                    neg.outers().count(),
                    1 + copper_region.holes().count(),
                    "{name}: outers"
                );
                assert!(
                    neg.holes().count() >= copper_islands,
                    "{name}: holes {} < islands {}",
                    neg.holes().count(),
                    copper_islands
                );
            }
        }
        let Geometry::Region(bx) = &e.layers[&LayerId::new("env-box")].geometry else {
            panic!()
        };
        let bb = bx.bounds().unwrap();
        assert!(
            (bb.min.x - (copper_bounds.min.x - 0.5)).abs() < 1e-9
                && (bb.max.y - (copper_bounds.max.y + 0.5)).abs() < 1e-9
        );
        let Geometry::Region(circ) = &e.layers[&LayerId::new("env-circle")].geometry else {
            panic!()
        };
        assert!(
            circ.contains(Point::new(copper_bounds.min.x, copper_bounds.min.y))
                || circ.contains(Point::new(copper_bounds.max.x, copper_bounds.max.y))
        );
        assert!(
            e.passes
                .iter()
                .all(|(_, p)| !p.layer.as_str().starts_with("env-")),
            "envelopes never mark by themselves"
        );
    }

    #[test]
    fn envelope_parameters_recompute_dependents_only() {
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
        let mut profile = pcb_positive_ldi();
        profile.working_layers.push(envelope_layer(
            "env",
            EnvelopeProvider::BoundingBox { margin_mm: 0.5 },
            vec![Operand::Layer(LayerId::new("wl-copper"))],
        ));
        profile
            .working_layers
            .push(negative("neg", "env", "wl-copper"));
        let mut ev = Evaluator::new();
        let first = ev.evaluate(&profile, &sources);
        let Geometry::Region(a) = &first.layers[&LayerId::new("neg")].geometry else {
            panic!()
        };
        let area_a = a.area();
        profile
            .working_layers
            .iter_mut()
            .find(|l| l.id.as_str() == "env")
            .unwrap()
            .envelope = Some(EnvelopeProvider::BoundingBox { margin_mm: 2.0 });
        let second = ev.evaluate(&profile, &sources);
        assert!(
            !second.layers[&LayerId::new("env")].cached
                && !second.layers[&LayerId::new("neg")].cached
        );
        assert!(
            second.layers[&LayerId::new("wl-copper")].cached,
            "the subject did not change"
        );
        let Geometry::Region(b) = &second.layers[&LayerId::new("neg")].geometry else {
            panic!()
        };
        assert!(b.area() > area_a + 1.0, "bigger margin, bigger negative");
    }

    #[test]
    fn unbounded_or_missing_domains_are_errors_not_guesses() {
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
        let mut profile = pcb_positive_ldi();
        profile.source_bindings.push(SourceBinding {
            id: SourceId::new("src-none"),
            name: "missing".into(),
            required: false,
            rules: vec![MatchRule::LayerName {
                format: None,
                name: "NOPE".into(),
            }],
        });
        profile.working_layers.push(envelope_layer(
            "env-empty",
            EnvelopeProvider::BoundingBox { margin_mm: 1.0 },
            vec![Operand::Source(SourceId::new("src-none"))],
        ));
        profile
            .working_layers
            .push(negative("neg-empty", "env-empty", "wl-copper"));
        profile.working_layers.push(envelope_layer(
            "env-small",
            EnvelopeProvider::BoundingBox { margin_mm: -5.0 },
            vec![Operand::Layer(LayerId::new("wl-copper"))],
        ));
        profile
            .working_layers
            .push(negative("neg-small", "env-small", "wl-copper"));
        let e = Evaluator::new().evaluate(&profile, &sources);
        let env_empty = &e.layers[&LayerId::new("env-empty")];
        assert!(
            env_empty
                .diagnostics
                .iter()
                .any(|d| d.code == "eval.envelope_unbounded" && d.severity == Severity::Error)
        );
        assert!(env_empty.geometry.is_empty());
        assert!(
            e.layers[&LayerId::new("neg-empty")].geometry.is_empty(),
            "no silent page-sized inverse"
        );
        assert!(
            e.layers[&LayerId::new("neg-small")]
                .diagnostics
                .iter()
                .any(|d| d.code == "eval.subject_outside_domain")
        );
    }

    #[test]
    fn buffer_envelope_honours_the_join_style() {
        let mut sources = Sources::default();
        sources.add("board.dxf", board());
        let mut profile = pcb_positive_ldi();
        profile.working_layers.push(WorkingLayer {
            id: LayerId::new("env"),
            name: "env".into(),
            kind: GeometryKind::Envelope,
            inputs: vec![Operand::Source(SourceId::new("src-outline"))],
            operands: Vec::new(),
            op: LayerOp::Union,
            envelope: Some(EnvelopeProvider::Buffer {
                distance_mm: 1.0,
                join: JoinStyle::Miter { limit: 4.0 },
            }),
            interpretation: Interpretation {
                loop_rule: LoopRuleCfg::MergeThenNest,
                stroke_width: StrokeWidth::None,
            },
            passes: Vec::new(),
            enabled: true,
            reference_only: true,
        });
        let ev = Evaluator::new().evaluate(&profile, &sources);
        let Geometry::Region(r) = &ev.layers[&LayerId::new("env")].geometry else {
            panic!()
        };
        // The outline rectangle (−2..40 × −2..20) grown by 1 with mitred corners.
        assert!((r.area() - 44.0 * 24.0).abs() < 1e-9, "{}", r.area());
        // JSON: a round join is omitted, a mitre is written with its limit.
        let json = profile::to_json(&profile);
        assert!(json.contains("\"join\": {"), "{json}");
        assert!(json.contains("\"kind\": \"miter\""));
    }
}
