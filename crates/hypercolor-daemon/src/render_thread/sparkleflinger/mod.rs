mod cpu;
mod face_overlay;
#[cfg(feature = "wgpu")]
pub(crate) mod gpu;
#[cfg(feature = "wgpu")]
mod gpu_area_sat;
#[cfg(feature = "wgpu")]
mod gpu_sampling;
mod transform;

#[cfg(all(feature = "wgpu", feature = "allocation-contract-tests"))]
#[doc(hidden)]
pub struct ProjectedLookupAllocationFixture(gpu::GpuProjectedLookupAllocationFixture);

#[cfg(all(feature = "wgpu", feature = "allocation-contract-tests"))]
impl ProjectedLookupAllocationFixture {
    #[must_use]
    pub fn new(source_count: usize) -> Self {
        Self(gpu::GpuProjectedLookupAllocationFixture::new(source_count))
    }

    #[must_use]
    pub fn run_round(&self) -> bool {
        self.0.run_round()
    }
}

use anyhow::{Result, bail};
#[cfg(feature = "wgpu")]
use hypercolor_core::bus::DisplayYuv420Frame;
#[cfg(all(feature = "wgpu", target_os = "windows"))]
use hypercolor_core::input::screen::ScreenBranchPublication;
use hypercolor_core::input::screen::ScreenNativeExecutionTarget;
use hypercolor_core::spatial::PreparedZonePlan;
#[cfg(feature = "wgpu")]
use hypercolor_core::types::canvas::Rgba;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor, SurfaceLease,
    SurfaceStateCounts,
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
pub(crate) struct ProjectedGroupTextureRequirement {
    pub(crate) group_id: ZoneId,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

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
    pub(crate) sample_target_space: bool,
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
            sample_target_space: false,
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
            sample_target_space: false,
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

