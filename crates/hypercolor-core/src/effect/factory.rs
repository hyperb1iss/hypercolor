//! Effect renderer factory.

use anyhow::{Context, Result, bail};

use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::effect::{EffectMetadata, EffectSource};

use super::builtin::create_builtin_renderer;
use super::traits::EffectRenderer;

const GPU_UNAVAILABLE_REASON: &str = "gpu effect renderer acceleration is not available yet";

/// Resolved effect renderer acceleration mode for the current runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectRendererAccelerationResolution {
    /// User-requested effect renderer acceleration mode.
    pub requested_mode: RenderAccelerationMode,
    /// Effective mode the effect renderer runtime can actually provide.
    pub effective_mode: RenderAccelerationMode,
    /// Why the runtime had to fall back, if it did.
    pub fallback_reason: Option<&'static str>,
}

/// Resolve the requested effect renderer acceleration mode against available capabilities.
///
/// # Errors
///
/// Returns an error when the caller explicitly requires GPU acceleration.
pub fn resolve_effect_renderer_acceleration_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<EffectRendererAccelerationResolution> {
    match requested_mode {
        RenderAccelerationMode::Cpu => Ok(EffectRendererAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: None,
        }),
        RenderAccelerationMode::Auto => Ok(EffectRendererAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: Some(GPU_UNAVAILABLE_REASON),
        }),
        RenderAccelerationMode::Gpu => {
            bail!("{GPU_UNAVAILABLE_REASON}; use cpu or auto until the GPU lane lands")
        }
    }
}

/// Build a renderer instance for the provided effect metadata.
///
/// # Errors
///
/// Returns an error when the effect source has no runnable renderer path.
pub fn create_renderer_for_metadata(metadata: &EffectMetadata) -> Result<Box<dyn EffectRenderer>> {
    create_renderer_for_metadata_with_effect_acceleration(metadata, RenderAccelerationMode::Cpu)
}

/// Build a renderer instance for the provided effect metadata and acceleration request.
///
/// # Errors
///
/// Returns an error when the requested acceleration mode is unsupported or
/// when the effect source has no runnable renderer path.
pub fn create_renderer_for_metadata_with_effect_acceleration(
    metadata: &EffectMetadata,
    requested_mode: RenderAccelerationMode,
) -> Result<Box<dyn EffectRenderer>> {
    let _resolution = resolve_effect_renderer_acceleration_mode(requested_mode)?;
    create_renderer_for_metadata_internal(metadata)
}

fn create_renderer_for_metadata_internal(
    metadata: &EffectMetadata,
) -> Result<Box<dyn EffectRenderer>> {
    match &metadata.source {
        EffectSource::Native { .. } => {
            let native_key = metadata
                .source
                .source_stem()
                .unwrap_or(metadata.name.as_str());
            create_builtin_renderer(native_key).with_context(|| {
                format!(
                    "native effect '{}' is registered but has no built-in renderer implementation",
                    metadata.name
                )
            })
        }
        EffectSource::Html { .. } => {
            #[cfg(feature = "servo")]
            {
                Ok(create_html_renderer(metadata))
            }

            #[cfg(not(feature = "servo"))]
            {
                create_html_renderer(metadata)
            }
        }
        EffectSource::Shader { path } => bail!(
            "shader effect '{}' is not runnable yet (source: {})",
            metadata.name,
            path.display()
        ),
    }
}

#[cfg(feature = "servo")]
fn create_html_renderer(_metadata: &EffectMetadata) -> Box<dyn EffectRenderer> {
    Box::new(super::servo::ServoRenderer::new())
}

#[cfg(not(feature = "servo"))]
fn create_html_renderer(metadata: &EffectMetadata) -> Result<Box<dyn EffectRenderer>> {
    bail!(
        "html effect '{}' requires the `servo` feature in hypercolor-core",
        metadata.name
    )
}
