use anyhow::{Result, bail};

use hypercolor_types::config::RenderAccelerationMode;

const GPU_COMPOSITOR_UNAVAILABLE_REASON: &str = "gpu compositor acceleration is not available yet";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositorAccelerationResolution {
    pub(crate) requested_mode: RenderAccelerationMode,
    pub(crate) effective_mode: RenderAccelerationMode,
    pub(crate) fallback_reason: Option<&'static str>,
}

pub(crate) fn resolve_compositor_acceleration_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    match requested_mode {
        RenderAccelerationMode::Cpu => Ok(CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: None,
        }),
        RenderAccelerationMode::Auto => Ok(CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: Some(GPU_COMPOSITOR_UNAVAILABLE_REASON),
        }),
        RenderAccelerationMode::Gpu => {
            bail!("{GPU_COMPOSITOR_UNAVAILABLE_REASON}; use cpu or auto until the GPU lane lands")
        }
    }
}
