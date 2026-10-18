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
use std::time::{Duration, Instant};

use super::*;
use crate::render_thread::frame_sampling::ZoneSamplingSet;
use crate::render_thread::sparkleflinger::{
    MediaTextureSourceKey, ZoneSamplingDispatch, ZoneSamplingRequest,
};

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
    let mut colors = Vec::new();
    match sparkleflinger
        .begin_sample_zones_into(
            ZoneSamplingRequest::ZoneTextures(&set.gpu_textures),
            &mut colors,
        )
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
    set.assemble_into(&mut colors, &[]);
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
