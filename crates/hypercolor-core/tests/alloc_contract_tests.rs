#[cfg(feature = "servo")]
compile_error!(
    "allocation-contract-tests owns the process allocator and cannot be combined with servo; run `just alloc-contracts`"
);

use std::{
    alloc::System,
    collections::HashMap,
    hint::black_box,
    marker::PhantomData,
    sync::Arc,
    time::{Duration, Instant},
};

use hypercolor_core::effect::{EffectPool, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::input::audio::AudioInput;
use hypercolor_core::input::audio::realtime::{AudioFrameRing, PushStats, push_frames};
use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteContext, InteractionRouteRequest, InteractionRouteSource,
    InteractionRouter, RoutedInteraction,
};
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::CaptureRotation;
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::wayland::{
    DoubleBuffer, SpaChunkView, SpaVideoFormat, decode_chunk,
};
use hypercolor_core::input::screen::{CaptureConfig, ScreenCaptureInput, TemporalSmoother};
use hypercolor_core::input::{
    AudioSource, AudioSourceRole, BrowserConnectionIncarnation, BrowserInputChildKey,
    BrowserInputEdge, BrowserInputSource, BrowserPreviewId, InputData, InputManager, InputSource,
    InteractionBatch, InteractionData, InteractionSource, InteractionSourceRole, ManagedSourceRole,
    MotionAggregate, ScreenData, ScreenSource, ScreenSourceRole, SourceKind, SourceRoleBinding,
    SourceSessionWriter, SourceStatusHandle, SourceStatusWriter,
};
use hypercolor_core::types::audio::{AudioData, AudioPipelineConfig};
use hypercolor_core::types::event::TimedInputEvent;
use hypercolor_types::effect::ControlValue;
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[cfg_attr(not(feature = "servo"), global_allocator)]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn register_test_source(manager: &mut InputManager, source: ManagedSourceRole) {
    manager
        .add_source(source)
        .expect("allocation fixture source should match its declared role");
}

fn allocation_control() -> (Stats, Stats) {
    let mut region = Region::new(GLOBAL);
    region.reset();

    let allocation = black_box(vec![0_u8; 4_096]);
    let after_allocation = region.change();
    drop(allocation);

    (after_allocation, region.change())
}

fn preallocated_control(storage: &mut Vec<u8>) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();

    let storage = black_box(storage);
    storage.push(7);
    black_box(storage.as_slice());
    let value = storage.pop();
    black_box(value);

    region.change()
}

fn prepared_effect_pool_commit_round(change_controls: bool) -> Stats {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    let effect_id = registry
        .iter()
        .find_map(|(id, entry)| {
            (entry.metadata.source.source_stem() == Some("solid_color")).then_some(*id)
        })
        .expect("solid color effect should be registered");
    let layout = SpatialLayout {
        id: "allocation-pool".to_owned(),
        name: "Allocation Pool".to_owned(),
        description: None,
        canvas_width: 8,
        canvas_height: 8,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let controls = HashMap::from([(
        "color".to_owned(),
        ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
    )]);
    let mut group = Zone {
        id: ZoneId::new(),
        name: "Allocation Group".to_owned(),
        description: None,
        effect_id: Some(effect_id),
        controls: controls.clone(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            controls,
            HashMap::new(),
            None,
        )],
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    };
    let mut pool = EffectPool::new();
    pool.reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("live effect pool should prepare");
    if change_controls {
        let updated = ControlValue::Color([0.0, 0.0, 1.0, 1.0]);
        group.controls.insert("color".to_owned(), updated.clone());
        let LayerSource::Effect { controls, .. } = &mut group.layers[0].source else {
            panic!("fixture should store an effect layer");
        };
        controls.insert("color".to_owned(), updated);
    }
    let prepared = pool
        .prepare_reconcile(std::slice::from_ref(&group), &registry, &HashMap::new())
        .expect("candidate effect pool should prepare");

    let mut region = Region::new(GLOBAL);
    region.reset();
    black_box(&mut pool).commit_reconcile(black_box(prepared));
    region.change()
}

