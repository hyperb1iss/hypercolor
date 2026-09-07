use super::*;

#[cfg(feature = "wgpu")]
use crate::render_thread::producer_queue::GpuTextureFrameOrigin;
use crate::render_thread::sparkleflinger::CompositionLayer;
#[cfg(feature = "wgpu")]
use hypercolor_core::input::screen::consumer::{CaptureEpoch, CaptureSourceId, PixelExtent};
#[cfg(feature = "wgpu")]
use hypercolor_core::input::screen::implementer::{
    CaptureColorimetry, CaptureGeometry, CapturePixelFormat, CaptureRotation, CpuReductionExecutor,
    PhysicalOrigin, ScreenPublicationHealth, ScreenPublicationMetadata, SourceScale,
};
#[cfg(feature = "wgpu")]
use hypercolor_core::input::screen::planner::{
    RegisteredScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenCaptureBackend, ScreenExactResource, ScreenExactResourceLedger, ScreenExtentRequest,
    ScreenInputGraphGeneration, ScreenPayloadKind, ScreenPlanBuilder,
    ScreenPublicationExecutorRequest, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy,
};
#[cfg(feature = "wgpu")]
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
#[cfg(feature = "wgpu")]
use std::time::{Duration, Instant};

#[test]
fn single_zone_preview_publishes_surface_frame() {
    let mut runtime = cpu_backed_runtime(4, 4);
    let zone = sample_zone(4, 4);
    let mut source = Canvas::new(4, 4);
    source.fill(Rgba::new(12, 34, 56, 255));
    runtime.target_canvases.insert(zone.id, source);

    let preview = runtime.compose_preview_grid_for_test(&[zone]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("single-zone preview should publish a pooled surface");
    };

    assert_eq!(surface.width(), 4);
    assert_eq!(surface.height(), 4);
    assert_eq!(surface.get_pixel(0, 0), Rgba::new(12, 34, 56, 255));
    assert_eq!(surface.get_pixel(3, 3), Rgba::new(12, 34, 56, 255));
}

#[test]
fn single_zone_preview_scales_zone_canvas_to_preview_extent() {
    let mut runtime = cpu_backed_runtime(4, 4);
    let zone = sample_zone(2, 2);
    let mut source = Canvas::new(2, 2);
    source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
    runtime.target_canvases.insert(zone.id, source);

    let preview = runtime.compose_preview_grid_for_test(&[zone]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("scaled single-zone preview should publish a pooled surface");
    };

    let top_left = surface.get_pixel(0, 0);
    let top_right = surface.get_pixel(3, 0);
    let bottom_left = surface.get_pixel(0, 3);
    let bottom_right = surface.get_pixel(3, 3);

    assert_eq!(surface.width(), 4);
    assert_eq!(surface.height(), 4);
    assert!(top_left.r > top_left.g && top_left.r > top_left.b);
    assert!(top_right.g > top_right.r && top_right.g > top_right.b);
    assert!(bottom_left.b > bottom_left.r && bottom_left.b > bottom_left.g);
    assert!(bottom_right.r > 180 && bottom_right.g > 180 && bottom_right.b < 120);
}

#[test]
fn compose_preview_ignores_display_zones() {
    let mut runtime = cpu_backed_runtime(4, 4);
    let preview_zone = sample_zone(4, 4);
    let display_zone = sample_display_zone(4, 4);
    let mut preview_canvas = Canvas::new(4, 4);
    preview_canvas.fill(Rgba::new(255, 0, 0, 255));
    let mut display_canvas = Canvas::new(4, 4);
    display_canvas.fill(Rgba::new(0, 0, 255, 255));
    runtime
        .target_canvases
        .insert(preview_zone.id, preview_canvas);
    runtime
        .target_canvases
        .insert(display_zone.id, display_canvas);

    let preview = runtime.compose_preview_grid_for_test(&[preview_zone, display_zone]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("mixed preview should publish a pooled surface");
    };

    assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(surface.get_pixel(3, 3), Rgba::new(255, 0, 0, 255));
}

#[test]
fn authoritative_scene_canvas_clips_rotated_zone_geometry() {
    let mut runtime = cpu_backed_runtime(8, 8);
    let mut zone = sample_zone(8, 8);
    zone.layout.zones = vec![rotated_zone("zone_rotated", FRAC_PI_4, 0.5)];
    let mut source = Canvas::new(8, 8);
    source.fill(Rgba::new(255, 0, 0, 255));
    runtime.target_canvases.insert(zone.id, source);

    let scene_frame = runtime
        .compose_scene_frame(&[zone])
        .expect("scene frame should allocate");
    let ProducerFrame::Surface(surface) = scene_frame else {
        panic!("authoritative scene canvas should publish a pooled surface");
    };

    assert_eq!(
        surface.get_pixel(1, 1),
        Rgba::new(0, 0, 0, 255),
        "pixels outside the rotated zone should remain untouched"
    );
    assert_eq!(
        surface.get_pixel(3, 3),
        Rgba::new(255, 0, 0, 255),
        "pixels inside the rotated zone should sample the source canvas"
    );
}

#[test]
fn authoritative_scene_canvas_preserves_zone_overlap_order() {
    let mut runtime = cpu_backed_runtime(8, 8);
    let mut back_zone = sample_zone(8, 8);
    back_zone.layout.zones = vec![rotated_zone("zone_back", FRAC_PI_4, 0.5)];
    let mut front_zone = sample_zone(8, 8);
    front_zone.layout.zones = vec![point_zone("zone_front")];
    front_zone.layout.zones[0].size = NormalizedPosition { x: 0.25, y: 0.25 };

    let mut back_source = Canvas::new(8, 8);
    back_source.fill(Rgba::new(255, 0, 0, 255));
    let mut front_source = Canvas::new(8, 8);
    front_source.fill(Rgba::new(0, 0, 255, 255));
    runtime.target_canvases.insert(back_zone.id, back_source);
    runtime.target_canvases.insert(front_zone.id, front_source);

    let scene_frame = runtime
        .compose_scene_frame(&[back_zone, front_zone])
        .expect("scene frame should allocate");
    let ProducerFrame::Surface(surface) = scene_frame else {
        panic!("authoritative scene canvas should publish a pooled surface");
    };

    assert_eq!(
        surface.get_pixel(4, 4),
        Rgba::new(0, 0, 255, 255),
        "later zones should overwrite earlier zones in overlapping regions"
    );
    assert_eq!(
        surface.get_pixel(2, 4),
        Rgba::new(255, 0, 0, 255),
        "pixels only covered by the back zone should keep its content"
    );
}

