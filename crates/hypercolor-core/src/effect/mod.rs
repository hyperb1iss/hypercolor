//! Effect engine — renderer traits, orchestration, and registry.
//!
//! This module defines the `EffectRenderer` trait that both the wgpu (native shader)
//! and Servo (HTML/Canvas) rendering backends implement. The `EffectEngine`
//! orchestrates the active effect lifecycle, and the `EffectRegistry` indexes
//! all discovered effects from the filesystem.

pub mod builtin;
mod engine;
mod factory;
mod lightscript;
mod loader;
mod meta_parser;
mod paths;
mod pool;
mod registry;
#[cfg(feature = "servo")]
mod servo;
#[cfg(feature = "servo")]
mod servo_bootstrap;
mod traits;
pub mod watcher;

pub use engine::EffectEngine;
pub use factory::{
    RenderAccelerationResolution, create_renderer_for_metadata,
    create_renderer_for_metadata_with_mode, resolve_render_acceleration_mode,
};
pub use lightscript::{LightscriptRuntime, control_update_script, normalized_level_to_db};
pub use loader::{HtmlDiscoveryReport, default_effect_search_paths, register_html_effects};
pub use meta_parser::{
    HtmlControlKind, HtmlControlMetadata, ParsedHtmlEffectMetadata, parse_html_effect_metadata,
};
pub use paths::{bundled_effects_root, resolve_html_source_path};
pub use pool::EffectPool;
pub use registry::{EffectEntry, EffectRegistry, RescanReport};
#[cfg(feature = "servo")]
pub use servo::{ConsoleMessage, HypercolorWebViewDelegate, ServoRenderer};
#[cfg(feature = "servo")]
pub use servo_bootstrap::bootstrap_software_rendering_context;
pub use traits::{EffectRenderer, FrameInput};
pub use watcher::{EffectWatchEvent, EffectWatcher};
