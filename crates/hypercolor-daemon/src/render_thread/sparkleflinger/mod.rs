mod cpu;
mod face_overlay;
#[cfg(feature = "wgpu")]
pub(crate) mod gpu;
#[cfg(feature = "wgpu")]
mod gpu_sampling;

use anyhow::{Result, bail};
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::DisplayFaceBlendMode;

use crate::performance::CompositorBackendKind;
#[cfg(feature = "wgpu")]
use crate::render_thread::gpu_device::GpuRenderDevice;
#[cfg(feature = "wgpu")]
use crate::render_thread::sparkleflinger::gpu::{GpuZoneSamplingDispatch, PendingGpuZoneSampling};

use super::producer_queue::ProducerFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompositionMode {
    Replace,
    Alpha,
    Add,
    Screen,
}

#[derive(Debug, Clone)]
pub struct CompositionLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
    opaque_hint: bool,
}

impl CompositionLayer {
    pub fn replace_canvas(canvas: Canvas) -> Self {
        Self::replace_opaque(ProducerFrame::Canvas(canvas))
    }

    pub fn replace_surface(surface: PublishedSurface) -> Self {
        Self::replace_opaque(ProducerFrame::Surface(surface))
    }

    pub fn alpha_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::alpha_opaque(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn add_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::add_opaque(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn screen_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::screen_opaque(ProducerFrame::Canvas(canvas), opacity)
    }

    #[allow(
        dead_code,
        reason = "used by tests and the optional wgpu compositor lane"
    )]
    pub(crate) fn replace(frame: ProducerFrame) -> Self {
        Self::from_parts(frame, CompositionMode::Replace, 1.0, false)
    }

    pub(crate) fn replace_opaque(frame: ProducerFrame) -> Self {
        Self::from_parts(frame, CompositionMode::Replace, 1.0, true)
    }

    pub(crate) fn from_parts(
        frame: ProducerFrame,
        mode: CompositionMode,
        opacity: f32,
        opaque_hint: bool,
    ) -> Self {
        Self {
            frame,
            mode,
            opacity,
            opaque_hint,
        }
    }

    #[allow(
        dead_code,
        reason = "used by tests and the optional wgpu compositor lane"
    )]
    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Alpha, opacity, false)
    }

    pub(crate) fn alpha_opaque(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Alpha, opacity, true)
    }

    #[allow(
        dead_code,
        reason = "used by tests and the optional wgpu compositor lane"
    )]
    pub(crate) fn add(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Add, opacity, false)
    }

    pub(crate) fn add_opaque(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Add, opacity, true)
    }

    #[allow(
        dead_code,
        reason = "used by tests and the optional wgpu compositor lane"
    )]
    pub(crate) fn screen(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Screen, opacity, false)
    }

    pub(crate) fn screen_opaque(frame: ProducerFrame, opacity: f32) -> Self {
        Self::from_parts(frame, CompositionMode::Screen, opacity, true)
    }

    fn is_bypass_candidate(&self) -> bool {
        #[cfg(feature = "servo-gpu-import")]
        if matches!(self.frame, ProducerFrame::Gpu(_)) {
            return false;
        }

        self.mode == CompositionMode::Replace && self.opacity >= 1.0
    }
}

#[derive(Debug, Clone)]
pub struct CompositionPlan {
    width: u32,
    height: u32,
    layers: Vec<CompositionLayer>,
    cpu_replay_cacheable: bool,
}

impl CompositionPlan {
    pub fn single(width: u32, height: u32, layer: CompositionLayer) -> Self {
        Self {
            width,
            height,
            layers: vec![layer],
            cpu_replay_cacheable: true,
        }
    }

    pub fn with_layers(width: u32, height: u32, layers: Vec<CompositionLayer>) -> Self {
        Self {
            width,
            height,
            layers,
            cpu_replay_cacheable: true,
        }
    }

