//! Parallel evaluation must not change results: layers, passes, coverage audits and path
//! plans come out identical with parallelism off and on any number of threads. The switch
//! that turns parallelism off is process-global, so this is its own test binary.

use lvp_core::dxf::write::{Document, Entity, Layer, Placed};
use lvp_core::eval::{Evaluation, Evaluator, Sources};
use lvp_core::geom::Point;
use lvp_core::model::{MatchRule, OptimizerMode, Profile};
use lvp_core::profile::{boundary_cutting, pcb_positive_ldi};
use lvp_core::source::{ReadOptions, read_any};

/// Everything an evaluation produced, as text: geometry, paths, audits, plans, diagnostics.
fn fingerprint(ev: &Evaluation) -> String {
    let layers: Vec<String> = ev
        .layers
        .iter()
        .map(|(id, l)| format!("{id} {:?} {:?}", l.geometry, l.diagnostics))
        .collect();
    let passes: Vec<String> = ev
        .passes
        .iter()
        .map(|(id, p)| {
            format!(
                "{id} {:?} {:?} {:?} {:?}",
                p.paths, p.precedence, p.audit, p.diagnostics
            )
        })
        .collect();
    format!(
        "{layers:?}\n{passes:?}\n{:?}\n{:?}",
        ev.groups, ev.diagnostics
    )
}

/// The evaluation without a thread pool and on pools of 1, 3 and 8 threads.
fn agree(profile: &Profile, sources: &Sources) {
    lvp_core::par::enable(false);
    let sequential = fingerprint(&Evaluator::new().evaluate(profile, sources));
    lvp_core::par::enable(true);
    for threads in [1, 3, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let parallel = pool.install(|| fingerprint(&Evaluator::new().evaluate(profile, sources)));
        assert!(
            parallel == sequential,
            "{threads} threads differ from the sequential evaluation"
        );
    }
}

fn source(name: &str, bytes: &[u8], sources: &mut Sources) {
    sources.add(
        name,
        read_any(name, bytes, &ReadOptions::default()).unwrap(),
    );
}

#[test]
fn evaluation_does_not_depend_on_the_number_of_threads() {
    // The board's Gerber pair stepped 2 × 2: several islands, a protected keep-clear layer,
    // a coverage audit and bleed-aware planning.
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/gerber");
    let mut sources = Sources::default();
    for name in ["board-F_Cu.gbr", "board-Edge_Cuts.gbr"] {
        let text = std::fs::read_to_string(format!("{root}/{name}")).unwrap();
        let text = text
            .replacen("G75*\n", "G75*\n%SRX2Y2I26J12*%\n", 1)
            .replace("M02*", "%SR*%\nM02*");
        source(name, text.as_bytes(), &mut sources);
    }
    let mut profile = pcb_positive_ldi();
    profile.optimizer.mode = OptimizerMode::BleedAware;
    agree(&profile, &sources);

    // Cutting a grid of plates with two holes each: precedence constraints on every move.
    let mut entities = Vec::new();
    for i in 0..8 {
        for j in 0..8 {
            let (x, y) = (i as f64 * 10.0 + (j % 3) as f64 * 0.7, j as f64 * 10.0);
            for (a, b) in [
                ((0.0, 0.0), (8.0, 8.0)),
                ((1.0, 1.0), (3.0, 3.0)),
                ((4.0, 4.5), (7.0, 6.5)),
            ] {
                entities.push(Placed {
                    layer: "SHAPES".into(),
                    entity: Entity::rect(
                        Point::new(x + a.0, y + a.1),
                        Point::new(x + b.0, y + b.1),
                    ),
                });
            }
        }
    }
    let doc = Document {
        units: lvp_core::units::Unit::Millimetre,
        layers: vec![Layer {
            name: "SHAPES".into(),
            color: 7,
        }],
        blocks: vec![],
        entities,
    };
    let mut sources = Sources::default();
    source("plates.dxf", doc.to_dxf().as_bytes(), &mut sources);
    let mut profile = boundary_cutting();
    profile.source_bindings[0].rules.insert(
        0,
        MatchRule::LayerName {
            format: None,
            name: "SHAPES".into(),
        },
    );
    for (mode, start) in [
        (OptimizerMode::MinimumJump, None),
        (OptimizerMode::BleedAware, Some([-50.0, -50.0])),
    ] {
        profile.optimizer.mode = mode;
        profile.optimizer.start_mm = start;
        agree(&profile, &sources);
    }
}