#[test]
fn authoritative_scene_canvas_uses_zone_sampling_mode() {
    let mut runtime = cpu_backed_runtime(4, 4);
    let mut zone = sample_zone(2, 2);
    zone.layout.zones = vec![point_zone("zone_sampling")];
    zone.layout.zones[0].size = NormalizedPosition { x: 1.0, y: 1.0 };
    zone.layout.zones[0].sampling_mode = Some(SamplingMode::Nearest);
    let mut source = Canvas::new(2, 2);
    source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
    runtime.target_canvases.insert(zone.id, source);

    let scene_frame = runtime
        .compose_scene_frame(&[zone])
        .expect("scene frame should allocate");
    let ProducerFrame::Surface(surface) = scene_frame else {
        panic!("authoritative scene canvas should publish a pooled surface");
    };

    assert_eq!(surface.get_pixel(1, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(surface.get_pixel(2, 0), Rgba::new(0, 255, 0, 255));
    assert_eq!(surface.get_pixel(1, 3), Rgba::new(0, 0, 255, 255));
    assert_eq!(surface.get_pixel(2, 3), Rgba::new(255, 255, 0, 255));
}

#[test]
fn render_scene_caches_compact_projection_metadata_until_layout_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut zone = sample_zone(2, 2);
    set_effect_zone(
        &mut zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    zone.layout.zones = vec![point_zone_at("zone_cached", 0.25, 0.5)];
    let display_zone_target_fps = HashMap::new();
    let mut zone_colors = Vec::new();

    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("first render should build the projection cache");
    let cached_bounds = runtime
        .scene_projection_cache
        .get(&zone.id)
        .expect("scene zone should have a cached projection")
        .zones[0]
        .bounds;

    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        16,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("same dependency key should keep the projection cache");

    assert_eq!(
        runtime
            .scene_projection_cache
            .get(&zone.id)
            .expect("scene zone should keep a cached projection")
            .zones[0]
            .bounds,
        cached_bounds
    );

    zone.layout.zones[0].size = NormalizedPosition::new(1.0, 1.0);
    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        2,
        32,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("layout changes should rebuild the projection cache");

    assert!(matches!(
        runtime
            .scene_projection_cache
            .get(&zone.id)
            .expect("scene zone should rebuild a cached projection")
            .zones[0]
            .bounds,
        Some(ProjectionBounds {
            x0: 0,
            y0: 0,
            x1: 3,
            y1: 4,
        })
    ));
}

#[test]
fn affine_projection_work_scales_linearly_through_8k() {
    for (width, height) in [(1_920, 1_080), (3_840, 2_160), (7_680, 4_320)] {
        let mut zone = sample_zone(width, height);
        let mut output = point_zone("full_scene");
        output.size = NormalizedPosition::new(1.0, 1.0);
        output.rotation = FRAC_PI_4;
        zone.layout.zones = vec![output];

        let work = build_zone_projection(&zone, width, height)
            .expect("projection metadata should allocate")
            .raster_work();

        assert_eq!(work.affine_setups, 1);
        assert_eq!(work.rows, u64::from(height));
        assert_eq!(work.pixels, u64::from(width) * u64::from(height));
    }
}

#[test]
fn projection_metadata_is_constant_size_for_large_dimensions() {
    let mut zone = sample_zone(u32::MAX, u32::MAX);
    let mut output = point_zone("large");
    output.position = NormalizedPosition::new(0.5, 0.5);
    output.size = NormalizedPosition::new(1.0, 1.0);
    zone.layout.zones = vec![output];
    let projection = build_zone_projection(&zone, u32::MAX, u32::MAX)
        .expect("projection metadata should allocate");

    assert_eq!(projection.zones.len(), zone.layout.zones.len());
    assert!(matches!(
        projection.zones[0].bounds,
        Some(ProjectionBounds {
            x0: 0,
            y0: 0,
            x1: u32::MAX,
            y1: u32::MAX,
        })
    ));
}

#[test]
fn axis_aligned_bilinear_fast_path_matches_general_projection() {
    let mut zone = point_zone("zone_fast_bilinear");
    zone.position = NormalizedPosition::new(0.5, 0.5);
    zone.size = NormalizedPosition::new(0.75, 0.5);
    zone.scale = 1.0;
    zone.rotation = 0.0;
    zone.sampling_mode = Some(SamplingMode::Bilinear);
    let layout = SpatialLayout {
        id: "fast-path-layout".into(),
        name: "Fast Path Layout".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![zone.clone()],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    let mut source = Canvas::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            source.set_pixel(
                x,
                y,
                Rgba::new((x * 40) as u8, (y * 50) as u8, ((x + y) * 30) as u8, 255),
            );
        }
    }
    let mut fast = Canvas::new(8, 8);
    let mut general = Canvas::new(8, 8);

    blit_zone_projection(&mut fast, &source, &zone, &layout, 8, 8);
    blit_general_zone_projection(
        &mut general,
        &source,
        &zone,
        zone.sampling_mode
            .as_ref()
            .expect("sampling mode should be set"),
        EdgeBehavior::Clamp,
        0,
        0,
        8,
        8,
        8,
        8,
    );

    assert_eq!(fast.as_rgba_bytes(), general.as_rgba_bytes());
}

#[test]
fn axis_aligned_nearest_fast_path_matches_general_projection() {
    let mut zone = point_zone("zone_fast_nearest");
    zone.position = NormalizedPosition::new(0.35, 0.6);
    zone.size = NormalizedPosition::new(0.5, 0.5);
    zone.scale = 1.0;
    zone.rotation = 0.0;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    let layout = SpatialLayout {
        id: "fast-path-layout-nearest".into(),
        name: "Fast Path Layout Nearest".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![zone.clone()],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    let mut source = Canvas::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            source.set_pixel(
                x,
                y,
                Rgba::new((x * 60) as u8, (y * 70) as u8, ((x + y) * 20) as u8, 255),
            );
        }
    }
    let mut fast = Canvas::new(8, 8);
    let mut general = Canvas::new(8, 8);

    blit_zone_projection(&mut fast, &source, &zone, &layout, 8, 8);
    blit_general_zone_projection(
        &mut general,
        &source,
        &zone,
        zone.sampling_mode
            .as_ref()
            .expect("sampling mode should be set"),
        EdgeBehavior::Clamp,
        0,
        0,
        8,
        8,
        8,
        8,
    );

    assert_eq!(fast.as_rgba_bytes(), general.as_rgba_bytes());
}

#[test]
fn full_scene_identity_fast_path_matches_projected_path() {
    let mut output = point_zone("zone_full_scene_identity");
    output.position = NormalizedPosition::new(0.5, 0.5);
    output.size = NormalizedPosition::new(1.0, 1.0);
    output.scale = 1.0;
    output.rotation = 0.0;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    let zone = Zone {
        id: ZoneId::new(),
        name: "Identity".into(),
        description: None,
        layers: Vec::new(),
        layout: SpatialLayout {
            id: "full-scene-identity".into(),
            name: "Full Scene Identity".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![output.clone()],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        },
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    };
    let projection =
        build_zone_projection(&zone, 4, 4).expect("projection metadata should allocate");
    let mut source = Canvas::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            source.set_pixel(
                x,
                y,
                Rgba::new((x * 40) as u8, (y * 50) as u8, ((x + y) * 30) as u8, 255),
            );
        }
    }
    let mut fast = Canvas::new(4, 4);
    let mut general = Canvas::new(4, 4);

    assert!(copy_full_scene_identity_projection(
        &mut fast,
        &source,
        &projection
    ));
    blit_general_zone_projection(
        &mut general,
        &source,
        &output,
        output
            .sampling_mode
            .as_ref()
            .expect("sampling mode should be set"),
        EdgeBehavior::Clamp,
        0,
        0,
        4,
        4,
        4,
        4,
    );

    assert_eq!(fast.as_rgba_bytes(), general.as_rgba_bytes());
}

#[test]
fn projected_composition_layers_match_nearest_projection() {
    let mut output = point_zone("zone_projected_composition");
    output.position = NormalizedPosition::new(0.5, 0.5);
    output.size = NormalizedPosition::new(1.0, 1.0);
    output.rotation = 0.0;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut zone = sample_zone(4, 4);
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    zone.layout.default_edge_behavior = EdgeBehavior::Clamp;
    let projection =
        build_zone_projection(&zone, 4, 4).expect("projection metadata should allocate");
    let source = patterned_source_canvas(4, 4);
    let layers = projection_composition_layers_for_zone(
        &ProducerFrame::Canvas(source.clone()),
        &zone,
        &projection,
        4,
        4,
    )
    .expect("nearest clamp projection should use composition layers");
    let mut projection_cache = HashMap::new();
    projection_cache.insert(zone.id, projection);
    let mut target_canvases = HashMap::new();
    target_canvases.insert(zone.id, source.clone());
    let mut projected = Canvas::new(4, 4);
    compose_authoritative_scene_canvas(
        &mut projected,
        std::slice::from_ref(&zone),
        &target_canvases,
        4,
        4,
        &projection_cache,
    );
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(4, 4, layers).with_cpu_replay_cacheable(false),
        true,
        Some(PreviewSurfaceRequest {
            width: 4,
            height: 4,
        }),
    );
    let actual = composed
        .sampling_surface
        .map(|surface| Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height()))
        .or(composed.sampling_canvas)
        .expect("CPU composition should materialize a scene canvas");

    assert_eq!(actual.as_rgba_bytes(), projected.as_rgba_bytes());
}

