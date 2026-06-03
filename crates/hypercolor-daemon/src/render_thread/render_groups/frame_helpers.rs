use hypercolor_core::input::ScreenData;
use hypercolor_core::spatial::sample_viewport;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, Rgba, RgbaF32};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::Zone;
use hypercolor_types::viewport::{FitMode, ViewportRect};

#[cfg(feature = "wgpu")]
use super::super::sparkleflinger::MediaTextureSourceKey;
use super::super::sparkleflinger::{
    ComposedFrameSet, CompositionAdjust, CompositionLayer, CompositionMode, CompositionTransform,
    SparkleFlinger,
};
use super::super::{producer_queue::ProducerFrame, usize_to_u32};
#[cfg(feature = "wgpu")]
use crate::performance::CompositorBackendKind;
use crate::performance::FullFrameCopyMetrics;

pub(super) fn passthrough_effect_layer(group: &Zone) -> Option<SceneLayer> {
    if !group.enabled {
        return None;
    }

    let mut layers = group
        .effective_layers()
        .into_iter()
        .filter(|layer| layer.enabled);
    let layer = layers.next()?;
    if layers.next().is_some() {
        return None;
    }
    if !matches!(&layer.source, LayerSource::Effect { .. }) {
        return None;
    }
    if layer.blend != LayerBlendMode::Replace {
        return None;
    }
    if (layer.opacity - 1.0).abs() > f32::EPSILON {
        return None;
    }
    if layer.transform != LayerTransform::default() {
        return None;
    }
    if layer.adjust != LayerAdjust::default() {
        return None;
    }
    if !layer.bindings.is_empty() {
        return None;
    }

    Some(layer)
}

pub(super) fn composition_layer_for_scene_layer(
    layer: &SceneLayer,
    frame: ProducerFrame,
) -> CompositionLayer {
    CompositionLayer::from_parts(
        frame,
        composition_mode_for_layer(layer.blend),
        layer.opacity,
        false,
    )
    .with_transform(CompositionTransform::from(layer.transform))
    .with_adjust(CompositionAdjust::from(layer.adjust))
}

fn composition_mode_for_layer(blend: LayerBlendMode) -> CompositionMode {
    match blend {
        LayerBlendMode::Replace => CompositionMode::Replace,
        LayerBlendMode::Alpha => CompositionMode::Alpha,
        LayerBlendMode::Tint => CompositionMode::Tint,
        LayerBlendMode::LumaReveal => CompositionMode::LumaReveal,
        LayerBlendMode::Add => CompositionMode::Add,
        LayerBlendMode::Screen => CompositionMode::Screen,
        LayerBlendMode::Multiply => CompositionMode::Multiply,
        LayerBlendMode::Overlay => CompositionMode::Overlay,
        LayerBlendMode::SoftLight => CompositionMode::SoftLight,
        LayerBlendMode::ColorDodge => CompositionMode::ColorDodge,
        LayerBlendMode::Difference => CompositionMode::Difference,
    }
}

pub(super) fn color_fill_frame(width: u32, height: u32, rgba: [f32; 4]) -> ProducerFrame {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(RgbaF32::new(rgba[0], rgba[1], rgba[2], rgba[3]).to_srgba());
    ProducerFrame::Canvas(canvas)
}

pub(super) fn screen_region_layer_frame(
    screen: Option<&ScreenData>,
    viewport: ViewportRect,
) -> Option<ProducerFrame> {
    let source_surface = screen?.canvas_downscale.as_ref()?;
    let source = Canvas::from_published_surface(source_surface);
    if source.width() == 0 || source.height() == 0 {
        return None;
    }
    let viewport = viewport.clamp();
    let rect = viewport.to_pixel_rect(source.width(), source.height());
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let mut target = Canvas::new(rect.width, rect.height);
    sample_viewport(&mut target, &source, viewport, FitMode::Stretch, 1.0);
    Some(ProducerFrame::Canvas(target))
}

