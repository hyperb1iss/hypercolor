//! Scoped layers through the render-zone pass, on both compositor backends:
//! which layers each output's LEDs show, where a framed layer's source
//! lands, and what a zone with no scoped layer still costs.

use std::time::{Duration, Instant};

use hypercolor_core::spatial::LayerFraming;
use hypercolor_types::layer::LayerScope;
use hypercolor_types::spatial::Corner;
use hypercolor_types::viewport::FitMode;

use super::gpu_layer_latch::{bilinear_led_zone, gpu_sparkleflinger_at, sample_zone_set};
use super::gpu_zone_acceptance::{color_fill_layer, leds_of, upload_fill, zone_set};
use super::*;
use crate::render_thread::render_zones::scope_classes::{ScopeClassIndex, ZoneClassKey};
use crate::render_thread::sparkleflinger::{ZoneSamplingDispatch, ZoneSamplingRequest};

const SCENE: (u32, u32) = (640, 480);
const RED: Rgba = Rgba::new(255, 0, 0, 255);
const GREEN: Rgba = Rgba::new(0, 255, 0, 255);
const BLUE: Rgba = Rgba::new(0, 0, 255, 255);
const MAGENTA: Rgba = Rgba::new(255, 0, 255, 255);
const FRAMES: u64 = 6;

const BACKENDS: [Backend; 2] = [Backend::Cpu, Backend::Gpu];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Cpu,
    Gpu,
}

/// A zone runtime and a compositor on one backend, rendering scenes the way
/// the render thread does and resolving the LEDs of each.
struct Rig {
    backend: Backend,
    sparkleflinger: SparkleFlinger,
    runtime: ZoneRuntime,
    registry: EffectRegistry,
    solid_id: EffectId,
    zone_colors: Vec<ZoneColors>,
    frame: u64,
}

impl Rig {
    /// `None` when the GPU backend is asked for and no adapter exists.
    fn new(backend: Backend) -> Option<Self> {
        let sparkleflinger = match backend {
            Backend::Cpu => SparkleFlinger::cpu(),
            Backend::Gpu => gpu_sparkleflinger_at(SCENE.0, SCENE.1)?,
        };
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        Some(Self {
            backend,
            sparkleflinger,
            runtime: ZoneRuntime::new(SCENE.0, SCENE.1),
            registry,
            solid_id,
            zone_colors: Vec::new(),
            frame: 0,
        })
    }

    /// A layer whose frame a test supplies each render.
    fn effect_layer(&self) -> SceneLayer {
        effect_layer(self.solid_id, HashMap::new())
    }

    /// Hand `layer` the frame its next render returns.
    fn inject(&mut self, layer: &SceneLayer, canvas: &Canvas) {
        self.runtime
            .inject_effect_layer_frame_for_test(layer.id, ProducerFrame::Canvas(canvas.clone()));
    }

    fn render(&mut self, zones: &[Zone], revision: u64) -> ZoneResult {
        self.frame += 1;
        render_scene_for_test_with_screen_and_sparkleflinger(
            &mut self.runtime,
            zones,
            revision,
            self.frame * 16,
            &HashMap::new(),
            &self.registry,
            &mut self.zone_colors,
            None,
            &mut self.sparkleflinger,
        )
        .unwrap_or_else(|error| panic!("{:?} scene should render: {error:#}", self.backend))
    }

    /// The LED colors `result` resolves to, sampled the way the render
    /// thread samples its strategy.
    fn leds(&mut self, result: &ZoneResult) -> Vec<ZoneColors> {
        match &result.led_sampling_strategy {
            LedSamplingStrategy::SparkleFlingerZones(set) => {
                sample_zone_set(&mut self.sparkleflinger, set)
            }
            LedSamplingStrategy::PreSampled(_) => self.zone_colors.clone(),
            LedSamplingStrategy::SparkleFlinger(engine) => match &result.scene_frame {
                ProducerFrame::GpuTexture(texture) => self
                    .sparkleflinger
                    .sample_texture_zone_plan(texture, engine.sampling_plan().as_ref())
                    .expect("scene frame sampling should run")
                    .expect("scene frame sampling should be admitted"),
                frame => engine.sample(&canvas_from_scene_frame(frame)),
            },
            _ => panic!("a rendered scene should carry a sampling strategy"),
        }
    }

    fn render_leds(&mut self, zones: &[Zone], revision: u64) -> Vec<ZoneColors> {
        let result = self.render(zones, revision);
        self.leds(&result)
    }

    /// The color of `frame` at each normalized position, wherever it lives.
    fn frame_colors(&mut self, frame: &ProducerFrame, positions: &[(f32, f32)]) -> Vec<[u8; 3]> {
        let probe =
            super::gpu_zone_acceptance::nearest_probe(frame.width(), frame.height(), positions);
        let samples = match frame {
            ProducerFrame::GpuTexture(texture) => self
                .sparkleflinger
                .sample_texture_zone_plan(texture, probe.sampling_plan().as_ref())
                .expect("frame probe sampling should run")
                .expect("frame probe sampling should be admitted"),
            frame => probe.sample(&canvas_from_scene_frame(frame)),
        };
        samples.iter().map(|zone| zone.colors[0]).collect()
    }
}

fn scoped(mut layer: SceneLayer, outputs: &[&str]) -> SceneLayer {
    layer.scope = Some(LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    });
    layer
}

fn blended(mut layer: SceneLayer, blend: BlendMode, opacity: f32) -> SceneLayer {
    layer.blend = blend;
    layer.opacity = opacity;
    layer
}

/// A matrix output of `columns` by `rows` LEDs.
fn matrix(id: &str, position: (f32, f32), size: (f32, f32), (columns, rows): (u32, u32)) -> Output {
    let mut output = point_zone(id);
    output.device_id = format!("mock:{id}");
    output.position = NormalizedPosition::new(position.0, position.1);
    output.size = NormalizedPosition::new(size.0, size.1);
    output.topology = LedTopology::Matrix {
        width: columns,
        height: rows,
        serpentine: false,
        start_corner: Corner::TopLeft,
    };
    output
}

fn zone(extent: (u32, u32), outputs: Vec<Output>, layers: Vec<SceneLayer>) -> Zone {
    let mut zone = sample_zone(extent.0, extent.1);
    zone.layers = layers;
    zone.layout.zones = outputs;
    zone.layout.default_sampling_mode = SamplingMode::Nearest;
    zone
}

fn rgb(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b]
}

/// A busy pattern, so a sampling footprint wider than a point mixes colors
/// and a wrong texel shows.
fn busy_canvas(width: u32, height: u32, seed: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let value = |scale: u32| ((x * scale + y * (scale + 7) + seed) % 251) as u8;
            canvas.set_pixel(x, y, Rgba::new(value(13), value(29), value(53), 255));
        }
    }
    canvas
}

