use std::collections::HashMap;
use std::f32::consts::FRAC_PI_4;

use gif::{Encoder, Frame, Repeat};
#[cfg(feature = "media-video")]
use hypercolor_core::asset::AssetTypeHint;
use hypercolor_core::asset::{AssetLibrary, AssetUploadOptions};
use hypercolor_core::bus::DisplayGroupViewport;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::input::InteractionData;
use hypercolor_types::asset::AssetId;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Rgba;
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::layer::MediaPlayback;
use hypercolor_types::scene::{SceneId, ZoneRole};
use hypercolor_types::spatial::{Corner, LedTopology, NormalizedPosition, Output, StripDirection};
use uuid::Uuid;

use super::projection::{build_group_projection, compose_authoritative_scene_canvas};
use crate::render_thread::sparkleflinger::CompositionLayer;

use super::*;

#[cfg(feature = "wgpu")]
#[test]
fn media_mime_prefers_gpu_texture_for_video_and_streams() {
    assert!(media_mime_prefers_gpu_texture("video/mp4"));
    assert!(media_mime_prefers_gpu_texture("video/webm"));
    assert!(media_mime_prefers_gpu_texture(
        "application/vnd.hypercolor.stream-url"
    ));
    assert!(!media_mime_prefers_gpu_texture("image/png"));
    assert!(!media_mime_prefers_gpu_texture("application/json"));
}

#[test]
fn generation_zero_surface_materialization_records_full_frame_copy() {
    let mut pool = RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(2, 2), 1);
    let surface = PublishedSurface::from_owned_canvas(Canvas::new(2, 2), 7, 42);
    let mut full_frame_copy = FullFrameCopyMetrics::default();

    let backed = surface_backed_frame(
        &mut pool,
        ProducerFrame::Surface(surface),
        &mut full_frame_copy,
    )
    .expect("surface should be materialized into the pool");

    assert!(matches!(backed, ProducerFrame::Surface(_)));
    assert_eq!(full_frame_copy.count, 1);
    assert_eq!(full_frame_copy.bytes, 16);
    assert_eq!(
        full_frame_copy.reason,
        Some("generation_zero_surface_pool_materialization")
    );
}

fn sample_group(width: u32, height: u32) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: "Preview Group".into(),
        description: None,
        effect_id: Some(EffectId::from(Uuid::now_v7())),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: SpatialLayout {
            id: "preview-group".into(),
            name: "Preview Group".into(),
            description: None,
            canvas_width: width,
            canvas_height: height,
            zones: Vec::new(),
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
    }
}

fn patterned_source_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            canvas.set_pixel(
                x,
                y,
                Rgba::new((x * 40) as u8, (y * 50) as u8, ((x + y) * 30) as u8, 255),
            );
        }
    }
    canvas
}

fn sample_display_group(width: u32, height: u32) -> Zone {
    let mut group = sample_group(width, height);
    group.display_target = Some(hypercolor_types::scene::DisplayFaceTarget {
        device_id: hypercolor_types::device::DeviceId::new(),
        blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Replace,
        opacity: 1.0,
    });
    group.role = ZoneRole::Display;
    group
}

fn sample_group_canvas_frame(
    display_target: &DisplayFaceTarget,
    finalized: bool,
) -> GroupCanvasFrame {
    GroupCanvasFrame {
        frame: DisplayGroupFrame::empty(),
        display_target: DisplayGroupTarget {
            device_id: display_target.device_id,
            blend_mode: display_target.blend_mode,
            opacity: display_target.opacity,
            finalized,
        },
    }
}

fn sample_display_route(device_id: hypercolor_types::device::DeviceId) -> DisplayGroupOutputRoute {
    DisplayGroupOutputRoute {
        device_id,
        width: 480,
        height: 480,
        circular: true,
        brightness: 1.0,
        frame_format: DisplayFrameFormat::Jpeg,
        viewport: DisplayGroupViewport {
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 1.0, y: 1.0 },
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Clamp,
        },
    }
}

