use super::*;

#[test]
fn legacy_single_effect_group_can_passthrough_layer_compositor() {
    let group = sample_group(4, 4);

    let layer = passthrough_effect_layer(&group)
        .expect("legacy single-effect group should bypass layer composition");

    assert_eq!(layer.id, group.legacy_layer_id());
}

#[test]
fn materialized_single_effect_layer_can_passthrough_layer_compositor() {
    let mut group = sample_group(4, 4);
    let effect_id = group.effect_id.expect("sample group should have an effect");
    group.layers = vec![SceneLayer::from_effect(
        group.legacy_layer_id(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];

    let layer = passthrough_effect_layer(&group)
        .expect("neutral materialized effect layer should bypass layer composition");

    assert_eq!(layer.id, group.legacy_layer_id());
}

#[test]
fn stacked_layers_use_layer_compositor() {
    let mut group = sample_group(4, 4);
    let effect_id = group.effect_id.expect("sample group should have an effect");
    let effect_layer = SceneLayer::from_effect(
        group.legacy_layer_id(),
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
        blend: LayerBlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    };
    group.layers = vec![effect_layer, overlay];

    assert!(passthrough_effect_layer(&group).is_none());
}

#[test]
fn adjusted_effect_layer_uses_layer_compositor() {
    let mut group = sample_group(4, 4);
    let effect_id = group.effect_id.expect("sample group should have an effect");
    let mut layer = SceneLayer::from_effect(
        group.legacy_layer_id(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.opacity = 0.5;
    group.layers = vec![layer];

    assert!(passthrough_effect_layer(&group).is_none());
}

#[test]
fn missing_media_layer_renders_transparent_black_and_reports_health() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::new(Vec::new());
    let mut group = sample_group(4, 4);
    group.effect_id = None;
    group.controls.clear();
    group.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Missing Media".into()),
        source: LayerSource::Media {
            asset_id: AssetId::new(),
            playback: MediaPlayback::default(),
        },
        blend: LayerBlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let layer_id = group.layers[0].id;
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[group.clone()],
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
            group_id,
            layer_id: event_layer_id,
            health: LayerHealth::AssetMissing,
            ..
        }] if *group_id == group.id && *event_layer_id == layer_id
    ));
}

#[test]
fn screen_region_layer_uses_latest_capture_canvas() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = EffectRegistry::new(Vec::new());
    let mut group = sample_display_group(2, 1);
    group.effect_id = None;
    group.controls.clear();
    group.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Screen".into()),
        source: LayerSource::ScreenRegion {
            viewport: hypercolor_types::viewport::ViewportRect::full(),
        },
        blend: LayerBlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let source = Canvas::from_vec(vec![255, 0, 0, 255, 0, 255, 0, 255], 2, 1);
    let screen = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(PublishedSurface::from_canvas(&source, 7, 11)),
        source_width: 2,
        source_height: 1,
    };
    let mut zones = Vec::new();

    let result = render_scene_for_test_with_screen(
        &mut runtime,
        &[group.clone()],
        1,
        0,
        &HashMap::from([(group.id, 60)]),
        &registry,
        &mut zones,
        Some(&screen),
    )
    .expect("screen region display group should render");
    let (_, frame) = result
        .group_canvases
        .first()
        .expect("display group should publish a direct frame");
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
fn gif_asset_layer_can_drive_direct_display_group() {
    let tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let mut library =
        AssetLibrary::open(tempdir.path().join("assets")).expect("asset library should open");
    let upload = library
        .add_bytes(&red_gif_bytes(), AssetUploadOptions::new("red.gif"))
        .expect("GIF upload should be accepted");
    let asset_library = Arc::new(RwLock::new(library));
    let mut runtime = ZoneRuntime::with_asset_library(4, 4, asset_library);
    let registry = EffectRegistry::new(Vec::new());
    let mut group = sample_display_group(2, 2);
    group.effect_id = None;
    group.controls.clear();
    group.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("GIF".into()),
        source: LayerSource::Media {
            asset_id: upload.record.id,
            playback: MediaPlayback::default(),
        },
        blend: LayerBlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[group.clone()],
        1,
        0,
        &HashMap::from([(group.id, 60)]),
        &registry,
        &mut zones,
    )
    .expect("GIF media display group should render");
    let (_, frame) = result
        .group_canvases
        .first()
        .expect("display group should publish a direct frame");
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
    let mut group = sample_group(4, 4);
    group.effect_id = None;
    group.controls.clear();
    group.layers = vec![SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: Some("Stream".into()),
        source: LayerSource::Media {
            asset_id: upload.record.id,
            playback: MediaPlayback::default(),
        },
        blend: LayerBlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
    let layer_id = group.layers[0].id;
    let mut zones = Vec::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[group.clone()],
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
            group_id,
            layer_id: event_layer_id,
            health: LayerHealth::Loading,
            ..
        }] if *group_id == group.id && *event_layer_id == layer_id
    ));
}

#[test]
fn note_effect_error_dedupes_until_cleared() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let error = ZoneEffectError {
        effect_id: "effect-1".into(),
        effect_name: "Test Effect".into(),
        group_id: ZoneId::new(),
        group_name: "Test Group".into(),
        error: "boom".into(),
    };

    assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
    assert_eq!(runtime.note_effect_error(&error), None);

    runtime.clear_effect_error();

    assert_eq!(runtime.note_effect_error(&error), Some(error));
}

#[test]
fn recovered_effect_error_is_reported_once_after_clear() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let error = ZoneEffectError {
        effect_id: "effect-1".into(),
        effect_name: "Test Effect".into(),
        group_id: ZoneId::new(),
        group_name: "Test Group".into(),
        error: "boom".into(),
    };

    assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
    runtime.clear_effect_error();

    assert_eq!(runtime.take_recovered_effect_error(), Some(error));
    assert_eq!(runtime.take_recovered_effect_error(), None);
}