/// A smooth pattern whose neighbouring pixels differ by at most one level,
/// so the two backends may disagree on a texel at a boundary and still
/// match within the parity tolerance.
fn smooth_canvas(width: u32, height: u32, seed: u8) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let red = ((x + u32::from(seed)) % 256) as u8;
            let green = ((y * 2 + u32::from(seed)) % 256) as u8;
            let blue = ((u32::midpoint(x, y) + 40) % 256) as u8;
            canvas.set_pixel(x, y, Rgba::new(red, green, blue, 255));
        }
    }
    canvas
}

/// A canvas that tells every pixel where it is: red rises with x, green
/// with y, and blue marks the pixel as part of the source.
fn position_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            canvas.set_pixel(
                x,
                y,
                Rgba::new(
                    (x * 255 / (width - 1)) as u8,
                    (y * 255 / (height - 1)) as u8,
                    255,
                    255,
                ),
            );
        }
    }
    canvas
}

/// The size a layer scoped to `outputs` renders at in `zone`.
fn framed_extent(zone: &Zone, outputs: &[&str]) -> (u32, u32) {
    LayerFraming::of(zone, &scoped(color_fill_layer(MAGENTA), outputs))
        .render_extent((zone.layout.canvas_width, zone.layout.canvas_height))
        .expect("the scope should name a placed output")
}

fn assert_leds_within(actual: &[ZoneColors], expected: &[ZoneColors], tolerance: u8, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: output count");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.zone_id, expected.zone_id, "{what}: output order");
        assert_eq!(actual.colors.len(), expected.colors.len(), "{what}");
        for (led, (actual_color, expected_color)) in
            actual.colors.iter().zip(&expected.colors).enumerate()
        {
            assert!(
                (0..3).all(
                    |channel| actual_color[channel].abs_diff(expected_color[channel]) <= tolerance
                ),
                "{what}: {} led {led}: {actual_color:?} against {expected_color:?}",
                actual.zone_id
            );
        }
    }
}

// ── What a zone with no scoped layer costs ───────────────────────────────

/// What a scene costs the GPU compositor after `FRAMES` frames.
#[derive(Debug, PartialEq, Eq)]
struct SceneCost {
    snapshot_textures: usize,
    compositor_surfaces: usize,
    projected_bind_groups: usize,
    sample_bind_groups: usize,
    summed_area_tables: usize,
    cpu_zone_targets: usize,
    spatial_engines: usize,
    /// LED sampling passes each frame's dispatch encoded.
    sample_passes: Vec<u32>,
}

/// The cost of each scene on the commit before scope classes existed
/// (`ab71c0638`), read off the same counters by this file's first version.
impl SceneCost {
    fn lone_zone_before_scope_classes() -> Self {
        Self {
            snapshot_textures: 4,
            compositor_surfaces: 1,
            projected_bind_groups: 2,
            sample_bind_groups: 1,
            summed_area_tables: 0,
            cpu_zone_targets: 0,
            spatial_engines: 1,
            sample_passes: vec![1; 6],
        }
    }

    fn two_zones_before_scope_classes() -> Self {
        Self {
            snapshot_textures: 5,
            compositor_surfaces: 2,
            projected_bind_groups: 4,
            sample_bind_groups: 2,
            summed_area_tables: 0,
            cpu_zone_targets: 0,
            spatial_engines: 2,
            sample_passes: vec![2; 6],
        }
    }
}

fn scene_cost(
    sparkleflinger: &SparkleFlinger,
    runtime: &ZoneRuntime,
    sample_passes: Vec<u32>,
) -> SceneCost {
    SceneCost {
        snapshot_textures: sparkleflinger
            .snapshot_texture_allocation_count_for_test()
            .expect("a GPU compositor counts its snapshots"),
        compositor_surfaces: sparkleflinger
            .compositor_surface_allocation_count_for_test()
            .expect("a GPU compositor counts its surfaces"),
        projected_bind_groups: sparkleflinger
            .projected_bind_group_creation_count_for_test()
            .expect("a GPU compositor counts its projected bind groups"),
        sample_bind_groups: sparkleflinger
            .sample_bind_group_creation_count_for_test()
            .expect("a GPU compositor counts its sampling bind groups"),
        summed_area_tables: sparkleflinger
            .summed_area_table_allocation_count_for_test()
            .expect("a GPU compositor counts its summed-area tables"),
        cpu_zone_targets: runtime.target_canvases.len(),
        spatial_engines: runtime.spatial_engines.len(),
        sample_passes,
    }
}

/// One zone at the scene extent, stacking a GPU effect under a color fill:
/// the lone-zone path, sampled from the compositor's output.
fn lone_zone(solid_id: EffectId) -> Zone {
    let mut zone = bilinear_led_zone("Main", "main_leds", 0.5, solid_id);
    zone.layout.canvas_width = SCENE.0;
    zone.layout.canvas_height = SCENE.1;
    let mut overlay = color_fill_layer(BLUE);
    overlay.blend = BlendMode::Add;
    zone.layers.push(overlay);
    zone
}

