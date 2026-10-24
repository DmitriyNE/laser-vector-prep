//! The generated IPC-2581 fixture reads back with the geometry the manifest promises, and
//! the PCB presets bind it through the layer functions.

use std::f64::consts::PI;

use cavalier_contours::polyline::PlineSource;
use lvp_core::eval::{BindingStatus, Evaluator, Sources};
use lvp_core::model::{LayerId, Pattern, SourceId};
use lvp_core::patterns::Geometry;
use lvp_core::pline;
use lvp_core::profile::pcb_positive_ldi;
use lvp_core::region::Region;
use lvp_core::source::{Primitive, ReadOptions, SourceDocument, read_any};
use serde_json::Value;

fn entry(name: &str) -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/manifest.json"
    ))
    .unwrap();
    let m: Value = serde_json::from_str(&text).unwrap();
    m["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name && e["format"] == "ipc2581")
        .unwrap()
        .clone()
}

fn load(entry: &Value) -> (String, SourceDocument) {
    let rel = entry["file"].as_str().unwrap();
    let path = format!("{}/../../fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    let file = rel.rsplit('/').next().unwrap().to_string();
    (
        file.clone(),
        read_any(&file, &bytes, &ReadOptions::default()).unwrap(),
    )
}

fn fills(doc: &SourceDocument, layer: &str) -> Region {
    let mut r = Region::empty();
    for p in &doc.layer(layer).unwrap().primitives {
        if let Primitive::Hatch { loops, rule, .. } = p {
            let part = Region::from_loops(loops.iter().cloned(), *rule);
            r = if r.is_empty() { part } else { r.union(&part) };
        }
    }
    r
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

#[test]
fn board_fixture_matches_the_manifest() {
    let e = entry("board");
    let (_, doc) = load(&e);
    let x = &e["expect"];
    assert!(
        doc.diagnostics
            .iter()
            .all(|d| d.severity != lvp_core::diag::Severity::Error),
        "{:?}",
        doc.diagnostics
    );
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    let want: Vec<&str> = x["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, want);
    for (layer, func) in x["functions"].as_object().unwrap() {
        assert_eq!(
            doc.layer(layer).unwrap().function.as_deref(),
            func.as_str(),
            "{layer}"
        );
    }

    let top = fills(&doc, "TOP");
    assert!(
        (top.area() - f(&x["top_fill_area_mm2"])).abs() < 1e-6,
        "{}",
        top.area()
    );
    let strokes: Vec<f64> = doc
        .layer("TOP")
        .unwrap()
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Path { width, .. } => Some(width.unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(strokes.len(), x["top_strokes"].as_u64().unwrap() as usize);
    assert!((strokes[0] - f(&x["top_stroke_width_mm"])).abs() < 1e-9);

    let bottom = doc.layer("BOTTOM").unwrap();
    let Primitive::Path { pline, width } = &bottom.primitives[0] else {
        panic!("arc trace is a centreline")
    };
    assert!((width.unwrap() - 0.3).abs() < 1e-9);
    assert!(!pline.is_closed());
    assert!((pline::length(pline) - f(&x["bottom_arc_length_mm"])).abs() < 1e-9);

    let sm = fills(&doc, "SM_TOP");
    assert!(
        (sm.area() - f(&x["sm_area_mm2"])).abs() < 1e-6,
        "{}",
        sm.area()
    );
    assert_eq!(sm.holes().count(), x["sm_holes"].as_u64().unwrap() as usize);
    let profile = fills(&doc, "Profile");
    assert!(
        (profile.area() - f(&x["profile_area_mm2"])).abs() < 1e-6,
        "{}",
        profile.area()
    );

    let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
    for want in x["diagnostics"].as_array().unwrap() {
        assert!(codes.contains(&want.as_str().unwrap()), "{codes:?}");
    }
    let _ = PI;
}

#[test]
fn pcb_preset_binds_conductor_and_profile_by_function() {
    let (file, doc) = load(&entry("board"));
    let mut sources = Sources::default();
    sources.add(file, doc);
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.2;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    assert_eq!(
        ev.bindings[&SourceId::new("src-copper")],
        BindingStatus::Matched(0, "TOP".into())
    );
    assert_eq!(
        ev.bindings[&SourceId::new("src-outline")],
        BindingStatus::Matched(0, "Profile".into())
    );
    let Geometry::Region(copper) = &ev.layers[&LayerId::new("wl-copper")].geometry else {
        panic!()
    };
    // Two pads joined by the trace, the rotated square, the oval, the donut (1 hole), the
    // triangle, the bar and the L: 7 islands.
    assert_eq!(copper.outers().count(), 7);
    assert_eq!(copper.holes().count(), 1);
}
