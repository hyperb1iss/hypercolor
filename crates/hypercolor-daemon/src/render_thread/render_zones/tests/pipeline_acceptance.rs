//! Spec 86 §8 acceptance cases that need the whole frame pipeline on the
//! GPU backend: the render-zone pass, LED sampling resolved against the
//! published frame, and the output stage that writes devices. The harness
//! drives `execute_frame` one frame at a time, so a case can inject a
//! failure or keep a layer loading between frames, which the render
//! thread's own loop cannot.

use std::time::{Duration, Instant};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{BackendManager, DeviceRegistry, SegmentRange};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_driver_api::DeviceBackend;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{FrameData, SceneChangeReason};
use hypercolor_types::scene::{Scene, UnassignedBehavior};
use tokio::sync::{Mutex, watch};

use super::*;
use crate::SceneTransactionQueue;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::output_power::OutputPowerState;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::frame_executor::execute_frame;
use crate::render_thread::frame_policy::SkipDecision;
use crate::render_thread::pipeline_runtime::PipelineRuntime;
use crate::render_thread::sparkleflinger::MediaTextureSourceKey;
use crate::render_thread::{CanvasDims, RenderThreadState};

const CANVAS: (u32, u32) = (320, 200);
/// The render loop's pacing at 60 fps, so deferred GPU samples land the way
/// they do live.
const FRAME_PACING: Duration = Duration::from_millis(16);
/// Frames a case waits for an expected published state before failing.
const MAX_WAIT_FRAMES: u32 = 120;
/// Frames a cold-start zone stays without a frame.
const PENDING_FRAMES: u32 = 6;

/// Arms one scene render failure on the zone runtime.
type InjectFailure = fn(&mut ZoneRuntime);

/// A render pipeline whose frames the test steps.
struct SteppedPipeline {
    state: RenderThreadState,
    /// Boxed so the futures that hold the pipeline stay small.
    runtime: Box<PipelineRuntime>,
}

impl SteppedPipeline {
    /// `None` when a GPU pipeline cannot start and GPU tests are optional.
    async fn start(
        mode: RenderAccelerationMode,
        scenes: Vec<Scene>,
        active_scene_id: SceneId,
        backend_manager: BackendManager,
    ) -> Option<Self> {
        let runtime = Box::new(
            match PipelineRuntime::new(
                CANVAS.0,
                CANVAS.1,
                SpatialEngine::new(empty_layout()),
                false,
                mode,
                FpsTier::Full,
            ) {
                Ok(runtime) => runtime,
                Err(error) => {
                    assert!(
                        mode != RenderAccelerationMode::Gpu
                            || std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                        "GPU pipeline test was required but initialization failed: {error:#}"
                    );
                    return None;
                }
            },
        );
        assert_eq!(
            runtime.render.sparkleflinger.supports_gpu_output_frames(),
            mode == RenderAccelerationMode::Gpu,
            "the pipeline should run on the {mode:?} backend"
        );
        let state = render_state(backend_manager, mode);
        let mut mutation = state.scene_manager.begin_mutation().await;
        for scene in scenes {
            mutation
                .create_scene(scene)
                .expect("test scene should be created");
        }
        mutation
            .activate(active_scene_id, None, SceneChangeReason::UserActivate)
            .expect("test scene should activate");
        state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("test scenes should commit");
        state.render_loop.write().await.start();
        Some(Self { state, runtime })
    }

    async fn activate(&self, scene_id: SceneId) {
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        mutation
            .activate(scene_id, None, SceneChangeReason::UserActivate)
            .expect("test scene should activate");
        self.state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("scene activation should commit");
    }

    fn zone_runtime(&mut self) -> &mut ZoneRuntime {
        &mut self.runtime.render.render_zone_runtime
    }

    async fn render_frame_number(&self) -> u64 {
        self.state.render_loop.read().await.frame_number()
    }

    /// Render one frame and return the frame it published.
    async fn frame(&mut self) -> FrameData {
        let _ = self.state.render_loop.write().await.tick();
        // Every stepped frame renders: a budget-driven reuse decision would
        // skip the scene pass a case injected into.
        let _ = execute_frame(
            &self.state,
            &mut self.runtime,
            Instant::now(),
            SkipDecision::None,
        )
        .await;
        tokio::time::sleep(FRAME_PACING).await;
        self.state.event_bus.frame_lane().borrow().clone()
    }

