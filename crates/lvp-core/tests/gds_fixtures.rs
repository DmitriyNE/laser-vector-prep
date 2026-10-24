//! The generated GDSII fixture reads back with the geometry the manifest promises.

use cavalier_contours::polyline::PlineSource;
use lvp_core::region::Region;
use lvp_core::source::{Primitive, ReadOptions, SourceDocument, read_any};
use lvp_core::units::Unit;
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
        .find(|e| e["name"] == name && e["format"] == "gds")
        .unwrap()
        .clone()
}

fn load(entry: &Value) -> SourceDocument {
    let rel = entry["file"].as_str().unwrap();
    let path = format!("{}/../../fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    read_any(
        rel.rsplit('/').next().unwrap(),
        &bytes,
        &ReadOptions::default(),
    )
    .unwrap()
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
fn cells_fixture_matches_the_manifest() {
    let e = entry("cells");
    let doc = load(&e);
    let x = &e["expect"];
    assert_eq!(doc.units, Some(Unit::Micrometre));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    let want: Vec<&str> = x["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, want);

    let l1 = fills(&doc, "1/0");
    assert_eq!(
        l1.outers().count(),
        x["layer_1_islands"].as_u64().unwrap() as usize
    );
    assert!(
        (l1.area() - f(&x["layer_1_area_mm2"])).abs() < 1e-12,
        "{}",
        l1.area()
    );
    let l2 = fills(&doc, "2/0");
    assert!(
        (l2.area() - f(&x["layer_2_area_mm2"])).abs() < 1e-12,
        "{}",
        l2.area()
    );

    let l3 = doc.layer("3/0").unwrap();
    let round: Vec<_> = l3
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Path { pline, width } => Some((pline, width.unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(
        round.len(),
        x["layer_3_round_paths"].as_u64().unwrap() as usize
    );
    assert!((round[0].1 - f(&x["layer_3_round_width_mm"])).abs() < 1e-12);
    assert!(!round[0].0.is_closed());
    let l3_fill = fills(&doc, "3/0");
    assert!(
        (l3_fill.area() - f(&x["layer_3_fill_area_mm2"])).abs() < 1e-12,
        "{}",
        l3_fill.area()
    );

    assert!((fills(&doc, "4/0").area() - f(&x["layer_4_area_mm2"])).abs() < 1e-12);

    let mut all = Region::empty();
    for l in &doc.layers {
        let r = fills(&doc, &l.name);
        all = if all.is_empty() { r } else { all.union(&r) };
    }
    let b = all.bounds().unwrap();
    let wb = x["bounds_mm"].as_array().unwrap();
    assert!(
        (b.min.x - f(&wb[0])).abs() < 1e-9 && (b.min.y - f(&wb[1])).abs() < 1e-9,
        "{b:?}"
    );
    assert!(
        (b.max.x - f(&wb[2])).abs() < 1e-9 && (b.max.y - f(&wb[3])).abs() < 1e-9,
        "{b:?}"
    );

    let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
    for want in x["diagnostics"].as_array().unwrap() {
        assert!(codes.contains(&want.as_str().unwrap()), "{codes:?}");
    }
}