fn sample_output(sparkleflinger: &mut SparkleFlinger, engine: &SpatialEngine) -> Vec<ZoneColors> {
    let plan = engine.sampling_plan();
    let mut colors = Vec::new();
    match sparkleflinger
        .begin_sample_zones_into(
            ZoneSamplingRequest::Output {
                prepared_zones: &plan,
                textures: &[],
            },
            &mut colors,
        )
        .expect("output sampling should dispatch")
    {
        ZoneSamplingDispatch::Ready => {}
        ZoneSamplingDispatch::Pending(mut pending) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !sparkleflinger
                .try_finish_pending_zone_sampling(&mut pending, &mut colors)
                .expect("output readback should finish")
            {
                assert!(Instant::now() < deadline, "output readback timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        ZoneSamplingDispatch::Unsupported | ZoneSamplingDispatch::Saturated => {
            panic!("output sampling should admit a bilinear plan")
        }
    }
    colors
}

/// Render `zone` alone for `FRAMES` frames on the lone-zone path and count
/// what it cost.
fn lone_zone_cost(zone: &Zone) -> Option<(SceneCost, ZoneRuntime)> {
    let mut sparkleflinger = gpu_sparkleflinger_at(SCENE.0, SCENE.1)?;
    let registry = builtin_registry();
    let mut runtime = ZoneRuntime::new(SCENE.0, SCENE.1);
    let mut zone_colors = Vec::new();
    let mut sample_passes = Vec::new();
    for frame in 0..FRAMES {
        let effect_frame = upload_fill(&mut sparkleflinger, 1, SCENE, RED);
        runtime.inject_effect_layer_frame_for_test(zone.layers[0].id, effect_frame);
        let result = render_scene_for_test_with_screen_and_sparkleflinger(
            &mut runtime,
            std::slice::from_ref(zone),
            1,
            frame * 16,
            &HashMap::new(),
            &registry,
            &mut zone_colors,
            None,
            &mut sparkleflinger,
        )
        .expect("lone-zone GPU scene should render");
        assert_eq!(
            result.scene_frame_zone,
            Some(zone.id),
            "frame {frame}: the lone-zone path should still run"
        );
        assert_eq!(result.scope_class_count, 1, "frame {frame}");
        assert!(result.class_frames.zones.is_empty(), "frame {frame}");
        let LedSamplingStrategy::SparkleFlinger(engine) = &result.led_sampling_strategy else {
            panic!("frame {frame}: a lone zone samples the compositor output");
        };
        let _ = sparkleflinger.take_led_sample_passes();
        let leds = sample_output(&mut sparkleflinger, engine);
        sample_passes.push(sparkleflinger.take_led_sample_passes());
        assert_eq!(
            leds_of(&leds, "main_leds"),
            Some(vec![[255, 0, 255]; 4]),
            "frame {frame}"
        );
    }
    Some((
        scene_cost(&sparkleflinger, &runtime, sample_passes),
        runtime,
    ))
}

/// A zone with no scoped layer is one class: one retained texture, one
/// sampling pass, and the lone-zone path, at exactly the allocation and
/// pass counts the commit before scope classes measured.
#[test]
fn a_zone_with_no_scoped_layer_costs_what_it_cost_before_scope_classes() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let Some((cost, runtime)) = lone_zone_cost(&lone_zone(solid_id)) else {
        return;
    };

    assert_eq!(cost, SceneCost::lone_zone_before_scope_classes());
    assert!(runtime.scope_classes.is_empty());
    assert!(runtime.class_spatial_engines.is_empty());
}

/// Two zones with no scoped layer cost what two zones cost before: a
/// texture and a sampling pass each.
#[test]
fn zones_with_no_scoped_layer_keep_one_texture_and_one_pass_each() {
    let Some(mut sparkleflinger) = gpu_sparkleflinger_at(SCENE.0, SCENE.1) else {
        return;
    };
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let zones = [
        bilinear_led_zone("Case", "case_leds", 0.3, solid_id),
        bilinear_led_zone("Accents", "accent_leds", 0.7, solid_id),
    ];
    let mut runtime = ZoneRuntime::new(SCENE.0, SCENE.1);
    let mut zone_colors = Vec::new();
    let mut sample_passes = Vec::new();
    for frame in 0..FRAMES {
        for (index, (zone, color)) in zones.iter().zip([RED, GREEN]).enumerate() {
            let key = u128::try_from(index).expect("zone index fits u128") + 1;
            let extent = (zone.layout.canvas_width, zone.layout.canvas_height);
            let layer_frame = upload_fill(&mut sparkleflinger, key, extent, color);
            runtime.inject_effect_layer_frame_for_test(zone.layers[0].id, layer_frame);
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
        .expect("two-zone GPU scene should render");
        assert_eq!(result.scope_class_count, 2, "frame {frame}");
        let set = zone_set(&result);
        assert_eq!(set.zones.len(), 2, "frame {frame}");
        let _ = sparkleflinger.take_led_sample_passes();
        let leds = sample_zone_set(&mut sparkleflinger, set);
        sample_passes.push(sparkleflinger.take_led_sample_passes());
        assert_eq!(leds_of(&leds, "case_leds"), Some(vec![[255, 0, 0]; 4]));
        assert_eq!(leds_of(&leds, "accent_leds"), Some(vec![[0, 255, 0]; 4]));
    }

    assert_eq!(
        scene_cost(&sparkleflinger, &runtime, sample_passes),
        SceneCost::two_zones_before_scope_classes()
    );
}

/// A layer whose scope names no output of its zone is dormant: it gets no
/// renderer, the zone keeps one class and the lone-zone path, and the
/// layer is never widened to the zone. The cost is the cost of the zone
/// without it.
#[test]
fn a_dormant_layer_costs_nothing_and_reaches_no_output() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut zone = lone_zone(solid_id);
    zone.layers.push(scoped(
        effect_layer(solid_id, HashMap::new()),
        &["keyboard_that_left"],
    ));
    let Some((cost, runtime)) = lone_zone_cost(&zone) else {
        return;
    };

    assert_eq!(cost, SceneCost::lone_zone_before_scope_classes());
    assert_eq!(
        runtime.effect_pool.slot_count(),
        1,
        "the dormant layer should build no renderer"
    );
    let classes = runtime
        .scope_classes
        .get(&zone.id)
        .expect("a zone with a scoped layer keeps its classes");
    assert!(classes.is_single());
    assert!(classes.framing(2).is_dormant());
}

/// An output that exists with no area is still an output: a layer scoped to
/// it reaches its LEDs. Only a scope that names no output at all is
/// dormant.
#[test]
fn a_layer_scoped_to_an_output_with_no_area_still_reaches_it() {
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let base = color_fill_layer(BLUE);
        let overlay = scoped(color_fill_layer(MAGENTA), &["dot"]);
        let scene = zone(
            (96, 64),
            vec![
                matrix("dot", (0.5, 0.5), (0.0, 0.0), (2, 2)),
                matrix("case", (0.5, 0.5), (0.8, 0.8), (3, 3)),
            ],
            vec![base, overlay],
        );

        let result = rig.render(std::slice::from_ref(&scene), 1);
        assert_eq!(result.scope_class_count, 2, "{backend:?}");
        let leds = rig.leds(&result);

        assert_eq!(
            leds_of(&leds, "dot"),
            Some(vec![rgb(MAGENTA); 4]),
            "{backend:?}: the collapsed output should show its layer"
        );
        assert_eq!(
            leds_of(&leds, "case"),
            Some(vec![rgb(BLUE); 9]),
            "{backend:?}: the unscoped output should not change"
        );
    }
}

// ── Which layers an output shows ─────────────────────────────────────────