fn point_zone(id: &str) -> Output {
    Output {
        id: id.into(),
        name: id.into(),
        device_id: id.into(),
        zone_name: None,
        position: NormalizedPosition { x: 0.5, y: 0.5 },
        size: NormalizedPosition { x: 0.2, y: 0.2 },
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 1,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

fn rotated_zone(id: &str, rotation: f32, size: f32) -> Output {
    let mut zone = point_zone(id);
    zone.size = NormalizedPosition { x: size, y: size };
    zone.rotation = rotation;
    zone
}

fn point_zone_at(id: &str, x: f32, y: f32) -> Output {
    let mut zone = point_zone(id);
    zone.position = NormalizedPosition::new(x, y);
    zone.size = NormalizedPosition::new(0.4, 0.4);
    zone
}

fn builtin_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
    registry
        .iter()
        .find_map(|(id, entry)| (entry.metadata.source.source_stem() == Some(stem)).then_some(*id))
        .expect("builtin effect should exist")
}

fn builtin_entry(registry: &EffectRegistry, stem: &str) -> hypercolor_core::effect::EffectEntry {
    registry
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.clone())
        })
        .expect("builtin effect should exist")
}

fn render_scene_for_test(
    runtime: &mut ZoneRuntime,
    groups: &[Zone],
    groups_revision: u64,
    elapsed_ms: u32,
    display_group_target_fps: &HashMap<ZoneId, u32>,
    registry: &EffectRegistry,
    zones: &mut Vec<ZoneColors>,
) -> Result<ZoneResult> {
    render_scene_for_test_with_screen(
        runtime,
        groups,
        groups_revision,
        elapsed_ms,
        display_group_target_fps,
        registry,
        zones,
        None,
    )
}

fn render_overlapping_groups_for_test(
    groups: &[Zone],
    registry: &EffectRegistry,
) -> Vec<ZoneColors> {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut zones = Vec::new();
    let result = render_scene_for_test(
        &mut runtime,
        groups,
        1,
        0,
        &HashMap::new(),
        registry,
        &mut zones,
    )
    .expect("overlapping multi-zone scene should render");

    let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
        panic!("overlapping multi-zone scene should be pre-sampled");
    };
    assert_eq!(layout.zones.len(), groups.len());
    assert_eq!(zones.len(), groups.len());
    zones
}

fn color_by_zone(zones: &[ZoneColors], zone_id: &str) -> [u8; 3] {
    zones
        .iter()
        .find(|zone| zone.zone_id == zone_id)
        .and_then(|zone| zone.colors.first().copied())
        .expect("zone color should be sampled")
}

fn render_scene_for_test_with_screen(
    runtime: &mut ZoneRuntime,
    groups: &[Zone],
    groups_revision: u64,
    elapsed_ms: u32,
    display_group_target_fps: &HashMap<ZoneId, u32>,
    registry: &EffectRegistry,
    zones: &mut Vec<ZoneColors>,
    screen: Option<&ScreenData>,
) -> Result<ZoneResult> {
    let mut sparkleflinger = SparkleFlinger::cpu();
    let inputs = ZoneFrameInputs {
        delta_secs: 1.0 / 60.0,
        audio: &AudioData::silence(),
        interaction: &InteractionData::default(),
        screen,
        sensors: &SystemSnapshot::empty(),
    };
    let context = RenderSceneContext {
        groups,
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key: SceneDependencyKey::new(groups_revision, registry.generation()),
        elapsed_ms,
        display_group_target_fps,
        registry,
        inputs,
    };
    runtime.render_scene(context, &mut sparkleflinger, zones)
}

fn blit_general_zone_projection(
    target: &mut Canvas,
    source: &Canvas,
    zone: &Output,
    sampling_mode: &SamplingMode,
    edge_behavior: EdgeBehavior,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    target_width: u32,
    target_height: u32,
) {
    for y in y0..y1 {
        for x in x0..x1 {
            let Some(local_position) =
                zone_local_position_for_scene_pixel(x, y, target_width, target_height, zone)
            else {
                continue;
            };
            target.set_pixel(
                x,
                y,
                sample_led(source, local_position, zone, sampling_mode, edge_behavior),
            );
        }
    }
}