#[test]
fn projected_contributors_refresh_current_cpu_replay() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::default();
    let mut zone = sample_zone(4, 4);
    make_color_fill_zone(&mut zone);
    let mut output = point_zone("cpu_replay");
    output.size = NormalizedPosition::new(1.0, 1.0);
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    let dependency_key = SceneDependencyKey::new(1, registry.generation());
    runtime
        .reconcile(
            std::slice::from_ref(&zone),
            Some(SceneId::DEFAULT),
            dependency_key,
            &registry,
            &HashMap::new(),
            None,
        )
        .expect("projection resources should reconcile");
    runtime
        .projected_scene_layers
        .try_reserve_exact(1)
        .expect("test projection scratch should allocate");
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let target_fps = HashMap::new();
    let context = RenderSceneContext {
        zones: std::slice::from_ref(&zone),
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key,
        elapsed_ms: 0,
        display_zone_target_fps: &target_fps,
        display_zone_descriptors: &HashMap::new(),
        registry: &registry,
        authoritative_spatial_engine: None,
        inputs: ZoneFrameInputs {
            delta_secs: 1.0 / 60.0,
            audio: &audio,
            interaction: &interaction,
            screen: None,
            sensors: &sensors,
            input_availability: InputSourceAvailability::default(),
            media: None,
            net: None,
            lighting: None,
        },
    };
    let mut sparkleflinger = SparkleFlinger::cpu();
    let mut output = super::super::render_pass::RenderedZonePassOutput::default();

    let projected = runtime
        .render_scene_contributor_frames(context, &mut sparkleflinger, true, &mut output)
        .expect("projected contributor should render");

    assert!(!projected.layers.is_empty());
    assert!(projected.cpu_replay_complete);
    assert_eq!(
        runtime
            .target_canvases
            .get(&zone.id)
            .expect("zone target should remain installed")
            .get_pixel(0, 0),
        Rgba::new(255, 0, 0, 255)
    );
}

#[test]
fn projected_composition_matches_rotated_scaled_translated_zone() {
    let mut output = point_zone("zone_transformed_composition");
    output.position = NormalizedPosition::new(0.35, 0.6);
    output.size = NormalizedPosition::new(0.65, 0.45);
    output.scale = 0.8;
    output.rotation = FRAC_PI_4;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut zone = sample_zone(8, 8);
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    let projection =
        build_zone_projection(&zone, 8, 8).expect("projection metadata should allocate");
    let mut source = patterned_source_canvas(8, 8);
    source.set_pixel(3, 4, Rgba::new(140, 60, 220, 0));
    let layers = projection_composition_layers_for_zone(
        &ProducerFrame::Canvas(source.clone()),
        &zone,
        &projection,
        8,
        8,
    )
    .expect("transformed nearest clamp projection should use composition layers");
    let projection_cache = HashMap::from([(zone.id, projection)]);
    let target_canvases = HashMap::from([(zone.id, source)]);
    let mut expected = Canvas::new(8, 8);
    compose_authoritative_scene_canvas(
        &mut expected,
        std::slice::from_ref(&zone),
        &target_canvases,
        8,
        8,
        &projection_cache,
    );
    let actual = compose_projection_layers_on_cpu(layers, 8, 8);

    assert_eq!(actual.as_rgba_bytes(), expected.as_rgba_bytes());
    assert_eq!(actual.get_pixel(0, 0), Rgba::BLACK);
    assert_eq!(actual.get_pixel(3, 4), Rgba::new(140, 60, 220, 255));
}

#[test]
fn projected_composition_preserves_zone_and_zone_overlap_order() {
    let mut back = sample_zone(8, 8);
    back.layout.zones = vec![rotated_zone("back_a", FRAC_PI_4, 0.7)];
    back.layout.zones.push(point_zone_at("back_b", 0.2, 0.2));
    back.layout.default_sampling_mode = SamplingMode::Nearest;
    let mut front = sample_zone(8, 8);
    front.layout.zones = vec![point_zone_at("front", 0.5, 0.5)];
    front.layout.zones[0].size = NormalizedPosition::new(0.35, 0.35);
    front.layout.default_sampling_mode = SamplingMode::Nearest;

    let back_projection =
        build_zone_projection(&back, 8, 8).expect("back projection metadata should allocate");
    let front_projection =
        build_zone_projection(&front, 8, 8).expect("front projection metadata should allocate");
    let mut back_source = Canvas::new(8, 8);
    back_source.fill(Rgba::new(255, 0, 0, 255));
    let mut front_source = Canvas::new(8, 8);
    front_source.fill(Rgba::new(0, 0, 255, 255));
    let mut layers = projection_composition_layers_for_zone(
        &ProducerFrame::Canvas(back_source.clone()),
        &back,
        &back_projection,
        8,
        8,
    )
    .expect("back projection should use composition layers");
    layers.extend(
        projection_composition_layers_for_zone(
            &ProducerFrame::Canvas(front_source.clone()),
            &front,
            &front_projection,
            8,
            8,
        )
        .expect("front projection should use composition layers"),
    );
    let projection_cache =
        HashMap::from([(back.id, back_projection), (front.id, front_projection)]);
    let target_canvases = HashMap::from([(back.id, back_source), (front.id, front_source)]);
    let zones = [back, front];
    let mut expected = Canvas::new(8, 8);
    compose_authoritative_scene_canvas(
        &mut expected,
        &zones,
        &target_canvases,
        8,
        8,
        &projection_cache,
    );

    let actual = compose_projection_layers_on_cpu(layers, 8, 8);

    assert_eq!(actual.as_rgba_bytes(), expected.as_rgba_bytes());
    assert_eq!(actual.get_pixel(4, 4), Rgba::new(0, 0, 255, 255));
    assert_eq!(actual.get_pixel(0, 7), Rgba::BLACK);
}

#[test]
fn projected_composition_rejects_bilinear_zones() {
    let mut output = point_zone("zone_bilinear_projection");
    output.sampling_mode = Some(SamplingMode::Bilinear);
    let mut zone = sample_zone(4, 4);
    zone.layout.zones = vec![output];
    let projection =
        build_zone_projection(&zone, 4, 4).expect("projection metadata should allocate");

    assert!(
        projection_composition_layers_for_zone(
            &ProducerFrame::Canvas(patterned_source_canvas(4, 4)),
            &zone,
            &projection,
            4,
            4,
        )
        .is_none()
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn gpu_projected_composition_matches_nearest_projection() {
    let Ok(mut sparkleflinger) =
        SparkleFlinger::new(hypercolor_types::config::RenderAccelerationMode::Gpu)
    else {
        return;
    };
    let mut output = point_zone("zone_gpu_projection");
    output.position = NormalizedPosition::new(0.5, 0.5);
    output.size = NormalizedPosition::new(1.0, 1.0);
    output.rotation = 0.0;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut zone = sample_zone(4, 4);
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    zone.layout.default_edge_behavior = EdgeBehavior::Clamp;
    let projection =
        build_zone_projection(&zone, 4, 4).expect("projection metadata should allocate");
    let source = patterned_source_canvas(4, 4);
    let Some(gpu_source) = sparkleflinger.upload_canvas_frame(&source) else {
        return;
    };
    let layers = projection_composition_layers_for_zone(
        &ProducerFrame::GpuTexture(gpu_source),
        &zone,
        &projection,
        4,
        4,
    )
    .expect("nearest clamp projection should use composition layers");
    let mut projection_cache = HashMap::new();
    projection_cache.insert(zone.id, projection);
    let mut target_canvases = HashMap::new();
    target_canvases.insert(zone.id, source);
    let mut projected = Canvas::new(4, 4);
    compose_authoritative_scene_canvas(
        &mut projected,
        std::slice::from_ref(&zone),
        &target_canvases,
        4,
        4,
        &projection_cache,
    );
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(4, 4, layers).with_cpu_replay_cacheable(false),
        false,
        None,
    );
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());

    let mut sample_zone = point_zone("projected_pixels");
    sample_zone.size = NormalizedPosition::new(1.0, 1.0);
    sample_zone.topology = LedTopology::Matrix {
        width: 4,
        height: 4,
        serpentine: false,
        start_corner: Corner::TopLeft,
    };
    sample_zone.sampling_mode = Some(SamplingMode::Nearest);
    sample_zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let sampling_engine = SpatialEngine::new(SpatialLayout {
        id: "projected-pixel-sampling".into(),
        name: "Projected Pixel Sampling".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![sample_zone],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    });
    let expected = sampling_engine.sample(&projected);
    let mut actual = Vec::new();
    assert!(
        sparkleflinger
            .sample_zone_plan_into(sampling_engine.sampling_plan().as_ref(), &mut actual)
            .expect("GPU zone sampling should sample the projected canvas")
    );

    assert_eq!(actual, expected);
}

