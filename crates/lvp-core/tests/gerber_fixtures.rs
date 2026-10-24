//! The generated Gerber fixtures read back with the geometry `fixtures/manifest.json`
//! promises, and the PCB presets accept them through the function and file-name rules.

use std::f64::consts::PI;

use cavalier_contours::polyline::PlineSource;
use lvp_core::eval::{BindingStatus, Evaluator, Sources};
use lvp_core::model::{LayerId, PassId, Pattern, SourceId};
use lvp_core::patterns::Geometry;
use lvp_core::pline;
use lvp_core::profile::{boundary_cutting, pcb_negative_hatch, pcb_positive_ldi};
use lvp_core::region::{self, LoopRule, Region};
use lvp_core::source::{Primitive, ReadOptions, SourceDocument, read_any};
use serde_json::Value;

fn manifest() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/manifest.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn entry(name: &str) -> Value {
    manifest()["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("fixture {name} missing from manifest"))
        .clone()
}

fn load(entry: &Value) -> (String, SourceDocument) {
    let rel = entry["file"].as_str().unwrap();
    let path = format!("{}/../../fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    let file_name = rel.rsplit('/').next().unwrap().to_string();
    let doc = read_any(&file_name, &bytes, &ReadOptions::default()).unwrap();
    (file_name, doc)
}

fn region_of(doc: &SourceDocument) -> Region {
    let mut r = Region::empty();
    for p in &doc.layers[0].primitives {
        let part = match p {
            Primitive::Hatch { loops, rule, .. } => {
                Region::from_loops(loops.iter().cloned(), *rule)
            }
            Primitive::Path { pline, width } => region::stroke(pline, width.unwrap()),
        };
        r = if r.is_empty() { part } else { r.union(&part) };
    }
    r
}

fn bounds(v: &Value) -> [f64; 4] {
    let a = v.as_array().unwrap();
    [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap())
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn copper_fixture_matches_the_dxf_board() {
    let e = entry("board-F_Cu");
    let (name, doc) = load(&e);
    assert_eq!(doc.layers.len(), 1);
    assert_eq!(doc.layers[0].name, name);
    assert_eq!(doc.layers[0].function.as_deref(), e["function"].as_str());
    assert_eq!(doc.version.as_deref(), Some("X2"));
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    assert_eq!(
        doc.layers[0].primitives.len(),
        e["expect"]["primitives"].as_u64().unwrap() as usize
    );
    let r = region_of(&doc);
    assert_eq!(
        r.outers().count(),
        e["expect"]["copper_islands"].as_u64().unwrap() as usize
    );
    assert_eq!(
        r.holes().count(),
        e["expect"]["copper_holes"].as_u64().unwrap() as usize
    );
    let b = r.bounds().unwrap();
    let want = bounds(&e["expect"]["copper_bounds_mm"]);
    assert!(
        close(b.min.x, want[0], 1e-6)
            && close(b.min.y, want[1], 1e-6)
            && close(b.max.x, want[2], 1e-6)
            && close(b.max.y, want[3], 1e-6),
        "{b:?} vs {want:?}"
    );
}

#[test]
fn outline_fixture_is_one_closed_path() {
    let e = entry("board-Edge_Cuts");
    let (_, doc) = load(&e);
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    let prims = &doc.layers[0].primitives;
    assert_eq!(prims.len(), 1);
    let Primitive::Path { pline, width } = &prims[0] else {
        panic!("outline must be a path");
    };
    assert_eq!(*width, Some(e["expect"]["width_mm"].as_f64().unwrap()));
    assert!(pline.is_closed());
    assert!(close(
        pline::length(pline),
        e["expect"]["length_mm"].as_f64().unwrap(),
        1e-6
    ));
    let b = pline::bounds(pline).unwrap();
    let want = bounds(&e["expect"]["bounds_mm"]);
    assert!(
        close(b.min.x, want[0], 1e-9)
            && close(b.max.x, want[2], 1e-9)
            && close(b.max.y, want[3], 1e-9)
    );
}

#[test]
fn features_fixture_has_the_promised_area_and_topology() {
    let e = entry("features");
    let (_, doc) = load(&e);
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    // Clear polarity is used, so the reader composited everything into one region.
    assert_eq!(doc.layers[0].primitives.len(), 1);
    let r = region_of(&doc);
    assert_eq!(
        r.outers().count(),
        e["expect"]["islands"].as_u64().unwrap() as usize
    );
    assert_eq!(
        r.holes().count(),
        e["expect"]["holes"].as_u64().unwrap() as usize
    );
    let want_area = e["expect"]["area_mm2"].as_f64().unwrap();
    assert!(
        close(r.area(), want_area, 1e-6),
        "area {} vs {want_area}",
        r.area()
    );
    let b = r.bounds().unwrap();
    let want = bounds(&e["expect"]["bounds_mm"]);
    assert!(
        close(b.min.x, want[0], 1e-9)
            && close(b.min.y, want[1], 1e-9)
            && close(b.max.x, want[2], 1e-9)
            && close(b.max.y, want[3], 1e-9),
        "{b:?} vs {want:?}"
    );
    // The thermal pad alone: four islands within its 4 mm circle around (40, 5).
    let thermal = r.islands().into_iter().filter(|i| {
        let c = i.bounds().unwrap();
        c.min.x >= 37.9 && c.max.x <= 42.1
    });
    assert_eq!(
        thermal.count(),
        e["expect"]["thermal_islands"].as_u64().unwrap() as usize
    );
    let _ = PI;
}

#[test]
fn pcb_presets_accept_the_gerber_pair() {
    let mut sources = Sources::default();
    for name in ["board-F_Cu", "board-Edge_Cuts"] {
        let (file, doc) = load(&entry(name));
        sources.add(file, doc);
    }

    // Positive LDI: copper and outline bound through the X2 functions, same islands as DXF.
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.2;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    assert_eq!(
        ev.bindings[&SourceId::new("src-copper")],
        BindingStatus::Matched(0, "board-F_Cu.gbr".into())
    );
    assert_eq!(
        ev.bindings[&SourceId::new("src-outline")],
        BindingStatus::Matched(1, "board-Edge_Cuts.gbr".into())
    );
    let Geometry::Region(copper) = &ev.layers[&LayerId::new("wl-copper")].geometry else {
        panic!()
    };
    assert_eq!(copper.outers().count(), 10);
    assert_eq!(copper.holes().count(), 1);
    assert_eq!(
        ev.pass(&PassId::new("pass-outline")).unwrap().paths.len(),
        1
    );

    // Negative: the drawn profile is the board interior, not a 0.05 mm ring.
    let mut profile = pcb_negative_hatch();
    for l in &mut profile.working_layers {
        for p in &mut l.passes {
            if let Pattern::Hatch { pitch_mm, .. } = &mut p.pattern {
                *pitch_mm = 0.2;
            }
        }
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let Geometry::Region(board) = &ev.layers[&LayerId::new("wl-board")].geometry else {
        panic!()
    };
    assert!(
        close(board.area(), 25.0 * 11.3 - (4.0 - PI), 1e-6),
        "{}",
        board.area()
    );
    let Geometry::Region(bg) = &ev.layers[&LayerId::new("wl-background")].geometry else {
        panic!()
    };
    assert_eq!(bg.outers().count(), 2);
    assert_eq!(bg.holes().count(), 10);

    // Cutting: one closed trace along the outline's centreline.
    let ev = Evaluator::new().evaluate(&boundary_cutting(), &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let cut = ev.pass(&PassId::new("pass-cut")).unwrap();
    assert_eq!(cut.paths.len(), 1);
    assert!(cut.paths[0].is_closed());
    assert!(close(
        pline::length(&cut.paths[0]),
        2.0 * (25.0 + 11.3) - 8.0 + 2.0 * PI,
        1e-6
    ));
    let _ = LoopRule::NonZero;
}