fn canvas_from_scene_frame(frame: &ProducerFrame) -> Canvas {
    match frame {
        ProducerFrame::Canvas(canvas) => canvas.clone(),
        ProducerFrame::Surface(surface) => {
            Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height())
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => {
            panic!("GPU scene frames are sampled by SparkleFlinger before CPU materialization")
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            panic!("GPU scene frames are sampled by SparkleFlinger before CPU materialization")
        }
    }
}

fn red_gif_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder =
            Encoder::new(&mut bytes, 1, 1, &[]).expect("test GIF encoder should initialize");
        encoder
            .set_repeat(Repeat::Infinite)
            .expect("test GIF repeat should set");
        let mut pixels = vec![255, 0, 0, 255];
        let mut frame = Frame::from_rgba_speed(1, 1, &mut pixels, 10);
        frame.delay = 10;
        encoder
            .write_frame(&frame)
            .expect("test GIF frame should encode");
    }
    bytes
}

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

#[test]
fn clear_inactive_groups_releases_cached_group_state() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_group(4, 4);
    let display_group = sample_display_group(4, 4);
    let display_target = display_group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);
    runtime.target_canvases.insert(group.id, Canvas::new(4, 4));
    runtime
        .spatial_engines
        .insert(group.id, SpatialEngine::new(group.layout.clone()));
    runtime.retain_materialized_group_frame(
        display_group.id,
        100,
        SceneDependencyKey::new(1, 1),
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );
    runtime.reconciled_dependency_key = Some(SceneDependencyKey::new(1, 1));

    assert!(runtime.has_inactive_group_resources());

    runtime.clear_inactive_groups();

    assert!(!runtime.has_inactive_group_resources());
    assert!(runtime.combined_led_layout.zones.is_empty());
}

#[test]
fn materialized_group_reuse_obeys_cadence_and_route_identity() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);

    runtime.retain_materialized_group_frame(
        group.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );

    let reused = runtime
        .reuse_retained_materialized_group_frame(
            group.id,
            120,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
        )
        .expect("retained materialized frame should be reused within cadence");
    assert_eq!(reused.display_target, group_canvas_frame.display_target);

    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                SceneDependencyKey::new(2, 1),
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                140,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let mut changed_route = display_route.clone();
    changed_route.width += 1;
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                dependency_key,
                &display_target,
                &changed_route,
                false,
            )
            .is_none()
    );

    let mut changed_target = display_target.clone();
    changed_target.opacity = 0.5;
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                Some(30),
                dependency_key,
                &changed_target,
                &display_route,
                false,
            )
            .is_none()
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                120,
                None,
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let unfinalized_group = sample_display_group(4, 4);
    let unfinalized_target = unfinalized_group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let unfinalized_route = sample_display_route(unfinalized_target.device_id);
    let unfinalized_frame = sample_group_canvas_frame(&unfinalized_target, false);
    runtime.retain_materialized_group_frame(
        unfinalized_group.id,
        100,
        dependency_key,
        &unfinalized_target,
        &unfinalized_route,
        false,
        &unfinalized_frame,
    );
    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                unfinalized_group.id,
                120,
                Some(30),
                dependency_key,
                &unfinalized_target,
                &unfinalized_route,
                false,
            )
            .is_none()
    );
}

