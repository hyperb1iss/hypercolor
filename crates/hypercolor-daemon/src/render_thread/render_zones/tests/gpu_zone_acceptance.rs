//! Spec 86 §8 acceptance cases the render-zone pass can show on its own,
//! each rendered on a required GPU compositor and sampled the way the
//! render thread samples it.

use std::time::Duration;

use hypercolor_core::input::SyntheticScreenPublisher;
use hypercolor_core::input::screen::PixelExtent;
use hypercolor_types::viewport::ViewportRect;

use super::gpu_layer_latch::{bilinear_led_zone, gpu_sparkleflinger_at, sample_zone_set};
use super::*;
use crate::render_thread::frame_sampling::{ZoneSamplingSet, ZoneSourceCounts};

const SCENE_WIDTH: u32 = 640;
const SCENE_HEIGHT: u32 = 480;
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
