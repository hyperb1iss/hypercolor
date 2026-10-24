//! Spec 86 §8 acceptance cases the render-zone pass can show on its own,
//! each rendered on a required GPU compositor and sampled the way the
//! render thread samples it.

use std::time::Duration;

use hypercolor_core::input::SyntheticScreenPublisher;
use hypercolor_core::input::screen::PixelExtent;
use hypercolor_types::layer::{BindingMap, BindingSource, LayerBinding, LayerParameter, TimeWave};
use hypercolor_types::viewport::ViewportRect;

use super::gpu_layer_latch::{
    alpha_lcd_face_zone, bilinear_led_zone, gpu_sparkleflinger_at, sample_zone_set,
    sample_zone_set_against,
};
use super::*;
use crate::render_thread::frame_sampling::{ZoneSamplingSet, ZoneSourceCounts};
use crate::render_thread::producer_queue::GpuTextureFrame;
use crate::render_thread::sparkleflinger::MediaTextureSourceKey;

const SCENE_WIDTH: u32 = 640;
const SCENE_HEIGHT: u32 = 480;
const LED_ZONE_EXTENT: (u32, u32) = (640, 630);
/// Real frames are paced by the render loop; pace these the same way so
/// frame-to-frame state (held frames, sample caches) sees realistic gaps.
const FRAME_PACING: Duration = Duration::from_millis(16);

const RED: Rgba = Rgba::new(255, 0, 0, 255);
const GREEN: Rgba = Rgba::new(0, 255, 0, 255);
const BLUE: Rgba = Rgba::new(0, 0, 255, 255);

/// Media and screen-region layers in a scene's secondary zones keep their
/// own zone's pixels on the GPU (Spec 86 §8 criterion 8). A primary zone, a
/// GIF media zone, and a screen-region zone place their outputs at the same
/// scene position, so the composited scene shows only the last of them;
/// each zone's LEDs must still show that zone's own layer.
///
/// Web-viewport layers are not covered here: reconciling one starts a real
/// Servo session (`WebViewportRenderer::init_with_canvas_size`), which this
/// harness cannot host. They render through the same effect-layer path as
/// the HTML layers the injected-frame cases cover.
#[test]
fn secondary_zone_media_and_screen_layers_keep_their_own_pixels() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let mut library =
        AssetLibrary::open(tempdir.path().join("assets")).expect("asset library should open");
    let gif = library
        .add_bytes(&red_gif_bytes(), AssetUploadOptions::new("red.gif"))
        .expect("GIF upload should be accepted");
    let mut runtime =
        ZoneRuntime::with_asset_library(SCENE_WIDTH, SCENE_HEIGHT, Arc::new(RwLock::new(library)));
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut primary = bilinear_led_zone("Primary", "primary_leds", 0.5, solid_id);
    primary.role = ZoneRole::Primary;
    primary.layers = vec![scene_layer(LayerSource::ColorFill {
        rgba: [0.0, 1.0, 0.0, 1.0],
    })];
    let mut media = bilinear_led_zone("Media", "media_leds", 0.5, solid_id);
    media.layers = vec![scene_layer(LayerSource::Media {
        asset_id: gif.record.id,
        playback: MediaPlayback::default(),
    })];
    let mut screen = bilinear_led_zone("Screen", "screen_leds", 0.5, solid_id);
    screen.layers = vec![scene_layer(LayerSource::ScreenRegion {
        viewport: ViewportRect::full(),
    })];
    let zones = [primary, media, screen];
    let mut publisher =
        SyntheticScreenPublisher::surface(PixelExtent::new(8, 8).expect("fixture extent"));
    let mut zone_colors = Vec::new();

    for frame in 0..4_u64 {
        let screen_frame = publisher.publish_rgba(&[0, 0, 255, 255].repeat(64));
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &zones,
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            Some(&screen_frame),
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        let set = zone_set(&result);
        assert_eq!(
            set.source_counts(),
            ZoneSourceCounts {
                gpu: 3,
                cpu: 0,
                held: 0,
            },
            "frame {frame}: every zone should sample its own GPU texture"
        );
        let leds = sample_zone_set(&mut sparkleflinger, set);
        for (output_id, color) in [
            ("primary_leds", GREEN),
            ("media_leds", RED),
            ("screen_leds", BLUE),
        ] {
            assert_eq!(
                leds_of(&leds, output_id),
                Some(vec![[color.r, color.g, color.b]; 4]),
                "frame {frame}: {output_id} should show its own zone's layer"
            );
        }
        std::thread::sleep(FRAME_PACING);
    }
}