#[cfg(feature = "wgpu")]
#[test]
fn gpu_projected_scene_frame_stays_gpu_resident() {
    let Ok(mut sparkleflinger) =
        SparkleFlinger::new(hypercolor_types::config::RenderAccelerationMode::Gpu)
    else {
        return;
    };
    let source = patterned_source_canvas(8, 8);
    let Some(gpu_source) = sparkleflinger.upload_canvas_frame(&source) else {
        return;
    };
    let mut output = point_zone("gpu_transformed_projection");
    output.position = NormalizedPosition::new(0.35, 0.6);
    output.size = NormalizedPosition::new(0.65, 0.45);
    output.scale = 0.8;
    output.rotation = FRAC_PI_4;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut zone = sample_zone(8, 8);
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    admit_projected_scene_resources(&mut sparkleflinger, std::slice::from_ref(&zone), 8, 8);
    let projection =
        build_zone_projection(&zone, 8, 8).expect("projection metadata should allocate");
    let layers = projection_composition_layers_for_zone(
        &ProducerFrame::GpuTexture(gpu_source),
        &zone,
        &projection,
        8,
        8,
    )
    .expect("transformed nearest clamp projection should use composition layers");
    let mut runtime = ZoneRuntime::new(8, 8);
    let frame = runtime
        .compose_projected_scene_frame(layers, &mut sparkleflinger)
        .expect("GPU projection should export the current output frame");

    assert!(matches!(frame, ProducerFrame::GpuTexture(_)));
}

#[cfg(feature = "wgpu")]
#[test]
fn two_gpu_resident_zones_produce_stable_projected_scene_frame() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let zones = gpu_projection_zones();
    let dependency_key = SceneDependencyKey::new(1, registry.generation());
    let mut runtime = ZoneRuntime::new(4, 4);
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            dependency_key,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("GPU zone projection resources should reconcile");
    let allocations_after_admission = sparkleflinger
        .snapshot_texture_allocation_count_for_test()
        .expect("required GPU compositor should expose snapshot allocations");
    let compositor_allocations_after_admission = sparkleflinger
        .compositor_surface_allocation_count_for_test()
        .expect("required GPU compositor should expose surface allocations");
    assert_eq!(runtime.scene_cpu_backing_bytes(), 0);
    let admitted_layer_capacity = runtime.projected_scene_layer_capacity();
    assert_eq!(admitted_layer_capacity, 3);
    let layer_allocations_after_admission = runtime.projected_scene_layer_allocation_count();
    assert_eq!(layer_allocations_after_admission, 1);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let target_fps = HashMap::new();
    let context = RenderSceneContext {
        zones: &zones,
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key,
        elapsed_ms: 0,
        display_zone_target_fps: &target_fps,
        display_zone_descriptors: &HashMap::new(),
        registry: &registry,
        authoritative_spatial_engine: None,
        inputs: ZoneFrameInputs {
            delta_secs: 1.0 / 60.0,
            audio: &audio,
            interaction: &interaction,
            screen: None,
            sensors: &sensors,
            input_availability: InputSourceAvailability::default(),
            media: None,
            net: None,
            lighting: None,
        },
    };
    let mut output = super::super::render_pass::RenderedZonePassOutput::default();

    let projected = runtime
        .render_scene_contributor_frames(context, &mut sparkleflinger, true, &mut output)
        .expect("GPU-resident zones should render for projection");
    let identities = projected
        .layers
        .iter()
        .skip(1)
        .map(|layer| {
            layer
                .gpu_frame_identity_for_test()
                .expect("each projected zone should remain GPU-resident")
        })
        .collect::<Vec<_>>();

    assert_eq!(identities.len(), 2);
    assert!(
        identities
            .iter()
            .all(|identity| identity.2 == GpuTextureFrameOrigin::ProjectionSnapshot)
    );
    assert_ne!(identities[0].0, identities[1].0);
    assert!(!projected.cpu_replay_complete);
    let static_surfaces_before_scene_projection = runtime.static_layer_surface_cache.entry_count();
    let scene_frame = runtime
        .compose_projected_scene_frame(projected.layers, &mut sparkleflinger)
        .expect("two stable GPU zone frames should produce a projected scene");
    assert_eq!(
        runtime.static_layer_surface_cache.entry_count(),
        static_surfaces_before_scene_projection,
        "GPU scene projection must not allocate a scene-sized CPU base surface"
    );
    assert!(
        !runtime
            .static_layer_surface_cache
            .contains(4, 4, Rgba::BLACK)
    );
    let ProducerFrame::GpuTexture(scene_gpu_frame) = &scene_frame else {
        panic!("projected scene should stay GPU-resident")
    };
    assert_eq!(
        scene_gpu_frame.origin,
        GpuTextureFrameOrigin::ImmutableSnapshot
    );
    let first_pixels = sample_full_gpu_canvas(&mut sparkleflinger, 4, 4);
    assert_eq!(first_pixels[0], [255, 0, 0]);
    assert_eq!(first_pixels[5], [0, 0, 255]);
    assert_eq!(first_pixels[10], [0, 0, 255]);
    assert_eq!(first_pixels[15], [255, 0, 0]);

    let mut second_output = super::super::render_pass::RenderedZonePassOutput::default();
    let second = runtime
        .render_scene_contributor_frames(context, &mut sparkleflinger, true, &mut second_output)
        .expect("projected zone snapshots should remain reusable");
    let second_identities = second
        .layers
        .iter()
        .skip(1)
        .map(|layer| {
            layer
                .gpu_frame_identity_for_test()
                .expect("reused projected zones should remain GPU-resident")
        })
        .collect::<Vec<_>>();

    assert_eq!(second_identities.len(), identities.len());
    for (first, second) in identities.iter().zip(&second_identities) {
        assert_eq!(first.0, second.0);
        assert!(second.1 > first.1);
    }
    let second_scene = runtime
        .compose_projected_scene_frame(second.layers, &mut sparkleflinger)
        .expect("second projected scene should use the second leased generation");
    assert!(matches!(second_scene, ProducerFrame::GpuTexture(_)));
    assert_eq!(
        sample_full_gpu_canvas(&mut sparkleflinger, 4, 4),
        first_pixels
    );
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(allocations_after_admission),
        "steady-state projection must not allocate GPU textures"
    );
    assert_eq!(
        sparkleflinger.compositor_surface_allocation_count_for_test(),
        Some(compositor_allocations_after_admission),
        "steady-state projection must reuse admitted compositor descriptors"
    );
    assert_eq!(
        runtime.projected_scene_layer_capacity(),
        admitted_layer_capacity,
        "steady-state projection must return its admitted layer scratch"
    );
    assert_eq!(runtime.scene_cpu_backing_bytes(), 0);
    assert_eq!(
        runtime.projected_scene_layer_allocation_count(),
        layer_allocations_after_admission,
        "steady projection must not allocate layer-vector backing"
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn six_projected_sources_use_only_admitted_bind_groups_after_warmup() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let zones = gpu_projection_zone_set(6);
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();

    let first = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("six-source GPU scene should render from admitted resources");
    assert!(matches!(first.scene_frame, ProducerFrame::GpuTexture(_)));

    let admitted_creations = sparkleflinger
        .projected_bind_group_creation_count_for_test()
        .expect("required GPU compositor should expose projected bind creation");
    assert_eq!(admitted_creations, zones.len() * 2);
    assert_eq!(
        sparkleflinger.projected_bind_group_entry_count_for_test(),
        Some(zones.len() * 2)
    );
    assert_eq!(
        sparkleflinger
            .projected_bind_group_source_storage_ids_for_test()
            .expect("required GPU compositor should expose admitted source identities")
            .len(),
        zones.len()
    );
    assert_eq!(
        sparkleflinger.screen_layer_host_allocation_count_for_test(),
        Some(0),
        "a zero-screen scene must not preflight or grow screen upload scratch"
    );

    for elapsed_ms in [16, 32, 48] {
        let frame = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &zones,
            1,
            elapsed_ms,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("warmed six-source GPU scene should keep rendering");
        assert!(matches!(frame.scene_frame, ProducerFrame::GpuTexture(_)));
        assert_eq!(
            sparkleflinger.projected_bind_group_creation_count_for_test(),
            Some(admitted_creations),
            "steady render lookup must never create a bind group"
        );
        assert_eq!(
            sparkleflinger.screen_layer_host_allocation_count_for_test(),
            Some(0),
            "zero-screen projection must bypass screen upload state"
        );
    }
}

