//! Entry points shared by the fuzz targets (`fuzz/`) and the crasher regression test
//! (`tests/fuzz_regressions.rs`), so a saved crasher replays exactly what found it.
//!
//! Inputs are untrusted bytes. Errors and diagnostics are fine; panics, hangs and unbounded
//! memory are the bugs these look for.

use crate::model::{MatchRule, Pattern, Profile, SourceFormat};
use crate::profile::builtin_presets;
use crate::project::Project;
use crate::source::{ReadOptions, read_as};

/// The fuzz targets, by name: one per reader plus the end-to-end pipeline.
pub const TARGETS: [&str; 7] = ["dxf", "gerber", "svg", "pdf", "gds", "ipc", "pipeline"];

/// Run the named target on `bytes`. Unknown names do nothing.
pub fn run(target: &str, bytes: &[u8]) {
    let format = match target {
        "dxf" => SourceFormat::Dxf,
        "gerber" => SourceFormat::Gerber,
        "svg" => SourceFormat::Svg,
        "pdf" => SourceFormat::Pdf,
        "gds" => SourceFormat::Gds,
        "ipc" => SourceFormat::Ipc2581,
        "pipeline" => return pipeline(bytes),
        _ => return,
    };
    let _ = read_as(format, "fuzz", bytes, &ReadOptions::default());
}

/// Import → evaluate → export. The first byte picks the file name (and so the format
/// detection fallback) and the preset; every profile slot is mapped onto the file's layers
/// in turn so the geometry stages see the input. Pitches are coarsened so that work stays
/// proportional to the input rather than to the preset's 20 µm fill.
pub fn pipeline(bytes: &[u8]) {
    let Some((&selector, file)) = bytes.split_first() else {
        return;
    };
    const NAMES: [&str; 6] = ["f.dxf", "f.gbr", "f.svg", "f.pdf", "f.gds", "f.xml"];
    let name = NAMES[usize::from(selector) % NAMES.len()];
    let mut project = Project::new();
    let Ok(summary) = project.add_source(name, file) else {
        return;
    };
    let layers: Vec<String> = summary.layers.iter().map(|l| l.name.clone()).collect();
    if layers.is_empty() {
        return;
    }
    let presets = builtin_presets();
    let mut profile = presets[usize::from(selector / 8) % presets.len()].clone();
    for (i, binding) in profile.source_bindings.iter_mut().enumerate() {
        binding.rules = vec![MatchRule::FileLayer {
            file: name.into(),
            name: layers[i % layers.len()].clone(),
        }];
    }
    coarsen(&mut profile, 0.1);
    project.set_profile(profile);
    let summary = project.evaluate();
    if summary.has_errors {
        return;
    }
    let _ = project.export_pdf();
    let _ = project.export_svg();
    let _ = project.export_dxf();
}

fn coarsen(profile: &mut Profile, min_pitch: f64) {
    for layer in &mut profile.working_layers {
        for pass in &mut layer.passes {
            match &mut pass.pattern {
                Pattern::ContourFill { pitch_mm, .. }
                | Pattern::Hatch { pitch_mm, .. }
                | Pattern::CrossHatch { pitch_mm, .. }
                | Pattern::Dots { pitch_mm, .. } => *pitch_mm = pitch_mm.max(min_pitch),
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
