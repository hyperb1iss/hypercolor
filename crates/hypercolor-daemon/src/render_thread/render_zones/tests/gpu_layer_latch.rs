//! Multi-zone scenes whose layers render on the GPU (Servo HTML effects) and
//! whose layouts refuse GPU projection (bilinear sampling) fall back to the
//! CPU zone path: every zone composes through the shared SparkleFlinger with
//! `requires_cpu_sampling_canvas`, so its CPU canvas can only come back
//! through the compositor's one-frame sampling readback latch.
//!
//! The rig shape mirrors the O11D scene: the daemon canvas is 640x480, the
//! two LED zone layouts are 640x630 with bilinear sampling, and LCD face
//! zones compose at their own extents on the same compositor. A compose at a
//! new extent replaces the compositor surfaces, which clears the latch, so a
//! face compose between two frames leaves every LED zone without a canvas.
//! Without a face, same-extent zones share the one latch and read each
//! other's staged readbacks.

use std::fmt::Write as _;
use std::time::Duration;

use super::super::projection::zones_support_projection_composition;
use super::*;
use crate::render_thread::sparkleflinger::MediaTextureSourceKey;

const SCENE_WIDTH: u32 = 640;
const SCENE_HEIGHT: u32 = 480;
const LED_ZONE_WIDTH: u32 = 640;
const LED_ZONE_HEIGHT: u32 = 630;
const LCD_FACE_EXTENT: u32 = 480;
const RENDERED_FRAMES: u64 = 8;
/// The sampling latch serves the previous compose's readback, so the first
/// frames may legitimately come back empty while it fills.
const WARMUP_FRAMES: u64 = 2;
/// Real frames are paced by the render loop; give each staged readback the
/// time a 60 fps frame would before the next frame resolves it.
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
    zone_id: ZoneId,
    color: Rgba,
}

/// The live rig: every LED and the whole scene canvas stay black.
#[test]
#[ignore = "reproduces multi-zone GPU black frame"]
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

/// Without a face compose the latch survives, but the second zone samples
/// the first zone's staged readback (and the first zone the second's).
#[test]
#[ignore = "reproduces multi-zone GPU black frame"]
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

/// Contrast case: one off-extent zone alone recovers after the first latch
/// frame, so the extent mismatch by itself does not black out the rig.
#[test]
#[ignore = "contrast case for the multi-zone GPU black frame repro; passes today"]
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

/// The single-zone Studio preview variant goes black once a face composes.
#[test]
#[ignore = "reproduces multi-zone GPU black frame"]
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

/// Renders `RENDERED_FRAMES` paced frames, feeding every layer a fresh
/// GPU-resident frame the way a Servo effect does, and asserts that after
/// the latch warmup every LED zone samples its own layer color.
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

        if frame == 0 {
            assert!(
                !zones_support_projection_composition(zones, &runtime.scene_projection_cache),
                "bilinear layouts must refuse GPU projection so zones take the CPU path"
            );
        }
        assert!(
            matches!(
                result.led_sampling_strategy,
                LedSamplingStrategy::PreSampled(_)
            ),
            "the CPU zone path pre-samples LED colors from zone target canvases"
        );

        let _ = write!(trace, "frame {frame}:");
        for expectation in expectations {
            let leds = zone_colors
                .iter()
                .find(|zone| zone.zone_id == expectation.output_id)
                .map(|zone| zone.colors.clone())
                .unwrap_or_default();
            let materialized = result
                .zone_canvases
                .iter()
                .find(|(zone_id, _)| *zone_id == expectation.zone_id)
                .map(|(_, frame)| canvas_from_scene_frame(frame).get_pixel(0, 0));
            let _ = write!(
                trace,
                " {} leds={:?} zone_canvas={};",
                expectation.zone_name,
                leds.first(),
                materialized.map_or_else(
                    || "none (GPU frame, target cleared)".to_owned(),
                    |pixel| format!("{pixel:?}")
                ),
            );
            let expected = [
                expectation.color.r,
                expectation.color.g,
                expectation.color.b,
            ];
            if frame >= WARMUP_FRAMES
                && (leds.is_empty() || leds.iter().any(|color| *color != expected))
            {
                failures.push(format!(
                    "frame {frame} {}: expected {expected:?}, sampled {:?}",
                    expectation.zone_name,
                    leds.first()
                ));
            }
        }
        let scene = canvas_from_scene_frame(&result.scene_frame);
        let scene_lit = scene
            .as_rgba_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..3] != [0, 0, 0])
            .count();
        let _ = writeln!(trace, " scene lit pixels={scene_lit}");

        std::thread::sleep(FRAME_PACING);
    }

    assert!(
        failures.is_empty(),
        "LED zones lost their GPU layer colors after warmup:\n{}\ntrace:\n{trace}",
        failures.join("\n")
    );
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
        zone_id: zone.id,
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