#[test]
fn latest_direct_group_reuse_keeps_display_face_visible_across_dependency_change() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let retained = PendingGroupCanvasFrame {
        frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
        display_target: display_target.clone(),
        empty_direct_shell: false,
    };

    runtime.retain_direct_group_frame(group.id, 100, SceneDependencyKey::new(1, 1), &retained);

    let reused = runtime
        .reuse_latest_direct_group_frame(&group)
        .expect("pending display face should reuse the previous direct frame");
    assert_eq!(reused.display_target, display_target);

    let mut changed_target = group.clone();
    changed_target
        .display_target
        .as_mut()
        .expect("display group should have a target")
        .opacity = 0.5;
    assert!(
        runtime
            .reuse_latest_direct_group_frame(&changed_target)
            .is_none()
    );

    let mut changed_size = group;
    changed_size.layout.canvas_width += 1;
    assert!(
        runtime
            .reuse_latest_direct_group_frame(&changed_size)
            .is_none()
    );
}

#[test]
fn latest_materialized_group_reuse_ignores_cadence_for_missed_frames() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let group = sample_display_group(4, 4);
    let display_target = group
        .display_target
        .as_ref()
        .expect("display group should have a target")
        .clone();
    let display_route = sample_display_route(display_target.device_id);
    let dependency_key = SceneDependencyKey::new(1, 1);
    let group_canvas_frame = sample_group_canvas_frame(&display_target, true);

    runtime.retain_materialized_group_frame(
        group.id,
        100,
        dependency_key,
        &display_target,
        &display_route,
        false,
        &group_canvas_frame,
    );

    assert!(
        runtime
            .reuse_retained_materialized_group_frame(
                group.id,
                140,
                Some(30),
                dependency_key,
                &display_target,
                &display_route,
                false,
            )
            .is_none()
    );

    let reused = runtime
        .reuse_latest_materialized_group_frame(group.id, &display_target, &display_route, false)
        .expect("latest materialized frame should latch when a fresh frame misses");
    assert_eq!(reused.display_target, group_canvas_frame.display_target);

    let mut changed_route = display_route.clone();
    changed_route.width += 1;
    assert!(
            runtime
                .reuse_latest_materialized_group_frame(
                    group.id,
                    &display_target,
                    &changed_route,
                    false,
                )
                .is_none()
        );

    let mut changed_target = display_target.clone();
    changed_target.opacity = 0.5;
    assert!(
            runtime
                .reuse_latest_materialized_group_frame(
                    group.id,
                    &changed_target,
                    &display_route,
                    false,
                )
                .is_none()
        );
}

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

#[test]
fn single_full_scene_group_renders_directly_into_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let producer_counts_before = crate::render_thread::producer_frame_counts();
    let group = Zone {
        id: ZoneId::new(),
        name: "Direct".into(),
        description: None,
        effect_id: Some(solid_id),
        controls: HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
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
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
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
fn single_full_display_group_keeps_shared_scene_canvas_blank() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.name = "Display".into();
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
    let mut zones = Vec::new();
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("single display group should render");

    let ProducerFrame::Surface(scene_surface) = result.scene_frame else {
        panic!("single display group should render into a surface");
    };
    let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
        panic!("display group should publish a surface-backed direct canvas");
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
fn empty_display_group_publishes_transparent_direct_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let mut group = sample_display_group(4, 4);
    group.name = "Display Shell".into();
    group.effect_id = None;
    group.controls.clear();
    group.layers.clear();
    let mut zones = Vec::new();
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("empty display group should render a stable direct surface");

    let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
        panic!("empty display group should publish a direct surface");
    };

    assert_eq!(result.active_group_canvas_ids, vec![group.id]);
    assert_eq!(result.logical_layer_count, 0);
    assert_eq!(
        group_canvas_frame.surface_for_test().get_pixel(0, 0),
        Rgba::TRANSPARENT
    );
    assert!(zones.is_empty());
}

