use super::*;

#[test]
fn late_group_sampling_failure_preserves_last_good_zones() {
    use hypercolor_core::spatial::SpatialSamplingCapacity;

    let mut runtime = ZoneRuntime::new(4, 4);
    let mut first_group = sample_group(4, 4);
    first_group.layout.zones = vec![point_zone("first")];
    let mut second_group = sample_group(100, 100);
    second_group.layout.zones = vec![point_zone("second")];
    second_group.layout.zones[0].sampling_mode = Some(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    });

    let first_engine =
        SpatialEngine::try_new(first_group.layout.clone()).expect("first group should prepare");
    let mut constrained_layout = second_group.layout.clone();
    constrained_layout.canvas_width = 1;
    constrained_layout.canvas_height = 1;
    let second_engine = SpatialEngine::try_new_with_sampling_capacity(
        constrained_layout,
        SpatialSamplingCapacity::new(1_024),
    )
    .expect("canonical second-group workspace should fit");

    runtime
        .target_canvases
        .insert(first_group.id, Canvas::new(4, 4));
    runtime
        .target_canvases
        .insert(second_group.id, Canvas::new(100, 100));
    runtime.spatial_engines.insert(first_group.id, first_engine);
    runtime
        .spatial_engines
        .insert(second_group.id, second_engine);
    let last_good = vec![ZoneColors {
        zone_id: "last_good".into(),
        colors: vec![[12, 34, 56]],
    }];
    let mut zones = last_good.clone();

    let result = runtime.sample_scene_group_led_zones(&[first_group, second_group], &mut zones);

    assert!(result.is_err());
    assert_eq!(zones, last_good);
    assert!(runtime.zone_sampling_scratch.is_empty());
}

