mod cpu;
#[cfg(feature = "wgpu")]
pub(crate) mod gpu;

use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use hypercolor_types::config::RenderAccelerationMode;

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
}

impl CompositionLayer {
    pub fn replace_canvas(canvas: Canvas) -> Self {
        Self::replace(ProducerFrame::Canvas(canvas))
    }

    pub fn replace_surface(surface: PublishedSurface) -> Self {
        Self::replace(ProducerFrame::Surface(surface))
    }

    pub fn alpha_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::alpha(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn add_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::add(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn screen_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::screen(ProducerFrame::Canvas(canvas), opacity)
    }

    pub(crate) fn replace(frame: ProducerFrame) -> Self {
        Self {
            frame,
            mode: CompositionMode::Replace,
            opacity: 1.0,
        }
    }

    pub(crate) fn from_parts(frame: ProducerFrame, mode: CompositionMode, opacity: f32) -> Self {
        Self {
            frame,
            mode,
            opacity,
        }
    }

    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
        }
    }

    pub(crate) fn add(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Add,
            opacity,
        }
    }

    pub(crate) fn screen(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Screen,
            opacity,
        }
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
}

impl CompositionPlan {
    pub fn single(width: u32, height: u32, layer: CompositionLayer) -> Self {
        Self {
            width,
            height,
            layers: vec![layer],
        }
    }

    pub fn with_layers(width: u32, height: u32, layers: Vec<CompositionLayer>) -> Self {
        Self {
            width,
            height,
            layers,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComposedFrameSet {
    pub sampling_canvas: Canvas,
    pub sampling_surface: Option<PublishedSurface>,
    pub preview_surface: Option<PublishedSurface>,
    pub bypassed: bool,
}

pub(crate) type RenderFrame = (Canvas, Option<PublishedSurface>);

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
    pub fn new(mode: RenderAccelerationMode) -> Self {
        let backend = match mode {
            RenderAccelerationMode::Cpu | RenderAccelerationMode::Auto => {
                SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new())
            }
            RenderAccelerationMode::Gpu => new_gpu_backend(),
        };
        Self { backend }
    }

    pub fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(backend) => backend.compose(plan),
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, cpu_fallback } => {
                if gpu.supports_plan(&plan)
                    && let Ok(composed) = gpu.compose(plan.clone())
                {
                    return composed;
                }
                cpu_fallback.compose(plan)
            }
        }
    }
}

#[cfg(feature = "wgpu")]
fn new_gpu_backend() -> SparkleFlingerBackend {
    match gpu::GpuSparkleFlinger::new() {
        Ok(gpu) => SparkleFlingerBackend::Gpu {
            gpu,
            cpu_fallback: cpu::CpuSparkleFlinger::new(),
        },
        Err(_) => SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new()),
    }
}

#[cfg(not(feature = "wgpu"))]
fn new_gpu_backend() -> SparkleFlingerBackend {
    SparkleFlingerBackend::Cpu(cpu::CpuSparkleFlinger::new())
}

pub(super) fn publish_composed_frame(frame: RenderFrame, bypassed: bool) -> ComposedFrameSet {
    let (sampling_canvas, sampling_surface) = frame;
    if let Some(sampling_surface) = sampling_surface {
        return ComposedFrameSet {
            sampling_canvas,
            sampling_surface: Some(sampling_surface),
            preview_surface: None,
            bypassed,
        };
    }

    let sampling_surface = PublishedSurface::from_owned_canvas(sampling_canvas, 0, 0);
    let sampling_canvas = Canvas::from_published_surface(&sampling_surface);
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface: Some(sampling_surface),
        preview_surface: None,
        bypassed,
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};
    use hypercolor_types::config::RenderAccelerationMode;

    use super::{CompositionLayer, CompositionPlan, SparkleFlinger};
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
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
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
            composed.sampling_canvas.as_rgba_bytes().as_ptr(),
            source.rgba_bytes().as_ptr()
        );
    }

    #[test]
    fn sparkleflinger_alpha_layers_respect_order() {
        let base = Rgba::new(255, 0, 0, 255);
        let overlay = Rgba::new(0, 0, 255, 255);
        let opacity = 0.25;
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
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
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Normal, opacity)
        );
        assert_ne!(
            composed.sampling_canvas.get_pixel(0, 0),
            reversed.sampling_canvas.get_pixel(0, 0)
        );
        let composed_surface = composed
            .sampling_surface
            .expect("composed frame should publish an immutable sampling surface");
        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes().as_ptr(),
            composed_surface.rgba_bytes().as_ptr()
        );
    }

    #[test]
    fn sparkleflinger_add_layers_use_additive_blend() {
        let base = Rgba::new(64, 0, 0, 255);
        let glow = Rgba::new(0, 96, 64, 255);
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::add(ProducerFrame::Canvas(solid_canvas(glow)), 1.0),
            ],
        ));

        assert_eq!(
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, glow, BlendMode::Add, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_screen_layers_use_screen_blend() {
        let base = Rgba::new(32, 64, 96, 255);
        let overlay = Rgba::new(96, 64, 32, 255);
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::screen(ProducerFrame::Canvas(solid_canvas(overlay)), 1.0),
            ],
        ));

        assert_eq!(
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Screen, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_reuses_first_replace_canvas_for_multi_layer_plans() {
        let base = solid_canvas(Rgba::new(255, 0, 0, 255));
        let base_ptr = base.as_rgba_bytes().as_ptr();
        let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        ));

        assert_eq!(composed.sampling_canvas.as_rgba_bytes().as_ptr(), base_ptr);
    }
}
