//! Effect engine — renderer traits, orchestration, and registry.
//!
//! This module defines the `EffectRenderer` trait that both the wgpu (native shader)
//! and Servo (HTML/Canvas) rendering backends implement. The `EffectEngine`
//! orchestrates the active effect lifecycle, and the `EffectRegistry` indexes
//! all discovered effects from the filesystem.

pub mod builtin;
mod engine;
mod loader;
mod meta_parser;
mod registry;
#[cfg(feature = "servo")]
mod servo_bootstrap;
mod traits;

pub use engine::EffectEngine;
pub use loader::{HtmlDiscoveryReport, default_effect_search_paths, register_html_effects};
pub use meta_parser::{
    HtmlControlKind, HtmlControlMetadata, ParsedHtmlEffectMetadata, parse_html_effect_metadata,
};
pub use registry::{EffectEntry, EffectRegistry};
#[cfg(feature = "servo")]
pub use servo_bootstrap::bootstrap_software_rendering_context;
pub use traits::{EffectRenderer, FrameInput};