/// Two outputs whose boxes overlap, and a layer scoped to one of them: the
/// other's LEDs are, bit for bit, what they are with no scoped layer at
/// all, under every sampling mode, because they never sample a texture
/// that holds the layer.
#[test]
fn a_scoped_layer_never_reaches_an_overlapping_output() {
    let modes = [
        SamplingMode::Nearest,
        SamplingMode::Bilinear,
        SamplingMode::AreaAverage {
            radius_x: 3.0,
            radius_y: 3.0,
        },
        SamplingMode::GaussianArea {
            sigma: 2.0,
            radius: 4,
        },
    ];
    for backend in BACKENDS {
        for mode in &modes {
            let Some(mut rig) = Rig::new(backend) else {
                continue;
            };
            let base = rig.effect_layer();
            let overlay = scoped(color_fill_layer(MAGENTA), &["keys"]);
            // The case's box encloses the keyboard's.
            let outputs = vec![
                matrix("keys", (0.5, 0.5), (0.5, 0.4), (6, 4)),
                matrix("case", (0.55, 0.5), (0.8, 0.8), (7, 5)),
            ];
            let mut unscoped = zone((96, 64), outputs.clone(), vec![base.clone()]);
            unscoped.layout.default_sampling_mode = mode.clone();
            let mut with_scope = unscoped.clone();
            with_scope.layers.push(overlay);
            let pattern = busy_canvas(96, 64, 5);

            rig.inject(&base, &pattern);
            let reference = rig.render_leds(std::slice::from_ref(&unscoped), 1);
            rig.inject(&base, &pattern);
            let result = rig.render(std::slice::from_ref(&with_scope), 2);
            assert_eq!(result.scope_class_count, 2, "{backend:?} {mode:?}");
            let leds = rig.leds(&result);

            let what = format!("{backend:?} {mode:?}");
            assert_eq!(
                leds_of(&leds, "case"),
                leds_of(&reference, "case"),
                "{what}: the unscoped output should not change"
            );
            assert_eq!(
                leds_of(&leds, "keys"),
                Some(vec![rgb(MAGENTA); 24]),
                "{what}: the scoped output should show its layer"
            );
            assert_ne!(
                leds_of(&reference, "keys"),
                Some(vec![rgb(MAGENTA); 24]),
                "{what}: the pattern should differ from the overlay"
            );
        }
    }
}

/// Every blend mode composes a scoped layer the same on both backends,
/// within the parity tolerance, with the layer on top of the stack and in
/// the middle of it.
#[test]
fn every_blend_mode_matches_across_backends_for_a_scoped_layer() {
    const MODES: [BlendMode; 11] = [
        BlendMode::Replace,
        BlendMode::Alpha,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::ColorDodge,
        BlendMode::Difference,
        BlendMode::Tint,
        BlendMode::LumaReveal,
    ];
    let Some(mut gpu) = Rig::new(Backend::Gpu) else {
        return;
    };
    let mut cpu = Rig::new(Backend::Cpu).expect("the CPU backend always starts");
    let mut revision = 0;
    for mode in MODES {
        for in_the_middle in [false, true] {
            revision += 1;
            let base = cpu.effect_layer();
            let overlay = scoped(blended(cpu.effect_layer(), mode, 0.7), &["keys"]);
            let mut layers = vec![base.clone(), overlay.clone()];
            if in_the_middle {
                layers.push(blended(
                    color_fill_layer(Rgba::new(40, 90, 200, 255)),
                    BlendMode::Alpha,
                    0.35,
                ));
            }
            let scene = zone(
                (96, 64),
                vec![
                    matrix("keys", (0.4, 0.5), (0.6, 0.3), (12, 6)),
                    matrix("case", (0.5, 0.5), (0.9, 0.9), (9, 7)),
                ],
                layers,
            );
            let extent = framed_extent(&scene, &["keys"]);
            let base_pattern = smooth_canvas(96, 64, 30);
            let overlay_pattern = smooth_canvas(extent.0, extent.1, 120);

            let mut leds = Vec::new();
            for rig in [&mut cpu, &mut gpu] {
                rig.inject(&base, &base_pattern);
                rig.inject(&overlay, &overlay_pattern);
                let result = rig.render(std::slice::from_ref(&scene), revision);
                assert_eq!(result.scope_class_count, 2);
                leds.push(rig.leds(&result));
            }
            let what = format!("{mode:?}, scoped layer in the middle: {in_the_middle}");
            assert_leds_within(&leds[1], &leds[0], 1, &what);
            assert_ne!(
                leds_of(&leds[0], "keys"),
                leds_of(&leds[0], "case"),
                "{what}: the scoped layer should show on its output"
            );
        }
    }
}

/// A scoped layer at the bottom of the stack, with nothing beneath it,
/// lights its own outputs. Every other output is black, also where its box
/// overlaps a scoped one: the zone is active, and nothing is drawn there.
#[test]
fn outputs_outside_a_scoped_bottom_layer_are_black() {
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let scene = zone(
            (96, 64),
            vec![
                matrix("keys", (0.3, 0.5), (0.4, 0.4), (4, 3)),
                matrix("overlapping", (0.4, 0.5), (0.5, 0.6), (4, 3)),
                matrix("apart", (0.85, 0.5), (0.2, 0.6), (2, 3)),
            ],
            vec![scoped(color_fill_layer(RED), &["keys"])],
        );

        for frame in 0..3 {
            let leds = rig.render_leds(std::slice::from_ref(&scene), 1);
            let what = format!("{backend:?} frame {frame}");
            assert_eq!(leds_of(&leds, "keys"), Some(vec![rgb(RED); 12]), "{what}");
            assert_eq!(
                leds_of(&leds, "overlapping"),
                Some(vec![[0, 0, 0]; 12]),
                "{what}"
            );
            assert_eq!(leds_of(&leds, "apart"), Some(vec![[0, 0, 0]; 6]), "{what}");
        }
    }
}

/// Outputs that see different layers each show their own stack, from one
/// render of each layer: a layer under every class is the same pixels
/// under each, and a scoped layer in the middle of the stack sits between
/// the layers around it.
#[test]
fn each_output_shows_its_own_stack_of_layers() {
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        // Red everywhere, green on a and b, a half blue wash everywhere,
        // then white on b alone.
        let scene = zone(
            (96, 64),
            vec![
                matrix("a", (0.2, 0.5), (0.2, 0.4), (2, 2)),
                matrix("b", (0.5, 0.5), (0.2, 0.4), (2, 2)),
                matrix("c", (0.8, 0.5), (0.2, 0.4), (2, 2)),
            ],
            vec![
                color_fill_layer(RED),
                scoped(color_fill_layer(GREEN), &["a", "b"]),
                blended(color_fill_layer(BLUE), BlendMode::Add, 1.0),
                scoped(color_fill_layer(Rgba::new(255, 255, 255, 255)), &["b"]),
            ],
        );

        let result = rig.render(std::slice::from_ref(&scene), 1);
        assert_eq!(result.scope_class_count, 3, "{backend:?}");
        let leds = rig.leds(&result);

        assert_eq!(
            leds_of(&leds, "a"),
            Some(vec![[0, 255, 255]; 4]),
            "{backend:?}"
        );
        assert_eq!(
            leds_of(&leds, "b"),
            Some(vec![[255, 255, 255]; 4]),
            "{backend:?}"
        );
        assert_eq!(
            leds_of(&leds, "c"),
            Some(vec![[255, 0, 255]; 4]),
            "{backend:?}"
        );
        // Sampled colors arrive class by class, and the routing layout
        // lists the outputs in that same order.
        let order = leds
            .iter()
            .map(|zone| zone.zone_id.as_str())
            .collect::<Vec<_>>();
        let routed = rig
            .runtime
            .combined_led_layout
            .zones
            .iter()
            .map(|output| output.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, routed, "{backend:?}");
    }
}

