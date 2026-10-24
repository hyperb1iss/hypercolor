//! Studio zone previews of GPU-resident scenes (Spec 86 §5.2).
//!
//! A GPU zone's frame lives in its own GPU texture, so the CPU converter
//! cannot publish it. Each zone's texture is scaled on the GPU to the zone
//! preview demand and read back at that size only; nothing is staged while
//! nobody watches zone previews. The scene preview of the same scenes keeps
//! landing beside them while every zone resizes the compositor (§8 criterion
//! 9).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use super::gpu_layer_latch::sample_zone_set;
use super::*;
use crate::preview_runtime::{PreviewDemandSummary, ZonePreviewSelection};
use crate::render_thread::sparkleflinger::{
    CompositionLayer, CompositionMode, MediaTextureSourceKey, ZonePreviewStage,
};
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
const PREVIEW_DEADLINE: Duration = Duration::from_secs(10);
/// The UI scene preview's transport size: smaller than the scene, so the
/// compositor scales it on the GPU before reading it back.
const SCENE_PREVIEW_REQUEST: PreviewSurfaceRequest = PreviewSurfaceRequest {
    width: 160,
    height: 100,
};
const LAGGED_FRAMES: u64 = 24;
const LAGGED_WARMUP_FRAMES: u64 = 3;
const FRAME_PACING: Duration = Duration::from_millis(16);

