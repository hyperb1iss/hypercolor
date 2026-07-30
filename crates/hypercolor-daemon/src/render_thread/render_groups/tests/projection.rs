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

    let scene_frame = runtime
        .compose_scene_frame(&[group])
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

    let scene_frame = runtime
        .compose_scene_frame(&[back_group, front_group])
        .expect("scene frame should allocate");
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

    let scene_frame = runtime
        .compose_scene_frame(&[group])
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
    let cached_bounds = runtime
        .scene_projection_cache
        .get(&group.id)
        .expect("scene group should have a cached projection")
        .zones[0]
        .bounds;

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
            .bounds,
        cached_bounds
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

    assert!(matches!(
        runtime
            .scene_projection_cache
            .get(&group.id)
            .expect("scene group should rebuild a cached projection")
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
        let mut group = sample_group(width, height);
        let mut zone = point_zone("full_scene");
        zone.size = NormalizedPosition::new(1.0, 1.0);
        zone.rotation = FRAC_PI_4;
        group.layout.zones = vec![zone];

        let work = build_group_projection(&group, width, height)
            .expect("projection metadata should allocate")
            .raster_work();

        assert_eq!(work.affine_setups, 1);
        assert_eq!(work.rows, u64::from(height));
        assert_eq!(work.pixels, u64::from(width) * u64::from(height));
    }
}

