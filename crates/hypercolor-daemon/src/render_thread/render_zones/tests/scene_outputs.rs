use super::*;

#[test]
fn late_zone_sampling_failure_preserves_last_good_zones() {
    use hypercolor_core::spatial::SpatialSamplingCapacity;

    let mut runtime = ZoneRuntime::new(4, 4);
    let mut first_zone = sample_zone(4, 4);
    first_zone.layout.zones = vec![point_zone("first")];
    let mut second_zone = sample_zone(100, 100);
    second_zone.layout.zones = vec![point_zone("second")];
    second_zone.layout.zones[0].sampling_mode = Some(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    });

    let first_engine =
        SpatialEngine::try_new(first_zone.layout.clone()).expect("first zone should prepare");
    let mut constrained_layout = second_zone.layout.clone();
    constrained_layout.canvas_width = 1;
    constrained_layout.canvas_height = 1;
    let second_engine = SpatialEngine::try_new_with_sampling_capacity(
        constrained_layout,
        SpatialSamplingCapacity::new(1_024),
    )
    .expect("canonical second-zone workspace should fit");

    runtime
        .target_canvases
        .insert(first_zone.id, Canvas::new(4, 4));
    runtime
        .target_canvases
        .insert(second_zone.id, Canvas::new(100, 100));
    runtime.spatial_engines.insert(first_zone.id, first_engine);
    runtime
        .spatial_engines
        .insert(second_zone.id, second_engine);
    let last_good = vec![ZoneColors {
        zone_id: "last_good".into(),
        colors: vec![[12, 34, 56]],
    }];
    let mut zone_colors = last_good.clone();

    let result = runtime.sample_scene_zone_led_zones(&[first_zone, second_zone], &mut zone_colors);

    assert!(result.is_err());
    assert_eq!(zone_colors, last_good);
    assert!(runtime.zone_sampling_scratch.is_empty());
}

