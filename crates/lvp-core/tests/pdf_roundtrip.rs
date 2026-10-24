//! A colour-separated PDF written by this app's exporter reads back as one layer per
//! output group, at 1:1 scale, with the stroke width the export declared.

use cavalier_contours::polyline::PlineSource;
use lvp_core::eval::{Evaluator, Sources};
use lvp_core::export::{collect_paths, export_pdf};
use lvp_core::model::{LayerId, Pattern};
use lvp_core::patterns::Geometry;
use lvp_core::pline;
use lvp_core::profile::pcb_positive_ldi;
use lvp_core::source::{Primitive, ReadOptions, read_any};
use lvp_core::units::Unit;

#[test]
fn exported_pdf_reads_back_as_colour_layers() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    ))
    .unwrap();
    let mut sources = Sources::default();
    sources.add(
        "board-islands.dxf",
        read_any(
            "board-islands.dxf",
            text.as_bytes(),
            &ReadOptions::default(),
        )
        .unwrap(),
    );
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.15;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors());
    let (exported, _) = collect_paths(&profile, &ev);
    let (bytes, _) = export_pdf(&profile, &ev).unwrap();

    let doc = read_any("board.pdf", &bytes, &ReadOptions::default()).unwrap();
    assert_eq!(doc.units, Some(Unit::Point));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["#0000ff", "#ff0000"]);
    assert!(
        doc.diagnostics
            .iter()
            .all(|d| d.severity != lvp_core::diag::Severity::Error),
        "{:?}",
        doc.diagnostics
    );

    let copper = &doc.layers[0];
    // Arcs went out as Bézier curves and come back as exact arcs.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.lines().any(|l| l.ends_with(" c")),
        "PDF should contain curve operators"
    );
    let bulged = copper
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Path { pline, .. } if pline.iter_vertexes().any(|v| v.bulge != 0.0)))
        .count();
    assert!(bulged > 0, "arcs survive the PDF round trip");
    let exported_copper = exported
        .iter()
        .filter(|p| p.color == lvp_core::model::Rgb::BLUE)
        .count();
    assert_eq!(copper.primitives.len(), exported_copper);
    for p in &copper.primitives {
        let Primitive::Path { pline, width } = p else {
            panic!("exported strokes come back as paths")
        };
        assert!(pline.is_closed() || pline.vertex_count() >= 2);
        assert!((width.unwrap() - 0.0254).abs() < 1e-6, "{width:?}");
    }
    // 1:1: the copper bounds match the evaluated region within a flattening tolerance.
    let Geometry::Region(region) = &ev.layers[&LayerId::new("wl-copper")].geometry else {
        panic!()
    };
    let rb = region.bounds().unwrap();
    let pb = copper
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Path { pline, .. } => pline::bounds(pline),
            _ => None,
        })
        .reduce(|a, b| a.union(b))
        .unwrap();
    // The page puts the drawing's bounds (the outline starts at the origin) at the export
    // margin, so copper comes back shifted by exactly that margin; rings start half a pitch
    // inside the boundary, so the extent is a little smaller than the region.
    let margin = profile.export.margin_mm;
    assert!(
        pb.min.x - margin > rb.min.x - 1e-6 && pb.min.x - margin < rb.min.x + 0.2,
        "{pb:?} vs {rb:?}"
    );
    assert!(pb.max.x - margin < rb.max.x + 1e-6 && pb.max.x - margin > rb.max.x - 0.2);
    assert!(pb.width() > rb.width() - 0.4);

    let outline = &doc.layers[1];
    assert_eq!(outline.primitives.len(), 1);
    let Primitive::Path { pline, .. } = &outline.primitives[0] else {
        panic!()
    };
    assert!(pline.is_closed());
    let b = pline::bounds(pline).unwrap();
    assert!(
        (b.width() - 25.0).abs() < 1e-3 && (b.height() - 11.3).abs() < 1e-3,
        "{b:?}"
    );
    assert!(
        (b.min.x - margin).abs() < 1e-3 && (b.min.y - margin).abs() < 1e-3,
        "{b:?}"
    );
}
