use hypercolor_core::types::canvas::{Canvas, PublishedSurface};

use super::super::ComposedFrameSet;
use crate::performance::CompositorBackendKind;

pub(super) fn gpu_composed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

pub(super) fn gpu_composed_with_preview_surface(
    preview_surface: PublishedSurface,
) -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(preview_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

pub(super) fn gpu_bypassed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

pub(super) fn gpu_composed_from_surface(
    sampling_surface: PublishedSurface,
    requires_cpu_sampling_canvas: bool,
) -> ComposedFrameSet {
    if requires_cpu_sampling_canvas {
        let sampling_canvas = Canvas::from_published_surface(&sampling_surface);
        return ComposedFrameSet {
            sampling_canvas: Some(sampling_canvas),
            sampling_surface: Some(sampling_surface),
            preview_surface: None,
            bypassed: false,
            backend: CompositorBackendKind::Gpu,
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
        };
    }

    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(sampling_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

pub(super) fn gpu_bypassed_surface_frame(
    surface: &PublishedSurface,
    requires_cpu_sampling_canvas: bool,
    requires_preview_surface: bool,
) -> ComposedFrameSet {
    let preview_surface =
        (!requires_cpu_sampling_canvas && requires_preview_surface).then(|| surface.clone());
    let (sampling_canvas, sampling_surface) = if requires_cpu_sampling_canvas {
        (
            Some(Canvas::from_published_surface(surface)),
            Some(surface.clone()),
        )
    } else {
        (None, None)
    };
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

pub(super) fn gpu_bypassed_canvas_frame(
    canvas: &Canvas,
    requires_cpu_sampling_canvas: bool,
    requires_preview_surface: bool,
) -> ComposedFrameSet {
    let published_surface = (requires_cpu_sampling_canvas || requires_preview_surface)
        .then(|| PublishedSurface::from_owned_canvas(canvas.clone(), 0, 0));
    let preview_surface = (!requires_cpu_sampling_canvas && requires_preview_surface).then(|| {
        published_surface
            .as_ref()
            .expect("preview bypass should allocate a published surface")
            .clone()
    });
    let (sampling_canvas, sampling_surface) = if requires_cpu_sampling_canvas {
        let sampling_surface =
            published_surface.expect("CPU sampling bypass should allocate a published surface");
        (
            Some(Canvas::from_published_surface(&sampling_surface)),
            Some(sampling_surface),
        )
    } else {
        (None, None)
    };
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}