pub(super) fn transparent_black_frame(width: u32, height: u32) -> ProducerFrame {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(Rgba::TRANSPARENT);
    ProducerFrame::Canvas(canvas)
}

pub(super) fn media_layer_producer_frame(
    layer_id: SceneLayerId,
    canvas: Canvas,
    mime_type: &str,
    sparkleflinger: &mut SparkleFlinger,
) -> ProducerFrame {
    #[cfg(feature = "wgpu")]
    if media_mime_prefers_gpu_texture(mime_type)
        && let Some(frame) = sparkleflinger
            .upload_media_canvas_frame(MediaTextureSourceKey::from_media_layer(layer_id), &canvas)
    {
        return ProducerFrame::GpuTexture(frame);
    }

    #[cfg(not(feature = "wgpu"))]
    let _ = layer_id;
    #[cfg(not(feature = "wgpu"))]
    let _ = mime_type;
    #[cfg(not(feature = "wgpu"))]
    let _ = sparkleflinger;

    ProducerFrame::Canvas(canvas)
}

#[cfg(feature = "wgpu")]
pub(super) fn media_mime_prefers_gpu_texture(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "video/mp4" | "video/webm" | "application/vnd.hypercolor.stream-url"
    )
}

pub(super) fn composed_frame_to_producer_frame(
    composed: ComposedFrameSet,
    sparkleflinger: &mut SparkleFlinger,
) -> Option<ProducerFrame> {
    composed
        .sampling_surface
        .map(ProducerFrame::Surface)
        .or_else(|| composed.sampling_canvas.map(ProducerFrame::Canvas))
        .or_else(|| composed.preview_surface.map(ProducerFrame::Surface))
        .or_else(|| {
            #[cfg(feature = "wgpu")]
            {
                if composed.backend == CompositorBackendKind::Gpu && !composed.gpu_readback_failed {
                    return sparkleflinger
                        .current_output_frame()
                        .ok()
                        .flatten()
                        .map(ProducerFrame::GpuTexture);
                }
            }

            None
        })
}

pub(super) fn surface_backed_frame(
    surface_pool: &mut RenderSurfacePool,
    frame: ProducerFrame,
    full_frame_copy: &mut FullFrameCopyMetrics,
) -> Option<ProducerFrame> {
    match frame {
        ProducerFrame::Canvas(canvas) => {
            let mut lease = surface_pool.dequeue()?;
            *lease.canvas_mut() = canvas;
            Some(ProducerFrame::Surface(lease.submit(0, 0)))
        }
        ProducerFrame::Surface(surface) if surface.generation() == 0 => {
            let mut lease = surface_pool.dequeue()?;
            full_frame_copy.record(
                usize_to_u32(surface.rgba_bytes().len()),
                "generation_zero_surface_pool_materialization",
            );
            *lease.canvas_mut() =
                Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height());
            Some(ProducerFrame::Surface(
                lease.submit(surface.frame_number(), surface.timestamp_ms()),
            ))
        }
        frame => Some(frame),
    }
}

pub(super) fn copy_producer_frame_to_canvas(
    frame: ProducerFrame,
    target: &mut Canvas,
    full_frame_copy: &mut FullFrameCopyMetrics,
) -> bool {
    match frame {
        ProducerFrame::Canvas(canvas) => {
            *target = canvas;
            true
        }
        ProducerFrame::Surface(surface) => {
            full_frame_copy.record(
                usize_to_u32(surface.rgba_bytes().len()),
                "surface_to_group_canvas_materialization",
            );
            *target = Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height());
            true
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(frame) => {
            let frame = ProducerFrame::Gpu(frame);
            frame.record_cpu_materialization_blocked();
            false
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(frame) => {
            let frame = ProducerFrame::GpuTexture(frame);
            frame.record_cpu_materialization_blocked();
            false
        }
    }
}

pub(super) fn producer_frame_is_gpu(frame: &ProducerFrame) -> bool {
    match frame {
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => true,
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => true,
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => false,
    }
}
