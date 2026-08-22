use std::collections::VecDeque;

use hypercolor_core::input::ScreenData;
use hypercolor_core::spatial::sample_viewport;
use hypercolor_types::canvas::{
    Canvas, LinearRgba, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
    SurfaceResourceError,
};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
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

const STATIC_LAYER_SURFACE_CACHE_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StaticLayerSurfaceKey {
    width: u32,
    height: u32,
    color: Rgba,
}

#[derive(Default)]
pub(super) struct StaticLayerSurfaceCache {
    surfaces: VecDeque<(StaticLayerSurfaceKey, PublishedSurface)>,
}

impl StaticLayerSurfaceCache {
    fn frame(&mut self, width: u32, height: u32, color: Rgba) -> anyhow::Result<ProducerFrame> {
        let key = StaticLayerSurfaceKey {
            width,
            height,
            color,
        };
        if let Some((_, surface)) = self.surfaces.iter().find(|(cached, _)| *cached == key) {
            return Ok(ProducerFrame::Surface(surface.clone()));
        }

        let mut pool =
            RenderSurfacePool::try_with_slot_count(SurfaceDescriptor::rgba8888(width, height), 1)?;
        let mut lease = pool
            .try_dequeue()?
            .expect("new static layer surface pool must expose its initial slot");
        lease.canvas_mut().fill(color);
        let surface = lease.submit(0, 0);
        if self.surfaces.len() == STATIC_LAYER_SURFACE_CACHE_CAPACITY {
            let _ = self.surfaces.pop_front();
        }
        self.surfaces.push_back((key, surface.clone()));
        Ok(ProducerFrame::Surface(surface))
    }