#[test]
fn single_full_scene_zone_renders_directly_into_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let producer_counts_before = crate::render_thread::producer_frame_counts();
    let controls = HashMap::from([(
        "color".into(),
        ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
    )]);
    let zone = Zone {
        id: ZoneId::new(),
        name: "Direct".into(),
        description: None,
        layers: vec![effect_layer(solid_id, controls)],
        layout: SpatialLayout {
            id: "direct-zone".into(),
            name: "Direct Zone".into(),
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
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("single zone should render");

    let ProducerFrame::Surface(surface) = &result.scene_frame else {
        panic!("single full-size zone should render into a surface");
    };
    let LedSamplingStrategy::SparkleFlinger(spatial_engine) = result.led_sampling_strategy.clone()
    else {
        panic!("single full-size zone should hand LED sampling to SparkleFlinger");
    };
    let sampled = spatial_engine.sample(&Canvas::from_rgba(
        surface.rgba_bytes(),
        surface.width(),
        surface.height(),
    ));

    assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(result.sample_us, 0);
    assert!(zone_colors.is_empty());
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
    assert!(
        crate::render_thread::producer_frame_counts().cpu_frames
            > producer_counts_before.cpu_frames
    );
    assert_eq!(
        runtime
            .target_canvases
            .get(&zone.id)
            .expect("reconcile should provision a zone canvas")
            .get_pixel(0, 0),
        Rgba::new(0, 0, 0, 255)
    );
}

#[test]
fn single_full_display_zone_keeps_shared_scene_canvas_blank() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut zone = sample_display_zone(4, 4);
    zone.name = "Display".into();
    set_effect_zone(
        &mut zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("single display zone should render");

    let ProducerFrame::Surface(scene_surface) = result.scene_frame else {
        panic!("single display zone should render into a surface");
    };
    let [(_, zone_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a surface-backed direct canvas");
    };

    assert_eq!(result.logical_layer_count, 0);
    assert_eq!(scene_surface.get_pixel(0, 0), Rgba::new(0, 0, 0, 255));
    assert_eq!(
        zone_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert!(zone_colors.is_empty());
}

#[test]
fn empty_display_zone_does_not_publish_direct_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let mut zone = sample_display_zone(4, 4);
    zone.name = "Display Shell".into();
    zone.layers.clear();
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("empty display zone should be ignored by direct rendering");

    assert!(result.display_zone_frames.is_empty());
    assert!(result.active_display_zone_ids.is_empty());
    assert_eq!(result.logical_layer_count, 0);
    assert!(zone_colors.is_empty());
}

#[test]
fn full_scene_zone_with_display_zone_keeps_display_faces_out_of_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut scene_zone = sample_zone(4, 4);
    set_effect_zone(
        &mut scene_zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    scene_zone.layout.zones = vec![point_zone("zone_preview")];
    let mut display_zone = sample_display_zone(4, 4);
    set_effect_zone(
        &mut display_zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    display_zone.layout.zones = vec![point_zone("zone_display")];
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[scene_zone.clone(), display_zone.clone()],
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("mixed scene and display zones should render");

    let ProducerFrame::Surface(scene_surface) = &result.scene_frame else {
        panic!("mixed full-scene render should publish a surface-backed scene canvas");
    };
    let [(_, zone_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a direct surface");
    };
    let LedSamplingStrategy::SparkleFlinger(spatial_engine) = result.led_sampling_strategy.clone()
    else {
        panic!("single scene zone should hand LED sampling to SparkleFlinger");
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
        zone_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(result.sample_us, 0);
    assert!(zone_colors.is_empty());
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].zone_id, "zone_preview");
    assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
    let [(_, reused_zone_canvas_frame)] = &reused.display_zone_frames[..] else {
        panic!("retained scene should keep direct display canvases");
    };
    assert_eq!(
        reused_zone_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(reused_spatial_engine.layout().zones.len(), 1);
    assert_eq!(reused_spatial_engine.layout().zones[0].id, "zone_preview");
}

#[test]
fn multiple_custom_zones_render_distinct_zone_colors() {
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
    let zones = vec![
        Zone {
            id: ZoneId::new(),
            name: "Left".into(),
            description: None,
            layers: vec![effect_layer(solid_id, left_controls)],
            layout: SpatialLayout {
                id: "left-zone".into(),
                name: "Left Zone".into(),
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
                id: "right-zone".into(),
                name: "Right Zone".into(),
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
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &zones,
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("multiple zones should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy.clone() else {
        panic!("multi-zone LED scenes should use pre-sampled per-zone colors");
    };

    assert_eq!(result.logical_layer_count, 2);
    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zone_colors.len(), 2);
    assert_eq!(zone_colors[0].zone_id, "zone_left");
    assert_eq!(zone_colors[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zone_colors[1].zone_id, "zone_right");
    assert_eq!(zone_colors[1].colors.first().copied(), Some([0, 0, 255]));
}

#[test]
fn overlapping_custom_zones_sample_each_zone_canvas_independently() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut red = sample_zone(4, 4);
    red.name = "Red".into();
    set_effect_zone(
        &mut red,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    red.layout.zones = vec![point_zone("zone_red")];
    let mut blue = sample_zone(4, 4);
    blue.name = "Blue".into();
    set_effect_zone(
        &mut blue,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    blue.layout.zones = vec![point_zone("zone_blue")];
    let zones = [red, blue];
    let mut zone_colors = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &zones,
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
    )
    .expect("overlapping multi-zone scene should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
        panic!("overlapping multi-zone scene should be pre-sampled");
    };

    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zone_colors.len(), 2);
    assert_eq!(zone_colors[0].zone_id, "zone_red");
    assert_eq!(zone_colors[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zone_colors[1].zone_id, "zone_blue");
    assert_eq!(zone_colors[1].colors.first().copied(), Some([0, 0, 255]));
}

#[test]
fn overlapping_custom_zones_are_order_independent_for_their_own_zones() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut red = sample_zone(4, 4);
    red.name = "Red".into();
    set_effect_zone(
        &mut red,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    red.layout.zones = vec![point_zone("zone_red")];
    let mut green = sample_zone(4, 4);
    green.name = "Green".into();
    set_effect_zone(
        &mut green,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
        )]),
    );
    green.layout.zones = vec![point_zone("zone_green")];
    let mut blue = sample_zone(4, 4);
    blue.name = "Blue".into();
    set_effect_zone(
        &mut blue,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    blue.layout.zones = vec![point_zone("zone_blue")];

    let forward =
        render_overlapping_zones_for_test(&[red.clone(), green.clone(), blue.clone()], &registry);
    let reversed = render_overlapping_zones_for_test(&[blue, green, red], &registry);

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
fn multiple_custom_zones_with_display_zone_exclude_display_faces_from_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_zone(4, 4);
    left.name = "Left".into();
    set_effect_zone(
        &mut left,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    left.layout.zones = vec![point_zone_at("zone_left", 0.25, 0.5)];
    let mut right = sample_zone(4, 4);
    right.name = "Right".into();
    set_effect_zone(
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
    set_effect_zone(
        &mut display,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    display.layout.zones = vec![point_zone("zone_display")];
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[left, right, display],
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("mixed scene and display zones should render");
    let [(_, zone_canvas_frame)] = &result.display_zone_frames[..] else {
        panic!("display zone should publish a direct surface");
    };
    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy.clone() else {
        panic!("multi-zone scene renders should use pre-sampled LED colors");
    };

    assert_eq!(result.logical_layer_count, 2);
    assert_eq!(
        zone_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(layout.zones.len(), 2);
    assert_eq!(zone_colors.len(), 2);
    assert_eq!(zone_colors[0].zone_id, "zone_left");
    assert_eq!(zone_colors[0].colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(zone_colors[1].zone_id, "zone_right");
    assert_eq!(zone_colors[1].colors.first().copied(), Some([0, 255, 0]));
    let reused = runtime
        .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
        .expect("retained multi-zone scene should be reusable");
    let LedSamplingStrategy::RetainedPreSampled {
        layout: reused_layout,
        zones: reused_zones,
    } = reused.led_sampling_strategy
    else {
        panic!("retained multi-zone scene should keep pre-sampled LED colors");
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
    set_effect_zone(
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
    set_effect_zone(
        &mut right,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 0.0, 1.0, 1.0]),
        )]),
    );
    right.layout.zones = vec![point_zone("zone_right")];
    let zones = vec![left.clone(), right.clone()];
    let mut zone_colors = Vec::new();
    let display_zone_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &zones,
        1,
        0,
        &display_zone_target_fps,
        &registry,
        &mut zone_colors,
    )
    .expect("display zones should render");

    assert!(runtime.target_canvases.is_empty());
    assert_eq!(result.display_zone_frames.len(), 2);
    assert!(result.display_zone_frames.iter().all(|(_, frame)| {
        frame.surface_for_test().width() > 0 && frame.surface_for_test().height() > 0
    }));
    assert!(zone_colors.is_empty());
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
fn zero_zone_scene_zones_keep_empty_presampled_led_strategy() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_zone(2, 2);
    left.name = "Left".into();
    set_effect_zone(
        &mut left,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    let mut right = sample_zone(2, 2);
    right.name = "Right".into();
    set_effect_zone(
        &mut right,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([0.0, 1.0, 0.0, 1.0]),
        )]),
    );
    let mut zone_colors = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[left, right],
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
    )
    .expect("zero-zone scene zones should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
        panic!("scene zones without LED zones should keep the empty pre-sampled path");
    };
    assert!(layout.zones.is_empty());
    assert!(zone_colors.is_empty());
}
