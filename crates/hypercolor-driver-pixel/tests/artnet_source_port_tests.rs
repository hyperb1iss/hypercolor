//! Art-Net source port handling: UDP 6454 when free, an ephemeral port when
//! another application owns it, and release when no Art-Net session needs it.
//!
//! These tests bind 0.0.0.0:6454 themselves. The lock serializes them under
//! plain `cargo test`; `.config/nextest.toml` does the same across processes.
//! Every packet still goes to a 127.0.0.1 receiver.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use hypercolor_driver_api::{DeviceBackend, DriverConfigSource, InstallationId};
use hypercolor_driver_pixel::controls::{pixel_config_values, values_entry};
use hypercolor_driver_pixel::{PixelBackend, PixelConfig, configured_devices};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::DeviceId;
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::timeout;

const ARTNET_PORT: u16 = 6454;
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// Serializes every test that touches UDP 6454.
static PORT_6454: Mutex<()> = Mutex::const_new(());

async fn loopback_receiver() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind loopback receiver");
    let addr = socket.local_addr().expect("receiver address");
    (socket, addr)
}

/// The config the backend reads, naming whichever device connects next.
#[derive(Default)]
struct LiveConfig(StdMutex<DriverConfigEntry>);

impl DriverConfigSource for LiveConfig {
    fn current(&self) -> DriverConfigEntry {
        self.0.lock().expect("config lock").clone()
    }
}

struct Rig {
    config: Arc<LiveConfig>,
    backend: PixelBackend,
}

impl std::ops::Deref for Rig {
    type Target = PixelBackend;

    fn deref(&self) -> &PixelBackend {
        &self.backend
    }
}

fn rig() -> Rig {
    let config = Arc::new(LiveConfig::default());
    let backend = PixelBackend::new(
        Arc::clone(&config) as Arc<dyn DriverConfigSource>,
        InstallationId::new("artnet-tests"),
    );
    Rig { config, backend }
}

async fn connect_artnet(rig: &Rig, id: &str, receiver: SocketAddr) -> DeviceId {
    let config: PixelConfig = serde_json::from_value(json!({ "devices": [{
        "id": id, "host": "127.0.0.1", "port": receiver.port(), "protocol": "artnet",
        "pixel_count": 1
    }]}))
    .expect("config fixture");
    *rig.config.0.lock().expect("config lock") =
        values_entry(&pixel_config_values(&config)).expect("entry");
    let device = configured_devices(&config)
        .expect("config should resolve")
        .into_iter()
        .next()
        .expect("one device");
    rig.adopt_device(&device).expect("adopt");
    rig.connect(&device.info.id).await.expect("connect");
    device.info.id
}

async fn source_port_of_next_packet(
    backend: &PixelBackend,
    id: &DeviceId,
    receiver: &UdpSocket,
) -> u16 {
    backend.write_colors(id, &[[1, 2, 3]]).await.expect("write");
    let mut buf = [0u8; 600];
    let (_, from) = timeout(RECV_TIMEOUT, receiver.recv_from(&mut buf))
        .await
        .expect("packet should arrive")
        .expect("receive should succeed");
    from.port()
}

fn bind_6454() -> std::io::Result<std::net::UdpSocket> {
    std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, ARTNET_PORT))
}

#[tokio::test]
async fn artnet_sends_from_6454_when_the_port_is_free() {
    let _guard = PORT_6454.lock().await;
    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let id = connect_artnet(&backend, "free", addr).await;

    assert_eq!(
        backend
            .connected_socket_local_addr(&id)
            .map(|local| local.port()),
        Some(ARTNET_PORT)
    );
    assert_eq!(
        source_port_of_next_packet(&backend, &id, &receiver).await,
        ARTNET_PORT,
        "Art-Net 4 uses UDP 6454 as both source and destination"
    );
    backend.disconnect(&id).await.expect("disconnect");
}

#[tokio::test]
async fn artnet_falls_back_when_another_application_owns_6454() {
    let _guard = PORT_6454.lock().await;
    let other_application = bind_6454().expect("6454 should be free for the test to own");
    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let id = connect_artnet(&backend, "taken", addr).await;

    let port = source_port_of_next_packet(&backend, &id, &receiver).await;
    assert_ne!(port, ARTNET_PORT, "the other application keeps 6454");
    assert_ne!(port, 0);

    // A second device keeps working on the same fallback.
    let second = connect_artnet(&backend, "taken-two", addr).await;
    assert_ne!(
        source_port_of_next_packet(&backend, &second, &receiver).await,
        ARTNET_PORT
    );
    backend.disconnect(&id).await.expect("disconnect");
    backend.disconnect(&second).await.expect("disconnect");
    drop(other_application);
}

