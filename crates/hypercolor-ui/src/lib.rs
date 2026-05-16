//! Leptos 0.8 CSR WASM web frontend for the Hypercolor lighting engine.
//!
//! Excluded from the Cargo workspace — build with `just ui-dev` (Trunk dev server)
//! or `just ui-build` (production). Communicates with `hypercolor-daemon` over
//! REST and WebSocket using `hypercolor-leptos-ext`.

pub mod control_surface_api;
pub mod control_surface_values;
pub mod control_surface_view;
pub mod label_utils;
pub mod tauri_bridge;
