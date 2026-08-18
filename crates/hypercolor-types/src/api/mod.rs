//! Shared REST API data contracts for the daemon's `/api/v1` surface.
//!
//! One definition per request/response shape, used by the daemon's
//! handlers (serialize + OpenAPI schema) and by every client — the web
//! UI and the TUI deserialize these exact types, so wire drift is a
//! compile error instead of a runtime surprise.
//!
//! Conventions:
//! - Field shapes are the daemon's wire truth (`u32` sizes, required
//!   fields the daemon always sends).
//! - Client-tolerance `#[serde(default)]`s are kept where they don't
//!   change what the daemon serializes.
//! - Everything derives `Clone + PartialEq` so reactive UIs can
//!   equality-gate on whole responses.
//!
//! Scope: the state-bearing and mutating contracts live here. Diagnostic
//! telemetry (system status internals, metrics payloads) deliberately
//! does NOT — those shapes move fast with perf work, and clients consume
//! tolerant subsets of them by design.

pub mod assets;
pub mod attachments;
pub mod common;
pub mod config;
pub mod controls;
pub mod devices;
pub mod diagnose;
pub mod displays;
pub mod effects;
pub mod envelope;
pub mod layers;
pub mod layouts;
pub mod library;
pub mod output;
pub mod profiles;
pub mod scene;
pub mod scenes;
pub mod simulators;
pub mod zones;

pub use common::Pagination;
