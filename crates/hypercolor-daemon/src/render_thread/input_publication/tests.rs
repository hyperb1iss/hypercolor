use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hypercolor_core::input::{InputData, InputManager, InputSource, SourceKind};
use tokio::sync::Mutex;

use super::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationPump, InputPublicationStatus, cadence_interval,
};

struct CountingSource {
    samples: Arc<AtomicUsize>,
    running: bool,
}

impl CountingSource {
    fn new(samples: Arc<AtomicUsize>) -> Self {
        Self {
            samples,
            running: false,
        }
    }
}

impl InputSource for CountingSource {
    fn name(&self) -> &'static str {
        "counting"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }
}

#[test]
fn demand_snapshot_uses_the_highest_typed_consumer_rate() {
    let demands = InputPublicationDemandHandle::new();
    let _authoritative = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(60),
    );
    let _preview = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 360),
    );
    let _diagnostic = demands.register(
        InputPublicationConsumer::Diagnostic,
        InputPublicationDemand::default().with_source(SourceKind::Screen, 144),
    );

    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 360);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 144);
    assert_eq!(snapshot.requested_hz(SourceKind::Audio), 60);
}

#[test]
fn concurrent_preview_registrations_union_and_drop_independently() {
    let demands = InputPublicationDemandHandle::new();
    let first = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 120),
    );
    let second = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default()
            .with_source(SourceKind::Interaction, 240)
            .with_source(SourceKind::Screen, 90),
    );

    assert_eq!(
        demands.registration_count(InputPublicationConsumer::Preview),
        2
    );
    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 240);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 90);

    drop(second);
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::Preview),
        1
    );
    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 120);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 0);
    drop(first);
    assert_eq!(demands.snapshot().max_requested_hz(), 0);
}

#[test]
fn authoritative_registration_does_not_blanket_unrequested_source_types() {
    let demands = InputPublicationDemandHandle::new();
    let _authoritative = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_source(SourceKind::Screen, 60),
    );

    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 60);
    assert_eq!(snapshot.requested_hz(SourceKind::Audio), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Media), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Network), 0);
}

#[test]
fn cadence_conversion_does_not_cap_large_requests() {
    assert_eq!(cadence_interval(u32::MAX), Duration::from_nanos(1));
    assert_eq!(cadence_interval(240), Duration::from_nanos(4_166_667));
}

#[tokio::test]
async fn pump_waits_for_live_demand_then_samples_without_render_frames() {
    let samples = Arc::new(AtomicUsize::new(0));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CountingSource::new(Arc::clone(&samples))));
    let manager = Arc::new(Mutex::new(manager));
    let demands = InputPublicationDemandHandle::new();
    let _demand = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(60),
    );
    let mut pump = InputPublicationPump::start(Arc::clone(&manager), demands)
        .await
        .expect("publication pump should start");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(samples.load(Ordering::Relaxed), 0);

    manager
        .lock()
        .await
        .start_all()
        .expect("counting source should start");
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pump should sample a newly live source");

    assert_eq!(pump.monitor().status(), InputPublicationStatus::Ready);
    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
    assert_eq!(pump.monitor().status(), InputPublicationStatus::Stopped);
}

#[tokio::test]
async fn dropping_the_pump_aborts_its_worker() {
    let samples = Arc::new(AtomicUsize::new(0));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CountingSource::new(Arc::clone(&samples))));
    manager.start_all().expect("counting source should start");
    let manager = Arc::new(Mutex::new(manager));
    let demands = InputPublicationDemandHandle::new();
    let _demand = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(120),
    );
    let pump = InputPublicationPump::start(manager, demands)
        .await
        .expect("publication pump should start");
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pump should publish before being dropped");

    drop(pump);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_drop = samples.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(samples.load(Ordering::Relaxed), after_drop);
}

#[tokio::test]
async fn pump_sleeps_with_zero_typed_demand() {
    let samples = Arc::new(AtomicUsize::new(0));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CountingSource::new(Arc::clone(&samples))));
    manager.start_all().expect("counting source should start");
    let mut pump = InputPublicationPump::start(
        Arc::new(Mutex::new(manager)),
        InputPublicationDemandHandle::new(),
    )
    .await
    .expect("publication pump should start");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(samples.load(Ordering::Relaxed), 0);
    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
}
