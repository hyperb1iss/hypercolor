//! The session contract, proven against a fake device.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
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
    /// Device-global state a session sets up and tears down, like streaming
    /// mode on a bridge. A second generation overlapping the first shows up
    /// here as a count above one, or as a teardown that hit its replacement.
    engaged: AtomicIsize,
    most_engaged: AtomicIsize,
    opens: AtomicUsize,
    /// Events in order, to prove what happened before what.
    log: Mutex<Vec<&'static str>>,
}

impl Wire {
    fn engage(&self) {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let engaged = self.engaged.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_engaged.fetch_max(engaged, Ordering::SeqCst);
        self.note("open");
    }

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
    /// Cleanup waits for a permit, so a test can hold a close in flight.
    finish_gate: Option<Arc<Semaphore>>,
    /// Transport release waits for a permit.
    release_gate: Option<Arc<Semaphore>>,
    /// Cleanup never finishes.
    hang_finish: bool,
    /// The open waits for a permit, standing in for blocking native work
    /// that dropping a future could not stop.
    open_gate: Option<Arc<Semaphore>>,
    /// The open fails with this.
    open_error: Option<DeviceError>,
}

struct FakeSession {
    wire: Arc<Wire>,
    /// Stands in for the transport: its count drops when the state does.
    _transport: Arc<()>,
    behavior: Behavior,
}

impl DeviceSession for FakeSession {
    type Facts = &'static str;

    const OPEN: Duration = Duration::from_millis(600);
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

    async fn finish(&mut self) -> Result<(), DeviceError> {
        if self.behavior.hang_finish {
            std::future::pending::<()>().await;
        }
        if let Some(gate) = &self.behavior.finish_gate {
            let _permit = gate.acquire().await.expect("gate stays open");
        }
        self.wire.finishes.fetch_add(1, Ordering::SeqCst);
        self.wire.engaged.fetch_sub(1, Ordering::SeqCst);
        self.wire.note("finish");
        Ok(())
    }

    async fn release(self) -> Result<(), DeviceError> {
        if let Some(gate) = &self.behavior.release_gate {
            let _permit = gate.acquire().await.expect("gate stays open");
        }
        self.wire.note("release");
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

    /// What a driver's connect does: hand the table an open that sets the
    /// device up. The yield is where real setup awaits the network.
    async fn connect(&self, mut behavior: Behavior) -> Result<(), DeviceError> {
        self.table
            .open(&self.id, || {
                let wire = Arc::clone(&self.wire);
                let gate = behavior.open_gate.take();
                let failure = behavior.open_error.take();
                let session = self.session(behavior);
                async move {
                    wire.engage();
                    tokio::task::yield_now().await;
                    if let Some(gate) = gate {
                        let _permit = gate.acquire().await.expect("gate stays open");
                    }
                    failure.map_or(Ok(session), Err)
                }
            })
            .await
    }

    async fn open(&self, behavior: Behavior) {
        self.connect(behavior).await.expect("connect");
    }

    async fn publish(&self, session: Session<FakeSession>) {
        self.table
            .open(&self.id, || async move { Ok(session) })
            .await
            .expect("open");
    }

    /// A connect whose open stops at `gate`, and whose caller walks away
    /// while it is there, the way a lifecycle connect timeout does.
    async fn connect_and_give_up(&self, gate: &Arc<Semaphore>) {
        let gave_up = tokio::time::timeout(
            Duration::from_millis(5),
            self.connect(Behavior {
                open_gate: Some(Arc::clone(gate)),
                ..Behavior::default()
            }),
        )
        .await
        .is_err();
        assert!(gave_up, "the open is still at its gate");
    }

    fn opens(&self) -> usize {
        self.wire.opens.load(Ordering::SeqCst)
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
    assert_eq!(
        fixture.wire.events(),
        ["open", "frame", "finish", "release"]
    );
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
    fixture.publish(session).await;

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
async fn concurrent_connects_share_one_open() {
    let fixture = Fixture::new();

    let (first, second, third) = tokio::join!(
        fixture.connect(Behavior::default()),
        fixture.connect(Behavior::default()),
        fixture.connect(Behavior::default()),
    );
    first.expect("first");
    second.expect("second");
    third.expect("third");

    assert_eq!(fixture.opens(), 1);
    assert_eq!(
        fixture.finishes(),
        0,
        "joining a connect tears nothing down"
    );
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn connects_waiting_on_an_open_share_its_failure() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));

    let (first, second, ()) = tokio::join!(
        fixture.connect(Behavior {
            open_gate: Some(Arc::clone(&gate)),
            open_error: Some(DeviceError::connection(fixture.id, "the device said no")),
            ..Behavior::default()
        }),
        async {
            tokio::task::yield_now().await;
            fixture.connect(Behavior::default()).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            gate.add_permits(1);
        },
    );

    assert!(matches!(first, Err(DeviceError::ConnectionFailed { .. })));
    assert_eq!(first, second, "both connects get the one result");
    assert_eq!(fixture.opens(), 1, "nobody retried behind the failure");
    assert!(!fixture.table.is_open(&fixture.id));

    fixture.open(Behavior::default()).await;
    assert!(
        fixture.table.is_open(&fixture.id),
        "a failed open gives the device back"
    );
}

#[tokio::test]
async fn reconnect_waits_for_the_old_generation_to_finish_closing() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture
        .open(Behavior {
            finish_gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        })
        .await;

