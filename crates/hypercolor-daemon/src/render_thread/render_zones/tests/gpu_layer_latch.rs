//! Multi-zone scenes whose layers render on the GPU (Servo HTML effects)
//! with bilinear layouts, at the owner's rig extents: a 640x480 scene, 640x630
//! LED zones, and LCD faces composing at their own extents on the same
//! compositor.
//!
//! Before Spec 86 these zones fell back to a CPU path whose one compositor-
//! wide readback latch a face compose cleared every frame, so every LED and
//! the scene canvas went black, and same-extent zones read each other's
//! staged readbacks. Each zone now samples its own retained GPU texture with
//! its own plan, so every case here must hold its colors from the first
//! frame.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::device::{BackendManager, SegmentRange};
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::UnassignedBehavior;

use super::*;
use crate::render_thread::frame_sampling::{ZoneSamplingSet, ZoneSourceCounts};
use crate::render_thread::pipeline_runtime::UnassignedOutputCache;
use crate::render_thread::producer_queue::GpuTextureFrame;
use crate::render_thread::sparkleflinger::{
    MediaTextureSourceKey, ZoneSamplingDispatch, ZoneSamplingRequest, ZoneTextureSample,
};
use crate::render_thread::unassigned_output::UnassignedOutputPlanner;

const SCENE_WIDTH: u32 = 640;
const SCENE_HEIGHT: u32 = 480;
const LED_ZONE_WIDTH: u32 = 640;
const LED_ZONE_HEIGHT: u32 = 630;
const LCD_FACE_EXTENT: u32 = 480;
const RENDERED_FRAMES: u64 = 8;
/// Real frames are paced by the render loop; pace the test the same way so
/// frame-to-frame state (held frames, sample caches) sees realistic gaps.
const FRAME_PACING: Duration = Duration::from_millis(20);

const CASE_RED: Rgba = Rgba::new(255, 0, 0, 255);
const ACCENT_BLUE: Rgba = Rgba::new(0, 0, 255, 255);
const FACE_GREEN: Rgba = Rgba::new(0, 255, 0, 255);

struct GpuLayerSource {
    layer_id: SceneLayerId,
    source_key: u128,
    width: u32,
    height: u32,
    color: Rgba,
}

struct LedExpectation {
    output_id: &'static str,
    zone_name: String,
    color: Rgba,
}

/// The live rig: two LED zones beside an LCD face.
#[test]
fn two_bilinear_led_zones_with_gpu_layers_and_lcd_face_keep_their_colors() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let face = alpha_lcd_face_zone("Corsair iCUE LINK LCD", solid_id);
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&accents, 2, ACCENT_BLUE),
        gpu_source(&face, 3, FACE_GREEN),
    ];
    let expectations = [
        led_expectation(&case, "case_leds", CASE_RED),
        led_expectation(&accents, "accent_leds", ACCENT_BLUE),
    ];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case, accents, face],
        &sources,
        &expectations,
    );
}

/// Two LED zones and no face: each keeps its own color.
#[test]
fn two_bilinear_led_zones_with_gpu_layers_keep_their_own_colors() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&accents, 2, ACCENT_BLUE),
    ];
    let expectations = [
        led_expectation(&case, "case_leds", CASE_RED),
        led_expectation(&accents, "accent_leds", ACCENT_BLUE),
    ];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case, accents],
        &sources,
        &expectations,
    );
}

/// One LED zone whose canvas differs from the scene's takes the multi-zone
/// path and keeps its color.
#[test]
fn single_bilinear_led_zone_off_scene_extent_with_gpu_layer_keeps_its_color() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.5, solid_id);
    let sources = [gpu_source(&case, 1, CASE_RED)];
    let expectations = [led_expectation(&case, "case_leds", CASE_RED)];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case],
        &sources,
        &expectations,
    );
}

/// The Studio zone-layout preview shape: one off-extent zone beside a face.
#[test]
fn single_bilinear_led_zone_off_scene_extent_with_lcd_face_keeps_its_color() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.5, solid_id);
    let face = alpha_lcd_face_zone("Corsair iCUE LINK LCD", solid_id);
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&face, 3, FACE_GREEN),
    ];
    let expectations = [led_expectation(&case, "case_leds", CASE_RED)];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case, face],
        &sources,
        &expectations,
    );
}

