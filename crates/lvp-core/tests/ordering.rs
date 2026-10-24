//! Path-order planning on the fixtures: the planner must shorten travel, keep cutting
//! constraints, drive the export order, and report footprint coverage over protected
//! regions.

use lvp_core::eval::{Evaluator, Sources};
use lvp_core::export::collect_paths;
use lvp_core::model::{GroupId, LayerId, OptimizerMode, PassId, Pattern};
use lvp_core::profile::{boundary_cutting, pcb_negative_hatch, pcb_positive_ldi};
use lvp_core::source::{ReadOptions, read_any};

fn sources(names: &[&str]) -> Sources {
    let mut s = Sources::default();
    for name in names {
        let path = format!("{}/../../fixtures/dxf/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap();
        s.add(
            *name,
            read_any(name, &bytes, &ReadOptions::default()).unwrap(),
        );
    }
    s
}

#[test]
fn minimum_jump_shortens_travel_and_orders_the_export() {
    let sources = sources(&["board-islands.dxf"]);
    let mut profile = pcb_positive_ldi();
    if let Pattern::ContourFill { pitch_mm, .. } = &mut profile.working_layers[0].passes[0].pattern
    {
        *pitch_mm = 0.1;
    }
    let mut ev = Evaluator::new();
    let generated = ev.evaluate(&profile, &sources);
    let copper = GroupId::new("grp-copper");
    let plan0 = generated.group_plan(&copper).unwrap();
    assert_eq!(plan0.mode, OptimizerMode::AsGenerated);
    assert_eq!(plan0.before, plan0.after);
    let n = plan0.order.len();
    assert!(n > 50);

    profile.optimizer.mode = OptimizerMode::MinimumJump;
    let planned = ev.evaluate(&profile, &sources);
    let plan = planned.group_plan(&copper).unwrap();
    assert_eq!(
        plan.order.len(),
        n,
        "every path is still emitted exactly once"
    );
    let mut seen: Vec<(String, usize)> = plan
        .order
        .iter()
        .map(|r| (r.pass.to_string(), r.index))
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), n);
    assert!(
        plan.after.jump_length_mm < plan.before.jump_length_mm,
        "{} vs {}",
        plan.after.jump_length_mm,
        plan.before.jump_length_mm
    );
    assert!(plan.after.objective <= plan.before.objective);
    // Contour rings are closed: seams rotate but nothing reverses.
    assert!(plan.order.iter().all(|r| !r.reversed));

    // Export follows the plan: same number of copper paths, first exported path is the
    // first planned one.
    let (paths, diags) = collect_paths(&profile, &planned);
    assert!(diags.is_empty());
    let copper_paths: Vec<_> = paths.iter().filter(|p| p.group == copper).collect();
    assert_eq!(copper_paths.len(), n);
    let first = &plan.order[0];
    let planned_first = lvp_core::order::oriented(
        &planned.pass(&first.pass).unwrap().paths[first.index],
        first,
    );
    let v0 = cavalier_contours::polyline::PlineSource::at(&planned_first, 0);
    assert!(
        (copper_paths[0].points[0].x - v0.x).abs() < 1e-9
            && (copper_paths[0].points[0].y - v0.y).abs() < 1e-9
    );

    // The plan is cached: evaluating again without changes reuses it.
    let again = ev.evaluate(&profile, &sources);
    assert_eq!(again.group_plan(&copper).unwrap().order, plan.order);
}

#[test]
fn cutting_keeps_holes_before_their_outer_under_optimisation() {
    let sources = sources(&["nested-holes.dxf"]);
    let mut profile = boundary_cutting();
    profile.source_bindings[0].rules.insert(
        0,
        lvp_core::model::MatchRule::LayerName {
            format: None,
            name: "SHAPES".into(),
        },
    );
    profile.optimizer.mode = OptimizerMode::MinimumJump;
    // Start far away so the greedy order would otherwise reach the outer square first.
    profile.optimizer.start_mm = Some([-50.0, -50.0]);
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let pass = ev.pass(&PassId::new("pass-cut")).unwrap();
    assert!(
        !pass.precedence.is_empty(),
        "nested holes yield constraints"
    );
    let plan = ev.group_plan(&GroupId::new("grp-cut")).unwrap();
    let pos = |i: usize| plan.order.iter().position(|r| r.index == i).unwrap();
    for &(hole, outer) in &pass.precedence {
        assert!(
            pos(hole) < pos(outer),
            "hole {hole} must precede outer {outer}: {:?}",
            plan.order
        );
    }
    assert!(plan.after.jump_length_mm <= plan.before.jump_length_mm + 1e-9);
}

#[test]
fn bleed_aware_reports_coverage_over_protected_copper() {
    let sources = sources(&["board-islands.dxf"]);
    let mut profile = pcb_negative_hatch();
    for l in &mut profile.working_layers {
        for p in &mut l.passes {
            if let Pattern::Hatch { pitch_mm, .. } = &mut p.pattern {
                *pitch_mm = 0.3;
            }
        }
    }
    profile.optimizer.mode = OptimizerMode::BleedAware;
    profile.optimizer.protected = vec![LayerId::new("wl-copper")];
    profile.optimizer.footprint_mm = 0.2;
    profile.optimizer.cell_mm = 0.05;
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors(), "{:?}", ev.all_diagnostics());
    let plan = ev.group_plan(&GroupId::new("grp-background")).unwrap();
    assert_eq!(plan.mode, OptimizerMode::BleedAware);
    let hm = plan
        .heatmap
        .as_ref()
        .expect("protected copper gives a heatmap");
    assert_eq!(hm.counts.len(), hm.width * hm.height);
    assert!(hm.mask.iter().any(|m| *m != 0));
    // Hatch rows jump across copper islands in generated order; the plan reports less
    // protected coverage and no more travel than it had to.
    let before = plan.before.covered_mm2.unwrap();
    let after = plan.after.covered_mm2.unwrap();
    assert!(before > 0.0);
    assert!(after <= before, "{after} vs {before}");
    assert!(plan.after.objective <= plan.before.objective);
    assert!(plan.after.peak_coverage.is_some());

    // The outline group has no protected overlap: statistics still exist.
    let outline = ev.group_plan(&GroupId::new("grp-outline")).unwrap();
    assert_eq!(outline.after.jumps, 0);
}

#[test]
fn unknown_protected_layer_is_a_warning_not_an_error() {
    let sources = sources(&["board-islands.dxf"]);
    let mut profile = pcb_positive_ldi();
    profile.optimizer.protected = vec![LayerId::new("ghost")];
    let ev = Evaluator::new().evaluate(&profile, &sources);
    assert!(!ev.has_errors());
    assert!(
        ev.diagnostics
            .iter()
            .any(|d| d.code == "order.protected_unknown")
    );
}