    let (closed, reopened, ()) = tokio::join!(
        fixture.table.close(&fixture.id),
        async {
            tokio::task::yield_now().await;
            fixture.connect(Behavior::default()).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(fixture.opens(), 1, "the reconnect must wait out the close");
            gate.add_permits(1);
        },
    );
    closed.expect("close");
    reopened.expect("reconnect");

    assert_eq!(
        fixture.wire.events(),
        ["open", "finish", "release", "open"],
        "the old generation is gone before the new one starts"
    );
    assert_eq!(fixture.wire.most_engaged.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.wire.engaged.load(Ordering::SeqCst), 1);
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn a_moved_device_closes_before_its_new_session_opens() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let old = fixture.table.frame_sink(&fixture.id).expect("old sink");

    let session = fixture.session(Behavior::default());
    fixture
        .table
        .open_unless(
            &fixture.id,
            |facts| *facts == "facts",
            || async move { Ok(session) },
        )
        .await
        .expect("keep");
    assert!(!old.is_closed(), "a matching session stays open");

    let session = fixture.session(Behavior::default());
    fixture
        .table
        .open_unless(
            &fixture.id,
            |facts| *facts == "somewhere else",
            || async move { Ok(session) },
        )
        .await
        .expect("move");
    assert!(old.is_closed());
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn close_waits_to_see_how_a_connect_ends() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));

    let (connected, closed, ()) = tokio::join!(
        fixture.connect(Behavior {
            open_gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        }),
        async {
            tokio::task::yield_now().await;
            fixture.table.close(&fixture.id).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            gate.add_permits(1);
        },
    );

    connected.expect("connect");
    closed.expect("close");
    assert!(!fixture.table.is_open(&fixture.id));
    assert_eq!(fixture.finishes(), 1);
}

