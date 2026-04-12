mod cpu;
#[cfg(feature = "wgpu")]
pub(crate) mod gpu;
#[cfg(feature = "wgpu")]
mod gpu_sampling;

use anyhow::Result;
#[cfg(not(feature = "wgpu"))]
use anyhow::bail;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;

use crate::performance::CompositorBackendKind;

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

    pub fn with_cpu_replay_cacheable(mut self, cacheable: bool) -> Self {
        self.cpu_replay_cacheable = cacheable;
        self
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
}

impl SparkleFlinger {
    pub fn cpu() -> Self {
        Self {
            backend: SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new()),
        }
    }

    pub fn new(mode: RenderAccelerationMode) -> Result<Self> {
        let backend = match mode {
            RenderAccelerationMode::Cpu | RenderAccelerationMode::Auto => {
                SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new())
            }
            RenderAccelerationMode::Gpu => new_gpu_backend()?,
        };
        Ok(Self { backend })
    }

    pub fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        let preview_surface_request = Some(PreviewSurfaceRequest {
            width: plan.width,
            height: plan.height,
        });
        self.compose_for_outputs(
            plan,
            true,
            preview_surface_request,
        )
    }

    pub fn compose_for_outputs(
        &mut self,
        plan: CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(backend) => {
                backend.compose(
                    plan,
                    requires_cpu_sampling_canvas,
                    preview_surface_request,
                )
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, cpu_fallback } => {
                if gpu.supports_plan(&plan)
                    && let Ok(composed) = gpu.compose(
                        &plan,
                        requires_cpu_sampling_canvas,
                        preview_surface_request,
                    )
                {
                    return composed;
                }
                let mut composed = cpu_fallback.compose(
                    plan,
                    requires_cpu_sampling_canvas,
                    preview_surface_request,
                );
                composed.backend = CompositorBackendKind::GpuFallback;
                composed
            }
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

    pub fn sample_zone_plan_into(
        &mut self,
        _prepared_zones: &[PreparedZonePlan],
        _zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => Ok(false),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.sample_zone_plan_into(_prepared_zones, _zones)
            }
        }
    }

    pub fn can_sample_zone_plan(&self, _prepared_zones: &[PreparedZonePlan]) -> bool {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => false,
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.can_sample_zone_plan(_prepared_zones),
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
}

#[cfg(feature = "wgpu")]
fn new_gpu_backend() -> Result<SparkleFlingerBackend> {
    let gpu = gpu::GpuSparkleFlinger::new()?;
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

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};

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