#[cfg(feature = "wgpu")]
#[test]
fn projected_bind_groups_retire_by_exact_source_lease_and_surface_generation() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let mut zones = gpu_projection_zone_set(6);
    let mut runtime = ZoneRuntime::new(4, 4);
    let first_dependency = SceneDependencyKey::new(1, registry.generation());
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            first_dependency,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("six-source GPU scene should admit");
    let first_surface_generation = sparkleflinger
        .active_surface_generation_for_test()
        .expect("GPU compositor surface should be installed");
    let admitted_creations = sparkleflinger
        .projected_bind_group_creation_count_for_test()
        .expect("GPU compositor should expose bind creation");

    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let target_fps = HashMap::new();
    let display_descriptors = HashMap::new();
    let context = RenderSceneContext {
        zones: &zones,
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key: first_dependency,
        elapsed_ms: 0,
        display_zone_target_fps: &target_fps,
        display_zone_descriptors: &display_descriptors,
        registry: &registry,
        authoritative_spatial_engine: None,
        inputs: ZoneFrameInputs {
            delta_secs: 1.0 / 60.0,
            audio: &audio,
            interaction: &interaction,
            screen: None,
            sensors: &sensors,
            input_availability: InputSourceAvailability::default(),
            media: None,
            net: None,
            lighting: None,
        },
    };
    let mut output = super::super::render_pass::RenderedZonePassOutput::default();
    let mut projected = runtime
        .render_scene_contributor_frames(context, &mut sparkleflinger, true, &mut output)
        .expect("projected contributors should render");
    let stale_layer = projected
        .layers
        .pop()
        .expect("six projected zones should publish a final source layer");
    let stale_storage_id = stale_layer
        .gpu_frame_identity_for_test()
        .expect("held projected layer should remain GPU-resident")
        .0;
    drop(projected);
    assert_eq!(stale_layer.gpu_frame_lease_count_for_test(), Some(2));

    zones.pop();
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(2, registry.generation()),
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("zone removal should commit an exact projected generation");
    assert_eq!(
        sparkleflinger.active_surface_generation_for_test(),
        Some(first_surface_generation),
        "same-size source changes should preserve the target surface generation"
    );
    assert_eq!(
        sparkleflinger.projected_bind_group_creation_count_for_test(),
        Some(admitted_creations),
        "unchanged source identities should reuse their prepared bindings"
    );
    assert_eq!(
        sparkleflinger.projected_bind_group_entry_count_for_test(),
        Some(zones.len() * 2)
    );
    assert!(
        !sparkleflinger
            .projected_bind_group_source_storage_ids_for_test()
            .expect("active projected source identities should be visible")
            .contains(&stale_storage_id)
    );
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(2),
        "both directions for the leased stale source must survive retirement"
    );
    assert_eq!(stale_layer.gpu_frame_lease_count_for_test(), Some(1));

    drop(stale_layer);
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(3, registry.generation()),
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("next admission should prune fully released source bindings");
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(0)
    );

    let source_ids_before_resize = sparkleflinger
        .projected_bind_group_source_storage_ids_for_test()
        .expect("source identities should be visible before resize");
    zones[0].layout.canvas_width = 2;
    zones[0].layout.canvas_height = 2;
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(4, registry.generation()),
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("source resize should prewarm the replacement source bindings");
    let source_ids_after_resize = sparkleflinger
        .projected_bind_group_source_storage_ids_for_test()
        .expect("source identities should be visible after resize");
    assert_ne!(source_ids_after_resize, source_ids_before_resize);
    assert_eq!(
        sparkleflinger.projected_bind_group_creation_count_for_test(),
        Some(admitted_creations + 2),
        "one resized source should prepare exactly two direction bindings"
    );
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(0),
        "unleased resized source bindings should prune during admission"
    );

    let canvas = sparkleflinger
        .prepare_canvas_resize(8, 8)
        .expect("target resize should prepare a distinct compositor generation");
    assert!(canvas.is_admitted());
    let requirements = projected_zone_requirements(&zones);
    let projected =
        sparkleflinger.prepare_projected_scene_resources(&requirements, true, 8, 8, Some(&canvas));
    sparkleflinger.apply_canvas_resize(canvas);
    sparkleflinger.apply_projected_scene_resources(projected);
    assert_ne!(
        sparkleflinger.active_surface_generation_for_test(),
        Some(first_surface_generation),
        "target resize must never alias a prior surface generation"
    );
    assert_eq!(
        sparkleflinger.projected_bind_group_creation_count_for_test(),
        Some(admitted_creations + 2 + zones.len() * 2),
        "a new target generation must prepare both directions for every source"
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn disabled_gpu_projection_releases_active_and_retired_resources() {
    assert_non_admitted_projection_releases_resources(false);
}

#[cfg(feature = "wgpu")]
#[test]
fn gpu_projection_resource_fallback_releases_active_and_retired_resources() {
    assert_non_admitted_projection_releases_resources(true);
}

#[cfg(feature = "wgpu")]
fn assert_non_admitted_projection_releases_resources(inject_failure: bool) {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let mut zones = gpu_projection_zone_set(3);
    zones[0].layout.canvas_width = 2;
    zones[0].layout.canvas_height = 2;
    let mut runtime = ZoneRuntime::new(4, 4);
    let first_dependency = SceneDependencyKey::new(1, registry.generation());
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            first_dependency,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("mixed-extent projected scene should admit");

    let mut first_layers = render_projected_layers_for_test(
        &mut runtime,
        &zones,
        first_dependency,
        &registry,
        0,
        &mut sparkleflinger,
    );
    let stale_layer = first_layers
        .pop()
        .expect("projected scene should publish a stale-source candidate");
    drop(first_layers);
    assert_eq!(stale_layer.gpu_frame_lease_count_for_test(), Some(2));

    zones.pop();
    let second_dependency = SceneDependencyKey::new(2, registry.generation());
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            second_dependency,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("source removal should retire its leased bindings");
    assert_eq!(
        sparkleflinger.projected_bind_group_entry_count_for_test(),
        Some(zones.len() * 2)
    );
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(2)
    );
    assert_eq!(stale_layer.gpu_frame_lease_count_for_test(), Some(1));

    let mut current_layers = render_projected_layers_for_test(
        &mut runtime,
        &zones,
        second_dependency,
        &registry,
        16,
        &mut sparkleflinger,
    );
    let current_layer = current_layers
        .pop()
        .expect("current projected source should remain externally leaseable");
    drop(current_layers);
    assert_eq!(current_layer.gpu_frame_lease_count_for_test(), Some(2));
    assert!(
        sparkleflinger
            .projected_snapshot_retained_bytes_for_test()
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(
        sparkleflinger
            .compositor_surface_cache_entry_count_for_test()
            .is_some_and(|entries| entries > 0)
    );

    let requirements = projected_zone_requirements(&zones);
    if inject_failure {
        sparkleflinger.fail_next_projected_scene_preparation_for_test();
    }
    let preparation =
        sparkleflinger.prepare_projected_scene_resources(&requirements, inject_failure, 4, 4, None);
    sparkleflinger.apply_projected_scene_resources(preparation);

    assert_eq!(
        sparkleflinger.projected_bind_group_entry_count_for_test(),
        Some(0)
    );
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(0)
    );
    assert_eq!(
        sparkleflinger.projected_snapshot_retained_bytes_for_test(),
        Some(0)
    );
    assert_eq!(
        sparkleflinger.compositor_surface_cache_entry_count_for_test(),
        Some(0)
    );
    assert_eq!(stale_layer.gpu_frame_lease_count_for_test(), Some(1));
    assert_eq!(current_layer.gpu_frame_lease_count_for_test(), Some(1));

    drop((stale_layer, current_layer));
    let disabled = sparkleflinger.prepare_projected_scene_resources(&[], false, 4, 4, None);
    sparkleflinger.apply_projected_scene_resources(disabled);
    assert_eq!(
        sparkleflinger.projected_bind_group_entry_count_for_test(),
        Some(0)
    );
    assert_eq!(
        sparkleflinger.retired_projected_bind_group_entry_count_for_test(),
        Some(0)
    );
    assert_eq!(
        sparkleflinger.projected_snapshot_retained_bytes_for_test(),
        Some(0)
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn mixed_screen_and_projected_sources_reuse_upload_scratch_and_preserve_pixels() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let mut zones = gpu_projection_zone_set(1);
    zones[0].layout.zones[0].size = NormalizedPosition::new(0.5, 0.5);
    let dependency_key = SceneDependencyKey::new(1, registry.generation());
    let mut runtime = ZoneRuntime::new(4, 4);
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            dependency_key,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("mixed screen scene should admit its projected source");
    let screen = ProducerFrame::screen_publication(cpu_screen_publication(4, 4, [0, 255, 0, 255]))
        .expect("RGBA screen publication should become a producer frame");

    let mut layers = render_projected_layers_for_test(
        &mut runtime,
        &zones,
        dependency_key,
        &registry,
        0,
        &mut sparkleflinger,
    );
    layers.insert(1, CompositionLayer::replace_opaque(screen.clone()));
    let first = runtime
        .compose_projected_scene_frame(layers, &mut sparkleflinger)
        .expect("mixed screen and projected sources should compose");
    assert!(matches!(first, ProducerFrame::GpuTexture(_)));
    let first_pixels = sample_full_gpu_canvas(&mut sparkleflinger, 4, 4);
    assert_eq!(first_pixels[0], [0, 255, 0]);
    assert_eq!(first_pixels[5], [255, 0, 0]);
    let scratch_allocations = sparkleflinger
        .screen_layer_host_allocation_count_for_test()
        .expect("GPU compositor should expose screen scratch growth");
    assert_eq!(scratch_allocations, 1);
    let bind_creations = sparkleflinger
        .projected_bind_group_creation_count_for_test()
        .expect("GPU compositor should expose projected bind creation");

    let mut layers = render_projected_layers_for_test(
        &mut runtime,
        &zones,
        dependency_key,
        &registry,
        16,
        &mut sparkleflinger,
    );
    layers.insert(1, CompositionLayer::replace_opaque(screen));
    let second = runtime
        .compose_projected_scene_frame(layers, &mut sparkleflinger)
        .expect("warmed mixed screen scene should compose");
    assert!(matches!(second, ProducerFrame::GpuTexture(_)));
    assert_eq!(
        sample_full_gpu_canvas(&mut sparkleflinger, 4, 4),
        first_pixels
    );
    assert_eq!(
        sparkleflinger.screen_layer_host_allocation_count_for_test(),
        Some(scratch_allocations),
        "mixed screen plans should reuse admitted host scratch"
    );
    assert_eq!(
        sparkleflinger.projected_bind_group_creation_count_for_test(),
        Some(bind_creations),
        "screen upload must not disturb admitted projected bindings"
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn mixed_extent_gpu_projection_reuses_every_admitted_compositor_set() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let mut zones = gpu_projection_zones();
    zones[0].layout.canvas_width = 8;
    zones[0].layout.canvas_height = 4;
    zones[1].layout.canvas_width = 4;
    zones[1].layout.canvas_height = 2;
    let canvas = sparkleflinger
        .prepare_canvas_resize(8, 8)
        .expect("mixed extent scene canvas should prepare");
    assert!(canvas.is_admitted());
    sparkleflinger.apply_canvas_resize(canvas);
    let dependency_key = SceneDependencyKey::new(1, registry.generation());
    let mut runtime = ZoneRuntime::new(8, 8);
    runtime
        .admit_reconcile(
            &zones,
            Some(SceneId::DEFAULT),
            dependency_key,
            &registry,
            &HashMap::new(),
            None,
            &mut sparkleflinger,
        )
        .expect("mixed GPU descriptors should admit transactionally");
    let admitted_allocations = sparkleflinger
        .compositor_surface_allocation_count_for_test()
        .expect("required GPU compositor should expose surface allocations");
    let mut zone_colors = Vec::new();

    for elapsed_ms in [0, 16, 32] {
        let rendered = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &zones,
            1,
            elapsed_ms,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("mixed GPU descriptors should render from admitted resources");
        assert!(matches!(rendered.scene_frame, ProducerFrame::GpuTexture(_)));
        assert_eq!(runtime.scene_cpu_backing_bytes(), 0);
        assert_eq!(
            sparkleflinger.compositor_surface_allocation_count_for_test(),
            Some(admitted_allocations),
            "steady mixed-size frames must not allocate compositor textures"
        );
    }
}

#[cfg(feature = "wgpu")]
#[test]
fn failed_gpu_projection_reuses_last_good_scene_across_dependency_change() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let zones = gpu_projection_zones();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();
    let first = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("initial GPU projection should render and retain");
    let ProducerFrame::GpuTexture(first_frame) = &first.scene_frame else {
        panic!("initial retained scene should stay GPU-resident")
    };
    assert_eq!(first_frame.origin, GpuTextureFrameOrigin::ImmutableSnapshot);
    let first_identity = (first_frame.storage_id, first_frame.content_generation);
    let first_pixels = sample_full_gpu_canvas(&mut sparkleflinger, 4, 4);
    let LedSamplingStrategy::SparkleFlinger(first_led_engine) = &first.led_sampling_strategy else {
        panic!("initial GPU scene should retain its SAT sampling plan")
    };
    let first_leds = sample_gpu_plan(&mut sparkleflinger, first_led_engine);

    let mut intervening = Canvas::new(4, 4);
    intervening.fill(Rgba::new(0, 255, 0, 255));
    let _ = sparkleflinger.compose_for_outputs(
        CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(intervening)),
        ),
        false,
        None,
    );
    assert_eq!(
        sample_full_gpu_canvas(&mut sparkleflinger, 4, 4)[0],
        [0, 255, 0]
    );

    let changed_dependency_key = SceneDependencyKey::new(2, registry.generation());
    assert!(runtime.reuse_scene(changed_dependency_key).is_none());
    runtime.fail_next_projected_scene_composition_for_test();
    let retained = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        2,
        16,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("failed projection should reuse the retained scene");
    let ProducerFrame::GpuTexture(retained_frame) = &retained.scene_frame else {
        panic!("failed projection should not materialize stale CPU targets")
    };

    assert_eq!(
        (retained_frame.storage_id, retained_frame.content_generation),
        first_identity
    );
    assert_eq!(
        sample_full_gpu_canvas(&mut sparkleflinger, 4, 4),
        first_pixels
    );
    let LedSamplingStrategy::SparkleFlinger(retained_led_engine) = &retained.led_sampling_strategy
    else {
        panic!("retained GPU scene should preserve its SAT sampling plan")
    };
    assert_eq!(
        sample_gpu_plan(&mut sparkleflinger, retained_led_engine),
        first_leds
    );
    assert!(runtime.reuse_scene(changed_dependency_key).is_none());
}

