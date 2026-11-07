//! The session contract, proven against a fake device.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hypercolor_driver_api::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver, DeviceDeliveryStatus,
    DeviceFrameSink, DeviceWriteOutcome,
};
use hypercolor_driver_support::{DeviceSession, FrameDelivery, Session, SessionTable};
use hypercolor_types::device::{DeviceError, DeviceId};
use tokio::sync::{Semaphore, oneshot};

const FRAME: [[u8; 3]; 1] = [[1, 2, 3]];

/// What the fake device saw, kept outside the session so tests can read it
/// after the session is gone.
#[derive(Default)]
struct Wire {
    frames: AtomicUsize,
    finishes: AtomicUsize,
    /// Events in order, to prove what happened before what.
    log: Mutex<Vec<&'static str>>,
}

impl Wire {
    fn note(&self, event: &'static str) {
        self.log.lock().expect("log lock").push(event);
    }

    fn events(&self) -> Vec<&'static str> {
        self.log.lock().expect("log lock").clone()
    }
}

#[derive(Default)]
struct Behavior {
    /// Writes wait for a permit, so a test can hold one in flight.
    gate: Option<Arc<Semaphore>>,
    /// Cleanup never finishes.
    hang_finish: bool,
}

struct FakeSession {
    wire: Arc<Wire>,
    /// Stands in for the transport: its count drops when the state does.
    _transport: Arc<()>,
    behavior: Behavior,
}

impl DeviceSession for FakeSession {
    type Facts = &'static str;

    const SETTLE: Duration = Duration::from_millis(40);
    const CLEANUP: Duration = Duration::from_millis(40);

    async fn write_frame(
        &mut self,
        _colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        if let Some(gate) = &self.behavior.gate {
            let _permit = gate.acquire().await.expect("gate stays open");
        }
        self.wire.frames.fetch_add(1, Ordering::SeqCst);
        self.wire.note("frame");
        Ok(DeviceWriteOutcome::Sent)
    }

    async fn finish(self) -> Result<(), DeviceError> {
        if self.behavior.hang_finish {
            std::future::pending::<()>().await;
        }
        self.wire.finishes.fetch_add(1, Ordering::SeqCst);
        self.wire.note("finish");
        Ok(())
    }
}

struct Fixture {
    id: DeviceId,
    wire: Arc<Wire>,
    transport: Arc<()>,
    table: SessionTable<FakeSession>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            id: DeviceId::new(),
            wire: Arc::default(),
            transport: Arc::new(()),
            table: SessionTable::new(),
        }
    }

    fn session(&self, behavior: Behavior) -> Session<FakeSession> {
        Session::new(
            self.id,
            "facts",
            FakeSession {
                wire: Arc::clone(&self.wire),
                _transport: Arc::clone(&self.transport),
                behavior,
            },
        )
    }

    async fn open(&self, behavior: Behavior) {
        self.table
            .install(self.session(behavior))
            .await
            .expect("install");
    }

    fn transport_released(&self) -> bool {
        Arc::strong_count(&self.transport) == 1
    }

    fn frames(&self) -> usize {
        self.wire.frames.load(Ordering::SeqCst)
    }

    fn finishes(&self) -> usize {
        self.wire.finishes.load(Ordering::SeqCst)
    }
}

fn is_disconnected(error: &DeviceError) -> bool {
    matches!(error, DeviceError::Disconnected { .. })
}

#[derive(Default)]
struct CountingObserver {
    started: AtomicUsize,
}

impl DeviceDeliveryObserver for CountingObserver {
    fn transport_started(&self, _id: DeviceDeliveryId) {
        self.started.fetch_add(1, Ordering::SeqCst);
    }
}

fn delivery_id() -> DeviceDeliveryId {
    DeviceDeliveryId {
        queue_generation: 1,
        sequence: 1,
    }
}

async fn deliver(sink: &dyn DeviceFrameSink, observer: Arc<CountingObserver>) -> DeviceDeliveryAck {
    sink.deliver_colors_shared_observed(delivery_id(), Arc::new(FRAME.to_vec()), observer)
        .await
}

#[tokio::test]
async fn open_session_writes_through_table_and_sink() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;

    assert!(fixture.table.is_open(&fixture.id));
    assert_eq!(fixture.table.facts(&fixture.id).as_deref(), Some(&"facts"));

    let sink = fixture.table.require(&fixture.id).expect("sink");
    assert_eq!(
        sink.write(&FRAME).await.expect("write"),
        DeviceWriteOutcome::Sent
    );

    let frame_sink = fixture.table.frame_sink(&fixture.id).expect("frame sink");
    assert!(!frame_sink.is_closed());
    let observer = Arc::new(CountingObserver::default());
    let ack = deliver(frame_sink.as_ref(), Arc::clone(&observer)).await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
    assert_eq!(observer.started.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.frames(), 2);
}