#[test]
fn single_full_scene_group_renders_directly_into_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let producer_counts_before = crate::render_thread::producer_frame_counts();
    let controls = HashMap::from([(
        "color".into(),
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    )]);
    let group = Zone {
        id: ZoneId::new(),
        name: "Direct".into(),
        description: None,
        layers: vec![effect_layer(solid_id, controls)],
        layout: SpatialLayout {
            id: "direct-group".into(),
            name: "Direct Group".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![point_zone("zone_direct")],
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
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("single group should render");

    let ProducerFrame::Surface(surface) = &result.scene_frame else {
        panic!("single full-size group should render into a surface");
    };
    let LedSamplingStrategy::SparkleFlinger(spatial_engine) = result.led_sampling_strategy.clone()
    else {
        panic!("single full-size group should hand LED sampling to SparkleFlinger");
    };
    let sampled = spatial_engine.sample(&Canvas::from_rgba(
        surface.rgba_bytes(),
        surface.width(),
        surface.height(),
    ));

    assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(result.sample_us, 0);
    assert!(zones.is_empty());
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
    assert!(
        crate::render_thread::producer_frame_counts().cpu_frames
            > producer_counts_before.cpu_frames
    );
    assert_eq!(
        runtime
            .target_canvases
            .get(&group.id)
            .expect("reconcile should provision a group canvas")
            .get_pixel(0, 0),
        Rgba::new(0, 0, 0, 255)
    );
}

#[test]
fn single_full_display_zone_keeps_shared_scene_canvas_blank() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_zone(4, 4);
    group.name = "Display".into();
    set_effect_group(
        &mut group,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("single display zone should render");

    let ProducerFrame::Surface(scene_surface) = result.scene_frame else {
        panic!("single display zone should render into a surface");
    };
    let [(_, group_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a surface-backed direct canvas");
    };

    assert_eq!(result.logical_layer_count, 0);
    assert_eq!(scene_surface.get_pixel(0, 0), Rgba::new(0, 0, 0, 255));
    assert_eq!(
        group_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert!(zones.is_empty());
}

#[test]
fn empty_display_zone_does_not_publish_direct_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let mut group = sample_display_zone(4, 4);
    group.name = "Display Shell".into();
    group.layers.clear();
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("empty display zone should be ignored by direct rendering");

    assert!(result.display_zone_frames.is_empty());
    assert!(result.active_display_zone_ids.is_empty());
    assert_eq!(result.logical_layer_count, 0);
    assert!(zones.is_empty());
}

#[test]
fn full_scene_group_with_display_zone_keeps_display_faces_out_of_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut scene_group = sample_group(4, 4);
    set_effect_group(
        &mut scene_group,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    scene_group.layout.zones = vec![point_zone("zone_preview")];
    let mut display_zone = sample_display_zone(4, 4);
    set_effect_group(
        &mut display_zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    display_zone.layout.zones = vec![point_zone("zone_display")];
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[scene_group.clone(), display_zone.clone()],
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("mixed scene and display zones should render");

    let ProducerFrame::Surface(scene_surface) = &result.scene_frame else {
        panic!("mixed full-scene render should publish a surface-backed scene canvas");
    };
    let [(_, group_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a direct surface");
    };
    let LedSamplingStrategy::SparkleFlinger(spatial_engine) = result.led_sampling_strategy.clone()
    else {
        panic!("single scene group should hand LED sampling to SparkleFlinger");
    };
    let sampled = spatial_engine.sample(&Canvas::from_rgba(
        scene_surface.rgba_bytes(),
        scene_surface.width(),
        scene_surface.height(),
    ));
    let reused = runtime
        .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
        .expect("retained scene should be reusable");
    let LedSamplingStrategy::SparkleFlinger(reused_spatial_engine) = reused.led_sampling_strategy
    else {
        panic!("retained single-scene render should stay SparkleFlinger-owned");
    };

    assert_eq!(scene_surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(
        group_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(result.sample_us, 0);
    assert!(zones.is_empty());
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].zone_id, "zone_preview");
    assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
    let [(_, reused_group_canvas_frame)] = &reused.display_zone_frames[..] else {
        panic!("retained scene should keep direct display canvases");
    };
    assert_eq!(
        reused_group_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(reused_spatial_engine.layout().zones.len(), 1);
    assert_eq!(reused_spatial_engine.layout().zones[0].id, "zone_preview");
}

#[test]
fn multiple_custom_groups_render_distinct_zone_colors() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let left_controls = HashMap::from([(
        "color".into(),
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    )]);
    let right_controls = HashMap::from([(
        "color".into(),
        ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
    )]);
    let groups = vec![
        Zone {
            id: ZoneId::new(),
            name: "Left".into(),
            description: None,
            layers: vec![effect_layer(solid_id, left_controls)],
            layout: SpatialLayout {
                id: "left-group".into(),
                name: "Left Group".into(),
                description: None,
                canvas_width: 4,
                canvas_height: 4,
                zones: vec![point_zone_at("zone_left", 0.25, 0.5)],
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
        },
        Zone {
            id: ZoneId::new(),
            name: "Right".into(),
            description: None,
            layers: vec![effect_layer(solid_id, right_controls)],
            layout: SpatialLayout {
                id: "right-group".into(),
                name: "Right Group".into(),
                description: None,
                canvas_width: 4,
                canvas_height: 4,
                zones: vec![point_zone_at("zone_right", 0.75, 0.5)],
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
        },
    ];
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &groups,
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("multiple groups should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy.clone() else {
        panic!("multi-group LED scenes should use pre-sampled per-group colors");
    };

    assert_eq!(result.logical_layer_count, 2);
    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].zone_id, "zone_left");
    assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zones[1].zone_id, "zone_right");
    assert_eq!(zones[1].colors.first().copied(), Some([0, 0, 255]));
}

#[test]
fn overlapping_custom_groups_sample_each_group_canvas_independently() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut red = sample_group(4, 4);
    red.name = "Red".into();
    set_effect_group(
        &mut red,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    red.layout.zones = vec![point_zone("zone_red")];
    let mut blue = sample_group(4, 4);
    blue.name = "Blue".into();
    set_effect_group(
        &mut blue,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    blue.layout.zones = vec![point_zone("zone_blue")];
    let groups = [red, blue];
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &groups,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zones,
    )
    .expect("overlapping multi-zone scene should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
        panic!("overlapping multi-zone scene should be pre-sampled");
    };

    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].zone_id, "zone_red");
    assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zones[1].zone_id, "zone_blue");
    assert_eq!(zones[1].colors.first().copied(), Some([0, 0, 255]));
}

