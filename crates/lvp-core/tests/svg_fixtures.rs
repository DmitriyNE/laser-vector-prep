//! The generated SVG fixture reads back with the geometry the manifest promises, and the
//! general-marking preset binds its layers by name.

use std::f64::consts::PI;

use cavalier_contours::polyline::PlineSource;
use lvp_core::eval::{Evaluator, Sources};
use lvp_core::model::{PassId, Pattern};
use lvp_core::profile::general_marking;
use lvp_core::region::{self, Region};
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
        .find(|e| e["name"] == name && e["format"] == "svg")
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
    let l = doc.layer(layer).unwrap();
    let mut r = Region::empty();
    for p in &l.primitives {
        if let Primitive::Hatch { loops, rule, .. } = p {
            let part = Region::from_loops(loops.iter().cloned(), *rule);
            r = if r.is_empty() { part } else { r.union(&part) };
        }
    }
    r
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn marking_fixture_matches_the_manifest() {
    let e = entry("marking");
    let (_, doc) = load(&e);
    assert!(
        doc.diagnostics
            .iter()
            .all(|d| d.severity != lvp_core::diag::Severity::Error),
        "{:?}",
        doc.diagnostics
    );
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    let want: Vec<&str> = e["expect"]["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, want);

    // MARK: four open stroked centrelines with their widths, none closed.
    let mark = doc.layer("MARK").unwrap();
    let widths: Vec<f64> = mark
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Path { pline, width } => {
                assert!(!pline.is_closed());
                width.unwrap()
            }
            _ => panic!("MARK must be linework"),
        })
        .collect();
    assert_eq!(
        widths.len(),
        e["expect"]["mark_open_paths"].as_u64().unwrap() as usize
    );
    let want_w: Vec<f64> = e["expect"]["mark_widths_mm"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let mut sorted = widths.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut want_sorted = want_w.clone();
    want_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for (a, b) in sorted.iter().zip(&want_sorted) {
        assert!(close(*a, *b, 1e-6), "{a} vs {b}");
    }

    // FILL: circle + even-odd ring + two scaled pads.
    let fill = fills(&doc, "FILL");
    assert!(
        close(
            fill.area(),
            e["expect"]["fill_area_mm2"].as_f64().unwrap(),
            0.05
        ),
        "{}",
        fill.area()
    );
    assert_eq!(
        fill.outers().count(),
        e["expect"]["fill_islands"].as_u64().unwrap() as usize
    );
    assert_eq!(
        fill.holes().count(),
        e["expect"]["fill_holes"].as_u64().unwrap() as usize
    );

    // CLIPPED: the rect survives only to the right of x = 50.
    let clipped = fills(&doc, "CLIPPED");
    assert!(
        close(
            clipped.area(),
            e["expect"]["clipped_area_mm2"].as_f64().unwrap(),
            1e-3
        ),
        "{}",
        clipped.area()
    );
    let b = clipped.bounds().unwrap();
    let wb = e["expect"]["clipped_bounds_mm"].as_array().unwrap();
    assert!(
        close(b.min.x, wb[0].as_f64().unwrap(), 1e-5)
            && close(b.max.x, wb[2].as_f64().unwrap(), 1e-5),
        "{b:?}"
    );
    let _ = PI;
}

#[test]
fn general_marking_preset_binds_the_svg_layers() {
    let (file, doc) = load(&entry("marking"));
    let mut sources = Sources::default();
    sources.add(file, doc);
    let mut profile = general_marking();
    // Coarse hatch for test speed.
    for l in &mut profile.working_layers {
        for p in &mut l.passes {
            if let Pattern::BoundaryHatch {
                pitch_mm,
                shell_pitch_mm,
                ..
            } = &mut p.pattern
            {
                *pitch_mm = 0.3;
                *shell_pitch_mm = 0.3;
            }
        }
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let marks = ev.pass(&PassId::new("pass-marks")).unwrap();
    assert_eq!(marks.paths.len(), 4);
    let fill = ev.pass(&PassId::new("pass-fill")).unwrap();
    assert!(
        fill.paths.iter().filter(|p| p.is_closed()).count() >= 4,
        "one shell per island"
    );
    assert!(fill.paths.len() > 20);
    let _ = region::stroke;
}
