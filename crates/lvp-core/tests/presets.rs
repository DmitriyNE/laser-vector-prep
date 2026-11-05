//! Every built-in preset evaluates a matching fixture end to end (spec §9: presets
//! demonstrate PCB positive LDI, inverted/background hatch, general marking and boundary
//! cutting). The positive LDI preset is covered by `export_pipeline.rs`.

use cavalier_contours::polyline::PlineSource;
use lvp_core::diag::Severity;
use lvp_core::dxf::read::{ReadOptions, read_str};
use lvp_core::dxf::write::{Document, Entity};
use lvp_core::eval::{BindingStatus, Evaluator, Sources};
use lvp_core::export::collect_paths;
use lvp_core::geom::Point;
use lvp_core::model::{LayerId, PassId, Pattern, Profile, Rgb, SourceId};
use lvp_core::patterns::Geometry;
use lvp_core::pline::{self, Pline};
use lvp_core::profile::{boundary_cutting, general_marking, pcb_negative_hatch};
use lvp_core::region::Region;
use lvp_core::source::{ReadOptions as AnyOptions, read_any};
use lvp_core::units::Unit;

fn fixture(name: &str) -> Sources {
    let path = format!("{}/../../fixtures/dxf/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut s = Sources::default();
    s.add(name, read_str(&text, &ReadOptions::default()).unwrap());
    s
}

/// Coarsen every pitch in the profile so the tests stay fast; semantics are unchanged.
fn coarsen(profile: &mut Profile, min_pitch: f64) {
    for layer in &mut profile.working_layers {
        for pass in &mut layer.passes {
            match &mut pass.pattern {
                Pattern::ContourFill { pitch_mm, .. }
                | Pattern::Hatch { pitch_mm, .. }
                | Pattern::CrossHatch { pitch_mm, .. }
                | Pattern::Dots { pitch_mm, .. } => {
                    *pitch_mm = pitch_mm.max(min_pitch);
                }
                Pattern::BoundaryHatch {
                    pitch_mm,
                    shell_pitch_mm,
                    ..
                } => {
                    *pitch_mm = pitch_mm.max(min_pitch);
                    *shell_pitch_mm = shell_pitch_mm.max(min_pitch);
                }
                Pattern::Passthrough { .. }
                | Pattern::BoundaryTrace { .. }
                | Pattern::Centreline { .. } => {}
            }
        }
    }
}

fn region_of(ev: &lvp_core::eval::Evaluation, id: &str) -> Region {
    match &ev.layers[&LayerId::new(id)].geometry {
        Geometry::Region(r) => r.clone(),
        other => panic!("{id} is not a region: {other:?}"),
    }
}

fn sample_points(p: &Pline) -> Vec<Point> {
    let mut pts = Vec::new();
    for i in 0..p.vertex_count() {
        let v = p.at(i);
        pts.push(Point::new(v.x, v.y));
        if let Some(next) = p.iter_segments().nth(i) {
            let (mid, _) = pline::seg_midpoint_and_left_normal(next.0, next.1);
            pts.push(mid);
        }
    }
    pts
}

#[test]
fn negative_preset_hatches_only_the_background() {
    let sources = fixture("board-islands.dxf");
    let mut profile = pcb_negative_hatch();
    coarsen(&mut profile, 0.2);
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    assert!(matches!(
        ev.bindings[&SourceId::new("src-outline")],
        BindingStatus::Matched(0, _)
    ));

    let board = region_of(&ev, "wl-board");
    let copper = region_of(&ev, "wl-copper");
    let clearance = region_of(&ev, "wl-clearance");
    let background = region_of(&ev, "wl-background");
    // The outline is 25 x 11.3 mm with 1 mm rounded corners (bulges are written to the DXF
    // with six decimals, hence the tolerance); the clearance grows the copper and lies
    // inside the board.
    let board_area = 25.0 * 11.3 - (4.0 - std::f64::consts::PI);
    assert!((board.area() - board_area).abs() < 1e-4, "{}", board.area());
    assert!(clearance.area() > copper.area());
    assert!(
        (background.area() - (board.area() - clearance.area())).abs() < 1e-6,
        "background {} board {} clearance {}",
        background.area(),
        board.area(),
        clearance.area()
    );
    // Ten copper islands become ten holes; the annulus interior is a second background island.
    assert_eq!(background.outers().count(), 2);
    assert_eq!(background.holes().count(), 10);

    let hatch = ev.pass(&PassId::new("pass-background-hatch")).unwrap();
    assert!(hatch.paths.len() > 50, "{}", hatch.paths.len());
    assert!(hatch.paths.iter().all(|p| !p.is_closed()));
    // Hatch endpoints lie exactly on the board boundary, so test against a board grown by a
    // hair; copper is 50 um away from any hatch point.
    let board_grown = board.offset(1e-6);
    for path in &hatch.paths {
        for pt in sample_points(path) {
            assert!(!copper.contains(pt), "hatch touches copper at {pt:?}");
            assert!(board_grown.contains(pt), "hatch leaves the board at {pt:?}");
        }
    }

    let (paths, diags) = collect_paths(&profile, &ev);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(paths.iter().filter(|p| p.color == Rgb::RED).count(), 1);
    assert_eq!(
        paths.iter().filter(|p| p.color == Rgb::BLUE).count(),
        hatch.paths.len()
    );
}

#[test]
fn general_marking_preset_marks_lines_and_fills_shapes() {
    let sources = fixture("open-linework.dxf");
    let mut profile = general_marking();
    coarsen(&mut profile, 0.2);
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());

    // MARK: seven open paths pass through untouched (the almost-closed square stays open).
    let marks = ev.pass(&PassId::new("pass-marks")).unwrap();
    assert_eq!(marks.paths.len(), 7);
    assert!(marks.paths.iter().all(|p| !p.is_closed()));

    // FILL: one closed shell plus open hatch lines, all inside the 10 x 4 mm rectangle.
    let fill = ev.pass(&PassId::new("pass-fill")).unwrap();
    let closed = fill.paths.iter().filter(|p| p.is_closed()).count();
    let open = fill.paths.len() - closed;
    assert_eq!(closed, 1);
    assert!(open > 10, "{open}");
    let rect = region_of(&ev, "wl-fill").offset(1e-6);
    for path in &fill.paths {
        for pt in sample_points(path) {
            assert!(rect.contains(pt), "fill leaves the shape at {pt:?}");
        }
    }
    let (paths, _) = collect_paths(&profile, &ev);
    assert_eq!(paths.iter().filter(|p| p.color == Rgb::BLACK).count(), 7);
}