#[tokio::test]
async fn retire_closes_everything_and_refuses_later_connects() {
    let fixture = Fixture::new();
    fixture.open(Behavior::default()).await;
    let sink = fixture.table.frame_sink(&fixture.id).expect("sink");

    fixture.table.retire().await.expect("retire");

    assert!(sink.is_closed());
    assert_eq!(fixture.finishes(), 1);
    assert!(fixture.transport_released());
    let error = fixture
        .connect(Behavior::default())
        .await
        .expect_err("a retired table refuses connects");
    assert!(matches!(error, DeviceError::ConnectionFailed { .. }));
    assert_eq!(fixture.opens(), 1, "a refused connect starts no open");
    assert!(!fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn retire_waits_for_a_close_already_in_flight() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture
        .open(Behavior {
            finish_gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        })
        .await;

    let (closed, retired, ()) = tokio::join!(
        fixture.table.close(&fixture.id),
        async {
            tokio::task::yield_now().await;
            let retired = fixture.table.retire().await;
            assert!(
                fixture.transport_released(),
                "retire must not return while a close still owns the transport"
            );
            retired
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert!(!fixture.transport_released());
            gate.add_permits(1);
        },
    );
    closed.expect("close");
    retired.expect("retire");
    assert_eq!(fixture.finishes(), 1);
}

#[tokio::test]
async fn retire_waits_for_a_connect_in_flight_and_closes_what_it_opened() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));

    let (connected, retired, ()) = tokio::join!(
        fixture.connect(Behavior {
            open_gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        }),
        async {
            tokio::task::yield_now().await;
            let retired = fixture.table.retire().await;
            assert!(
                fixture.transport_released(),
                "retire must not return while an open still owns the transport"
            );
            retired
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert!(!fixture.transport_released(), "the open is still in flight");
            gate.add_permits(1);
        },
    );

    retired.expect("retire");
    let error = connected.expect_err("a retired table refuses the session");
    assert!(matches!(error, DeviceError::ConnectionFailed { .. }));
    assert!(!fixture.table.is_open(&fixture.id));
    assert_eq!(fixture.finishes(), 1, "the refused session still cleans up");
}

#[tokio::test]
async fn an_open_keeps_running_after_its_connect_gives_up() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture.connect_and_give_up(&gate).await;

    assert!(
        !fixture.transport_released(),
        "the open still owns what it is setting up"
    );

    // Nobody can claim the device out from under the work in flight: this
    // connect joins it instead of starting a second open.
    let (joined, ()) = tokio::join!(fixture.connect(Behavior::default()), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        gate.add_permits(1);
    });

    joined.expect("the second connect shares the open");
    assert_eq!(fixture.opens(), 1);
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn an_open_nobody_waits_for_ends_closed() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture.connect_and_give_up(&gate).await;

    gate.add_permits(1);
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(!fixture.table.is_open(&fixture.id));
    assert_eq!(fixture.finishes(), 1, "it closes through normal cleanup");
    assert!(fixture.transport_released());

    fixture.open(Behavior::default()).await;
    assert!(
        fixture.table.is_open(&fixture.id),
        "the device is free again"
    );
}

#[tokio::test]
async fn retire_waits_for_an_open_whose_connect_gave_up() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture.connect_and_give_up(&gate).await;

    let (retired, ()) = tokio::join!(fixture.table.retire(), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!fixture.transport_released(), "the open is still running");
        gate.add_permits(1);
    });

    retired.expect("retire");
    assert!(fixture.transport_released());
    assert_eq!(fixture.finishes(), 1);
}

