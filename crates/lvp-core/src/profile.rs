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

/// One step migrations: `(from, f)` maps schema `from` to `from + 1`.
type Migration = fn(Value) -> Result<Value, String>;
const MIGRATIONS: &[(u32, Migration)] = &[(1, migrate_v1_to_v2)];

/// v1 had one ordered `inputs` list; subtraction meant "first minus the rest". v2 keeps
/// `inputs` as group A and adds `operands` (group B): for subtraction, intersection and
/// xor the first input stays in A and the rest move to B; union layers are unchanged.
fn migrate_v1_to_v2(mut value: Value) -> Result<Value, String> {
    let Some(layers) = value.get_mut("workingLayers").and_then(Value::as_array_mut) else {
        return Ok(value);
    };
    for layer in layers {
        let op = layer
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("union")
            .to_string();
        let inputs = layer
            .get("inputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let (a, b) = if op == "union" || inputs.len() < 2 {
            (inputs, Vec::new())
        } else {
            (inputs[..1].to_vec(), inputs[1..].to_vec())
        };
        let obj = layer
            .as_object_mut()
            .ok_or("working layer is not an object")?;
        obj.insert("inputs".into(), Value::Array(a));
        obj.insert("operands".into(), Value::Array(b));
    }
    Ok(value)
}

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
        if l.op != LayerOp::Union && l.operands.is_empty() {
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

/// Read-only presets shipped with the app. Users duplicate them to edit.
pub fn builtin_presets() -> Vec<Profile> {
    vec![pcb_positive_ldi()]
}

/// Copper exposed with inward contours (blue), mechanical outline passed through (red):
/// the configuration validated on the Full Spectrum UV galvo.
pub fn pcb_positive_ldi() -> Profile {
    let mut p = Profile::empty(
        "builtin-pcb-positive-ldi",
        "PCB positive LDI (contour fill)",
    );
    p.source_bindings = vec![
        SourceBinding {
            id: SourceId::new("src-copper"),
            name: "Copper".into(),
            required: true,
            rules: vec![MatchRule::LayerName {
                format: Some(SourceFormat::Dxf),
                name: "TOP_LAYER".into(),
            }],
        },
        SourceBinding {
            id: SourceId::new("src-outline"),
            name: "Board outline".into(),
            required: false,
            rules: vec![MatchRule::LayerName {
                format: Some(SourceFormat::Dxf),
                name: "ROUTE_TOOL_PATH".into(),
            }],
        },
    ];
    p.output_groups = vec![
        OutputGroup {
            id: GroupId::new("grp-copper"),
            name: "Copper".into(),
            color: Rgb::BLUE,
            stroke_width_mm: None,
            export: true,
            visible: true,
        },
        OutputGroup {
            id: GroupId::new("grp-outline"),
            name: "Outline".into(),
            color: Rgb::RED,
            stroke_width_mm: None,
            export: true,
            visible: true,
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
            interpretation: Interpretation::default(),
            passes: vec![Pass {
                id: PassId::new("pass-copper-fill"),
                name: "Contour fill 20 um".into(),
                pattern: Pattern::ContourFill {
                    pitch_mm: 0.02,
                    first_offset_mm: None,
                    gap_fill: true,
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
    ];
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostics;

    #[test]
    fn preset_roundtrips_and_validates() {
        let p = pcb_positive_ldi();
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
        let json = to_json(&p);
        let back = parse_profile(&json).unwrap();
        assert_eq!(back, p);
        assert!(json.contains("\"schemaVersion\": 2"));
        assert!(json.contains("\"kind\": \"contourFill\""));
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
    fn v1_subtraction_migrates_to_two_groups() {
        let v1 = r#"{"schemaVersion": 1, "profileId": "p", "name": "old",
            "workingLayers": [
              {"id": "a", "name": "a", "kind": "region", "inputs": [{"source": "s1"}, {"source": "s2"}, {"layer": "x"}], "op": "subtraction"},
              {"id": "b", "name": "b", "kind": "region", "inputs": [{"source": "s1"}, {"source": "s2"}], "op": "union"},
              {"id": "c", "name": "c", "kind": "region", "inputs": [{"source": "s1"}], "op": "intersection"}
            ]}"#;
        let p = parse_profile(v1).unwrap();
        assert_eq!(p.schema_version, SCHEMA_VERSION);
        let a = p.layer(&LayerId::new("a")).unwrap();
        assert_eq!(a.inputs, vec![Operand::Source(SourceId::new("s1"))]);
        assert_eq!(
            a.operands,
            vec![
                Operand::Source(SourceId::new("s2")),
                Operand::Layer(LayerId::new("x"))
            ]
        );
        let b = p.layer(&LayerId::new("b")).unwrap();
        assert_eq!(b.inputs.len(), 2);
        assert!(b.operands.is_empty());
        let c = p.layer(&LayerId::new("c")).unwrap();
        assert_eq!(c.inputs.len(), 1);
        assert!(c.operands.is_empty());
        // Migrated output re-parses as current.
        assert_eq!(parse_profile(&to_json(&p)).unwrap(), p);
    }

    #[test]
    fn minimal_profile_gets_defaults() {
        let json = r#"{"schemaVersion": 2, "profileId": "p", "name": "Empty"}"#;
        let p = parse_profile(json).unwrap();
        assert_eq!(p.export.stroke_width_mm, 0.0254);
        assert_eq!(p.optimizer.mode, OptimizerMode::AsGenerated);
        assert!(p.working_layers.is_empty());
    }

    #[test]
    fn shape_errors_are_reported() {
        let json = r#"{"schemaVersion": 2, "profileId": "p", "name": "x", "workingLayers": [{"id": "a", "name": "a", "kind": "blob", "inputs": [], "op": "union"}]}"#;
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
        };
        let d = validate(&p);
        assert!(d.has_errors());
        let codes: Vec<&str> = d.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"profile.unknown_group"));
        assert!(codes.contains(&"profile.unknown_layer"));
        assert!(codes.contains(&"profile.color_collision"));
        assert!(codes.contains(&"profile.pitch"));
    }
}
