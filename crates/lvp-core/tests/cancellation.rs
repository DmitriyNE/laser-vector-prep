//! Cooperative cancellation (spec §12 #19). The control block is process-global, so these
//! tests live in their own test binary and run one after another.

use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use lvp_core::cancel;
use lvp_core::project::Project;
use lvp_core::source::{CANCELLED, ReadOptions, read_any};

static SERIAL: Mutex<()> = Mutex::new(());

/// The board Gerber pair stepped and repeated `nx` × `ny` times.
fn panel(nx: u32, ny: u32) -> [(String, Vec<u8>); 2] {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/gerber");
    ["board-F_Cu.gbr", "board-Edge_Cuts.gbr"].map(|name| {
        let text = std::fs::read_to_string(format!("{root}/{name}")).unwrap();
        let text = text
            .replacen("G75*\n", &format!("G75*\n%SRX{nx}Y{ny}I26J12*%\n"), 1)
            .replace("M02*", "%SR*%\nM02*");
        (name.to_string(), text.into_bytes())
    })
}

fn project(nx: u32, ny: u32) -> Project {
    let mut p = Project::new();
    for (name, bytes) in panel(nx, ny) {
        p.add_source(&name, &bytes).unwrap();
    }
    p
}

#[test]
fn a_cancelled_evaluation_stops_early_and_keeps_the_previous_result() {
    let _guard = SERIAL.lock().unwrap();
    let mut p = project(1, 1);
    p.set_profile(lvp_core::profile::pcb_positive_ldi());
    let first = p.evaluate();

    // Cancelled before it starts (still queued, as the page may do), with a new profile
    // already set: nothing is computed.
    cancel::cancel(7);
    cancel::begin(7);
    p.set_profile(lvp_core::profile::pcb_negative_hatch());
    assert!(p.try_evaluate().is_err());
    cancel::end();
    // The previous evaluation, and the profile it describes, are still what summaries,
    // previews and exports use.
    let json = |s: &lvp_core::project::EvaluationSummary| serde_json::to_string(s).unwrap();
    assert_eq!(json(&p.summary().unwrap()), json(&first));
    let pdf = p.export_pdf().unwrap();
    assert!(!pdf.is_empty());

    // A later computation is not affected by the old cancellation (the original profile
    // again: everything comes from the cache).
    p.set_profile(lvp_core::profile::pcb_positive_ldi());
    cancel::begin(8);
    assert!(p.try_evaluate().is_ok());
    cancel::end();
}

#[test]
fn cancelling_a_running_evaluation_stops_it_promptly() {
    let _guard = SERIAL.lock().unwrap();
    let mut p = project(2, 2);
    let started = Instant::now();
    // Cancel as soon as the first step (the copper layer) is done: mid-run on any machine.
    let canceller = std::thread::spawn(|| {
        while !(cancel::CONTROL.running.load(Ordering::Acquire) == 21
            && cancel::progress().0 >= 1000)
        {
            std::hint::spin_loop();
        }
        cancel::cancel(21);
        Instant::now()
    });
    cancel::begin(21);
    let result = p.try_evaluate();
    let stopped = Instant::now();
    cancel::end();
    let cancelled_at = canceller.join().unwrap();
    assert!(
        result.is_err(),
        "finished in {:?} before the cancel",
        started.elapsed()
    );
    // Debug build: the checks between rings, islands, layers and sweeps keep this short.
    let reaction = stopped.duration_since(cancelled_at);
    assert!(
        reaction < Duration::from_secs(2),
        "took {reaction:?} to stop"
    );
    // Progress was reported while it ran.
    let (done, total) = cancel::progress();
    assert!(total > 0 && done < total, "progress {done}/{total}");

    // Nothing partial was cached: the full evaluation afterwards is complete and identical
    // to one on a fresh project.
    let again = p.evaluate();
    let fresh = project(2, 2).evaluate();
    let paths = |s: &lvp_core::project::EvaluationSummary| -> Vec<usize> {
        s.passes.iter().map(|x| x.paths).collect()
    };
    assert_eq!(paths(&again), paths(&fresh));
}

/// A fine fill cancelled at several moments while its passes run: the pattern, its coverage
/// audit (stroking and uniting thousands of rings) and the plans all notice promptly.
#[test]
fn cancelling_a_fine_fill_stops_promptly_at_any_stage() {
    let _guard = SERIAL.lock().unwrap();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/dxf/board-islands.dxf"
    );
    let mut p = Project::new();
    p.add_source("board-islands.dxf", &std::fs::read(path).unwrap())
        .unwrap();
    let mut profile = lvp_core::profile::pcb_positive_ldi();
    p.set_profile(profile.clone());
    p.evaluate(); // the layers are cached from here on
    let layers_done = 1000 * profile.working_layers.len() as u32;
    for (k, delay) in [0u64, 300, 1000, 2500].into_iter().enumerate() {
        for l in &mut profile.working_layers {
            for pass in &mut l.passes {
                if let lvp_core::model::Pattern::ContourFill { pitch_mm, .. } = &mut pass.pattern {
                    *pitch_mm = 0.002 + k as f64 * 1e-6;
                }
            }
        }
        p.set_profile(profile.clone());
        let epoch = 30 + k as u32;
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let canceller = {
            let finished = finished.clone();
            std::thread::spawn(move || {
                while !(cancel::CONTROL.running.load(Ordering::Acquire) == epoch
                    && cancel::progress().0 >= layers_done)
                {
                    if finished.load(Ordering::Acquire) {
                        return None;
                    }
                    std::hint::spin_loop();
                }
                std::thread::sleep(Duration::from_millis(delay));
                cancel::cancel(epoch);
                Some(Instant::now())
            })
        };
        cancel::begin(epoch);
        let result = p.try_evaluate();
        let stopped = Instant::now();
        finished.store(true, Ordering::Release);
        cancel::end();
        let cancelled_at = canceller.join().unwrap();
        // A fast machine may finish before a late cancel.
        if let (Err(_), Some(at)) = (result, cancelled_at) {
            let reaction = stopped.saturating_duration_since(at);
            assert!(
                reaction < Duration::from_secs(2),
                "{delay} ms in: took {reaction:?} to stop"
            );
        }
    }
}

#[test]
fn a_cancelled_import_is_refused() {
    let _guard = SERIAL.lock().unwrap();
    // Large enough that the reader reaches its periodic check (see `source::Budget`).
    let [(name, bytes), _] = panel(10, 10);
    cancel::cancel(31);
    cancel::begin(31);
    let err = read_any(&name, &bytes, &ReadOptions::default()).unwrap_err();
    cancel::end();
    assert_eq!(err.message, CANCELLED);
    // The cancellation does not outlive its computation.
    let [(name, bytes), _] = panel(1, 1);
    assert!(read_any(&name, &bytes, &ReadOptions::default()).is_ok());
}
