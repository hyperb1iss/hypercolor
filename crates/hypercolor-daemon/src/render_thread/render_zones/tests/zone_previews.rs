//! Studio zone previews of a GPU-resident multi-zone scene (Spec 86 §5.2).
//!
//! Every LED zone's frame lives in its own GPU texture, so the CPU converter
//! cannot publish it. Each zone's texture is scaled on the GPU to the zone
//! preview demand and read back at that size only; nothing is staged while
//! nobody watches zone previews.

use std::time::{Duration, Instant};

use super::*;
use crate::preview_runtime::PreviewDemandSummary;
use crate::render_thread::sparkleflinger::{MediaTextureSourceKey, ZonePreviewStage};
use crate::render_thread::zone_previews::{
    ZonePreviewDemand, collect_gpu_zone_previews, stage_gpu_zone_previews, zone_preview_sources,
};

const SCENE_WIDTH: u32 = 320;
const SCENE_HEIGHT: u32 = 200;
const CASE_EXTENT: (u32, u32) = (640, 630);
const ACCENT_EXTENT: (u32, u32) = (160, 100);
const FACE_EXTENT: u32 = 480;
const CASE_RED: Rgba = Rgba::new(255, 0, 0, 255);
const ACCENT_BLUE: Rgba = Rgba::new(0, 0, 255, 255);
const FACE_GREEN: Rgba = Rgba::new(0, 255, 0, 255);
const PREVIEW_DEADLINE: Duration = Duration::from_secs(2);

struct GpuLayerSource {
    layer_id: SceneLayerId,
    source_key: u128,
    width: u32,
    height: u32,
    color: Rgba,
}

/// Two bilinear LED zones at their own extents beside an LCD face that
/// composes through the same compositor at a third size.
struct PreviewScene {
    sparkleflinger: SparkleFlinger,
    runtime: ZoneRuntime,
    registry: EffectRegistry,
    zones: Vec<Zone>,
    sources: Vec<GpuLayerSource>,
    rendered_frames: u64,
}