    async fn frame_where(
        &mut self,
        description: &str,
        predicate: impl Fn(&FrameData) -> bool,
    ) -> FrameData {
        let mut last = String::from("no frame published");
        for _ in 0..MAX_WAIT_FRAMES {
            let frame = self.frame().await;
            if predicate(&frame) {
                return frame;
            }
            last = frame_summary(&frame);
        }
        panic!("no frame within {MAX_WAIT_FRAMES} frames showed {description}; last: {last}");
    }
}

/// A render failure on a GPU multi-zone scene resumes from the retained
/// colors instead of going dark (Spec 86 §4.2, §8 criterion 6). A retained
/// zone set keeps no colors of its own, so the frame must hold the published
/// colors. Both resume paths run: a projected scene composition that fails
/// without a CPU replay reuses the last good scene inside the zone pass, and
/// a render error reuses the retained scene in the composer.
#[tokio::test]
async fn gpu_multi_zone_render_failures_resume_from_retained_colors() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut scene = make_scene("Overlapping Zones");
    scene.transition.duration_ms = 0;
    scene.zones = overlapping_zones(solid_id, [255, 0, 0], [0, 0, 255]);
    scene.unassigned_behavior = UnassignedBehavior::Off;
    let Some(mut pipeline) = SteppedPipeline::start(
        RenderAccelerationMode::Gpu,
        vec![scene.clone()],
        scene.id,
        BackendManager::new(),
    )
    .await
    else {
        return;
    };
    let lit = |frame: &FrameData| {
        zone_leds(frame, "zone_red") == Some(vec![[255, 0, 0]])
            && zone_leds(frame, "zone_blue") == Some(vec![[0, 0, 255]])
    };
    pipeline.frame_where("both zones lit", lit).await;

    let failures: [(&str, InjectFailure); 2] = [
        (
            "projected scene composition",
            ZoneRuntime::fail_next_projected_scene_composition_for_test,
        ),
        ("scene render", ZoneRuntime::fail_next_scene_render_for_test),
    ];
    for (failure, inject) in failures {
        inject(pipeline.zone_runtime());
        let frame_number = pipeline.render_frame_number().await;
        let frame = pipeline.frame().await;
        assert!(
            !pipeline
                .zone_runtime()
                .injected_scene_failure_pending_for_test(),
            "the {failure} failure should have hit the rendered frame"
        );
        assert!(
            pipeline.render_frame_number().await > frame_number,
            "the {failure} failure frame should complete"
        );
        assert!(
            lit(&frame),
            "after a {failure} failure the LEDs should resume from the retained colors: {}",
            frame_summary(&frame)
        );
        for _ in 0..3 {
            let frame = pipeline.frame().await;
            assert!(
                lit(&frame),
                "frames after a {failure} failure should stay lit: {}",
                frame_summary(&frame)
            );
        }
    }
}

