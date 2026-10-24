//! Import -> evaluate -> export on the board fixture, checked end to end.

use lvp_core::dxf::read::{ReadOptions, read_str};
use lvp_core::eval::{Evaluator, Sources};
use lvp_core::export::{collect_paths, export_pdf, export_pdf_per_group};
use lvp_core::model::{GroupId, Rgb};
use lvp_core::profile::pcb_positive_ldi;

fn board_sources() -> Sources {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    ))
    .unwrap();
    let mut s = Sources::default();
    s.add(
        "board-islands.dxf",
        read_str(&text, &ReadOptions::default()).unwrap(),
    );
    s
}

#[test]
fn board_fixture_exports_colour_separated_pdf() {
    let sources = board_sources();
    let mut profile = pcb_positive_ldi();
    // A coarser pitch keeps the test fast; the geometry semantics are identical.
    if let lvp_core::model::Pattern::ContourFill { pitch_mm, .. } =
        &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.1;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());

    let (paths, diags) = collect_paths(&profile, &ev);
    assert!(diags.is_empty());
    let copper = paths.iter().filter(|p| p.color == Rgb::BLUE).count();
    let outline = paths.iter().filter(|p| p.color == Rgb::RED).count();
    assert!(copper > 50, "copper trajectories {copper}");
    assert_eq!(outline, 1);
    assert!(paths.iter().all(|p| (p.width_mm - 0.0254).abs() < 1e-12));
    // Copper rings never leave the copper bounds; the outline is the board outline.
    for p in paths.iter().filter(|p| p.color == Rgb::BLUE) {
        let b = p.bounds().unwrap();
        assert!(
            b.min.x >= 2.4 - 1e-6
                && b.max.x <= 22.5 + 1e-6
                && b.min.y >= 2.0 - 1e-6
                && b.max.y <= 9.3 + 1e-6
        );
    }

    let (pdf, _) = export_pdf(&profile, &ev).unwrap();
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("0 0 1 RG") && s.contains("1 0 0 RG"));
    assert!(s.contains("0.072 w"));
    assert_eq!(s.matches(" m\n").count(), paths.len());
    // 25 x 11.3 mm outline plus 5 mm margins -> 35 x 21.3 mm page.
    assert!(s.contains(&format!(
        "/MediaBox [0 0 {} {}]",
        lvp_core::export::pdf::num(35.0 * 72.0 / 25.4),
        lvp_core::export::pdf::num(21.3 * 72.0 / 25.4)
    )));

    let (files, _) = export_pdf_per_group(&profile, &ev).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].group, GroupId::new("grp-copper"));
    let media = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .split("/MediaBox ")
            .nth(1)
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        media(&files[0].bytes),
        media(&files[1].bytes),
        "per-group files share one page layout"
    );
}

#[test]
fn stroke_width_change_keeps_geometry() {
    let sources = board_sources();
    let mut profile = pcb_positive_ldi();
    if let lvp_core::model::Pattern::ContourFill { pitch_mm, .. } =
        &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.2;
    }
    let ev = Evaluator::new().evaluate(&profile, &sources);
    let (thin, _) = export_pdf(&profile, &ev).unwrap();
    profile.export.stroke_width_mm = 0.05;
    let (thick, _) = export_pdf(&profile, &ev).unwrap();
    // Compare the content streams minus the width operator: identical centrelines, order
    // and scale; byte offsets may differ only because the operand text does.
    let content_ops = |b: &[u8]| {
        let s = String::from_utf8_lossy(b).to_string();
        let start = s.find("stream\n").unwrap() + 7;
        let end = s.find("endstream").unwrap();
        s[start..end]
            .lines()
            .filter(|l| !l.ends_with(" w"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        content_ops(&thin),
        content_ops(&thick),
        "only the width operator differs"
    );
    assert!(String::from_utf8_lossy(&thick).contains(&format!(
        "{} w",
        lvp_core::export::pdf::num(0.05 * 72.0 / 25.4)
    )));
}

#[test]
fn disabled_groups_and_reference_layers_are_not_exported() {
    let sources = board_sources();
    let mut profile = pcb_positive_ldi();
    if let lvp_core::model::Pattern::ContourFill { pitch_mm, .. } =
        &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.2;
    }
    profile.output_groups[1].export = false;
    let ev = Evaluator::new().evaluate(&profile, &sources);
    let (paths, _) = collect_paths(&profile, &ev);
    assert!(paths.iter().all(|p| p.color == Rgb::BLUE));

    profile.output_groups[1].export = true;
    profile.working_layers[0].reference_only = true;
    let ev = Evaluator::new().evaluate(&profile, &sources);
    let (paths, _) = collect_paths(&profile, &ev);
    assert!(
        paths.iter().all(|p| p.color == Rgb::RED),
        "reference layer emits no marks"
    );
}