/// A display face stacking a GPU-resident effect under a GIF at its 1x1
/// intrinsic size still gets its frame. The face asks for CPU output, so the
/// GPU compositor's refusal of the off-extent canvas used to fall back to
/// the CPU compositor, which cannot read the effect's texture, and the face
/// published nothing.
#[test]
fn display_face_mixing_a_gpu_effect_with_an_off_extent_gif_keeps_its_frame() {
    const FACE_EXTENT: (u32, u32) = (8, 8);
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let mut library =
        AssetLibrary::open(tempdir.path().join("assets")).expect("asset library should open");
    let gif = library
        .add_bytes(&red_gif_bytes(), AssetUploadOptions::new("red.gif"))
        .expect("GIF upload should be accepted");
    let mut runtime =
        ZoneRuntime::with_asset_library(SCENE_WIDTH, SCENE_HEIGHT, Arc::new(RwLock::new(library)));
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut face = sample_display_zone(FACE_EXTENT.0, FACE_EXTENT.1);
    face.name = "Face".into();
    set_effect_zone(&mut face, solid_id, HashMap::new());
    let mut overlay = scene_layer(LayerSource::Media {
        asset_id: gif.record.id,
        playback: MediaPlayback::default(),
    });
    overlay.blend = BlendMode::Add;
    face.layers.push(overlay);
    let mut zone_colors = Vec::new();
    let mut saw_surface = false;

    // The face's composed surface lands when its readback finishes, a few
    // frames in on a fast GPU and later on a software adapter.
    for frame in 0..120_u64 {
        if saw_surface {
            break;
        }
        let effect_frame = upload_fill(&mut sparkleflinger, 1, FACE_EXTENT, GREEN);
        runtime.inject_effect_layer_frame_for_test(face.layers[0].id, effect_frame);
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            std::slice::from_ref(&face),
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("display face scene should render");
        let [(_, face_frame)] = &result.display_zone_frames[..] else {
            panic!("frame {frame}: the face should publish a frame");
        };
        if let ProducerFrame::Surface(surface) = &face_frame.frame {
            assert_eq!(
                surface.get_pixel(0, 0),
                Rgba::new(255, 255, 0, 255),
                "frame {frame}: the face should add the GIF over its effect"
            );
            saw_surface = true;
        }
        std::thread::sleep(FRAME_PACING);
    }
    assert!(saw_surface, "the face should publish a composed surface");
}

