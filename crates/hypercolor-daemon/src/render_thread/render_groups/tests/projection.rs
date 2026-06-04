use super::*;

#[test]
fn single_group_preview_publishes_surface_frame() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_group(4, 4);
    let mut source = Canvas::new(4, 4);
    source.fill(Rgba::new(12, 34, 56, 255));
    runtime.target_canvases.insert(group.id, source);

    let preview = runtime.compose_preview_grid_for_test(&[group]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("single-group preview should publish a pooled surface");
    };

    assert_eq!(surface.width(), 4);
    assert_eq!(surface.height(), 4);
    assert_eq!(surface.get_pixel(0, 0), Rgba::new(12, 34, 56, 255));
    assert_eq!(surface.get_pixel(3, 3), Rgba::new(12, 34, 56, 255));
}

#[test]
fn single_group_preview_scales_group_canvas_to_preview_extent() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_group(2, 2);
    let mut source = Canvas::new(2, 2);
    source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
    runtime.target_canvases.insert(group.id, source);

    let preview = runtime.compose_preview_grid_for_test(&[group]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("scaled single-group preview should publish a pooled surface");
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
fn compose_preview_ignores_display_groups() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let preview_group = sample_group(4, 4);
    let display_group = sample_display_group(4, 4);
    let mut preview_canvas = Canvas::new(4, 4);
    preview_canvas.fill(Rgba::new(255, 0, 0, 255));
    let mut display_canvas = Canvas::new(4, 4);
    display_canvas.fill(Rgba::new(0, 0, 255, 255));
    runtime
        .target_canvases
        .insert(preview_group.id, preview_canvas);
    runtime
        .target_canvases
        .insert(display_group.id, display_canvas);

    let preview = runtime.compose_preview_grid_for_test(&[preview_group, display_group]);
    let ProducerFrame::Surface(surface) = preview else {
        panic!("mixed preview should publish a pooled surface");
    };

    assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(surface.get_pixel(3, 3), Rgba::new(255, 0, 0, 255));
}

