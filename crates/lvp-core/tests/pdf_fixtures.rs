//! The generated PDF fixture reads back with the geometry the manifest promises.

use cavalier_contours::polyline::PlineSource;
use lvp_core::pline;
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
        .find(|e| e["name"] == name && e["format"] == "pdf")
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
fn features_fixture_matches_the_manifest() {
    let e = entry("features");
    let doc = load(&e);
    let x = &e["expect"];
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    let want: Vec<&str> = x["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, want);

    let outline = doc.layer("Outline").unwrap();
    assert_eq!(
        outline.primitives.len(),
        x["outline_paths"].as_u64().unwrap() as usize
    );
    let Primitive::Path { pline, width } = &outline.primitives[0] else {
        panic!()
    };
    assert!(pline.is_closed());
    assert!((width.unwrap() - f(&x["outline_width_mm"])).abs() < 1e-9);
    let b = pline::bounds(pline).unwrap();
    assert!((b.width() - f(&x["outline_side_mm"])).abs() < 1e-9);

    // Bézier circle: the kappa approximation overshoots the area by about 0.03 %.
    let red = f(&x["red_area_mm2"]);
    let red_area = fills(&doc, "#ff0000").area();
    assert!((red_area - red).abs() < red * 1e-3, "{red_area} vs {red}");
    let black = fills(&doc, "#000000");
    assert!((black.area() - f(&x["black_area_mm2"])).abs() < 1e-6);
    assert_eq!(
        black.holes().count(),
        x["black_holes"].as_u64().unwrap() as usize
    );
    assert!((fills(&doc, "#009900").area() - f(&x["green_area_mm2"])).abs() < 1e-6);
    let grey = fills(&doc, "#808080");
    assert!(
        (grey.area() - f(&x["grey_area_mm2"])).abs() < 1e-6,
        "{}",
        grey.area()
    );
    let gb = grey.bounds().unwrap();
    let wb = x["grey_bounds_mm"].as_array().unwrap();
    assert!(
        (gb.min.x - f(&wb[0])).abs() < 1e-6 && (gb.max.x - f(&wb[2])).abs() < 1e-6,
        "{gb:?}"
    );

    let codes: Vec<&str> = doc.diagnostics.iter().map(|d| d.code).collect();
    for want in x["diagnostics"].as_array().unwrap() {
        assert!(codes.contains(&want.as_str().unwrap()), "{codes:?}");
    }
}
