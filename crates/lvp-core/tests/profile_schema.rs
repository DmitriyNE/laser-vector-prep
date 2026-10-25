//! The published JSON Schema of the profile format (`docs/profile-schema.json`) is the one
//! generated from the serde types, and it accepts what the app writes. Regenerate it with
//! `just schema` after changing the model.

use lvp_core::profile::{builtin_presets, json_schema, to_json};
use serde_json::{Value, json};

const PUBLISHED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/profile-schema.json"
);

#[test]
fn published_schema_is_current() {
    let generated = json_schema();
    if std::env::var_os("UPDATE_PROFILE_SCHEMA").is_some() {
        std::fs::write(PUBLISHED, &generated).unwrap();
    }
    let published = std::fs::read_to_string(PUBLISHED).unwrap_or_default();
    assert!(
        published == generated,
        "docs/profile-schema.json is stale: run `just schema`"
    );
}

fn validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(&json_schema()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

#[test]
fn presets_and_saved_profiles_conform() {
    let v = validator();
    let mut documents: Vec<(String, Value)> = builtin_presets()
        .iter()
        .map(|p| {
            (
                p.profile_id.clone(),
                serde_json::from_str(&to_json(p)).unwrap(),
            )
        })
        .collect();
    // Profiles written by the deployed v1 app have the same shape (v2 only adds a field).
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/profiles");
    for e in std::fs::read_dir(root).unwrap() {
        let path = e.unwrap().path();
        documents.push((
            path.display().to_string(),
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        ));
    }
    for (name, doc) in &documents {
        let errors: Vec<String> = v.iter_errors(doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

#[test]
fn malformed_profiles_are_rejected() {
    let v = validator();
    let preset: Value = serde_json::from_str(&to_json(&builtin_presets()[0])).unwrap();
    let broken = |f: &dyn Fn(&mut Value)| {
        let mut d = preset.clone();
        f(&mut d);
        d
    };
    for (what, doc) in [
        (
            "unknown pattern kind",
            broken(&|d| d["workingLayers"][0]["passes"][0]["pattern"]["kind"] = json!("spiral")),
        ),
        (
            "pitch as text",
            broken(&|d| d["workingLayers"][0]["passes"][0]["pattern"]["pitchMm"] = json!("0.02")),
        ),
        (
            "missing name",
            broken(&|d| {
                d.as_object_mut().unwrap().remove("name");
            }),
        ),
        (
            "colour with two channels",
            broken(&|d| d["outputGroups"][0]["color"] = json!([0, 0])),
        ),
        (
            "audit width as text",
            broken(&|d| d["outputGroups"][0]["auditWidthMm"] = json!("wide")),
        ),
    ] {
        assert!(!v.is_valid(&doc), "accepted: {what}");
    }
}
