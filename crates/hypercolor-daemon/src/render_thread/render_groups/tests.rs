use std::collections::HashMap;
use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;

use anyhow::Result;
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
use hypercolor_types::canvas::{Rgba, RgbaF32};
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::layer::MediaPlayback;
use hypercolor_types::scene::{SceneId, ZoneRole};
use hypercolor_types::spatial::{Corner, LedTopology, NormalizedPosition, Output, StripDirection};
use uuid::Uuid;

use super::projection::{build_group_projection, compose_authoritative_scene_canvas};

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
    .expect("surface allocation should succeed")
    .expect("surface should be materialized into the pool");

    assert!(matches!(backed, ProducerFrame::Surface(_)));
    assert_eq!(full_frame_copy.count, 1);
    assert_eq!(full_frame_copy.bytes, 16);
    assert_eq!(
        full_frame_copy.reason,
        Some("generation_zero_surface_pool_materialization")
    );
}

#[test]
fn wide_runtime_admits_one_scene_surface_without_eager_fanout() {
    let runtime =
        ZoneRuntime::try_new(7_681, 1).expect("addressable wide runtime should construct");

    assert_eq!(runtime.scene_surface_pool.descriptor().width, 7_681);
    assert_eq!(runtime.scene_surface_pool.descriptor().height, 1);
    assert_eq!(runtime.scene_surface_pool.materialized_slot_count(), 1);
    assert!(runtime.scene_surface_pool.slot_count() > 1);
}

#[test]
fn failed_scene_resize_preserves_last_good_geometry() {
    let mut runtime = ZoneRuntime::try_new(4, 4).expect("small runtime should construct");
    let previous_plan = runtime.combined_led_spatial_engine.sampling_plan();

    let result = runtime.try_resize_scene(u32::MAX, u32::MAX);

    assert!(matches!(
        result,
        Err(ZoneRuntimePreparationError::Spatial(
            SpatialPlanError::CanvasByteLengthOverflow {
                width: u32::MAX,
                height: u32::MAX,
            }
        ))
    ));
    assert_eq!(runtime.scene_width, 4);
    assert_eq!(runtime.scene_height, 4);
    assert_eq!(
        runtime.scene_surface_pool.descriptor(),
        SurfaceDescriptor::rgba8888(4, 4)
    );
    assert!(Arc::ptr_eq(
        &previous_plan,
        &runtime.combined_led_spatial_engine.sampling_plan()
    ));
}

#[test]
fn authoritative_group_reuses_the_exact_prepared_sampling_plan() {
    let mut runtime = ZoneRuntime::try_new(4, 4).expect("small runtime should construct");
    let mut group = sample_group(4, 4);
    make_color_fill_group(&mut group);
    group.layout.zones.push(point_zone("primary"));
    let authoritative =
        SpatialEngine::try_new(group.layout.clone()).expect("group layout should be addressable");
    group.layout = authoritative.layout().as_ref().clone();
    let authoritative_plan = authoritative.sampling_plan();

    runtime
        .reconcile(
            std::slice::from_ref(&group),
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(1, 1),
            &EffectRegistry::default(),
            &HashMap::new(),
            Some(&authoritative),
        )
        .expect("valid authoritative group should reconcile");

    let group_engine = runtime
        .spatial_engines
        .get(&group.id)
        .expect("group engine should be installed");
    assert!(Arc::ptr_eq(
        &authoritative_plan,
        &group_engine.sampling_plan()
    ));
    assert!(Arc::ptr_eq(
        &authoritative_plan,
        &runtime.combined_led_spatial_engine.sampling_plan()
    ));
}