// ── Where a framed layer's source lands ──────────────────────────────────

/// What the placement chain says an LED shows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LedSource {
    /// This point of the source, in source pixels.
    At([f64; 2]),
    /// Nothing: the placed source does not reach the LED.
    OffSource,
    /// Too close to the source's edge to call.
    Edge,
}

/// The source point each LED of `output` shows, from the placement chain
/// alone: the LED's sampled canvas pixel, into the output's own rotated
/// rectangle, then back through the layer's anchor, scale, and rotation.
fn expected_led_sources(
    output: &Output,
    (columns, rows): (u32, u32),
    zone: (u32, u32),
    transform: LayerTransform,
    source: (u32, u32),
) -> Vec<LedSource> {
    let (zone_w, zone_h) = (f64::from(zone.0), f64::from(zone.1));
    let (size_x, size_y) = (f64::from(output.size.x), f64::from(output.size.y));
    let (sin, cos) = f64::from(output.rotation).sin_cos();
    let frame_w = size_x * (cos * zone_w).hypot(sin * zone_h);
    let frame_h = size_y * (sin * zone_w).hypot(cos * zone_h);
    let (layer_sin, layer_cos) = f64::from(transform.rotation).sin_cos();
    let mut sources = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let local_x = f64::from(column) / f64::from(columns - 1);
            let local_y = f64::from(row) / f64::from(rows - 1);
            // Where the sampler puts the LED, and the pixel it reads.
            let offset_x = (local_x - 0.5) * size_x;
            let offset_y = (local_y - 0.5) * size_y;
            let canvas_x = f64::from(output.position.x) + offset_x * cos - offset_y * sin;
            let canvas_y = f64::from(output.position.y) + offset_x * sin + offset_y * cos;
            let pixel_x = (canvas_x * (zone_w - 1.0)).round() + 0.5;
            let pixel_y = (canvas_y * (zone_h - 1.0)).round() + 0.5;
            // That pixel in the frame. A border LED's pixel can sit just
            // past the frame, whose edge runs on.
            let delta_x = pixel_x / zone_w - f64::from(output.position.x);
            let delta_y = pixel_y / zone_h - f64::from(output.position.y);
            let frame_u = ((delta_x * cos + delta_y * sin) / size_x + 0.5).clamp(0.0, 1.0);
            let frame_v = ((-delta_x * sin + delta_y * cos) / size_y + 0.5).clamp(0.0, 1.0);
            // And in the stretched source, through the layer's transform.
            let anchored_x = (frame_u - f64::from(transform.anchor.x)) * frame_w;
            let anchored_y = (frame_v - f64::from(transform.anchor.y)) * frame_h;
            let u = (anchored_x * layer_cos + anchored_y * layer_sin)
                / (f64::from(transform.scale[0]) * frame_w)
                + 0.5;
            let v = (-anchored_x * layer_sin + anchored_y * layer_cos)
                / (f64::from(transform.scale[1]) * frame_h)
                + 0.5;
            let margin = u.min(1.0 - u).min(v).min(1.0 - v);
            sources.push(if margin.abs() < 0.02 {
                LedSource::Edge
            } else if margin < 0.0 {
                LedSource::OffSource
            } else {
                LedSource::At([u * f64::from(source.0), v * f64::from(source.1)])
            });
        }
    }
    sources
}

/// For an output turned by an arbitrary angle on a canvas that is not
/// square, under a layer transform that moves, scales, and turns, every
/// LED shows the source point the placement chain gives it, on both
/// backends. An LED the placed source does not reach keeps what is beneath
/// the layer.
#[test]
fn a_framed_source_lands_on_the_leds_that_sample_it() {
    const GRID: (u32, u32) = (9, 7);
    let transform = LayerTransform {
        anchor: NormalizedPosition::new(0.45, 0.55),
        scale: [0.8, 0.7],
        rotation: 0.35,
        fit: FitMode::Stretch,
    };
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let mut pad = matrix("pad", (0.45, 0.55), (0.5, 0.4), GRID);
        pad.rotation = 0.6;
        let mut layer = scoped(rig.effect_layer(), &["pad"]);
        layer.transform = transform;
        let scene = zone(SCENE, vec![pad.clone()], vec![layer.clone()]);
        let extent = framed_extent(&scene, &["pad"]);
        let expected = expected_led_sources(&pad, GRID, SCENE, transform, extent);

        rig.inject(&layer, &position_canvas(extent.0, extent.1));
        let leds = rig.render_leds(std::slice::from_ref(&scene), 1);
        let colors = leds_of(&leds, "pad").expect("the pad should be sampled");

        let (mut shown, mut dark) = (0, 0);
        for (led, (color, expected)) in colors.iter().zip(&expected).enumerate() {
            match expected {
                LedSource::Edge => {}
                LedSource::OffSource => {
                    dark += 1;
                    assert_eq!(*color, [0, 0, 0], "{backend:?} led {led} is off the source");
                }
                LedSource::At(point) => {
                    shown += 1;
                    let red = point[0].floor() * 255.0 / f64::from(extent.0 - 1);
                    let green = point[1].floor() * 255.0 / f64::from(extent.1 - 1);
                    assert!(
                        (f64::from(color[0]) - red).abs() <= 2.0
                            && (f64::from(color[1]) - green).abs() <= 2.0
                            && color[2] == 255,
                        "{backend:?} led {led}: showed {color:?}, placement puts source \
                         point {point:?} there ({red:.1}, {green:.1})"
                    );
                }
            }
        }
        assert!(
            shown >= 20,
            "{backend:?}: only {shown} LEDs were on the source"
        );
        assert!(
            dark >= 4,
            "{backend:?}: only {dark} LEDs were off the source"
        );
    }
}

