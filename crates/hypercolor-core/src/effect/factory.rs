//! Effect renderer factory.

use anyhow::{Context, Result, bail};

use hypercolor_types::effect::{EffectMetadata, EffectSource};

use super::builtin::create_builtin_renderer;
use super::traits::EffectRenderer;

/// Build a renderer instance for the provided effect metadata.
///
/// # Errors
///
/// Returns an error when the effect source has no runnable renderer path.
pub fn create_renderer_for_metadata(metadata: &EffectMetadata) -> Result<Box<dyn EffectRenderer>> {
    match &metadata.source {
        EffectSource::Native { .. } => create_builtin_renderer(&metadata.name).with_context(|| {
            format!(
                "native effect '{}' is registered but has no built-in renderer implementation",
                metadata.name
            )
        }),
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
    Box::new(super::servo_renderer::ServoRenderer::new())
}

#[cfg(not(feature = "servo"))]
fn create_html_renderer(metadata: &EffectMetadata) -> Result<Box<dyn EffectRenderer>> {
    bail!(
        "html effect '{}' requires the `servo` feature in hypercolor-core",
        metadata.name
    )
}
