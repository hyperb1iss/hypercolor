use hypercolor_core::blend_math::{
    blend_opaque_normal_rgba_pixels_in_place, blend_rgba_pixels_in_place,
};
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::overlay::OverlayBlendMode;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, publish_composed_frame,
};
use crate::render_thread::producer_queue::ProducerFrame;

#[derive(Debug, Default)]
pub(super) struct CpuSparkleFlinger {
    cached_composition: Option<CachedCpuComposition>,
}

#[derive(Debug, Clone)]
struct CachedCpuComposition {
    key: CachedCpuCompositionKey,
    surface: PublishedSurface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedCpuCompositionKey {
    width: u32,
    height: u32,
    layers: Vec<CachedCpuCompositionLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedCpuCompositionLayer {
    storage: PublishedSurfaceStorageIdentity,
    mode: CompositionMode,
    opacity_bits: u32,
    opaque_hint: bool,
}

impl CpuSparkleFlinger {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[allow(
        clippy::unused_self,
        reason = "the CPU compositor keeps an instance method to match the GPU flinger API"
    )]
    pub(super) fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        let CompositionPlan {
            width,
            height,
            mut layers,
        } = plan;

        if layers.len() == 1
            && let Some(layer) = layers.pop()
            && layer.is_bypass_candidate()
        {
            return publish_composed_frame(layer.frame.into_render_frame(), true);
        }

        let cached_key = cached_composition_key(width, height, &layers);
        if let Some(cached_surface) = cached_key.as_ref().and_then(|key| {
            self.cached_composition
                .as_ref()
                .filter(|cached| cached.key == *key)
                .map(|cached| cached.surface.clone())
        }) {
            return cached_surface_frame(cached_surface);
        }

        let mut layers = layers.into_iter();
        let (mut sampling_canvas, mut sampling_canvas_opaque) =
            if let Some(first_layer) = layers.next() {
                take_base_canvas(first_layer, width, height)
            } else {
                (Canvas::new(width, height), true)
            };
        for layer in layers {
            sampling_canvas_opaque =
                compose_layer(&mut sampling_canvas, sampling_canvas_opaque, layer);
        }

        let composed = publish_composed_frame((sampling_canvas, None), false);
        if let Some(key) = cached_key
            && let Some(surface) = composed.sampling_surface.clone()
        {
            self.cached_composition = Some(CachedCpuComposition { key, surface });
        }
        composed
    }
}

fn cached_composition_key(
    width: u32,
    height: u32,
    layers: &[CompositionLayer],
) -> Option<CachedCpuCompositionKey> {
    let mut cached_layers = Vec::with_capacity(layers.len());
    for layer in layers {
        cached_layers.push(CachedCpuCompositionLayer {
            storage: cached_layer_storage(&layer.frame)?,
            mode: layer.mode,
            opacity_bits: layer.opacity.to_bits(),
            opaque_hint: layer.opaque_hint,
        });
    }

    Some(CachedCpuCompositionKey {
        width,
        height,
        layers: cached_layers,
    })
}

fn cached_layer_storage(frame: &ProducerFrame) -> Option<PublishedSurfaceStorageIdentity> {
    match frame {
        ProducerFrame::Surface(surface) => Some(surface.storage_identity()),
        ProducerFrame::Canvas(canvas) if canvas.is_shared() => Some(canvas.storage_identity()),
        ProducerFrame::Canvas(_) => None,
    }
}

fn cached_surface_frame(surface: PublishedSurface) -> ComposedFrameSet {
    publish_composed_frame((Canvas::from_published_surface(&surface), Some(surface)), false)
}

fn take_base_canvas(layer: CompositionLayer, width: u32, height: u32) -> (Canvas, bool) {
    if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        let (canvas, _) = layer.frame.into_render_frame();
        return (canvas, layer.opaque_hint);
    }

    let mut canvas = Canvas::new(width, height);
    let opaque = compose_layer(&mut canvas, true, layer);
    (canvas, opaque)
}

fn compose_layer(target: &mut Canvas, target_opaque: bool, layer: CompositionLayer) -> bool {
    let (source_canvas, _) = layer.frame.into_render_frame();
    if target.width() != source_canvas.width() || target.height() != source_canvas.height() {
        *target = Canvas::new(source_canvas.width(), source_canvas.height());
    }

    let opacity = layer.opacity.clamp(0.0, 1.0);
    if layer.mode == CompositionMode::Replace && opacity >= 1.0 {
        target
            .as_rgba_bytes_mut()
            .copy_from_slice(source_canvas.as_rgba_bytes());
        return layer.opaque_hint;
    }

    if opacity <= 0.0 {
        return target_opaque;
    }

    let blend_mode = match layer.mode {
        CompositionMode::Replace | CompositionMode::Alpha => OverlayBlendMode::Normal,
        CompositionMode::Add => OverlayBlendMode::Add,
        CompositionMode::Screen => OverlayBlendMode::Screen,
    };
    let result_opaque = target_opaque && layer.opaque_hint;
    if blend_mode == OverlayBlendMode::Normal && result_opaque {
        blend_opaque_normal_rgba_pixels_in_place(
            target.as_rgba_bytes_mut(),
            source_canvas.as_rgba_bytes(),
            opacity,
        );
        return true;
    }
    blend_rgba_pixels_in_place(
        target.as_rgba_bytes_mut(),
        source_canvas.as_rgba_bytes(),
        blend_mode,
        opacity,
    );
    result_opaque
}
