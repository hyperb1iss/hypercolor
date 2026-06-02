mod cpu;
mod face_overlay;
#[cfg(feature = "wgpu")]
pub(crate) mod gpu;
#[cfg(feature = "wgpu")]
mod gpu_sampling;
mod transform;

use anyhow::{Result, bail};
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::{DeviceId, DisplayFrameFormat};
use hypercolor_types::event::ZoneColors;
#[cfg(feature = "wgpu")]
use hypercolor_types::layer::SceneLayerId;
use hypercolor_types::layer::{LayerAdjust, LayerTransform};
use hypercolor_types::scene::{DisplayFaceBlendMode, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};
use hypercolor_types::viewport::FitMode;

#[cfg(feature = "wgpu")]
use super::producer_queue::GpuTextureFrame;
use crate::performance::CompositorBackendKind;
#[cfg(feature = "wgpu")]
use crate::render_thread::gpu_device::GpuRenderDevice;
#[cfg(feature = "wgpu")]
use crate::render_thread::sparkleflinger::gpu::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, GpuZoneSamplingDispatch,
    PendingGpuDisplayFinalize, PendingGpuZoneSampling,
};

use super::producer_queue::ProducerFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompositionMode {
    Replace,
    Alpha,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    ColorDodge,
    Difference,
    Tint,
    LumaReveal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CompositionTransform {
    pub(crate) anchor: NormalizedPosition,
    pub(crate) scale: [f32; 2],
    pub(crate) rotation: f32,
    pub(crate) fit: FitMode,
}

impl CompositionTransform {
    fn is_identity(self) -> bool {
        self == Self::default()
    }
}

impl Default for CompositionTransform {
    fn default() -> Self {
        Self {
            anchor: NormalizedPosition::new(0.5, 0.5),
            scale: [1.0, 1.0],
            rotation: 0.0,
            fit: FitMode::Cover,
        }
    }
}

impl From<LayerTransform> for CompositionTransform {
    fn from(value: LayerTransform) -> Self {
        let value = value.normalized();
        Self {
            anchor: value.anchor,
            scale: value.scale,
            rotation: value.rotation,
            fit: value.fit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CompositionAdjust {
    pub(crate) brightness: f32,
    pub(crate) saturation: f32,
    pub(crate) hue_shift: f32,
    pub(crate) tint: [f32; 4],
    pub(crate) tint_strength: f32,
    pub(crate) contrast: f32,
}

impl CompositionAdjust {
    fn is_identity(self) -> bool {
        self == Self::default()
    }

    pub(crate) const fn to_layer_adjust(self) -> LayerAdjust {
        LayerAdjust {
            brightness: self.brightness,
            saturation: self.saturation,
            hue_shift: self.hue_shift,
            tint: self.tint,
            tint_strength: self.tint_strength,
            contrast: self.contrast,
        }
    }
}

impl Default for CompositionAdjust {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            tint_strength: 0.0,
            contrast: 0.0,
        }
    }
}

impl From<LayerAdjust> for CompositionAdjust {
    fn from(value: LayerAdjust) -> Self {
        let value = value.normalized();
        Self {
            brightness: value.brightness,
            saturation: value.saturation,
            hue_shift: value.hue_shift,
            tint: value.tint,
            tint_strength: value.tint_strength,
            contrast: value.contrast,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompositionLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
    opaque_hint: bool,
    transform: Option<CompositionTransform>,
    adjust: Option<CompositionAdjust>,
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
            transform: None,
            adjust: None,
        }
    }

    pub(crate) fn with_transform(mut self, transform: CompositionTransform) -> Self {
        if !transform.is_identity() {
            self.transform = Some(transform);
        }
        self
    }

    pub(crate) fn with_adjust(mut self, adjust: CompositionAdjust) -> Self {
        if !adjust.is_identity() {
            self.adjust = Some(adjust);
        }
        self
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
        #[cfg(feature = "wgpu")]
        if matches!(self.frame, ProducerFrame::GpuTexture(_)) {
            return false;
        }

        self.mode == CompositionMode::Replace
            && self.opacity >= 1.0
            && self.transform.is_none()
            && self.adjust.is_none()
    }

    fn frame_matches_size(&self, width: u32, height: u32) -> bool {
        self.frame.width() == width && self.frame.height() == height
    }

    fn can_bypass_for_size(&self, width: u32, height: u32) -> bool {
        self.is_bypass_candidate() && self.frame_matches_size(width, height)
    }

    fn needs_processing_for_size(&self, width: u32, height: u32) -> bool {
        self.transform.is_some() || self.adjust.is_some() || !self.frame_matches_size(width, height)
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
    #[cfg(feature = "wgpu")]
    fn contains_gpu_frames(&self) -> bool {
        self.layers.iter().any(|layer| match &layer.frame {
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => true,
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => true,
            _ => false,
        })
    }

    #[cfg_attr(
        not(feature = "wgpu"),
        allow(dead_code, reason = "only used by the optional wgpu compositor lane")
    )]
    #[cfg(not(feature = "wgpu"))]
    const fn contains_gpu_frames(&self) -> bool {
        false
    }
}

#[cfg(feature = "wgpu")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MediaTextureSourceKey(u128);

#[cfg(feature = "wgpu")]
impl MediaTextureSourceKey {
    pub(crate) fn from_media_layer(layer_id: SceneLayerId) -> Self {
        Self(layer_id.as_uuid().as_u128())
    }

    #[cfg(test)]
    pub(crate) const fn for_test(value: u128) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DisplayFinalizeCacheKey {
    pub(crate) group_id: ZoneId,
    pub(crate) device_id: DeviceId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) circular: bool,
    pub(crate) frame_format: DisplayFrameFormat,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplayFinalizeParams {
    pub(crate) cache_key: DisplayFinalizeCacheKey,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) circular: bool,
    pub(crate) brightness: f32,
    pub(crate) viewport_position: NormalizedPosition,
    pub(crate) viewport_size: NormalizedPosition,
    pub(crate) viewport_rotation: f32,
    pub(crate) viewport_scale: f32,
    pub(crate) viewport_edge_behavior: EdgeBehavior,
    pub(crate) blend_mode: DisplayFaceBlendMode,
    pub(crate) opacity: f32,
}

#[derive(Debug, Clone)]
pub struct ComposedFrameSet {
    pub sampling_canvas: Option<Canvas>,
    pub sampling_surface: Option<PublishedSurface>,
    pub preview_surface: Option<PublishedSurface>,
    pub bypassed: bool,
    pub(crate) backend: CompositorBackendKind,
    pub(crate) gpu_readback_failed: bool,
    pub(crate) compositor_acceleration_downgraded: bool,
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
    face_overlay_surface_pool: RenderSurfacePool,
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

#[cfg(feature = "wgpu")]
pub(crate) enum DisplayFinalizeDispatch {
    Unsupported,
    Saturated,
    Pending(PendingDisplayFinalization),
}

#[cfg(feature = "wgpu")]
pub(crate) enum DisplayFinalizeFrame {
    Rgba(PublishedSurface),
    Yuv420(DisplayYuv420Frame),
}

#[cfg(feature = "wgpu")]
pub(crate) struct PendingDisplayFinalization(PendingGpuDisplayFinalize);

impl SparkleFlinger {
    pub fn cpu() -> Self {
        Self {
            backend: SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new()),
            preview_surface_pool: new_preview_surface_pool(),
            composition_surface_pool: new_composition_surface_pool(),
            face_overlay_surface_pool: new_composition_surface_pool(),
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
            face_overlay_surface_pool: new_composition_surface_pool(),
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
                let failure_width = plan.width;
                let failure_height = plan.height;
                let contains_gpu_frames = plan.contains_gpu_frames();
                let gpu_compose_result = if gpu.supports_plan(&plan) {
                    Some(gpu.compose(&plan, requires_cpu_sampling_canvas, preview_surface_request))
                } else {
                    None
                };
                match gpu_compose_result {
                    Some(Ok(composed)) => return composed,
                    Some(Err(error)) if contains_gpu_frames => {
                        tracing::warn!(
                            %error,
                            "GPU producer composition failed; refusing CPU readback fallback"
                        );
                        return gpu_frame_without_cpu_fallback(
                            failure_width,
                            failure_height,
                            preview_surface_request,
                            &mut self.preview_surface_pool,
                        );
                    }
                    None if contains_gpu_frames => {
                        tracing::warn!(
                            "Unsupported GPU producer plan; refusing CPU readback fallback"
                        );
                        return gpu_frame_without_cpu_fallback(
                            failure_width,
                            failure_height,
                            preview_surface_request,
                            &mut self.preview_surface_pool,
                        );
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

    #[cfg(feature = "wgpu")]
    pub(crate) fn begin_media_upload_frame(&mut self) {
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &mut self.backend {
            gpu.begin_media_upload_frame();
        }
    }

    pub(crate) fn supports_gpu_output_frames(&self) -> bool {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { .. } => true,
        }
    }

    #[allow(
        dead_code,
        reason = "display-face composition keeps the reusable surface path beside the slice path"
    )]
    pub(crate) fn compose_face_overlay(
        &mut self,
        scene: &PublishedSurface,
        face: &PublishedSurface,
        blend_mode: DisplayFaceBlendMode,
        opacity: f32,
    ) -> PublishedSurface {
        face_overlay::compose_face_overlay(
            scene,
            face,
            blend_mode,
            opacity,
            &mut self.face_overlay_surface_pool,
        )
    }

    pub(crate) fn blend_face_overlay_rgba(
        scene_rgba: &mut [u8],
        face_rgba: &[u8],
        blend_mode: DisplayFaceBlendMode,
        opacity: f32,
    ) {
        face_overlay::blend_face_overlay_rgba(scene_rgba, face_rgba, blend_mode, opacity);
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn begin_finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<DisplayFinalizeDispatch> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(DisplayFinalizeDispatch::Unsupported),
            SparkleFlingerBackend::Gpu { gpu, .. } => Ok(map_gpu_display_finalize_dispatch(
                gpu.begin_finalize_display_face(scene, face, params)?,
            )),
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn begin_finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<DisplayFinalizeDispatch> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(DisplayFinalizeDispatch::Unsupported),
            SparkleFlingerBackend::Gpu { gpu, .. } => Ok(map_gpu_display_finalize_dispatch(
                gpu.begin_finalize_display_face_yuv420(scene, face, params)?,
            )),
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn try_finish_pending_display_finalization(
        &mut self,
        pending: &mut PendingDisplayFinalization,
    ) -> Result<Option<DisplayFinalizeFrame>> {
        match (&mut self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => Ok(None),
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingDisplayFinalization(pending)) => {
                Ok(gpu
                    .try_finish_pending_display_finalization(pending)?
                    .map(map_gpu_display_finalize_frame))
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn discard_pending_display_finalization(
        &mut self,
        pending: PendingDisplayFinalization,
    ) {
        match (&mut self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => {}
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingDisplayFinalization(pending)) => {
                gpu.discard_pending_display_finalization(pending);
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn retain_display_finalize_groups(&mut self, active_group_ids: &[ZoneId]) {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {}
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.retain_display_finalize_groups(active_group_ids);
            }
        }
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
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
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

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the public wrapper mirrors the GPU backend signature even when only the CPU path is compiled"
    )]
    pub(crate) fn finish_pending_zone_sampling(
        &mut self,
        pending: PendingZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<()> {
        #[cfg(not(feature = "wgpu"))]
        let _ = zones;
        match (&mut self.backend, pending) {
            (SparkleFlingerBackend::Cpu(_), _) => Ok(()),
            #[cfg(feature = "wgpu")]
            (SparkleFlingerBackend::Gpu { gpu, .. }, PendingZoneSampling::Gpu(pending)) => {
                gpu.finish_pending_zone_sampling(pending, zones)
            }
            #[allow(unreachable_patterns)]
            _ => Ok(()),
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

    #[cfg(feature = "wgpu")]
    pub(crate) fn current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(None),
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.current_output_frame(),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn upload_canvas_frame(&mut self, canvas: &Canvas) -> Option<GpuTextureFrame> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.upload_canvas_frame(canvas),
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn upload_media_canvas_frame(
        &mut self,
        source: MediaTextureSourceKey,
        canvas: &Canvas,
    ) -> Option<GpuTextureFrame> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.upload_media_canvas_frame(source, canvas),
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
fn map_gpu_display_finalize_dispatch(
    dispatch: GpuDisplayFinalizeDispatch,
) -> DisplayFinalizeDispatch {
    match dispatch {
        GpuDisplayFinalizeDispatch::Unsupported => DisplayFinalizeDispatch::Unsupported,
        GpuDisplayFinalizeDispatch::Saturated => DisplayFinalizeDispatch::Saturated,
        GpuDisplayFinalizeDispatch::Pending(pending) => {
            DisplayFinalizeDispatch::Pending(PendingDisplayFinalization(pending))
        }
    }
}

#[cfg(feature = "wgpu")]
fn map_gpu_display_finalize_frame(frame: GpuDisplayFinalizeFrame) -> DisplayFinalizeFrame {
    match frame {
        GpuDisplayFinalizeFrame::Rgba(surface) => DisplayFinalizeFrame::Rgba(surface),
        GpuDisplayFinalizeFrame::Yuv420(frame) => DisplayFinalizeFrame::Yuv420(frame),
    }
}

#[cfg(feature = "wgpu")]
fn gpu_frame_without_cpu_fallback(
    width: u32,
    height: u32,
    preview_surface_request: Option<PreviewSurfaceRequest>,
    preview_surface_pool: &mut RenderSurfacePool,
) -> ComposedFrameSet {
    let preview_canvas = preview_surface_request
        .filter(|request| request.width < width || request.height < height)
        .map(|_| Canvas::new(width, height));
    let preview_surface = preview_surface_request
        .zip(preview_canvas.as_ref())
        .and_then(|request| {
            scaled_preview_surface_from_rgba(
                request.1.as_rgba_bytes(),
                width,
                height,
                request.0,
                preview_surface_pool,
            )
        });
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface,
        bypassed: false,
        backend: CompositorBackendKind::GpuFallback,
        gpu_readback_failed: true,
        compositor_acceleration_downgraded: false,
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
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
        };
    }

    if !requires_published_surface {
        return ComposedFrameSet {
            sampling_canvas: requires_cpu_sampling_canvas.then_some(sampling_canvas),
            sampling_surface: None,
            preview_surface: None,
            bypassed,
            backend: CompositorBackendKind::Cpu,
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
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
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
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
        ProducerFrame::Gpu(_) => {
            frame.record_cpu_materialization_blocked();
            None
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            frame.record_cpu_materialization_blocked();
            None
        }
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
    use hypercolor_types::spatial::NormalizedPosition;
    use hypercolor_types::viewport::FitMode;

    use super::{
        CompositionAdjust, CompositionLayer, CompositionMode, CompositionPlan,
        CompositionTransform, PreviewSurfaceRequest, SparkleFlinger,
    };
    #[cfg(feature = "wgpu")]
    use super::{gpu_frame_without_cpu_fallback, new_preview_surface_pool};
    #[cfg(feature = "wgpu")]
    use crate::performance::CompositorBackendKind;
    use crate::render_thread::producer_queue::ProducerFrame;

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    fn row_canvas(colors: &[Rgba]) -> Canvas {
        let mut rgba = Vec::with_capacity(colors.len() * 4);
        for color in colors {
            rgba.extend_from_slice(&[color.r, color.g, color.b, color.a]);
        }
        Canvas::from_vec(
            rgba,
            u32::try_from(colors.len()).expect("test row width should fit u32"),
            1,
        )
    }

    fn compose_transformed_source(source: Canvas, width: u32, height: u32, fit: FitMode) -> Canvas {
        let mut sparkleflinger = SparkleFlinger::cpu();
        sparkleflinger
            .compose(CompositionPlan::single(
                width,
                height,
                CompositionLayer::replace(ProducerFrame::Canvas(source)).with_transform(
                    CompositionTransform {
                        anchor: NormalizedPosition::new(0.5, 0.5),
                        scale: [1.0, 1.0],
                        rotation: 0.0,
                        fit,
                    },
                ),
            ))
            .sampling_canvas
            .expect("transformed layer should materialize a canvas")
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
        // Independent copy of the previous display encoder math, kept as a regression fence.
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
        let mut sparkleflinger = SparkleFlinger::cpu();

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
                "slice face overlay mismatch for {blend_mode:?}",
            );

            let surface = sparkleflinger.compose_face_overlay(&scene, &face, blend_mode, 0.6);
            assert_eq!(
                surface.rgba_bytes(),
                expected.as_slice(),
                "surface face overlay mismatch for {blend_mode:?}",
            );
        }
    }

    #[test]
    fn sparkleflinger_composes_face_modes_as_general_layers() {
        let scene = patterned_surface(48);
        let face = patterned_surface(144);
        let mut sparkleflinger = SparkleFlinger::cpu();

        for (composition_mode, face_mode) in [
            (CompositionMode::Tint, DisplayFaceBlendMode::Tint),
            (
                CompositionMode::LumaReveal,
                DisplayFaceBlendMode::LumaReveal,
            ),
        ] {
            let mut expected = legacy_face_overlay_rgba(&scene, &face, face_mode, 0.6);
            for (expected_pixel, scene_pixel) in expected
                .chunks_exact_mut(4)
                .zip(scene.rgba_bytes().chunks_exact(4))
            {
                expected_pixel[3] = scene_pixel[3];
            }
            let composed = sparkleflinger.compose(CompositionPlan::with_layers(
                2,
                2,
                vec![
                    CompositionLayer::replace_opaque(ProducerFrame::Surface(scene.clone())),
                    CompositionLayer::from_parts(
                        ProducerFrame::Surface(face.clone()),
                        composition_mode,
                        0.6,
                        true,
                    ),
                ],
            ));
            assert_eq!(
                composed
                    .sampling_canvas
                    .expect("general layer composition should materialize a canvas")
                    .as_rgba_bytes(),
                expected.as_slice(),
                "general layer composition mismatch for {composition_mode:?}",
            );
        }
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn sparkleflinger_uploads_canvas_as_gpu_texture_frame() {
        let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
            return;
        };
        let source = solid_canvas(Rgba::new(32, 96, 160, 255));
        let Some(frame) = sparkleflinger.upload_canvas_frame(&source) else {
            panic!("GPU canvas upload should return a texture frame");
        };

        assert_eq!(frame.width, source.width());
        assert_eq!(frame.height, source.height());

        let composed = sparkleflinger.compose_for_outputs(
            CompositionPlan::single(
                source.width(),
                source.height(),
                CompositionLayer::replace(ProducerFrame::GpuTexture(frame)),
            ),
            false,
            None,
        );

        assert_eq!(composed.backend, CompositorBackendKind::Gpu);
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert!(
            sparkleflinger
                .current_output_frame()
                .is_ok_and(|frame| frame.is_some())
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn sparkleflinger_refuses_gpu_frame_cpu_readback_fallback() {
        let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
            return;
        };
        let base = solid_canvas(Rgba::new(20, 40, 60, 255));
        let overlay = solid_canvas(Rgba::new(200, 40, 80, 192));
        sparkleflinger.compose(CompositionPlan::single(
            2,
            2,
            CompositionLayer::replace(ProducerFrame::Canvas(base.clone())),
        ));
        let gpu_frame = sparkleflinger
            .current_output_frame()
            .expect("GPU output frame export should not fail")
            .expect("GPU output frame should be available");
        let fallback_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::GpuTexture(gpu_frame)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        );

        let composed = sparkleflinger.compose_for_outputs(
            fallback_plan,
            true,
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
        );

        assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
        assert!(composed.gpu_readback_failed);
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert_eq!(
            composed
                .preview_surface
                .expect("scaled fallback preview should remain available")
                .rgba_bytes(),
            &[0, 0, 0, 255],
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn sparkleflinger_gpu_readback_failure_composes_black() {
        let mut preview_surface_pool = new_preview_surface_pool();
        let composed = gpu_frame_without_cpu_fallback(
            2,
            2,
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
            &mut preview_surface_pool,
        );

        assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
        assert!(composed.gpu_readback_failed);
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert_eq!(
            composed
                .preview_surface
                .expect("scaled fallback preview should remain available")
                .rgba_bytes(),
            &[0, 0, 0, 255],
        );
    }

    #[test]
    fn sparkleflinger_face_overlay_uses_black_when_scene_dims_do_not_match_face() {
        let scene =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 7, 11);
        let mut face_canvas = Canvas::new(1, 1);
        face_canvas.fill(Rgba::new(0, 0, 255, 255));
        let face = PublishedSurface::from_owned_canvas(face_canvas, 8, 12);
        let black = PublishedSurface::from_owned_canvas(Canvas::new(1, 1), 0, 0);
        let expected = legacy_face_overlay_rgba(&black, &face, DisplayFaceBlendMode::Tint, 0.75);
        let mut sparkleflinger = SparkleFlinger::cpu();

        let surface =
            sparkleflinger.compose_face_overlay(&scene, &face, DisplayFaceBlendMode::Tint, 0.75);

        assert_eq!(surface.rgba_bytes(), expected.as_slice());
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
    fn sparkleflinger_extended_blend_modes_use_linear_blend_math() {
        let base = Rgba::new(96, 128, 192, 255);
        let overlay = Rgba::new(128, 96, 64, 255);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::from_parts(
                    ProducerFrame::Canvas(solid_canvas(overlay)),
                    CompositionMode::Multiply,
                    1.0,
                    false,
                ),
            ],
        ));

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("CPU multiply compose should materialize a canvas")
                .get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Multiply, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_transform_fit_modes_sample_expected_pixels() {
        let red = Rgba::new(255, 0, 0, 255);
        let green = Rgba::new(0, 255, 0, 255);
        let source = row_canvas(&[red, green]);

        let stretch = compose_transformed_source(source.clone(), 4, 1, FitMode::Stretch);
        assert_eq!(stretch.get_pixel(0, 0), red);
        assert_eq!(stretch.get_pixel(1, 0), red);
        assert_eq!(stretch.get_pixel(2, 0), green);
        assert_eq!(stretch.get_pixel(3, 0), green);

        let tile = compose_transformed_source(source.clone(), 4, 1, FitMode::Tile);
        assert_eq!(tile.get_pixel(0, 0), red);
        assert_eq!(tile.get_pixel(1, 0), green);
        assert_eq!(tile.get_pixel(2, 0), red);
        assert_eq!(tile.get_pixel(3, 0), green);

        let mirror = compose_transformed_source(source.clone(), 4, 1, FitMode::Mirror);
        assert_eq!(mirror.get_pixel(0, 0), red);
        assert_eq!(mirror.get_pixel(1, 0), green);
        assert_eq!(mirror.get_pixel(2, 0), green);
        assert_eq!(mirror.get_pixel(3, 0), red);

        let contain = compose_transformed_source(source, 4, 4, FitMode::Contain);
        assert_eq!(contain.get_pixel(0, 0), Rgba::TRANSPARENT);
        assert_eq!(contain.get_pixel(0, 1), red);
        assert_eq!(contain.get_pixel(3, 2), green);
        assert_eq!(contain.get_pixel(0, 3), Rgba::TRANSPARENT);
    }

    #[test]
    fn sparkleflinger_layer_adjust_applies_before_blending() {
        let mut sparkleflinger = SparkleFlinger::cpu();
        let adjusted = sparkleflinger.compose(CompositionPlan::single(
            2,
            2,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::WHITE)))
                .with_adjust(CompositionAdjust {
                    tint: [0.0, 0.0, 1.0, 1.0],
                    tint_strength: 1.0,
                    ..CompositionAdjust::default()
                }),
        ));

        assert_eq!(
            adjusted
                .sampling_canvas
                .as_ref()
                .expect("adjusted layer should materialize a canvas")
                .get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
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