    fn into_base_layer(mut self) -> Self {
        self.mode = CompositionMode::Replace;
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
        if matches!(self.frame, ProducerFrame::ScreenPublication(_)) {
            return false;
        }
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

    /// Mirrors the direct-copy fast path in `compose_layer_into_gpu`: a fully
    /// opaque replace layer with no processing copies straight into the output
    /// texture without reading the current surface.
    #[cfg(feature = "wgpu")]
    fn replaces_output_directly(&self, width: u32, height: u32) -> bool {
        self.mode == CompositionMode::Replace
            && self.opacity >= 1.0
            && !self.needs_processing_for_size(width, height)
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn gpu_frame_identity_for_test(
        &self,
    ) -> Option<(u64, u64, super::producer_queue::GpuTextureFrameOrigin)> {
        let ProducerFrame::GpuTexture(frame) = &self.frame else {
            return None;
        };
        Some((frame.storage_id, frame.content_generation, frame.origin))
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn gpu_frame_lease_count_for_test(&self) -> Option<usize> {
        let ProducerFrame::GpuTexture(frame) = &self.frame else {
            return None;
        };
        frame
            .immutable_lease
            .as_ref()
            .map(std::sync::Arc::strong_count)
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
            layers: vec![layer.into_base_layer()],
            cpu_replay_cacheable: true,
        }
    }

    pub fn with_layers(width: u32, height: u32, layers: Vec<CompositionLayer>) -> Self {
        let mut layers = layers;
        if let Some(layer) = layers.first_mut() {
            layer.mode = CompositionMode::Replace;
        }
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

    #[cfg(feature = "wgpu")]
    fn into_layers(self) -> Vec<CompositionLayer> {
        self.layers
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
struct ReusableCompositionPlan<'a> {
    target: &'a mut Vec<CompositionLayer>,
    plan: Option<CompositionPlan>,
}

#[cfg(feature = "wgpu")]
impl<'a> ReusableCompositionPlan<'a> {
    fn new(target: &'a mut Vec<CompositionLayer>, width: u32, height: u32) -> Self {
        Self {
            plan: Some(
                CompositionPlan::with_layers(width, height, std::mem::take(target))
                    .with_cpu_replay_cacheable(false),
            ),
            target,
        }
    }

    fn plan(&self) -> &CompositionPlan {
        self.plan
            .as_ref()
            .expect("reusable composition plan remains installed until drop")
    }
}

#[cfg(feature = "wgpu")]
impl Drop for ReusableCompositionPlan<'_> {
    fn drop(&mut self) {
        if let Some(plan) = self.plan.take() {
            *self.target = plan.into_layers();
        }
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

#[cfg_attr(not(feature = "wgpu"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DisplayFinalizeCacheKey {
    pub(crate) group_id: ZoneId,
    pub(crate) device_id: DeviceId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) circular: bool,
    pub(crate) frame_format: DisplayFrameFormat,
}

#[cfg_attr(not(feature = "wgpu"), allow(dead_code))]
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
    screen_plan_generation: Option<u64>,
    active_preview_request: Option<PreviewSurfaceRequest>,
    preview_surface_pool: RenderSurfacePool,
    composition_surface_pool: RenderSurfacePool,
    face_overlay_surface_pool: RenderSurfacePool,
}

pub(crate) enum SparkleFlingerCanvasPreparation {
    Cpu(cpu::CpuCanvasPreparation),
    #[cfg(feature = "wgpu")]
    Gpu(Box<gpu::GpuCanvasPreparation>),
    #[cfg(feature = "wgpu")]
    GpuCpuFallback(cpu::CpuCanvasPreparation),
}

pub(crate) enum SparkleFlingerSamplingPreparation {
    Cpu,
    #[cfg(feature = "wgpu")]
    Gpu(gpu_sampling::GpuSamplingPreparation),
}

impl SparkleFlingerSamplingPreparation {
    pub(crate) const fn requires_cpu_sampling(&self) -> bool {
        match self {
            Self::Cpu => true,
            #[cfg(feature = "wgpu")]
            Self::Gpu(preparation) => !preparation.is_admitted(),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) const fn is_admitted(&self) -> bool {
        match self {
            Self::Cpu => false,
            Self::Gpu(preparation) => preparation.is_admitted(),
        }
    }
}

pub(crate) enum SparkleFlingerProjectedScenePreparation {
    Cpu,
    #[cfg(feature = "wgpu")]
    Gpu(gpu::GpuProjectedScenePreparation),
}

impl SparkleFlingerProjectedScenePreparation {
    pub(crate) const fn gpu_projection_admitted(&self) -> bool {
        match self {
            Self::Cpu => false,
            #[cfg(feature = "wgpu")]
            Self::Gpu(preparation) => preparation.is_admitted(),
        }
    }
}

impl SparkleFlingerCanvasPreparation {
    pub(crate) const fn requires_cpu_sampling(&self) -> bool {
        match self {
            Self::Cpu(_) => true,
            #[cfg(feature = "wgpu")]
            Self::Gpu(preparation) => !preparation.is_admitted(),
            #[cfg(feature = "wgpu")]
            Self::GpuCpuFallback(_) => true,
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn is_admitted(&self) -> bool {
        match self {
            Self::Cpu(_) => true,
            #[cfg(feature = "wgpu")]
            Self::Gpu(preparation) => preparation.is_admitted(),
            #[cfg(feature = "wgpu")]
            Self::GpuCpuFallback(_) => false,
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn gpu_output_admitted(&self) -> bool {
        matches!(self, Self::Gpu(preparation) if preparation.is_admitted())
    }

    #[cfg(feature = "wgpu")]
    fn gpu_preparation(&self) -> Option<&gpu::GpuCanvasPreparation> {
        match self {
            Self::Gpu(preparation) => Some(preparation.as_ref()),
            Self::Cpu(_) | Self::GpuCpuFallback(_) => None,
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn force_cpu_fallback(&mut self, width: u32, height: u32) -> Result<()> {
        if matches!(self, Self::Gpu(_)) {
            *self = Self::GpuCpuFallback(cpu::CpuCanvasPreparation::try_new(width, height)?);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SparkleFlingerSurfacePoolCounts {
    pub(crate) preview: SurfaceStateCounts,
    pub(crate) compositor: SurfaceStateCounts,
}

impl SparkleFlingerSurfacePoolCounts {
    #[cfg(feature = "wgpu")]
    pub(crate) fn merged(self, other: Self) -> Self {
        Self {
            preview: merge_surface_state_counts(self.preview, other.preview),
            compositor: merge_surface_state_counts(self.compositor, other.compositor),
        }
    }
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
    #[cfg_attr(not(feature = "wgpu"), allow(unused_variables))]
    pub(crate) fn prepare_zone_sampling_plan(
        &mut self,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
    ) -> SparkleFlingerSamplingPreparation {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => SparkleFlingerSamplingPreparation::Cpu,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => SparkleFlingerSamplingPreparation::Gpu(
                gpu.prepare_zone_sampling_plan(width, height, prepared_zones),
            ),
        }
    }

    pub(crate) fn apply_zone_sampling_plan(
        &mut self,
        preparation: SparkleFlingerSamplingPreparation,
    ) {
        match (&mut self.backend, preparation) {
            (SparkleFlingerBackend::Cpu(_), SparkleFlingerSamplingPreparation::Cpu) => {}
            #[cfg(feature = "wgpu")]
            (
                SparkleFlingerBackend::Gpu { gpu, .. },
                SparkleFlingerSamplingPreparation::Gpu(preparation),
            ) => gpu.apply_zone_sampling_plan(preparation),
            #[allow(
                unreachable_patterns,
                reason = "the mismatch arm is reachable only in wgpu builds"
            )]
            _ => unreachable!("sampling preparation must belong to its SparkleFlinger backend"),
        }
    }

    pub(crate) fn prepare_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<SparkleFlingerCanvasPreparation> {
        #[cfg(feature = "wgpu")]
        let active_preview_request = self.active_preview_request;
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(SparkleFlingerCanvasPreparation::Cpu(
                cpu::CpuCanvasPreparation::try_new(width, height)?,
            )),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                let preparation = gpu.prepare_canvas_resize(width, height, active_preview_request);
                if preparation.is_admitted() {
                    Ok(SparkleFlingerCanvasPreparation::Gpu(Box::new(preparation)))
                } else {
                    Ok(SparkleFlingerCanvasPreparation::GpuCpuFallback(
                        cpu::CpuCanvasPreparation::try_new(width, height)?,
                    ))
                }
            }
        }
    }

    pub(crate) fn apply_canvas_resize(&mut self, preparation: SparkleFlingerCanvasPreparation) {
        match (&mut self.backend, preparation) {
            (
                SparkleFlingerBackend::Cpu(cpu),
                SparkleFlingerCanvasPreparation::Cpu(preparation),
            ) => preparation.apply(
                cpu,
                &mut self.preview_surface_pool,
                &mut self.composition_surface_pool,
            ),
            #[cfg(feature = "wgpu")]
            (
                SparkleFlingerBackend::Gpu { gpu, .. },
                SparkleFlingerCanvasPreparation::Gpu(preparation),
            ) => gpu.apply_canvas_resize(*preparation),
            #[cfg(feature = "wgpu")]
            (
                SparkleFlingerBackend::Gpu { gpu, cpu_fallback },
                SparkleFlingerCanvasPreparation::GpuCpuFallback(preparation),
            ) => {
                gpu.apply_canvas_resize(gpu::GpuCanvasPreparation::cpu_fallback());
                preparation.apply(
                    cpu_fallback,
                    &mut self.preview_surface_pool,
                    &mut self.composition_surface_pool,
                );
            }
            #[allow(
                unreachable_patterns,
                reason = "the mismatch arm is reachable only in wgpu builds"
            )]
            _ => unreachable!("canvas preparation must belong to its SparkleFlinger backend"),
        }
    }

    pub(crate) fn synchronize_screen_plan_generation(&mut self, generation: u64) -> bool {
        let previous = self.screen_plan_generation.replace(generation);
        let changed = previous.is_some_and(|previous| previous != generation);
        if changed {
            self.release_native_screen_caches();
        }
        changed
    }

    pub(crate) fn screen_native_execution_target(&self) -> Option<&ScreenNativeExecutionTarget> {
        #[cfg(all(feature = "wgpu", target_os = "windows"))]
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &self.backend {
            return gpu.screen_native_execution_target();
        }
        None
    }

    pub(crate) fn release_native_screen_caches(&mut self) {
        #[cfg(all(feature = "wgpu", target_os = "windows"))]
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &mut self.backend {
            gpu.release_native_screen_caches();
        }
    }

    #[cfg(all(feature = "wgpu", target_os = "windows"))]
    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &std::sync::Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        match &mut self.backend {
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.copy_screen_publication(publication),
            SparkleFlingerBackend::Cpu(_) => Ok(None),
        }
    }

    pub(crate) fn surface_pool_counts(&mut self) -> SparkleFlingerSurfacePoolCounts {
        let preview = self.preview_surface_pool.slot_counts();
        let composition = self.composition_surface_pool.slot_counts();
        let face_overlay = self.face_overlay_surface_pool.slot_counts();
        let counts = SparkleFlingerSurfacePoolCounts {
            preview,
            compositor: merge_surface_state_counts(composition, face_overlay),
        };
        #[cfg(feature = "wgpu")]
        let counts = match &mut self.backend {
            SparkleFlingerBackend::Gpu { gpu, .. } => counts.merged(gpu.surface_pool_counts()),
            SparkleFlingerBackend::Cpu(_) => counts,
        };
        counts
    }

    pub fn cpu() -> Self {
        Self {
            backend: SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new()),
            screen_plan_generation: None,
            active_preview_request: None,
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

    #[cfg(all(test, feature = "wgpu", target_os = "windows"))]
    pub(crate) fn new_required_dx12_for_test() -> Result<Self> {
        let render_device =
            GpuRenderDevice::new_dx12_required("SparkleFlinger required DX12 test compositor")?;
        Self::new_with_gpu_device(RenderAccelerationMode::Gpu, Some(render_device))
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
            screen_plan_generation: None,
            active_preview_request: None,
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
        let (composed, commit_preview_request) = match &mut self.backend {
            SparkleFlingerBackend::Cpu(backend) => (
                backend.compose_with_surface_pools(
                    plan,
                    requires_cpu_sampling_canvas,
                    preview_surface_request,
                    &mut self.preview_surface_pool,
                    &mut self.composition_surface_pool,
                ),
                true,
            ),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, cpu_fallback } => {
                let failure_width = plan.width;
                let failure_height = plan.height;
                let contains_gpu_frames = plan.contains_gpu_frames();
                let gpu_compose_outcome = if gpu.supports_plan(&plan) {
                    Some(gpu.compose_attempt(
                        &plan,
                        requires_cpu_sampling_canvas,
                        preview_surface_request,
                    ))
                } else {
                    None
                };
                match gpu_compose_outcome {
                    Some(gpu::GpuComposeOutcome::Produced(composed)) => (composed, true),
                    Some(gpu::GpuComposeOutcome::Retained(composed)) => (composed, false),
                    Some(gpu::GpuComposeOutcome::Failed(error)) if contains_gpu_frames => {
                        tracing::warn!(
                            %error,
                            "GPU producer composition failed; refusing CPU readback fallback"
                        );
                        (
                            gpu_frame_without_cpu_fallback(
                                failure_width,
                                failure_height,
                                preview_surface_request,
                                &mut self.preview_surface_pool,
                            ),
                            false,
                        )
                    }
                    None if contains_gpu_frames => {
                        tracing::warn!(
                            "Unsupported GPU producer plan; refusing CPU readback fallback"
                        );
                        (
                            gpu_frame_without_cpu_fallback(
                                failure_width,
                                failure_height,
                                preview_surface_request,
                                &mut self.preview_surface_pool,
                            ),
                            false,
                        )
                    }
                    Some(gpu::GpuComposeOutcome::Failed(_)) | None => {
                        let mut composed = cpu_fallback.compose_with_surface_pools(
                            plan,
                            requires_cpu_sampling_canvas,
                            preview_surface_request,
                            &mut self.preview_surface_pool,
                            &mut self.composition_surface_pool,
                        );
                        composed.backend = CompositorBackendKind::GpuFallback;
                        (composed, true)
                    }
                }
            }
        };
        if commit_preview_request {
            self.active_preview_request = preview_surface_request;
        }
        composed
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn compose_projected_scene_layers(
        &mut self,
        width: u32,
        height: u32,
        layers: &mut Vec<CompositionLayer>,
    ) -> ComposedFrameSet {
        let reusable = ReusableCompositionPlan::new(layers, width, height);
        let plan = reusable.plan();
        let SparkleFlingerBackend::Gpu { gpu, .. } = &mut self.backend else {
            return gpu_frame_without_cpu_fallback(
                width,
                height,
                None,
                &mut self.preview_surface_pool,
            );
        };
        if !gpu.supports_plan(plan) {
            tracing::warn!("Unsupported projected GPU scene plan; refusing CPU fallback");
            return gpu_frame_without_cpu_fallback(
                width,
                height,
                None,
                &mut self.preview_surface_pool,
            );
        }
        match gpu.compose(plan, false, None) {
            Ok(composed) => composed,
            Err(error) => {
                tracing::warn!(%error, "GPU projected scene composition failed");
                gpu_frame_without_cpu_fallback(width, height, None, &mut self.preview_surface_pool)
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn materialize_output_surface(
        &mut self,
        frame: ProducerFrame,
    ) -> Option<PublishedSurface> {
        if frame.width() == 0 || frame.height() == 0 {
            return None;
        }

        let output_request = PreviewSurfaceRequest {
            width: frame.width(),
            height: frame.height(),
        };
        let plan = CompositionPlan::single(
            output_request.width,
            output_request.height,
            CompositionLayer::replace(frame),
        )
        .with_cpu_replay_cacheable(false);
        let composed = self.compose_for_outputs(plan, false, Some(output_request));

        if composed.gpu_readback_failed {
            tracing::debug!(
                "GPU output surface materialization missed; retaining prior frame if available"
            );
        }

        composed.preview_surface.or(composed.sampling_surface)
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn begin_media_upload_frame(&mut self) {
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &mut self.backend {
            gpu.begin_media_upload_frame();
        }
    }

    pub(crate) fn prepare_projected_scene_resources(
        &self,
        requirements: &[ProjectedGroupTextureRequirement],
        gpu_projection_admitted: bool,
        scene_width: u32,
        scene_height: u32,
        canvas_preparation: Option<&SparkleFlingerCanvasPreparation>,
    ) -> SparkleFlingerProjectedScenePreparation {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = (
                    requirements,
                    gpu_projection_admitted,
                    scene_width,
                    scene_height,
                    canvas_preparation,
                );
                SparkleFlingerProjectedScenePreparation::Cpu
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                SparkleFlingerProjectedScenePreparation::Gpu(gpu.prepare_projected_scene_resources(
                    requirements,
                    gpu_projection_admitted,
                    scene_width,
                    scene_height,
                    canvas_preparation.and_then(SparkleFlingerCanvasPreparation::gpu_preparation),
                ))
            }
        }
    }

    pub(crate) fn apply_projected_scene_resources(
        &mut self,
        preparation: SparkleFlingerProjectedScenePreparation,
    ) {
        match (&mut self.backend, preparation) {
            (SparkleFlingerBackend::Cpu(_), SparkleFlingerProjectedScenePreparation::Cpu) => {}
            #[cfg(feature = "wgpu")]
            (
                SparkleFlingerBackend::Gpu { gpu, .. },
                SparkleFlingerProjectedScenePreparation::Gpu(preparation),
            ) => gpu.apply_projected_scene_resources(preparation),
            #[cfg(feature = "wgpu")]
            _ => unreachable!("projection preparation must belong to its SparkleFlinger backend"),
        }
    }

    pub(crate) fn has_projected_group_resource(
        &self,
        group_id: ZoneId,
        width: u32,
        height: u32,
    ) -> bool {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = (group_id, width, height);
                false
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.has_projected_group_resource(group_id, width, height)
            }
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the wrapper preserves the fallible GPU snapshot contract in CPU-only builds"
    )]
    pub(crate) fn stabilize_projected_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: ProducerFrame,
    ) -> Result<ProducerFrame> {
        #[cfg(feature = "wgpu")]
        {
            return match (&mut self.backend, frame) {
                (
                    SparkleFlingerBackend::Gpu { gpu, .. },
                    ProducerFrame::GpuTexture(gpu_frame),
                ) if gpu_frame.origin
                    == crate::render_thread::producer_queue::GpuTextureFrameOrigin::CompositorOutput =>
                {
                    gpu.snapshot_projected_group_frame(group_id, gpu_frame)
                        .map(ProducerFrame::GpuTexture)
                }
                (_, frame) => Ok(frame),
            };
        }
        #[cfg(not(feature = "wgpu"))]
        {
            let _ = group_id;
            Ok(frame)
        }
    }

    pub(crate) fn supports_gpu_output_frames(&self) -> bool {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.canvas_gpu_admitted(),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn max_texture_dimension_2d(&self) -> Option<u32> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.max_texture_dimension_2d()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn gpu_backend_name_for_test(&self) -> Option<&str> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.backend_name()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn snapshot_texture_allocation_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.snapshot_texture_allocation_count()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn compositor_surface_allocation_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.compositor_surface_allocation_count())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn projected_bind_group_creation_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.projected_bind_group_creation_count())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn projected_bind_group_entry_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.projected_bind_group_entry_count()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn projected_bind_group_source_storage_ids_for_test(&self) -> Option<Vec<u64>> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.projected_bind_group_source_storage_ids())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn retired_projected_bind_group_entry_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.retired_projected_bind_group_entry_count())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn projected_snapshot_retained_bytes_for_test(&self) -> Option<u64> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.projected_snapshot_retained_bytes()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn compositor_surface_cache_entry_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.compositor_surface_cache_entry_count())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn screen_layer_host_allocation_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                Some(gpu.screen_layer_host_allocation_count())
            }
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn active_surface_generation_for_test(&self) -> Option<u64> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.active_surface_generation(),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn fail_next_projected_scene_preparation_for_test(&self) {
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &self.backend {
            gpu.fail_next_projected_scene_preparation();
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
        self.active_preview_request = preview_surface_request;

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
    pub fn can_sample_zone_plan(&mut self, prepared_zones: &[PreparedZonePlan]) -> bool {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.can_sample_zone_plan(prepared_zones),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn fail_next_gpu_sampling_preparation(&mut self) -> bool {
        match &mut self.backend {
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.fail_next_sampling_preparation();
                true
            }
            SparkleFlingerBackend::Cpu(_) => false,
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

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the wrapper preserves the fallible GPU snapshot contract in CPU-only builds"
    )]
    pub(crate) fn stabilize_scene_frame(&mut self, frame: ProducerFrame) -> Result<ProducerFrame> {
        #[cfg(feature = "wgpu")]
        {
            return match (&mut self.backend, frame) {
                (SparkleFlingerBackend::Gpu { gpu, .. }, ProducerFrame::GpuTexture(gpu_frame)) => {
                    gpu.snapshot_scene_frame(gpu_frame)
                        .map(ProducerFrame::GpuTexture)
                }
                (_, frame) => Ok(frame),
            };
        }
        #[cfg(not(feature = "wgpu"))]
        {
            Ok(frame)
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn opaque_black_gpu_frame(&self) -> Option<ProducerFrame> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } if gpu.canvas_gpu_admitted() => {
                gpu.opaque_black_frame().map(ProducerFrame::GpuTexture)
            }
            SparkleFlingerBackend::Gpu { .. } => None,
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn immutable_current_output_frame(&mut self) -> Result<Option<ProducerFrame>> {
        if let SparkleFlingerBackend::Gpu { gpu, .. } = &mut self.backend {
            return gpu
                .snapshot_current_output_frame()
                .map(|frame| frame.map(ProducerFrame::GpuTexture));
        }
        Ok(None)
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the wrapper preserves the fallible GPU restore contract in CPU-only builds"
    )]
    pub(crate) fn restore_scene_frame(&mut self, frame: &ProducerFrame) -> Result<bool> {
        #[cfg(feature = "wgpu")]
        if let (SparkleFlingerBackend::Gpu { gpu, .. }, ProducerFrame::GpuTexture(gpu_frame)) =
            (&mut self.backend, frame)
        {
            gpu.restore_scene_frame(gpu_frame)?;
            return Ok(true);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = frame;
        Ok(false)
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

fn merge_surface_state_counts(
    mut total: SurfaceStateCounts,
    counts: SurfaceStateCounts,
) -> SurfaceStateCounts {
    total.free = total.free.saturating_add(counts.free);
    total.dequeued = total.dequeued.saturating_add(counts.dequeued);
    total.published = total.published.saturating_add(counts.published);
    total
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
    let preview_surface = preview_surface_request
        .filter(|request| request.width < width || request.height < height)
        .and_then(|request| black_preview_surface(request, preview_surface_pool));
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
        ProducerFrame::ScreenPublication(_) => scaled_preview_surface_from_rgba(
            frame.cpu_rgba_bytes()?,
            frame.width(),
            frame.height(),
            request,
            preview_surface_pool,
        ),
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

    let mut lease = dequeue_preview_lease(request, preview_surface_pool)?;
    let preview_bytes = lease.canvas_mut().as_rgba_bytes_mut();
    let source_width_usize = usize::try_from(source_width).ok()?;
    let source_height_usize = usize::try_from(source_height).ok()?;
    let target_width_usize = usize::try_from(request.width).ok()?;
    let target_height_usize = usize::try_from(request.height).ok()?;

    // Nearest-neighbor scale: precompute the source byte offset per target
    // column once instead of redoing the divide for every pixel.
    let mut source_x_offsets = Vec::with_capacity(target_width_usize);
    for x in 0..target_width_usize {
        let source_x = x
            .saturating_mul(source_width_usize)
            .checked_div(target_width_usize.max(1))?
            .min(source_width_usize.saturating_sub(1));
        source_x_offsets.push(source_x.checked_mul(4)?);
    }

    let target_row_bytes = target_width_usize.checked_mul(4)?;
    for (y, target_row) in preview_bytes
        .chunks_exact_mut(target_row_bytes.max(1))
        .take(target_height_usize)
        .enumerate()
    {
        let source_y = y
            .saturating_mul(source_height_usize)
            .checked_div(target_height_usize.max(1))?
            .min(source_height_usize.saturating_sub(1));
        let source_row_offset = source_y.checked_mul(source_width_usize)?.checked_mul(4)?;
        for (x, source_x_offset) in source_x_offsets.iter().enumerate() {
            let source_offset = source_row_offset.checked_add(*source_x_offset)?;
            let target_offset = x * 4;
            target_row[target_offset..target_offset + 4]
                .copy_from_slice(&rgba[source_offset..source_offset + 4]);
        }
    }

    Some(lease.submit(0, 0))
}

#[cfg(feature = "wgpu")]
fn black_preview_surface(
    request: PreviewSurfaceRequest,
    preview_surface_pool: &mut RenderSurfacePool,
) -> Option<PublishedSurface> {
    if request.width == 0 || request.height == 0 {
        return None;
    }
    let mut lease = dequeue_preview_lease(request, preview_surface_pool)?;
    lease.canvas_mut().fill(Rgba::new(0, 0, 0, 255));
    Some(lease.submit(0, 0))
}

fn dequeue_preview_lease(
    request: PreviewSurfaceRequest,
    preview_surface_pool: &mut RenderSurfacePool,
) -> Option<SurfaceLease<'_>> {
    let descriptor = SurfaceDescriptor::rgba8888(request.width, request.height);
    if preview_surface_pool.descriptor() != descriptor {
        *preview_surface_pool = RenderSurfacePool::try_with_slot_count(descriptor, 2).ok()?;
    }
    preview_surface_pool.try_dequeue().ok().flatten()
}

#[cfg(test)]
mod tests;
