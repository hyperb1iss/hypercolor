//! REST API client — thin wrappers around the daemon's HTTP endpoints.
//!
//! Organized by domain: effects, devices, layouts, library (presets/favorites),
//! config, and system status.
#![allow(dead_code)] // API surface is pre-built for upcoming features
#![cfg_attr(test, allow(unused_imports))]

use serde::Deserialize;

pub mod client;
pub mod config;
pub mod devices;
pub mod effects;
pub mod layouts;
pub mod library;
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
pub use devices::*;
pub use effects::*;
pub use layouts::*;
pub use library::*;
pub use system::*;