#[cfg(feature = "wgpu")]
#[test]
fn first_gpu_projection_failure_returns_typed_error_without_partial_cpu_scene() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let zones = gpu_projection_zones();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();
    runtime.fail_next_projected_scene_composition_for_test();

    let Err(error) = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    ) else {
        panic!("first unreplayable GPU projection failure must not publish partial CPU output")
    };

    assert!(
        error
            .downcast_ref::<super::super::model::GpuProjectionReplayUnavailable>()
            .is_some()
    );
    assert!(runtime.retained_frame.is_none());
}

#[cfg(feature = "wgpu")]
#[test]
fn projected_scene_resources_allocate_only_during_admission_changes() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let mut zones = gpu_projection_zones();
    let initial_allocations = sparkleflinger
        .snapshot_texture_allocation_count_for_test()
        .expect("required GPU compositor should expose snapshot allocations");
    let requirements = projected_zone_requirements(&zones);
    let prepared =
        sparkleflinger.prepare_projected_scene_resources(&requirements, true, 4, 4, None);
    sparkleflinger.apply_projected_scene_resources(prepared);
    let stable_allocations = initial_allocations + zones.len();
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations)
    );

    let prepared =
        sparkleflinger.prepare_projected_scene_resources(&requirements, true, 4, 4, None);
    sparkleflinger.apply_projected_scene_resources(prepared);
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations)
    );

    zones[1].layout.canvas_width = 2;
    zones[1].layout.canvas_height = 2;
    let resized_requirements = projected_zone_requirements(&zones);
    let prepared =
        sparkleflinger.prepare_projected_scene_resources(&resized_requirements, true, 4, 4, None);
    sparkleflinger.apply_projected_scene_resources(prepared);
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations + 1)
    );

    let canvas = sparkleflinger
        .prepare_canvas_resize(8, 8)
        .expect("resized scene snapshot generations should prepare");
    sparkleflinger.apply_canvas_resize(canvas);
    let prepared =
        sparkleflinger.prepare_projected_scene_resources(&resized_requirements, true, 8, 8, None);
    sparkleflinger.apply_projected_scene_resources(prepared);
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations + 3),
        "scene resize should allocate only its two immutable generations"
    );

    let prepared = sparkleflinger.prepare_projected_scene_resources(
        &resized_requirements[..1],
        true,
        8,
        8,
        None,
    );
    sparkleflinger.apply_projected_scene_resources(prepared);
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations + 3)
    );
    let prepared =
        sparkleflinger.prepare_projected_scene_resources(&resized_requirements, true, 8, 8, None);
    sparkleflinger.apply_projected_scene_resources(prepared);
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(stable_allocations + 4)
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn unsupported_filter_and_wrap_skip_gpu_projection_without_rejecting_scene() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let mut zones = gpu_projection_zones();
    zones[0].layout.zones[0].sampling_mode = Some(SamplingMode::Bilinear);
    zones[1].layout.zones[0].edge_behavior = Some(EdgeBehavior::Wrap);
    let allocations_before = sparkleflinger
        .snapshot_texture_allocation_count_for_test()
        .expect("required GPU compositor should expose snapshot allocations");
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();

    let rendered = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("unsupported GPU projection geometry must retain the valid CPU path");

    assert!(!matches!(
        rendered.scene_frame,
        ProducerFrame::GpuTexture(_)
    ));
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(allocations_before)
    );
    assert!(zones.iter().all(|zone| {
        !sparkleflinger.has_projected_zone_resource(
            zone.id,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }));
    assert_eq!(runtime.scene_cpu_backing_bytes(), 192);
}