#[tokio::test]
async fn an_open_past_its_budget_is_dropped_and_gives_the_device_back() {
    let fixture = Fixture::new();

    let error = fixture
        .connect(Behavior {
            open_gate: Some(Arc::new(Semaphore::new(0))),
            ..Behavior::default()
        })
        .await
        .expect_err("the open never finishes");

    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(fixture.transport_released());
    fixture.open(Behavior::default()).await;
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn a_panicking_open_gives_the_device_back() {
    let fixture = Fixture::new();

    let error = fixture
        .table
        .open(&fixture.id, || async { panic!("the open blew up") })
        .await
        .expect_err("the open never reported");

    assert!(matches!(error, DeviceError::ProtocolError { .. }));
    fixture.open(Behavior::default()).await;
    assert!(fixture.table.is_open(&fixture.id));
}

#[tokio::test]
async fn every_wait_on_a_wedged_close_is_bounded() {
    let fixture = Fixture::new();
    fixture
        .open(Behavior {
            release_gate: Some(Arc::new(Semaphore::new(0))),
            ..Behavior::default()
        })
        .await;

    let bound = Duration::from_secs(2);
    let (first, second, connect) = tokio::join!(
        tokio::time::timeout(bound, fixture.table.close(&fixture.id)),
        async {
            tokio::task::yield_now().await;
            tokio::time::timeout(bound, fixture.table.close(&fixture.id)).await
        },
        async {
            tokio::task::yield_now().await;
            tokio::time::timeout(bound, fixture.connect(Behavior::default())).await
        },
    );

    let moved = fixture.session(Behavior::default());
    let moved = tokio::time::timeout(
        bound,
        fixture
            .table
            .open_unless(&fixture.id, |_| false, || async move { Ok(moved) }),
    )
    .await;

    for waited in [first, second, connect, moved] {
        let error = waited
            .expect("the wait is bounded")
            .expect_err("still closing");
        assert!(matches!(error, DeviceError::Timeout { .. }));
    }
    let error = tokio::time::timeout(bound, fixture.table.retire())
        .await
        .expect("the wait is bounded")
        .expect_err("still closing");
    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(!fixture.transport_released(), "the close still owns it");
}

#[tokio::test]
async fn a_refused_open_answers_its_connect_past_a_wedged_close() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    let bound = Duration::from_secs(2);

    let (connected, retired, ()) = tokio::join!(
        tokio::time::timeout(
            bound,
            fixture.connect(Behavior {
                open_gate: Some(Arc::clone(&gate)),
                release_gate: Some(Arc::new(Semaphore::new(0))),
                ..Behavior::default()
            }),
        ),
        async {
            tokio::task::yield_now().await;
            tokio::time::timeout(bound, fixture.table.retire()).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            gate.add_permits(1);
        },
    );

    let error = connected
        .expect("the wait is bounded")
        .expect_err("a retired table refuses the session");
    assert!(matches!(error, DeviceError::ConnectionFailed { .. }));
    let error = retired
        .expect("the wait is bounded")
        .expect_err("still closing");
    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(!fixture.transport_released(), "the close still owns it");
}

#[tokio::test]
async fn retire_reports_a_failed_cleanup() {
    let fixture = Fixture::new();
    fixture
        .open(Behavior {
            hang_finish: true,
            ..Behavior::default()
        })
        .await;

    let error = fixture.table.retire().await.expect_err("cleanup hung");

    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(fixture.transport_released());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_that_outlives_close_is_a_failure_and_holds_the_generation() {
    let fixture = Fixture::new();
    let mut session = fixture.session(Behavior::default());
    let finished = Arc::new(AtomicBool::new(false));
    // Blocking code never reaches an await, so its abort cannot land.
    session.spawn({
        let finished = Arc::clone(&finished);
        async move {
            std::thread::sleep(Duration::from_millis(250));
            finished.store(true, Ordering::SeqCst);
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    fixture.wire.engage();
    fixture.publish(session).await;

    let error = fixture
        .table
        .close(&fixture.id)
        .await
        .expect_err("a task still running is not a clean close");
    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(
        finished.load(Ordering::SeqCst),
        "close joins a late task before it lets the device go"
    );
    assert!(fixture.transport_released());

    fixture.open(Behavior::default()).await;
}

#[tokio::test]
async fn a_release_that_outlasts_close_keeps_the_device_closing() {
    let fixture = Fixture::new();
    let gate = Arc::new(Semaphore::new(0));
    fixture
        .open(Behavior {
            release_gate: Some(Arc::clone(&gate)),
            ..Behavior::default()
        })
        .await;

    let error = fixture
        .table
        .close(&fixture.id)
        .await
        .expect_err("an unfinished release is not a clean close");
    assert!(matches!(error, DeviceError::Timeout { .. }));
    assert!(!fixture.table.is_open(&fixture.id));
    assert!(!fixture.transport_released(), "the close still owns it");

    let (reopened, ()) = tokio::join!(fixture.connect(Behavior::default()), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fixture.opens(), 1, "a reconnect waits for the transport");
        gate.add_permits(1);
    });
    reopened.expect("reconnect");
    assert_eq!(fixture.opens(), 2);
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
