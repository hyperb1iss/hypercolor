use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use hypercolor_core::input::{InputData, InputManager, InputSource, SourceKind};

struct CountingSource {
    kind: SourceKind,
    samples: Arc<AtomicUsize>,
    delta_bits: Arc<AtomicU32>,
    running: bool,
}

impl CountingSource {
    fn new(kind: SourceKind, samples: Arc<AtomicUsize>, delta_bits: Arc<AtomicU32>) -> Self {
        Self {
            kind,
            samples,
            delta_bits,
            running: false,
        }
    }
}

impl InputSource for CountingSource {
    fn name(&self) -> &'static str {
        "counting-source"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.sample_with_delta_secs(0.0)
    }

    fn sample_with_delta_secs(&mut self, delta_secs: f32) -> anyhow::Result<InputData> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.delta_bits
            .store(delta_secs.to_bits(), Ordering::Relaxed);
        Ok(InputData::None)
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

#[test]
fn typed_sampling_only_publishes_due_source_kinds() {
    let audio_samples = Arc::new(AtomicUsize::new(0));
    let audio_delta = Arc::new(AtomicU32::new(0));
    let interaction_samples = Arc::new(AtomicUsize::new(0));
    let interaction_delta = Arc::new(AtomicU32::new(0));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CountingSource::new(
        SourceKind::Audio,
        Arc::clone(&audio_samples),
        Arc::clone(&audio_delta),
    )));
    manager.add_source(Box::new(CountingSource::new(
        SourceKind::Interaction,
        Arc::clone(&interaction_samples),
        Arc::clone(&interaction_delta),
    )));
    manager.start_all().expect("counting sources should start");

    manager.sample_source_kinds(&[(SourceKind::Interaction, 1.0 / 120.0)]);
    assert_eq!(audio_samples.load(Ordering::Relaxed), 0);
    assert_eq!(interaction_samples.load(Ordering::Relaxed), 1);
    assert_eq!(
        f32::from_bits(interaction_delta.load(Ordering::Relaxed)),
        1.0 / 120.0
    );

    manager.sample_source_kinds(&[
        (SourceKind::Audio, 1.0 / 20.0),
        (SourceKind::Interaction, 1.0 / 240.0),
    ]);
    assert_eq!(audio_samples.load(Ordering::Relaxed), 1);
    assert_eq!(interaction_samples.load(Ordering::Relaxed), 2);
    assert_eq!(
        f32::from_bits(audio_delta.load(Ordering::Relaxed)),
        1.0 / 20.0
    );
    assert_eq!(
        f32::from_bits(interaction_delta.load(Ordering::Relaxed)),
        1.0 / 240.0
    );
}
