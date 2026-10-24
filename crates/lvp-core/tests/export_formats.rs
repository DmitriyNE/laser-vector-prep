//! SVG and DXF exports read back through the app's own readers: one layer per output group,
//! every planned path once, widths and coordinates intact.

use cavalier_contours::polyline::PlineSource;
use lvp_core::eval::{Evaluator, Sources};
use lvp_core::export::{collect_paths, export_dxf, export_svg};
use lvp_core::model::{GroupId, OptimizerMode, Pattern, Rgb};
use lvp_core::pline;
use lvp_core::profile::pcb_positive_ldi;
use lvp_core::source::{Primitive, ReadOptions, read_any};

fn board() -> Sources {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    ))
    .unwrap();
    let mut s = Sources::default();
    s.add(
        "board-islands.dxf",
        read_any(
            "board-islands.dxf",
            text.as_bytes(),
            &ReadOptions::default(),
        )
        .unwrap(),
    );
    s
}

#[test]
fn svg_export_round_trips_as_inkscape_layers() {
    let sources = board();
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.15;
    }
    profile.optimizer.mode = OptimizerMode::MinimumJump;
    let ev = Evaluator::new().evaluate(&profile, &sources);
    let (paths, _) = collect_paths(&profile, &ev);
    let (bytes, diags) = export_svg(&profile, &ev).unwrap();
    assert!(diags.is_empty());
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.contains(r#"width="35mm""#) && text.contains(r#"inkscape:label="Copper""#),
        "{}",
        &text[..300]
    );

    let doc = read_any("board.svg", &bytes, &ReadOptions::default()).unwrap();
    assert!(
        doc.diagnostics
            .iter()
            .all(|d| d.severity != lvp_core::diag::Severity::Error),
        "{:?}",
        doc.diagnostics
    );
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["Copper", "Outline"]);
    let copper = doc.layer("Copper").unwrap();
    assert_eq!(
        copper.primitives.len(),
        paths.iter().filter(|p| p.color == Rgb::BLUE).count()
    );
    for p in &copper.primitives {
        let Primitive::Path { width, .. } = p else {
            panic!("strokes come back as paths")
        };
        assert!((width.unwrap() - 0.0254).abs() < 1e-6, "{width:?}");
    }
    // The first copper path in the SVG is the first planned path (export order is kept).
    let first_export = &paths.iter().find(|p| p.color == Rgb::BLUE).unwrap().points[0];
    let Primitive::Path { pline, .. } = &copper.primitives[0] else {
        panic!()
    };
    let v = pline.at(0);
    // Model → page: + margin in x; y keeps its orientation after the double flip.
    assert!(
        (v.x - (first_export.x + 5.0)).abs() < 1e-5 && (v.y - (first_export.y + 5.0)).abs() < 1e-5,
        "{v:?} vs {first_export:?}"
    );
    let outline = doc.layer("Outline").unwrap();
    let Primitive::Path { pline, .. } = &outline.primitives[0] else {
        panic!()
    };
    assert!(pline.is_closed());
    let b = pline::bounds(pline).unwrap();
    assert!(
        (b.width() - 25.0).abs() < 1e-4 && (b.height() - 11.3).abs() < 1e-4,
        "{b:?}"
    );
}

#[test]
fn dxf_export_keeps_arcs_and_model_coordinates() {
    let sources = board();
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.15;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    let copper_plan = ev.group_plan(&GroupId::new("grp-copper")).unwrap();
    let (bytes, diags) = export_dxf(&profile, &ev).unwrap();
    assert!(diags.is_empty(), "{diags:?}");

    let doc = read_any("board.dxf", &bytes, &ReadOptions::default()).unwrap();
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    let names: Vec<&str> = doc
        .layers
        .iter()
        .filter(|l| !l.primitives.is_empty())
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(names, vec!["Copper", "Outline"]);
    let copper = doc.layer("Copper").unwrap();
    assert_eq!(copper.primitives.len(), copper_plan.order.len());
    // Rings around round pads keep their arcs: at least one bulged vertex somewhere.
    let bulged = copper
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Path { pline, .. } if pline.iter_vertexes().any(|v| v.bulge != 0.0)))
        .count();
    assert!(bulged > 0, "arcs survive the DXF export");
    let outline = doc.layer("Outline").unwrap();
    let Primitive::Path { pline, width } = &outline.primitives[0] else {
        panic!()
    };
    assert!(
        width.is_none(),
        "DXF carries centrelines without a cosmetic width"
    );
    assert!(pline.is_closed());
    let b = pline::bounds(pline).unwrap();
    // No page offset: the outline sits where the source had it.
    assert!(
        b.min.x.abs() < 1e-6
            && b.min.y.abs() < 1e-6
            && (b.max.x - 25.0).abs() < 1e-6
            && (b.max.y - 11.3).abs() < 1e-6,
        "{b:?}"
    );
    assert_eq!(doc.layer("Copper").unwrap().color, Some(5));
    assert_eq!(doc.layer("Outline").unwrap().color, Some(1));
}

#[test]
fn layer_names_are_made_dxf_safe() {
    assert_eq!(
        lvp_core::export::dxf::layer_name("Copper/Top: fine"),
        "Copper_Top_ fine"
    );
    assert_eq!(lvp_core::export::dxf::layer_name("  "), "GROUP");
    assert_eq!(lvp_core::export::dxf::nearest_aci(Rgb(0, 160, 0)), 3);
    assert_eq!(lvp_core::export::dxf::nearest_aci(Rgb(10, 10, 10)), 7);
}