/// Two zones whose outputs sit at the same scene position: each zone's LEDs
/// still read that zone's own pixels (Spec 64 §2.3), where the old projected
/// path sampled whichever zone composited last.
#[test]
fn overlapping_bilinear_led_zones_keep_their_own_colors() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.5, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.5, solid_id);
    let face = alpha_lcd_face_zone("Corsair iCUE LINK LCD", solid_id);
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&accents, 2, ACCENT_BLUE),
        gpu_source(&face, 3, FACE_GREEN),
    ];
    let expectations = [
        led_expectation(&case, "case_leds", CASE_RED),
        led_expectation(&accents, "accent_leds", ACCENT_BLUE),
    ];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case, accents, face],
        &sources,
        &expectations,
    );
}

/// A GPU-backed zone whose layout samples Gaussian-area samples on the GPU
/// beside a bilinear zone, each from its own texture.
#[test]
fn gaussian_gpu_zone_samples_on_the_gpu_beside_a_bilinear_zone() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let mut accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    accents.layout.zones[0].sampling_mode = Some(SamplingMode::GaussianArea {
        sigma: 2.0,
        radius: 3,
    });
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&accents, 2, ACCENT_BLUE),
    ];
    let expectations = [
        led_expectation(&case, "case_leds", CASE_RED),
        led_expectation(&accents, "accent_leds", ACCENT_BLUE),
    ];

    assert_scene_keeps_led_colors(
        &mut sparkleflinger,
        &registry,
        &[case, accents],
        &sources,
        &expectations,
    );
}

/// A zone whose layers hand back the same frames reuses its snapshot without
/// composing: its content generation holds, so sampling hits its cache. A
/// zone whose input changes composes again, and both keep their colors. The
/// frame telemetry counts each skip, and each pass a dispatch encodes.
#[test]
fn unchanged_zone_layers_reuse_their_snapshot() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let upload = |sparkleflinger: &mut SparkleFlinger, source: &GpuLayerSource, color: Rgba| {
        let mut canvas = Canvas::new(source.width, source.height);
        canvas.fill(color);
        sparkleflinger
            .upload_media_canvas_frame(MediaTextureSourceKey::for_test(source.source_key), &canvas)
            .expect("GPU layer frame should upload")
    };
    let case_source = gpu_source(&case, 1, CASE_RED);
    let accent_source = gpu_source(&accents, 2, ACCENT_BLUE);
    let case_frame = upload(&mut sparkleflinger, &case_source, CASE_RED);
    let mut accent_frame = upload(&mut sparkleflinger, &accent_source, ACCENT_BLUE);
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut generations = Vec::new();
    let mut composition_skips = Vec::new();
    let mut sample_passes = Vec::new();
    let mut repeat_sample_passes = Vec::new();

    for frame in 0..4_u64 {
        if frame == 3 {
            accent_frame = upload(&mut sparkleflinger, &accent_source, FACE_GREEN);
        }
        runtime.inject_effect_layer_frame_for_test(
            case_source.layer_id,
            ProducerFrame::GpuTexture(case_frame.clone()),
        );
        runtime.inject_effect_layer_frame_for_test(
            accent_source.layer_id,
            ProducerFrame::GpuTexture(accent_frame.clone()),
        );
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &[case.clone(), accents.clone()],
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
            panic!("a GPU multi-zone scene should sample each zone's own texture");
        };
        generations.push(
            set.gpu_textures
                .iter()
                .map(|texture| texture.frame.content_generation)
                .collect::<Vec<_>>(),
        );
        composition_skips.push(result.zone_composition_skips);
        let leds = sample_zone_set(&mut sparkleflinger, set);
        sample_passes.push(sparkleflinger.take_led_sample_passes());
        let _ = sample_zone_set(&mut sparkleflinger, set);
        repeat_sample_passes.push(sparkleflinger.take_led_sample_passes());
        let led = |output_id: &str| {
            leds.iter()
                .find(|zone| zone.zone_id == output_id)
                .and_then(|zone| zone.colors.first().copied())
        };
        assert_eq!(led("case_leds"), Some([255, 0, 0]), "frame {frame}");
        let accent_color = if frame == 3 { [0, 255, 0] } else { [0, 0, 255] };
        assert_eq!(led("accent_leds"), Some(accent_color), "frame {frame}");
    }

    assert_eq!(
        generations[1], generations[0],
        "unchanged layers should not recompose"
    );
    assert_eq!(
        generations[2], generations[0],
        "unchanged layers should not recompose"
    );
    assert_eq!(
        generations[3][0], generations[0][0],
        "the unchanged zone keeps its snapshot"
    );
    assert!(
        generations[3][1] > generations[0][1],
        "a zone whose input changed should compose again: {generations:?}"
    );
    assert_eq!(
        composition_skips,
        vec![0, 2, 2, 1],
        "every zone whose layers matched its held frame should count a skip"
    );
    assert_eq!(
        (sample_passes[0], sample_passes[3]),
        (2, 2),
        "a dispatch over new zone content encodes one pass per zone texture: {sample_passes:?}"
    );
    assert_eq!(
        repeat_sample_passes,
        vec![0; 4],
        "a dispatch the sample cache answers encodes no passes"
    );
}