/// A zone with no snapshot yet whose layers are all still loading samples
/// nothing on its first frames: its LEDs hold the published colors when the
/// published frame carries them in the same shape, and go black otherwise
/// (Spec 86 §2.2, §8 criterion 14). The zone beside it renders its own
/// colors meanwhile, and the cold zone shows its own once it loads.
#[tokio::test]
async fn cold_start_zone_holds_published_colors_or_goes_black() {
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut first = make_scene("Red And Blue");
    first.transition.duration_ms = 0;
    first.zones = overlapping_zones(solid_id, [255, 0, 0], [0, 0, 255]);
    first.unassigned_behavior = UnassignedBehavior::Off;
    // Same outputs, so the published frame has the cold zone's shape.
    let mut matching = make_scene("Green And Yellow");
    matching.transition.duration_ms = 0;
    matching.zones = overlapping_zones(solid_id, [0, 255, 0], [255, 255, 0]);
    matching.unassigned_behavior = UnassignedBehavior::Off;
    // The cold zone's output grows from one LED to four.
    let mut reshaped = make_scene("Green And Yellow Strip");
    reshaped.transition.duration_ms = 0;
    reshaped.zones = overlapping_zones(solid_id, [0, 255, 0], [255, 255, 0]);
    let mut strip = point_zone("zone_blue");
    strip.topology = LedTopology::Strip {
        count: 4,
        direction: StripDirection::LeftToRight,
    };
    reshaped.zones[1].layout.zones = vec![strip];
    reshaped.unassigned_behavior = UnassignedBehavior::Off;
    let Some(mut pipeline) = SteppedPipeline::start(
        RenderAccelerationMode::Gpu,
        vec![first.clone(), matching.clone(), reshaped.clone()],
        first.id,
        BackendManager::new(),
    )
    .await
    else {
        return;
    };
    pipeline
        .frame_where("the first scene lit", |frame| {
            zone_led(frame, "zone_red") == Some([255, 0, 0])
                && zone_led(frame, "zone_blue") == Some([0, 0, 255])
        })
        .await;

    for (scene, held, case) in [
        (&matching, vec![[0, 0, 255]], "a matching published shape"),
        (
            &reshaped,
            vec![[0, 0, 0]; 4],
            "a mismatched published shape",
        ),
    ] {
        pipeline.activate(scene.id).await;
        let cold_layer = scene.zones[1].layers[0].id;
        // The cold zone stays loading for at least `PENDING_FRAMES`, and on
        // until its neighbor has rendered, however many frames a slow GPU
        // takes to deliver that sample.
        pipeline
            .zone_runtime()
            .pend_effect_layer_renders_for_test(cold_layer, 4 * MAX_WAIT_FRAMES);
        let mut neighbor_rendered = false;
        for index in 0..MAX_WAIT_FRAMES {
            let frame = pipeline.frame().await;
            assert_eq!(
                zone_leds(&frame, "zone_blue"),
                Some(held.clone()),
                "pending frame {index} with {case}: {}",
                frame_summary(&frame)
            );
            neighbor_rendered |= zone_led(&frame, "zone_red") == Some([0, 255, 0]);
            if neighbor_rendered && index + 1 >= PENDING_FRAMES {
                break;
            }
        }
        assert!(
            pipeline
                .zone_runtime()
                .pending_effect_layers
                .contains_key(&cold_layer),
            "with {case}: the cold zone should have stayed pending for every stepped frame"
        );
        assert!(
            neighbor_rendered,
            "with {case}: the loaded zone should render while the cold zone waits"
        );
        pipeline
            .zone_runtime()
            .pend_effect_layer_renders_for_test(cold_layer, 0);
        let loaded = vec![[255, 255, 0]; held.len()];
        pipeline
            .frame_where("the cold zone's own colors once it loads", |frame| {
                zone_leds(frame, "zone_blue") == Some(loaded.clone())
            })
            .await;
    }
}

/// A zone whose layout defaults to Gaussian-area sampling while its own
/// outputs override to Bilinear, with `Fallback(zone)` active, samples both
/// kinds of output correctly on the GPU (Spec 86 §4.3, §11.1, §8 criterion
/// 17 as amended): the assigned outputs with their Bilinear override and the
/// synthesized fallback outputs, which carry no override, with the layout's
/// Gaussian default. Striped content tells the two modes apart. Each lands
/// within the sampler's CPU parity tolerance of the CPU sampler, and the
/// unplaced device receives what the CPU backend sends it for the same
/// scene.
#[tokio::test]
async fn bilinear_outputs_under_a_gaussian_default_sample_assigned_and_fallback_outputs_on_the_gpu()
{
    let Some(gpu) = run_striped_fallback(RenderAccelerationMode::Gpu).await else {
        return;
    };
    let cpu = run_striped_fallback(RenderAccelerationMode::Cpu)
        .await
        .expect("the CPU pipeline always starts");
    let stripes = striped_canvas(STRIPED_EXTENT.0, STRIPED_EXTENT.1);
    let expected_assigned = flatten(&SpatialEngine::new(gpu.zone.layout.clone()).sample(&stripes));
    let mut gaussian_layout = gpu.zone.layout.clone();
    gaussian_layout.zones = gpu
        .fallback_outputs
        .clone()
        .expect("a GPU-backed fallback zone should register its GPU fallback plan");
    assert!(
        gaussian_layout
            .zones
            .iter()
            .all(|output| output.sampling_mode.is_none()),
        "synthesized fallback outputs should carry no sampling override"
    );
    let mut bilinear_layout = gaussian_layout.clone();
    bilinear_layout.default_sampling_mode = SamplingMode::Bilinear;
    let expected_fallback = flatten(&SpatialEngine::new(gaussian_layout).sample(&stripes));
    let bilinear_fallback = flatten(&SpatialEngine::new(bilinear_layout).sample(&stripes));
    assert_ne!(
        expected_fallback, bilinear_fallback,
        "the striped content should tell Gaussian and Bilinear sampling apart"
    );

    assert!(
        within(&gpu.assigned, &expected_assigned, 1),
        "the assigned outputs should sample Bilinear on the GPU: {:?}, expected {expected_assigned:?}",
        gpu.assigned
    );
    let fallback_samples = gpu
        .fallback_samples
        .expect("the GPU dispatch should sample the fallback outputs");
    assert!(
        within(&fallback_samples, &expected_fallback, 1),
        "the fallback outputs should sample the layout's Gaussian default on the GPU: \
         {fallback_samples:?}, expected {expected_fallback:?} (Bilinear would be \
         {bilinear_fallback:?})"
    );
    assert!(
        within(&cpu.assigned, &expected_assigned, 0),
        "the CPU backend reference should sample the assigned outputs exactly"
    );
    // Device output re-encodes each sample, which can stretch the sampler's
    // one-step parity tolerance to two.
    assert!(
        within(&gpu.device, &cpu.device, 2),
        "the unplaced device should get the CPU backend's fallback colors: GPU {:?}, CPU {:?}",
        gpu.device,
        cpu.device
    );
}

