use super::*;
use hypercolor_types::layer::SceneLayerId;

fn static_surface(frame: &ProducerFrame) -> &PublishedSurface {
    match frame {
        ProducerFrame::Surface(surface) => surface,
        ProducerFrame::Canvas(_) => panic!("static layer frame should be surface-backed"),
        ProducerFrame::ScreenPublication(_) => {
            panic!("static layer frame should be surface-backed")
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => panic!("static layer frame should not use imported GPU storage"),
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            panic!("static layer frame should not use compositor GPU storage")
        }
    }
}

#[test]
fn static_layer_surfaces_reuse_final_srgba_storage() {
    let mut cache = StaticLayerSurfaceCache::default();
    let rgba = [0.25, 0.5, 0.75, 0.4];

    let first =
        color_fill_frame(&mut cache, 3, 2, rgba).expect("static layer surface should allocate");
    let second =
        color_fill_frame(&mut cache, 3, 2, rgba).expect("static layer surface should allocate");
    let expected = LinearRgba::new(rgba[0], rgba[1], rgba[2], rgba[3]).to_encoded();

    assert_eq!(
        static_surface(&first).storage_identity(),
        static_surface(&second).storage_identity()
    );
    assert_eq!(static_surface(&first).generation(), 1);
    assert_eq!(static_surface(&first).width(), 3);
    assert_eq!(static_surface(&first).height(), 2);
    assert!(
        static_surface(&first)
            .rgba_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [expected.r, expected.g, expected.b, expected.a])
    );
    assert_eq!(cache.entry_count(), 1);
}

#[test]
fn transparent_static_layer_surface_preserves_zero_alpha() {
    let mut cache = StaticLayerSurfaceCache::default();

    let frame = transparent_black_frame(&mut cache, 2, 2)
        .expect("transparent layer surface should allocate");

    assert!(
        static_surface(&frame)
            .rgba_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 0, 0])
    );
}

#[test]
fn static_layer_allocation_failure_does_not_poison_cache() {
    let mut cache = StaticLayerSurfaceCache::default();

    let error = color_fill_frame(&mut cache, u32::MAX, u32::MAX, [1.0, 0.0, 0.0, 1.0])
        .expect_err("overflowed static layer surface should be rejected");

    assert!(error.to_string().contains("overflow"));
    assert_eq!(cache.entry_count(), 0);
}

#[test]
fn static_layer_surface_cache_stays_bounded() {
    let mut cache = StaticLayerSurfaceCache::default();
    let first = color_fill_frame(&mut cache, 1, 1, [1.0, 0.0, 0.0, 1.0])
        .expect("static layer surface should allocate");
    let first_identity = static_surface(&first).storage_identity();

    for width in 2..=33 {
        color_fill_frame(&mut cache, width, 1, [1.0, 0.0, 0.0, 1.0])
            .expect("static layer surface should allocate");
    }

    assert_eq!(cache.entry_count(), 32);
    let recreated = color_fill_frame(&mut cache, 1, 1, [1.0, 0.0, 0.0, 1.0])
        .expect("static layer surface should allocate");
    assert_ne!(
        static_surface(&recreated).storage_identity(),
        first_identity
    );
    assert_eq!(cache.entry_count(), 32);
}

#[test]
fn empty_layer_stack_does_not_forge_a_layer_identity() {
    let mut zone = sample_zone(4, 4);
    zone.layers.clear();

    assert!(passthrough_effect_layer(&zone).is_none());
}

#[test]
fn single_effect_layer_can_passthrough_layer_compositor() {
    let mut zone = sample_zone(4, 4);
    let effect_id = zone
        .effect_ids()
        .next()
        .expect("sample zone should have an effect");
    let layer_id = SceneLayerId::new();
    zone.layers = vec![SceneLayer::from_effect(
        layer_id,
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];

    let layer = passthrough_effect_layer(&zone)
        .expect("neutral materialized effect layer should bypass layer composition");

    assert_eq!(layer.id, layer_id);
}

#[test]
fn stacked_layers_use_layer_compositor() {
    let mut zone = sample_zone(4, 4);
    let effect_id = zone
        .effect_ids()
        .next()
        .expect("sample zone should have an effect");
    let effect_layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    let overlay = SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [1.0, 0.0, 0.0, 1.0],
        },
        blend: BlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    };
    zone.layers = vec![effect_layer, overlay];

    assert!(passthrough_effect_layer(&zone).is_none());
}

#[test]
fn adjusted_effect_layer_uses_layer_compositor() {
    let mut zone = sample_zone(4, 4);
    let effect_id = zone
        .effect_ids()
        .next()
        .expect("sample zone should have an effect");
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.opacity = 0.5;
    zone.layers = vec![layer];

    assert!(passthrough_effect_layer(&zone).is_none());
}

