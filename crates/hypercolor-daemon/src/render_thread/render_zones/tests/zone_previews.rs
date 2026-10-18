//! Studio zone previews of GPU-resident scenes (Spec 86 §5.2).
//!
//! A GPU zone's frame lives in its own GPU texture, so the CPU converter
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
const RED: Rgba = Rgba::new(255, 0, 0, 255);
const GREEN: Rgba = Rgba::new(0, 255, 0, 255);
const BLUE: Rgba = Rgba::new(0, 0, 255, 255);
const WHITE: Rgba = Rgba::new(255, 255, 255, 255);
const YELLOW: Rgba = Rgba::new(255, 255, 0, 255);
/// Top-left, top-right, bottom-left, bottom-right: a flipped, mirrored, or
/// transposed preview moves at least one of them.
const QUADRANTS: [Rgba; 4] = [RED, GREEN, BLUE, WHITE];
const SMALL_PREVIEW: (u32, u32) = (64, 48);
const PREVIEW_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum LayerFill {
    Solid(Rgba),
    Quadrants([Rgba; 4]),
}

struct GpuLayerSource {
    layer_id: SceneLayerId,
    source_key: u128,
    width: u32,
    height: u32,
    fill: LayerFill,
}

struct PreviewScene {
    sparkleflinger: SparkleFlinger,
    runtime: ZoneRuntime,
    registry: EffectRegistry,
    zones: Vec<Zone>,
    sources: Vec<GpuLayerSource>,
    rendered_frames: u64,
}

impl PreviewScene {
    /// Two bilinear LED zones at their own extents beside an LCD face that
    /// composes through the same compositor at a third size.
    fn rig() -> Option<Self> {
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let case = led_zone("Case", "case_leds", CASE_EXTENT, solid_id);
        let accents = led_zone("Accents", "accent_leds", ACCENT_EXTENT, solid_id);
        let mut face = sample_display_zone(FACE_EXTENT, FACE_EXTENT);
        set_effect_zone(&mut face, solid_id, HashMap::new());
        face.layers[0].blend = BlendMode::Alpha;
        let sources = vec![
            layer_source(&case, 1, LayerFill::Quadrants(QUADRANTS)),
            layer_source(&accents, 2, LayerFill::Solid(BLUE)),
            layer_source(&face, 3, LayerFill::Solid(GREEN)),
        ];
        Self::with_zones(registry, vec![case, accents, face], sources)
    }

    /// One zone at the scene's own extent, which renders on the single-zone
    /// path instead of the multi-zone one.
    fn single_zone() -> Option<Self> {
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let zone = led_zone(
            "Primary",
            "primary_leds",
            (SCENE_WIDTH, SCENE_HEIGHT),
            solid_id,
        );
        let sources = vec![layer_source(&zone, 1, LayerFill::Quadrants(QUADRANTS))];
        Self::with_zones(registry, vec![zone], sources)
    }

    fn with_zones(
        registry: EffectRegistry,
        zones: Vec<Zone>,
        sources: Vec<GpuLayerSource>,
    ) -> Option<Self> {
        Some(Self {
            sparkleflinger: gpu_sparkleflinger()?,
            runtime: ZoneRuntime::new(SCENE_WIDTH, SCENE_HEIGHT),
            registry,
            zones,
            sources,
            rendered_frames: 0,
        })
    }

    fn zone(&self, index: usize) -> &Zone {
        &self.zones[index]
    }