fn screen_push_round(
    input: &mut ScreenCaptureInput,
    frame: &[u8],
    width: u32,
    height: u32,
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    input
        .push_frame(black_box(frame), width, height)
        .expect("benchmark frame resources remain admitted");
    region.change()
}

fn patterned_rgba_frame(width: u32, height: u32) -> Vec<u8> {
    let byte_len = usize::try_from(width)
        .ok()
        .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("test frame extent should fit usize");
    let mut frame = vec![0_u8; byte_len];
    for (index, pixel) in frame.chunks_exact_mut(4).enumerate() {
        let coordinate = index.to_le_bytes()[0];
        pixel.copy_from_slice(&[
            coordinate,
            coordinate.wrapping_mul(3),
            coordinate.wrapping_mul(7),
            255,
        ]);
    }
    frame
}

fn steady_screen_shape_push_control(
    input: &mut ScreenCaptureInput,
    width: u32,
    height: u32,
) -> (Stats, Stats) {
    let frame = patterned_rgba_frame(width, height);

    for _ in 0..3 {
        input
            .push_frame(&frame, width, height)
            .expect("benchmark frame resources remain admitted");
    }

    (
        screen_push_round(input, &frame, width, height),
        screen_push_round(input, &frame, width, height),
    )
}

fn steady_screen_push_control() -> [(Stats, Stats); 3] {
    let mut input = ScreenCaptureInput::new(CaptureConfig {
        grid_cols: 16,
        grid_rows: 9,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    });

    [
        steady_screen_shape_push_control(&mut input, 333, 777),
        steady_screen_shape_push_control(&mut input, 1_001, 333),
        steady_screen_shape_push_control(&mut input, 641, 479),
    ]
}

fn steady_screen_sample_control() -> (Stats, Stats) {
    let mut input = ScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("screen input should start");
    let frame = patterned_rgba_frame(64, 48);
    input
        .push_frame(&frame, 64, 48)
        .expect("warm screen frame remains admitted");
    drop(input.sample().expect("warm screen sample succeeds"));

    let sample = |input: &mut ScreenCaptureInput| {
        let mut region = Region::new(GLOBAL);
        region.reset();
        drop(black_box(input).sample().expect("screen sample succeeds"));
        region.change()
    };
    (sample(&mut input), sample(&mut input))
}

fn smoother_round(
    smoother: &mut TemporalSmoother,
    colors: &mut [[u8; 3]],
    width: u32,
    height: u32,
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    smoother.apply_for_elapsed_grid(colors, width, height, Duration::from_millis(16));
    region.change()
}

fn steady_smoother_shape_control(
    smoother: &mut TemporalSmoother,
    width: u32,
    height: u32,
) -> (Stats, Stats) {
    let len = usize::try_from(width)
        .ok()
        .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
        .expect("test smoother extent should fit usize");
    let mut colors = vec![[96, 32, 160]; len];
    smoother.apply_for_elapsed_grid(&mut colors, width, height, Duration::from_millis(16));
    smoother.apply_for_elapsed_grid(&mut colors, width, height, Duration::from_millis(16));

    (
        smoother_round(smoother, &mut colors, width, height),
        smoother_round(smoother, &mut colors, width, height),
    )
}

fn steady_smoother_control() -> [(Stats, Stats); 3] {
    let mut smoother = TemporalSmoother::new(0.3, 100.0);
    [
        steady_smoother_shape_control(&mut smoother, 333, 777),
        steady_smoother_shape_control(&mut smoother, 1_001, 333),
        steady_smoother_shape_control(&mut smoother, 641, 479),
    ]
}

fn audio_callback_round(input: &[f32], ring: &AudioFrameRing) -> (PushStats, Stats) {
    let mut region = Region::new(GLOBAL);
    region.reset();
    let pushed = black_box(push_frames(black_box(input), black_box(ring)));
    (pushed, region.change())
}