#[test]
fn full_scene_group_with_display_group_keeps_display_faces_out_of_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut scene_group = sample_group(4, 4);
    scene_group.effect_id = Some(solid_id);
    scene_group.controls =
        HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    scene_group.layout.zones = vec![point_zone("zone_preview")];
    let mut display_group = sample_display_group(4, 4);
    display_group.effect_id = Some(solid_id);
    display_group.controls =
        HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
    display_group.layout.zones = vec![point_zone("zone_display")];
    let mut zones = Vec::new();
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[scene_group.clone(), display_group.clone()],
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("mixed scene and display groups should render");

    let ProducerFrame::Surface(scene_surface) = &result.scene_frame else {
        panic!("mixed full-scene render should publish a surface-backed scene canvas");
    };
    let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
        panic!("display group should publish a direct surface");
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
    let [(_, reused_group_canvas_frame)] = &reused.group_canvases[..] else {
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
    let groups = vec![
        Zone {
            id: ZoneId::new(),
            name: "Left".into(),
            description: None,
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            control_bindings: HashMap::new(),
            preset_id: None,
            layers: Vec::new(),
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
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
            control_bindings: HashMap::new(),
            preset_id: None,
            layers: Vec::new(),
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
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &groups,
        1,
        0,
        &display_group_target_fps,
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
    red.effect_id = Some(solid_id);
    red.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    red.layout.zones = vec![point_zone("zone_red")];
    let mut blue = sample_group(4, 4);
    blue.name = "Blue".into();
    blue.effect_id = Some(solid_id);
    blue.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
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
    red.effect_id = Some(solid_id);
    red.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    red.layout.zones = vec![point_zone("zone_red")];
    let mut green = sample_group(4, 4);
    green.name = "Green".into();
    green.effect_id = Some(solid_id);
    green.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    green.layout.zones = vec![point_zone("zone_green")];
    let mut blue = sample_group(4, 4);
    blue.name = "Blue".into();
    blue.effect_id = Some(solid_id);
    blue.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
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
fn multiple_custom_groups_with_display_group_exclude_display_faces_from_led_sampling() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_group(4, 4);
    left.name = "Left".into();
    left.effect_id = Some(solid_id);
    left.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    left.layout.zones = vec![point_zone_at("zone_left", 0.25, 0.5)];
    let mut right = sample_group(4, 4);
    right.name = "Right".into();
    right.effect_id = Some(solid_id);
    right.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    right.layout.zones = vec![point_zone_at("zone_right", 0.75, 0.5)];
    let mut display = sample_display_group(4, 4);
    display.name = "Display".into();
    display.effect_id = Some(solid_id);
    display.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
    display.layout.zones = vec![point_zone("zone_display")];
    let mut zones = Vec::new();
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &[left, right, display],
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("mixed scene and display groups should render");
    let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
        panic!("display group should publish a direct surface");
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
fn multiple_display_groups_publish_surface_backed_direct_canvases() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut left = sample_display_group(4, 4);
    left.name = "Left Display".into();
    left.effect_id = Some(solid_id);
    left.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    left.layout.zones = vec![point_zone("zone_left")];
    let mut right = sample_display_group(4, 4);
    right.name = "Right Display".into();
    right.effect_id = Some(solid_id);
    right.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
    right.layout.zones = vec![point_zone("zone_right")];
    let groups = vec![left.clone(), right.clone()];
    let mut zones = Vec::new();
    let display_group_target_fps = HashMap::new();

    let result = render_scene_for_test(
        &mut runtime,
        &groups,
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display groups should render");

    assert!(runtime.target_canvases.is_empty());
    assert_eq!(result.group_canvases.len(), 2);
    assert!(result.group_canvases.iter().all(|(_, frame)| {
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
    assert_eq!(reused.group_canvases.len(), 2);
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
    left.effect_id = Some(solid_id);
    left.controls = HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
    let mut right = sample_group(2, 2);
    right.name = "Right".into();
    right.effect_id = Some(solid_id);
    right.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
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

#[test]
fn retained_scene_invalidates_when_registry_generation_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut replacement = builtin_entry(&registry, "rainbow");
    replacement.metadata.id = solid_id;
    let mut group = sample_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::new();
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("single group should render");
    let ProducerFrame::Surface(first_surface) = &first.scene_frame else {
        panic!("single group should publish a surface-backed scene frame");
    };

    assert!(
        runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .is_some(),
        "retained scene should be reusable before the registry changes"
    );

    registry.register(replacement);

    assert!(
        runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .is_none(),
        "registry generation changes should invalidate retained scene reuse"
    );

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        1,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("registry generation change should force a rerender");
    let ProducerFrame::Surface(second_surface) = &second.scene_frame else {
        panic!("single group should keep publishing a surface-backed scene frame");
    };

    assert_ne!(
        second_surface.get_pixel(0, 0),
        first_surface.get_pixel(0, 0),
        "same group revision should still rebuild when the registry entry changes"
    );
}

#[test]
fn retained_direct_canvas_invalidates_when_registry_generation_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let mut registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut replacement = builtin_entry(&registry, "rainbow");
    replacement.metadata.id = solid_id;
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    registry.register(replacement);

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("registry generation change should bypass retained direct-canvas reuse");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert_ne!(
        second_frame.surface_for_test().get_pixel(0, 0),
        first_frame.surface_for_test().get_pixel(0, 0),
        "direct canvases should rerender immediately when the active registry entry changes"
    );
    assert!(
        second_frame.surface_for_test().storage_identity()
            != first_frame.surface_for_test().storage_identity()
            || second_frame.surface_for_test().generation()
                != first_frame.surface_for_test().generation(),
        "the retained direct surface should not be reused across registry generations"
    );
}

#[test]
fn retained_direct_canvas_invalidates_when_groups_revision_changes() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        2,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("group revision change should bypass retained direct-canvas reuse");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert!(
        second_frame.surface_for_test().storage_identity()
            != first_frame.surface_for_test().storage_identity()
            || second_frame.surface_for_test().generation()
                != first_frame.surface_for_test().generation(),
        "the retained direct surface should not be reused across group revisions"
    );
}

#[test]
fn empty_display_group_does_not_reuse_previous_face_surface() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render the assigned face");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };
    assert_eq!(
        first_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );

    group.effect_id = None;
    group.controls.clear();
    group.control_bindings.clear();
    group.preset_id = None;
    group.layers.clear();

    let cleared = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("empty display group should render a transparent shell");
    let [(_, cleared_frame)] = &cleared.group_canvases[..] else {
        panic!("empty display group should still publish a direct surface");
    };

    assert!(cleared_frame.empty_direct_shell);
    assert_eq!(
        cleared_frame.surface_for_test().get_pixel(0, 0),
        Rgba::TRANSPARENT
    );
}

#[test]
fn zero_zone_display_group_reuses_retained_surface_until_target_interval() {
    let mut runtime = ZoneRuntime::new(4, 4);
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut group = sample_display_group(4, 4);
    group.effect_id = Some(solid_id);
    group.controls = HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
    let display_group_target_fps = HashMap::from([(group.id, 30)]);
    let mut zones = Vec::new();

    let first = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        0,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should render");
    let [(_, first_frame)] = &first.group_canvases[..] else {
        panic!("display group should publish a direct surface");
    };

    let second = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        10,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should reuse retained surface");
    let [(_, second_frame)] = &second.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    let third = render_scene_for_test(
        &mut runtime,
        std::slice::from_ref(&group),
        1,
        40,
        &display_group_target_fps,
        &registry,
        &mut zones,
    )
    .expect("display group should rerender once its interval elapses");
    let [(_, third_frame)] = &third.group_canvases[..] else {
        panic!("display group should keep publishing a direct surface");
    };

    assert_eq!(
        first_frame.surface_for_test().storage_identity(),
        second_frame.surface_for_test().storage_identity()
    );
    assert_eq!(
        first_frame.surface_for_test().generation(),
        second_frame.surface_for_test().generation()
    );
    assert_eq!(
        first_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );
    assert_eq!(
        third_frame.surface_for_test().get_pixel(0, 0),
        Rgba::new(0, 255, 0, 255)
    );
    assert!(
        third_frame.surface_for_test().storage_identity()
            != second_frame.surface_for_test().storage_identity()
            || third_frame.surface_for_test().generation()
                != second_frame.surface_for_test().generation()
    );
}
