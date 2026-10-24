//! Profile persistence: versioned JSON, migrations, validation and built-in presets.
//!
//! JSON is config data, never code. A profile from a newer schema is rejected with a
//! precise message and nothing else changes. Older schemas migrate step by step.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::diag::Diagnostic;
use crate::model::*;

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    /// Not JSON, or not an object.
    Syntax(String),
    MissingSchemaVersion,
    /// Schema is newer than this build understands.
    Newer {
        found: u32,
        supported: u32,
    },
    /// Migration from an old version failed.
    Migration {
        from: u32,
        message: String,
    },
    /// JSON is valid but does not match the schema.
    Shape(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::Syntax(m) => write!(f, "profile is not valid JSON: {m}"),
            ProfileError::MissingSchemaVersion => write!(f, "profile has no numeric schemaVersion"),
            ProfileError::Newer { found, supported } => write!(
                f,
                "profile schema version {found} is newer than the supported version {supported}; update the app to import it"
            ),
            ProfileError::Migration { from, message } => {
                write!(f, "could not migrate profile from schema {from}: {message}")
            }
            ProfileError::Shape(m) => write!(f, "profile does not match the schema: {m}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// One step migrations: `(from, f)` maps schema `from` to `from + 1`. Empty until a schema
/// version has shipped; pre-deployment changes redefine the current version instead.
type Migration = fn(Value) -> Result<Value, String>;
const MIGRATIONS: &[(u32, Migration)] = &[];

/// Parse (and migrate) a profile JSON document.
pub fn parse_profile(json: &str) -> Result<Profile, ProfileError> {
    let mut value: Value =
        serde_json::from_str(json).map_err(|e| ProfileError::Syntax(e.to_string()))?;
    if !value.is_object() {
        return Err(ProfileError::Syntax("top level is not an object".into()));
    }
    let mut version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .ok_or(ProfileError::MissingSchemaVersion)?;
    if version > SCHEMA_VERSION {
        return Err(ProfileError::Newer {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    while version < SCHEMA_VERSION {
        let Some((_, migrate)) = MIGRATIONS.iter().find(|(from, _)| *from == version) else {
            return Err(ProfileError::Migration {
                from: version,
                message: "no migration path".into(),
            });
        };
        value = migrate(value).map_err(|message| ProfileError::Migration {
            from: version,
            message,
        })?;
        version += 1;
        value["schemaVersion"] = Value::from(version);
    }
    serde_json::from_value(value).map_err(|e| ProfileError::Shape(e.to_string()))
}

pub fn to_json(profile: &Profile) -> String {
    let mut s = serde_json::to_string_pretty(profile).expect("profile serialises");
    s.push('\n');
    s
}

/// Structural validation: ids unique, references resolvable, parameters sane, colour
/// collisions. Geometry-level problems (cycles, bindings) come from evaluation.
pub fn validate(profile: &Profile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut dup = |id: &str, what: &str, out: &mut Vec<Diagnostic>| {
        if !seen.insert(id.to_string()) {
            out.push(Diagnostic::error(
                "profile.duplicate_id",
                format!("{what} id {id:?} is used more than once"),
            ));
        }
    };
    for b in &profile.source_bindings {
        dup(b.id.as_str(), "source binding", &mut out);
        if b.rules.is_empty() {
            out.push(Diagnostic::warning(
                "profile.binding_no_rules",
                format!("source binding {:?} has no matching rules", b.name),
            ));
        }
        for r in &b.rules {
            let empty = match r {
                MatchRule::LayerName { name, .. } => name.trim().is_empty(),
                MatchRule::FileLayer { file, name } => {
                    file.trim().is_empty() || name.trim().is_empty()
                }
                MatchRule::FileName { pattern, .. } | MatchRule::Function { pattern } => {
                    pattern.trim().is_empty()
                }
            };
            if empty {
                out.push(Diagnostic::warning(
                    "profile.binding_rule_empty",
                    format!(
                        "source binding {:?} has a rule with an empty name or pattern",
                        b.name
                    ),
                ));
            }
        }
    }
    for id in &profile.optimizer.protected {
        if profile.layer(id).is_none() {
            out.push(
                Diagnostic::warning(
                    "profile.protected_unknown",
                    format!("path order protects unknown layer {id}"),
                )
                .with_context(id.to_string()),
            );
        }
    }
    for g in &profile.output_groups {
        dup(g.id.as_str(), "output group", &mut out);
        if let Some(w) = g.stroke_width_mm
            && w <= 0.0
        {
            out.push(Diagnostic::error(
                "profile.stroke_width",
                format!("output group {:?} has a non-positive stroke width", g.name),
            ));
        }
    }
    for l in &profile.working_layers {
        dup(l.id.as_str(), "working layer", &mut out);
        for p in &l.passes {
            dup(p.id.as_str(), "pass", &mut out);
        }
    }
    let group_ids: HashSet<&GroupId> = profile.output_groups.iter().map(|g| &g.id).collect();
    let layer_ids: HashSet<&LayerId> = profile.working_layers.iter().map(|l| &l.id).collect();
    let source_ids: HashSet<&SourceId> = profile.source_bindings.iter().map(|b| &b.id).collect();
    for l in &profile.working_layers {
        if l.inputs.is_empty() {
            out.push(
                Diagnostic::warning(
                    "profile.layer_no_inputs",
                    format!("working layer {:?} has no inputs", l.name),
                )
                .with_context(l.id.to_string()),
            );
        }
        if l.kind != GeometryKind::Envelope && l.op != LayerOp::Union && l.operands.is_empty() {
            out.push(
                Diagnostic::warning(
                    "profile.no_operands",
                    format!(
                        "layer {:?} uses {:?} but has no operands in group B",
                        l.name, l.op
                    ),
                )
                .with_context(l.id.to_string()),
            );
        }
        if l.op == LayerOp::Union && !l.operands.is_empty() {
            out.push(
                Diagnostic::info(
                    "profile.union_operands",
                    format!("layer {:?} is a union; group B is merged into it", l.name),
                )
                .with_context(l.id.to_string()),
            );
        }
        for op in l.inputs.iter().chain(&l.operands) {
            match op {
                Operand::Source(s) if !source_ids.contains(s) => out.push(
                    Diagnostic::error(
                        "profile.unknown_source",
                        format!("layer {:?} references unknown source binding {s}", l.name),
                    )
                    .with_context(l.id.to_string()),
                ),
                Operand::Layer(id) if !layer_ids.contains(id) => out.push(
                    Diagnostic::error(
                        "profile.unknown_layer",
                        format!("layer {:?} references unknown layer {id}", l.name),
                    )
                    .with_context(l.id.to_string()),
                ),
                _ => {}
            }
        }
        match (l.kind, &l.envelope) {
            (GeometryKind::Envelope, None) => out.push(
                Diagnostic::error("profile.envelope_provider", format!("envelope layer {:?} has no provider", l.name)).with_context(l.id.to_string()),
            ),
            (GeometryKind::Envelope, Some(EnvelopeProvider::Buffer { distance_mm, .. })) if *distance_mm <= 0.0 => out.push(
                Diagnostic::warning("profile.envelope_distance", format!("envelope {:?} buffers by {distance_mm} mm; a positive distance is needed for an exterior band", l.name))
                    .with_context(l.id.to_string()),
            ),
            (GeometryKind::Envelope, Some(_)) if !l.operands.is_empty() || l.op != LayerOp::Union => out.push(
                Diagnostic::info("profile.envelope_ignores_op", format!("envelope {:?} uses only its reference set (group A); the operation and group B are ignored", l.name))
                    .with_context(l.id.to_string()),
            ),
            (k, Some(_)) if k != GeometryKind::Envelope => out.push(
                Diagnostic::warning("profile.envelope_unused", format!("layer {:?} has an envelope provider but is not an envelope layer", l.name)).with_context(l.id.to_string()),
            ),
            _ => {}
        }
        if l.kind == GeometryKind::Linework && l.op != LayerOp::Union {
            out.push(
                Diagnostic::error(
                    "profile.linework_op",
                    format!(
                        "linework layer {:?} can only combine inputs by union",
                        l.name
                    ),
                )
                .with_context(l.id.to_string()),
            );
        }
        for p in &l.passes {
            if !group_ids.contains(&p.group) {
                out.push(
                    Diagnostic::error(
                        "profile.unknown_group",
                        format!("pass {:?} targets unknown output group {}", p.name, p.group),
                    )
                    .with_context(p.id.to_string()),
                );
            }
            match &p.pattern {
                Pattern::ContourFill {
                    pitch_mm,
                    first_offset_mm,
                    ..
                } => {
                    if pitch_mm.is_nan() || *pitch_mm <= 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.pitch",
                                format!("pass {:?} has a non-positive pitch", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if let Pattern::ContourFill {
                        end_spacing_mm: Some(e),
                        ..
                    } = &p.pattern
                        && *e < 0.0
                    {
                        out.push(
                            Diagnostic::error(
                                "profile.end_spacing",
                                format!("pass {:?} has a negative end spacing", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if first_offset_mm.is_some_and(|f| f < 0.0) {
                        out.push(
                            Diagnostic::error(
                                "profile.first_offset",
                                format!("pass {:?} has a negative first offset", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                }
                Pattern::BoundaryTrace {
                    rings,
                    ring_spacing_mm,
                    kerf_mm,
                    ..
                } => {
                    if *rings == 0 {
                        out.push(
                            Diagnostic::error(
                                "profile.rings",
                                format!("pass {:?} has zero rings", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if *rings > 1 && *ring_spacing_mm <= 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.ring_spacing",
                                format!(
                                    "pass {:?} has several rings but no positive ring spacing",
                                    p.name
                                ),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if *kerf_mm < 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.kerf",
                                format!("pass {:?} has a negative kerf", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                }
                Pattern::Hatch {
                    pitch_mm, inset_mm, ..
                }
                | Pattern::CrossHatch {
                    pitch_mm, inset_mm, ..
                } => {
                    if pitch_mm.is_nan() || *pitch_mm <= 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.pitch",
                                format!("pass {:?} has a non-positive pitch", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if *inset_mm < 0.0 {
                        out.push(Diagnostic::warning("profile.inset", format!("pass {:?} has a negative inset: the hatch extends outside the region", p.name)).with_context(p.id.to_string()));
                    }
                    if let Pattern::CrossHatch { angles_deg, .. } = &p.pattern
                        && angles_deg.is_empty()
                    {
                        out.push(
                            Diagnostic::error(
                                "profile.angles",
                                format!("pass {:?} has no hatch angles", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                }
                Pattern::BoundaryHatch {
                    shells,
                    shell_pitch_mm,
                    pitch_mm,
                    angles_deg,
                    ..
                } => {
                    if *shells > 0 && *shell_pitch_mm <= 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.shell_pitch",
                                format!("pass {:?} has shells but no positive shell pitch", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if pitch_mm.is_nan() || *pitch_mm <= 0.0 {
                        out.push(
                            Diagnostic::error(
                                "profile.pitch",
                                format!("pass {:?} has a non-positive hatch pitch", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                    if angles_deg.is_empty() {
                        out.push(
                            Diagnostic::error(
                                "profile.angles",
                                format!("pass {:?} has no hatch angles", p.name),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                }
                Pattern::Passthrough { simplify_mm, .. } => {
                    if simplify_mm.is_some_and(|s| s < 0.0) {
                        out.push(
                            Diagnostic::error(
                                "profile.simplify",
                                format!(
                                    "pass {:?} has a negative simplification tolerance",
                                    p.name
                                ),
                            )
                            .with_context(p.id.to_string()),
                        );
                    }
                }
            }
        }
    }
    // Colour collisions between groups that export.
    let mut by_color: HashMap<Rgb, Vec<&str>> = HashMap::new();
    for g in profile.output_groups.iter().filter(|g| g.export) {
        by_color.entry(g.color).or_default().push(&g.name);
    }
    for (color, names) in by_color {
        if names.len() > 1 {
            out.push(Diagnostic::warning(
                "profile.color_collision",
                format!("output groups {} share the exact colour {:?}; the laser software cannot tell them apart", names.join(", "), color),
            ));
        }
    }
    if profile.export.stroke_width_mm.is_nan() || profile.export.stroke_width_mm <= 0.0 {
        out.push(Diagnostic::error(
            "profile.stroke_width",
            "export stroke width must be positive",
        ));
    }
    out
}

/// Read-only presets shipped with the app (spec §9). Users duplicate them to edit. The
/// first one is the default profile of a fresh project.
pub fn builtin_presets() -> Vec<Profile> {
    vec![
        pcb_positive_ldi(),
        pcb_negative_hatch(),
        general_marking(),
        boundary_cutting(),
    ]
}

fn dxf_layer(name: &str) -> MatchRule {
    MatchRule::LayerName {
        format: Some(SourceFormat::Dxf),
        name: name.into(),
    }
}

fn any_layer(name: &str) -> MatchRule {
    MatchRule::LayerName {
        format: None,
        name: name.into(),
    }
}

fn function(pattern: &str) -> MatchRule {
    MatchRule::Function {
        pattern: pattern.into(),
    }
}

fn gerber_file(pattern: &str) -> MatchRule {
    MatchRule::FileName {
        format: Some(SourceFormat::Gerber),
        pattern: pattern.into(),
    }
}

/// Rules for the top copper layer: DXF layer name, then Gerber X2 function, then the
/// common Gerber file-name conventions (Protel/Altium, KiCad, Eagle).
fn copper_rules() -> Vec<MatchRule> {
    vec![
        dxf_layer("TOP_LAYER"),
        function("Copper,L1,Top"),
        function("CONDUCTOR,TOP"),
        gerber_file("*.gtl"),
        gerber_file("*-F_Cu.gbr"),
        gerber_file("*.cmp"),
    ]
}

/// Rules for the mechanical outline: DXF layer name, then Gerber profile function, then
/// file-name conventions.
fn outline_rules() -> Vec<MatchRule> {
    vec![
        dxf_layer("ROUTE_TOOL_PATH"),
        function("Profile*"),
        gerber_file("*.gko"),
        gerber_file("*.gm1"),
        gerber_file("*-Edge_Cuts.gbr"),
    ]
}

fn slot(id: &str, name: &str, required: bool, rules: Vec<MatchRule>) -> SourceBinding {
    SourceBinding {
        id: SourceId::new(id),
        name: name.into(),
        required,
        rules,
    }
}

fn group(id: &str, name: &str, color: Rgb) -> OutputGroup {
    OutputGroup {
        id: GroupId::new(id),
        name: name.into(),
        color,
        stroke_width_mm: None,
        export: true,
        visible: true,
        order_mode: None,
    }
}

fn pass(id: &str, name: &str, pattern: Pattern, group: &str) -> Pass {
    Pass {
        id: PassId::new(id),
        name: name.into(),
        pattern,
        group: GroupId::new(group),
        enabled: true,
    }
}

/// A layer with one operand group, no envelope and the default interpretation.
fn layer(
    id: &str,
    name: &str,
    kind: GeometryKind,
    inputs: Vec<Operand>,
    passes: Vec<Pass>,
) -> WorkingLayer {
    WorkingLayer {
        id: LayerId::new(id),
        name: name.into(),
        kind,
        inputs,
        operands: Vec::new(),
        op: LayerOp::Union,
        envelope: None,
        interpretation: Interpretation::default(),
        passes,
        enabled: true,
        reference_only: false,
    }
}

fn reference(mut l: WorkingLayer) -> WorkingLayer {
    l.reference_only = true;
    l
}

fn envelope_layer(
    id: &str,
    name: &str,
    provider: EnvelopeProvider,
    inputs: Vec<Operand>,
) -> WorkingLayer {
    let mut l = layer(id, name, GeometryKind::Envelope, inputs, Vec::new());
    l.envelope = Some(provider);
    reference(l)
}

/// Read closed contours as loops whatever width they were drawn with (Gerber profiles are
/// thin draws); open paths stay errors.
fn contour_interpretation() -> Interpretation {
    Interpretation {
        loop_rule: LoopRuleCfg::MergeThenNest,
        stroke_width: StrokeWidth::None,
    }
}

fn outline_passthrough() -> Pass {
    pass(
        "pass-outline",
        "Outline passthrough",
        Pattern::Passthrough {
            simplify_mm: None,
            reverse: false,
        },
        "grp-outline",
    )
}

/// The background around the copper exposed instead of the copper (negative process):
/// board interior (named boundary from the mechanical outline) minus the copper grown by a
/// clearance, hatched at 45 degrees; outline passed through in red.
pub fn pcb_negative_hatch() -> Profile {
    let mut p = Profile::empty(
        "builtin-pcb-negative-hatch",
        "PCB negative (background hatch)",
    );
    p.source_bindings = vec![
        slot("src-copper", "Copper", true, copper_rules()),
        // Required here: it is the domain of the inversion.
        slot("src-outline", "Board outline", true, outline_rules()),
    ];
    p.output_groups = vec![
        group("grp-background", "Background", Rgb::BLUE),
        group("grp-outline", "Outline", Rgb::RED),
    ];
    let mut background = layer(
        "wl-background",
        "Background",
        GeometryKind::Region,
        vec![Operand::Layer(LayerId::new("wl-board"))],
        vec![pass(
            "pass-background-hatch",
            "Hatch 20 um at 45 deg",
            Pattern::Hatch {
                pitch_mm: 0.02,
                angle_deg: 45.0,
                phase_mm: 0.0,
                serpentine: true,
                inset_mm: 0.0,
            },
            "grp-background",
        )],
    );
    background.operands = vec![Operand::Layer(LayerId::new("wl-clearance"))];
    background.op = LayerOp::Subtraction;
    p.working_layers = vec![
        reference(layer(
            "wl-copper",
            "Copper",
            GeometryKind::Region,
            vec![Operand::Source(SourceId::new("src-copper"))],
            Vec::new(),
        )),
        {
            let mut board = envelope_layer(
                "wl-board",
                "Board interior",
                EnvelopeProvider::NamedBoundary,
                vec![Operand::Source(SourceId::new("src-outline"))],
            );
            board.interpretation = contour_interpretation();
            board
        },
        envelope_layer(
            "wl-clearance",
            "Copper + 50 um clearance",
            EnvelopeProvider::Buffer {
                distance_mm: 0.05,
                join: JoinStyle::Round,
            },
            vec![Operand::Layer(LayerId::new("wl-copper"))],
        ),
        background,
        layer(
            "wl-outline",
            "Outline",
            GeometryKind::Linework,
            vec![Operand::Source(SourceId::new("src-outline"))],
            vec![outline_passthrough()],
        ),
    ];
    // Exposure lands on the background; the copper must stay unexposed, so jumps avoid it.
    p.optimizer = OptimizerSettings {
        mode: OptimizerMode::BleedAware,
        protected: vec![LayerId::new("wl-copper")],
        ..OptimizerSettings::default()
    };
    p
}

/// Engraving-style marking: linework passed through as drawn, closed shapes filled with
/// one perimeter shell and a 45 degree hatch.
pub fn general_marking() -> Profile {
    let mut p = Profile::empty(
        "builtin-general-marking",
        "General marking (lines + hatched fill)",
    );
    // Any format: marking artwork comes from DXF and SVG alike.
    p.source_bindings = vec![
        slot("src-marks", "Marks", true, vec![any_layer("MARK")]),
        slot("src-fill", "Fill", false, vec![any_layer("FILL")]),
    ];
    p.output_groups = vec![
        group("grp-marks", "Marks", Rgb::BLACK),
        group("grp-fill", "Fill", Rgb::BLUE),
    ];
    p.working_layers = vec![
        layer(
            "wl-marks",
            "Marks",
            GeometryKind::Linework,
            vec![Operand::Source(SourceId::new("src-marks"))],
            vec![pass(
                "pass-marks",
                "Marks passthrough",
                Pattern::Passthrough {
                    simplify_mm: None,
                    reverse: false,
                },
                "grp-marks",
            )],
        ),
        layer(
            "wl-fill",
            "Fill",
            GeometryKind::Region,
            vec![Operand::Source(SourceId::new("src-fill"))],
            vec![pass(
                "pass-fill",
                "Shell + hatch 50 um",
                Pattern::BoundaryHatch {
                    shells: 1,
                    shell_pitch_mm: 0.05,
                    inset_mm: 0.025,
                    pitch_mm: 0.05,
                    angles_deg: vec![45.0],
                    phase_mm: 0.0,
                    serpentine: true,
                },
                "grp-fill",
            )],
        ),
    ];
    p.optimizer.mode = OptimizerMode::MinimumJump;
    p
}

/// Cutting along a closed outline: one trace on the boundary, inner cut-outs before the
/// outer contour. Kerf compensation is left at zero for the user to enter.
pub fn boundary_cutting() -> Profile {
    let mut p = Profile::empty(
        "builtin-boundary-cutting",
        "Boundary cutting (contour trace)",
    );
    let mut rules = vec![
        dxf_layer("ROUTE_TOOL_PATH"),
        dxf_layer("CUT"),
        dxf_layer("OUTLINE"),
    ];
    rules.extend(outline_rules().into_iter().skip(1));
    p.source_bindings = vec![slot("src-cut", "Cut outline", true, rules)];
    p.output_groups = vec![group("grp-cut", "Cut", Rgb::RED)];
    let mut cut = layer(
        "wl-cut",
        "Cut",
        GeometryKind::Region,
        vec![Operand::Source(SourceId::new("src-cut"))],
        vec![pass(
            "pass-cut",
            "Boundary trace, holes first",
            Pattern::BoundaryTrace {
                side: OffsetSide::On,
                offset_mm: 0.0,
                kerf_mm: 0.0,
                rings: 1,
                ring_spacing_mm: 0.0,
                reverse: false,
                inner_first: true,
                join: JoinStyle::Round,
            },
            "grp-cut",
        )],
    );
    cut.interpretation = contour_interpretation();
    p.working_layers = vec![cut];
    // Holes stay before their outline: the trace emits that constraint.
    p.optimizer.mode = OptimizerMode::MinimumJump;
    p
}

/// Copper exposed with inward contours (blue), mechanical outline passed through (red):
/// the configuration validated on the Full Spectrum UV galvo.
pub fn pcb_positive_ldi() -> Profile {
    let mut p = Profile::empty(
        "builtin-pcb-positive-ldi",
        "PCB positive LDI (contour fill)",
    );
    p.source_bindings = vec![
        slot("src-copper", "Copper", true, copper_rules()),
        slot("src-outline", "Board outline", false, outline_rules()),
    ];
    p.output_groups = vec![
        OutputGroup {
            id: GroupId::new("grp-copper"),
            name: "Copper".into(),
            color: Rgb::BLUE,
            stroke_width_mm: None,
            export: true,
            visible: true,
            order_mode: None,
        },
        OutputGroup {
            id: GroupId::new("grp-outline"),
            name: "Outline".into(),
            color: Rgb::RED,
            stroke_width_mm: None,
            export: true,
            visible: true,
            order_mode: None,
        },
    ];
    p.working_layers = vec![
        WorkingLayer {
            id: LayerId::new("wl-copper"),
            name: "Copper".into(),
            kind: GeometryKind::Region,
            inputs: vec![Operand::Source(SourceId::new("src-copper"))],
            operands: Vec::new(),
            op: LayerOp::Union,
            envelope: None,
            interpretation: Interpretation::default(),
            passes: vec![Pass {
                id: PassId::new("pass-copper-fill"),
                name: "Contour fill 20 um".into(),
                pattern: Pattern::ContourFill {
                    pitch_mm: 0.02,
                    first_offset_mm: None,
                    gap_fill: true,
                    end_spacing_mm: None,
                    simplify_mm: None,
                },
                group: GroupId::new("grp-copper"),
                enabled: true,
            }],
            enabled: true,
            reference_only: false,
        },
        WorkingLayer {
            id: LayerId::new("wl-outline"),
            name: "Outline".into(),
            kind: GeometryKind::Linework,
            inputs: vec![Operand::Source(SourceId::new("src-outline"))],
            operands: Vec::new(),
            op: LayerOp::Union,
            envelope: None,
            interpretation: Interpretation::default(),
            passes: vec![Pass {
                id: PassId::new("pass-outline"),
                name: "Outline passthrough".into(),
                pattern: Pattern::Passthrough {
                    simplify_mm: None,
                    reverse: false,
                },
                group: GroupId::new("grp-outline"),
                enabled: true,
            }],
            enabled: true,
            reference_only: false,
        },
        // Keep-clear area for path ordering: the background around the copper inside the
        // artwork's bounding box. Jump footprints there could expose resist that must stay
        // unexposed, so bleed-aware ordering keeps jumps over copper where it can.
        envelope_layer(
            "wl-artwork-box",
            "Artwork box",
            EnvelopeProvider::BoundingBox { margin_mm: 1.0 },
            vec![Operand::Layer(LayerId::new("wl-copper"))],
        ),
        {
            let mut keep = layer(
                "wl-keep-clear",
                "Keep clear (background)",
                GeometryKind::Region,
                vec![Operand::Layer(LayerId::new("wl-artwork-box"))],
                Vec::new(),
            );
            keep.operands = vec![Operand::Layer(LayerId::new("wl-copper"))];
            keep.op = LayerOp::Subtraction;
            reference(keep)
        },
    ];
    p.optimizer = OptimizerSettings {
        mode: OptimizerMode::BleedAware,
        protected: vec![LayerId::new("wl-keep-clear")],
        ..OptimizerSettings::default()
    };
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostics;

    #[test]
    fn presets_roundtrip_and_validate() {
        let presets = builtin_presets();
        assert_eq!(presets.len(), 4);
        let ids: HashSet<&str> = presets.iter().map(|p| p.profile_id.as_str()).collect();
        assert_eq!(ids.len(), presets.len(), "preset ids must be unique");
        for p in &presets {
            assert!(validate(p).is_empty(), "{}: {:?}", p.name, validate(p));
            let json = to_json(p);
            let back = parse_profile(&json).unwrap();
            assert_eq!(&back, p, "{}", p.name);
            assert!(json.contains("\"schemaVersion\": 1"));
            assert!(
                p.working_layers
                    .iter()
                    .any(|l| !l.reference_only && !l.passes.is_empty()),
                "{} produces no output",
                p.name
            );
        }
        // Path order defaults: bleed-aware for the two LDI presets (each protects what must
        // stay unexposed), minimum jump for the others.
        let modes: Vec<(OptimizerMode, Vec<&str>)> = presets
            .iter()
            .map(|p| {
                (
                    p.optimizer.mode,
                    p.optimizer.protected.iter().map(|l| l.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            modes,
            vec![
                (OptimizerMode::BleedAware, vec!["wl-keep-clear"]),
                (OptimizerMode::BleedAware, vec!["wl-copper"]),
                (OptimizerMode::MinimumJump, vec![]),
                (OptimizerMode::MinimumJump, vec![]),
            ]
        );
        // The default project profile is the positive LDI one.
        assert_eq!(presets[0], pcb_positive_ldi());
        assert!(to_json(&presets[0]).contains("\"kind\": \"contourFill\""));
    }

    #[test]
    fn newer_schema_is_rejected_precisely() {
        let json = r#"{"schemaVersion": 99, "profileId": "x", "name": "x"}"#;
        assert_eq!(
            parse_profile(json).unwrap_err(),
            ProfileError::Newer {
                found: 99,
                supported: SCHEMA_VERSION
            }
        );
        assert_eq!(
            parse_profile(r#"{"profileId": "x"}"#).unwrap_err(),
            ProfileError::MissingSchemaVersion
        );
        assert!(matches!(
            parse_profile("not json").unwrap_err(),
            ProfileError::Syntax(_)
        ));
        assert!(matches!(
            parse_profile("[1,2]").unwrap_err(),
            ProfileError::Syntax(_)
        ));
    }

    #[test]
    fn older_schema_without_a_migration_is_rejected_precisely() {
        let json = r#"{"schemaVersion": 0, "profileId": "x", "name": "x"}"#;
        assert_eq!(
            parse_profile(json).unwrap_err(),
            ProfileError::Migration {
                from: 0,
                message: "no migration path".into()
            }
        );
    }

    #[test]
    fn minimal_profile_gets_defaults() {
        let json = r#"{"schemaVersion": 1, "profileId": "p", "name": "Empty"}"#;
        let p = parse_profile(json).unwrap();
        assert_eq!(p.export.stroke_width_mm, 0.0254);
        assert_eq!(p.optimizer.mode, OptimizerMode::MinimumJump);
        assert!(p.working_layers.is_empty());
    }

    #[test]
    fn shape_errors_are_reported() {
        let json = r#"{"schemaVersion": 1, "profileId": "p", "name": "x", "workingLayers": [{"id": "a", "name": "a", "kind": "blob", "inputs": [], "op": "union"}]}"#;
        assert!(matches!(
            parse_profile(json).unwrap_err(),
            ProfileError::Shape(_)
        ));
    }

    #[test]
    fn validation_catches_references_and_collisions() {
        let mut p = pcb_positive_ldi();
        p.working_layers[0].passes[0].group = GroupId::new("nope");
        p.working_layers[1]
            .inputs
            .push(Operand::Layer(LayerId::new("ghost")));
        p.output_groups[1].color = Rgb::BLUE;
        p.working_layers[0].passes[0].pattern = Pattern::ContourFill {
            pitch_mm: 0.0,
            first_offset_mm: None,
            gap_fill: true,
            end_spacing_mm: None,
            simplify_mm: None,
        };
        let d = validate(&p);
        assert!(d.has_errors());
        let codes: Vec<&str> = d.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"profile.unknown_group"));
        assert!(codes.contains(&"profile.unknown_layer"));
        assert!(codes.contains(&"profile.color_collision"));
        assert!(codes.contains(&"profile.pitch"));
        let mut q = pcb_positive_ldi();
        q.optimizer.protected.push(LayerId::new("ghost"));
        assert!(
            validate(&q)
                .iter()
                .any(|d| d.code == "profile.protected_unknown")
        );
    }
}