fn sample_round(session: &SourceSessionWriter, base: Instant, first_offset: u64) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    let mut all_accepted = true;

    for offset in first_offset..first_offset + 128 {
        let sampled_at = base + Duration::from_millis(offset);
        let deadline = base + Duration::from_mins(2) + Duration::from_millis(offset);
        all_accepted &= black_box(session.record_sample(sampled_at, deadline, 1)) == Ok(true);
    }

    let stats = region.change();
    assert!(all_accepted);
    stats
}

fn steady_source_sample_control() -> (Stats, Stats) {
    let (writer, _) = SourceStatusWriter::new(
        "allocation-source",
        SourceKind::Screen,
        "test",
        true,
        true,
        true,
    );
    let session = writer
        .begin_session(1)
        .expect("allocation source session should start");
    let base = Instant::now();
    assert_eq!(
        session.record_sample(base, base + Duration::from_mins(1), 1),
        Ok(true)
    );

    (
        sample_round(&session, base, 1),
        sample_round(&session, base, 129),
    )
}

fn availability_round(handle: &SourceStatusHandle, now: Instant) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        black_box(handle.availability_at(now));
    }
    region.change()
}

fn steady_availability_control() -> (Stats, Stats) {
    let (writer, handle) = SourceStatusWriter::new(
        "availability-source",
        SourceKind::Interaction,
        "test",
        true,
        true,
        true,
    );
    let session = writer
        .begin_session(1)
        .expect("availability source session should start");
    let sampled_at = Instant::now();
    session
        .record_sample(sampled_at, sampled_at + Duration::from_mins(1), 1)
        .expect("availability sample should publish");

    (
        availability_round(&handle, sampled_at),
        availability_round(&handle, sampled_at),
    )
}

struct SharedSampleSource<R> {
    sample: Arc<InputData>,
    running: bool,
    role: PhantomData<R>,
}

impl<R> SharedSampleSource<R> {
    fn new(sample: InputData) -> Self {
        Self {
            sample: Arc::new(sample),
            running: false,
            role: PhantomData,
        }
    }
}

impl<R: Send> InputSource for SharedSampleSource<R> {
    fn name(&self) -> &'static str {
        "shared-allocation-sample"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn sample_shared_and_drain_into(
        &mut self,
        _delta_secs: f32,
        _events: &mut Vec<TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        Ok(self.running.then(|| Arc::clone(&self.sample)))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for SharedSampleSource<AudioSourceRole> {
    type Role = AudioSourceRole;
}

impl AudioSource for SharedSampleSource<AudioSourceRole> {}

impl SourceRoleBinding for SharedSampleSource<ScreenSourceRole> {
    type Role = ScreenSourceRole;
}

impl ScreenSource for SharedSampleSource<ScreenSourceRole> {}

impl SourceRoleBinding for SharedSampleSource<InteractionSourceRole> {
    type Role = InteractionSourceRole;
}

impl InteractionSource for SharedSampleSource<InteractionSourceRole> {}

fn manager_sample_round(manager: &mut InputManager) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        black_box(&mut *manager).sample_sources(1.0 / 60.0);
    }
    region.change()
}

fn steady_manager_sampling_control() -> (Stats, Stats) {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::audio(Box::new(SharedSampleSource::<AudioSourceRole>::new(
            InputData::Audio(AudioData::silence()),
        ))),
    );
    register_test_source(
        &mut manager,
        ManagedSourceRole::screen(Box::new(SharedSampleSource::<ScreenSourceRole>::new(
            InputData::Screen(ScreenData::from_zones(Vec::new(), 0, 0)),
        ))),
    );
    register_test_source(
        &mut manager,
        ManagedSourceRole::interaction(Box::new(SharedSampleSource::<InteractionSourceRole>::new(
            InputData::Interaction(InteractionData::default()),
        ))),
    );
    manager
        .start_all()
        .expect("allocation sources should start");
    manager.sample_sources(1.0 / 60.0);
    let graph = manager.input_graph_handle().snapshot();
    assert_eq!(graph.slots().len(), 3);
    assert!(graph.slots().iter().all(|slot| slot.latest().is_some()));

    (
        manager_sample_round(&mut manager),
        manager_sample_round(&mut manager),
    )
}