/// A layer scoped to two outputs is one source across the bounding box of
/// both: it runs on from the first output into the second.
#[test]
fn a_scope_of_two_outputs_is_continuous_across_both() {
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let strip = |id: &str, x: f32| matrix(id, (x, 0.5), (0.3, 0.2), (5, 1));
        let layer = scoped(rig.effect_layer(), &["left", "right"]);
        let scene = zone(
            SCENE,
            vec![strip("left", 0.25), strip("right", 0.7)],
            vec![layer.clone()],
        );
        let extent = framed_extent(&scene, &["left", "right"]);

        rig.inject(&layer, &position_canvas(extent.0, extent.1));
        let leds = rig.render_leds(std::slice::from_ref(&scene), 1);

        // The frame spans canvas x 0.1 to 0.85.
        let mut last = -1.0;
        for (id, centre) in [("left", 0.25_f32), ("right", 0.7)] {
            let colors = leds_of(&leds, id).expect("the strip should be sampled");
            for (led, color) in colors.iter().enumerate() {
                let canvas_x = centre + (led as f32 / 4.0 - 0.5) * 0.3;
                let expected = (canvas_x - 0.1) / 0.75 * 255.0;
                let red = f32::from(color[0]);
                assert!(
                    (red - expected).abs() <= 3.0,
                    "{backend:?} {id} led {led}: red {red} against {expected}"
                );
                assert!(
                    red >= last,
                    "{backend:?} {id} led {led}: the source runs on"
                );
                last = red;
            }
        }
    }
}

// ── Input ────────────────────────────────────────────────────────────────

/// A canvas that is black but for a block around one point.
fn marker_canvas((width, height): (u32, u32), (centre_x, centre_y): (f32, f32)) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(Rgba::new(0, 0, 0, 255));
    for y in 0..height {
        for x in 0..width {
            if (x as f32 + 0.5 - centre_x).abs() <= 6.0 && (y as f32 + 0.5 - centre_y).abs() <= 6.0
            {
                canvas.set_pixel(x, y, Rgba::new(255, 255, 255, 255));
            }
        }
    }
    canvas
}

/// A finger on a scoped device is told the point of the layer's own source
/// that shows under it: a mark the effect draws at the point it was given
/// lights the LED under the finger, whatever the output's rotation and the
/// layer's own transform. A touch on a device outside the scope carries no
/// point.
#[test]
fn a_touch_is_told_the_source_point_under_the_finger() {
    const GRID: (u32, u32) = (7, 5);
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let pad_device = DeviceId::new();
        let other_device = DeviceId::new();
        rig.runtime.observe_device_bindings(
            1,
            [("mock:pad", pad_device), ("mock:other", other_device)].into_iter(),
        );
        let mut pad = matrix("pad", (0.4, 0.55), (0.45, 0.4), GRID);
        pad.rotation = -0.5;
        let other = matrix("other", (0.8, 0.3), (0.3, 0.3), (3, 3));
        let mut layer = scoped(rig.effect_layer(), &["pad"]);
        layer.transform = LayerTransform {
            anchor: NormalizedPosition::new(0.5, 0.45),
            scale: [1.2, 0.9],
            rotation: -0.25,
            fit: FitMode::Stretch,
        };
        let scene = zone(SCENE, vec![pad, other], vec![layer.clone()]);
        let extent = framed_extent(&scene, &["pad"]);
        rig.inject(&layer, &marker_canvas(extent, (-100.0, -100.0)));
        let _ = rig.render(std::slice::from_ref(&scene), 1);
        let surfaces = rig
            .runtime
            .effect_layer_surfaces_for_test(&scene, 0)
            .expect("the layer should see its zone's device surfaces");

        // The LED two columns in and one row down, well off-centre.
        let (column, row) = (2_u32, 1_u32);
        let touch = (column as f32 / 6.0, row as f32 / 4.0);
        let source = surfaces
            .segment_source_position(pad_device, None, touch.0, touch.1)
            .expect("the finger is on the layer's source");
        let (source_x, source_y) = source.to_pixel(extent.0, extent.1);
        assert_eq!(
            surfaces.segment_source_position(other_device, None, 0.5, 0.5),
            None,
            "{backend:?}: a device outside the scope gets no point"
        );

        rig.inject(
            &layer,
            &marker_canvas(extent, (source_x + 0.5, source_y + 0.5)),
        );
        let leds = rig.render_leds(std::slice::from_ref(&scene), 1);
        let colors = leds_of(&leds, "pad").expect("the pad should be sampled");
        let touched = usize::try_from(row * GRID.0 + column).expect("small grid");
        assert_eq!(
            colors[touched],
            [255, 255, 255],
            "{backend:?}: the mark should light the LED under the finger"
        );
        let lit = colors.iter().filter(|color| **color != [0, 0, 0]).count();
        assert!(
            lit <= 2,
            "{backend:?}: the mark lit {lit} LEDs; it should sit under one finger"
        );
        assert_eq!(
            leds_of(&leds, "other"),
            Some(vec![[0, 0, 0]; 9]),
            "{backend:?}"
        );
    }
}

/// A layer with no scope and a transform is told the point of its own
/// source too. The canvas point alone, which is what it used to be told,
/// is where the LED samples the canvas, not where the layer draws.
#[test]
fn an_unscoped_layer_with_a_transform_is_told_its_own_source_point() {
    const GRID: (u32, u32) = (7, 5);
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let pad_device = DeviceId::new();
        rig.runtime
            .observe_device_bindings(1, [("mock:pad", pad_device)].into_iter());
        let pad = matrix("pad", (0.5, 0.5), (0.6, 0.6), GRID);
        let mut layer = rig.effect_layer();
        layer.transform = LayerTransform {
            anchor: NormalizedPosition::new(0.4, 0.5),
            scale: [0.8, 0.8],
            rotation: 0.3,
            fit: FitMode::Stretch,
        };
        let scene = zone(SCENE, vec![pad], vec![layer.clone()]);
        rig.inject(&layer, &marker_canvas(SCENE, (-100.0, -100.0)));
        let _ = rig.render(std::slice::from_ref(&scene), 1);
        let surfaces = rig
            .runtime
            .effect_layer_surfaces_for_test(&scene, 0)
            .expect("the layer should see its zone's device surfaces");

        let (column, row) = (4_u32, 3_u32);
        let touch = (column as f32 / 6.0, row as f32 / 4.0);
        let canvas = surfaces
            .segment_canvas_position(pad_device, None, touch.0, touch.1)
            .expect("the pad is placed");
        let source = surfaces
            .segment_source_position(pad_device, None, touch.0, touch.1)
            .expect("the finger is on the layer's source");
        assert!(
            (canvas.x - source.x).abs() > 0.02 || (canvas.y - source.y).abs() > 0.02,
            "{backend:?}: the transform should move the point"
        );

        let (source_x, source_y) = source.to_pixel(SCENE.0, SCENE.1);
        rig.inject(
            &layer,
            &marker_canvas(SCENE, (source_x + 0.5, source_y + 0.5)),
        );
        let leds = rig.render_leds(std::slice::from_ref(&scene), 1);
        let colors = leds_of(&leds, "pad").expect("the pad should be sampled");
        let touched = usize::try_from(row * GRID.0 + column).expect("small grid");
        assert_eq!(
            colors[touched],
            [255, 255, 255],
            "{backend:?}: the mark should light the LED under the finger"
        );
    }
}