#[test]
fn unavailable_effect_does_not_hide_available_layers_or_change_authored_state() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let mut zone = sample_zone(4, 4);
    let missing_layer = zone.layers[0].clone();
    let solid = effect_layer(
        builtin_effect_id(&registry, "solid_color"),
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    zone.layers = vec![solid, missing_layer.clone()];
    zone.layers[1].blend = BlendMode::Replace;
    let authored = zone.clone();
    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut Vec::new(),
    )
    .expect("available layers render");
    assert_eq!(
        canvas_from_scene_frame(&result.scene_frame).get_pixel(0, 0),
        Rgba::new(255, 0, 0, 255)
    );
    assert_eq!(zone, authored);
    let events = runtime.drain_layer_runtime_events();
    assert!(events.iter().any(|event| matches!(event,
        HypercolorEvent::LayerHealthChanged { layer_id, health: LayerHealth::Failed { reason }, .. }
        if *layer_id == missing_layer.id && reason == "Effect is not installed")));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, HypercolorEvent::EffectError { .. }))
    );
}

#[test]
fn unavailable_effect_clears_stale_frames_and_recovers_without_scene_edits() {
    for opacity in [1.0, 0.5] {
        let mut runtime = ZoneRuntime::new(4, 4);
        let mut registry = builtin_registry();
        let effect_id = builtin_effect_id(&registry, "solid_color");
        let mut zone = sample_zone(4, 4);
        set_effect_zone(
            &mut zone,
            effect_id,
            HashMap::from([(
                "color".into(),
                ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
            )]),
        );
        zone.layers[0].opacity = opacity;
        let authored = zone.clone();
        let initial = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&zone),
            1,
            0,
            &HashMap::new(),
            &registry,
            &mut Vec::new(),
        )
        .expect("initial effect renders");
        assert!(
            canvas_from_scene_frame(&initial.scene_frame)
                .get_pixel(0, 0)
                .a
                > 0
        );
        runtime.drain_layer_runtime_events();
        let entry = registry.remove(&effect_id).expect("registered effect");
        let missing = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&zone),
            1,
            16,
            &HashMap::new(),
            &registry,
            &mut Vec::new(),
        )
        .expect("missing effect stays inactive");
        assert_eq!(
            canvas_from_scene_frame(&missing.scene_frame).get_pixel(0, 0),
            Rgba::TRANSPARENT
        );
        assert!(
            runtime
                .drain_layer_runtime_events()
                .iter()
                .any(|event| matches!(
                    event,
                    HypercolorEvent::LayerHealthChanged {
                        health: LayerHealth::Failed { .. },
                        ..
                    }
                ))
        );
        registry.register(entry);
        let recovered = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&zone),
            1,
            32,
            &HashMap::new(),
            &registry,
            &mut Vec::new(),
        )
        .expect("registry installation recovers effect");
        assert!(
            canvas_from_scene_frame(&recovered.scene_frame)
                .get_pixel(0, 0)
                .a
                > 0
        );
        assert!(
            runtime
                .drain_layer_runtime_events()
                .iter()
                .any(|event| matches!(
                    event,
                    HypercolorEvent::LayerHealthChanged {
                        health: LayerHealth::Active,
                        ..
                    }
                ))
        );
        assert_eq!(zone, authored);
    }
}

#[test]
fn unavailable_effect_on_direct_display_stays_transparent_between_cadence_ticks() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::new(Vec::new());
    let zone = sample_display_zone(4, 4);
    let cadence = HashMap::from([(zone.id, 30)]);
    for elapsed in [0, 1, 16, 34] {
        let result = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&zone),
            1,
            elapsed,
            &cadence,
            &registry,
            &mut Vec::new(),
        )
        .expect("direct missing effect stays inactive");
        let (_, frame) = result.display_zone_frames.first().expect("direct frame");
        assert!(
            frame
                .surface_for_test()
                .rgba_bytes()
                .iter()
                .all(|byte| *byte == 0)
        );
    }
    assert!(matches!(
        runtime.drain_layer_runtime_events().as_slice(),
        [HypercolorEvent::LayerHealthChanged {
            health: LayerHealth::Failed { reason },
            ..
        }] if reason == "Effect is not installed"
    ));
}