const STRIPED_EXTENT: (u32, u32) = (160, 100);

/// What one stepped run of the striped fallback scene ended with.
struct StripedFallbackRun {
    zone: Zone,
    assigned: Vec<[u8; 3]>,
    device: Vec<[u8; 3]>,
    /// The fallback plan's outputs, when the GPU sampled them.
    fallback_outputs: Option<Vec<Output>>,
    /// The fallback colors the latest GPU dispatch returned.
    fallback_samples: Option<Vec<[u8; 3]>>,
}

/// Render the striped Gaussian-default zone beside a solid zone with
/// `Fallback(striped zone)` on `mode`, feeding the striped content as the
/// zone's producer frame, and report what the outputs show once settled.
async fn run_striped_fallback(mode: RenderAccelerationMode) -> Option<StripedFallbackRun> {
    const FALLBACK_LEDS: u32 = 8;
    const SETTLE_FRAMES: u32 = 30;
    let (backend, device_id, backend_manager) = unplaced_mock_device(FALLBACK_LEDS).await;
    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut striped_output = point_zone("zone_striped");
    striped_output.size = NormalizedPosition::new(1.0, 0.2);
    striped_output.topology = LedTopology::Strip {
        count: 8,
        direction: StripDirection::LeftToRight,
    };
    striped_output.sampling_mode = Some(SamplingMode::Bilinear);
    let mut striped = solid_zone("Striped", solid_id, [0, 0, 0], vec![striped_output]);
    striped.layout.canvas_width = STRIPED_EXTENT.0;
    striped.layout.canvas_height = STRIPED_EXTENT.1;
    striped.layout.default_sampling_mode = SamplingMode::GaussianArea {
        sigma: 2.0,
        radius: 3,
    };
    let striped_layer = striped.layers[0].id;
    let mut scene = make_scene("Gaussian Fallback");
    scene.transition.duration_ms = 0;
    scene.zones = vec![
        striped.clone(),
        solid_zone(
            "Solid",
            solid_id,
            [0, 0, 255],
            vec![point_zone("zone_solid")],
        ),
    ];
    scene.unassigned_behavior = UnassignedBehavior::Fallback(striped.id);
    let mut pipeline =
        SteppedPipeline::start(mode, vec![scene.clone()], scene.id, backend_manager).await?;
    let stripes = striped_canvas(STRIPED_EXTENT.0, STRIPED_EXTENT.1);

    let mut published = FrameData::empty();
    for _ in 0..SETTLE_FRAMES {
        let frame = if mode == RenderAccelerationMode::Gpu {
            ProducerFrame::GpuTexture(
                pipeline
                    .runtime
                    .render
                    .sparkleflinger
                    .upload_media_canvas_frame(MediaTextureSourceKey::for_test(1), &stripes)
                    .expect("the striped producer frame should upload"),
            )
        } else {
            ProducerFrame::Canvas(stripes.clone())
        };
        pipeline
            .zone_runtime()
            .inject_effect_layer_frame_for_test(striped_layer, frame);
        published = pipeline.frame().await;
    }
    let cache = &pipeline
        .runtime
        .render
        .output_artifacts
        .unassigned_output_cache;
    let fallback = cache
        .gpu_fallback()
        .filter(|fallback| fallback.zone_id == striped.id);
    let fallback_outputs = fallback.map(|fallback| fallback.engine.layout().zones.clone());
    let fallback_samples = fallback_outputs.as_ref().and_then(|outputs| {
        let shape = outputs
            .iter()
            .map(|output| ZoneColors {
                zone_id: output.id.clone(),
                colors: vec![
                    [0, 0, 0];
                    usize::try_from(FALLBACK_LEDS).expect("LED count fits usize")
                ],
            })
            .collect::<Vec<_>>();
        cache.gpu_fallback_colors(striped.id, &shape).map(flatten)
    });
    Some(StripedFallbackRun {
        zone: striped,
        assigned: zone_leds(&published, "zone_striped").unwrap_or_default(),
        device: backend.last_colors(&device_id).unwrap_or_default(),
        fallback_outputs,
        fallback_samples,
    })
}