// ── Pictures for people, and screens ─────────────────────────────────────

/// The scene canvas and a zone's preview show each output's box from its
/// own class, and a screen's backdrop is the frame of the class of the
/// output that places it.
#[test]
fn previews_and_backdrops_show_each_output_from_its_class() {
    for backend in BACKENDS {
        let Some(mut rig) = Rig::new(backend) else {
            continue;
        };
        let mut screen = matrix("screen", (0.75, 0.25), (0.3, 0.3), (1, 1));
        screen.zone_name = Some(hypercolor_core::spatial::DISPLAY_ZONE_NAME.to_owned());
        let scene = zone(
            SCENE,
            vec![
                matrix("keys", (0.25, 0.5), (0.3, 0.4), (2, 2)),
                matrix("case", (0.6, 0.8), (0.3, 0.2), (2, 2)),
                screen,
            ],
            vec![
                color_fill_layer(RED),
                scoped(color_fill_layer(GREEN), &["keys"]),
                scoped(color_fill_layer(BLUE), &["screen"]),
            ],
        );
        // In the middle of each box, and where no output sits.
        let spots = [(0.25, 0.5), (0.6, 0.8), (0.75, 0.25), (0.05, 0.05)];

        let first = rig.render(std::slice::from_ref(&scene), 1);
        assert!(
            first.class_frames.pictures.is_empty(),
            "{backend:?}: a picture is made only for a preview"
        );
        rig.runtime.request_zone_pictures(|_| true);
        let result = rig.render(std::slice::from_ref(&scene), 1);
        assert_eq!(result.scope_class_count, 3, "{backend:?}");

        // The scene canvas, composed from the classes. The zone is the
        // scene's only one and has its extent, so its base class shows
        // where no output sits, as the zone's canvas always did.
        assert_eq!(
            rig.frame_colors(&result.scene_frame, &spots),
            vec![rgb(GREEN), rgb(RED), rgb(BLUE), rgb(RED)],
            "{backend:?}: scene canvas"
        );

        // The zone's own preview: its base class everywhere, each other
        // output's box from that output's class.
        let [(pictured_zone, picture)] = &result.class_frames.pictures[..] else {
            panic!("{backend:?}: the previewed zone should have one picture");
        };
        assert_eq!(*pictured_zone, scene.id);
        let picture = picture.clone();
        assert_eq!(
            rig.frame_colors(&picture, &spots),
            vec![rgb(GREEN), rgb(RED), rgb(BLUE), rgb(RED)],
            "{backend:?}: zone preview"
        );
        let previews = crate::render_thread::zone_previews::zone_preview_frames(
            &result.zone_canvases,
            &result.class_frames,
        );
        assert_eq!(previews.len(), 1, "{backend:?}");
        assert_eq!(
            rig.frame_colors(&previews[0].1, &spots[..1]),
            vec![rgb(GREEN)],
            "{backend:?}: the preview is made from the picture"
        );

        // The screen's backdrop source: the frame of its output's class,
        // which holds the screen's layer over its box. Around the box the
        // layer's edge runs on, so nothing there says what the base is.
        let screen_class = result.class_frames.class_of(scene.id, Some(2));
        assert!(!screen_class.is_base(), "{backend:?}");
        let backdrop = result
            .class_frames
            .frame(ZoneClassKey::new(scene.id, screen_class))
            .expect("the screen's class should have a frame")
            .clone();
        assert_eq!(
            rig.frame_colors(&backdrop, &spots[2..3]),
            vec![rgb(BLUE)],
            "{backend:?}: the screen's class shows the screen's layer on its box"
        );
        assert!(
            result
                .class_frames
                .revision(ZoneClassKey::new(scene.id, screen_class))
                .is_some(),
            "{backend:?}: a class frame carries its content revision"
        );
        assert_eq!(
            result.class_frames.class_of(scene.id, None),
            ScopeClassIndex::BASE,
            "{backend:?}: a screen that shows the whole canvas reads the base class"
        );
        let base = result
            .zone_canvases
            .iter()
            .find_map(|(zone_id, frame)| (*zone_id == scene.id).then_some(frame.clone()))
            .expect("the zone's frame is its base class");
        assert_eq!(
            rig.frame_colors(&base, &spots),
            vec![rgb(RED); 4],
            "{backend:?}: base class"
        );
    }
}

// ── Admission ────────────────────────────────────────────────────────────

/// When the GPU refuses a scene's class textures, the scene takes the same
/// fallback a refused zone texture takes: it composes and samples on the
/// CPU, each class in a canvas of its own from the first frame, and no
/// scope is widened and no layer lost.
#[test]
fn refused_class_textures_fall_back_like_refused_zone_textures() {
    let Some(mut rig) = Rig::new(Backend::Gpu) else {
        return;
    };
    // Three stacks of two layers and more, all at one size: what a readback
    // latch keyed by size would mix up.
    let scene = zone(
        (96, 64),
        vec![
            matrix("keys", (0.3, 0.5), (0.4, 0.4), (3, 2)),
            matrix("pads", (0.5, 0.5), (0.4, 0.4), (3, 2)),
            matrix("case", (0.4, 0.5), (0.7, 0.7), (3, 2)),
        ],
        vec![
            color_fill_layer(RED),
            blended(
                color_fill_layer(Rgba::new(0, 0, 0, 255)),
                BlendMode::Add,
                1.0,
            ),
            scoped(color_fill_layer(GREEN), &["keys"]),
            scoped(color_fill_layer(BLUE), &["pads"]),
        ],
    );
    rig.sparkleflinger
        .fail_next_projected_scene_preparation_for_test();

    for frame in 0..4 {
        let result = rig.render(std::slice::from_ref(&scene), 1);
        assert!(
            matches!(
                result.led_sampling_strategy,
                LedSamplingStrategy::PreSampled(_)
            ),
            "frame {frame}: a scene without its class textures samples on the CPU"
        );
        assert_eq!(result.scope_class_count, 3, "frame {frame}");
        let leds = rig.leds(&result);
        assert_eq!(
            leds_of(&leds, "keys"),
            Some(vec![rgb(GREEN); 6]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "pads"),
            Some(vec![rgb(BLUE); 6]),
            "frame {frame}"
        );
        assert_eq!(
            leds_of(&leds, "case"),
            Some(vec![rgb(RED); 6]),
            "frame {frame}"
        );
    }
    assert_eq!(
        rig.runtime.target_canvases.len(),
        3,
        "each class composes into a CPU canvas of its own"
    );
}

