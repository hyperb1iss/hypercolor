use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hypercolor_core::input::{
    INPUT_EVENT_RING_CAPACITY, InputData, InputManager, InputSource, InteractionBatch,
    InteractionData, MotionAggregate,
};
use hypercolor_core::types::event::{InputButtonState, InputEvent, TimedInputEvent};

struct SequencedInteractionSource {
    running: bool,
    generation: u64,
}

impl InputSource for SequencedInteractionSource {
    fn name(&self) -> &'static str {
        "sequenced-interaction"
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

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("test generation exhausted");
        let sample = InteractionData {
            generation: self.generation,
            batch: InteractionBatch {
                motion: MotionAggregate {
                    dx: 0.25,
                    dy: -0.125,
                    distance: 0.5,
                },
                window_secs: 0.5,
                dropped_events: u32::from(self.generation.is_multiple_of(5)),
                ..InteractionBatch::default()
            },
            ..InteractionData::default()
        };
        (
            Ok(InputData::Interaction(sample)),
            vec![key_event(self.generation)],
        )
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }
}

fn key_event(seq: u64) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::Key {
            source_id: "coherent-source".to_owned(),
            key: "KeyA".to_owned(),
            state: InputButtonState::Pressed,
        },
        at_ms: seq,
        seq,
        physical_code: None,
        repeat_count: 1,
    }
}

#[test]
fn concurrent_publication_never_tears_sample_from_event_tail() {
    const PUBLICATIONS: u64 = (INPUT_EVENT_RING_CAPACITY as u64) * 8;
    let mut manager = InputManager::new();
    manager.add_source(Box::new(SequencedInteractionSource {
        running: false,
        generation: 0,
    }));
    manager.start_all().expect("source should start");
    let graph = manager.input_graph_handle().snapshot();
    let slot = graph.slots()[0].clone();
    let start = Arc::new(Barrier::new(2));
    let complete = Arc::new(AtomicBool::new(false));
    let writer = {
        let start = Arc::clone(&start);
        let complete = Arc::clone(&complete);
        thread::spawn(move || {
            start.wait();
            for _ in 0..PUBLICATIONS {
                manager.sample_sources(1.0 / 60.0);
                thread::yield_now();
            }
            complete.store(true, Ordering::Release);
        })
    };

    start.wait();
    let mut observed = 0;
    while !complete.load(Ordering::Acquire) || observed < PUBLICATIONS {
        let mut replay = Vec::new();
        let publication = slot.read_publication_since(u64::MAX, &mut replay);
        assert!(replay.is_empty());
        assert_eq!(publication.events.dropped, 0);
        let Some(sample) = publication.sample else {
            thread::yield_now();
            continue;
        };
        let InputData::Interaction(sample) = sample.as_ref() else {
            panic!("expected interaction sample");
        };
        assert_eq!(sample.generation, publication.events.next_cursor);
        let generation = sample.generation as f64;
        assert_eq!(publication.interaction_transients.dx, generation * 0.25);
        assert_eq!(publication.interaction_transients.dy, generation * -0.125);
        assert_eq!(
            publication.interaction_transients.distance,
            generation * 0.5
        );
        assert_eq!(
            publication.interaction_transients.window_secs,
            generation * 0.5
        );
        assert_eq!(
            publication.interaction_transients.dropped_events,
            sample.generation / 5
        );
        assert_eq!(publication.interaction_transients.invalid_events, 0);
        observed = observed.max(sample.generation);
    }
    writer.join().expect("publication writer");

    let mut retained = Vec::new();
    let publication = slot.read_publication_since(0, &mut retained);
    let InputData::Interaction(sample) = publication.sample.expect("final sample").as_ref().clone()
    else {
        panic!("expected interaction sample");
    };
    assert_eq!(sample.generation, PUBLICATIONS);
    assert_eq!(publication.events.next_cursor, PUBLICATIONS);
    assert_eq!(
        publication.events.dropped,
        PUBLICATIONS - INPUT_EVENT_RING_CAPACITY as u64
    );
    assert_eq!(publication.events.dropped_total, publication.events.dropped);
    assert_eq!(retained.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(
        retained.first().expect("oldest retained event").seq,
        publication.events.dropped + 1
    );
    assert_eq!(
        retained.last().expect("newest retained event").seq,
        PUBLICATIONS
    );
    assert_eq!(
        publication.interaction_transients.dx,
        PUBLICATIONS as f64 * 0.25
    );
    assert_eq!(
        publication.interaction_transients.dropped_events,
        PUBLICATIONS / 5
    );
}
