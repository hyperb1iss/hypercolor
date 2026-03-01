//! Effect engine — renderer traits, orchestration, and registry.
//!
//! This module defines the `EffectRenderer` trait that both the wgpu (native shader)
//! and Servo (HTML/Canvas) rendering backends implement. The `EffectEngine`
//! orchestrates the active effect lifecycle, and the `EffectRegistry` indexes
//! all discovered effects from the filesystem.

pub mod builtin;
mod engine;
mod registry;
mod traits;

pub use engine::EffectEngine;
pub use registry::{EffectEntry, EffectRegistry};
pub use traits::{EffectRenderer, FrameInput};