    #[must_use]
    pub fn with_cpu_replay_cacheable(mut self, cacheable: bool) -> Self {
        self.cpu_replay_cacheable = cacheable;
        self
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(dead_code, reason = "only used by the optional wgpu compositor lane")
    )]
    #[cfg(feature = "servo-gpu-import")]
    fn contains_gpu_frames(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| matches!(layer.frame, ProducerFrame::Gpu(_)))
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(dead_code, reason = "only used by the optional wgpu compositor lane")
    )]
    #[cfg(not(feature = "servo-gpu-import"))]
    const fn contains_gpu_frames(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct ComposedFrameSet {
    pub sampling_canvas: Option<Canvas>,
    pub sampling_surface: Option<PublishedSurface>,
    pub preview_surface: Option<PublishedSurface>,
    pub bypassed: bool,
    pub(crate) backend: CompositorBackendKind,
}

pub(crate) type RenderFrame = (Canvas, Option<PublishedSurface>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewSurfaceRequest {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
enum SparkleFlingerBackend {
    Cpu(cpu::CpuSparkleFlinger),
    #[cfg(feature = "wgpu")]
    Gpu {
        gpu: gpu::GpuSparkleFlinger,
        cpu_fallback: cpu::CpuSparkleFlinger,
    },
}

#[derive(Debug)]
pub struct SparkleFlinger {
    backend: SparkleFlingerBackend,
    preview_surface_pool: RenderSurfacePool,
    composition_surface_pool: RenderSurfacePool,
}

#[cfg_attr(not(feature = "wgpu"), allow(dead_code))]
pub(crate) enum ZoneSamplingDispatch {
    Unsupported,
    Ready,
    Saturated,
    Pending(PendingZoneSampling),
}

pub(crate) enum PendingZoneSampling {
    #[cfg(feature = "wgpu")]
    Gpu(PendingGpuZoneSampling),
}

impl SparkleFlinger {
    pub fn cpu() -> Self {
        Self {
            backend: SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new()),
            preview_surface_pool: new_preview_surface_pool(),
            composition_surface_pool: new_composition_surface_pool(),
        }
    }

    pub fn new(mode: RenderAccelerationMode) -> Result<Self> {
        Self::new_with_gpu_device(
            mode,
            #[cfg(feature = "wgpu")]
            None,
        )
    }

    pub(crate) fn new_with_gpu_device(
        mode: RenderAccelerationMode,
        #[cfg(feature = "wgpu")] render_device: Option<GpuRenderDevice>,
    ) -> Result<Self> {
        let backend = match mode {
            RenderAccelerationMode::Cpu => {
                SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new())
            }
            RenderAccelerationMode::Auto => bail!(
                "auto compositor acceleration must be resolved before constructing SparkleFlinger"
            ),
            RenderAccelerationMode::Gpu => new_gpu_backend(
                #[cfg(feature = "wgpu")]
                render_device,
            )?,
        };
        Ok(Self {
            backend,
            preview_surface_pool: new_preview_surface_pool(),
            composition_surface_pool: new_composition_surface_pool(),
        })
    }

    pub fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        let preview_surface_request = Some(PreviewSurfaceRequest {
            width: plan.width,
            height: plan.height,
        });
        self.compose_for_outputs(plan, true, preview_surface_request)
    }

    pub fn compose_for_outputs(
        &mut self,
        plan: CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(backend) => backend.compose_with_surface_pools(
                plan,
                requires_cpu_sampling_canvas,
                preview_surface_request,
                &mut self.preview_surface_pool,
                &mut self.composition_surface_pool,
            ),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, cpu_fallback } => {
                let gpu_compose_result = if gpu.supports_plan(&plan) {
                    Some(gpu.compose(&plan, requires_cpu_sampling_canvas, preview_surface_request))
                } else {
                    None
                };
                match gpu_compose_result {
                    Some(Ok(composed)) => return composed,
                    Some(Err(error)) if plan.contains_gpu_frames() => {
                        tracing::debug!(
                            %error,
                            "Skipping CPU compositor fallback for GPU producer frame"
                        );
                        return gpu_frame_without_cpu_fallback();
                    }
                    None if plan.contains_gpu_frames() => {
                        tracing::debug!(
                            "Skipping CPU compositor fallback for unsupported GPU producer plan"
                        );
                        return gpu_frame_without_cpu_fallback();
                    }
                    Some(Err(_)) | None => {}
                }
                let mut composed = cpu_fallback.compose_with_surface_pools(
                    plan,
                    requires_cpu_sampling_canvas,
                    preview_surface_request,
                    &mut self.preview_surface_pool,
                    &mut self.composition_surface_pool,
                );
                composed.backend = CompositorBackendKind::GpuFallback;
                composed
            }
        }
    }

    pub(crate) fn blend_face_overlay_rgba(
        scene_rgba: &mut [u8],
        face_rgba: &[u8],
        blend_mode: DisplayFaceBlendMode,
        opacity: f32,
    ) {
        face_overlay::blend_face_overlay_rgba(scene_rgba, face_rgba, blend_mode, opacity);
    }

    pub(crate) fn preview_only_frame(
        &mut self,
        frame: ProducerFrame,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        let backend = self.backend_kind();
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {}
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.discard_preview_work(),
        }

        ComposedFrameSet {
            sampling_canvas: None,
            sampling_surface: None,
            preview_surface: preview_surface_for_frame(
                &frame,
                preview_surface_request,
                &mut self.preview_surface_pool,
            ),
            bypassed: true,
            backend,
        }
    }

    pub fn sample_zone_plan(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
    ) -> Result<Option<Vec<ZoneColors>>> {
        let mut zones = Vec::new();
        if self.sample_zone_plan_into(prepared_zones, &mut zones)? {
            return Ok(Some(zones));
        }
        Ok(None)
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(
            unused_variables,
            reason = "GPU-only parameters still present in the CPU-only build"
        )
    )]
    #[allow(
        clippy::ptr_arg,
        reason = "GPU sampling appends zone results through the shared Vec buffer"
    )]
    pub fn sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(false),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.sample_zone_plan_into(prepared_zones, zones)
            }
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the public wrapper mirrors the GPU backend signature even when only the CPU path is compiled"
    )]
    pub(crate) fn begin_sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<ZoneSamplingDispatch> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = prepared_zones;
                let _ = zones;
                Ok(ZoneSamplingDispatch::Unsupported)
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => Ok(
                match gpu.begin_sample_zone_plan_into(prepared_zones, zones)? {
                    GpuZoneSamplingDispatch::Unsupported => ZoneSamplingDispatch::Unsupported,
                    GpuZoneSamplingDispatch::Ready => ZoneSamplingDispatch::Ready,
                    GpuZoneSamplingDispatch::Saturated => ZoneSamplingDispatch::Saturated,
                    GpuZoneSamplingDispatch::Pending(pending) => {
                        ZoneSamplingDispatch::Pending(PendingZoneSampling::Gpu(pending))
                    }
                },
            ),
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the public wrapper mirrors the GPU backend signature even when only the CPU path is compiled"
    )]
    pub(crate) fn try_finish_pending_zone_sampling(
        &mut self,
        pending: &mut PendingZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        #[cfg(not(feature = "wgpu"))]
        let _ = zones;
        match (&mut self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => Ok(false),
            #[cfg(feature = "wgpu")]
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingZoneSampling::Gpu(pending)) => {
                gpu.try_finish_pending_zone_sampling(pending, zones)
            }
            #[allow(unreachable_patterns)]
            _ => Ok(false),
        }
    }

    pub(crate) fn discard_pending_zone_sampling(&mut self, pending: PendingZoneSampling) {
        match (&mut self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => {}
            #[cfg(feature = "wgpu")]
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingZoneSampling::Gpu(pending)) => {
                gpu.discard_pending_zone_sampling(pending);
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(
            unused_variables,
            reason = "GPU-only parameter still present in the CPU-only build"
        )
    )]
    pub(crate) fn pending_zone_sampling_matches_current_work(
        &self,
        pending: &PendingZoneSampling,
        prepared_zones: &[PreparedZonePlan],
    ) -> bool {
        match (&self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => false,
            #[cfg(feature = "wgpu")]
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingZoneSampling::Gpu(pending)) => {
                gpu.pending_zone_sampling_matches_current_work(pending, prepared_zones)
            }
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(
            unused_variables,
            reason = "GPU-only parameter still present in the CPU-only build"
        )
    )]
    pub fn can_sample_zone_plan(&self, prepared_zones: &[PreparedZonePlan]) -> bool {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.can_sample_zone_plan(prepared_zones),
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the public wrapper mirrors the GPU backend signature even when only the CPU path is compiled"
    )]
    pub(crate) fn read_back_current_output_surface_for_cpu_sampling(
        &mut self,
    ) -> Result<Option<PublishedSurface>> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(None),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.read_back_current_output_surface_for_cpu_sampling()
            }
        }
    }

    pub(crate) fn max_pending_zone_sampling(&self) -> usize {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => 0,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.max_pending_zone_sampling(),
        }
    }

    pub fn resolve_preview_surface(&mut self) -> Result<Option<PublishedSurface>> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(None),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.resolve_preview_surface(),
        }
    }

    pub fn submit_pending_preview_work(&mut self) -> Result<()> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(()),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.submit_pending_preview_work(),
        }
    }

    pub(crate) fn take_last_sample_readback_wait_blocked(&mut self) -> bool {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.take_last_sample_readback_wait_blocked(),
        }
    }

    fn backend_kind(&self) -> CompositorBackendKind {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => CompositorBackendKind::Cpu,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { .. } => CompositorBackendKind::Gpu,
        }
    }
}