/// Without a texture per class, a scoped zone whose layers live on the GPU
/// has no way to keep its scopes apart: the only road from a GPU layer to a
/// CPU canvas is a readback latch that cannot tell one class from another.
/// The frame fails and says so, instead of showing a layer on outputs
/// outside its scope.
#[test]
fn a_scoped_zone_with_gpu_layers_fails_loudly_without_its_class_textures() {
    let Some(mut rig) = Rig::new(Backend::Gpu) else {
        return;
    };
    let base = rig.effect_layer();
    let scene = zone(
        (96, 64),
        vec![
            matrix("keys", (0.3, 0.5), (0.4, 0.4), (3, 2)),
            matrix("case", (0.4, 0.5), (0.7, 0.7), (3, 2)),
        ],
        vec![base.clone(), scoped(color_fill_layer(GREEN), &["keys"])],
    );
    rig.sparkleflinger
        .fail_next_projected_scene_preparation_for_test();
    let gpu_frame = upload_fill(&mut rig.sparkleflinger, 3, (96, 64), RED);
    rig.runtime
        .inject_effect_layer_frame_for_test(base.id, gpu_frame);

    let error = render_scene_for_test_with_screen_and_sparkleflinger(
        &mut rig.runtime,
        std::slice::from_ref(&scene),
        1,
        16,
        &HashMap::new(),
        &rig.registry,
        &mut rig.zone_colors,
        None,
        &mut rig.sparkleflinger,
    )
    .err()
    .expect("a scoped zone with GPU layers and no class textures should fail");

    assert!(
        error
            .downcast_ref::<crate::render_thread::render_zones::render_pass::ScopeClassesNeedGpuTextures>()
            .is_some(),
        "{error:#}"
    );
}

/// A zone that changes size takes its classes with it: every class texture
/// a frame samples is at the zone's size for that frame, and a framed layer
/// renders at the extent its frame has on the new canvas.
#[test]
fn a_resized_zone_never_samples_a_class_at_the_old_extent() {
    let Some(mut rig) = Rig::new(Backend::Gpu) else {
        return;
    };
    let layer = scoped(rig.effect_layer(), &["keys"]);
    let mut scene = zone(
        (320, 200),
        vec![
            matrix("keys", (0.3, 0.5), (0.4, 0.4), (3, 2)),
            matrix("case", (0.4, 0.5), (0.7, 0.7), (3, 2)),
        ],
        vec![color_fill_layer(RED), layer.clone()],
    );
    let mut extents = Vec::new();
    for (revision, size) in [(1, (320, 200)), (2, (640, 630)), (3, (160, 100))] {
        scene.layout.canvas_width = size.0;
        scene.layout.canvas_height = size.1;
        let extent = framed_extent(&scene, &["keys"]);
        extents.push(extent);
        for frame in 0..3 {
            let mut overlay = Canvas::new(extent.0, extent.1);
            overlay.fill(GREEN);
            rig.inject(&layer, &overlay);
            let result = rig.render(std::slice::from_ref(&scene), revision);
            let set = zone_set(&result);
            assert_eq!(set.gpu_textures.len(), 2, "{size:?} frame {frame}");
            for texture in &set.gpu_textures {
                assert_eq!(
                    (texture.frame.width, texture.frame.height),
                    size,
                    "{size:?} frame {frame}: a class was sampled at another extent"
                );
            }
            let leds = rig.leds(&result);
            assert_eq!(
                leds_of(&leds, "keys"),
                Some(vec![rgb(GREEN); 6]),
                "{size:?} frame {frame}"
            );
            assert_eq!(
                leds_of(&leds, "case"),
                Some(vec![rgb(RED); 6]),
                "{size:?} frame {frame}"
            );
        }
    }
    assert_eq!(extents, vec![(320, 200), (640, 630), (160, 100)]);
}

/// A scoped overlay over a base that holds still costs one composition
/// pass per frame: the base class keeps its held frame, and the class that
/// adds the overlay starts from it.
#[test]
fn an_unchanged_class_reuses_its_held_frame() {
    let Some(mut rig) = Rig::new(Backend::Gpu) else {
        return;
    };
    let layer = scoped(rig.effect_layer(), &["keys"]);
    let scene = zone(
        (96, 64),
        vec![
            matrix("keys", (0.3, 0.5), (0.4, 0.4), (3, 2)),
            matrix("case", (0.7, 0.5), (0.4, 0.4), (3, 2)),
        ],
        vec![color_fill_layer(RED), layer.clone()],
    );
    let extent = framed_extent(&scene, &["keys"]);
    let upload = |rig: &mut Rig, shade: u8| {
        let mut overlay = Canvas::new(extent.0, extent.1);
        overlay.fill(Rgba::new(0, shade, 0, 255));
        rig.sparkleflinger
            .upload_media_canvas_frame(
                crate::render_thread::sparkleflinger::MediaTextureSourceKey::for_test(7),
                &overlay,
            )
            .expect("overlay frame should upload")
    };
    let dim = upload(&mut rig, 10);
    let mut skips = Vec::new();
    let mut generations = Vec::new();
    for frame in 0..3 {
        // The same producer frame twice, then a new one.
        let (overlay, shade) = if frame < 2 {
            (dim.clone(), 10)
        } else {
            (upload(&mut rig, 200), 200)
        };
        rig.runtime
            .inject_effect_layer_frame_for_test(layer.id, ProducerFrame::GpuTexture(overlay));
        let result = rig.render(std::slice::from_ref(&scene), 1);
        skips.push(result.zone_composition_skips);
        let set = zone_set(&result);
        generations.push(
            set.gpu_textures
                .iter()
                .map(|texture| texture.frame.content_generation)
                .collect::<Vec<_>>(),
        );
        let leds = rig.leds(&result);
        assert_eq!(leds_of(&leds, "keys"), Some(vec![[0, shade, 0]; 6]));
        assert_eq!(leds_of(&leds, "case"), Some(vec![rgb(RED); 6]));
    }

    // The same overlay frame skips both classes; a new one composes its
    // own class again and leaves the base class held.
    assert_eq!(skips, vec![0, 2, 1]);
    assert_eq!(generations[1], generations[0]);
    assert_eq!(generations[2][0], generations[0][0], "the base class held");
    assert!(generations[2][1] > generations[0][1], "{generations:?}");
}