fn typed_manager_sample_round(
    manager: &mut InputManager,
    due_sources: &[(SourceKind, f32)],
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        black_box(&mut *manager).sample_source_kinds(black_box(due_sources));
    }
    region.change()
}

fn steady_typed_manager_sampling_control() -> (Stats, Stats) {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::audio(Box::new(SharedSampleSource::<AudioSourceRole>::new(
            InputData::Audio(AudioData::silence()),
        ))),
    );
    register_test_source(
        &mut manager,
        ManagedSourceRole::interaction(Box::new(SharedSampleSource::<InteractionSourceRole>::new(
            InputData::Interaction(InteractionData::default()),
        ))),
    );
    manager
        .start_all()
        .expect("typed allocation sources should start");
    let due_sources = [
        (SourceKind::Audio, 1.0 / 60.0),
        (SourceKind::Interaction, 1.0 / 240.0),
    ];
    manager.sample_source_kinds(&due_sources);

    (
        typed_manager_sample_round(&mut manager, &due_sources),
        typed_manager_sample_round(&mut manager, &due_sources),
    )
}

fn steady_audio_manager_sampling_control() -> (Stats, Stats) {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::audio(Box::new(AudioInput::new(&AudioPipelineConfig::default()))),
    );
    manager
        .start_all()
        .expect("manual audio source should start");
    manager.sample_sources(1.0 / 60.0);
    (
        manager_sample_round(&mut manager),
        manager_sample_round(&mut manager),
    )
}

fn browser_sample_round(
    source: &mut BrowserInputSource,
    events: &mut Vec<TimedInputEvent>,
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        events.clear();
        let sample = black_box(&mut *source)
            .sample_shared_and_drain_into(1.0 / 60.0, black_box(events))
            .expect("browser allocation sample should succeed");
        black_box(sample);
    }
    region.change()
}

fn steady_browser_sampling_control() -> (Stats, Stats) {
    let mut source = BrowserInputSource::new();
    source
        .start()
        .expect("browser allocation source should start");
    let attachment = source
        .handle()
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(1),
            BrowserPreviewId::new("allocation-preview"),
        ))
        .expect("browser allocation preview should attach");
    attachment
        .inject([BrowserInputEdge::Move {
            norm_x: 0.25,
            norm_y: 0.75,
        }])
        .expect("browser allocation motion should inject");
    let mut events = Vec::with_capacity(4);
    let warm = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("browser allocation warmup should succeed");
    drop(warm);

    (
        browser_sample_round(&mut source, &mut events),
        browser_sample_round(&mut source, &mut events),
    )
}

fn router_resolution_round(
    manager: &mut InputManager,
    router: &mut InteractionRouter,
    consumer: ConsumerIncarnation,
    sources: &[InteractionRouteSource],
    context: InteractionRouteContext,
    output: &mut RoutedInteraction,
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        black_box(&mut *manager).sample_sources(1.0 / 60.0);
        black_box(&mut *router).resolve_into(
            black_box(consumer),
            InteractionRouteRequest::host(),
            black_box(sources),
            black_box(context),
            black_box(&mut *output),
        );
    }
    region.change()
}

fn steady_router_resolution_control() -> (Stats, Stats) {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::interaction(Box::new(SharedSampleSource::<InteractionSourceRole>::new(
            InputData::Interaction(InteractionData {
                batch: InteractionBatch {
                    motion: MotionAggregate {
                        dx: 0.25,
                        dy: -0.125,
                        distance: 0.375,
                    },
                    window_secs: 1.0 / 60.0,
                    ..InteractionBatch::default()
                },
                ..InteractionData::default()
            }),
        ))),
    );
    manager
        .start_all()
        .expect("allocation interaction source should start");
    manager.sample_sources(1.0 / 60.0);
    let graph = manager.input_graph_handle().snapshot();
    let sources = graph
        .slots()
        .iter()
        .filter_map(|slot| {
            InteractionRouteSource::manager_slot("allocation-interaction", 1, slot.clone())
        })
        .collect::<Vec<_>>();
    let consumer = ConsumerIncarnation::new(1);
    let context = InteractionRouteContext {
        source_graph_generation: graph.generation(),
        ..InteractionRouteContext::default()
    };
    let mut router = InteractionRouter::default();
    let mut output = RoutedInteraction::new(consumer);
    router.resolve_into(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context,
        &mut output,
    );

    let first = router_resolution_round(
        &mut manager,
        &mut router,
        consumer,
        &sources,
        context,
        &mut output,
    );
    assert_eq!(output.interaction.batch.motion.dx, 0.25);
    let second = router_resolution_round(
        &mut manager,
        &mut router,
        consumer,
        &sources,
        context,
        &mut output,
    );
    assert_eq!(output.interaction.batch.motion.dx, 0.25);
    (first, second)
}