#[test]
fn authoritative_scene_canvas_clips_rotated_zone_geometry() {
    let mut runtime = ZoneRuntime::new(8, 8);
    let mut group = sample_group(8, 8);
    group.layout.zones = vec![rotated_zone("zone_rotated", FRAC_PI_4, 0.5)];
    let mut source = Canvas::new(8, 8);
    source.fill(Rgba::new(255, 0, 0, 255));
    runtime.target_canvases.insert(group.id, source);

    let scene_frame = runtime.compose_scene_frame(&[group]);
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
fn authoritative_scene_canvas_preserves_group_overlap_order() {
    let mut runtime = ZoneRuntime::new(8, 8);
    let mut back_group = sample_group(8, 8);
    back_group.layout.zones = vec![rotated_zone("zone_back", FRAC_PI_4, 0.5)];
    let mut front_group = sample_group(8, 8);
    front_group.layout.zones = vec![point_zone("zone_front")];
    front_group.layout.zones[0].size = NormalizedPosition { x: 0.25, y: 0.25 };

    let mut back_source = Canvas::new(8, 8);
    back_source.fill(Rgba::new(255, 0, 0, 255));
    let mut front_source = Canvas::new(8, 8);
    front_source.fill(Rgba::new(0, 0, 255, 255));
    runtime.target_canvases.insert(back_group.id, back_source);
    runtime.target_canvases.insert(front_group.id, front_source);

    let scene_frame = runtime.compose_scene_frame(&[back_group, front_group]);
    let ProducerFrame::Surface(surface) = scene_frame else {
        panic!("authoritative scene canvas should publish a pooled surface");
    };

    assert_eq!(
        surface.get_pixel(4, 4),
        Rgba::new(0, 0, 255, 255),
        "later groups should overwrite earlier groups in overlapping regions"
    );
    assert_eq!(
        surface.get_pixel(2, 4),
        Rgba::new(255, 0, 0, 255),
        "pixels only covered by the back group should keep its content"
    );
}

#[test]
fn authoritative_scene_canvas_uses_zone_sampling_mode() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut group = sample_group(2, 2);
    group.layout.zones = vec![point_zone("zone_sampling")];
    group.layout.zones[0].size = NormalizedPosition { x: 1.0, y: 1.0 };
    group.layout.zones[0].sampling_mode = Some(SamplingMode::Nearest);
    let mut source = Canvas::new(2, 2);
    source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
    runtime.target_canvases.insert(group.id, source);

    let scene_frame = runtime.compose_scene_frame(&[group]);
    let ProducerFrame::Surface(surface) = scene_frame else {
        panic!("authoritative scene canvas should publish a pooled surface");
    };

    assert_eq!(surface.get_pixel(1, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(surface.get_pixel(2, 0), Rgba::new(0, 255, 0, 255));
    assert_eq!(surface.get_pixel(1, 3), Rgba::new(0, 0, 255, 255));
    assert_eq!(surface.get_pixel(2, 3), Rgba::new(255, 255, 0, 255));
}

#[test]
fn render_scene_reuses_projection_cache_until_layout_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_group(2, 2);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    group.layout.zones = vec![point_zone_at("zone_cached", 0.25, 0.5)];
    let display_group_target_fps = HashMap::new();
    let mut zones = Vec::new();

    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("first render should build the projection cache");
    let cached_samples = runtime
        .scene_projection_cache
        .get(&group.id)
        .expect("scene group should have a cached projection")
        .zones[0]
        .samples
        .as_ptr();

    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        16,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("same dependency key should keep the projection cache");

    assert_eq!(
        runtime
            .scene_projection_cache
            .get(&group.id)
            .expect("scene group should keep a cached projection")
            .zones[0]
            .samples
            .as_ptr(),
        cached_samples
    );

    group.layout.zones[0].size = NormalizedPosition::new(1.0, 1.0);
    render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        2,
        32,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("layout changes should rebuild the projection cache");

    assert!(
        runtime
            .scene_projection_cache
            .get(&group.id)
            .expect("scene group should rebuild a cached projection")
            .zones[0]
            .samples
            .len()
            > 4
    );
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
        spaces: None,
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
        spaces: None,
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
    let mut zone = point_zone("zone_full_scene_identity");
    zone.position = NormalizedPosition::new(0.5, 0.5);
    zone.size = NormalizedPosition::new(1.0, 1.0);
    zone.scale = 1.0;
    zone.rotation = 0.0;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let group = Zone {
        id: ZoneId::new(),
        name: "Identity".into(),
        description: None,
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: SpatialLayout {
            id: "full-scene-identity".into(),
            name: "Full Scene Identity".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![zone.clone()],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
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
    let projection = build_group_projection(&group, 4, 4);
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
        &zone,
        zone.sampling_mode
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
    let mut zone = point_zone("zone_projected_composition");
    zone.position = NormalizedPosition::new(0.5, 0.5);
    zone.size = NormalizedPosition::new(1.0, 1.0);
    zone.rotation = 0.0;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut group = sample_group(4, 4);
    group.layout.zones = vec![zone];
    group.layout.default_sampling_mode = SamplingMode::Nearest;
    group.layout.default_edge_behavior = EdgeBehavior::Clamp;
    let projection = build_group_projection(&group, 4, 4);
    let source = patterned_source_canvas(4, 4);
    let layers = projection_composition_layers_for_group(
        &ProducerFrame::Canvas(source.clone()),
        &group,
        &projection,
        4,
        4,
    )
    .expect("nearest clamp projection should use composition layers");
    let mut projection_cache = HashMap::new();
    projection_cache.insert(group.id, projection);
    let mut target_canvases = HashMap::new();
    target_canvases.insert(group.id, source.clone());
    let mut projected = Canvas::new(4, 4);
    compose_authoritative_scene_canvas(
        &mut projected,
        std::slice::from_ref(&group),
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
fn projected_composition_rejects_bilinear_zones() {
    let mut zone = point_zone("zone_bilinear_projection");
    zone.sampling_mode = Some(SamplingMode::Bilinear);
    let mut group = sample_group(4, 4);
    group.layout.zones = vec![zone];
    let projection = build_group_projection(&group, 4, 4);

    assert!(
        projection_composition_layers_for_group(
            &ProducerFrame::Canvas(patterned_source_canvas(4, 4)),
            &group,
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
    let mut zone = point_zone("zone_gpu_projection");
    zone.position = NormalizedPosition::new(0.5, 0.5);
    zone.size = NormalizedPosition::new(1.0, 1.0);
    zone.rotation = 0.0;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut group = sample_group(4, 4);
    group.layout.zones = vec![zone];
    group.layout.default_sampling_mode = SamplingMode::Nearest;
    group.layout.default_edge_behavior = EdgeBehavior::Clamp;
    let projection = build_group_projection(&group, 4, 4);
    let source = patterned_source_canvas(4, 4);
    let Some(gpu_source) = sparkleflinger.upload_canvas_frame(&source) else {
        return;
    };
    let layers = projection_composition_layers_for_group(
        &ProducerFrame::GpuTexture(gpu_source),
        &group,
        &projection,
        4,
        4,
    )
    .expect("nearest clamp projection should use composition layers");
    let mut projection_cache = HashMap::new();
    projection_cache.insert(group.id, projection);
    let mut target_canvases = HashMap::new();
    target_canvases.insert(group.id, source);
    let mut projected = Canvas::new(4, 4);
    compose_authoritative_scene_canvas(
        &mut projected,
        std::slice::from_ref(&group),
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
        spaces: None,
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
    let Some(gpu_source) = sparkleflinger.upload_canvas_frame(&patterned_source_canvas(4, 4))
    else {
        return;
    };
    let mut runtime = ZoneRuntime::new(4, 4);
    let frame = runtime
        .compose_projected_scene_frame(
            vec![CompositionLayer::replace_opaque(ProducerFrame::GpuTexture(
                gpu_source,
            ))],
            &mut sparkleflinger,
        )
        .expect("GPU projection should export the current output frame");

    assert!(matches!(frame, ProducerFrame::GpuTexture(_)));
}
