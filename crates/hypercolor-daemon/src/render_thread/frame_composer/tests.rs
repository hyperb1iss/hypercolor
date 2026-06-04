use super::{
    PreviewSurfaceDemandLane, PreviewSurfaceRequest, PreviewSurfaceRequestContext,
    effective_render_group_layer_count, preview_surface_request,
    producer_frame_requires_composition_for_preview, render_group_requires_full_composition,
    requires_cpu_sampling_canvas, requires_published_surface,
};
use std::sync::Arc;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

use crate::preview_runtime::PreviewDemandSummary;
use crate::render_thread::frame_sampling::LedSamplingStrategy;
use crate::render_thread::producer_queue::ProducerFrame;
use crate::render_thread::sparkleflinger::SparkleFlinger;
use hypercolor_types::config::RenderAccelerationMode;

#[test]
fn render_group_layer_count_adds_transition_base_once() {
    assert_eq!(effective_render_group_layer_count(1, 4), 4);
    assert_eq!(effective_render_group_layer_count(2, 4), 5);
}

#[test]
fn cpu_sampling_canvas_only_depends_on_preview_receivers_and_gpu_sampling() {
    assert!(!requires_cpu_sampling_canvas(true));
    assert!(requires_cpu_sampling_canvas(false));
}

#[test]
fn composer_requires_cpu_sampling_canvas_for_gaussian_gpu_sampling_plan() {
    let Ok(sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
        return;
    };
    let spatial_engine = SpatialEngine::new(SpatialLayout {
        id: "layout".into(),
        name: "Layout".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![Output {
            id: "strip".into(),
            name: "Strip".into(),
            device_id: "device".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 4,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::GaussianArea {
                sigma: 1.0,
                radius: 2,
            }),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    });

    assert!(requires_cpu_sampling_canvas(
        sparkleflinger.can_sample_zone_plan(spatial_engine.sampling_plan().as_ref())
    ));
}

#[test]
fn render_group_full_composition_is_required_when_sparkleflinger_owns_led_sampling() {
    let strategy = LedSamplingStrategy::SparkleFlinger(SpatialEngine::new(SpatialLayout {
        id: "layout".into(),
        name: "Layout".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }));
    assert!(render_group_requires_full_composition(false, &strategy));
}

#[test]
fn render_group_presampled_leds_can_bypass_full_composition_without_transition() {
    let strategy = LedSamplingStrategy::PreSampled(Arc::new(SpatialLayout {
        id: "layout".into(),
        name: "Layout".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }));
    assert!(!render_group_requires_full_composition(false, &strategy));
    assert!(render_group_requires_full_composition(true, &strategy));
}

#[test]
fn cpu_producer_preview_does_not_force_full_composition() {
    let canvas = Canvas::new(4, 4);
    let surface = PublishedSurface::from_owned_canvas(Canvas::new(4, 4), 1, 16);

    assert!(!producer_frame_requires_composition_for_preview(
        &ProducerFrame::Canvas(canvas),
        true,
    ));
    assert!(!producer_frame_requires_composition_for_preview(
        &ProducerFrame::Surface(surface),
        true,
    ));
}

#[test]
fn published_surface_depends_on_preview_and_screen_passthrough_receivers() {
    assert!(!requires_published_surface(false, false, false, false, 0));
    assert!(requires_published_surface(true, false, true, false, 0));
    assert!(requires_published_surface(false, true, false, true, 0));
    assert!(!requires_published_surface(false, true, true, true, 0));
}

#[test]
fn published_surface_depends_on_scene_canvas_receivers() {
    assert!(requires_published_surface(false, false, false, false, 1));
}

fn demand_lane(
    receivers: usize,
    tracked_receivers: usize,
    demand: PreviewDemandSummary,
) -> PreviewSurfaceDemandLane {
    PreviewSurfaceDemandLane {
        receivers,
        tracked_receivers,
        demand,
    }
}

#[test]
fn preview_surface_request_uses_scaled_tracked_demand() {
    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 1280,
            canvas_height: 720,
            publish_canvas_preview: true,
            effect_running: true,
            canvas: demand_lane(
                1,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 640,
                    max_height: 360,
                    ..PreviewDemandSummary::default()
                },
            ),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 640,
            height: 360,
        })
    );
}

#[test]
fn preview_surface_request_handles_zero_canvas_dimensions_without_panicking() {
    // A tracked demand with non-zero dimensions would otherwise reach
    // `max_width.clamp(1, canvas_width)`, which panics when the canvas
    // dimension is 0 because `clamp` requires `min <= max`.
    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 0,
            canvas_height: 480,
            publish_canvas_preview: true,
            effect_running: true,
            canvas: demand_lane(
                1,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 320,
                    max_height: 240,
                    ..PreviewDemandSummary::default()
                },
            ),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 0,
            height: 480,
        })
    );

    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 640,
            canvas_height: 0,
            publish_canvas_preview: true,
            effect_running: true,
            canvas: demand_lane(
                1,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 320,
                    max_height: 240,
                    ..PreviewDemandSummary::default()
                },
            ),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 640,
            height: 0,
        })
    );
}

#[test]
fn preview_surface_request_falls_back_to_full_size_for_untracked_receivers() {
    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 1280,
            canvas_height: 720,
            publish_canvas_preview: true,
            effect_running: true,
            canvas: demand_lane(
                2,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 640,
                    max_height: 360,
                    ..PreviewDemandSummary::default()
                },
            ),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 1280,
            height: 720,
        })
    );
}

#[test]
fn preview_surface_request_uses_scaled_tracked_scene_canvas_demand() {
    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 1280,
            canvas_height: 720,
            effect_running: true,
            scene_canvas: demand_lane(
                1,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 320,
                    max_height: 180,
                    ..PreviewDemandSummary::default()
                },
            ),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 320,
            height: 180,
        })
    );
}

#[test]
fn preview_surface_request_uses_full_resolution_for_authoritative_global_lane() {
    assert_eq!(
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: 1280,
            canvas_height: 720,
            effect_running: true,
            scene_canvas: demand_lane(1, 0, PreviewDemandSummary::default()),
            ..PreviewSurfaceRequestContext::default()
        }),
        Some(PreviewSurfaceRequest {
            width: 1280,
            height: 720,
        })
    );
}
