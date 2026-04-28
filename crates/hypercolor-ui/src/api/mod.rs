//! REST API client — thin wrappers around the daemon's HTTP endpoints.
//!
//! Organized by domain: effects, devices, layouts, library (presets/favorites),
//! config, and system status.
#![cfg_attr(test, allow(dead_code, unused_imports))]

use serde::Deserialize;

pub mod client;
pub mod config;
pub mod controls;
pub mod device_metrics;
pub mod devices;
pub mod displays;
pub mod drivers;
pub mod effects;
pub mod layouts;
pub mod library;
pub mod scenes;
pub mod simulators;
pub mod system;

// ── Shared Envelope ─────────────────────────────────────────────────────────

/// Mirrors the daemon's envelope: `{ "data": T, "meta": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
}

// ── Re-exports ──────────────────────────────────────────────────────────────
// Flat re-exports so existing `crate::api::FooBar` imports keep working.

pub use config::*;
#[allow(unused_imports)]
pub use controls::*;
pub use device_metrics::*;
pub use devices::*;
pub use displays::*;
pub use drivers::*;
pub use effects::*;
pub use layouts::*;
pub use library::*;
pub use scenes::*;
pub use simulators::*;
pub use system::*;
