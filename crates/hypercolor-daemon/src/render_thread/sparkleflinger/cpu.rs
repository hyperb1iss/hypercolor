use hypercolor_core::blend_math::{
    RgbaBlendMode, blend_opaque_normal_rgba_pixels_in_place, blend_rgba_pixels_in_place,
};
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
    publish_composed_frame, scaled_preview_surface_from_rgba,
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
        dead_code,
        reason = "direct CPU compose is used by GPU comparison tests"
    )]
    pub(super) fn compose(
        &mut self,
        plan: CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        let mut preview_surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(1, 1), 2);
        let mut composition_surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(1, 1), 2);
        self.compose_with_surface_pools(
            plan,
            requires_cpu_sampling_canvas,
            preview_surface_request,
            &mut preview_surface_pool,
            &mut composition_surface_pool,
        )
    }

    #[allow(
        clippy::unused_self,
        reason = "the CPU compositor keeps an instance method to match the GPU flinger API"
    )]
    pub(super) fn compose_with_surface_pools(
        &mut self,
        plan: CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
        preview_surface_pool: &mut RenderSurfacePool,
        composition_surface_pool: &mut RenderSurfacePool,
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
                    preview_surface_pool,
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
                preview_surface_pool,
            );
        }

        let (sampling_canvas, sampling_surface) = if can_reuse_first_replace_canvas(&layers) {
            let sampling_canvas = compose_layers_into_owned_canvas(width, height, layers);
            (sampling_canvas, None)
        } else {
            let sampling_surface =
                compose_layers_into_surface(width, height, layers, composition_surface_pool);
            (
                Canvas::from_published_surface(&sampling_surface),
                Some(sampling_surface),
            )
        };
        let preview_surface = preview_surface_request.and_then(|request| {
            let (rgba, width, height) = sampling_surface.as_ref().map_or_else(
                || {
                    (
                        sampling_canvas.as_rgba_bytes(),
                        sampling_canvas.width(),
                        sampling_canvas.height(),
                    )
                },
                |surface| (surface.rgba_bytes(), surface.width(), surface.height()),
            );
            scaled_preview_surface_from_rgba(rgba, width, height, request, preview_surface_pool)
        });

        let mut composed = publish_composed_frame(
            (sampling_canvas, sampling_surface),
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
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
    }
}

fn cached_surface_frame(
    surface: PublishedSurface,
    requires_cpu_sampling_canvas: bool,
    preview_surface_request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
    preview_surface_pool: &mut RenderSurfacePool,
) -> ComposedFrameSet {
    let requires_published_surface = preview_surface_request
        .is_some_and(|request| request.width == width && request.height == height);
    let preview_surface = preview_surface_request.and_then(|request| {
        scaled_preview_surface_from_rgba(
            surface.rgba_bytes(),
            width,
            height,
            request,
            preview_surface_pool,
        )
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

fn can_reuse_first_replace_canvas(layers: &[CompositionLayer]) -> bool {
    layers.first().is_some_and(|layer| {
        layer.is_bypass_candidate()
            && matches!(&layer.frame, ProducerFrame::Canvas(canvas) if !canvas.is_shared())
    })
}

fn compose_layers_into_owned_canvas(
    width: u32,
    height: u32,
    layers: Vec<CompositionLayer>,
) -> Canvas {
    let mut layers = layers.into_iter();
    let (mut canvas, mut opaque) = if let Some(first_layer) = layers.next() {
        take_base_canvas(first_layer, width, height)
    } else {
        (Canvas::new(width, height), true)
    };
    for layer in layers {
        opaque = compose_layer(&mut canvas, opaque, layer);
    }
    canvas
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

fn compose_layers_into_surface(
    width: u32,
    height: u32,
    layers: Vec<CompositionLayer>,
    composition_surface_pool: &mut RenderSurfacePool,
) -> PublishedSurface {
    let descriptor = SurfaceDescriptor::rgba8888(width, height);
    if composition_surface_pool.descriptor() != descriptor {
        *composition_surface_pool = RenderSurfacePool::with_slot_count(descriptor, 2);
    }

    let Some(mut lease) = composition_surface_pool.dequeue() else {
        let mut canvas = Canvas::new(width, height);
        compose_layers_into_canvas(&mut canvas, layers);
        return PublishedSurface::from_owned_canvas(canvas, 0, 0);
    };

    compose_layers_into_canvas(lease.canvas_mut(), layers);
    lease.submit(0, 0)
}

fn compose_layers_into_canvas(target: &mut Canvas, layers: Vec<CompositionLayer>) {
    target.clear();
    let mut opaque = true;
    for layer in layers {
        opaque = compose_layer(target, opaque, layer);
    }
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
        CompositionMode::Replace | CompositionMode::Alpha => RgbaBlendMode::Normal,
        CompositionMode::Add => RgbaBlendMode::Add,
        CompositionMode::Screen => RgbaBlendMode::Screen,
    };
    let result_opaque = target_opaque && layer.opaque_hint;
    if blend_mode == RgbaBlendMode::Normal && result_opaque {
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
