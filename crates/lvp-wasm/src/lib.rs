//! wasm-bindgen surface over `lvp-core`.
//!
//! The worker owns exactly one [`Project`]; every export below is a thin, synchronous
//! adapter around it. Structured data crosses as JSON strings (small), geometry as typed
//! arrays (large, transferable), files as byte vectors. All geometry logic lives (and is
//! tested) in `lvp-core`.
//!
//! The module is compiled with shared memory (see `.cargo/config.toml`). Rayon is backed by
//! `wasm-bindgen-rayon`: the host must `await initThreadPool(n)` once before calling anything
//! that may run in parallel.

use std::cell::RefCell;

use lvp_core::model::{GroupId, LayerId, PassId};
use lvp_core::profile;
use lvp_core::project::{PreviewBuffer, Project};
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Panic messages go to the worker console instead of a bare `unreachable` trap.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

thread_local! {
    static PROJECT: RefCell<Project> = RefCell::new(Project::new());
}

fn with<R>(f: impl FnOnce(&mut Project) -> R) -> R {
    PROJECT.with(|p| f(&mut p.borrow_mut()))
}

fn js<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

fn json<T: serde::Serialize>(v: &T) -> Result<String, JsError> {
    serde_json::to_string(v).map_err(js)
}

/// Version of the geometry core compiled into this module.
#[wasm_bindgen]
pub fn core_version() -> String {
    lvp_core::VERSION.to_string()
}

/// Number of rayon worker threads (requires the thread pool to be initialised).
#[wasm_bindgen]
pub fn thread_count() -> usize {
    rayon::current_num_threads()
}

/// Called once the rayon pool is running: geometry may now fan out to it.
#[wasm_bindgen]
pub fn enable_parallelism() {
    lvp_core::par::enable(true);
}

/// Built-in read-only presets as a JSON array of profiles.
#[wasm_bindgen]
pub fn builtin_presets_json() -> Result<String, JsError> {
    json(&profile::builtin_presets())
}

/// Import a file of any supported format (detected from content and name); returns its
/// `SourceSummary` as JSON.
#[wasm_bindgen]
pub fn add_source(name: &str, bytes: &[u8]) -> Result<String, JsError> {
    with(|p| p.add_source(name, bytes).map_err(js).and_then(|s| json(&s)))
}

#[wasm_bindgen]
pub fn replace_source(index: usize, bytes: &[u8]) -> Result<String, JsError> {
    with(|p| {
        p.replace_source(index, bytes)
            .map_err(js)
            .and_then(|s| json(&s))
    })
}

/// File extensions the app accepts, as a JSON array (for file pickers).
#[wasm_bindgen]
pub fn accepted_extensions_json() -> Result<String, JsError> {
    json(&lvp_core::source::all_extensions())
}

#[wasm_bindgen]
pub fn remove_source(index: usize) {
    with(|p| p.remove_source(index));
}

/// All imported sources as a JSON array of `SourceSummary`.
#[wasm_bindgen]
pub fn sources_json() -> Result<String, JsError> {
    with(|p| json(&p.source_summaries()))
}

/// Replace the active profile. Rejects invalid or newer-schema JSON without changing state.
#[wasm_bindgen]
pub fn set_profile_json(json_text: &str) -> Result<(), JsError> {
    with(|p| p.set_profile_json(json_text).map_err(js))
}

#[wasm_bindgen]
pub fn profile_json() -> String {
    with(|p| p.profile_json())
}

/// Structural diagnostics for a candidate profile, without applying it.
#[wasm_bindgen]
pub fn validate_profile_json(json_text: &str) -> Result<String, JsError> {
    let candidate = profile::parse_profile(json_text).map_err(js)?;
    let diags: Vec<lvp_core::project::DiagnosticJson> = profile::validate(&candidate)
        .iter()
        .map(Into::into)
        .collect();
    json(&diags)
}

/// Evaluate with the current profile and sources; returns the `EvaluationSummary`.
#[wasm_bindgen]
pub fn evaluate() -> Result<String, JsError> {
    with(|p| json(&p.evaluate()))
}

#[wasm_bindgen]
pub fn summary_json() -> Result<Option<String>, JsError> {
    with(|p| p.summary().map(|s| json(&s)).transpose())
}

