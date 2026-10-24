//! Two-group subtraction on the board fixture: copper minus a band around the outline.

use lvp_core::dxf::read::{ReadOptions, read_str};
use lvp_core::eval::{Evaluator, Sources};
use lvp_core::model::*;
use lvp_core::patterns::Geometry;
use lvp_core::profile::pcb_positive_ldi;

#[test]
fn copper_minus_outline_band_keeps_copper() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    ))
    .unwrap();
    let mut sources = Sources::default();
    sources.add_dxf(
        "board-islands.dxf",
        read_str(&text, &ReadOptions::default()).unwrap(),
    );
    let mut profile = pcb_positive_ldi();
    // Outline as a 1 mm band (stroke of the closed outline), copper as filled regions.
    profile.working_layers.push(WorkingLayer {
        id: LayerId::new("wl-band"),
        name: "band".into(),
        kind: GeometryKind::Region,
        inputs: vec![Operand::Source(SourceId::new("src-outline"))],
        operands: vec![],
        op: LayerOp::Union,
        envelope: None,
        interpretation: Interpretation {
            loop_rule: LoopRuleCfg::MergeThenNest,
            stroke_width: StrokeWidth::Fixed { mm: 1.0 },
        },
        passes: vec![],
        enabled: true,
        reference_only: true,
    });
    profile.working_layers.push(WorkingLayer {
        id: LayerId::new("wl-diff"),
        name: "copper minus band".into(),
        kind: GeometryKind::Region,
        inputs: vec![Operand::Layer(LayerId::new("wl-copper"))],
        operands: vec![Operand::Layer(LayerId::new("wl-band"))],
        op: LayerOp::Subtraction,
        envelope: None,
        interpretation: Interpretation::default(),
        passes: vec![],
        enabled: true,
        reference_only: false,
    });
    let e = Evaluator::new().evaluate(&profile, &sources);
    let area = |id: &str| match &e.layers[&LayerId::new(id)].geometry {
        Geometry::Region(r) => (r.area(), r.outers().count(), r.holes().count()),
        Geometry::Linework(_) => (0.0, 0, 0),
    };
    let copper = area("wl-copper");
    let band = area("wl-band");
    let diff = area("wl-diff");
    eprintln!("copper {copper:?} band {band:?} diff {diff:?}");
    assert!(
        band.0 > 30.0 && band.1 == 1 && band.2 == 1,
        "band is a ring: {band:?}"
    );
    assert!(
        (diff.0 - copper.0).abs() < 1e-6,
        "copper untouched by the band: {diff:?} vs {copper:?}"
    );
    assert_eq!(diff.1, copper.1);
}

#[test]
fn stroke_of_closed_outline_is_a_ring() {
    use cavalier_contours::polyline::PlineSource;
    use lvp_core::dxf::read::Primitive;
    use lvp_core::region::{self, Region};
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    ))
    .unwrap();
    let doc = read_str(&text, &ReadOptions::default()).unwrap();
    let outline = doc
        .layer("ROUTE_TOOL_PATH")
        .unwrap()
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Path { pline, .. } => Some(pline.clone()),
            _ => None,
        })
        .unwrap();
    eprintln!(
        "outline: closed={} vertices={} area={}",
        outline.is_closed(),
        outline.vertex_count(),
        outline.area()
    );
    for v in outline.iter_vertexes() {
        eprintln!("  v ({:.6}, {:.6}) bulge {:.9}", v.x, v.y, v.bulge);
    }
    let band = region::stroke(&outline, 1.0);
    eprintln!(
        "stroke: area={} outers={} holes={}",
        band.area(),
        band.outers().count(),
        band.holes().count()
    );
    let via_union = Region::empty().union(&band);
    eprintln!(
        "empty ∪ stroke: area={} outers={} holes={}",
        via_union.area(),
        via_union.outers().count(),
        via_union.holes().count()
    );
    let expected = 2.0 * (25.0 + 11.3) * 1.0 + std::f64::consts::PI * (1.5 * 1.5 - 0.5 * 0.5)
        - 4.0 * 2.0 * 1.0; // rough: perimeter band
    eprintln!("expected roughly {expected}");
    assert_eq!(band.holes().count(), 1);
    assert_eq!(via_union.holes().count(), 1);
}