    /// Render one frame, feeding every layer a fresh GPU-resident frame the
    /// way a Servo effect does.
    fn render(&mut self) -> ZoneResult {
        for source in &self.sources {
            let canvas = paint(source.fill, source.width, source.height);
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
        .expect("GPU scene should render");
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

    /// Render, stage, and collect frame after frame until `done` accepts the
    /// finished previews, the way the render thread does across frames.
    fn render_until(
        &mut self,
        demand: ZonePreviewDemand,
        done: impl Fn(&[(ZoneId, PublishedSurface)]) -> bool,
    ) -> (ZoneResult, Vec<(ZoneId, PublishedSurface)>) {
        let deadline = Instant::now() + PREVIEW_DEADLINE;
        loop {
            let result = self.render();
            self.stage(demand, &result);
            let previews = self.collect(demand, &result);
            if done(&previews) {
                return (result, previews);
            }
            assert!(
                Instant::now() < deadline,
                "GPU zone previews did not finish: {} landed",
                previews.len()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn render_until_previews(
        &mut self,
        demand: ZonePreviewDemand,
        count: usize,
    ) -> (ZoneResult, Vec<(ZoneId, PublishedSurface)>) {
        self.render_until(demand, |previews| previews.len() == count)
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
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let demand = small_demand();

    let (result, previews) = scene.render_until_previews(demand, 2);

    assert!(matches!(
        result.led_sampling_strategy,
        LedSamplingStrategy::SparkleFlingerZones(_)
    ));
    let (case, accents, face) = (scene.zone(0), scene.zone(1), scene.zone(2));
    for zone in [case, accents] {
        assert!(
            zone_frame(&result, zone.id).is_gpu_resident(),
            "{} should publish its GPU texture for previews",
            zone.name
        );
    }
    assert!(
        previews.iter().all(|(zone_id, _)| *zone_id != face.id),
        "display faces publish their finalized frames, not a GPU zone preview"
    );
    assert_quadrant_preview(&previews, case, SMALL_PREVIEW);
    assert_solid_preview(&previews, accents, SMALL_PREVIEW, BLUE);

    let sources = zone_preview_sources(demand, &result.zone_canvases, &previews);
    for zone in [case, accents] {
        let Some(ProducerFrame::Surface(surface)) = find_frame(&sources, zone.id) else {
            panic!("{} should publish its transport-sized preview", zone.name);
        };
        assert_eq!((surface.width(), surface.height()), SMALL_PREVIEW);
    }
    let case_only = zone_preview_sources(demand, &result.zone_canvases, &previews[..1]);
    let landed = previews[0].0;
    for zone in [case, accents] {
        assert_eq!(
            find_frame(&case_only, zone.id).is_some(),
            zone.id == landed,
            "a GPU zone without a finished preview is left out of publication"
        );
    }
}

#[test]
fn gpu_zone_previews_read_back_full_size_only_on_a_full_resolution_ask() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let full = PreviewDemandSummary {
        any_full_resolution: true,
        ..sized_demand(SMALL_PREVIEW)
    };
    let demand = ZonePreviewDemand::from_summary(true, true, false, full);

    let (_, previews) = scene.render_until_previews(demand, 2);

    assert_quadrant_preview(&previews, scene.zone(0), CASE_EXTENT);
    assert_solid_preview(&previews, scene.zone(1), ACCENT_EXTENT, BLUE);
}

#[test]
fn single_zone_gpu_scene_publishes_its_zone_preview() {
    let Some(mut scene) = PreviewScene::single_zone() else {
        return;
    };

    let (result, previews) = scene.render_until_previews(small_demand(), 1);

    assert!(matches!(
        result.led_sampling_strategy,
        LedSamplingStrategy::SparkleFlinger(_)
    ));
    assert_quadrant_preview(&previews, scene.zone(0), SMALL_PREVIEW);
}

#[test]
fn changed_zone_content_restages_its_preview() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let demand = small_demand();
    let _ = scene.render_until_previews(demand, 2);
    let staged_before = scene.stage_count();

    scene.sources[1].fill = LayerFill::Solid(YELLOW);
    let accents_id = scene.zone(1).id;
    let (_, previews) = scene.render_until(demand, |previews| {
        preview_for(previews, accents_id).is_some_and(|surface| surface.get_pixel(0, 0) == YELLOW)
    });

    assert_solid_preview(&previews, scene.zone(1), SMALL_PREVIEW, YELLOW);
    assert!(scene.stage_count() > staged_before);
}

#[test]
fn gpu_zone_previews_stage_nothing_without_a_due_subscriber() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let unwatched =
        ZonePreviewDemand::from_summary(false, false, false, PreviewDemandSummary::default());
    let not_due = ZonePreviewDemand::from_summary(true, false, false, sized_demand(SMALL_PREVIEW));

    for demand in [unwatched, not_due] {
        for _ in 0..4 {
            let result = scene.render();
            scene.stage(demand, &result);
            assert!(scene.collect(demand, &result).is_empty());
            assert!(matches!(
                zone_preview_sources(demand, &result.zone_canvases, &[]),
                std::borrow::Cow::Borrowed(_)
            ));
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

    let _ = scene.render_until_previews(small_demand(), 2);
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
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let result = scene.render();
    let case_id = scene.zone(0).id;
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

fn assert_quadrant_preview(
    previews: &[(ZoneId, PublishedSurface)],
    zone: &Zone,
    extent: (u32, u32),
) {
    let surface = expect_preview(previews, zone, extent);
    let (width, height) = extent;
    let probes = [
        (width / 4, height / 4),
        (width * 3 / 4, height / 4),
        (width / 4, height * 3 / 4),
        (width * 3 / 4, height * 3 / 4),
    ];
    for ((x, y), expected) in probes.into_iter().zip(QUADRANTS) {
        assert_eq!(
            surface.get_pixel(x, y),
            expected,
            "{} preview quadrant at ({x}, {y})",
            zone.name
        );
    }
}

fn assert_solid_preview(
    previews: &[(ZoneId, PublishedSurface)],
    zone: &Zone,
    extent: (u32, u32),
    color: Rgba,
) {
    let surface = expect_preview(previews, zone, extent);
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

fn expect_preview<'a>(
    previews: &'a [(ZoneId, PublishedSurface)],
    zone: &Zone,
    extent: (u32, u32),
) -> &'a PublishedSurface {
    let surface = preview_for(previews, zone.id)
        .unwrap_or_else(|| panic!("{} should have a GPU zone preview", zone.name));
    assert_eq!(
        (surface.width(), surface.height()),
        extent,
        "{} preview size",
        zone.name
    );
    surface
}

fn preview_for(
    previews: &[(ZoneId, PublishedSurface)],
    zone_id: ZoneId,
) -> Option<&PublishedSurface> {
    previews
        .iter()
        .find_map(|(id, surface)| (*id == zone_id).then_some(surface))
}

fn zone_frame(result: &ZoneResult, zone_id: ZoneId) -> &ProducerFrame {
    find_frame(&result.zone_canvases, zone_id)
        .expect("every scene zone should publish a preview source")
}

fn find_frame(frames: &[(ZoneId, ProducerFrame)], zone_id: ZoneId) -> Option<&ProducerFrame> {
    frames
        .iter()
        .find_map(|(id, frame)| (*id == zone_id).then_some(frame))
}

fn paint(fill: LayerFill, width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    match fill {
        LayerFill::Solid(color) => canvas.fill(color),
        LayerFill::Quadrants(colors) => {
            for y in 0..height {
                for x in 0..width {
                    let right = usize::from(x >= width / 2);
                    let bottom = usize::from(y >= height / 2);
                    canvas.set_pixel(x, y, colors[bottom * 2 + right]);
                }
            }
        }
    }
    canvas
}

fn small_demand() -> ZonePreviewDemand {
    ZonePreviewDemand::from_summary(true, true, false, sized_demand(SMALL_PREVIEW))
}

fn sized_demand((max_width, max_height): (u32, u32)) -> PreviewDemandSummary {
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

fn layer_source(zone: &Zone, source_key: u128, fill: LayerFill) -> GpuLayerSource {
    GpuLayerSource {
        layer_id: zone.layers[0].id,
        source_key,
        width: zone.layout.canvas_width,
        height: zone.layout.canvas_height,
        fill,
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