/// What one frame of [`PreviewScene::preview_frame_a_frame_behind`] produced.
struct LaggedPreviewFrame {
    /// A scene preview map pending when the frame began was still pending
    /// after the frame's zones and LCD face composed.
    carried_map_survived: bool,
    scene_preview: Option<PublishedSurface>,
    zone_previews: Vec<(ZoneId, PublishedSurface)>,
    leds: Vec<ZoneColors>,
}

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

    /// A placed LED zone beside a zone with layers but no members yet, the
    /// way Studio creates one before any device joins it.
    fn with_memberless_zone() -> Option<Self> {
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let case = led_zone("Case", "case_leds", CASE_EXTENT, solid_id);
        let mut memberless = led_zone("New Zone", "unplaced", CASE_EXTENT, solid_id);
        memberless.layout.zones.clear();
        let sources = vec![
            layer_source(&case, 1, LayerFill::Quadrants(QUADRANTS)),
            layer_source(&memberless, 2, LayerFill::Solid(YELLOW)),
        ];
        Self::with_zones(registry, vec![case, memberless], sources)
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

    fn stage(&mut self, demand: &ZonePreviewDemand, result: &ZoneResult) {
        stage_gpu_zone_previews(
            &mut self.sparkleflinger,
            demand,
            &result.zone_canvases,
            &result.active_display_zone_ids,
        );
    }

    fn collect(
        &mut self,
        demand: &ZonePreviewDemand,
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
        demand: &ZonePreviewDemand,
        done: impl Fn(&[(ZoneId, PublishedSurface)]) -> bool,
    ) -> (ZoneResult, Vec<(ZoneId, PublishedSurface)>) {
        let deadline = Instant::now() + PREVIEW_DEADLINE;
        loop {
            self.sparkleflinger.wait_for_gpu_frame_budget();
            let result = self.render();
            self.stage(demand, &result);
            let previews = self.collect(demand, &result);
            self.sparkleflinger.end_gpu_frame();
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
        demand: &ZonePreviewDemand,
        count: usize,
    ) -> (ZoneResult, Vec<(ZoneId, PublishedSurface)>) {
        self.render_until(demand, |previews| previews.len() == count)
    }

    /// Run one frame's preview work after `render`, in render-thread order,
    /// on a GPU that runs a frame behind: a scene preview map begun during
    /// this frame finishes no earlier than the next frame.
    ///
    /// The frame composer replaces the canvas with the scene frame and asks
    /// for a transport-sized scene preview; the preview advances right after
    /// composition, the LEDs sample their zone set, zone previews stage, and
    /// the scene preview finalizes before zone previews are collected.
    /// `map_carried` says whether a scene preview map was still pending when
    /// this frame began rendering.
    fn preview_frame_a_frame_behind(
        &mut self,
        demand: &ZonePreviewDemand,
        result: &ZoneResult,
        map_carried: bool,
    ) -> LaggedPreviewFrame {
        let plan = CompositionPlan::single(
            SCENE_WIDTH,
            SCENE_HEIGHT,
            CompositionLayer::from_parts(
                result.scene_frame.clone(),
                CompositionMode::Replace,
                1.0,
                true,
            ),
        );
        let composed =
            self.sparkleflinger
                .compose_for_outputs(plan, false, Some(SCENE_PREVIEW_REQUEST));
        let carried_map_survived =
            map_carried && self.sparkleflinger.has_pending_preview_map_for_test();
        let mut scene_preview = composed.preview_surface;
        if scene_preview.is_none() {
            scene_preview = self.resolve_scene_preview_a_frame_behind(carried_map_survived);
        }
        let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
            panic!("a GPU multi-zone scene should sample each zone's own texture");
        };
        let leds = sample_zone_set(&mut self.sparkleflinger, set);
        self.stage(demand, result);
        if scene_preview.is_none() {
            scene_preview = self.resolve_scene_preview_a_frame_behind(
                carried_map_survived && self.sparkleflinger.has_pending_preview_map_for_test(),
            );
        }
        let zone_previews = self.collect(demand, result);
        LaggedPreviewFrame {
            carried_map_survived,
            scene_preview,
            zone_previews,
            leds,
        }
    }

    /// Resolve the scene preview once, holding back a map begun this frame.
    fn resolve_scene_preview_a_frame_behind(
        &mut self,
        map_from_earlier_frame: bool,
    ) -> Option<PublishedSurface> {
        self.sparkleflinger
            .submit_pending_preview_work()
            .expect("scene preview work should submit");
        if self.sparkleflinger.has_pending_preview_map_for_test() && !map_from_earlier_frame {
            self.sparkleflinger
                .defer_next_preview_map_resolve_for_test();
        }
        self.sparkleflinger
            .resolve_preview_surface()
            .expect("scene preview should resolve")
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

    let (result, previews) = scene.render_until_previews(&demand, 2);

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

    let sources = zone_preview_sources(&demand, &result.zone_canvases, &previews);
    for zone in [case, accents] {
        let Some(ProducerFrame::Surface(surface)) = find_frame(&sources, zone.id) else {
            panic!("{} should publish its transport-sized preview", zone.name);
        };
        assert_eq!((surface.width(), surface.height()), SMALL_PREVIEW);
    }
    let case_only = zone_preview_sources(&demand, &result.zone_canvases, &previews[..1]);
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

    let (_, previews) = scene.render_until_previews(&demand, 2);

    assert_quadrant_preview(&previews, scene.zone(0), CASE_EXTENT);
    assert_solid_preview(&previews, scene.zone(1), ACCENT_EXTENT, BLUE);
}

#[test]
fn single_zone_gpu_scene_publishes_its_zone_preview() {
    let Some(mut scene) = PreviewScene::single_zone() else {
        return;
    };

    let (result, previews) = scene.render_until_previews(&small_demand(), 1);

    assert!(matches!(
        result.led_sampling_strategy,
        LedSamplingStrategy::SparkleFlinger(_)
    ));
    assert_quadrant_preview(&previews, scene.zone(0), SMALL_PREVIEW);
}

/// A zone with no members places nothing in the scene canvas, but its zone
/// preview still shows its layers, as it does on the CPU compositor, and
/// the placed zone beside it keeps sampling and previewing its own texture.
#[test]
fn memberless_zone_previews_its_own_layers() {
    let Some(mut scene) = PreviewScene::with_memberless_zone() else {
        return;
    };

    let (result, previews) = scene.render_until_previews(&small_demand(), 2);

    let LedSamplingStrategy::SparkleFlingerZones(set) = &result.led_sampling_strategy else {
        panic!("a scene with a memberless zone should stay GPU-resident");
    };
    let leds = sample_zone_set(&mut scene.sparkleflinger, set);
    assert_eq!(leds.len(), 1, "only the placed zone has LEDs to sample");
    assert_eq!(leds[0].zone_id, "case_leds");
    assert_quadrant_preview(&previews, scene.zone(0), SMALL_PREVIEW);
    assert_solid_preview(&previews, scene.zone(1), SMALL_PREVIEW, YELLOW);
}

#[test]
fn changed_zone_content_restages_its_preview() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let demand = small_demand();
    let _ = scene.render_until_previews(&demand, 2);
    let staged_before = scene.stage_count();

    scene.sources[1].fill = LayerFill::Solid(YELLOW);
    let accents_id = scene.zone(1).id;
    let (_, previews) = scene.render_until(&demand, |previews| {
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

    for demand in [unwatched.clone(), not_due] {
        for _ in 0..4 {
            let result = scene.render();
            scene.stage(&demand, &result);
            assert!(scene.collect(&demand, &result).is_empty());
            assert!(matches!(
                zone_preview_sources(&demand, &result.zone_canvases, &[]),
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

    let _ = scene.render_until_previews(&small_demand(), 2);
    assert_eq!(scene.lane_count(), 2);

    let result = scene.render();
    scene.stage(&unwatched, &result);
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

/// A subscriber that selects one zone gets that zone's preview, and the
/// other zone is never staged or read back. Narrowing the selection
/// releases the deselected zone's preview lane, and selecting nothing
/// releases every lane while a subscriber still watches.
#[test]
fn gpu_zone_previews_stage_only_selected_zones() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let (case_id, accents_id) = (scene.zone(0).id, scene.zone(1).id);
    let both = small_demand();
    let _ = scene.render_until_previews(&both, 2);
    assert_eq!(scene.lane_count(), 2);

    let case_only = small_demand().with_selection(ZonePreviewSelection::from_zone_ids([case_id]));
    let result = scene.render();
    scene.stage(&case_only, &result);
    assert_eq!(
        scene.lane_count(),
        1,
        "deselecting a zone releases its preview lane"
    );
    // Both zones change, so the selected zone must restage to show its new
    // color while the deselected zone's change never reaches a preview.
    let staged_before = scene.stage_count();
    scene.sources[0].fill = LayerFill::Solid(YELLOW);
    scene.sources[1].fill = LayerFill::Solid(RED);
    let (_, previews) = scene.render_until(&case_only, |previews| {
        preview_for(previews, case_id).is_some_and(|surface| surface.get_pixel(0, 0) == YELLOW)
    });
    assert!(preview_for(&previews, accents_id).is_none());
    assert_solid_preview(&previews, scene.zone(0), SMALL_PREVIEW, YELLOW);
    assert!(
        scene.stage_count() > staged_before,
        "the selected zone keeps staging"
    );
    assert_eq!(scene.lane_count(), 1);

    let none = small_demand().with_selection(ZonePreviewSelection::from_zone_ids([]));
    let result = scene.render();
    scene.stage(&none, &result);
    assert!(scene.collect(&none, &result).is_empty());
    assert_eq!(scene.lane_count(), 0, "an empty selection stages nothing");
}

/// The scene preview lands on the next frame, carrying the previous frame's
/// pixels, when the GPU has not finished its readback by the end of the
/// frame that staged it, even though every frame composes both LED zones
/// and the LCD face at their own extents before composing the preview
/// again, and zone previews keep landing beside it (Spec 86 §8 criterion
/// 9). The accent zone changes color every frame, so each preview names the
/// frame it came from. A compositor that cancels the pending
/// preview readback on each size switch starves the scene preview forever
/// on any GPU that runs a frame behind, and one that rebuilds the preview's
/// readback set or scale bind groups on each switch allocates every frame.
#[test]
fn late_scene_preview_readbacks_still_land_while_zones_resize_the_compositor() {
    let Some(mut scene) = PreviewScene::rig() else {
        return;
    };
    let demand = small_demand();
    let (case_id, accents_id) = (scene.zone(0).id, scene.zone(1).id);
    let mut scene_previews = 0_u64;
    let mut carried_maps = 0_u64;
    let mut surviving_maps = 0_u64;
    let mut zone_preview_frames: HashMap<ZoneId, u64> = HashMap::new();
    let mut failures = Vec::new();
    let mut trace = String::new();
    let mut warm_allocations = None;

    let accent_color = |frame: u64| {
        if frame.is_multiple_of(2) {
            BLUE
        } else {
            YELLOW
        }
    };

    for frame in 0..LAGGED_FRAMES {
        if frame == LAGGED_WARMUP_FRAMES {
            warm_allocations = scene.sparkleflinger.preview_resource_allocations_for_test();
        }
        scene.sources[1].fill = LayerFill::Solid(accent_color(frame));
        let map_carried = scene.sparkleflinger.has_pending_preview_map_for_test();
        let result = scene.render();
        let lagged = scene.preview_frame_a_frame_behind(&demand, &result, map_carried);
        carried_maps += u64::from(map_carried);
        surviving_maps += u64::from(lagged.carried_map_survived);
        let _ = write!(
            trace,
            "frame {frame}: carried={map_carried} survived={} scene_preview={} zone_previews={};",
            lagged.carried_map_survived,
            lagged.scene_preview.is_some(),
            lagged.zone_previews.len(),
        );

        if let Some(preview) = &lagged.scene_preview {
            scene_previews += 1;
            let size = (preview.width(), preview.height());
            if size != (SCENE_PREVIEW_REQUEST.width, SCENE_PREVIEW_REQUEST.height) {
                failures.push(format!("frame {frame}: scene preview arrived at {size:?}"));
            }
            let center = preview.get_pixel(preview.width() / 2, preview.height() / 2);
            let staged = accent_color(frame.saturating_sub(1));
            if frame == 0 || center != staged {
                failures.push(format!(
                    "frame {frame}: scene preview center is {center:?}, not the accent color \
                     {staged:?} the previous frame staged"
                ));
            }
        }
        for (zone_id, surface) in &lagged.zone_previews {
            *zone_preview_frames.entry(*zone_id).or_default() += 1;
            if (surface.width(), surface.height()) != SMALL_PREVIEW {
                failures.push(format!(
                    "frame {frame}: zone preview arrived at {}x{}",
                    surface.width(),
                    surface.height()
                ));
            }
        }
        let accent_leds = lagged
            .leds
            .iter()
            .find(|zone| zone.zone_id == "accent_leds")
            .map(|zone| zone.colors.clone())
            .unwrap_or_default();
        let accent = accent_color(frame);
        if accent_leds.is_empty()
            || accent_leds
                .iter()
                .any(|color| *color != [accent.r, accent.g, accent.b])
        {
            failures.push(format!(
                "frame {frame}: accent LEDs sampled {:?}",
                accent_leds.first()
            ));
        }
        let _ = writeln!(trace);
        std::thread::sleep(FRAME_PACING);
    }

    assert!(
        failures.is_empty(),
        "previews or LEDs went wrong:\n{}\ntrace:\n{trace}",
        failures.join("\n")
    );
    assert!(
        carried_maps > 0,
        "the frame-behind GPU model never carried a scene preview map into a frame:\n{trace}"
    );
    assert_eq!(
        surviving_maps, carried_maps,
        "a pending scene preview map was cancelled by the next frame's zone and face \
         composition:\n{trace}"
    );
    assert!(
        scene_previews + 1 >= LAGGED_FRAMES,
        "only {scene_previews} of {LAGGED_FRAMES} frames delivered the previous frame's scene \
         preview:\n{trace}"
    );
    assert_eq!(
        scene.sparkleflinger.preview_resource_allocations_for_test(),
        warm_allocations,
        "switching compositor sizes reallocated scene preview readback sets or scale bind \
         groups after warmup"
    );
    for zone_id in [case_id, accents_id] {
        let landed = zone_preview_frames.get(&zone_id).copied().unwrap_or(0);
        assert!(
            landed * 3 >= LAGGED_FRAMES,
            "zone {zone_id} delivered a preview on only {landed} of {LAGGED_FRAMES} frames:\n{trace}"
        );
    }
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