/// GPU-backed zones never materialize into CPU zone canvases (Spec 86 §7,
/// criterion 11): a GPU multi-zone scene records no producer full-frame copy
/// on frames that compose or reuse a held composition, every zone samples
/// on the GPU, and an unassigned `Fallback(zone)` plan reads the fallback
/// zone's texture, whose pass rides the zone set's dispatch.
#[test]
fn gpu_zones_record_no_zone_canvas_materialization_with_unassigned_fallback() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let zones = [case.clone(), accents.clone()];
    let sources = [
        gpu_source(&case, 1, CASE_RED),
        gpu_source(&accents, 2, ACCENT_BLUE),
    ];
    let mut manager = BackendManager::new();
    manager.map_device_with_segment(
        "mock:unplaced",
        "mock",
        DeviceId::new(),
        Some(SegmentRange::new(0, 4)),
    );
    let mut unassigned_cache = UnassignedOutputCache::default();
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut layer_frames = Vec::new();

    for frame in 0..RENDERED_FRAMES {
        // Fresh layer frames compose; repeating the last ones from the
        // midpoint on takes the held-composition path.
        if frame < RENDERED_FRAMES / 2 {
            layer_frames = sources
                .iter()
                .map(|source| {
                    let mut canvas = Canvas::new(source.width, source.height);
                    canvas.fill(source.color);
                    sparkleflinger
                        .upload_media_canvas_frame(
                            MediaTextureSourceKey::for_test(source.source_key),
                            &canvas,
                        )
                        .expect("GPU layer frame should upload")
                })
                .collect();
        }
        for (source, layer_frame) in sources.iter().zip(&layer_frames) {
            runtime.inject_effect_layer_frame_for_test(
                source.layer_id,
                ProducerFrame::GpuTexture(layer_frame.clone()),
            );
        }

        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &zones,
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");

        assert_eq!(
            result.producer_full_frame_copy.count, 0,
            "frame {frame}: GPU zones recorded a full-frame copy ({:?})",
            result.producer_full_frame_copy.reason
        );
        if frame >= RENDERED_FRAMES / 2 {
            assert_eq!(
                result.zone_composition_skips, 2,
                "frame {frame}: repeated layer frames should reuse each held composition"
            );
        }
        let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
            panic!("a GPU multi-zone scene should sample each zone's own texture");
        };
        assert_eq!(
            set.source_counts(),
            ZoneSourceCounts {
                gpu: 2,
                cpu: 0,
                held: 0,
            },
            "frame {frame}"
        );
        let case_texture = match result
            .zone_canvases
            .iter()
            .find(|(zone_id, _)| *zone_id == case.id)
            .map(|(_, frame)| frame)
        {
            Some(ProducerFrame::GpuTexture(texture)) => texture.clone(),
            _ => panic!("frame {frame}: the fallback zone should publish its GPU texture"),
        };

        let plan = UnassignedOutputPlanner::new(&manager, &mut unassigned_cache)
            .plan(
                Arc::clone(&set.layout),
                &UnassignedBehavior::Fallback(case.id),
                &zones,
                &result.zone_canvases,
            )
            .expect("the unassigned fallback should plan");
        assert!(
            plan.layout()
                .zones
                .iter()
                .any(|output| output.device_id == "mock:unplaced"),
            "frame {frame}: the unplaced device should be an unassigned output"
        );
        let fallback = unassigned_cache
            .gpu_fallback()
            .filter(|fallback| fallback.zone_id == case.id)
            .expect("a GPU-backed fallback zone should sample its texture on the GPU");

        if frame == 0 {
            let fallback_plan = fallback.engine.sampling_plan();
            let fallback_outputs = fallback_plan.len();
            let mut textures = set.gpu_textures.clone();
            textures.push(ZoneTextureSample {
                frame: GpuTextureFrame {
                    immutable_lease: None,
                    ..case_texture
                },
                prepared_zones: fallback_plan,
            });
            let _ = sparkleflinger.take_led_sample_passes();
            let colors = sample_zone_textures(&mut sparkleflinger, &textures);
            assert_eq!(
                sparkleflinger.take_led_sample_passes(),
                3,
                "the fallback pass rides the zone set's dispatch"
            );
            let fallback_colors = &colors[colors.len() - fallback_outputs..];
            assert!(
                fallback_colors
                    .iter()
                    .flat_map(|zone| zone.colors.iter())
                    .all(|color| *color == [255, 0, 0]),
                "unassigned outputs should sample the fallback zone: {fallback_colors:?}"
            );
        }
    }
}