fn render_state(
    backend_manager: BackendManager,
    mode: RenderAccelerationMode,
) -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let scene_manager =
        SceneService::with_temporary_store(SceneManager::with_default(), Arc::clone(&event_bus))
            .expect("temporary scene store should open");
    let scene_plan = scene_manager.plan_reader();
    let asset_tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_registry())),
        asset_library: Arc::new(RwLock::new(
            AssetLibrary::open(asset_tempdir.path().join("assets"))
                .expect("test asset library should open"),
        )),
        spatial_engine: SpatialService::new(SpatialEngine::new(empty_layout())),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        zone_layout_previews: Arc::new(
            crate::zone_layout_preview::ZoneLayoutPreviewStore::default(),
        ),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager,
        scene_plan,
        input_manager: InputManager::new(),
        interaction_routing: crate::interaction_routing::InteractionRoutingControl::default(),
        power_state,
        scene_transactions: SceneTransactionQueue::default(),
        screen_capture_configured: false,
        canvas_dims: CanvasDims::new(CANVAS.0, CANVAS.1),
        render_acceleration_mode: mode,
        render_gpu_device: None,
        configured_max_fps_tier: FpsTier::Full.into(),
        face_fps_cap: 30,
    }
}

/// A mock device the router knows but no layout output covers, so every
/// scene leaves its LEDs unassigned.
async fn unplaced_mock_device(
    led_count: u32,
) -> (Arc<MockDeviceBackend>, DeviceId, BackendManager) {
    let device_id = DeviceId::new();
    let backend = Arc::new(MockDeviceBackend::new().with_device(&MockDeviceConfig {
        name: "Unplaced Strip".into(),
        led_count,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        id: Some(device_id),
    }));
    backend
        .connect(&device_id)
        .await
        .expect("mock device should connect");
    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(backend.clone());
    backend_manager.map_device_with_segment(
        "mock:unplaced",
        "mock",
        device_id,
        Some(SegmentRange::new(
            0,
            usize::try_from(led_count).expect("LED count fits usize"),
        )),
    );
    (backend, device_id, backend_manager)
}

fn empty_layout() -> SpatialLayout {
    zone_layout(Vec::new())
}

fn zone_layout(outputs: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "pipeline-test".into(),
        name: "Pipeline Test".into(),
        description: None,
        canvas_width: CANVAS.0,
        canvas_height: CANVAS.1,
        zones: outputs,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// Two zones whose single LEDs sit at the same scene position.
fn overlapping_zones(solid_id: EffectId, red_led: [u8; 3], blue_led: [u8; 3]) -> Vec<Zone> {
    vec![
        solid_zone("Red", solid_id, red_led, vec![point_zone("zone_red")]),
        solid_zone("Blue", solid_id, blue_led, vec![point_zone("zone_blue")]),
    ]
}

fn solid_zone(name: &str, solid_id: EffectId, color: [u8; 3], outputs: Vec<Output>) -> Zone {
    let mut zone = sample_zone(CANVAS.0, CANVAS.1);
    zone.name = name.into();
    set_effect_zone(
        &mut zone,
        solid_id,
        HashMap::from([(
            "color".into(),
            ControlValue::linear_color([
                f32::from(color[0]) / 255.0,
                f32::from(color[1]) / 255.0,
                f32::from(color[2]) / 255.0,
                1.0,
            ]),
        )]),
    );
    zone.layout = zone_layout(outputs);
    zone
}

/// Vertical red and blue stripes two pixels wide.
fn striped_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let color = if x % 4 < 2 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 0, 255, 255)
            };
            canvas.set_pixel(x, y, color);
        }
    }
    canvas
}