#[test]
fn boundary_cutting_preset_traces_outline_holes_first() {
    // The fixture outline matches the first rule.
    let sources = fixture("board-islands.dxf");
    let profile = boundary_cutting();
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let cut = ev.pass(&PassId::new("pass-cut")).unwrap();
    assert_eq!(cut.paths.len(), 1);
    assert!(cut.paths[0].is_closed());
    // Rounded corners of 1 mm, bulges rounded to six decimals in the file.
    let perimeter = 2.0 * (25.0 + 11.3) - 8.0 + 2.0 * std::f64::consts::PI;
    assert!(
        (pline::length(&cut.paths[0]) - perimeter).abs() < 1e-4,
        "{}",
        pline::length(&cut.paths[0])
    );

    // A "CUT" layer matches the second rule; the inner cut-out is emitted before the outer.
    let mut d = Document::new(Unit::Millimetre);
    d.layer("CUT", 1);
    d.add(
        "CUT",
        Entity::rect(Point::new(0.0, 0.0), Point::new(20.0, 10.0)),
    );
    d.add(
        "CUT",
        Entity::rect(Point::new(5.0, 3.0), Point::new(8.0, 7.0)),
    );
    let mut sources = Sources::default();
    sources.add(
        "part.dxf",
        read_str(&d.to_dxf(), &ReadOptions::default()).unwrap(),
    );
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    assert_eq!(
        ev.bindings[&SourceId::new("src-cut")],
        BindingStatus::Matched(0, "CUT".into())
    );
    let cut = ev.pass(&PassId::new("pass-cut")).unwrap();
    assert_eq!(cut.paths.len(), 2);
    let inner = pline::bounds(&cut.paths[0]).unwrap();
    let outer = pline::bounds(&cut.paths[1]).unwrap();
    assert!(inner.max.x - inner.min.x < outer.max.x - outer.min.x);
    assert!((pline::length(&cut.paths[1]) - 60.0).abs() < 1e-6);

    // Nothing matching: the required slot is missing and output is blocked, not guessed.
    let ev = Evaluator::new().evaluate(&profile, &fixture("nested-holes.dxf"));
    assert_eq!(
        ev.bindings[&SourceId::new("src-cut")],
        BindingStatus::Missing
    );
    assert!(ev.has_errors());
    assert!(
        ev.pass(&PassId::new("pass-cut"))
            .is_none_or(|p| p.paths.is_empty())
    );
}