/// A zone whose only layer reports `Pending` for whole frames keeps
/// sampling its last snapshot, unchanged: the same texture and content
/// generation, a GPU source rather than a held one, and the same colors
/// (Spec 86 §2.2, §8 criterion 14).
#[test]
fn all_pending_zone_layers_hold_the_zone_snapshot() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let zones = [case.clone(), accents.clone()];
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut held_snapshot = None;

    for frame in 0..4_u64 {
        if frame == 0 {
            let case_frame = upload_fill(&mut sparkleflinger, 1, LED_ZONE_EXTENT, RED);
            runtime.inject_effect_layer_frame_for_test(case.layers[0].id, case_frame);
        } else {
            // Both polls of the frame come back pending.
            runtime.pend_effect_layer_renders_for_test(case.layers[0].id, 2);
        }
        let accent_frame = upload_fill(&mut sparkleflinger, 2, LED_ZONE_EXTENT, BLUE);
        runtime.inject_effect_layer_frame_for_test(accents.layers[0].id, accent_frame);

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
        assert!(
            runtime.pending_effect_layers.is_empty(),
            "frame {frame}: the zone should have polled its pending layer twice"
        );
        let set = zone_set(&result);
        assert_eq!(
            set.source_counts(),
            ZoneSourceCounts {
                gpu: 2,
                cpu: 0,
                held: 0,
            },
            "frame {frame}: a zone with a snapshot keeps sampling it"
        );
        let case_texture = &set.gpu_textures[0].frame;
        let snapshot = (case_texture.storage_id, case_texture.content_generation);
        match held_snapshot {
            None => held_snapshot = Some(snapshot),
            Some(held) => assert_eq!(
                snapshot, held,
                "frame {frame}: a pending zone should keep its snapshot unchanged"
            ),
        }
        let leds = sample_zone_set(&mut sparkleflinger, set);
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![[255, 0, 0]; 4]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "frame {frame}"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// A zone with no snapshot yet whose layers are all pending contributes no
/// GPU source: its LEDs take the published frame's colors only when those
/// have the LEDs' own shape, and are black otherwise (Spec 86 §2.2, §8
/// criterion 14). `cold_start_zone_holds_published_colors_or_goes_black`
/// drives the same rule through the frame pipeline, where a reshaped scene's
/// first sample is usually still in flight and the frame goes black by the
/// unavailable-sample rule instead.
#[test]
fn cold_start_zone_holds_published_colors_only_in_their_own_shape() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let case_frame = upload_fill(&mut sparkleflinger, 1, LED_ZONE_EXTENT, RED);
    runtime.inject_effect_layer_frame_for_test(case.layers[0].id, case_frame);
    runtime.pend_effect_layer_renders_for_test(accents.layers[0].id, 2);

    let result = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut runtime,
        &[case, accents],
        1,
        0,
        &HashMap::new(),
        &registry,
        &mut zone_colors,
        None,
        &mut sparkleflinger,
    )
    .expect("multi-zone GPU scene should render");
    let set = zone_set(&result);
    assert_eq!(
        set.source_counts(),
        ZoneSourceCounts {
            gpu: 1,
            cpu: 0,
            held: 1,
        },
        "a cold zone with nothing to compose should contribute no GPU source"
    );
    let published = |led_count: usize| {
        vec![ZoneColors {
            zone_id: "accent_leds".into(),
            colors: vec![[9, 9, 9]; led_count],
        }]
    };
    for (case_name, published, held) in [
        ("the same shape", published(4), [9, 9, 9]),
        ("another shape", published(1), [0, 0, 0]),
        ("no colors", Vec::new(), [0, 0, 0]),
    ] {
        let leds = sample_zone_set_against(&mut sparkleflinger, set, &published);
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![[255, 0, 0]; 4]),
            "published frame with {case_name}"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![held; 4]),
            "the cold zone against a published frame with {case_name}"
        );
    }
}