#[cfg(feature = "wgpu")]
#[test]
fn gpu_projection_allocation_failure_falls_back_without_rejecting_scene() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let zones = gpu_projection_zones();
    let allocations_before = sparkleflinger
        .snapshot_texture_allocation_count_for_test()
        .expect("required GPU compositor should expose snapshot allocations");
    sparkleflinger.fail_next_projected_scene_preparation_for_test();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();

    let rendered = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("GPU projection allocation failure must retain the valid CPU path");

    assert!(!matches!(
        rendered.scene_frame,
        ProducerFrame::GpuTexture(_)
    ));
    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(allocations_before)
    );
    assert!(zones.iter().all(|zone| {
        !sparkleflinger.has_projected_zone_resource(
            zone.id,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }));
    assert_eq!(runtime.scene_cpu_backing_bytes(), 192);
}

#[cfg(feature = "wgpu")]
#[test]
fn oversized_cpu_canvas_fallback_skips_gpu_projection_resource_preparation() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let oversized_width = sparkleflinger
        .max_texture_dimension_2d()
        .expect("required GPU compositor should expose its texture limit")
        .checked_add(1)
        .expect("GPU texture limit must leave room for a CPU-only extent");
    let candidate = sparkleflinger
        .prepare_canvas_resize(oversized_width, 1)
        .expect("addressable CPU fallback canvas should prepare");
    let gpu_projection_admitted = candidate.gpu_output_admitted();
    assert!(!gpu_projection_admitted);
    let zones = gpu_projection_zones();
    let requirements = projected_zone_requirements(&zones);
    let allocations_before = sparkleflinger
        .snapshot_texture_allocation_count_for_test()
        .expect("required GPU compositor should expose snapshot allocations");

    let projected = sparkleflinger.prepare_projected_scene_resources(
        &requirements,
        gpu_projection_admitted,
        oversized_width,
        1,
        None,
    );
    sparkleflinger.apply_projected_scene_resources(projected);

    assert_eq!(
        sparkleflinger.snapshot_texture_allocation_count_for_test(),
        Some(allocations_before)
    );
    assert!(zones.iter().all(|zone| {
        !sparkleflinger.has_projected_zone_resource(
            zone.id,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }));
    sparkleflinger.apply_canvas_resize(candidate);
    assert!(!sparkleflinger.supports_gpu_output_frames());
}

#[cfg(feature = "wgpu")]
#[test]
fn initial_and_switched_scenes_admit_gpu_projection_before_rendering() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let initial_zones = gpu_projection_zones();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone_colors = Vec::new();
    let initial = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &initial_zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("initial scene should admit GPU projection before its first render");
    assert!(matches!(
        initial.scene_frame,
        ProducerFrame::GpuTexture(ref frame)
            if frame.origin == GpuTextureFrameOrigin::ImmutableSnapshot
    ));
    assert!(initial_zones.iter().all(|zone| {
        sparkleflinger.has_projected_zone_resource(
            zone.id,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }));

    let switched_zones = gpu_projection_zones();
    let switched = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &switched_zones,
        2,
        16,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("scene switch should admit its GPU projection before rendering");
    assert!(matches!(
        switched.scene_frame,
        ProducerFrame::GpuTexture(ref frame)
            if frame.origin == GpuTextureFrameOrigin::ImmutableSnapshot
    ));
    assert!(switched_zones.iter().all(|zone| {
        sparkleflinger.has_projected_zone_resource(
            zone.id,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }));
    assert_eq!(
        sample_full_gpu_canvas(&mut sparkleflinger, 4, 4)[5],
        [0, 0, 255]
    );
}

#[cfg(feature = "wgpu")]
fn required_gpu_sparkleflinger() -> Option<SparkleFlinger> {
    match SparkleFlinger::new_required_gpu_for_test() {
        Ok(mut sparkleflinger) => {
            if std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_some()
                && let Some(required) =
                    crate::render_thread::gpu_device::GpuRenderDevice::required_test_backend()
            {
                assert_eq!(
                    sparkleflinger.gpu_backend_name_for_test(),
                    Some(crate::render_thread::gpu_device::backend_name(required)),
                    "mandatory GPU projection tests must exercise the platform backend"
                );
            }
            let canvas = sparkleflinger
                .prepare_canvas_resize(4, 4)
                .expect("required GPU test canvas should prepare");
            assert!(canvas.is_admitted());
            sparkleflinger.apply_canvas_resize(canvas);
            Some(sparkleflinger)
        }
        Err(error) => {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU projection test was required but initialization failed: {error}"
            );
            None
        }
    }
}

#[cfg(feature = "wgpu")]
fn sample_full_gpu_canvas(
    sparkleflinger: &mut SparkleFlinger,
    width: u32,
    height: u32,
) -> Vec<[u8; 3]> {
    let mut zone = point_zone("gpu_scene_pixels");
    zone.size = NormalizedPosition::new(1.0, 1.0);
    zone.topology = LedTopology::Matrix {
        width,
        height,
        serpentine: false,
        start_corner: Corner::TopLeft,
    };
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let engine = SpatialEngine::new(SpatialLayout {
        id: "gpu-scene-pixels".into(),
        name: "GPU Scene Pixels".into(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones: vec![zone],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    });
    sample_gpu_plan(sparkleflinger, &engine)
        .into_iter()
        .next()
        .expect("full-canvas sampling should produce one zone")
        .colors
}

#[cfg(feature = "wgpu")]
fn sample_gpu_plan(sparkleflinger: &mut SparkleFlinger, engine: &SpatialEngine) -> Vec<ZoneColors> {
    let mut sampled = Vec::new();
    assert!(
        sparkleflinger
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampling should complete")
    );
    sampled
}

#[cfg(feature = "wgpu")]
fn admit_projected_scene_resources(
    sparkleflinger: &mut SparkleFlinger,
    zones: &[Zone],
    width: u32,
    height: u32,
) {
    let canvas = sparkleflinger
        .prepare_canvas_resize(width, height)
        .expect("GPU scene canvas resources should prepare");
    assert!(canvas.is_admitted());
    sparkleflinger.apply_canvas_resize(canvas);
    let requirements = projected_zone_requirements(zones);
    let projected =
        sparkleflinger.prepare_projected_scene_resources(&requirements, true, width, height, None);
    sparkleflinger.apply_projected_scene_resources(projected);
}

#[cfg(feature = "wgpu")]
fn projected_zone_requirements(
    zones: &[Zone],
) -> Vec<super::super::super::sparkleflinger::ProjectedZoneTextureRequirement> {
    zones
        .iter()
        .map(
            |zone| super::super::super::sparkleflinger::ProjectedZoneTextureRequirement {
                zone_id: zone.id,
                width: zone.layout.canvas_width,
                height: zone.layout.canvas_height,
            },
        )
        .collect()
}

fn compose_projection_layers_on_cpu(
    mut layers: Vec<CompositionLayer>,
    width: u32,
    height: u32,
) -> Canvas {
    layers.insert(
        0,
        CompositionLayer::replace_opaque(ProducerFrame::Canvas(Canvas::new(width, height))),
    );
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(width, height, layers).with_cpu_replay_cacheable(false),
        true,
        None,
    );
    composed
        .sampling_canvas
        .or_else(|| {
            composed
                .sampling_surface
                .map(|surface| Canvas::from_published_surface(&surface))
        })
        .expect("CPU projection should materialize a scene canvas")
}