#[test]
fn projection_metadata_is_constant_size_for_large_dimensions() {
    let mut group = sample_group(u32::MAX, u32::MAX);
    let mut zone = point_zone("large");
    zone.position = NormalizedPosition::new(0.5, 0.5);
    zone.size = NormalizedPosition::new(1.0, 1.0);
    group.layout.zones = vec![zone];
    let projection = build_group_projection(&group, u32::MAX, u32::MAX)
        .expect("projection metadata should allocate");

    assert_eq!(projection.zones.len(), group.layout.zones.len());
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
    let projection =
        build_group_projection(&group, 4, 4).expect("projection metadata should allocate");
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
    let projection =
        build_group_projection(&group, 4, 4).expect("projection metadata should allocate");
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
fn projected_contributors_refresh_current_cpu_replay() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::default();
    let mut group = sample_group(4, 4);
    make_color_fill_group(&mut group);
    let mut zone = point_zone("cpu_replay");
    zone.size = NormalizedPosition::new(1.0, 1.0);
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    group.layout.zones = vec![zone];
    group.layout.default_sampling_mode = SamplingMode::Nearest;
    let dependency_key = SceneDependencyKey::new(1, registry.generation());
    runtime
        .reconcile(
            std::slice::from_ref(&group),
            Some(SceneId::DEFAULT),
            dependency_key,
            &registry,
            &HashMap::new(),
            None,
        )
        .expect("projection resources should reconcile");
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let target_fps = HashMap::new();
    let descriptors = HashMap::new();
    let context = RenderSceneContext {
        groups: std::slice::from_ref(&group),
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key,
        elapsed_ms: 0,
        display_group_target_fps: &target_fps,
        display_group_descriptors: &descriptors,
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
    let mut output = super::super::render_pass::RenderedGroupPassOutput::default();

    let projected = runtime
        .render_scene_contributor_frames(context, &mut sparkleflinger, true, &mut output)
        .expect("projected contributor should render");

    assert!(!projected.layers.is_empty());
    assert!(projected.cpu_replay_complete);
    assert_eq!(
        runtime
            .target_canvases
            .get(&group.id)
            .expect("group target should remain installed")
            .get_pixel(0, 0),
        Rgba::new(255, 0, 0, 255)
    );
}

#[test]
fn projected_composition_matches_rotated_scaled_translated_zone() {
    let mut zone = point_zone("zone_transformed_composition");
    zone.position = NormalizedPosition::new(0.35, 0.6);
    zone.size = NormalizedPosition::new(0.65, 0.45);
    zone.scale = 0.8;
    zone.rotation = FRAC_PI_4;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut group = sample_group(8, 8);
    group.layout.zones = vec![zone];
    group.layout.default_sampling_mode = SamplingMode::Nearest;
    let projection =
        build_group_projection(&group, 8, 8).expect("projection metadata should allocate");
    let mut source = patterned_source_canvas(8, 8);
    source.set_pixel(3, 4, Rgba::new(140, 60, 220, 0));
    let layers = projection_composition_layers_for_group(
        &ProducerFrame::Canvas(source.clone()),
        &group,
        &projection,
        8,
        8,
    )
    .expect("transformed nearest clamp projection should use composition layers");
    let projection_cache = HashMap::from([(group.id, projection)]);
    let target_canvases = HashMap::from([(group.id, source)]);
    let mut expected = Canvas::new(8, 8);
    compose_authoritative_scene_canvas(
        &mut expected,
        std::slice::from_ref(&group),
        &target_canvases,
        8,
        8,
        &projection_cache,
    );
    let mut runtime = ZoneRuntime::new(8, 8);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let actual = runtime
        .compose_projected_scene_frame(layers, &mut sparkleflinger)
        .and_then(ProducerFrame::into_cpu_render_frame)
        .map(|(canvas, _)| canvas)
        .expect("CPU projection should materialize a scene canvas");

    assert_eq!(actual.as_rgba_bytes(), expected.as_rgba_bytes());
    assert_eq!(actual.get_pixel(0, 0), Rgba::BLACK);
    assert_eq!(actual.get_pixel(3, 4), Rgba::new(140, 60, 220, 255));
}

#[test]
fn projected_composition_preserves_zone_and_group_overlap_order() {
    let mut back = sample_group(8, 8);
    back.layout.zones = vec![rotated_zone("back_a", FRAC_PI_4, 0.7)];
    back.layout.zones.push(point_zone_at("back_b", 0.2, 0.2));
    back.layout.default_sampling_mode = SamplingMode::Nearest;
    let mut front = sample_group(8, 8);
    front.layout.zones = vec![point_zone_at("front", 0.5, 0.5)];
    front.layout.zones[0].size = NormalizedPosition::new(0.35, 0.35);
    front.layout.default_sampling_mode = SamplingMode::Nearest;

    let back_projection =
        build_group_projection(&back, 8, 8).expect("back projection metadata should allocate");
    let front_projection =
        build_group_projection(&front, 8, 8).expect("front projection metadata should allocate");
    let mut back_source = Canvas::new(8, 8);
    back_source.fill(Rgba::new(255, 0, 0, 255));
    let mut front_source = Canvas::new(8, 8);
    front_source.fill(Rgba::new(0, 0, 255, 255));
    let mut layers = projection_composition_layers_for_group(
        &ProducerFrame::Canvas(back_source.clone()),
        &back,
        &back_projection,
        8,
        8,
    )
    .expect("back projection should use composition layers");
    layers.extend(
        projection_composition_layers_for_group(
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
    let groups = [back, front];
    let mut expected = Canvas::new(8, 8);
    compose_authoritative_scene_canvas(
        &mut expected,
        &groups,
        &target_canvases,
        8,
        8,
        &projection_cache,
    );

    let mut runtime = ZoneRuntime::new(8, 8);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let actual = runtime
        .compose_projected_scene_frame(layers, &mut sparkleflinger)
        .and_then(ProducerFrame::into_cpu_render_frame)
        .map(|(canvas, _)| canvas)
        .expect("CPU projection should materialize a scene canvas");

    assert_eq!(actual.as_rgba_bytes(), expected.as_rgba_bytes());
    assert_eq!(actual.get_pixel(4, 4), Rgba::new(0, 0, 255, 255));
    assert_eq!(actual.get_pixel(0, 7), Rgba::BLACK);
}

#[test]
fn projected_composition_rejects_bilinear_zones() {
    let mut zone = point_zone("zone_bilinear_projection");
    zone.sampling_mode = Some(SamplingMode::Bilinear);
    let mut group = sample_group(4, 4);
    group.layout.zones = vec![zone];
    let projection =
        build_group_projection(&group, 4, 4).expect("projection metadata should allocate");

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
    let projection =
        build_group_projection(&group, 4, 4).expect("projection metadata should allocate");
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
    let source = patterned_source_canvas(8, 8);
    let Some(gpu_source) = sparkleflinger.upload_canvas_frame(&source) else {
        return;
    };
    let mut zone = point_zone("gpu_transformed_projection");
    zone.position = NormalizedPosition::new(0.35, 0.6);
    zone.size = NormalizedPosition::new(0.65, 0.45);
    zone.scale = 0.8;
    zone.rotation = FRAC_PI_4;
    zone.sampling_mode = Some(SamplingMode::Nearest);
    zone.edge_behavior = Some(EdgeBehavior::Clamp);
    let mut group = sample_group(8, 8);
    group.layout.zones = vec![zone];
    group.layout.default_sampling_mode = SamplingMode::Nearest;
    let projection =
        build_group_projection(&group, 8, 8).expect("projection metadata should allocate");
    let layers = projection_composition_layers_for_group(
        &ProducerFrame::GpuTexture(gpu_source),
        &group,
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
fn failed_gpu_projection_reuses_last_good_scene_across_dependency_change() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let groups = gpu_projection_groups();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zones = Vec::new();
    let Some(first_frame) = sparkleflinger.upload_canvas_frame(&patterned_source_canvas(4, 4))
    else {
        panic!("required GPU test should upload the retained scene")
    };
    let retained_result = ZoneResult {
        scene_frame: ProducerFrame::GpuTexture(first_frame.clone()),
        group_canvases: Vec::new(),
        zone_canvases: Vec::new(),
        active_group_canvas_ids: Vec::new(),
        led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(
            runtime.combined_led_spatial_engine.clone(),
        ),
        producer_full_frame_copy: FullFrameCopyMetrics::default(),
        render_us: 0,
        sample_us: 0,
        scene_compose_us: 0,
        logical_layer_count: 2,
    };
    runtime.retain_frame(
        SceneDependencyKey::new(1, registry.generation()),
        &retained_result,
        &zones,
    );

    let changed_dependency_key = SceneDependencyKey::new(2, registry.generation());
    assert!(runtime.reuse_scene(changed_dependency_key).is_none());
    runtime.fail_next_projected_scene_composition_for_test();
    let retained = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &groups,
        2,
        16,
        &HashMap::new(),
        &registry,
        &mut zones,
        None,
        &mut sparkleflinger,
    )
    .expect("failed projection should reuse the retained scene");
    let ProducerFrame::GpuTexture(retained_frame) = retained.scene_frame else {
        panic!("failed projection should not materialize stale CPU targets")
    };

    assert_eq!(retained_frame.storage_id, first_frame.storage_id);
    assert_eq!(
        retained_frame.content_generation,
        first_frame.content_generation
    );
    assert!(matches!(
        retained.led_sampling_strategy,
        LedSamplingStrategy::SparkleFlinger(_)
    ));
    assert!(runtime.reuse_scene(changed_dependency_key).is_none());
}

#[cfg(feature = "wgpu")]
#[test]
fn first_gpu_projection_failure_returns_typed_error_without_partial_cpu_scene() {
    let Some(mut sparkleflinger) = required_gpu_sparkleflinger() else {
        return;
    };
    let registry = EffectRegistry::default();
    let groups = gpu_projection_groups();
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zones = Vec::new();
    runtime.fail_next_projected_scene_composition_for_test();

    let Err(error) = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &groups,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zones,
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
fn required_gpu_sparkleflinger() -> Option<SparkleFlinger> {
    match SparkleFlinger::new(hypercolor_types::config::RenderAccelerationMode::Gpu) {
        Ok(sparkleflinger) => Some(sparkleflinger),
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
fn gpu_projection_groups() -> [Zone; 2] {
    let mut back = sample_group(4, 4);
    make_color_fill_group(&mut back);
    let mut back_zone = point_zone("back");
    back_zone.size = NormalizedPosition::new(1.0, 1.0);
    back_zone.sampling_mode = Some(SamplingMode::Nearest);
    back_zone.edge_behavior = Some(EdgeBehavior::Clamp);
    back.layout.zones = vec![back_zone];
    back.layout.default_sampling_mode = SamplingMode::Nearest;
    let mut front = sample_group(4, 4);
    make_color_fill_group(&mut front);
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
