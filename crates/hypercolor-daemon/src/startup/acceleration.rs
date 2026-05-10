#[cfg(feature = "wgpu")]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(feature = "wgpu"))]
use anyhow::bail;

#[cfg(feature = "wgpu")]
use crate::render_thread::gpu_device::GpuRenderDevice;
use hypercolor_types::config::RenderAccelerationMode;

const GPU_COMPOSITOR_UNAVAILABLE_REASON: &str = "gpu compositor acceleration is unavailable";
#[cfg(feature = "wgpu")]
const AUTO_GPU_PROBE_FAILED_REASON: &str = "gpu compositor probe failed; using CPU compositor path";
#[cfg(not(feature = "wgpu"))]
const AUTO_GPU_NOT_BUILT_REASON: &str = "gpu compositor acceleration is unavailable because hypercolor-daemon was built without the `wgpu` feature; using CPU compositor path";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuCompositorProbeInfo {
    pub(crate) adapter_name: String,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
    pub(crate) linux_servo_gpu_import_backend_compatible: bool,
    pub(crate) linux_servo_gpu_import_backend_reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompositorAccelerationResolution {
    pub(crate) requested_mode: RenderAccelerationMode,
    pub(crate) effective_mode: RenderAccelerationMode,
    pub(crate) fallback_reason: Option<&'static str>,
    pub(crate) gpu_probe: Option<GpuCompositorProbeInfo>,
    #[cfg(feature = "wgpu")]
    pub(crate) gpu_render_device: Option<GpuRenderDevice>,
}

#[cfg(feature = "wgpu")]
struct GpuCompositorProbeResult {
    info: GpuCompositorProbeInfo,
    render_device: Option<GpuRenderDevice>,
}

#[cfg(feature = "wgpu")]
impl GpuCompositorProbeResult {
    #[cfg(test)]
    const fn from_info(info: GpuCompositorProbeInfo) -> Self {
        Self {
            info,
            render_device: None,
        }
    }
}

pub(crate) const fn cpu_compositor_acceleration_resolution() -> CompositorAccelerationResolution {
    CompositorAccelerationResolution {
        requested_mode: RenderAccelerationMode::Cpu,
        effective_mode: RenderAccelerationMode::Cpu,
        fallback_reason: None,
        gpu_probe: None,
        #[cfg(feature = "wgpu")]
        gpu_render_device: None,
    }
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
            #[cfg(feature = "wgpu")]
            gpu_render_device: None,
        }),
        RenderAccelerationMode::Auto => resolve_auto_mode(requested_mode),
        RenderAccelerationMode::Gpu => resolve_explicit_gpu_mode(requested_mode),
    }
}

#[cfg(feature = "wgpu")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "sibling resolvers return Result; uniform shape keeps the dispatch match clean"
)]
fn resolve_auto_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    Ok(resolve_auto_mode_from_probe(
        requested_mode,
        probe_gpu_compositor(),
        AUTO_GPU_PROBE_FAILED_REASON,
    ))
}

#[cfg(not(feature = "wgpu"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the non-wgpu path keeps the same `Result` signature as wgpu builds"
)]
fn resolve_auto_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    Ok(CompositorAccelerationResolution {
        requested_mode,
        effective_mode: RenderAccelerationMode::Cpu,
        fallback_reason: Some(AUTO_GPU_NOT_BUILT_REASON),
        gpu_probe: None,
    })
}