/// A zone resized while its producer keeps handing back the same retained
/// frame composes that frame into its replacement snapshot at the new
/// extent instead of reusing the composition it held at the old one
/// (Spec 86 §2.1, §8 criterion 14).
#[test]
fn resized_zone_recomposes_its_retained_frame_into_the_replacement_snapshot() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    // The producer renders once at the original extent and retains that
    // frame from then on, as Servo does under unchanging parameters.
    let case_frame = upload_fill(&mut sparkleflinger, 1, LED_ZONE_EXTENT, RED);
    let accent_frame = upload_fill(&mut sparkleflinger, 2, LED_ZONE_EXTENT, BLUE);
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut original_snapshot = None;

    for frame in 0..5_u64 {
        let revision = if frame < 2 { 1 } else { 2 };
        if frame == 2 {
            case.layout.canvas_width = 320;
            case.layout.canvas_height = 315;
        }
        runtime.inject_effect_layer_frame_for_test(case.layers[0].id, case_frame.clone());
        runtime.inject_effect_layer_frame_for_test(accents.layers[0].id, accent_frame.clone());
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &[case.clone(), accents.clone()],
            revision,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        let skips = result.zone_composition_skips;
        let set = zone_set(&result);
        let case_texture = &set.gpu_textures[0].frame;
        let extent = (case_texture.width, case_texture.height);
        match frame {
            0 | 1 => {
                assert_eq!(extent, LED_ZONE_EXTENT, "frame {frame}");
                original_snapshot = Some(case_texture.storage_id);
                assert_eq!(
                    skips,
                    if frame == 0 { 0 } else { 2 },
                    "frame {frame}: retained frames should reuse each zone's composition"
                );
            }
            2 => {
                assert_eq!(
                    extent,
                    (320, 315),
                    "the resized zone should sample a snapshot at its new extent"
                );
                assert_ne!(
                    Some(case_texture.storage_id),
                    original_snapshot,
                    "the resized zone should compose into its replacement snapshot"
                );
                assert_eq!(
                    skips, 0,
                    "no zone may reuse a composition held across the resize"
                );
            }
            _ => {
                assert_eq!(extent, (320, 315), "frame {frame}");
                assert_eq!(
                    skips, 2,
                    "frame {frame}: the replacement snapshot is reused once composed"
                );
            }
        }
        let leds = sample_zone_set(&mut sparkleflinger, set);
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![[255, 0, 0]; 4]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "frame {frame}"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// A layer adjustment driven by a binding changes the zone's output while
/// the producer frame stays the same, and the composition skip does not
/// swallow it: a square-wave time binding on brightness turns the zone
/// black and back while its producer frame never changes, and the frames
/// between the binding's edges still reuse the held composition (Spec 86
/// §2.1, §8 criterion 15).
#[test]
fn bound_layer_adjustment_recomposes_a_retained_producer_frame() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    case.layers[0].bindings = vec![LayerBinding {
        target: LayerParameter::Brightness,
        source: BindingSource::Time {
            rate_hz: 1.0,
            wave: TimeWave::Square,
        },
        map: BindingMap::linear(-1.0..=1.0, 0.0..=1.0),
    }];
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let case_frame = upload_fill(&mut sparkleflinger, 1, LED_ZONE_EXTENT, RED);
    let accent_frame = upload_fill(&mut sparkleflinger, 2, LED_ZONE_EXTENT, BLUE);
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    // The square wave is high for the first half of each second and low
    // for the second: full brightness, then none.
    let frames: [(u64, [u8; 3], u32); 5] = [
        (0, [255, 0, 0], 0),
        (100, [255, 0, 0], 2),
        (600, [0, 0, 0], 1),
        (700, [0, 0, 0], 2),
        (1_000, [255, 0, 0], 1),
    ];

    for (elapsed_ms, case_color, expected_skips) in frames {
        runtime.inject_effect_layer_frame_for_test(case.layers[0].id, case_frame.clone());
        runtime.inject_effect_layer_frame_for_test(accents.layers[0].id, accent_frame.clone());
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &[case.clone(), accents.clone()],
            1,
            elapsed_ms,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        assert_eq!(
            result.zone_composition_skips, expected_skips,
            "at {elapsed_ms} ms: only zones whose bound inputs held still may skip"
        );
        let leds = sample_zone_set(&mut sparkleflinger, zone_set(&result));
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![case_color; 4]),
            "at {elapsed_ms} ms: the bound brightness should reach the zone's LEDs"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "at {elapsed_ms} ms"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// A screen-region layer has no stable identity, so its zone composes on
/// every frame, even when the screen shows the same pixels twice: each
/// screen zone's snapshot generation advances on every frame, only the
/// unchanged zone beside them counts a composition skip, and their LEDs
/// follow the screen (Spec 86 §2.1, §8 criterion 15). One screen zone crops
/// the whole screen at its own extent, which composes as the CPU canvas it
/// is; the other crops half of it, which composes once uploaded.
#[test]
fn screen_region_layer_recomposes_every_frame() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut whole = bilinear_led_zone("Whole Screen", "whole_leds", 0.3, solid_id);
    whole.layers = vec![scene_layer(LayerSource::ScreenRegion {
        viewport: ViewportRect::full(),
    })];
    let mut half = bilinear_led_zone("Half Screen", "half_leds", 0.5, solid_id);
    half.layers = vec![scene_layer(LayerSource::ScreenRegion {
        viewport: ViewportRect::new(0.0, 0.0, 0.5, 1.0),
    })];
    let accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    let accent_frame = upload_fill(&mut sparkleflinger, 2, LED_ZONE_EXTENT, BLUE);
    let mut publisher = SyntheticScreenPublisher::surface(
        PixelExtent::new(LED_ZONE_EXTENT.0, LED_ZONE_EXTENT.1).expect("fixture extent"),
    );
    let screen_pixels = usize::try_from(LED_ZONE_EXTENT.0 * LED_ZONE_EXTENT.1)
        .expect("fixture pixel count fits usize");
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut last_generations: Option<(u64, u64)> = None;

    for (frame, color) in [RED, RED, GREEN, GREEN, BLUE, RED].into_iter().enumerate() {
        let frame = u64::try_from(frame).expect("frame index fits u64");
        let screen_frame =
            publisher.publish_rgba(&[color.r, color.g, color.b, 255].repeat(screen_pixels));
        runtime.inject_effect_layer_frame_for_test(accents.layers[0].id, accent_frame.clone());
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &[whole.clone(), half.clone(), accents.clone()],
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            Some(&screen_frame),
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        assert_eq!(
            result.zone_composition_skips,
            u32::from(frame > 0),
            "frame {frame}: only the unchanged accent zone may skip composing"
        );
        let set = zone_set(&result);
        let generations = (
            set.gpu_textures[0].frame.content_generation,
            set.gpu_textures[1].frame.content_generation,
        );
        if let Some(last) = last_generations {
            assert!(
                generations.0 > last.0 && generations.1 > last.1,
                "frame {frame}: both screen zones should compose again \
                 (generations {generations:?} after {last:?})"
            );
        }
        last_generations = Some(generations);
        let leds = sample_zone_set(&mut sparkleflinger, set);
        for output_id in ["whole_leds", "half_leds"] {
            assert_eq!(
                leds_of(&leds, output_id),
                Some(vec![[color.r, color.g, color.b]; 4]),
                "frame {frame}: {output_id} should follow the screen"
            );
        }
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "frame {frame}"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// A single full-scene zone whose only layer is a screen-region crop
/// composes on the GPU like any other single-zone scene, and its LEDs show
/// the screen. The crop is a CPU canvas at its own extent, which the GPU
/// compositor takes only once uploaded.
#[test]
fn single_screen_region_zone_composes_on_the_gpu() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut screen = bilinear_led_zone("Screen", "screen_leds", 0.5, solid_id);
    screen.layout.canvas_width = SCENE_WIDTH;
    screen.layout.canvas_height = SCENE_HEIGHT;
    screen.layers = vec![scene_layer(LayerSource::ScreenRegion {
        viewport: ViewportRect::full(),
    })];
    let mut publisher =
        SyntheticScreenPublisher::surface(PixelExtent::new(8, 8).expect("fixture extent"));
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();

    for (frame, color) in [BLUE, GREEN, RED].into_iter().enumerate() {
        let frame = u64::try_from(frame).expect("frame index fits u64");
        let screen_frame = publisher.publish_rgba(&[color.r, color.g, color.b, 255].repeat(64));
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            std::slice::from_ref(&screen),
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            Some(&screen_frame),
            &mut sparkleflinger,
        )
        .expect("single-zone GPU scene should render");
        let LedSamplingStrategy::SparkleFlinger(engine) = &result.led_sampling_strategy else {
            panic!("frame {frame}: a single full-scene zone should take the single-zone path");
        };
        let ProducerFrame::GpuTexture(scene_frame) = &result.scene_frame else {
            panic!("frame {frame}: the single-zone scene frame should stay GPU-resident");
        };
        let leds = sparkleflinger
            .sample_texture_zone_plan(scene_frame, engine.sampling_plan().as_ref())
            .expect("scene sampling should run")
            .expect("scene sampling should be admitted");
        assert_eq!(
            leds_of(&leds, "screen_leds"),
            Some(vec![[color.r, color.g, color.b]; 4]),
            "frame {frame}: the zone's LEDs should show the screen"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// A zone with layers but no outputs, the way Studio creates one before any
/// device joins it, places nothing in the scene canvas, so it must not take
/// the scene off the GPU. The other zones' LEDs keep their colors, the
/// scene canvas keeps showing them, and the memberless zone keeps its own
/// GPU frame for its zone preview.
#[test]
fn memberless_zone_keeps_every_other_zone_lit() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    case.layers = vec![color_fill_layer(GREEN)];
    let mut accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    accents.layers = vec![color_fill_layer(BLUE)];
    let mut memberless = bilinear_led_zone("New Zone", "unplaced", 0.5, solid_id);
    memberless.layout.zones.clear();
    memberless.layers = vec![color_fill_layer(RED)];
    let memberless_id = memberless.id;
    let zones = [case, accents, memberless];
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();

    for frame in 0..4_u64 {
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
        .unwrap_or_else(|error| {
            panic!("frame {frame}: a memberless zone failed the whole scene: {error:#}")
        });
        let leds = sample_zone_set(&mut sparkleflinger, zone_set(&result));
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![[0, 255, 0]; 4]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "frame {frame}"
        );
        assert!(
            result.scene_frame.is_gpu_resident(),
            "frame {frame}: the scene canvas should stay a GPU projection"
        );
        assert_eq!(
            scene_colors_at(
                &mut sparkleflinger,
                &result.scene_frame,
                &[(0.3, 0.5), (0.7, 0.5), (0.5, 0.1)],
            ),
            vec![[0, 255, 0], [0, 0, 255], [0, 0, 0]],
            "frame {frame}: the scene canvas should show every placed zone and \
             nothing of the memberless one"
        );
        let Some((_, memberless_frame)) = result
            .zone_canvases
            .iter()
            .find(|(zone_id, _)| *zone_id == memberless_id)
        else {
            panic!("frame {frame}: the memberless zone should publish its frame");
        };
        let ProducerFrame::GpuTexture(memberless_texture) = memberless_frame else {
            panic!("frame {frame}: the memberless zone's frame should stay on the GPU");
        };
        assert_eq!(
            texture_colors_at(&mut sparkleflinger, memberless_texture, &[(0.5, 0.5)]),
            vec![[255, 0, 0]],
            "frame {frame}: the memberless zone's own frame should show its layer"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// An output with no extent has no placement in the scene canvas, so it
/// projects nothing, the way the CPU raster skips it, and it does not take
/// its zone's other outputs or the other zones off the GPU projection.
#[test]
fn output_without_extent_projects_nothing_and_keeps_the_scene_on_the_gpu() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut case = bilinear_led_zone("Case", "case_leds", 0.3, solid_id);
    case.layers = vec![color_fill_layer(GREEN)];
    let mut collapsed = point_zone_at("collapsed_leds", 0.5, 0.15);
    collapsed.size = NormalizedPosition::new(0.0, 0.0);
    case.layout.zones.push(collapsed);
    let mut accents = bilinear_led_zone("WLED Accents", "accent_leds", 0.7, solid_id);
    accents.layers = vec![color_fill_layer(BLUE)];
    let zones = [case, accents];
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();

    for frame in 0..3_u64 {
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
        .unwrap_or_else(|error| panic!("frame {frame}: the scene failed: {error:#}"));
        let leds = sample_zone_set(&mut sparkleflinger, zone_set(&result));
        assert_eq!(
            leds_of(&leds, "case_leds"),
            Some(vec![[0, 255, 0]; 4]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "accent_leds"),
            Some(vec![[0, 0, 255]; 4]),
            "frame {frame}"
        );
        assert!(
            result.scene_frame.is_gpu_resident(),
            "frame {frame}: the scene canvas should stay a GPU projection"
        );
        assert_eq!(
            scene_colors_at(
                &mut sparkleflinger,
                &result.scene_frame,
                &[(0.3, 0.5), (0.7, 0.5), (0.5, 0.15), (0.1, 0.1), (0.9, 0.9)],
            ),
            vec![[0, 255, 0], [0, 0, 255], [0, 0, 0], [0, 0, 0], [0, 0, 0]],
            "frame {frame}: only the placed outputs should show"
        );
        std::thread::sleep(FRAME_PACING);
    }
}

/// Area-sampled zones at three extents beside a blended LCD face, which
/// switches the shared compositor's size every frame, allocate their
/// summed-area tables once: after warmup the table, snapshot, and projected
/// bind group counters stay flat while every frame recomposes and resamples
/// every zone (Spec 86 §3.2, §8 criterion 11).
#[test]
fn area_sampled_zones_allocate_no_summed_area_tables_after_warmup() {
    const WARMUP_FRAMES: u64 = 3;
    const MEASURED_FRAMES: u64 = 12;
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE_WIDTH, SCENE_HEIGHT) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let area = SamplingMode::AreaAverage {
        radius_x: 4.0,
        radius_y: 4.0,
    };
    let mut zones = [
        bilinear_led_zone("Case", "case_leds", 0.2, solid_id),
        bilinear_led_zone("WLED Accents", "accent_leds", 0.5, solid_id),
        bilinear_led_zone("Desk", "desk_leds", 0.8, solid_id),
    ];
    let extents = [LED_ZONE_EXTENT, (160, 100), (320, 200)];
    let colors = [RED, GREEN, BLUE];
    for (zone, extent) in zones.iter_mut().zip(extents) {
        zone.layout.default_sampling_mode = area.clone();
        zone.layout.canvas_width = extent.0;
        zone.layout.canvas_height = extent.1;
    }
    let face = alpha_lcd_face_zone("Corsair iCUE LINK LCD", solid_id);
    let scene_zones = [
        zones[0].clone(),
        zones[1].clone(),
        zones[2].clone(),
        face.clone(),
    ];
    let mut runtime = ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT);
    let mut zone_colors = Vec::new();
    let mut warm_counts = None;

    for frame in 0..WARMUP_FRAMES + MEASURED_FRAMES {
        // Fresh producer frames every frame: every zone composes again and
        // every dispatch misses the sample cache, so each frame encodes its
        // area passes.
        for (index, (zone, color)) in zones.iter().zip(colors).enumerate() {
            let key = u128::try_from(index).expect("zone index fits u128") + 1;
            let extent = (zone.layout.canvas_width, zone.layout.canvas_height);
            let layer_frame = upload_fill(&mut sparkleflinger, key, extent, color);
            runtime.inject_effect_layer_frame_for_test(zone.layers[0].id, layer_frame);
        }
        let face_frame = upload_fill(&mut sparkleflinger, 9, (480, 480), GREEN);
        runtime.inject_effect_layer_frame_for_test(face.layers[0].id, face_frame);

        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            &scene_zones,
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        assert_eq!(result.zone_composition_skips, 0, "frame {frame}");
        let set = zone_set(&result);
        let _ = sparkleflinger.take_led_sample_passes();
        let leds = sample_zone_set(&mut sparkleflinger, set);
        assert_eq!(
            sparkleflinger.take_led_sample_passes(),
            3,
            "frame {frame}: each zone texture should be sampled again"
        );
        for (output_id, color) in [
            ("case_leds", RED),
            ("accent_leds", GREEN),
            ("desk_leds", BLUE),
        ] {
            assert_eq!(
                leds_of(&leds, output_id),
                Some(vec![[color.r, color.g, color.b]; 4]),
                "frame {frame}: {output_id}"
            );
        }

        let counts = (
            sparkleflinger
                .summed_area_table_allocation_count_for_test()
                .expect("a GPU compositor counts its summed-area tables"),
            sparkleflinger
                .snapshot_texture_allocation_count_for_test()
                .expect("a GPU compositor counts its snapshots"),
            sparkleflinger
                .projected_bind_group_creation_count_for_test()
                .expect("a GPU compositor counts its projected bind groups"),
            sparkleflinger
                .sample_bind_group_creation_count_for_test()
                .expect("a GPU compositor counts its sampling bind groups"),
        );
        if frame + 1 == WARMUP_FRAMES {
            assert_eq!(
                counts.0, 3,
                "warmup should allocate one summed-area table per area-sampled extent"
            );
            warm_counts = Some(counts);
        } else if let Some(warm) = warm_counts {
            assert_eq!(
                counts, warm,
                "frame {frame}: (summed-area tables, snapshots, projected bind groups, \
                 sampling bind groups) should stay at their warmup counts"
            );
        }
        std::thread::sleep(FRAME_PACING);
    }
}

fn zone_set(result: &ZoneResult) -> &ZoneSamplingSet {
    let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
        panic!("a GPU multi-zone scene should sample each zone's own texture");
    };
    set
}

