//! REST API client — thin wrappers around the daemon's HTTP endpoints.
//!
//! Organized by domain: effects, devices, layouts, library (presets/favorites),
//! config, and system status.

use std::future::Future;

use leptos::prelude::{Get, LocalResource, expect_context};

use crate::app::WsContext;

pub mod assets;
pub mod client;
pub mod config;
pub mod controls;
pub mod device_metrics;
pub mod devices;
pub mod displays;
pub mod drivers;
pub mod effects;
pub mod http_transport;
pub mod layers;
pub mod layouts;
pub mod library;
pub mod output;
pub mod scenes;
pub mod system;
pub mod zones;

// ── Shared Envelope ─────────────────────────────────────────────────────────

pub fn daemon_resource<T, Fut>(fetcher: impl Fn() -> Fut + 'static) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    let connection_generation = expect_context::<WsContext>().connection_generation;
    LocalResource::new(move || {
        connection_generation.get();
        fetcher()
    })
}

// ── Re-exports ──────────────────────────────────────────────────────────────
// Flat re-exports so existing `crate::api::FooBar` imports keep working.

pub use assets::*;
pub use client::{ApiError, ApiResult, MutationOutcome};
pub use config::*;
pub use controls::*;
pub use device_metrics::*;
pub use devices::*;
pub use displays::*;
pub use drivers::*;
pub use effects::*;
pub use layers::*;
pub use layouts::*;
pub use library::*;
pub use output::*;
pub use scenes::*;
pub use system::*;
// Zone mutations stay under `api::zones` so their revision precondition is
// visible at call sites.