#[tokio::test]
async fn retained_sink_is_revoked_and_holds_no_state_after_close() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.frame_sink(&fixture.id).expect("sink");

    fixture.table.close(&fixture.id).await.expect("close");

    assert!(fixture.transport_released());
    assert!(sink.is_closed());
    assert_eq!(fixture.finishes(), 1);
    assert!(!fixture.table.is_open(&fixture.id));
    assert!(fixture.table.frame_sink(&fixture.id).is_none());

    let error = sink
        .write_colors_shared(Arc::new(FRAME.to_vec()))
        .await
        .expect_err("retained sink must reject");
    assert!(is_disconnected(&error));

    let observer = Arc::new(CountingObserver::default());
    let ack = deliver(sink.as_ref(), Arc::clone(&observer)).await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(!ack.transport_started);
    assert_eq!(observer.started.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.frames(), 0);
}

#[tokio::test]
async fn closing_an_unknown_device_reports_disconnected() {
    let fixture = Fixture::new();
    let error = fixture
        .table
        .close(&fixture.id)
        .await
        .expect_err("nothing open");
    assert!(is_disconnected(&error));
}

#[tokio::test]
async fn admitted_write_finishes_before_cleanup_runs() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture
        .open(Behavior {
            gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        })
        .await;
    let sink = fixture.table.require(&fixture.id).expect("sink");
    let write = tokio::spawn({
        let sink = sink.clone();
        async move { sink.write(&FRAME).await }
    });
    tokio::task::yield_now().await;

    let close = tokio::spawn({
        let table = Arc::new(fixture);
        let closing = Arc::clone(&table);
        async move {
            let result = closing.table.close(&closing.id).await;
            (table, result)
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    gate.add_permits(1);

    assert_eq!(
        write.await.expect("write task").expect("settled write"),
        DeviceWriteOutcome::Sent
    );
    let (fixture, result) = close.await.expect("close task");
    result.expect("close");
    assert_eq!(fixture.wire.events(), ["frame", "finish"]);
}

#[tokio::test]
async fn stuck_write_is_cancelled_and_cleanup_still_runs() {
    let fixture = Fixture::new();
    fixture
        .open(Behavior {
            gate: Some(Arc::new(Semaphore::new(0))),
            ..Behavior::default()
        })
        .await;
    let sink = fixture.table.require(&fixture.id).expect("sink");
    let write = tokio::spawn({
        let sink = sink.clone();
        async move { sink.write(&FRAME).await }
    });
    tokio::task::yield_now().await;

    fixture.table.close(&fixture.id).await.expect("close");

    let error = write
        .await
        .expect("write task")
        .expect_err("a cancelled write must fail");
    assert!(is_disconnected(&error));
    assert_eq!(fixture.frames(), 0);
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.transport_released());
}

#[tokio::test]
async fn hung_cleanup_times_out_and_still_releases_the_transport() {
    let fixture = Fixture::new();
    fixture
        .open(Behavior {
            hang_finish: true,
            ..Behavior::default()
        })
        .await;

    let error = fixture
        .table
        .close(&fixture.id)
        .await
        .expect_err("cleanup hung");

    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(fixture.transport_released());
    assert!(!fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn owned_tasks_stop_before_cleanup_runs() {
    let fixture = Fixture::new();
    let mut session = fixture.session(Behavior::default());
    let wire = Arc::clone(&fixture.wire);
    let (stopped, stopped_rx) = oneshot::channel::<()>();
    session.spawn(async move {
        std::future::pending::<()>().await;
        drop((stopped, wire));
    });
    fixture.table.install(session).await.expect("install");

    fixture.table.close(&fixture.id).await.expect("close");

    // The task's captures dropped when it was joined.
    assert!(stopped_rx.await.is_err());
    assert_eq!(Arc::strong_count(&fixture.wire), 1);
}

#[tokio::test]
async fn abandoned_caller_does_not_stop_the_close() {
    let fixture = Fixture::new();
    fixture
        .open(Behavior {
            gate: Some(Arc::new(Semaphore::new(0))),
            ..Behavior::default()
        })
        .await;
    let sink = fixture.table.require(&fixture.id).expect("sink");
    let _write = tokio::spawn({
        let sink = sink.clone();
        async move { sink.write(&FRAME).await }
    });
    tokio::task::yield_now().await;

    // The caller gives up long before the stuck write is cancelled.
    let gave_up = tokio::time::timeout(Duration::from_millis(5), fixture.table.close(&fixture.id))
        .await
        .is_err();
    assert!(gave_up);
    assert!(sink.is_revoked());

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.transport_released());
}

#[tokio::test]
async fn reconnect_is_a_new_generation() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let old = fixture.table.frame_sink(&fixture.id).expect("old sink");

    fixture.table.close(&fixture.id).await.expect("close");
    fixture.open(Behavior::default()).await;
    let new = fixture.table.frame_sink(&fixture.id).expect("new sink");

    assert!(old.is_closed());
    assert!(!new.is_closed());
    old.write_colors_shared(Arc::new(FRAME.to_vec()))
        .await
        .expect_err("old generation stays revoked");
    new.write_colors_shared(Arc::new(FRAME.to_vec()))
        .await
        .expect("new generation writes");
    assert_eq!(fixture.frames(), 1);
}

#[tokio::test]
async fn installing_over_a_session_closes_the_displaced_one() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let displaced = fixture.table.frame_sink(&fixture.id).expect("sink");

    fixture.open(Behavior::default()).await;

    assert!(displaced.is_closed());
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn retire_closes_everything_and_refuses_later_installs() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.frame_sink(&fixture.id).expect("sink");

    fixture.table.retire().await.expect("retire");

    assert!(sink.is_closed());
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.transport_released());
    assert!(fixture.table.ensure_accepting(&fixture.id).is_err());

    // A connect that was in flight during retirement finishes opening.
    let late = fixture.session(Behavior::default());
    let late_sink = late.sink();
    let error = fixture
        .table
        .install(late)
        .await
        .expect_err("a retired table refuses installs");

    assert!(matches!(error, DeviceError::ConnectionFailed { .. }));
    assert!(late_sink.is_revoked());
    assert!(!fixture.table.is_open(&fixture.id));
    assert_eq!(fixture.finishes(), 2, "the refused session still cleans up");
    assert!(fixture.transport_released());
}