#[cfg(feature = "wgpu")]
fn gpu_frame_without_cpu_fallback() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: false,
        backend: CompositorBackendKind::GpuFallback,
    }
}

#[cfg(feature = "wgpu")]
fn new_gpu_backend(render_device: Option<GpuRenderDevice>) -> Result<SparkleFlingerBackend> {
    let gpu = if let Some(render_device) = render_device {
        gpu::GpuSparkleFlinger::with_render_device(render_device)?
    } else {
        gpu::GpuSparkleFlinger::new()?
    };
    Ok(SparkleFlingerBackend::Gpu {
        gpu,
        cpu_fallback: cpu::CpuSparkleFlinger::new(),
    })
}

#[cfg(not(feature = "wgpu"))]
fn new_gpu_backend() -> Result<SparkleFlingerBackend> {
    bail!(
        "gpu compositor acceleration is not available yet; rebuild hypercolor-daemon with the `wgpu` feature or use cpu/auto"
    )
}

pub(super) fn publish_composed_frame(
    frame: RenderFrame,
    bypassed: bool,
    requires_cpu_sampling_canvas: bool,
    requires_published_surface: bool,
) -> ComposedFrameSet {
    let (sampling_canvas, sampling_surface) = frame;
    if let Some(sampling_surface) = sampling_surface {
        let sampling_canvas = requires_cpu_sampling_canvas.then_some(sampling_canvas);
        let sampling_surface = requires_published_surface.then_some(sampling_surface);
        return ComposedFrameSet {
            sampling_canvas,
            sampling_surface,
            preview_surface: None,
            bypassed,
            backend: CompositorBackendKind::Cpu,
        };
    }

    if !requires_published_surface {
        return ComposedFrameSet {
            sampling_canvas: requires_cpu_sampling_canvas.then_some(sampling_canvas),
            sampling_surface: None,
            preview_surface: None,
            bypassed,
            backend: CompositorBackendKind::Cpu,
        };
    }

    let sampling_surface = PublishedSurface::from_owned_canvas(sampling_canvas, 0, 0);
    let sampling_canvas =
        requires_cpu_sampling_canvas.then(|| Canvas::from_published_surface(&sampling_surface));
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface: Some(sampling_surface),
        preview_surface: None,
        bypassed,
        backend: CompositorBackendKind::Cpu,
    }
}