fn flatten(zones: &[ZoneColors]) -> Vec<[u8; 3]> {
    zones
        .iter()
        .flat_map(|zone| zone.colors.iter().copied())
        .collect()
}

/// Whether `actual` matches `expected` within `tolerance` steps per channel.
/// One step is the GPU sampler's CPU parity tolerance.
fn within(actual: &[[u8; 3]], expected: &[[u8; 3]], tolerance: u8) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(*expected) <= tolerance)
        })
}

fn zone_leds(frame: &FrameData, zone_id: &str) -> Option<Vec<[u8; 3]>> {
    frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == zone_id)
        .map(|zone| zone.colors.clone())
}

fn zone_led(frame: &FrameData, zone_id: &str) -> Option<[u8; 3]> {
    zone_leds(frame, zone_id).and_then(|colors| colors.first().copied())
}

fn frame_summary(frame: &FrameData) -> String {
    let zones = frame
        .zones
        .iter()
        .map(|zone| format!("{}={:?}", zone.zone_id, zone.colors))
        .collect::<Vec<_>>()
        .join(", ");
    format!("frame {} [{zones}]", frame.frame_number)
}

/// A GPU slower than the frame cadence still drives a multi-zone scene's
/// LEDs. Each sample finishes only after the next frame has dispatched a
/// newer one, so every sample in flight at a frame start is stale; the
/// frame must publish the newest one that finished instead of dropping
/// them all, and the LEDs follow scene changes without leaking readback
/// slots.
#[tokio::test]
async fn slow_gpu_readbacks_keep_multi_zone_leds_following_the_scene() {
    use crate::render_thread::sparkleflinger::SAMPLE_READBACK_WITHHELD_POLLS;
    use std::sync::atomic::Ordering;

    /// Resets the knob even when an assertion fails.
    struct WithheldPolls;
    impl Drop for WithheldPolls {
        fn drop(&mut self) {
            SAMPLE_READBACK_WITHHELD_POLLS.store(0, Ordering::Relaxed);
        }
    }

    let registry = builtin_registry();
    let solid_id = builtin_effect_id(&registry, "solid_color");
    let mut first = make_scene("Red And Blue");
    first.transition.duration_ms = 0;
    first.zones = overlapping_zones(solid_id, [255, 0, 0], [0, 0, 255]);
    first.unassigned_behavior = UnassignedBehavior::Off;
    let mut second = make_scene("Green And Yellow");
    second.transition.duration_ms = 0;
    second.zones = overlapping_zones(solid_id, [0, 255, 0], [255, 255, 0]);
    second.unassigned_behavior = UnassignedBehavior::Off;
    let Some(mut pipeline) = SteppedPipeline::start(
        RenderAccelerationMode::Gpu,
        vec![first.clone(), second.clone()],
        first.id,
        BackendManager::new(),
    )
    .await
    else {
        return;
    };
    let _withheld = WithheldPolls;
    SAMPLE_READBACK_WITHHELD_POLLS.store(4, Ordering::Relaxed);
    let shows = |red: [u8; 3], blue: [u8; 3]| {
        move |frame: &FrameData| {
            zone_leds(frame, "zone_red") == Some(vec![red])
                && zone_leds(frame, "zone_blue") == Some(vec![blue])
        }
    };

    for round in 0..4 {
        pipeline.activate(first.id).await;
        pipeline
            .frame_where(
                &format!("round {round}: the first scene with slow readbacks"),
                shows([255, 0, 0], [0, 0, 255]),
            )
            .await;
        pipeline.activate(second.id).await;
        pipeline
            .frame_where(
                &format!("round {round}: the second scene with slow readbacks"),
                shows([0, 255, 0], [255, 255, 0]),
            )
            .await;
    }
}