#[test]
fn overlapping_custom_groups_are_order_independent_for_their_own_zones() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut red = sample_group(4, 4);
    red.name = "Red".into();
    set_effect_group(
        &mut red,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    red.layout.zones = vec![point_zone("zone_red")];
    let mut green = sample_group(4, 4);
    green.name = "Green".into();
    set_effect_group(
        &mut green,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
        )]),
    );
    green.layout.zones = vec![point_zone("zone_green")];
    let mut blue = sample_group(4, 4);
    blue.name = "Blue".into();
    set_effect_group(
        &mut blue,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    blue.layout.zones = vec![point_zone("zone_blue")];

    let forward =
        render_overlapping_groups_for_test(&[red.clone(), green.clone(), blue.clone()], &registry);
    let reversed = render_overlapping_groups_for_test(&[blue, green, red], &registry);

    assert_eq!(
        color_by_zone(&forward, "zone_red"),
        color_by_zone(&reversed, "zone_red")
    );
    assert_eq!(
        color_by_zone(&forward, "zone_green"),
        color_by_zone(&reversed, "zone_green")
    );
    assert_eq!(
        color_by_zone(&forward, "zone_blue"),
        color_by_zone(&reversed, "zone_blue")
    );
    assert_eq!(color_by_zone(&forward, "zone_red"), [255, 0, 0]);
    assert_eq!(color_by_zone(&forward, "zone_green"), [0, 255, 0]);
    assert_eq!(color_by_zone(&forward, "zone_blue"), [0, 0, 255]);
}

#[test]
fn multiple_custom_groups_with_display_zone_exclude_display_faces_from_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_group(4, 4);
    left.name = "Left".into();
    set_effect_group(
        &mut left,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    left.layout.zones = vec![point_zone_at("zone_left", 0.25, 0.5)];
    let mut right = sample_group(4, 4);
    right.name = "Right".into();
    set_effect_group(
        &mut right,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
        )]),
    );
    right.layout.zones = vec![point_zone_at("zone_right", 0.75, 0.5)];
    let mut display = sample_display_zone(4, 4);
    display.name = "Display".into();
    set_effect_group(
        &mut display,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    display.layout.zones = vec![point_zone("zone_display")];
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[left, right, display],
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("mixed scene and display zones should render");
    let [(_, group_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a direct surface");
    };
    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy.clone() else {
        panic!("multi-group scene renders should use pre-sampled LED colors");
    };

    assert_eq!(result.logical_layer_count, 2);
    assert_eq!(
        group_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].zone_id, "zone_left");
    assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zones[1].zone_id, "zone_right");
    assert_eq!(zones[1].colors.first().copied(), Some([0, 255, 0]));
    let reused = runtime
        .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
        .expect("retained multi-group scene should be reusable");
    let LedSamplingStrategy::RetainedPreSampled {
        layout: reused_layout,
        zones: reused_zones,
    } = reused.led_sampling_strategy
    else {
        panic!("retained multi-group scene should keep pre-sampled LED colors");
    };
    assert_eq!(reused_layout.zones.len(), 2);
    assert_eq!(reused_layout.zones[0].id, "zone_left");
    assert_eq!(reused_layout.zones[1].id, "zone_right");
    assert_eq!(reused_zones.len(), 2);
    assert_eq!(reused_zones[0].zone_id, "zone_left");
    assert_eq!(reused_zones[1].zone_id, "zone_right");
}

#[test]
fn multiple_display_zones_publish_surface_backed_direct_canvases() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_display_zone(4, 4);
    left.name = "Left Display".into();
    set_effect_group(
        &mut left,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    left.layout.zones = vec![point_zone("zone_left")];
    let mut right = sample_display_zone(4, 4);
    right.name = "Right Display".into();
    set_effect_group(
        &mut right,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    right.layout.zones = vec![point_zone("zone_right")];
    let groups = vec![left.clone(), right.clone()];
    let mut zones = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &groups,
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display zones should render");

    assert!(runtime.target_canvases.is_empty());
    assert_eq!(result.display_zone_frames.len(), 2);
    assert!(result.display_zone_frames.iter().all(|(_, frame)| {
        frame.surface_for_test().width() > 0 && frame.surface_for_test().height() > 0
    }));
    assert!(zones.is_empty());
    let reused = runtime
        .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
        .expect("display-only scene should keep an empty retained LED layout");
    let LedSamplingStrategy::RetainedPreSampled { layout, zones } = reused.led_sampling_strategy
    else {
        panic!("display-only scene should keep an empty retained LED layout");
    };
    assert_eq!(reused.display_zone_frames.len(), 2);
    assert!(layout.zones.is_empty());
    assert!(zones.is_empty());
}

#[test]
fn zero_zone_scene_groups_keep_empty_presampled_led_strategy() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_group(2, 2);
    left.name = "Left".into();
    set_effect_group(
        &mut left,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    let mut right = sample_group(2, 2);
    right.name = "Right".into();
    set_effect_group(
        &mut right,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
        )]),
    );
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[left, right],
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zones,
    )
    .expect("zero-zone scene groups should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
        panic!("scene groups without LED zones should keep the empty pre-sampled path");
    };
    assert!(layout.zones.is_empty());
    assert!(zones.is_empty());
}
