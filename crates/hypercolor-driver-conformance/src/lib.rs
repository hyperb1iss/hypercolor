//! Contract tests shared by every driver.
//!
//! Drivers depend on this crate from `dev-dependencies` only. Each one
//! implements [`SessionRig`] over the emulator its own tests already use and
//! calls [`assert_session_contract`], so all of them prove the same thing:
//! once a session is closed, a frame sink retained from it rejects, nothing
//! reaches the device, and the session's transport is gone.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeliveryResult, DeviceBackend, DeviceDeliveryId, DeviceFrameSink, Frame, FrameObserver,
    IoStage, Receipt, SkipReason,
};
use hypercolor_types::device::{DeviceError, DeviceId};
use tokio::net::UdpSocket;

/// How long a rig waits for traffic it expects.
pub const TRAFFIC_WAIT: Duration = Duration::from_secs(2);

/// How long a rig listens for traffic that must not arrive.
pub const QUIET_WINDOW: Duration = Duration::from_millis(200);

/// One driver wired to an emulated device on loopback.
#[async_trait::async_trait]
pub trait SessionRig: Send {
    /// Whether the device answers cleanup, so [`Self::teardowns`] already
    /// counts it the moment the driver's close returns. A goodbye written to
    /// a socket and forgotten arrives a little later.
    const TEARDOWN_ANSWERED: bool = false;

    /// The backend under test, with the device already adopted.
    fn backend(&self) -> &dyn DeviceBackend;

    /// The adopted device.
    fn device_id(&self) -> DeviceId;

    /// A frame the driver will not drop as unchanged. Each `shade` must
    /// differ from the last.
    fn frame(&self, shade: u8) -> Vec<[u8; 3]>;

    /// Whether the emulated device receives anything within `window`.
    async fn saw_traffic(&mut self, window: Duration) -> bool;

    /// Forget everything the device received so far, including cleanup
    /// traffic that may still be in the socket buffer.
    async fn discard_traffic(&mut self);

    /// Prove no session's transport is left: everything the emulator saw
    /// opened has closed.
    ///
    /// Sockets private to a session show it directly: the peer sees EOF or
    /// the local port can be bound again. A driver sending from one socket
    /// shared by its whole backend has nothing to show here, and proves the
    /// opposite in its own test: closing one session leaves the others
    /// running. Either way, a goodbye that nothing answers has landed by the
    /// time this returns, so [`Self::teardowns`] can be read after it.
    async fn assert_transport_released(&mut self);

    /// How many times session cleanup has reached the device: a stream stop,
    /// a mode restore, a realtime exit. Stays zero for a protocol with no
    /// such goodbye.
    async fn teardowns(&mut self) -> usize {
        0
    }

    /// Whether the device is still in the state an open session puts it in,
    /// as far as the emulator can see. This is state outside any one
    /// connection, the kind one generation's cleanup could take from the
    /// next. Stays `true` for a protocol with no such state.
    async fn engaged(&mut self) -> bool {
        true
    }
}

/// What an observer saw of the attempts offered to one lane.
#[derive(Default)]
pub struct DeliveryLog {
    started: Mutex<Vec<DeviceDeliveryId>>,
    receipts: Mutex<Vec<Receipt>>,
}

impl DeliveryLog {
    /// A frame for `id` that reports to this log.
    #[must_use]
    pub fn frame(self: &Arc<Self>, id: DeviceId, sequence: u64, colors: Vec<[u8; 3]>) -> Frame {
        Frame::new(
            id,
            DeviceDeliveryId {
                queue_generation: 1,
                sequence,
            },
            Arc::clone(self) as Arc<dyn FrameObserver>,
            Instant::now(),
            Arc::new(colors),
        )
    }

    /// Sequences whose transport started, in order.
    ///
    /// # Panics
    ///
    /// Panics when the log's lock is poisoned.
    #[must_use]
    pub fn started(&self) -> Vec<u64> {
        self.started
            .lock()
            .expect("start log")
            .iter()
            .map(|id| id.sequence)
            .collect()
    }

