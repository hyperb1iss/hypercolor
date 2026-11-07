//! Contract tests shared by every driver.
//!
//! Drivers depend on this crate from `dev-dependencies` only. Each one
//! implements [`SessionRig`] over the emulator its own tests already use and
//! calls [`assert_session_contract`], so all of them prove the same thing:
//! once a session is closed, a frame sink retained from it rejects, nothing
//! reaches the device, and the session's transport is gone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryId, DeviceDeliveryObserver, DeviceDeliveryStatus, DeviceFrameSink,
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

#[derive(Default)]
struct StartCounter(AtomicUsize);

impl DeviceDeliveryObserver for StartCounter {
    fn transport_started(&self, _id: DeviceDeliveryId) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
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

    assert_disconnected(
        sink.write_colors_shared(Arc::new(rig.frame(shade))).await,
        "a retained sink must reject writes",
    );
    let starts = Arc::new(StartCounter::default());
    let ack = sink
        .deliver_colors_shared_observed(
            DeviceDeliveryId {
                queue_generation: 1,
                sequence: u64::from(shade),
            },
            Arc::new(rig.frame(shade.wrapping_add(1))),
            Arc::clone(&starts) as Arc<dyn DeviceDeliveryObserver>,
        )
        .await;
    assert_eq!(
        ack.status,
        DeviceDeliveryStatus::Failed,
        "a retained sink must fail deliveries"
    );
    assert!(
        !ack.transport_started,
        "a rejected delivery must not claim transport"
    );
    assert_eq!(
        starts.0.load(Ordering::SeqCst),
        0,
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
    sink.write_colors_shared(Arc::new(rig.frame(shade)))
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
    first
        .write_colors_shared(Arc::new(rig.frame(10)))
        .await
        .expect("write to an open session");
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
        .write_colors_shared(Arc::new(rig.frame(40)))
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