fn audio_input_construction_round(config: &AudioPipelineConfig) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    let input = black_box(AudioInput::new(black_box(config)));
    let stats = region.change();
    drop(input);
    stats
}

fn prepared_audio_commit_round(config: &AudioPipelineConfig) -> Stats {
    let mut manager = InputManager::new();
    register_test_source(
        &mut manager,
        ManagedSourceRole::audio(Box::new(AudioInput::new(config))),
    );
    manager
        .start_all()
        .expect("manual audio source should start");
    let mut prepared = manager
        .plan_audio_runtime_config(false, config, "prepared-audio", false)
        .expect("manual audio source should support preparation")
        .prepare()
        .expect("disabled audio preparation should stay local");

    let mut region = Region::new(GLOBAL);
    region.reset();
    let retirement = black_box(&mut manager)
        .commit_audio_runtime_config(black_box(&mut prepared))
        .expect("prepared audio state should commit");
    black_box(&retirement);
    let stats = region.change();
    retirement.retire();
    stats
}

#[cfg(target_os = "linux")]
fn wayland_decode_round(data: &[u8], buffers: &mut DoubleBuffer) -> Stats {
    let view = SpaChunkView::new(
        data,
        4,
        18,
        10,
        2,
        2,
        SpaVideoFormat::Rgba,
        None,
        CaptureRotation::Identity,
    );
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..128 {
        assert_eq!(
            black_box(decode_chunk(black_box(&view), black_box(&mut *buffers))).drop_reason(),
            None
        );
    }
    region.change()
}