/// Renders `RENDERED_FRAMES` paced frames, feeding every layer a fresh
/// GPU-resident frame the way a Servo effect does, and asserts on every frame
/// that each LED zone samples its own layer color and that the projected
/// scene canvas is lit.
fn assert_scene_keeps_led_colors(
    sparkleflinger: &mut SparkleFlinger,
    registry: &EffectRegistry,
    zones: &[Zone],
    sources: &[GpuLayerSource],
    expectations: &[LedExpectation],
) {
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut trace = String::new();
    let mut failures = Vec::new();
    let scene_probe = SpatialEngine::new(scene_probe_layout());

    for frame in 0..RENDERED_FRAMES {
        for source in sources {
            let mut canvas = Canvas::new(source.width, source.height);
            canvas.fill(source.color);
            let gpu_frame = sparkleflinger
                .upload_media_canvas_frame(
                    MediaTextureSourceKey::for_test(source.source_key),
                    &canvas,
                )
                .expect("GPU layer frame should upload");
            runtime.inject_effect_layer_frame_for_test(
                source.layer_id,
                ProducerFrame::GpuTexture(gpu_frame),
            );
        }

        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            zones,
            1,
            frame * 16,
            &HashMap::new(),
            registry,
            &mut zone_colors,
            None,
            sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
            panic!("a GPU multi-zone scene should sample each zone's own texture");
        };
        let leds_by_output = sample_zone_set(sparkleflinger, set);

        let _ = write!(trace, "frame {frame}:");
        for expectation in expectations {
            let leds = leds_by_output
                .iter()
                .find(|zone| zone.zone_id == expectation.output_id)
                .map(|zone| zone.colors.clone())
                .unwrap_or_default();
            let _ = write!(trace, " {} leds={:?};", expectation.zone_name, leds.first());
            let expected = [
                expectation.color.r,
                expectation.color.g,
                expectation.color.b,
            ];
            if leds.is_empty() || leds.iter().any(|color| *color != expected) {
                failures.push(format!(
                    "frame {frame} {}: expected {expected:?}, sampled {:?}",
                    expectation.zone_name,
                    leds.first()
                ));
            }
        }
        let scene_lit = scene_lit_samples(sparkleflinger, &result.scene_frame, &scene_probe);
        let _ = writeln!(trace, " scene lit samples={scene_lit}");
        if scene_lit == 0 {
            failures.push(format!(
                "frame {frame}: the projected scene canvas is black"
            ));
        }

        std::thread::sleep(FRAME_PACING);
    }

    assert!(
        failures.is_empty(),
        "LED zones lost their GPU layer colors:\n{}\ntrace:\n{trace}",
        failures.join("\n")
    );
}

/// Sample a zone set the way the render thread does: one dispatch over the
/// zones' textures, then splice CPU-sampled and held zones into zone order.
fn sample_zone_set(sparkleflinger: &mut SparkleFlinger, set: &ZoneSamplingSet) -> Vec<ZoneColors> {
    let mut colors = sample_zone_textures(sparkleflinger, &set.gpu_textures);
    set.assemble_into(&mut colors, &[]);
    colors
}

