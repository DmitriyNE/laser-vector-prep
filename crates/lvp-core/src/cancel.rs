//! Cooperative cancellation and progress of long computations.
//!
//! The host and the computation share one [`Control`] block. In the browser it lives in the
//! wasm module's shared memory, so the page writes to it while the geometry worker is busy
//! inside a synchronous call. Each computation runs under its own epoch ([`begin`]); the
//! host cancels one by writing its epoch to `cancel`, before it starts (still queued behind
//! another request) or while it runs. Long loops call [`cancelled`] and stop early; the
//! evaluator then discards what they produced instead of caching it. Outside a computation
//! (and in native tests, which never call [`begin`]) nothing is ever cancelled.
//!
//! Progress counts thousandths of a step: [`steps`] sets the number of steps, [`step`]
//! reports a fraction of the current one and [`next_step`] completes it. It only grows.

use std::sync::atomic::{AtomicU32, Ordering};

/// Shared with the host; the layout is part of the wasm interface (four `u32`s).
#[repr(C)]
pub struct Control {
    /// The computation with this epoch is cancelled.
    pub cancel: AtomicU32,
    /// Epoch of the running computation; 0 when idle.
    pub running: AtomicU32,
    /// Progress of the running computation, in thousandths of a step.
    pub done: AtomicU32,
    pub total: AtomicU32,
}

pub static CONTROL: Control = Control {
    cancel: AtomicU32::new(0),
    running: AtomicU32::new(0),
    done: AtomicU32::new(0),
    total: AtomicU32::new(0),
};

/// Where the current step started, in thousandths.
static STEP_BASE: AtomicU32 = AtomicU32::new(0);

const STEP: u32 = 1000;

/// A computation was stopped by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Start a computation under `epoch` (non-zero, increasing).
pub fn begin(epoch: u32) {
    CONTROL.done.store(0, Ordering::Relaxed);
    CONTROL.total.store(0, Ordering::Relaxed);
    STEP_BASE.store(0, Ordering::Relaxed);
    CONTROL.running.store(epoch, Ordering::Release);
}

/// The computation finished (or stopped).
pub fn end() {
    CONTROL.running.store(0, Ordering::Release);
}

/// Has the host cancelled the running computation?
pub fn cancelled() -> bool {
    let running = CONTROL.running.load(Ordering::Acquire);
    running != 0 && CONTROL.cancel.load(Ordering::Acquire) == running
}

/// `Err(Cancelled)` if the running computation was cancelled.
pub fn check() -> Result<(), Cancelled> {
    if cancelled() { Err(Cancelled) } else { Ok(()) }
}

/// Cancel the computation `epoch` (what the host does; also used by tests).
pub fn cancel(epoch: u32) {
    CONTROL.cancel.store(epoch, Ordering::Release);
}

/// The computation has `n` steps.
pub fn steps(n: u32) {
    CONTROL
        .total
        .store(n.saturating_mul(STEP), Ordering::Relaxed);
    CONTROL.done.store(0, Ordering::Relaxed);
    STEP_BASE.store(0, Ordering::Relaxed);
}

/// The current step is `fraction` (0..=1) done. Safe to call from several threads.
pub fn step(fraction: f64) {
    let base = STEP_BASE.load(Ordering::Relaxed);
    let at = base + (fraction.clamp(0.0, 1.0) * f64::from(STEP)) as u32;
    CONTROL.done.fetch_max(at, Ordering::Relaxed);
}

/// The current step is complete; the next one starts. Safe to call from several threads
/// (steps that run concurrently each complete once).
pub fn next_step() {
    let base = STEP_BASE.fetch_add(STEP, Ordering::Relaxed) + STEP;
    CONTROL.done.fetch_max(base, Ordering::Relaxed);
}

/// Progress of the running computation as (done, total) thousandths of steps.
pub fn progress() -> (u32, u32) {
    (
        CONTROL.done.load(Ordering::Relaxed),
        CONTROL.total.load(Ordering::Relaxed),
    )
}