fn heater(files: &[&str]) -> Sources {
    let mut s = Sources::default();
    for f in files {
        let path = format!(
            "{}/../../fixtures/designs/heater/{f}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).unwrap();
        s.add(*f, read_any(f, &bytes, &AnyOptions::default()).unwrap());
    }
    s
}

fn errors(ev: &lvp_core::eval::Evaluation) -> Vec<String> {
    ev.layers
        .values()
        .flat_map(|l| &l.diagnostics)
        .chain(&ev.diagnostics)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

#[test]
fn a_kicad_profile_drawn_in_pieces_is_healed_for_cutting_and_inversion() {
    // KiCad 10 writes the heater's outline as eight separate draws, arcs all clockwise and
    // two joints 9 nm apart: the presets' heal layers close it into one loop.
    let sources = heater(&["heater-Edge_Cuts.gm1", "heater-F_Cu.gtl"]);
    let profile = boundary_cutting();
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let Geometry::Linework(outline) = &ev.layers[&LayerId::new("wl-outline")].geometry else {
        panic!()
    };
    assert_eq!(outline.len(), 1);
    assert!(outline[0].is_closed());
    let cut = region_of(&ev, "wl-cut");
    assert_eq!((cut.outers().count(), cut.holes().count()), (1, 0));
    assert!((cut.area() - outline[0].area().abs()).abs() < 1e-6);
    // A disc of radius 25.5 mm less its cap above y = 24.31 (2030.6 mm²), with a 14 mm
    // wide tab up to y = 36.5 (170.7 mm², less 0.4 mm² of rounded corners).
    assert!((cut.area() - 2201.1).abs() < 0.5, "{}", cut.area());
    let trace = &ev.pass(&PassId::new("pass-cut")).unwrap().paths;
    assert_eq!(trace.len(), 1);
    assert!(trace[0].is_closed());

    // Without healing, region reading joins only the exact joints and names the gap.
    let mut unhealed = profile.clone();
    unhealed.working_layers[0].heal = None;
    let ev = Evaluator::new().evaluate(&unhealed, &sources);
    let open = ev.layers[&LayerId::new("wl-cut")]
        .diagnostics
        .iter()
        .find(|d| d.code == "eval.open_linework_in_region")
        .unwrap();
    assert!(
        open.message.starts_with(
            "2 open path(s) cannot bound a region (the nearest two loose ends are 9.00 nm apart)"
        ),
        "{}",
        open.message
    );
    assert_eq!(ev.layers[&LayerId::new("wl-cut")].loose_ends.len(), 4);

    // The negative preset's board interior is the same healed outline.
    let mut negative = pcb_negative_hatch();
    coarsen(&mut negative, 0.2);
    let ev = Evaluator::new().evaluate(&negative, &sources);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert!((region_of(&ev, "wl-board").area() - cut.area()).abs() < 1e-6);
    let outline = &ev.pass(&PassId::new("pass-outline")).unwrap().paths;
    assert_eq!(outline.len(), 1);
}
