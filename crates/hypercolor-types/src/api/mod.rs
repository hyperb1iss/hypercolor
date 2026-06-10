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

pub mod common;
pub mod devices;
pub mod effects;
pub mod scenes;
pub mod zones;

pub use common::Pagination;
