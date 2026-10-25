//! Replays every saved fuzz crasher (`fixtures/fuzz/<target>/*`) through the entry point
//! that found it. Each must finish without panicking and within a generous time limit
//! (debug build; the original failures were panics, hangs and runaway allocations).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(20);

#[test]
fn saved_crashers_no_longer_fail() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/fuzz");
    let mut failures = Vec::new();
    let mut replayed = 0;
    for target in lvp_core::fuzz::TARGETS {
        let Ok(dir) = std::fs::read_dir(format!("{root}/{target}")) else {
            continue;
        };
        let mut files: Vec<_> = dir.map(|e| e.unwrap().path()).collect();
        files.sort();
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            let start = Instant::now();
            let outcome = catch_unwind(AssertUnwindSafe(|| lvp_core::fuzz::run(target, &bytes)));
            let elapsed = start.elapsed();
            let name = format!("{target}/{}", path.file_name().unwrap().to_string_lossy());
            replayed += 1;
            if outcome.is_err() {
                failures.push(format!("{name}: panicked"));
            } else if elapsed > LIMIT {
                failures.push(format!("{name}: took {elapsed:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{replayed} replayed, failures:\n{}",
        failures.join("\n")
    );
}