/// A coexisting application that binds 6454 with `SO_REUSEPORT` must keep
/// all of its incoming traffic. Joining its reuse group would let the kernel
/// load-balance that traffic onto a socket that never reads.
#[cfg(unix)]
#[tokio::test]
async fn artnet_never_joins_another_applications_reuseport_group() {
    use socket2::{Domain, Protocol, Socket, Type};

    let _guard = PORT_6454.lock().await;
    let other_application =
        Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).expect("socket");
    other_application
        .set_reuse_address(true)
        .expect("SO_REUSEADDR");
    other_application
        .set_reuse_port(true)
        .expect("SO_REUSEPORT");
    other_application
        .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, ARTNET_PORT)).into())
        .expect("6454 should be free for the test to own");

    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let id = connect_artnet(&backend, "polite", addr).await;
    assert_ne!(
        backend
            .connected_socket_local_addr(&id)
            .map(|local| local.port()),
        Some(ARTNET_PORT),
        "the backend must not share 6454 with another application"
    );
    assert_ne!(
        source_port_of_next_packet(&backend, &id, &receiver).await,
        ARTNET_PORT
    );
    backend.disconnect(&id).await.expect("disconnect");
    drop(other_application);
}

#[tokio::test]
async fn artnet_releases_6454_after_its_last_session() {
    let _guard = PORT_6454.lock().await;
    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let first = connect_artnet(&backend, "first", addr).await;
    let second = connect_artnet(&backend, "second", addr).await;
    assert_eq!(
        source_port_of_next_packet(&backend, &second, &receiver).await,
        ARTNET_PORT,
        "sessions share one 6454 socket"
    );

    // The first device never sent a frame, so its disconnect sends nothing.
    backend.disconnect(&first).await.expect("disconnect");
    assert!(
        bind_6454().is_err(),
        "the remaining Art-Net session still holds 6454"
    );

    backend.disconnect(&second).await.expect("disconnect");
    let rebound = bind_6454().expect("6454 is released when no Art-Net session needs it");
    drop(rebound);
}

#[tokio::test]
async fn retained_sink_does_not_keep_6454_bound() {
    let _guard = PORT_6454.lock().await;
    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let id = connect_artnet(&backend, "retained", addr).await;
    assert_eq!(
        source_port_of_next_packet(&backend, &id, &receiver).await,
        ARTNET_PORT
    );

    // The core output queue keeps the sink across disconnect.
    let sink = backend
        .frame_sink(&id)
        .expect("connected device has a sink");
    backend.disconnect(&id).await.expect("disconnect");
    let rebound = bind_6454().expect("disconnect releases 6454 even while a sink is retained");
    assert!(sink.is_closed());
    assert!(
        sink.write_colors_shared_outcome(std::sync::Arc::new(vec![[1, 1, 1]]))
            .await
            .is_err()
    );
    drop(rebound);
    drop(sink);
}

#[tokio::test]
async fn concurrent_reconnects_leave_one_open_session_and_release_6454() {
    let _guard = PORT_6454.lock().await;
    let (receiver, addr) = loopback_receiver().await;
    let backend = rig();
    let id = connect_artnet(&backend, "racing", addr).await;
    let first_sink = backend.frame_sink(&id).expect("sink");

    let (left, right) = tokio::join!(backend.connect(&id), backend.connect(&id));
    left.expect("concurrent connect");
    right.expect("concurrent connect");
    let current_sink = backend.frame_sink(&id).expect("sink");
    assert!(first_sink.is_closed(), "the displaced session is retired");
    assert!(!current_sink.is_closed());

    // One write through the backend reaches the receiver exactly once.
    assert_eq!(
        source_port_of_next_packet(&backend, &id, &receiver).await,
        ARTNET_PORT
    );
    let mut buf = [0u8; 600];
    assert!(
        timeout(Duration::from_millis(200), receiver.recv_from(&mut buf))
            .await
            .is_err(),
        "no orphaned session sends a second copy"
    );

    backend.disconnect(&id).await.expect("disconnect");
    let _ = timeout(Duration::from_millis(200), receiver.recv_from(&mut buf)).await;
    for sink in [&first_sink, &current_sink] {
        assert!(sink.is_closed());
        assert!(
            sink.write_colors_shared_outcome(std::sync::Arc::new(vec![[2, 2, 2]]))
                .await
                .is_err()
        );
    }
    assert!(
        timeout(Duration::from_millis(200), receiver.recv_from(&mut buf))
            .await
            .is_err(),
        "retired sinks never transmit"
    );
    drop(bind_6454().expect("every retired session released 6454"));
}