#[tokio::test]
async fn dropped_session_transmits_nothing_and_releases_state() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.frame_sink(&fixture.id).expect("sink");

    let Fixture {
        table,
        transport,
        wire,
        ..
    } = fixture;
    drop(table);

    assert!(sink.is_closed());
    assert_eq!(Arc::strong_count(&transport), 1);
    sink.write_colors_shared(Arc::new(FRAME.to_vec()))
        .await
        .expect_err("abandoned sessions reject");
    assert_eq!(wire.frames.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dropped_session_releases_state_once_inflight_io_ends() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture
        .open(Behavior {
            gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        })
        .await;
    let sink = fixture.table.require(&fixture.id).expect("sink");
    let write = tokio::spawn({
        let sink = sink.clone();
        async move { sink.write(&FRAME).await }
    });
    tokio::task::yield_now().await;

    let Fixture {
        table, transport, ..
    } = fixture;
    drop(table);

    assert!(write.await.expect("write task").is_err());
    assert_eq!(Arc::strong_count(&transport), 1);
}

#[tokio::test]
async fn sessions_keep_a_shared_transport_alive_for_each_other() {
    let shared = Arc::new(());
    let first = Fixture {
        transport: Arc::clone(&shared),
        ..Fixture::new()
    };
    let second = Fixture {
        transport: Arc::clone(&shared),
        ..Fixture::new()
    };
    first.open(Behavior::default()).await;
    second.open(Behavior::default()).await;

    first.table.close(&first.id).await.expect("close");

    let sink = second.table.require(&second.id).expect("second sink");
    sink.write(&FRAME).await.expect("second keeps writing");
    assert_eq!(second.frames(), 1);
}

#[tokio::test]
async fn busy_sessions_refuse_try_admit() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.require(&fixture.id).expect("sink");

    let held = sink.admit().await.expect("admitted");
    assert!(sink.try_admit().is_none());
    drop(held);
    assert!(sink.try_admit().is_some());

    fixture.table.close(&fixture.id).await.expect("close");
    assert!(sink.try_admit().is_none());
}

#[tokio::test]
async fn transact_reaches_the_state_until_the_session_closes() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.require(&fixture.id).expect("sink");

    sink.transact(async |session| {
        session.write_frame(&FRAME, None).await.expect("write");
    })
    .await
    .expect("open session");
    assert_eq!(fixture.frames(), 1);

    fixture.table.close(&fixture.id).await.expect("close");
    let error = sink
        .transact(async |session| {
            session.write_frame(&FRAME, None).await.expect("write");
        })
        .await
        .expect_err("a closed session rejects");
    assert!(is_disconnected(&error));
    assert_eq!(fixture.frames(), 1);
}

#[tokio::test]
async fn close_cancels_a_stuck_transaction() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.require(&fixture.id).expect("sink");
    let stuck = tokio::spawn({
        let sink = sink.clone();
        async move {
            sink.transact(async |_session| std::future::pending::<()>().await)
                .await
        }
    });
    tokio::task::yield_now().await;

    fixture.table.close(&fixture.id).await.expect("close");

    let error = stuck
        .await
        .expect("transaction task")
        .expect_err("a cancelled transaction must fail");
    assert!(is_disconnected(&error));
    assert_eq!(fixture.finishes(), 1);
}
