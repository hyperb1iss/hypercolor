//! Art-Net source port handling: UDP 6454 when free, an ephemeral port when
//! another application owns it, and release when no Art-Net session needs it.
//!
//! These tests bind 0.0.0.0:6454 themselves, so they run one at a time in
//! their own test binary. Every packet still goes to a 127.0.0.1 receiver.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_pixel::{PixelBackend, PixelConfig, configured_devices};
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

async fn connect_artnet(backend: &PixelBackend, id: &str, receiver: SocketAddr) -> DeviceId {
    let config: PixelConfig = serde_json::from_value(json!({ "devices": [{
        "id": id, "host": "127.0.0.1", "port": receiver.port(), "protocol": "artnet",
        "pixel_count": 1
    }]}))
    .expect("config fixture");
    let device = configured_devices(&config, None)
        .expect("config should resolve")
        .into_iter()
        .next()
        .expect("one device");
    backend.adopt_device(&device).expect("adopt");
    backend.connect(&device.info.id).await.expect("connect");
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
    let backend = PixelBackend::new();
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
    let backend = PixelBackend::new();
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
    let backend = PixelBackend::new();
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
    let backend = PixelBackend::new();
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
