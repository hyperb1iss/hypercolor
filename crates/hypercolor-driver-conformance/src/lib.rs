//! Contract tests shared by every driver.
//!
//! Drivers depend on this crate from `dev-dependencies` only. Each one
//! implements [`SessionRig`] over the emulator its own tests already use and
//! calls [`assert_session_contract`], so all of them prove the same thing:
//! once a session is closed, a frame sink retained from it rejects, nothing
//! reaches the device, and the session's transport is gone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

    /// Prove the transport of the session that just closed is gone.
    ///
    /// Sockets private to a session show it directly: the peer sees EOF or
    /// the local port can be bound again. A driver sending from one socket
    /// shared by its whole backend has nothing to show here, and proves the
    /// opposite in its own test: closing one session leaves the others
    /// running.
    async fn assert_transport_released(&mut self);
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

/// Run the whole session contract against `rig`.
///
/// # Panics
///
/// Panics when the driver breaks the contract.
pub async fn assert_session_contract(rig: &mut impl SessionRig) {
    let id = rig.device_id();

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

    // Retiring ends the backend: sessions close and connects are refused.
    rig.backend().retire().await.expect("retire");
    rig.assert_transport_released().await;
    assert!(
        rig.backend().frame_sink(&id).is_none(),
        "a retired backend must leave no sink behind"
    );
    assert_sink_is_revoked(rig, second.as_ref(), 60).await;
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