fn preview_surface_for_frame(
    frame: &ProducerFrame,
    preview_surface_request: Option<PreviewSurfaceRequest>,
    preview_surface_pool: &mut RenderSurfacePool,
) -> Option<PublishedSurface> {
    let request = preview_surface_request?;
    match frame {
        ProducerFrame::Surface(surface) => {
            if preview_request_matches_dimensions(request, surface.width(), surface.height()) {
                return Some(surface.clone());
            }
            scaled_preview_surface_from_rgba(
                surface.rgba_bytes(),
                surface.width(),
                surface.height(),
                request,
                preview_surface_pool,
            )
        }
        ProducerFrame::Canvas(canvas) => {
            if preview_request_matches_dimensions(request, canvas.width(), canvas.height()) {
                return Some(PublishedSurface::from_owned_canvas(canvas.clone(), 0, 0));
            }
            scaled_preview_surface_from_rgba(
                canvas.as_rgba_bytes(),
                canvas.width(),
                canvas.height(),
                request,
                preview_surface_pool,
            )
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
    }
}

fn new_preview_surface_pool() -> RenderSurfacePool {
    RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(1, 1), 2)
}

fn new_composition_surface_pool() -> RenderSurfacePool {
    RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(1, 1), 2)
}

fn preview_request_matches_dimensions(
    request: PreviewSurfaceRequest,
    width: u32,
    height: u32,
) -> bool {
    request.width == width && request.height == height
}

pub(super) fn scaled_preview_surface_from_rgba(
    rgba: &[u8],
    source_width: u32,
    source_height: u32,
    request: PreviewSurfaceRequest,
    preview_surface_pool: &mut RenderSurfacePool,
) -> Option<PublishedSurface> {
    if request.width == 0
        || request.height == 0
        || preview_request_matches_dimensions(request, source_width, source_height)
    {
        return None;
    }

    let descriptor = SurfaceDescriptor::rgba8888(request.width, request.height);
    if preview_surface_pool.descriptor() != descriptor {
        *preview_surface_pool = RenderSurfacePool::with_slot_count(descriptor, 2);
    }

    let mut lease = preview_surface_pool.dequeue()?;
    let preview_bytes = lease.canvas_mut().as_rgba_bytes_mut();
    let source_width_usize = usize::try_from(source_width).ok()?;
    let source_height_usize = usize::try_from(source_height).ok()?;
    let target_width_usize = usize::try_from(request.width).ok()?;
    let target_height_usize = usize::try_from(request.height).ok()?;

    for y in 0..target_height_usize {
        let source_y = y
            .saturating_mul(source_height_usize)
            .checked_div(target_height_usize.max(1))?
            .min(source_height_usize.saturating_sub(1));
        for x in 0..target_width_usize {
            let source_x = x
                .saturating_mul(source_width_usize)
                .checked_div(target_width_usize.max(1))?
                .min(source_width_usize.saturating_sub(1));
            let source_offset = source_y
                .checked_mul(source_width_usize)?
                .checked_add(source_x)?
                .checked_mul(4)?;
            let target_offset = y
                .checked_mul(target_width_usize)?
                .checked_add(x)?
                .checked_mul(4)?;
            preview_bytes[target_offset..target_offset + 4]
                .copy_from_slice(&rgba[source_offset..source_offset + 4]);
        }
    }

    Some(lease.submit(0, 0))
}