/// One dispatch over `textures`, waiting for its readback.
fn sample_zone_textures(
    sparkleflinger: &mut SparkleFlinger,
    textures: &[ZoneTextureSample],
) -> Vec<ZoneColors> {
    let mut colors = Vec::new();
    match sparkleflinger
        .begin_sample_zones_into(ZoneSamplingRequest::ZoneTextures(textures), &mut colors)
        .expect("zone set sampling should dispatch")
    {
        ZoneSamplingDispatch::Ready => {}
        ZoneSamplingDispatch::Pending(mut pending) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !sparkleflinger
                .try_finish_pending_zone_sampling(&mut pending, &mut colors)
                .expect("zone set readback should finish")
            {
                assert!(Instant::now() < deadline, "zone set readback timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        ZoneSamplingDispatch::Unsupported | ZoneSamplingDispatch::Saturated => {
            panic!("zone set sampling should admit bilinear plans")
        }
    }
    colors
}

/// Count lit samples on an 8x8 nearest grid over the scene canvas.
fn scene_lit_samples(
    sparkleflinger: &mut SparkleFlinger,
    scene_frame: &ProducerFrame,
    scene_probe: &SpatialEngine,
) -> usize {
    let samples = match scene_frame {
        ProducerFrame::GpuTexture(texture) => sparkleflinger
            .sample_texture_zone_plan(texture, scene_probe.sampling_plan().as_ref())
            .expect("scene probe sampling should run")
            .expect("scene probe sampling should be admitted"),
        frame => scene_probe.sample(&canvas_from_scene_frame(frame)),
    };
    samples
        .iter()
        .flat_map(|zone| zone.colors.iter())
        .filter(|color| **color != [0, 0, 0])
        .count()
}

fn scene_probe_layout() -> SpatialLayout {
    let mut probe = point_zone_at("scene_probe", 0.5, 0.5);
    probe.size = NormalizedPosition::new(1.0, 1.0);
    probe.topology = LedTopology::Matrix {
        width: 8,
        height: 8,
        serpentine: false,
        start_corner: Corner::TopLeft,
    };
    probe.sampling_mode = Some(SamplingMode::Nearest);
    SpatialLayout {
        id: "scene-probe".into(),
        name: "Scene probe".into(),
        description: None,
        canvas_width: SCENE_WIDTH,
        canvas_height: SCENE_HEIGHT,
        zones: vec![probe],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn bilinear_led_zone(name: &str, output_id: &str, x: f32, effect_id: EffectId) -> Zone {
    let mut zone = sample_zone(LED_ZONE_WIDTH, LED_ZONE_HEIGHT);
    zone.name = name.into();
    set_effect_zone(&mut zone, effect_id, HashMap::new());
    let mut output = point_zone_at(output_id, x, 0.5);
    output.topology = LedTopology::Strip {
        count: 4,
        direction: StripDirection::LeftToRight,
    };
    zone.layout.zones = vec![output];
    zone.layout.default_sampling_mode = SamplingMode::Bilinear;
    zone.layout.default_edge_behavior = EdgeBehavior::Clamp;
    zone
}

fn alpha_lcd_face_zone(name: &str, effect_id: EffectId) -> Zone {
    let mut zone = sample_display_zone(LCD_FACE_EXTENT, LCD_FACE_EXTENT);
    zone.name = name.into();
    set_effect_zone(&mut zone, effect_id, HashMap::new());
    zone.layers[0].blend = BlendMode::Alpha;
    zone
}

fn gpu_source(zone: &Zone, source_key: u128, color: Rgba) -> GpuLayerSource {
    GpuLayerSource {
        layer_id: zone.layers[0].id,
        source_key,
        width: zone.layout.canvas_width,
        height: zone.layout.canvas_height,
        color,
    }
}

fn led_expectation(zone: &Zone, output_id: &'static str, color: Rgba) -> LedExpectation {
    LedExpectation {
        output_id,
        zone_name: zone.name.clone(),
        color,
    }
}

fn gpu_sparkleflinger_at(width: u32, height: u32) -> Option<SparkleFlinger> {
    match SparkleFlinger::new_required_gpu_for_test() {
        Ok(mut sparkleflinger) => {
            let canvas = sparkleflinger
                .prepare_canvas_resize(width, height)
                .expect("GPU scene canvas should prepare");
            assert!(canvas.is_admitted());
            sparkleflinger.apply_canvas_resize(canvas);
            Some(sparkleflinger)
        }
        Err(error) => {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU latch test was required but initialization failed: {error}"
            );
            None
        }
    }
}