fn cpu_backed_runtime(width: u32, height: u32) -> ZoneRuntime {
    let mut runtime = ZoneRuntime::new(width, height);
    runtime.scene_surface_pool = Some(
        ZoneRuntime::prepare_scene_surface_pool(
            width,
            height,
            SCENE_SURFACE_POOL_INITIAL_SLOTS,
            SCENE_SURFACE_POOL_MAX_SLOTS,
        )
        .expect("test CPU scene surface should prepare"),
    );
    runtime
}

#[cfg(feature = "wgpu")]
fn gpu_projection_zones() -> [Zone; 2] {
    let mut back = sample_zone(4, 4);
    make_color_fill_zone(&mut back);
    let mut back_zone = point_zone("back");
    back_zone.size = NormalizedPosition::new(1.0, 1.0);
    back_zone.sampling_mode = Some(SamplingMode::Nearest);
    back_zone.edge_behavior = Some(EdgeBehavior::Clamp);
    back.layout.zones = vec![back_zone];
    back.layout.default_sampling_mode = SamplingMode::Nearest;
    let mut front = sample_zone(4, 4);
    make_color_fill_zone(&mut front);
    let LayerSource::ColorFill { rgba } = &mut front.layers[0].source else {
        unreachable!("color-fill helper should create a color layer")
    };
    *rgba = [0.0, 0.0, 1.0, 1.0];
    let mut front_zone = point_zone("front");
    front_zone.size = NormalizedPosition::new(0.5, 0.5);
    front_zone.sampling_mode = Some(SamplingMode::Nearest);
    front_zone.edge_behavior = Some(EdgeBehavior::Clamp);
    front.layout.zones = vec![front_zone];
    front.layout.default_sampling_mode = SamplingMode::Nearest;
    [back, front]
}

#[cfg(feature = "wgpu")]
fn gpu_projection_zone_set(count: usize) -> Vec<Zone> {
    (0..count)
        .map(|index| {
            let mut zone = sample_zone(4, 4);
            make_color_fill_zone(&mut zone);
            let LayerSource::ColorFill { rgba } = &mut zone.layers[0].source else {
                unreachable!("color-fill helper should create a color layer")
            };
            *rgba = match index % 3 {
                0 => [1.0, 0.0, 0.0, 1.0],
                1 => [0.0, 1.0, 0.0, 1.0],
                _ => [0.0, 0.0, 1.0, 1.0],
            };
            let mut output = point_zone(&format!("projected_{index}"));
            output.size = NormalizedPosition::new(1.0, 1.0);
            output.sampling_mode = Some(SamplingMode::Nearest);
            output.edge_behavior = Some(EdgeBehavior::Clamp);
            zone.layout.zones = vec![output];
            zone.layout.default_sampling_mode = SamplingMode::Nearest;
            zone
        })
        .collect()
}

#[cfg(feature = "wgpu")]
fn render_projected_layers_for_test(
    runtime: &mut ZoneRuntime,
    zones: &[Zone],
    dependency_key: SceneDependencyKey,
    registry: &EffectRegistry,
    elapsed_ms: u64,
    sparkleflinger: &mut SparkleFlinger,
) -> Vec<CompositionLayer> {
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let target_fps = HashMap::new();
    let display_descriptors = HashMap::new();
    let context = RenderSceneContext {
        zones,
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key,
        elapsed_ms,
        display_zone_target_fps: &target_fps,
        display_zone_descriptors: &display_descriptors,
        registry,
        authoritative_spatial_engine: None,
        inputs: ZoneFrameInputs {
            delta_secs: 1.0 / 60.0,
            audio: &audio,
            interaction: &interaction,
            screen: None,
            sensors: &sensors,
            input_availability: InputSourceAvailability::default(),
            media: None,
            net: None,
            lighting: None,
        },
    };
    let mut output = super::super::render_pass::RenderedZonePassOutput::default();
    runtime
        .render_scene_contributor_frames(context, sparkleflinger, true, &mut output)
        .expect("projected contributors should render")
        .layers
}

#[cfg(feature = "wgpu")]
fn cpu_screen_publication(
    width: u32,
    height: u32,
    rgba: [u8; 4],
) -> Arc<hypercolor_core::input::screen::ScreenBranchPublication> {
    let extent = PixelExtent::new(width, height).expect("screen fixture extent is non-empty");
    let source_id = CaptureSourceId::new("synthetic:mixed-compositor-screen")
        .expect("screen fixture source id is non-empty");
    let source_epoch = CaptureEpoch {
        source_id,
        topology_generation: 1,
        session_generation: 1,
    };
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("screen fixture geometry is valid");
    let source = ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        source_epoch.clone(),
        ResolvedScreenSourceConfig::new(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                1,
                1,
            ),
        ),
    );
    let executor = CpuReductionExecutor::new(
        NonZeroUsize::new(1).expect("fixture worker count is non-zero"),
        NonZeroU32::MIN,
    )
    .expect("fixture reducer should construct");
    let demand = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                NonZeroU32::new(width),
                NonZeroU32::new(height),
                ScreenUpscalePolicy::Allow,
            ),
            ScreenAspectPolicy::Cover,
            Arc::new(
                hypercolor_core::input::screen::ScreenProcessingProfile::new(
                    hypercolor_core::input::screen::ScreenProcessingProfileConfig::default(),
                ),
            ),
        ),
        NonZeroU32::new(60).expect("fixture cadence is non-zero"),
    )
    .resolve_with_color_capabilities(&source, executor.capabilities())
    .expect("screen fixture demand should resolve");
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("screen fixture demand revision remains representable");
    let mut preparing = builder
        .prepare(
            [demand],
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("screen fixture plan should prepare");
    let mut lifetimes = Vec::new();
    for required_source in preparing.required_sources().to_vec() {
        let ticket = preparing
            .worker_ticket(&required_source)
            .expect("screen fixture worker ticket should exist");
        let resources = ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .expect("screen fixture exact resources should allocate");
        let worker_lifetimes = resources
            .iter()
            .map(|resource| ticket.bind_resource_lifetime(resource))
            .collect::<Result<Vec<_>, _>>()
            .expect("screen fixture lifetimes should bind");
        let token = ticket
            .acknowledge(
                ScreenExactResourceLedger::try_new(resources)
                    .expect("screen fixture ledger should validate"),
                &worker_lifetimes,
            )
            .expect("screen fixture resources should satisfy the ticket");
        preparing
            .acknowledge(token)
            .expect("screen fixture token should belong to the candidate");
        lifetimes.push(worker_lifetimes);
    }
    let armed = preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("screen fixture plan should arm: {}", failure.error()));
    let committed = builder
        .commit(armed, demand_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("screen fixture plan should commit: {}", failure.error()));
    drop(lifetimes);
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved screen fixture retirement should reclaim");
    let descriptor = plan.branches()[0].descriptor().clone();
    let state = builder.committed_state();
    let binding = state
        .worker_bindings()
        .iter()
        .find(|binding| state.publisher(&descriptor, binding).is_ok())
        .cloned()
        .expect("screen fixture binding should own the descriptor");
    let publisher = hub
        .publisher(&descriptor, &binding)
        .expect("screen fixture publisher should remain committed");
    let captured_at = Instant::now();
    let metadata = ScreenPublicationMetadata::try_intent(
        source_epoch,
        plan.generation(),
        NonZeroU64::MIN,
        captured_at,
        captured_at + Duration::from_secs(1),
    )
    .expect("screen fixture metadata should validate");
    let mut prepared = hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &metadata)
        .expect("screen fixture publication slot should reserve");
    for pixel in prepared
        .surface_pixels_mut()
        .expect("screen fixture surface should be CPU-writable")
        .chunks_exact_mut(4)
    {
        pixel.copy_from_slice(&rgba);
    }
    let receipt = hub
        .finalize_writable_publication(prepared, captured_at, ScreenPublicationHealth::Healthy)
        .expect("screen fixture publication should finalize");
    Arc::clone(receipt.publication())
}
