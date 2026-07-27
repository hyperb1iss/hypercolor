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

use hypercolor_core::input::{
    InputData, InputManager, InputSource, InteractionData, ScreenData, SourceKind,
    SourceSessionWriter, SourceStatusWriter,
};
use hypercolor_core::types::audio::AudioData;
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

    let (first_samples, second_samples) = steady_source_sample_control();
    assert_eq!(first_samples, Stats::default());
    assert_eq!(second_samples, Stats::default());

    let (first_manager_samples, second_manager_samples) = steady_manager_sampling_control();
    assert_eq!(first_manager_samples, Stats::default());
    assert_eq!(second_manager_samples, first_manager_samples);
}