#[test]
fn counting_allocator_is_active_and_scoped() {
    drop(black_box(vec![0_u8; 4_096]));

    let first_allocation = allocation_control();
    let second_allocation = allocation_control();

    assert_eq!(first_allocation, second_allocation);
    assert_eq!(first_allocation.0.allocations, 1);
    assert_eq!(first_allocation.0.deallocations, 0);
    assert_eq!(first_allocation.0.reallocations, 0);
    assert_eq!(first_allocation.0.bytes_allocated, 4_096);
    assert_eq!(first_allocation.0.bytes_deallocated, 0);
    assert_eq!(first_allocation.0.bytes_reallocated, 0);
    assert_eq!(first_allocation.1.allocations, 1);
    assert_eq!(first_allocation.1.deallocations, 1);
    assert_eq!(first_allocation.1.reallocations, 0);
    assert_eq!(first_allocation.1.bytes_allocated, 4_096);
    assert_eq!(first_allocation.1.bytes_deallocated, 4_096);
    assert_eq!(first_allocation.1.bytes_reallocated, 0);

    let mut storage = Vec::with_capacity(1);
    storage.push(7);
    black_box(storage.pop());

    let first_preallocated = preallocated_control(&mut storage);
    let second_preallocated = preallocated_control(&mut storage);

    assert_eq!(first_preallocated, Stats::default());
    assert_eq!(second_preallocated, first_preallocated);

    let audio_ring = AudioFrameRing::with_channels(2_048, 2);
    let audio_frames = vec![0.25_f32; 512];
    let first_audio_push = audio_callback_round(&audio_frames, &audio_ring);
    let second_audio_push = audio_callback_round(&audio_frames, &audio_ring);
    assert_eq!(
        first_audio_push.0,
        PushStats {
            accepted: 256,
            dropped: 0,
        }
    );
    assert_eq!(second_audio_push.0, first_audio_push.0);
    assert_eq!(first_audio_push.1, Stats::default());
    assert_eq!(second_audio_push.1, first_audio_push.1);

    let (first_samples, second_samples) = steady_source_sample_control();
    assert_eq!(first_samples, Stats::default());
    assert_eq!(second_samples, Stats::default());

    let (first_availability, second_availability) = steady_availability_control();
    assert_eq!(first_availability, Stats::default());
    assert_eq!(second_availability, first_availability);

    let (first_manager_samples, second_manager_samples) = steady_manager_sampling_control();
    assert_eq!(first_manager_samples, Stats::default());
    assert_eq!(second_manager_samples, first_manager_samples);

    let (first_typed_samples, second_typed_samples) = steady_typed_manager_sampling_control();
    assert_eq!(first_typed_samples, Stats::default());
    assert_eq!(second_typed_samples, first_typed_samples);

    let (first_audio_samples, second_audio_samples) = steady_audio_manager_sampling_control();
    assert_eq!(first_audio_samples, Stats::default());
    assert_eq!(second_audio_samples, first_audio_samples);

    let (first_browser_samples, second_browser_samples) = steady_browser_sampling_control();
    assert_eq!(first_browser_samples, Stats::default());
    assert_eq!(second_browser_samples, first_browser_samples);

    for (first_smoother, second_smoother) in steady_smoother_control() {
        assert_eq!(first_smoother, Stats::default());
        assert_eq!(second_smoother, first_smoother);
    }

    for (first_screen_push, second_screen_push) in steady_screen_push_control() {
        assert_eq!(second_screen_push, first_screen_push);
        assert_eq!(first_screen_push.reallocations, 0);
        assert!(
            first_screen_push.allocations <= 2,
            "parallel scheduling must stay constant, not scale per zone: {first_screen_push:?}"
        );
        assert!(
            first_screen_push.bytes_allocated <= 128,
            "screen analysis allocated frame-sized state after warmup: {first_screen_push:?}"
        );
    }

    let (first_screen_sample, second_screen_sample) = steady_screen_sample_control();
    assert_eq!(first_screen_sample, Stats::default());
    assert_eq!(second_screen_sample, first_screen_sample);

    let (first_route, second_route) = steady_router_resolution_control();
    assert_eq!(first_route, Stats::default());
    assert_eq!(second_route, first_route);

    let audio_config = AudioPipelineConfig::default();
    let analyzer_construction = audio_input_construction_round(&audio_config);
    let first_audio_commit = prepared_audio_commit_round(&audio_config);
    let second_audio_commit = prepared_audio_commit_round(&audio_config);
    assert_eq!(first_audio_commit, second_audio_commit);
    assert!(
        first_audio_commit.bytes_allocated.saturating_mul(4)
            < analyzer_construction.bytes_allocated,
        "prepared commit rebuilt heavy analyzer state: commit={first_audio_commit:?}, construction={analyzer_construction:?}"
    );

    for commit in [
        prepared_effect_pool_commit_round(false),
        prepared_effect_pool_commit_round(true),
    ] {
        assert_eq!(
            commit.allocations, 0,
            "prepared pool commit allocated: {commit:?}"
        );
        assert_eq!(
            commit.reallocations, 0,
            "prepared pool commit reallocated: {commit:?}"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let data = [
            90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8, 70, 71, 9, 10, 11, 12, 13, 14, 15, 16,
        ];
        let mut buffers =
            DoubleBuffer::try_with_capacity(16).expect("tiny fixture buffers fit in memory");
        assert_eq!(
            decode_chunk(
                &SpaChunkView::new(
                    &data,
                    4,
                    18,
                    10,
                    2,
                    2,
                    SpaVideoFormat::Rgba,
                    None,
                    CaptureRotation::Identity,
                ),
                &mut buffers,
            )
            .drop_reason(),
            None
        );
        let first_decode = wayland_decode_round(&data, &mut buffers);
        let second_decode = wayland_decode_round(&data, &mut buffers);
        assert_eq!(first_decode, Stats::default());
        assert_eq!(second_decode, first_decode);
    }
}