    #[cfg(test)]
    pub(super) fn entry_count(&self) -> usize {
        self.surfaces.len()
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn contains(&self, width: u32, height: u32, color: Rgba) -> bool {
        let key = StaticLayerSurfaceKey {
            width,
            height,
            color,
        };
        self.surfaces.iter().any(|(cached, _)| *cached == key)
    }
}

pub(super) fn passthrough_effect_layer(group: &Zone) -> Option<SceneLayer> {
    if !group.enabled {
        return None;
    }

    let mut layers = group.layers.iter().filter(|layer| layer.enabled);
    let layer = layers.next()?;
    if layers.next().is_some() {
        return None;
    }
    if !matches!(&layer.source, LayerSource::Effect { .. }) {
        return None;
    }
    if layer.blend != BlendMode::Replace {
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

    Some(layer.clone())
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

fn composition_mode_for_layer(blend: BlendMode) -> CompositionMode {
    match blend {
        BlendMode::Replace => CompositionMode::Replace,
        BlendMode::Alpha => CompositionMode::Alpha,
        BlendMode::Tint => CompositionMode::Tint,
        BlendMode::LumaReveal => CompositionMode::LumaReveal,
        BlendMode::Add => CompositionMode::Add,
        BlendMode::Screen => CompositionMode::Screen,
        BlendMode::Multiply => CompositionMode::Multiply,
        BlendMode::Overlay => CompositionMode::Overlay,
        BlendMode::SoftLight => CompositionMode::SoftLight,
        BlendMode::ColorDodge => CompositionMode::ColorDodge,
        BlendMode::Difference => CompositionMode::Difference,
    }
}

pub(super) fn color_fill_frame(
    cache: &mut StaticLayerSurfaceCache,
    width: u32,
    height: u32,
    rgba: [f32; 4],
) -> anyhow::Result<ProducerFrame> {
    cache.frame(
        width,
        height,
        LinearRgba::new(rgba[0], rgba[1], rgba[2], rgba[3]).to_encoded(),
    )
}

pub(super) fn screen_region_layer_frame(
    screen: Option<&ScreenData>,
    viewport: ViewportRect,
) -> anyhow::Result<Option<ProducerFrame>> {
    let Some(source_surface) = screen.and_then(|screen| screen.canvas_downscale.as_ref()) else {
        return Ok(None);
    };
    let source = Canvas::from_published_surface(source_surface);
    if source.width() == 0 || source.height() == 0 {
        return Ok(None);
    }
    let viewport = viewport.clamp();
    let rect = viewport.to_pixel_rect(source.width(), source.height());
    if rect.width == 0 || rect.height == 0 {
        return Ok(None);
    }
    let mut target = Canvas::try_new(rect.width, rect.height)?;
    sample_viewport(&mut target, &source, viewport, FitMode::Stretch, 1.0);
    Ok(Some(ProducerFrame::Canvas(target)))
}

pub(super) fn transparent_black_frame(
    cache: &mut StaticLayerSurfaceCache,
    width: u32,
    height: u32,
) -> anyhow::Result<ProducerFrame> {
    cache.frame(width, height, Rgba::TRANSPARENT)
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
    immutable_gpu_output: bool,
) -> Option<ProducerFrame> {
    #[cfg(not(feature = "wgpu"))]
    let _ = immutable_gpu_output;

    let frame = composed
        .sampling_surface
        .map(ProducerFrame::Surface)
        .or_else(|| composed.sampling_canvas.map(ProducerFrame::Canvas))
        .or_else(|| composed.preview_surface.map(ProducerFrame::Surface));

    #[cfg(feature = "wgpu")]
    let frame = frame.or_else(|| {
        if composed.backend == CompositorBackendKind::Gpu && !composed.gpu_readback_failed {
            if immutable_gpu_output {
                return sparkleflinger
                    .immutable_current_output_frame()
                    .ok()
                    .flatten();
            }
            return sparkleflinger
                .current_output_frame()
                .ok()
                .flatten()
                .map(ProducerFrame::GpuTexture);
        }

        None
    });
    #[cfg(not(feature = "wgpu"))]
    let _ = sparkleflinger;

    frame
}

// The final wildcard also covers the GPU frame variants when those features
// are enabled, so it must stay a wildcard in every feature combination.
#[cfg_attr(
    not(any(feature = "wgpu", feature = "servo-gpu-import")),
    allow(clippy::match_wildcard_for_single_variants)
)]
pub(super) fn surface_backed_frame(
    surface_pool: &mut RenderSurfacePool,
    frame: ProducerFrame,
    full_frame_copy: &mut FullFrameCopyMetrics,
) -> anyhow::Result<Option<ProducerFrame>> {
    match frame {
        ProducerFrame::Canvas(canvas) => {
            let Some(mut lease) = surface_pool.try_dequeue()? else {
                return Ok(None);
            };
            *lease.canvas_mut() = canvas;
            Ok(Some(ProducerFrame::Surface(lease.submit(0, 0))))
        }
        ProducerFrame::Surface(surface) if surface.generation() == 0 => {
            let Some(mut lease) = surface_pool.try_dequeue()? else {
                return Ok(None);
            };
            if let Err(error) = lease.canvas_mut().try_copy_from_published_surface(&surface) {
                lease.release();
                return Err(error.into());
            }
            full_frame_copy.record(
                usize_to_u32(surface.rgba_bytes().len()),
                "generation_zero_surface_pool_materialization",
            );
            Ok(Some(ProducerFrame::Surface(
                lease.submit(surface.frame_number(), surface.timestamp_ms()),
            )))
        }
        frame => Ok(Some(frame)),
    }
}

pub(super) fn copy_producer_frame_to_canvas(
    frame: ProducerFrame,
    target: &mut Canvas,
    full_frame_copy: &mut FullFrameCopyMetrics,
) -> Result<bool, SurfaceResourceError> {
    match frame {
        ProducerFrame::Canvas(canvas) => {
            *target = canvas;
            Ok(true)
        }
        ProducerFrame::Surface(surface) => {
            target.try_copy_from_published_surface(&surface)?;
            full_frame_copy.record(
                usize_to_u32(surface.rgba_bytes().len()),
                "surface_to_group_canvas_materialization",
            );
            Ok(true)
        }
        ProducerFrame::ScreenPublication(publication) => {
            let frame = ProducerFrame::ScreenPublication(publication);
            let byte_count = usize_to_u32(frame.width() as usize * frame.height() as usize * 4);
            let Some((canvas, _)) = frame.into_cpu_render_frame() else {
                return Ok(false);
            };
            *target = canvas;
            full_frame_copy.record(byte_count, "screen_publication_to_group_canvas");
            Ok(true)
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(frame) => {
            let frame = ProducerFrame::Gpu(frame);
            frame.record_cpu_materialization_blocked();
            Ok(false)
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(frame) => {
            let frame = ProducerFrame::GpuTexture(frame);
            frame.record_cpu_materialization_blocked();
            Ok(false)
        }
    }
}

pub(super) fn producer_frame_is_gpu(frame: &ProducerFrame) -> bool {
    match frame {
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => true,
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => true,
        ProducerFrame::Canvas(_)
        | ProducerFrame::Surface(_)
        | ProducerFrame::ScreenPublication(_) => false,
    }
}
