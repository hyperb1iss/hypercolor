use hypercolor_core::blend_math::{
    blend_opaque_normal_rgba_pixels_in_place, blend_rgba_pixels_in_place,
};
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::overlay::OverlayBlendMode;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
    publish_composed_frame,
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
    pub(super) fn compose(
        &mut self,
        plan: CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        let CompositionPlan {
            width,
            height,
            mut layers,
            cpu_replay_cacheable,
        } = plan;
        let requires_full_size_preview =
            preview_request_matches_plan(preview_surface_request, width, height);
        let requires_published_surface =
            preview_surface_request.is_some() && requires_full_size_preview;

        if layers.len() == 1
            && let Some(layer) = layers.pop()
            && layer.is_bypass_candidate()
        {
            let (canvas, surface) = layer.frame.into_render_frame();
            let preview_surface = preview_surface_request.and_then(|request| {
                scaled_preview_surface_from_rgba(
                    canvas.as_rgba_bytes(),
                    canvas.width(),
                    canvas.height(),
                    request,
                )
            });
            let mut composed = publish_composed_frame(
                (canvas, surface),
                true,
                requires_cpu_sampling_canvas,
                requires_published_surface,
            );
            if !requires_full_size_preview {
                composed.preview_surface = preview_surface;
            }
            return composed;
        }

        let cached_key = cpu_replay_cacheable
            .then(|| cached_composition_key(width, height, &layers))
            .flatten();
        if let Some(cached_surface) = cached_key.as_ref().and_then(|key| {
            self.cached_composition
                .as_ref()
                .filter(|cached| cached.key == *key)
                .map(|cached| cached.surface.clone())
        }) {
            return cached_surface_frame(
                cached_surface,
                requires_cpu_sampling_canvas,
                preview_surface_request,
                width,
                height,
            );
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
        let preview_surface = preview_surface_request.and_then(|request| {
            scaled_preview_surface_from_rgba(
                sampling_canvas.as_rgba_bytes(),
                sampling_canvas.width(),
                sampling_canvas.height(),
                request,
            )
        });

        let mut composed = publish_composed_frame(
            (sampling_canvas, None),
            false,
            requires_cpu_sampling_canvas,
            requires_published_surface || cached_key.is_some(),
        );
        if !requires_full_size_preview {
            composed.preview_surface = preview_surface;
        }
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

fn cached_surface_frame(
    surface: PublishedSurface,
    requires_cpu_sampling_canvas: bool,
    preview_surface_request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
) -> ComposedFrameSet {
    let requires_published_surface = preview_surface_request
        .is_some_and(|request| request.width == width && request.height == height);
    let preview_surface = preview_surface_request.and_then(|request| {
        scaled_preview_surface_from_rgba(surface.rgba_bytes(), width, height, request)
    });
    let mut composed = publish_composed_frame(
        (Canvas::from_published_surface(&surface), Some(surface)),
        false,
        requires_cpu_sampling_canvas,
        requires_published_surface,
    );
    if !requires_published_surface {
        composed.preview_surface = preview_surface;
    }
    composed
}

fn preview_request_matches_plan(
    request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
) -> bool {
    request.is_some_and(|request| request.width == width && request.height == height)
}

fn scaled_preview_surface_from_rgba(
    rgba: &[u8],
    source_width: u32,
    source_height: u32,
    request: PreviewSurfaceRequest,
) -> Option<PublishedSurface> {
    if request.width == 0
        || request.height == 0
        || request.width == source_width && request.height == source_height
    {
        return None;
    }
    let mut preview = Canvas::new(request.width, request.height);
    let preview_bytes = preview.as_rgba_bytes_mut();
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
    Some(PublishedSurface::from_owned_canvas(preview, 0, 0))
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