#[test]
fn missing_media_does_not_replace_available_layers() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let mut zone = sample_zone(4, 4);
    let solid = effect_layer(
        builtin_effect_id(&registry, "solid_color"),
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([1.0, 0.0, 0.0, 1.0]),
        )]),
    );
    let mut missing = solid.clone();
    missing.id = SceneLayerId::new();
    missing.source = LayerSource::Media {
        asset_id: AssetId::new(),
        playback: MediaPlayback::default(),
    };
    missing.blend = BlendMode::Replace;
    let missing_id = missing.id;
    zone.layers = vec![solid, missing];
    let authored = zone.clone();
    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut Vec::new(),
    )
    .expect("available layer renders beside missing media");
    assert_eq!(
        canvas_from_scene_frame(&result.scene_frame).get_pixel(0, 0),
        Rgba::new(255, 0, 0, 255)
    );
    assert_eq!(zone, authored);
    assert!(
        runtime
            .drain_layer_runtime_events()
            .iter()
            .any(|event| matches!(
                event,
                HypercolorEvent::LayerHealthChanged {
                    layer_id,
                    health: LayerHealth::AssetMissing,
                    ..
                } if *layer_id == missing_id
            ))
    );
    zone.layers.remove(0);
    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&zone),
        2,
        16,
        &HashMap::new(),
        &registry,
        &mut Vec::new(),
    )
    .expect("missing media clears the previous available frame");
    assert_eq!(
        canvas_from_scene_frame(&result.scene_frame).get_pixel(0, 0),
        Rgba::TRANSPARENT
    );
}

#[test]
fn missing_media_layer_renders_transparent_black_and_reports_health() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::new(Vec::new());
    let mut zone = sample_zone(4, 4);
    zone.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Missing Media".into()),
        source: LayerSource::Media {
            asset_id: AssetId::new(),
            playback: MediaPlayback::default(),
        },
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let layer_id = zone.layers[0].id;
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[zone.clone()],
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zones,
    )
    .expect("missing media should not fail scene rendering");
    let canvas = canvas_from_scene_frame(&result.scene_frame);

    assert_eq!(canvas.get_pixel(0, 0), Rgba::TRANSPARENT);
    assert!(matches!(
        runtime.drain_layer_runtime_events().as_slice(),
        [HypercolorEvent::LayerHealthChanged {
            zone_id,
            layer_id: event_layer_id,
            health: LayerHealth::AssetMissing,
            ..
        }] if *zone_id == zone.id && *event_layer_id == layer_id
    ));
}

#[test]
fn screen_region_layer_uses_latest_capture_canvas() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::new(Vec::new());
    let mut zone = sample_display_zone(2, 1);
    zone.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Screen".into()),
        source: LayerSource::ScreenRegion {
            viewport: hypercolor_types::viewport::ViewportRect::full(),
        },
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let screen = hypercolor_core::input::SyntheticScreenPublisher::surface(
        hypercolor_core::input::screen::PixelExtent::new(2, 1).expect("fixture extent"),
    )
    .publish_rgba(&[255, 0, 0, 255, 0, 255, 0, 255]);
    let mut zones = Vec::new();

    let result = render_scene_for_test_with_screen(
        &mut runtime,
        &[zone.clone()],
        1,
        0,
        &HashMap::from([(zone.id, 60)]),
        &registry,
        &mut zones,
        Some(&screen),
    )
    .expect("screen region display zone should render");
    let (_, frame) = result
        .display_zone_frames
        .first()
        .expect("display zone should publish a direct frame");
    let surface = frame.surface_for_test();

    assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(surface.get_pixel(1, 0), Rgba::new(0, 255, 0, 255));
    assert!(matches!(
        runtime.drain_layer_runtime_events().as_slice(),
        [HypercolorEvent::LayerHealthChanged {
            health: LayerHealth::Active,
            ..
        }]
    ));
}

#[test]
fn gif_asset_layer_can_drive_direct_display_zone() {
    let tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let mut library =
        AssetLibrary::open(tempdir.path().join("assets")).expect("asset library should open");
    let upload = library
        .add_bytes(&red_gif_bytes(), AssetUploadOptions::new("red.gif"))
        .expect("GIF upload should be accepted");
    let asset_library = Arc::new(RwLock::new(library));
    let mut runtime = ZoneRuntime::with_asset_library(4, 4, asset_library);
    let registry = EffectRegistry::new(Vec::new());
    let mut zone = sample_display_zone(2, 2);
    zone.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("GIF".into()),
        source: LayerSource::Media {
            asset_id: upload.record.id,
            playback: MediaPlayback::default(),
        },
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[zone.clone()],
        1,
        0,
        &HashMap::from([(zone.id, 60)]),
        &registry,
        &mut zones,
    )
    .expect("GIF media display zone should render");
    let (_, frame) = result
        .display_zone_frames
        .first()
        .expect("display zone should publish a direct frame");
    let surface = frame.surface_for_test();
    let canvas = Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height());

    assert_eq!(canvas.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert!(matches!(
        runtime.drain_layer_runtime_events().as_slice(),
        [HypercolorEvent::LayerHealthChanged {
            health: LayerHealth::Active,
            ..
        }]
    ));
}

