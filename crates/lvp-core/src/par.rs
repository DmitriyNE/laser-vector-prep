//! Data parallelism over rayon where a thread pool exists. Natively one always does. In the
//! browser the geometry worker starts the shared-memory pool and then calls [`enable`]; the
//! Node-side wasm tests never start one, and touching rayon there would try to spawn
//! threads, so the same code runs sequentially until enabled.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

static ENABLED: AtomicBool = AtomicBool::new(!cfg!(target_arch = "wasm32"));

/// Allow (or forbid) parallel execution.
pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Threads [`map`] runs on: the pool's, or 1 while parallelism is off.
pub fn threads() -> usize {
    if enabled() {
        rayon::current_num_threads()
    } else {
        1
    }
}

/// `items.map(f)`, in parallel when enabled; results keep the input order.
pub fn map<T, R, F>(items: Vec<T>, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync + Send,
{
    if enabled() && items.len() > 1 {
        items.into_par_iter().map(f).collect()
    } else {
        items.into_iter().map(f).collect()
    }
}