    /// Receipts so far, in settling order.
    ///
    /// # Panics
    ///
    /// Panics when the log's lock is poisoned.
    #[must_use]
    pub fn receipts(&self) -> Vec<Receipt> {
        self.receipts.lock().expect("receipt log").clone()
    }

    /// Wait for `count` attempts to settle.
    ///
    /// # Panics
    ///
    /// Panics when they have not within [`TRAFFIC_WAIT`].
    pub async fn settled(&self, count: usize) -> Vec<Receipt> {
        let deadline = Instant::now() + TRAFFIC_WAIT;
        loop {
            let receipts = self.receipts();
            if receipts.len() >= count {
                return receipts;
            }
            assert!(
                Instant::now() < deadline,
                "{} of {count} attempts settled",
                receipts.len()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl FrameObserver for DeliveryLog {
    fn transport_started(&self, id: DeviceDeliveryId) {
        self.started.lock().expect("start log").push(id);
    }

    fn settled(&self, receipt: &Receipt) {
        self.receipts
            .lock()
            .expect("receipt log")
            .push(receipt.clone());
    }
}

/// Offer one frame to a backend outside the host queue and wait for how it
/// ended.
#[async_trait::async_trait]
pub trait OfferToBackend {
    /// See the trait.
    async fn offer(&self, id: &DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Receipt;
}

#[async_trait::async_trait]
impl<T: DeviceBackend + ?Sized> OfferToBackend for T {
    async fn offer(&self, id: &DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Receipt {
        let mut frame = Frame::direct(*id, colors);
        let receipt = frame.wait_for_receipt();
        let _ = self.submit_frame(id, frame).await;
        receipt.received().await.expect("an attempt always settles")
    }
}

/// Offer one frame to a sink outside the host queue and wait for how it
/// ended.
#[async_trait::async_trait]
pub trait OfferToSink {
    /// See the trait.
    async fn offer_to_sink(&self, id: DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Receipt;
}

#[async_trait::async_trait]
impl<T: DeviceFrameSink + ?Sized> OfferToSink for T {
    async fn offer_to_sink(&self, id: DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Receipt {
        let mut frame = Frame::direct(id, colors);
        let receipt = frame.wait_for_receipt();
        let _ = self.submit(frame).await;
        receipt.received().await.expect("an attempt always settles")
    }
}

/// Assert the transport completed the write.
///
/// # Panics
///
/// Panics on any other result.
#[track_caller]
pub fn assert_written(receipt: &Receipt) {
    assert!(
        matches!(receipt.result(), DeliveryResult::Written { .. }),
        "expected a written frame, got {:?}",
        receipt.result()
    );
}

/// Assert the lane skipped a frame the device already shows.
///
/// # Panics
///
/// Panics on any other result.
#[track_caller]
pub fn assert_unchanged(receipt: &Receipt) {
    assert!(
        matches!(
            receipt.result(),
            DeliveryResult::NotWritten(SkipReason::Unchanged)
        ),
        "expected an unchanged frame, got {:?}",
        receipt.result()
    );
}

/// Assert the lane could not take the frame yet.
///
/// # Panics
///
/// Panics on any other result.
#[track_caller]
pub fn assert_deferred(receipt: &Receipt) {
    assert!(
        matches!(
            receipt.result(),
            DeliveryResult::NotWritten(SkipReason::Deferred { .. })
        ),
        "expected a deferred frame, got {:?}",
        receipt.result()
    );
}

fn assert_disconnected(result: Result<(), DeviceError>, operation: &str) {
    let error = result.expect_err(operation);
    assert!(
        matches!(error, DeviceError::Disconnected { .. }),
        "{operation}: expected Disconnected, got {error}"
    );
}

async fn assert_sink_is_revoked(rig: &mut impl SessionRig, sink: &dyn DeviceFrameSink, shade: u8) {
    assert!(sink.is_closed(), "a closed session's sink must say so");
    rig.discard_traffic().await;
    let id = rig.device_id();

    assert_disconnected(
        sink.write_direct(id, Arc::new(rig.frame(shade))).await,
        "a retained sink must reject writes",
    );
    let log = Arc::new(DeliveryLog::default());
    let frame = log.frame(id, u64::from(shade), rig.frame(shade.wrapping_add(1)));
    let _ = sink.submit(frame).await;
    let receipts = log.settled(1).await;
    assert_eq!(receipts.len(), 1, "an attempt settles exactly once");
    assert!(
        matches!(
            receipts[0].result(),
            DeliveryResult::Failed {
                stage: IoStage::Admission,
                ..
            }
        ),
        "a retained sink must refuse a frame before transport: {:?}",
        receipts[0].result()
    );
    assert!(
        log.started().is_empty(),
        "a rejected delivery must not report transport start"
    );
    assert!(
        !rig.saw_traffic(QUIET_WINDOW).await,
        "nothing may reach the device after its session closes"
    );
}

/// Tasks alive on the current runtime.
fn alive_tasks() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

/// Write one frame through a fresh sink and require it to reach the device.
async fn assert_stream_works(rig: &mut impl SessionRig, shade: u8, what: &str) {
    let id = rig.device_id();
    let sink = rig
        .backend()
        .frame_sink(&id)
        .unwrap_or_else(|| panic!("{what}: an open session has a sink"));
    rig.discard_traffic().await;
    sink.write_direct(id, Arc::new(rig.frame(shade)))
        .await
        .unwrap_or_else(|error| panic!("{what}: write failed: {error}"));
    assert!(
        rig.saw_traffic(TRAFFIC_WAIT).await,
        "{what}: the frame must reach the device"
    );
    assert!(rig.engaged().await, "{what}: the device must stay engaged");
}

/// Run the whole session contract against `rig`.
///
/// # Panics
///
/// Panics when the driver breaks the contract.
pub async fn assert_session_contract<R: SessionRig>(rig: &mut R) {
    let id = rig.device_id();
    let tasks_at_rest = alive_tasks();

    rig.backend().connect(&id).await.expect("connect");
    let first = rig.backend().frame_sink(&id).expect("connected sink");
    assert!(!first.is_closed(), "a fresh sink must be open");
    let colors = rig.frame(10);
    let bytes = u64::try_from(colors.len() * 3).expect("frame size");
    let log = Arc::new(DeliveryLog::default());
    let _ = first.submit(log.frame(id, 1, colors)).await;
    let receipts = log.settled(1).await;
    assert_eq!(receipts.len(), 1, "an attempt settles exactly once");
    assert_eq!(
        log.started(),
        [1],
        "a written frame reports its transport start"
    );
    match receipts[0].result() {
        DeliveryResult::Written { bytes: written, .. } => assert_eq!(
            *written, bytes,
            "written bytes count the frame, three per LED, excluding framing"
        ),
        other => panic!("a frame to an open session must be written: {other:?}"),
    }
    assert!(
        rig.saw_traffic(TRAFFIC_WAIT).await,
        "an open session must reach the device"
    );

    rig.backend().disconnect(&id).await.expect("disconnect");
    rig.assert_transport_released().await;
    assert!(
        rig.backend().frame_sink(&id).is_none(),
        "a closed session must leave no sink behind"
    );
    assert_sink_is_revoked(rig, first.as_ref(), 20).await;
    assert_disconnected(
        rig.backend().write_colors(&id, &rig.frame(30)).await,
        "a closed session must reject direct writes",
    );

    // Reconnecting opens a new generation and never revives the old sink.
    rig.backend().connect(&id).await.expect("reconnect");
    let second = rig.backend().frame_sink(&id).expect("reconnected sink");
    assert!(!second.is_closed(), "the new generation must be open");
    second
        .write_direct(id, Arc::new(rig.frame(40)))
        .await
        .expect("write to the new generation");
    assert!(
        rig.saw_traffic(TRAFFIC_WAIT).await,
        "the new generation must reach the device"
    );
    assert_sink_is_revoked(rig, first.as_ref(), 50).await;

    assert_generations_never_overlap(rig).await;

    // Retiring ends the backend: sessions close and connects are refused.
    // A close already in flight is part of what it waits for, even one whose
    // caller stopped waiting, or the old backend's cleanup could land on its
    // replacement.
    let live = rig.backend().frame_sink(&id).expect("live sink");
    let teardowns = rig.teardowns().await;
    let abandoned =
        tokio::time::timeout(Duration::from_millis(1), rig.backend().disconnect(&id)).await;
    if let Ok(closed) = abandoned {
        closed.expect("disconnect before retiring");
    }
    rig.backend().retire().await.expect("retire");
    if R::TEARDOWN_ANSWERED {
        assert!(
            rig.teardowns().await > teardowns,
            "retiring must wait for a close already in flight"
        );
    }
    rig.assert_transport_released().await;
    assert!(
        rig.backend().frame_sink(&id).is_none(),
        "a retired backend must leave no sink behind"
    );
    assert_sink_is_revoked(rig, live.as_ref(), 60).await;
    rig.backend()
        .connect(&id)
        .await
        .expect_err("a retired backend must refuse connects");
    assert!(
        rig.backend().frame_sink(&id).is_none(),
        "a refused connect must publish nothing"
    );
    assert!(
        !rig.saw_traffic(QUIET_WINDOW).await,
        "a refused connect must leave the device alone"
    );

    assert_no_task_outlives_sessions(tasks_at_rest).await;
}

/// One generation of a device at a time: a teardown only ever hits the
/// session it belongs to, wherever device state lives.
async fn assert_generations_never_overlap(rig: &mut impl SessionRig) {
    let id = rig.device_id();

    // Concurrent connects of an open device share it.
    let teardowns = rig.teardowns().await;
    {
        let backend = rig.backend();
        let (first, second) = tokio::join!(backend.connect(&id), backend.connect(&id));
        first.expect("first connect of an open device");
        second.expect("second connect of an open device");
    }
    assert_eq!(
        rig.teardowns().await,
        teardowns,
        "connecting an open device must not tear it down"
    );
    assert_stream_works(rig, 70, "after connecting an open device").await;

    // Concurrent connects of a closed device open it once.
    rig.backend().disconnect(&id).await.expect("disconnect");
    rig.assert_transport_released().await;
    let teardowns = rig.teardowns().await;
    {
        let backend = rig.backend();
        let (first, second) = tokio::join!(backend.connect(&id), backend.connect(&id));
        first.expect("first concurrent connect");
        second.expect("second concurrent connect");
    }
    assert_eq!(
        rig.teardowns().await,
        teardowns,
        "the loser of a connect race must not tear down the winner"
    );
    assert_stream_works(rig, 80, "after concurrent connects").await;

    // A reconnect that overlaps a disconnect waits for its cleanup, so that
    // cleanup cannot land on the new session.
    let teardowns = rig.teardowns().await;
    {
        let backend = rig.backend();
        let (closed, reopened) = tokio::join!(backend.disconnect(&id), async {
            tokio::task::yield_now().await;
            backend.connect(&id).await
        });
        closed.expect("disconnect overlapping a reconnect");
        reopened.expect("reconnect overlapping a disconnect");
    }
    assert!(
        rig.teardowns().await <= teardowns + 1,
        "one close is at most one teardown"
    );
    assert_stream_works(rig, 90, "after a reconnect overlapped a disconnect").await;
}

/// Everything a session spawned is gone, however quiet it was.
async fn assert_no_task_outlives_sessions(tasks_at_rest: usize) {
    let deadline = Instant::now() + TRAFFIC_WAIT;
    while alive_tasks() > tasks_at_rest {
        assert!(
            Instant::now() < deadline,
            "{} tasks alive after every session closed, {tasks_at_rest} before the first one",
            alive_tasks()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Assert that a session's private UDP port is free again.
///
/// # Panics
///
/// Panics when something in this process still holds the port.
pub async fn assert_udp_port_released(port: u16) {
    UdpSocket::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|error| panic!("UDP port {port} is still held: {error}"));
}
