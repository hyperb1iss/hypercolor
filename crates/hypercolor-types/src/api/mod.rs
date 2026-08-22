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
//! - Required fields stay required in both serde and OpenAPI; lockstep
//!   clients update with every wire change.
//! - Everything derives `Clone + PartialEq` so reactive UIs can
//!   equality-gate on whole responses.
//!
//! Scope includes the complete versioned REST surface, so the daemon and
//! clients compile against the same field vocabulary.

pub mod assets;
pub mod attachments;
pub mod capture;
pub mod config;
pub mod controls;
pub mod devices;
pub mod diagnose;
pub mod displays;
pub mod drivers;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod library;
pub mod output;
pub mod scene;
pub mod scenes;
pub mod simulators;
pub mod system;

pub use envelope::{ApiResponse, ListResponse, PageInfo, ResponseMeta};
