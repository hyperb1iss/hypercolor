#[cfg(feature = "wgpu")]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(feature = "wgpu"))]
use anyhow::bail;

use hypercolor_types::config::RenderAccelerationMode;

const GPU_COMPOSITOR_UNAVAILABLE_REASON: &str = "gpu compositor acceleration is not available yet";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuCompositorProbeInfo {
    pub(crate) adapter_name: String,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompositorAccelerationResolution {
    pub(crate) requested_mode: RenderAccelerationMode,
    pub(crate) effective_mode: RenderAccelerationMode,
    pub(crate) fallback_reason: Option<&'static str>,
    pub(crate) gpu_probe: Option<GpuCompositorProbeInfo>,
}

pub(crate) fn resolve_compositor_acceleration_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    match requested_mode {
        RenderAccelerationMode::Cpu => Ok(CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: None,
            gpu_probe: None,
        }),
        RenderAccelerationMode::Auto => resolve_auto_mode(requested_mode),
        RenderAccelerationMode::Gpu => resolve_explicit_gpu_mode(requested_mode),
    }
}

#[cfg(feature = "wgpu")]
fn resolve_auto_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    if let Ok(compositor) = crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger::new() {
        return Ok(CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Gpu,
            fallback_reason: None,
            gpu_probe: Some(GpuCompositorProbeInfo::from(compositor.describe())),
        });
    }

    Ok(CompositorAccelerationResolution {
        requested_mode,
        effective_mode: RenderAccelerationMode::Cpu,
        fallback_reason: None,
        gpu_probe: None,
    })
}

#[cfg(not(feature = "wgpu"))]
fn resolve_auto_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    Ok(CompositorAccelerationResolution {
        requested_mode,
        effective_mode: RenderAccelerationMode::Cpu,
        fallback_reason: None,
        gpu_probe: None,
    })
}

#[cfg(feature = "wgpu")]
fn resolve_explicit_gpu_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger::new()
        .map(|compositor| CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Gpu,
            fallback_reason: None,
            gpu_probe: Some(GpuCompositorProbeInfo::from(compositor.describe())),
        })
        .with_context(|| {
            format!(
                "{GPU_COMPOSITOR_UNAVAILABLE_REASON}; ensure a compatible adapter is present and the compositor shader path initializes cleanly"
            )
        })
}

#[cfg(not(feature = "wgpu"))]
fn resolve_explicit_gpu_mode(
    _requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    bail!(
        "{GPU_COMPOSITOR_UNAVAILABLE_REASON}; rebuild hypercolor-daemon with the `wgpu` feature or use cpu/auto"
    )
}

#[cfg(feature = "wgpu")]
impl From<crate::render_thread::sparkleflinger::gpu::GpuCompositorProbe>
    for GpuCompositorProbeInfo
{
    fn from(probe: crate::render_thread::sparkleflinger::gpu::GpuCompositorProbe) -> Self {
        Self {
            adapter_name: probe.adapter_name,
            backend: probe.backend,
            texture_format: probe.texture_format,
            max_texture_dimension_2d: probe.max_texture_dimension_2d,
            max_storage_textures_per_shader_stage: probe.max_storage_textures_per_shader_stage,
        }
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::config::RenderAccelerationMode;

    use super::resolve_compositor_acceleration_mode;

    #[cfg(not(feature = "wgpu"))]
    #[test]
    fn auto_mode_stays_on_cpu_without_warning() {
        let resolution = resolve_compositor_acceleration_mode(RenderAccelerationMode::Auto)
            .expect("auto mode should resolve");
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Cpu);
        assert!(resolution.fallback_reason.is_none());
        assert!(resolution.gpu_probe.is_none());
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn auto_mode_prefers_gpu_when_adapter_is_available() {
        let probe = crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger::new();
        if probe.is_err() {
            return;
        }

        let resolution = resolve_compositor_acceleration_mode(RenderAccelerationMode::Auto)
            .expect("auto mode should resolve when wgpu is enabled");
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Gpu);
        assert!(resolution.fallback_reason.is_none());
        assert!(resolution.gpu_probe.is_some());
    }

    #[cfg(not(feature = "wgpu"))]
    #[test]
    fn explicit_gpu_requires_wgpu_feature() {
        let error = resolve_compositor_acceleration_mode(RenderAccelerationMode::Gpu)
            .expect_err("explicit gpu should fail when wgpu is disabled");
        assert!(format!("{error:#}").contains("rebuild hypercolor-daemon with the `wgpu` feature"));
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn explicit_gpu_resolves_when_adapter_is_available() {
        let probe = crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger::new();
        if probe.is_err() {
            return;
        }

        let resolution = resolve_compositor_acceleration_mode(RenderAccelerationMode::Gpu)
            .expect("gpu mode should resolve when a compatible adapter is available");
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Gpu);
        assert!(resolution.fallback_reason.is_none());
        assert!(resolution.gpu_probe.is_some());
    }
}