fn leds_of(zones: &[ZoneColors], output_id: &str) -> Option<Vec<[u8; 3]>> {
    zones
        .iter()
        .find(|zone| zone.zone_id == output_id)
        .map(|zone| zone.colors.clone())
}

/// A GPU-resident producer frame of `extent` filled with `color`, standing
/// in for one Servo render. Uploading under the same `key` again yields a
/// new content generation of the same texture.
fn upload_fill(
    sparkleflinger: &mut SparkleFlinger,
    key: u128,
    (width, height): (u32, u32),
    color: Rgba,
) -> ProducerFrame {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(color);
    ProducerFrame::GpuTexture(
        sparkleflinger
            .upload_media_canvas_frame(MediaTextureSourceKey::for_test(key), &canvas)
            .expect("GPU layer frame should upload"),
    )
}

fn color_fill_layer(color: Rgba) -> SceneLayer {
    scene_layer(LayerSource::ColorFill {
        rgba: [
            f32::from(color.r) / 255.0,
            f32::from(color.g) / 255.0,
            f32::from(color.b) / 255.0,
            1.0,
        ],
    })
}

/// The scene canvas's color at each normalized position, read with nearest
/// sampling wherever the frame lives.
fn scene_colors_at(
    sparkleflinger: &mut SparkleFlinger,
    scene_frame: &ProducerFrame,
    positions: &[(f32, f32)],
) -> Vec<[u8; 3]> {
    let probe = nearest_probe(SCENE_WIDTH, SCENE_HEIGHT, positions);
    let samples = match scene_frame {
        ProducerFrame::GpuTexture(texture) => sparkleflinger
            .sample_texture_zone_plan(texture, probe.sampling_plan().as_ref())
            .expect("scene probe sampling should run")
            .expect("scene probe sampling should be admitted"),
        frame => probe.sample(&canvas_from_scene_frame(frame)),
    };
    samples.iter().map(|zone| zone.colors[0]).collect()
}

