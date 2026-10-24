//! The DXF reader against the generated fixtures and their manifest expectations.

use std::path::PathBuf;

use cavalier_contours::polyline::PlineSource;
use lvp_core::diag::Diagnostics;
use lvp_core::dxf::read::{DxfDocument, Primitive, ReadOptions, read_str};
use lvp_core::geom::BBox;
use lvp_core::pline;
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn manifest() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures_dir().join("manifest.json")).unwrap())
        .unwrap()
}

fn load(entry: &Value) -> DxfDocument {
    let text =
        std::fs::read_to_string(fixtures_dir().join(entry["file"].as_str().unwrap())).unwrap();
    read_str(&text, &ReadOptions::default()).unwrap()
}

fn bounds_of(doc: &DxfDocument, layer: Option<&str>) -> Option<BBox> {
    doc.layers
        .iter()
        .filter(|l| layer.is_none_or(|n| l.name == n))
        .flat_map(|l| l.primitives.iter())
        .filter_map(|p| match p {
            Primitive::Path { pline: pl, .. } => pline::bounds(pl),
            Primitive::Hatch { loops, .. } => loops
                .iter()
                .filter_map(pline::bounds)
                .reduce(|a, b| a.union(b)),
        })
        .reduce(|a, b| a.union(b))
}

fn assert_bbox(b: Option<BBox>, want: &Value, what: &str) {
    let b = b.unwrap_or_else(|| panic!("{what}: no geometry"));
    let w: Vec<f64> = want
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let got = [b.min.x, b.min.y, b.max.x, b.max.y];
    for (g, e) in got.iter().zip(&w) {
        assert!((g - e).abs() < 1e-6, "{what}: bounds {got:?} vs {w:?}");
    }
}

fn closed_open_counts(doc: &DxfDocument, layer: &str) -> (usize, usize) {
    let mut closed = 0;
    let mut open = 0;
    for p in &doc.layer(layer).unwrap().primitives {
        match p {
            Primitive::Path { pline: pl, .. } => {
                if pl.is_closed() {
                    closed += 1
                } else {
                    open += 1
                }
            }
            Primitive::Hatch { loops, .. } => closed += loops.len(),
        }
    }
    (closed, open)
}

#[test]
fn every_fixture_reads_with_manifest_layers_and_bounds() {
    let m = manifest();
    for entry in m["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["format"] == "dxf")
    {
        let name = entry["name"].as_str().unwrap();
        let doc = load(entry);
        assert!(
            !doc.diagnostics.has_errors(),
            "{name}: {:?}",
            doc.diagnostics
        );
        for (layer, info) in entry["layers"].as_object().unwrap() {
            assert!(doc.layer(layer).is_some(), "{name}: layer {layer} missing");
            assert_bbox(
                bounds_of(&doc, Some(layer)),
                &info["bounds_mm"],
                &format!("{name}/{layer}"),
            );
        }
        assert_bbox(bounds_of(&doc, None), &entry["bounds_mm"], name);
    }
}

#[test]
fn open_linework_stays_open() {
    let m = manifest();
    let entry = m["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "open-linework")
        .unwrap();
    let doc = load(entry);
    let (closed, open) = closed_open_counts(&doc, "MARK");
    assert_eq!(
        closed,
        entry["expect"]["MARK"]["closed_contours"].as_u64().unwrap() as usize
    );
    assert_eq!(
        open,
        entry["expect"]["MARK"]["open_paths"].as_u64().unwrap() as usize
    );
    let (closed, open) = closed_open_counts(&doc, "FILL");
    assert_eq!((closed, open), (1, 0));
}

#[test]
fn nested_holes_and_blocks_resolve_to_expected_contours() {
    let m = manifest();
    let fx = m["fixtures"].as_array().unwrap();
    let nested = fx.iter().find(|f| f["name"] == "nested-holes").unwrap();
    let doc = load(nested);
    assert_eq!(
        closed_open_counts(&doc, "SHAPES").0,
        nested["expect"]["closed_contours"].as_u64().unwrap() as usize
    );

    let blocks = fx.iter().find(|f| f["name"] == "blocks-insert").unwrap();
    let doc = load(blocks);
    assert_eq!(
        closed_open_counts(&doc, "PADS").0,
        blocks["expect"]["resolved_closed_contours"]
            .as_u64()
            .unwrap() as usize
    );
    // Arcs survive the uniform-scale inserts.
    let arcs = doc
        .layer("PADS")
        .unwrap()
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Path { pline: pl, .. } if pl.iter_vertexes().any(|v| v.bulge != 0.0)))
        .count();
    assert_eq!(arcs, 4);
}

#[test]
fn inch_fixture_is_converted_to_mm() {
    let m = manifest();
    let entry = m["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "units-inch")
        .unwrap();
    let doc = load(entry);
    assert_eq!(doc.units, Some(lvp_core::units::Unit::Inch));
    assert_bbox(
        bounds_of(&doc, None),
        &entry["expect"]["bounds_mm"],
        "units-inch",
    );
}

#[test]
fn board_fixture_copper_width() {
    let m = manifest();
    let entry = m["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "board-islands")
        .unwrap();
    let doc = load(entry);
    let b = bounds_of(&doc, Some("TOP_LAYER")).unwrap();
    assert!((b.width() - entry["expect"]["copper_width_mm"].as_f64().unwrap()).abs() < 1e-9);
    let widths: Vec<f64> = doc
        .layer("TOP_LAYER")
        .unwrap()
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Path { width: Some(w), .. } => Some(*w),
            _ => None,
        })
        .collect();
    assert_eq!(widths.len(), 2);
}