#[cfg(feature = "wgpu")]
fn resolve_explicit_gpu_mode(
    requested_mode: RenderAccelerationMode,
) -> Result<CompositorAccelerationResolution> {
    probe_gpu_compositor()
        .map(|probe| CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Gpu,
            fallback_reason: None,
            gpu_probe: Some(probe.info),
            gpu_render_device: probe.render_device,
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
fn probe_gpu_compositor() -> Result<GpuCompositorProbeResult> {
    let render_device = GpuRenderDevice::new("SparkleFlinger GPU compositor")?;
    let info = crate::render_thread::sparkleflinger::gpu::probe_render_device(&render_device)
        .map(GpuCompositorProbeInfo::from)?;
    Ok(GpuCompositorProbeResult {
        info,
        render_device: Some(render_device),
    })
}

#[cfg(feature = "wgpu")]
fn resolve_auto_mode_from_probe(
    requested_mode: RenderAccelerationMode,
    probe: Result<GpuCompositorProbeResult>,
    fallback_reason: &'static str,
) -> CompositorAccelerationResolution {
    match probe {
        Ok(probe) => CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Gpu,
            fallback_reason: None,
            gpu_probe: Some(probe.info),
            gpu_render_device: probe.render_device,
        },
        Err(_) => CompositorAccelerationResolution {
            requested_mode,
            effective_mode: RenderAccelerationMode::Cpu,
            fallback_reason: Some(fallback_reason),
            gpu_probe: None,
            gpu_render_device: None,
        },
    }
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
            linux_servo_gpu_import_backend_compatible: probe
                .linux_servo_gpu_import_backend_compatible,
            linux_servo_gpu_import_backend_reason: probe.linux_servo_gpu_import_backend_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "wgpu")]
    use anyhow::anyhow;
    use hypercolor_types::config::RenderAccelerationMode;

    use super::resolve_compositor_acceleration_mode;
    #[cfg(feature = "wgpu")]
    use super::{
        AUTO_GPU_PROBE_FAILED_REASON, GpuCompositorProbeInfo, GpuCompositorProbeResult,
        resolve_auto_mode_from_probe,
    };

    #[cfg(feature = "wgpu")]
    fn test_gpu_probe() -> GpuCompositorProbeInfo {
        GpuCompositorProbeInfo {
            adapter_name: "test-adapter".to_owned(),
            backend: "test-backend",
            texture_format: "rgba8unorm",
            max_texture_dimension_2d: 16_384,
            max_storage_textures_per_shader_stage: 8,
            linux_servo_gpu_import_backend_compatible: cfg!(target_os = "linux"),
            linux_servo_gpu_import_backend_reason: None,
        }
    }

    #[test]
    fn explicit_cpu_uses_cpu_without_fallback_reason() {
        let resolution = resolve_compositor_acceleration_mode(RenderAccelerationMode::Cpu)
            .expect("cpu mode should resolve");
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Cpu);
        assert!(resolution.fallback_reason.is_none());
        assert!(resolution.gpu_probe.is_none());
    }

    #[cfg(not(feature = "wgpu"))]
    #[test]
    fn auto_mode_uses_cpu_fallback_reason_without_wgpu() {
        let resolution = resolve_compositor_acceleration_mode(RenderAccelerationMode::Auto)
            .expect("auto mode should resolve");
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Cpu);
        assert!(
            resolution
                .fallback_reason
                .expect("auto fallback should explain why CPU was selected")
                .contains("built without the `wgpu` feature")
        );
        assert!(resolution.gpu_probe.is_none());
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn auto_mode_selects_gpu_when_probe_succeeds() {
        let resolution = resolve_auto_mode_from_probe(
            RenderAccelerationMode::Auto,
            Ok(GpuCompositorProbeResult::from_info(test_gpu_probe())),
            AUTO_GPU_PROBE_FAILED_REASON,
        );
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Gpu);
        assert!(resolution.fallback_reason.is_none());
        assert!(resolution.gpu_probe.is_some());
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn auto_mode_selects_cpu_with_reason_when_probe_fails() {
        let resolution = resolve_auto_mode_from_probe(
            RenderAccelerationMode::Auto,
            Err(anyhow!("adapter unavailable")),
            AUTO_GPU_PROBE_FAILED_REASON,
        );
        assert_eq!(resolution.effective_mode, RenderAccelerationMode::Cpu);
        assert_eq!(
            resolution.fallback_reason,
            Some(AUTO_GPU_PROBE_FAILED_REASON)
        );
        assert!(resolution.gpu_probe.is_none());
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
