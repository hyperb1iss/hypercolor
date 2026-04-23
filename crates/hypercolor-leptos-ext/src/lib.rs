#![forbid(unsafe_code)]

extern crate self as hypercolor_leptos_ext;

pub mod utils;
pub use utils::MaybeSend;

#[cfg(feature = "ws-core")]
pub mod ws;

#[cfg(all(feature = "events", target_arch = "wasm32"))]
pub mod events;

#[cfg(all(feature = "canvas", target_arch = "wasm32"))]
pub mod canvas;

#[cfg(all(feature = "raf", target_arch = "wasm32"))]
pub mod raf;

#[cfg(all(feature = "prelude", target_arch = "wasm32"))]
pub mod prelude;

#[cfg(feature = "axum")]
pub mod axum;

#[cfg(feature = "leptos")]
pub mod leptos;