#[cfg(feature = "media-video")]
#[test]
fn stream_media_layer_reports_loading_until_first_frame() {
    let tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let mut library =
        AssetLibrary::open(tempdir.path().join("assets")).expect("asset library should open");
    let mut options = AssetUploadOptions::new("camera.stream");
    options.type_hint = Some(AssetTypeHint::Stream);
    let upload = library
        .add_bytes(b"http://1.1.1.1/hypercolor-missing-live.m3u8\n", options)
        .expect("stream URL upload should be accepted");
    let asset_library = Arc::new(RwLock::new(library));
    let mut runtime = ZoneRuntime::with_asset_library(4, 4, asset_library);
    let registry = EffectRegistry::new(Vec::new());
    let mut zone = sample_zone(4, 4);
    zone.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Stream".into()),
        source: LayerSource::Media {
            asset_id: upload.record.id,
            playback: MediaPlayback::default(),
        },
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let layer_id = zone.layers[0].id;
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[zone.clone()],
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zones,
    )
    .expect("stream media layer should not fail scene rendering");
    let canvas = canvas_from_scene_frame(&result.scene_frame);

    assert_eq!(canvas.get_pixel(0, 0), Rgba::TRANSPARENT);
    assert!(matches!(
        runtime.drain_layer_runtime_events().as_slice(),
        [HypercolorEvent::LayerHealthChanged {
            zone_id,
            layer_id: event_layer_id,
            health: LayerHealth::Loading,
            ..
        }] if *zone_id == zone.id && *event_layer_id == layer_id
    ));
}

#[test]
fn note_effect_error_dedupes_until_cleared() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let error = ZoneEffectError {
        effect_id: "effect-1".into(),
        effect_name: "Test Effect".into(),
        zone_id: ZoneId::new(),
        zone_name: "Test Zone".into(),
        error: "boom".into(),
    };

    assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
    assert_eq!(runtime.note_effect_error(&error), None);

    runtime.clear_effect_error();

    assert_eq!(runtime.note_effect_error(&error), Some(error));
}

/// A scene render failure logs once when it starts, again only when its
/// error or outcome changes, and counts every frame it held for in between,
/// even across the zone resources a failed frame releases.
#[test]
fn scene_render_failure_logs_once_per_transition() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let unadmitted = anyhow::anyhow!("CPU scene composition was not admitted");
    let other = anyhow::anyhow!("render-zone resources were not admitted");

    assert!(runtime.note_scene_render_failure(&unadmitted, false));
    for _ in 1..457 {
        assert!(!runtime.note_scene_render_failure(&unadmitted, false));
    }
    assert_eq!(runtime.clear_scene_render_failure(), Some(457));
    assert_eq!(runtime.clear_scene_render_failure(), None);

    assert!(runtime.note_scene_render_failure(&unadmitted, false));
    assert!(
        runtime.note_scene_render_failure(&unadmitted, true),
        "falling back to the retained frame is a new outcome"
    );
    assert!(
        runtime.note_scene_render_failure(&other, true),
        "a different error is a new failure"
    );

    let mut zone = sample_zone(4, 4);
    make_color_fill_zone(&mut zone);
    runtime
        .reconcile(
            std::slice::from_ref(&zone),
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(1, 1),
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
        )
        .expect("color fill zone should reconcile");
    runtime.clear_inactive_zones();
    assert!(runtime.reconciled_dependency_key.is_none());
    assert!(
        !runtime.note_scene_render_failure(&other, true),
        "releasing zone resources must not forget the logged failure"
    );
}

/// A failed frame releases every zone resource but keeps the effect error,
/// so the same error is not new on the next failed frame. A scene that stops
/// rendering zones forgets it, so the next failure is new again.
#[test]
fn effect_error_survives_released_zone_resources_until_forgotten() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zone = sample_zone(4, 4);
    make_color_fill_zone(&mut zone);
    let error = ZoneEffectError {
        effect_id: "effect-1".into(),
        effect_name: "Test Effect".into(),
        zone_id: zone.id,
        zone_name: "Test Zone".into(),
        error: "boom".into(),
    };
    runtime
        .reconcile(
            std::slice::from_ref(&zone),
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(1, 1),
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
        )
        .expect("color fill zone should reconcile");

    assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
    runtime.clear_inactive_zones();
    assert!(runtime.reconciled_dependency_key.is_none());
    assert_eq!(runtime.note_effect_error(&error), None);

    runtime.forget_effect_error();
    assert_eq!(runtime.note_effect_error(&error), Some(error));
}

#[test]
fn recovered_effect_error_is_reported_once_after_clear() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let error = ZoneEffectError {
        effect_id: "effect-1".into(),
        effect_name: "Test Effect".into(),
        zone_id: ZoneId::new(),
        zone_name: "Test Zone".into(),
        error: "boom".into(),
    };

    assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
    runtime.clear_effect_error();

    assert_eq!(runtime.take_recovered_effect_error(), Some(error));
    assert_eq!(runtime.take_recovered_effect_error(), None);
}