#[test]
fn custom_group_plan_failure_preserves_every_installed_spatial_plan() {
    let mut runtime = ZoneRuntime::try_new(4, 4).expect("small runtime should construct");
    let mut group = sample_group(4, 4);
    make_color_fill_group(&mut group);
    group.layout.zones.push(point_zone("custom"));
    let original_dependency = SceneDependencyKey::new(1, 1);
    runtime
        .reconcile(
            std::slice::from_ref(&group),
            Some(SceneId::DEFAULT),
            original_dependency,
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
        )
        .expect("valid custom group should reconcile");
    let original_group_plan = runtime
        .spatial_engines
        .get(&group.id)
        .expect("group engine should be installed")
        .sampling_plan();
    let original_combined_plan = runtime.combined_led_spatial_engine.sampling_plan();

    group.layout.default_sampling_mode = SamplingMode::GaussianArea {
        sigma: 1.0,
        radius: u32::MAX,
    };
    let result = runtime.reconcile(
        std::slice::from_ref(&group),
        Some(SceneId::DEFAULT),
        SceneDependencyKey::new(2, 1),
        &EffectRegistry::default(),
        &HashMap::new(),
        None,
    );

    assert!(matches!(
        result,
        Err(error)
            if error.downcast_ref::<SpatialPlanError>()
                == Some(&SpatialPlanError::GaussianKernelUnaddressable { radius: u32::MAX })
    ));
    assert_eq!(runtime.reconciled_dependency_key, Some(original_dependency));
    assert!(Arc::ptr_eq(
        &original_group_plan,
        &runtime
            .spatial_engines
            .get(&group.id)
            .expect("last good group engine should remain installed")
            .sampling_plan()
    ));
    assert!(Arc::ptr_eq(
        &original_combined_plan,
        &runtime.combined_led_spatial_engine.sampling_plan()
    ));
}

#[test]
fn abandoned_prepared_group_resources_preserve_the_installed_generation() {
    let mut runtime = ZoneRuntime::try_new(4, 4).expect("small runtime should construct");
    let mut live_group = sample_group(4, 4);
    make_color_fill_group(&mut live_group);
    live_group.layout.zones.push(point_zone("live"));
    let live_dependency = SceneDependencyKey::new(1, 1);
    runtime
        .reconcile(
            std::slice::from_ref(&live_group),
            Some(SceneId::DEFAULT),
            live_dependency,
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
        )
        .expect("live group should reconcile");
    let live_plan = runtime.combined_led_spatial_engine.sampling_plan();

    let mut candidate_group = sample_group(8, 8);
    make_color_fill_group(&mut candidate_group);
    candidate_group.layout.zones.push(point_zone("candidate"));
    let prepared = runtime
        .prepare_reconcile_for_scene_dimensions(
            std::slice::from_ref(&candidate_group),
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(2, 1),
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
            8,
            8,
        )
        .expect("candidate resources should fully prepare");
    drop(prepared);

    assert_eq!(runtime.reconciled_dependency_key, Some(live_dependency));
    assert_eq!(runtime.scene_width, 4);
    assert_eq!(runtime.scene_height, 4);
    assert!(Arc::ptr_eq(
        &live_plan,
        &runtime.combined_led_spatial_engine.sampling_plan()
    ));
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

fn make_color_fill_group(group: &mut Zone) {
    group.effect_id = None;
    group.layers = vec![hypercolor_types::layer::SceneLayer {
        id: hypercolor_types::layer::SceneLayerId::new(),
        name: None,
        source: hypercolor_types::layer::LayerSource::ColorFill {
            rgba: [1.0, 0.0, 0.0, 1.0],
        },
        blend: hypercolor_types::layer::LayerBlendMode::Replace,
        opacity: 1.0,
        transform: hypercolor_types::layer::LayerTransform::default(),
        adjust: hypercolor_types::layer::LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }];
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
    elapsed_ms: u64,
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
    elapsed_ms: u64,
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
        input_availability: Default::default(),
        media: None,
        net: None,
        lighting: None,
    };
    let context = RenderSceneContext {
        groups,
        active_scene_id: Some(SceneId::DEFAULT),
        dependency_key: SceneDependencyKey::new(groups_revision, registry.generation()),
        elapsed_ms,
        display_group_target_fps,
        display_group_descriptors: &HashMap::new(),
        registry,
        authoritative_spatial_engine: None,
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
        ProducerFrame::ScreenPublication(_) => {
            frame
                .clone()
                .into_cpu_render_frame()
                .expect("screen publication test frame is CPU-backed")
                .0
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

mod invalidation;
mod layers;
mod projection;
mod retention;
mod scene_outputs;