/// Flattened polylines for drawing. Path `i` spans `coords[2*starts[i] .. 2*starts[i+1]]`.
#[wasm_bindgen]
pub struct Preview {
    inner: PreviewBuffer,
}

#[wasm_bindgen]
impl Preview {
    #[wasm_bindgen(getter)]
    pub fn coords(&self) -> Vec<f32> {
        self.inner.coords.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn bulges(&self) -> Vec<f32> {
        self.inner.bulges.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn starts(&self) -> Vec<u32> {
        self.inner.starts.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn closed(&self) -> Vec<u8> {
        self.inner.closed.clone()
    }
}

impl From<PreviewBuffer> for Preview {
    fn from(inner: PreviewBuffer) -> Self {
        Self { inner }
    }
}

#[wasm_bindgen]
pub fn preview_pass(id: &str) -> Option<Preview> {
    with(|p| p.preview_pass(&PassId::new(id)).map(Into::into))
}

#[wasm_bindgen]
pub fn preview_layer(id: &str) -> Option<Preview> {
    with(|p| p.preview_layer(&LayerId::new(id)).map(Into::into))
}

#[wasm_bindgen]
pub fn preview_source(file: usize, layer: &str) -> Option<Preview> {
    with(|p| p.preview_source(file, layer).map(Into::into))
}

/// Hypothetical jumps of an output group's planned order (preview only, never exported).
#[wasm_bindgen]
pub fn preview_jumps(group_id: &str) -> Option<Preview> {
    with(|p| p.preview_jumps(&GroupId::new(group_id)).map(Into::into))
}

/// Jump footprint coverage raster over the protected regions of a group.
#[wasm_bindgen]
pub struct HeatmapBuffer {
    origin_x: f64,
    origin_y: f64,
    cell_mm: f64,
    width: u32,
    height: u32,
    counts: Vec<u16>,
    mask: Vec<u8>,
}

#[wasm_bindgen]
impl HeatmapBuffer {
    #[wasm_bindgen(getter)]
    pub fn origin_x(&self) -> f64 {
        self.origin_x
    }
    #[wasm_bindgen(getter)]
    pub fn origin_y(&self) -> f64 {
        self.origin_y
    }
    #[wasm_bindgen(getter)]
    pub fn cell_mm(&self) -> f64 {
        self.cell_mm
    }
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }
    #[wasm_bindgen(getter)]
    pub fn counts(&self) -> Vec<u16> {
        self.counts.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn mask(&self) -> Vec<u8> {
        self.mask.clone()
    }
}

#[wasm_bindgen]
pub fn heatmap(group_id: &str) -> Option<HeatmapBuffer> {
    with(|p| {
        p.heatmap(&GroupId::new(group_id)).map(|h| HeatmapBuffer {
            origin_x: h.origin.x,
            origin_y: h.origin.y,
            cell_mm: h.cell_mm,
            width: h.width as u32,
            height: h.height as u32,
            counts: h.counts.clone(),
            mask: h.mask.clone(),
        })
    })
}

/// Colour-separated PDF of every exporting group.
#[wasm_bindgen]
pub fn export_pdf() -> Result<Vec<u8>, JsError> {
    with(|p| p.export_pdf().map_err(js))
}

#[wasm_bindgen]
pub struct GroupPdf {
    group: String,
    name: String,
    bytes: Vec<u8>,
}

#[wasm_bindgen]
impl GroupPdf {
    #[wasm_bindgen(getter)]
    pub fn group(&self) -> String {
        self.group.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

/// SVG with one Inkscape layer per exporting group (same page as the PDF).
#[wasm_bindgen]
pub fn export_svg() -> Result<Vec<u8>, JsError> {
    with(|p| p.export_svg().map_err(js))
}

/// DXF with one layer per exporting group, arcs preserved, model coordinates.
#[wasm_bindgen]
pub fn export_dxf() -> Result<Vec<u8>, JsError> {
    with(|p| p.export_dxf().map_err(js))
}

/// One PDF per exporting group on a shared page layout.
#[wasm_bindgen]
pub fn export_pdf_per_group() -> Result<Vec<GroupPdf>, JsError> {
    with(|p| {
        p.export_pdf_per_group().map_err(js).map(|files| {
            files
                .into_iter()
                .map(|f| GroupPdf {
                    group: f.group.0,
                    name: f.name,
                    bytes: f.bytes,
                })
                .collect()
        })
    })
}
