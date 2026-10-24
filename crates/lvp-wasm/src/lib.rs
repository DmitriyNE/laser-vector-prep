//! wasm-bindgen surface over `lvp-core`.
//!
//! Keep this layer thin and dumb: flat `f64` buffers in, flat `f64` buffers out, so the
//! Worker can hand results to the main thread as transferable typed arrays. All geometry
//! logic lives (and is tested) in `lvp-core`.

use lvp_core::flatten;
use lvp_core::geom::{BBox, Point};
use wasm_bindgen::prelude::*;

/// Version of the geometry core compiled into this module.
#[wasm_bindgen]
pub fn core_version() -> String {
    lvp_core::VERSION.to_string()
}

/// Flatten an arc into an interleaved `[x0, y0, x1, y1, ...]` buffer (millimetres, radians).
#[wasm_bindgen]
pub fn flatten_arc(
    cx: f64,
    cy: f64,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    tolerance: f64,
) -> Result<Vec<f64>, JsError> {
    let pts = flatten::flatten_arc(Point::new(cx, cy), radius, start_angle, sweep, tolerance)
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(pts.iter().flat_map(|p| [p.x, p.y]).collect())
}

/// Bounding box `[min_x, min_y, max_x, max_y]` of an interleaved point buffer.
/// Returns `None` (JS `undefined`) for an empty buffer.
#[wasm_bindgen]
pub fn bbox_of(points: &[f64]) -> Result<Option<Vec<f64>>, JsError> {
    if !points.len().is_multiple_of(2) {
        return Err(JsError::new("point buffer must have an even length"));
    }
    let (pairs, _) = points.as_chunks::<2>();
    let b = BBox::of_points(pairs.iter().map(|[x, y]| Point::new(*x, *y)));
    Ok(b.map(|b| vec![b.min.x, b.min.y, b.max.x, b.max.y]))
}
