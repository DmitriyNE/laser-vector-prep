//! Laser Vector Prep geometry core.
//!
//! Pure Rust, no browser or wasm dependencies, so everything here is testable with
//! `cargo test` on the host. All lengths are physical millimetres; all angles are radians
//! measured in document coordinates (counter-clockwise positive, +y up).

pub mod flatten;
pub mod geom;
pub mod units;

/// Crate version, surfaced to the UI so exports can record which core produced them.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