/// A GPU texture's color at each normalized position, read with nearest
/// sampling.
fn texture_colors_at(
    sparkleflinger: &mut SparkleFlinger,
    texture: &GpuTextureFrame,
    positions: &[(f32, f32)],
) -> Vec<[u8; 3]> {
    let probe = nearest_probe(texture.width, texture.height, positions);
    sparkleflinger
        .sample_texture_zone_plan(texture, probe.sampling_plan().as_ref())
        .expect("texture probe sampling should run")
        .expect("texture probe sampling should be admitted")
        .iter()
        .map(|zone| zone.colors[0])
        .collect()
}

/// One single-LED nearest output per position over a `width` by `height`
/// canvas.
fn nearest_probe(width: u32, height: u32, positions: &[(f32, f32)]) -> SpatialEngine {
    let outputs = positions
        .iter()
        .enumerate()
        .map(|(index, &(x, y))| {
            let mut probe = point_zone_at(&format!("probe_{index}"), x, y);
            probe.size = NormalizedPosition::new(0.01, 0.01);
            probe.sampling_mode = Some(SamplingMode::Nearest);
            probe
        })
        .collect();
    SpatialEngine::new(SpatialLayout {
        id: "probe".into(),
        name: "Probe".into(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones: outputs,
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    })
}

fn scene_layer(source: LayerSource) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source,
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}
