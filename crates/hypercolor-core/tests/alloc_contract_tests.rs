#[cfg(feature = "servo")]
compile_error!(
    "allocation-contract-tests owns the process allocator and cannot be combined with servo; run `just alloc-contracts`"
);

use std::{
    alloc::System,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

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
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputEdge, BrowserInputSource,
    BrowserPreviewId, InputData, InputManager, InputSource, InteractionBatch, InteractionData,
    MotionAggregate, ScreenData, SourceKind, SourceSessionWriter, SourceStatusHandle,
    SourceStatusWriter,
};
use hypercolor_core::types::audio::{AudioData, AudioPipelineConfig};
use hypercolor_core::types::event::TimedInputEvent;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[cfg_attr(not(feature = "servo"), global_allocator)]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

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

struct SharedSampleSource {
    kind: SourceKind,
    sample: Arc<InputData>,
    running: bool,
}

impl SharedSampleSource {
    fn new(kind: SourceKind, sample: InputData) -> Self {
        Self {
            kind,
            sample: Arc::new(sample),
            running: false,
        }
    }
}

impl InputSource for SharedSampleSource {
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

    fn is_audio_source(&self) -> bool {
        self.kind == SourceKind::Audio
    }

    fn is_screen_source(&self) -> bool {
        self.kind == SourceKind::Screen
    }

    fn is_interaction_source(&self) -> bool {
        self.kind == SourceKind::Interaction
    }
}

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
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Audio,
        InputData::Audio(AudioData::silence()),
    )));
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Screen,
        InputData::Screen(ScreenData::from_zones(Vec::new(), 0, 0)),
    )));
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Interaction,
        InputData::Interaction(InteractionData::default()),
    )));
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
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Audio,
        InputData::Audio(AudioData::silence()),
    )));
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Interaction,
        InputData::Interaction(InteractionData::default()),
    )));
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
    manager.add_source(Box::new(AudioInput::new(&AudioPipelineConfig::default())));
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
    manager.add_source(Box::new(SharedSampleSource::new(
        SourceKind::Interaction,
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
    )));
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
    manager.add_source(Box::new(AudioInput::new(config)));
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

    #[cfg(target_os = "linux")]
    {
        let data = [
            90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8, 70, 71, 9, 10, 11, 12, 13, 14, 15, 16,
        ];
        let mut buffers = DoubleBuffer::with_capacity(16);
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
