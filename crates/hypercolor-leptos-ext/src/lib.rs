#![forbid(unsafe_code)]

extern crate self as hypercolor_leptos_ext;

pub mod utils;
pub use utils::MaybeSend;

#[cfg(feature = "ws-core")]
pub mod ws;

#[cfg(feature = "events")]
pub mod events;

#[cfg(feature = "canvas")]
pub mod canvas;

#[cfg(feature = "raf")]
pub mod raf;

#[cfg(feature = "prelude")]
pub mod prelude;

#[cfg(feature = "axum")]
pub mod axum;

#[cfg(feature = "leptos")]
pub mod leptos;