impl PreviewScene {
    fn new() -> Option<Self> {
        let sparkleflinger = gpu_sparkleflinger()?;
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let case = led_zone("Case", "case_leds", CASE_EXTENT, solid_id);
        let accents = led_zone("Accents", "accent_leds", ACCENT_EXTENT, solid_id);
        let mut face = sample_display_zone(FACE_EXTENT, FACE_EXTENT);
        set_effect_zone(&mut face, solid_id, HashMap::new());
        face.layers[0].blend = BlendMode::Alpha;
        let sources = vec![
            layer_source(&case, 1, CASE_RED),
            layer_source(&accents, 2, ACCENT_BLUE),
            layer_source(&face, 3, FACE_GREEN),
        ];
        Some(Self {
            sparkleflinger,
            runtime: ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT),
            registry,
            zones: vec![case, accents, face],
            sources,
            rendered_frames: 0,
        })
    }

    fn case(&self) -> &Zone {
        &self.zones[0]
    }

    fn accents(&self) -> &Zone {
        &self.zones[1]
    }

    fn face(&self) -> &Zone {
        &self.zones[2]
    }

    /// Render one frame, feeding every layer a fresh GPU-resident frame the
    /// way a Servo effect does.
    fn render(&mut self) -> ZoneResult {
        for source in &self.sources {
            let mut canvas = Canvas::new(source.width, source.height);
            canvas.fill(source.color);
            let frame = self
                .sparkleflinger
                .upload_media_canvas_frame(
                    MediaTextureSourceKey::for_test(source.source_key),
                    &canvas,
                )
                .expect("GPU layer frame should upload");
            self.runtime.inject_effect_layer_frame_for_test(
                source.layer_id,
                ProducerFrame::GpuTexture(frame),
            );
        }
        let mut zone_colors = Vec::new();
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut self.runtime,
            &self.zones,
            1,
            self.rendered_frames * 16,
            &HashMap::new(),
            &self.registry,
            &mut zone_colors,
            None,
            &mut self.sparkleflinger,
        )
        .expect("multi-zone GPU scene should render");
        self.rendered_frames += 1;
        result
    }

    fn stage(&mut self, demand: ZonePreviewDemand, result: &ZoneResult) {
        stage_gpu_zone_previews(
            &mut self.sparkleflinger,
            demand,
            &result.zone_canvases,
            &result.active_display_zone_ids,
        );
    }

    fn collect(
        &mut self,
        demand: ZonePreviewDemand,
        result: &ZoneResult,
    ) -> Vec<(ZoneId, PublishedSurface)> {
        collect_gpu_zone_previews(
            &mut self.sparkleflinger,
            demand,
            &result.zone_canvases,
            &result.active_display_zone_ids,
        )
    }

    /// Render, stage, and collect frame after frame until every LED zone has
    /// a finished preview, the way the render thread does across frames.
    fn render_until_previews(
        &mut self,
        demand: ZonePreviewDemand,
    ) -> (ZoneResult, Vec<(ZoneId, PublishedSurface)>) {
        let deadline = Instant::now() + PREVIEW_DEADLINE;
        loop {
            let result = self.render();
            self.stage(demand, &result);
            let previews = self.collect(demand, &result);
            if previews.len() == 2 {
                return (result, previews);
            }
            assert!(
                Instant::now() < deadline,
                "GPU zone previews did not finish: {} of 2 landed",
                previews.len()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn lane_count(&self) -> usize {
        self.sparkleflinger
            .zone_preview_lane_count_for_test()
            .expect("the GPU compositor tracks zone preview lanes")
    }

    fn stage_count(&self) -> usize {
        self.sparkleflinger
            .zone_preview_stage_count_for_test()
            .expect("the GPU compositor counts zone preview stages")
    }
}

#[test]
fn gpu_zone_previews_carry_each_zones_pixels_at_the_demanded_size() {
    let Some(mut scene) = PreviewScene::new() else {
        return;
    };
    let demand = ZonePreviewDemand::from_summary(true, true, false, sized_demand(64, 48));

    let (result, previews) = scene.render_until_previews(demand);

    for zone in [scene.case(), scene.accents()] {
        let frame = zone_frame(&result, zone.id);
        assert!(
            frame.is_gpu_resident(),
            "{} should publish its GPU texture for previews",
            zone.name
        );
    }
    assert!(
        previews
            .iter()
            .all(|(zone_id, _)| *zone_id != scene.face().id),
        "display faces publish their finalized frames, not a GPU zone preview"
    );
    assert_solid_preview(&previews, scene.case(), (64, 48), CASE_RED);
    assert_solid_preview(&previews, scene.accents(), (64, 48), ACCENT_BLUE);

    let sources = zone_preview_sources(&result.zone_canvases, &previews);
    for zone in [scene.case(), scene.accents()] {
        let ProducerFrame::Surface(surface) = zone_frame_in(&sources, zone.id) else {
            panic!("{} should publish its transport-sized preview", zone.name);
        };
        assert_eq!((surface.width(), surface.height()), (64, 48));
    }
}

#[test]
fn gpu_zone_previews_read_back_full_size_only_on_a_full_resolution_ask() {
    let Some(mut scene) = PreviewScene::new() else {
        return;
    };
    let full = PreviewDemandSummary {
        any_full_resolution: true,
        ..sized_demand(64, 48)
    };
    let demand = ZonePreviewDemand::from_summary(true, true, false, full);

    let (_, previews) = scene.render_until_previews(demand);

    assert_solid_preview(&previews, scene.case(), CASE_EXTENT, CASE_RED);
    assert_solid_preview(&previews, scene.accents(), ACCENT_EXTENT, ACCENT_BLUE);
}

#[test]
fn gpu_zone_previews_stage_nothing_without_a_due_subscriber() {
    let Some(mut scene) = PreviewScene::new() else {
        return;
    };
    let unwatched =
        ZonePreviewDemand::from_summary(false, false, false, PreviewDemandSummary::default());
    let not_due = ZonePreviewDemand::from_summary(true, false, false, sized_demand(64, 48));

    for demand in [unwatched, not_due] {
        for _ in 0..4 {
            let result = scene.render();
            scene.stage(demand, &result);
            assert!(scene.collect(demand, &result).is_empty());
        }
    }

    assert_eq!(
        scene.stage_count(),
        0,
        "no zone preview readback was staged"
    );
    assert_eq!(
        scene.lane_count(),
        0,
        "no zone preview resources were allocated"
    );

    let watched = ZonePreviewDemand::from_summary(true, true, false, sized_demand(64, 48));
    let _ = scene.render_until_previews(watched);
    assert_eq!(scene.lane_count(), 2);

    let result = scene.render();
    scene.stage(unwatched, &result);
    assert_eq!(
        scene.lane_count(),
        0,
        "the last subscriber leaving releases every zone preview lane"
    );
}

#[test]
fn unchanged_zone_texture_keeps_its_latched_preview() {
    let Some(mut scene) = PreviewScene::new() else {
        return;
    };
    let result = scene.render();
    let case_id = scene.case().id;
    let frame = zone_frame(&result, case_id).clone();

    assert_eq!(
        scene
            .sparkleflinger
            .stage_zone_preview(case_id, &frame, 32, 32)
            .expect("zone preview should stage"),
        ZonePreviewStage::Staged
    );
    let deadline = Instant::now() + PREVIEW_DEADLINE;
    while scene
        .sparkleflinger
        .zone_preview_surface(case_id)
        .expect("zone preview readback should finish")
        .is_none()
    {
        assert!(Instant::now() < deadline, "zone preview readback timed out");
        std::thread::sleep(Duration::from_millis(1));
    }

    assert_eq!(
        scene
            .sparkleflinger
            .stage_zone_preview(case_id, &frame, 32, 32)
            .expect("zone preview should stage"),
        ZonePreviewStage::Current
    );
    assert_eq!(scene.stage_count(), 1, "unchanged content reads back once");
}

fn assert_solid_preview(
    previews: &[(ZoneId, PublishedSurface)],
    zone: &Zone,
    extent: (u32, u32),
    color: Rgba,
) {
    let surface = previews
        .iter()
        .find_map(|(zone_id, surface)| (*zone_id == zone.id).then_some(surface))
        .unwrap_or_else(|| panic!("{} should have a GPU zone preview", zone.name));
    assert_eq!(
        (surface.width(), surface.height()),
        extent,
        "{} preview size",
        zone.name
    );
    let expected = [color.r, color.g, color.b, color.a];
    let mismatched = surface
        .rgba_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| **pixel != expected)
        .count();
    assert_eq!(
        mismatched, 0,
        "{} preview should carry only its own color {expected:?}",
        zone.name
    );
}

fn zone_frame(result: &ZoneResult, zone_id: ZoneId) -> &ProducerFrame {
    zone_frame_in(&result.zone_canvases, zone_id)
}

fn zone_frame_in(frames: &[(ZoneId, ProducerFrame)], zone_id: ZoneId) -> &ProducerFrame {
    frames
        .iter()
        .find_map(|(id, frame)| (*id == zone_id).then_some(frame))
        .expect("every scene zone should publish a preview source")
}

fn sized_demand(max_width: u32, max_height: u32) -> PreviewDemandSummary {
    PreviewDemandSummary {
        subscribers: 1,
        max_fps: 30,
        max_width,
        max_height,
        ..PreviewDemandSummary::default()
    }
}

fn led_zone(name: &str, output_id: &str, extent: (u32, u32), effect_id: EffectId) -> Zone {
    let mut zone = sample_zone(extent.0, extent.1);
    zone.name = name.into();
    set_effect_zone(&mut zone, effect_id, HashMap::new());
    let mut output = point_zone_at(output_id, 0.5, 0.5);
    output.topology = LedTopology::Strip {
        count: 4,
        direction: StripDirection::LeftToRight,
    };
    zone.layout.zones = vec![output];
    zone
}

fn layer_source(zone: &Zone, source_key: u128, color: Rgba) -> GpuLayerSource {
    GpuLayerSource {
        layer_id: zone.layers[0].id,
        source_key,
        width: zone.layout.canvas_width,
        height: zone.layout.canvas_height,
        color,
    }
}

fn gpu_sparkleflinger() -> Option<SparkleFlinger> {
    match SparkleFlinger::new_required_gpu_for_test() {
        Ok(mut sparkleflinger) => {
            let canvas = sparkleflinger
                .prepare_canvas_resize(SCENE_WIDTH, SCENE_HEIGHT)
                .expect("GPU scene canvas should prepare");
            assert!(canvas.is_admitted());
            sparkleflinger.apply_canvas_resize(canvas);
            Some(sparkleflinger)
        }
        Err(error) => {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU zone preview test was required but initialization failed: {error}"
            );
            None
        }
    }
}