#[cfg(test)]
mod tests {
    use hypercolor_core::blend_math::{
        RgbaBlendMode, blend_rgba_pixels_in_place, decode_srgb_channel, encode_srgb_channel,
        screen_blend,
    };
    use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::scene::DisplayFaceBlendMode;

    use super::{CompositionLayer, CompositionPlan, PreviewSurfaceRequest, SparkleFlinger};
    use crate::render_thread::producer_queue::ProducerFrame;

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    fn expected_blend(dst: Rgba, src: Rgba, mode: BlendMode, opacity: f32) -> Rgba {
        let dst = dst.to_linear_f32();
        let src = src.to_linear_f32();
        let blended = mode.blend(
            [dst.r, dst.g, dst.b, dst.a],
            [src.r, src.g, src.b, src.a],
            opacity,
        );
        RgbaF32::new(blended[0], blended[1], blended[2], blended[3]).to_srgba()
    }

    fn patterned_surface(seed: u8) -> PublishedSurface {
        let rgba = vec![
            seed,
            32,
            224,
            255,
            192,
            seed,
            48,
            192,
            12,
            180,
            seed,
            96,
            240,
            220,
            seed / 2,
            255,
        ];
        PublishedSurface::from_owned_canvas(Canvas::from_vec(rgba, 2, 2), 7, 11)
    }

    fn legacy_face_overlay_rgba(
        scene: &PublishedSurface,
        face: &PublishedSurface,
        blend_mode: DisplayFaceBlendMode,
        opacity: f32,
    ) -> Vec<u8> {
        let mut target_rgba = scene.rgba_bytes().to_vec();
        match blend_mode {
            DisplayFaceBlendMode::Replace => {
                legacy_replace_face_rgba_in_place(&mut target_rgba, face.rgba_bytes(), opacity);
            }
            DisplayFaceBlendMode::Tint => {
                legacy_blend_face_material_tint_rgba(&mut target_rgba, face.rgba_bytes(), opacity);
            }
            DisplayFaceBlendMode::LumaReveal => {
                legacy_blend_face_luma_reveal_rgba(&mut target_rgba, face.rgba_bytes(), opacity);
            }
            _ => {
                let Some(canvas_blend_mode) = blend_mode.standard_canvas_blend_mode() else {
                    return target_rgba;
                };
                blend_rgba_pixels_in_place(
                    &mut target_rgba,
                    face.rgba_bytes(),
                    RgbaBlendMode::from(canvas_blend_mode),
                    opacity,
                );
            }
        }

        for pixel in target_rgba.chunks_exact_mut(4) {
            pixel[3] = u8::MAX;
        }
        target_rgba
    }

    fn legacy_replace_face_rgba_in_place(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        for (target_pixel, source_pixel) in target_rgba
            .chunks_exact_mut(4)
            .zip(source_rgba.chunks_exact(4))
        {
            let source_alpha = (f32::from(source_pixel[3]) / 255.0) * opacity;
            target_pixel[0] =
                encode_srgb_channel(decode_srgb_channel(source_pixel[0]) * source_alpha);
            target_pixel[1] =
                encode_srgb_channel(decode_srgb_channel(source_pixel[1]) * source_alpha);
            target_pixel[2] =
                encode_srgb_channel(decode_srgb_channel(source_pixel[2]) * source_alpha);
            target_pixel[3] = u8::MAX;
        }
    }

    fn legacy_blend_face_material_tint_rgba(
        target_rgba: &mut [u8],
        source_rgba: &[u8],
        opacity: f32,
    ) {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }

        for (dst_px, src_px) in target_rgba
            .chunks_exact_mut(4)
            .zip(source_rgba.chunks_exact(4))
        {
            let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
            if alpha <= 0.0 {
                continue;
            }

            let dst = [
                decode_srgb_channel(dst_px[0]),
                decode_srgb_channel(dst_px[1]),
                decode_srgb_channel(dst_px[2]),
            ];
            let src = [
                decode_srgb_channel(src_px[0]),
                decode_srgb_channel(src_px[1]),
                decode_srgb_channel(src_px[2]),
            ];
            let material = legacy_effect_tint_material(dst, src);

            dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, material[0] * alpha));
            dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, material[1] * alpha));
            dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, material[2] * alpha));
        }
    }

    fn legacy_blend_face_luma_reveal_rgba(
        target_rgba: &mut [u8],
        source_rgba: &[u8],
        opacity: f32,
    ) {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }

        for (dst_px, src_px) in target_rgba
            .chunks_exact_mut(4)
            .zip(source_rgba.chunks_exact(4))
        {
            let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
            if alpha <= 0.0 {
                continue;
            }

            let dst = [
                decode_srgb_channel(dst_px[0]),
                decode_srgb_channel(dst_px[1]),
                decode_srgb_channel(dst_px[2]),
            ];
            let src = [
                decode_srgb_channel(src_px[0]),
                decode_srgb_channel(src_px[1]),
                decode_srgb_channel(src_px[2]),
            ];
            let material = legacy_effect_tint_material(dst, src);
            let reveal = legacy_smoothstep(0.18, 0.92, legacy_linear_rgb_luma(src));
            let inside = [
                src[0].mul_add(1.0 - reveal, material[0] * reveal),
                src[1].mul_add(1.0 - reveal, material[1] * reveal),
                src[2].mul_add(1.0 - reveal, material[2] * reveal),
            ];

            dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, inside[0] * alpha));
            dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, inside[1] * alpha));
            dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, inside[2] * alpha));
        }
    }

    fn legacy_effect_tint_material(effect_rgb: [f32; 3], face_rgb: [f32; 3]) -> [f32; 3] {
        let luma = legacy_linear_rgb_luma(face_rgb);
        let colorfulness = legacy_rgb_colorfulness(face_rgb);
        let neutral = 0.18_f32.mul_add(1.0 - luma, luma).clamp(0.18, 1.0);
        let emission_strength = (1.0 - colorfulness) * luma * 0.12;

        std::array::from_fn(|index| {
            let tint = neutral.mul_add(1.0 - 0.72, face_rgb[index].max(neutral * 0.75) * 0.72);
            let filtered = effect_rgb[index] * tint;
            screen_blend(filtered, face_rgb[index] * emission_strength)
        })
    }

    fn legacy_linear_rgb_luma(rgb: [f32; 3]) -> f32 {
        (rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722).clamp(0.0, 1.0)
    }

    fn legacy_rgb_colorfulness(rgb: [f32; 3]) -> f32 {
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        (max - min).clamp(0.0, 1.0)
    }

    fn legacy_smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
        if edge0 >= edge1 {
            return if x >= edge1 { 1.0 } else { 0.0 };
        }
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    #[test]
    fn sparkleflinger_rejects_unresolved_auto_mode() {
        let error = SparkleFlinger::new(RenderAccelerationMode::Auto)
            .expect_err("auto mode must be resolved during daemon startup");
        assert!(
            error
                .to_string()
                .contains("must be resolved before constructing SparkleFlinger")
        );
    }

    #[test]
    fn sparkleflinger_face_overlay_matches_legacy_math_for_every_mode() {
        let scene = patterned_surface(48);
        let face = patterned_surface(144);

        for blend_mode in [
            DisplayFaceBlendMode::Replace,
            DisplayFaceBlendMode::Alpha,
            DisplayFaceBlendMode::Tint,
            DisplayFaceBlendMode::LumaReveal,
            DisplayFaceBlendMode::Add,
            DisplayFaceBlendMode::Screen,
            DisplayFaceBlendMode::Multiply,
            DisplayFaceBlendMode::Overlay,
            DisplayFaceBlendMode::SoftLight,
            DisplayFaceBlendMode::ColorDodge,
            DisplayFaceBlendMode::Difference,
        ] {
            let expected = legacy_face_overlay_rgba(&scene, &face, blend_mode, 0.6);
            let mut composed = scene.rgba_bytes().to_vec();
            SparkleFlinger::blend_face_overlay_rgba(
                &mut composed,
                face.rgba_bytes(),
                blend_mode,
                0.6,
            );

            assert_eq!(
                composed, expected,
                "face overlay mismatch for {blend_mode:?}",
            );
        }
    }

    #[test]
    fn sparkleflinger_bypasses_single_replace_surface() {
        let source =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::single(
            2,
            2,
            CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
        ));

        let surface = composed
            .sampling_surface
            .expect("single replace layer should bypass into a surface");
        assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
        assert!(composed.preview_surface.is_none());
        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("bypass path should materialize a canvas view")
                .as_rgba_bytes()
                .as_ptr(),
            source.rgba_bytes().as_ptr()
        );
    }

    #[test]
    fn sparkleflinger_preview_only_frame_reuses_full_size_surface() {
        let source =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
        let mut sparkleflinger = SparkleFlinger::cpu();

        let composed = sparkleflinger.preview_only_frame(
            ProducerFrame::Surface(source.clone()),
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        );

        let preview_surface = composed
            .preview_surface
            .expect("full-size preview-only path should reuse the existing surface");
        assert_eq!(
            preview_surface.storage_identity(),
            source.storage_identity()
        );
        assert!(composed.bypassed);
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
    }

    #[test]
    fn sparkleflinger_preview_only_frame_scales_surface_preview() {
        let mut source_canvas = Canvas::new(2, 2);
        source_canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
        source_canvas.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
        source_canvas.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
        source_canvas.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
        let source = PublishedSurface::from_owned_canvas(source_canvas, 7, 11);
        let mut sparkleflinger = SparkleFlinger::cpu();

        let composed = sparkleflinger.preview_only_frame(
            ProducerFrame::Surface(source),
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
        );

        let preview_surface = composed
            .preview_surface
            .expect("scaled preview-only path should materialize a preview surface");
        assert_eq!(preview_surface.width(), 1);
        assert_eq!(preview_surface.height(), 1);
    }

    #[test]
    fn sparkleflinger_scaled_preview_reuses_surface_pool_after_warmup() {
        let source =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let request = Some(PreviewSurfaceRequest {
            width: 1,
            height: 1,
        });

        let first = sparkleflinger
            .preview_only_frame(ProducerFrame::Surface(source.clone()), request)
            .preview_surface
            .expect("first scaled preview should publish")
            .rgba_bytes()
            .as_ptr()
            .addr();
        let second = sparkleflinger
            .preview_only_frame(ProducerFrame::Surface(source.clone()), request)
            .preview_surface
            .expect("second scaled preview should publish")
            .rgba_bytes()
            .as_ptr()
            .addr();
        let third = sparkleflinger
            .preview_only_frame(ProducerFrame::Surface(source), request)
            .preview_surface
            .expect("third scaled preview should publish")
            .rgba_bytes()
            .as_ptr()
            .addr();

        assert_ne!(first, second);
        assert_eq!(first, third);
    }

    #[test]
    fn sparkleflinger_composed_frame_reuses_surface_pool_after_warmup() {
        let base =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 7, 11);
        let overlay =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(0, 0, 255, 255)), 8, 12);
        let plan = CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(base)),
                CompositionLayer::alpha(ProducerFrame::Surface(overlay), 0.5),
            ],
        )
        .with_cpu_replay_cacheable(false);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let request = Some(PreviewSurfaceRequest {
            width: 2,
            height: 2,
        });

        let first_surface = sparkleflinger
            .compose_for_outputs(plan.clone(), false, request)
            .sampling_surface
            .expect("first composed surface should publish");
        let first = first_surface.rgba_bytes().as_ptr().addr();
        let second_surface = sparkleflinger
            .compose_for_outputs(plan.clone(), false, request)
            .sampling_surface
            .expect("second composed surface should publish");
        let second = second_surface.rgba_bytes().as_ptr().addr();

        drop(first_surface);
        drop(second_surface);

        let third = sparkleflinger
            .compose_for_outputs(plan, false, request)
            .sampling_surface
            .expect("third composed surface should publish")
            .rgba_bytes()
            .as_ptr()
            .addr();

        assert_ne!(first, second);
        assert_eq!(first, third);
    }

    #[test]
    fn sparkleflinger_skips_sampling_surface_when_not_requested() {
        let base = solid_canvas(Rgba::new(255, 0, 0, 255));
        let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose_for_outputs(
            CompositionPlan::with_layers(
                2,
                2,
                vec![
                    CompositionLayer::replace(ProducerFrame::Canvas(base)),
                    CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
                ],
            ),
            true,
            None,
        );

        assert!(composed.sampling_canvas.is_some());
        assert!(composed.sampling_surface.is_none());
    }

    #[test]
    fn sparkleflinger_skips_sampling_surface_for_uncacheable_shared_multilayer_plans() {
        let base_surface =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 0, 0);
        let overlay_surface =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(0, 0, 255, 255)), 0, 0);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose_for_outputs(
            CompositionPlan::with_layers(
                2,
                2,
                vec![
                    CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
                    CompositionLayer::alpha_canvas(
                        Canvas::from_published_surface(&overlay_surface),
                        0.5,
                    ),
                ],
            )
            .with_cpu_replay_cacheable(false),
            true,
            None,
        );

        assert!(composed.sampling_canvas.is_some());
        assert!(composed.sampling_surface.is_none());
    }

    #[test]
    fn sparkleflinger_cpu_scales_preview_surface_when_requested() {
        let base = solid_canvas(Rgba::new(255, 0, 0, 255));
        let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose_for_outputs(
            CompositionPlan::with_layers(
                2,
                2,
                vec![
                    CompositionLayer::replace(ProducerFrame::Canvas(base)),
                    CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
                ],
            ),
            true,
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
        );

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU sampling should keep the full-size canvas")
                .width(),
            2
        );
        let preview_surface = composed
            .preview_surface
            .expect("scaled preview requests should publish a preview surface");
        assert_eq!(preview_surface.width(), 1);
        assert_eq!(preview_surface.height(), 1);
    }

    #[test]
    fn sparkleflinger_alpha_layers_respect_order() {
        let base = Rgba::new(255, 0, 0, 255);
        let overlay = Rgba::new(0, 0, 255, 255);
        let opacity = 0.25;
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(overlay)), opacity),
            ],
        ));
        let reversed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(overlay))),
                CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(base)), opacity),
            ],
        ));

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Normal, opacity)
        );
        assert_ne!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .get_pixel(0, 0),
            reversed
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .get_pixel(0, 0)
        );
        let composed_surface = composed
            .sampling_surface
            .expect("composed frame should publish an immutable sampling surface");
        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .as_rgba_bytes()
                .as_ptr(),
            composed_surface.rgba_bytes().as_ptr()
        );
    }

    #[test]
    fn sparkleflinger_add_layers_use_additive_blend() {
        let base = Rgba::new(64, 0, 0, 255);
        let glow = Rgba::new(0, 96, 64, 255);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::add(ProducerFrame::Canvas(solid_canvas(glow)), 1.0),
            ],
        ));

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU add compose should materialize a canvas")
                .get_pixel(0, 0),
            expected_blend(base, glow, BlendMode::Add, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_screen_layers_use_screen_blend() {
        let base = Rgba::new(32, 64, 96, 255);
        let overlay = Rgba::new(96, 64, 32, 255);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::screen(ProducerFrame::Canvas(solid_canvas(overlay)), 1.0),
            ],
        ));

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU screen compose should materialize a canvas")
                .get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Screen, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_reuses_first_replace_canvas_for_multi_layer_plans() {
        let base = solid_canvas(Rgba::new(255, 0, 0, 255));
        let base_ptr = base.as_rgba_bytes().as_ptr();
        let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        ));

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU multi-layer compose should materialize a canvas")
                .as_rgba_bytes()
                .as_ptr(),
            base_ptr
        );
    }

    #[test]
    fn sparkleflinger_reuses_cached_shared_multilayer_compositions() {
        let base_surface =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 32, 0, 255)), 1, 1);
        let overlay_surface =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 255, 255)), 1, 1);
        let mut sparkleflinger = SparkleFlinger::cpu();

        let first = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
                CompositionLayer::alpha_canvas(
                    Canvas::from_published_surface(&overlay_surface),
                    0.35,
                ),
            ],
        ));
        let second = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
                CompositionLayer::alpha_canvas(
                    Canvas::from_published_surface(&overlay_surface),
                    0.35,
                ),
            ],
        ));

        let first_surface = first
            .sampling_surface
            .expect("initial shared composition should publish a sampling surface");
        let second_surface = second
            .sampling_surface
            .expect("cached shared composition should publish a sampling surface");
        assert_eq!(
            first_surface.storage_identity(),
            second_surface.storage_identity()
        );
        assert_eq!(
            first_surface.rgba_bytes().as_ptr(),
            second_surface.rgba_bytes().as_ptr()
        );
        assert!(!second.bypassed);
    }

    #[test]
    fn sparkleflinger_does_not_reuse_cached_composition_after_canvas_mutation() {
        let mut base = solid_canvas(Rgba::new(255, 32, 0, 255));
        let overlay = solid_canvas(Rgba::new(32, 64, 255, 255));
        let mut sparkleflinger = SparkleFlinger::cpu();

        let first = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_canvas(base.clone()),
                CompositionLayer::alpha_canvas(overlay.clone(), 0.35),
            ],
        ));
        base.set_pixel(0, 0, Rgba::new(0, 255, 0, 255));
        let second = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_canvas(base),
                CompositionLayer::alpha_canvas(overlay, 0.35),
            ],
        ));

        let first_surface = first
            .sampling_surface
            .expect("initial composition should publish a sampling surface");
        let second_surface = second
            .sampling_surface
            .expect("mutated composition should publish a sampling surface");
        assert_ne!(
            first_surface.storage_identity(),
            second_surface.storage_identity()
        );
        assert_ne!(
            first
                .sampling_canvas
                .as_ref()
                .expect("initial composition should materialize a canvas")
                .get_pixel(0, 0),
            second
                .sampling_canvas
                .as_ref()
                .expect("mutated composition should materialize a canvas")
                .get_pixel(0, 0)
        );
    }
}
