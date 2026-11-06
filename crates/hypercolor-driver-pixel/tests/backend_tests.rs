//! Loopback integration tests for the pixel controller backend.
//!
//! Every test binds its receiver on 127.0.0.1 and configures the device to
//! stream there. Nothing leaves the host: no LAN, multicast, or broadcast.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryId, DeviceDeliveryObserver, DeviceDeliveryStatus,
    DeviceWriteOutcome, DiscoveredDevice, DriverConfigSource, InstallationId, OutputCadence,
};
use hypercolor_driver_pixel::controls::{pixel_config_values, values_entry};
use hypercolor_driver_pixel::{
    KEEPALIVE_INTERVAL, METADATA_DEVICE_CONFIG, PixelBackend, PixelConfig, configured_devices,
    derived_cid,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{DeviceError, DeviceId};
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::time::timeout;

const RECV_TIMEOUT: Duration = Duration::from_secs(2);
const QUIET_WINDOW: Duration = Duration::from_millis(200);

async fn loopback_receiver() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind loopback receiver");
    let addr = socket.local_addr().expect("receiver address");
    (socket, addr)
}

async fn recv(socket: &UdpSocket) -> Vec<u8> {
    let mut buf = vec![0u8; 2048];
    let (len, _) = timeout(RECV_TIMEOUT, socket.recv_from(&mut buf))
        .await
        .expect("packet should arrive")
        .expect("receive should succeed");
    buf.truncate(len);
    buf
}

async fn recv_n(socket: &UdpSocket, count: usize) -> Vec<Vec<u8>> {
    let mut packets = Vec::with_capacity(count);
    for _ in 0..count {
        packets.push(recv(socket).await);
    }
    packets
}

async fn assert_quiet(socket: &UdpSocket) {
    let mut buf = [0u8; 2048];
    assert!(
        timeout(QUIET_WINDOW, socket.recv_from(&mut buf))
            .await
            .is_err(),
        "no packet expected"
    );
}

fn installation() -> InstallationId {
    InstallationId::new("backend-tests")
}

fn config_for(device: serde_json::Value) -> PixelConfig {
    serde_json::from_value(json!({ "devices": [device] })).expect("config fixture")
}

fn entry_for(config: &PixelConfig) -> DriverConfigEntry {
    values_entry(&pixel_config_values(config)).expect("entry")
}

/// The first discovery result a scan of `config` produces.
fn first_device(config: &PixelConfig) -> DiscoveredDevice {
    configured_devices(config)
        .expect("config should resolve")
        .remove(0)
}

/// A live driver config the test edits while the backend runs, standing in
/// for the daemon's config manager.
#[derive(Default)]
struct LiveConfig(StdMutex<DriverConfigEntry>);

impl LiveConfig {
    fn set(&self, config: &PixelConfig) {
        *self.0.lock().expect("config lock") = entry_for(config);
    }

    fn clear(&self) {
        self.set(&PixelConfig::default());
    }

    fn disable(&self) {
        self.0.lock().expect("config lock").enabled = false;
    }
}

impl DriverConfigSource for LiveConfig {
    fn current(&self) -> DriverConfigEntry {
        self.0.lock().expect("config lock").clone()
    }
}

/// A backend authorized by a [`LiveConfig`] the test controls.
struct Rig {
    config: Arc<LiveConfig>,
    backend: PixelBackend,
}

impl Rig {
    fn new() -> Self {
        Self::with_installation(installation())
    }

    fn with_installation(installation_id: InstallationId) -> Self {
        let config = Arc::new(LiveConfig::default());
        let backend = PixelBackend::new(
            Arc::clone(&config) as Arc<dyn DriverConfigSource>,
            installation_id,
        );
        Self { config, backend }
    }

    /// Configure `config`, then discover, adopt, and connect its first device.
    async fn connected(&self, config: &PixelConfig) -> DiscoveredDevice {
        self.config.set(config);
        let device = first_device(config);
        self.backend.adopt_device(&device).expect("adopt");
        self.backend
            .connect(&device.info.id)
            .await
            .expect("connect");
        device
    }
}

/// Discover, adopt, and connect the single device in `config`.
async fn connect(config: &PixelConfig) -> (PixelBackend, DeviceId) {
    let rig = Rig::new();
    let id = rig.connected(config).await.info.id;
    (rig.backend, id)
}

fn gradient(count: usize) -> Vec<[u8; 3]> {
    (0..count)
        .map(|i| {
            let i = u8::try_from(i % 256).expect("fits");
            [i, i.wrapping_add(1), i.wrapping_add(2)]
        })
        .collect()
}

fn u16_be(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

fn u32_be(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

// ── E1.31 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn e131_frames_split_into_universes_with_per_universe_sequences() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "falcon", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 200, "start_universe": 5
    }));
    let (backend, id) = connect(&config).await;
    let colors = gradient(200);

    for frame in 1..=2_u8 {
        backend.write_colors(&id, &colors).await.expect("write");
        let packets = recv_n(&receiver, 2).await;
        for (index, packet) in packets.iter().enumerate() {
            let universe = 5 + u16::try_from(index).expect("fits");
            assert_eq!(&packet[4..16], b"ASC-E1.17\0\0\0");
            assert_eq!(
                &packet[22..38],
                derived_cid(&installation(), "falcon").as_bytes()
            );
            assert_eq!(&packet[44..54], b"Hypercolor");
            assert_eq!(packet[108], 100, "standard default priority");
            assert_eq!(u16_be(packet, 109), 0, "no sync address");
            assert_eq!(packet[111], frame, "sequence per universe");
            assert_eq!(packet[112], 0, "no options");
            assert_eq!(u16_be(packet, 113), universe);
        }
        let first = &packets[0];
        assert_eq!(first.len(), 126 + 510);
        assert_eq!(&first[126..129], &[0, 1, 2]);
        let second = &packets[1];
        assert_eq!(second.len(), 126 + 90, "30 remaining pixels");
        assert_eq!(
            u16_be(second, 123),
            91,
            "property count includes START code"
        );
        // Pixel 170 opens universe 6.
        assert_eq!(&second[126..129], &[170, 171, 172]);
    }
}

#[tokio::test]
async fn e131_sync_packet_follows_each_frame() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "synced", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 171, "sync_universe": 7000, "priority": 150, "source_name": "Stage"
    }));
    let (backend, id) = connect(&config).await;

    for frame in 1..=2_u8 {
        backend
            .write_colors(&id, &gradient(171))
            .await
            .expect("write");
        let packets = recv_n(&receiver, 3).await;
        for data in &packets[..2] {
            assert_eq!(
                u16_be(data, 109),
                7000,
                "data packets name the sync address"
            );
            assert_eq!(data[108], 150);
            assert_eq!(&data[44..49], b"Stage");
        }
        let sync = &packets[2];
        assert_eq!(sync.len(), 49);
        assert_eq!(u32_be(sync, 18), 0x0000_0008, "VECTOR_ROOT_E131_EXTENDED");
        assert_eq!(u32_be(sync, 40), 0x0000_0001, "sync framing vector");
        assert_eq!(sync[44], frame, "sync packets keep their own sequence");
        assert_eq!(u16_be(sync, 45), 7000);
    }
}

#[tokio::test]
async fn e131_disconnect_blacks_out_then_terminates_each_universe() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "bye", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 171
    }));
    let (backend, id) = connect(&config).await;
    backend
        .write_colors(&id, &vec![[255, 255, 255]; 171])
        .await
        .expect("write");
    recv_n(&receiver, 2).await;

    backend.disconnect(&id).await.expect("disconnect");
    let packets = recv_n(&receiver, 2 + 2 * 3).await;
    for black in &packets[..2] {
        assert_eq!(black[112], 0);
        assert!(black[126..].iter().all(|&slot| slot == 0), "black frame");
    }
    for terminated in &packets[2..] {
        assert_eq!(terminated[112], 0x40, "Stream_Terminated");
    }
    let universe_one: Vec<u8> = packets
        .iter()
        .filter(|packet| u16_be(packet, 113) == 1)
        .map(|packet| packet[111])
        .collect();
    assert_eq!(universe_one, vec![2, 3, 4, 5], "sequence keeps counting");
    assert_quiet(&receiver).await;

    assert!(matches!(
        backend.write_colors(&id, &gradient(1)).await,
        Err(DeviceError::Disconnected { .. })
    ));
}

#[tokio::test]
async fn disconnect_without_output_sends_nothing() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "idle", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 10
    }));
    let (backend, id) = connect(&config).await;
    backend.disconnect(&id).await.expect("disconnect");
    assert_quiet(&receiver).await;
}

// ── Art-Net ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn artnet_frames_address_port_and_zero_fill_lower_slots() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "pars", "host": "127.0.0.1", "port": addr.port(), "protocol": "artnet",
        "pixel_count": 2, "start_universe": 0x0123, "start_channel": 17, "color_order": "rgbw"
    }));
    let (backend, id) = connect(&config).await;

    for frame in 1..=2_u8 {
        backend
            .write_colors(&id, &[[255, 128, 0], [10, 20, 30]])
            .await
            .expect("write");
        let packet = recv(&receiver).await;
        assert_eq!(&packet[..8], b"Art-Net\0");
        assert_eq!(&packet[8..10], &[0x00, 0x50], "OpDmx low byte first");
        assert_eq!(&packet[10..12], &[0x00, 0x0E]);
        assert_eq!(packet[12], frame, "sequence");
        assert_eq!(packet[14], 0x23, "SubUni");
        assert_eq!(packet[15], 0x01, "Net");
        // Slots 1-16 unused, then two RGBW pixels = 24 channels.
        assert_eq!(u16_be(&packet, 16), 24);
        let data = &packet[18..];
        assert!(data[..16].iter().all(|&slot| slot == 0));
        assert_eq!(&data[16..20], &[255, 128, 0, 0]);
        assert_eq!(&data[20..24], &[0, 10, 20, 10]);
    }

    backend.disconnect(&id).await.expect("disconnect");
    let black = recv(&receiver).await;
    assert!(black[18..].iter().all(|&slot| slot == 0));
    assert_quiet(&receiver).await;
}

/// The CID the connected device stamps on its first E1.31 packet.
async fn wire_cid(installation: InstallationId, pinned: Option<&str>) -> Vec<u8> {
    let (receiver, addr) = loopback_receiver().await;
    let mut config = config_for(json!({
        "id": "falcon", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 1
    }));
    config.devices[0].cid = pinned.map(|cid| cid.parse().expect("valid UUID"));
    let rig = Rig::with_installation(installation);
    let id = rig.connected(&config).await.info.id;
    rig.backend
        .write_colors(&id, &[[1, 2, 3]])
        .await
        .expect("write");
    recv(&receiver).await[22..38].to_vec()
}

#[tokio::test]
async fn derived_cids_are_scoped_to_the_installation_on_the_wire() {
    let install_a = InstallationId::new("install-a");
    let first = wire_cid(install_a.clone(), None).await;
    assert_eq!(first, wire_cid(install_a.clone(), None).await, "stable");
    assert_eq!(first, derived_cid(&install_a, "falcon").as_bytes());
    assert_ne!(
        first,
        wire_cid(InstallationId::new("install-b"), None).await,
        "another installation with the same device id differs"
    );
}

#[tokio::test]
async fn a_pinned_cid_survives_installation_scoping_on_the_wire() {
    let pinned = "6f1c7a2e-3b4d-4e5f-8a9b-0c1d2e3f4a5b";
    let expected = uuid::Uuid::parse_str(pinned).expect("valid UUID");
    assert_eq!(
        wire_cid(InstallationId::new("install-a"), Some(pinned)).await,
        expected.as_bytes()
    );
}

#[tokio::test]
async fn e131_and_ddp_keep_ephemeral_source_ports() {
    for (protocol, standard_port) in [("e131", 5568), ("ddp", 4048)] {
        let (receiver, addr) = loopback_receiver().await;
        let config = config_for(json!({
            "id": "ephemeral", "host": "127.0.0.1", "port": addr.port(), "protocol": protocol,
            "pixel_count": 1
        }));
        let (backend, id) = connect(&config).await;
        backend
            .write_colors(&id, &[[1, 2, 3]])
            .await
            .expect("write");
        let mut buf = [0u8; 700];
        let (len, from) = timeout(RECV_TIMEOUT, receiver.recv_from(&mut buf))
            .await
            .expect("packet should arrive")
            .expect("receive should succeed");
        let expected_len = if protocol == "e131" { 126 + 3 } else { 10 + 3 };
        assert_eq!(len, expected_len, "{protocol} packet arrived");
        assert_ne!(
            from.port(),
            standard_port,
            "{protocol} sends from an ephemeral port"
        );
        assert_ne!(from.port(), 6454, "{protocol} never uses the Art-Net port");
    }
}

#[tokio::test]
async fn artnet_odd_channel_counts_pad_to_even() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "odd", "host": "127.0.0.1", "port": addr.port(), "protocol": "artnet",
        "pixel_count": 1
    }));
    let (backend, id) = connect(&config).await;
    backend
        .write_colors(&id, &[[1, 2, 3]])
        .await
        .expect("write");
    let packet = recv(&receiver).await;
    assert_eq!(u16_be(&packet, 16), 4);
    assert_eq!(&packet[18..], &[1, 2, 3, 0]);
}

// ── DDP ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ddp_frames_fragment_and_push_on_the_last_packet() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "fpp", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 600, "ddp_offset": 30
    }));
    let (backend, id) = connect(&config).await;
    let colors = gradient(600);

    backend.write_colors(&id, &colors).await.expect("write");
    let packets = recv_n(&receiver, 2).await;
    assert_eq!(packets[0][0], 0x40, "first packet holds the latch");
    assert_eq!(packets[1][0], 0x41, "last packet pushes");
    assert_eq!(
        (packets[0][1], packets[1][1]),
        (1, 2),
        "each packet takes the next sequence number"
    );
    for packet in &packets {
        assert_eq!(packet[2], 0x0B, "RGB 8-bit");
        assert_eq!(packet[3], 0x01, "default output device");
    }
    assert_eq!(u32_be(&packets[0], 4), 30);
    assert_eq!(u16_be(&packets[0], 8), 1440);
    assert_eq!(u32_be(&packets[1], 4), 30 + 1440);
    assert_eq!(u16_be(&packets[1], 8), 360);
    assert_eq!(&packets[1][10..13], &[224, 225, 226], "pixel 480");

    backend.write_colors(&id, &colors).await.expect("write");
    let next = recv_n(&receiver, 2).await;
    assert_eq!((next[0][1], next[1][1]), (3, 4));
}

#[tokio::test]
async fn ddp_segments_land_at_their_offsets() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "ports", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "segments": [{ "pixel_count": 2 }, { "pixel_count": 1, "ddp_offset": 3000 }]
    }));
    let (backend, id) = connect(&config).await;
    backend
        .write_colors(&id, &[[1, 1, 1], [2, 2, 2], [3, 3, 3]])
        .await
        .expect("write");
    let packets = recv_n(&receiver, 2).await;
    assert_eq!((packets[0][0], u32_be(&packets[0], 4)), (0x40, 0));
    assert_eq!(&packets[0][10..], &[1, 1, 1, 2, 2, 2]);
    assert_eq!((packets[1][0], u32_be(&packets[1], 4)), (0x41, 3000));
    assert_eq!(&packets[1][10..], &[3, 3, 3]);
}

// ── Lifecycle and pacing ────────────────────────────────────────────────

#[tokio::test]
async fn short_frames_pad_with_black() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "pad", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 3, "color_order": "grb"
    }));
    let (backend, id) = connect(&config).await;
    backend
        .write_colors(&id, &[[10, 20, 30]])
        .await
        .expect("write");
    let packet = recv(&receiver).await;
    assert_eq!(&packet[10..], &[20, 10, 30, 0, 0, 0, 0, 0, 0]);
}

struct CountingObserver(Arc<AtomicUsize>);

impl DeviceDeliveryObserver for CountingObserver {
    fn transport_started(&self, _id: DeviceDeliveryId) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn frame_sink_delivers_and_reports_transport_start() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "sink", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 4
    }));
    let (backend, id) = connect(&config).await;
    let sink = backend
        .frame_sink(&id)
        .expect("connected device has a sink");

    let outcome = sink
        .write_colors_shared_outcome(Arc::new(gradient(4)))
        .await
        .expect("sink write");
    assert_eq!(outcome, DeviceWriteOutcome::Sent);
    recv(&receiver).await;

    let started = Arc::new(AtomicUsize::new(0));
    let ack = sink
        .deliver_colors_shared_observed(
            DeviceDeliveryId {
                queue_generation: 1,
                sequence: 1,
            },
            Arc::new(gradient(4)),
            Arc::new(CountingObserver(Arc::clone(&started))),
        )
        .await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
    assert_eq!(started.load(Ordering::Relaxed), 1);
    recv(&receiver).await;
}

#[tokio::test]
async fn frame_sink_rejects_writes_after_disconnect() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "closed", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 1
    }));
    let (backend, id) = connect(&config).await;
    // The output queue keeps its sink across disconnect, so hold one too.
    let sink = backend
        .frame_sink(&id)
        .expect("connected device has a sink");
    assert!(!sink.is_closed());
    sink.write_colors_shared_outcome(Arc::new(vec![[9, 9, 9]]))
        .await
        .expect("write before disconnect");
    recv(&receiver).await;

    backend.disconnect(&id).await.expect("disconnect");
    recv_n(&receiver, 1 + 3).await; // black frame, then three terminated
    assert!(sink.is_closed());

    assert!(matches!(
        sink.write_colors_shared_outcome(Arc::new(vec![[1, 1, 1]]))
            .await,
        Err(DeviceError::Disconnected { .. })
    ));
    let started = Arc::new(AtomicUsize::new(0));
    let ack = sink
        .deliver_colors_shared_observed(
            DeviceDeliveryId {
                queue_generation: 1,
                sequence: 7,
            },
            Arc::new(vec![[2, 2, 2]]),
            Arc::new(CountingObserver(Arc::clone(&started))),
        )
        .await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(!ack.transport_started);
    assert_eq!(started.load(Ordering::Relaxed), 0);
    assert_quiet(&receiver).await;
}

#[tokio::test]
async fn reconnect_retires_the_previous_session_without_a_blackout() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "again", "host": "127.0.0.1", "port": addr.port(), "protocol": "e131",
        "pixel_count": 1
    }));
    let (backend, id) = connect(&config).await;
    let old_sink = backend.frame_sink(&id).expect("sink");
    old_sink
        .write_colors_shared_outcome(Arc::new(vec![[5, 5, 5]]))
        .await
        .expect("write before reconnect");
    recv(&receiver).await;

    backend.connect(&id).await.expect("reconnect");
    assert!(old_sink.is_closed());
    assert!(matches!(
        old_sink
            .write_colors_shared_outcome(Arc::new(vec![[6, 6, 6]]))
            .await,
        Err(DeviceError::Disconnected { .. })
    ));
    assert_quiet(&receiver).await;

    backend
        .write_colors(&id, &[[7, 7, 7]])
        .await
        .expect("new session writes");
    let packet = recv(&receiver).await;
    assert_eq!(&packet[126..129], &[7, 7, 7]);
    assert_quiet(&receiver).await;
}

#[tokio::test]
async fn reconnect_to_a_moved_receiver_shuts_down_the_old_one() {
    let (old_receiver, old_addr) = loopback_receiver().await;
    let (new_receiver, new_addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "moved", "host": "127.0.0.1", "port": old_addr.port(), "protocol": "e131",
        "pixel_count": 1
    }));
    let rig = Rig::new();
    let retained = rig.connected(&config).await;
    let id = retained.info.id;
    let backend = &rig.backend;
    backend
        .write_colors(&id, &[[9, 9, 9]])
        .await
        .expect("write");
    recv(&old_receiver).await;

    // The entry moves; the retained record still names the old port.
    rig.config.set(&config_for(json!({
        "id": "moved", "host": "127.0.0.1", "port": new_addr.port(), "protocol": "e131",
        "pixel_count": 1
    })));
    backend.adopt_device(&retained).expect("still configured");
    backend.connect(&id).await.expect("reconnect");

    let shutdown = recv_n(&old_receiver, 1 + 3).await;
    assert!(
        shutdown[0][126..].iter().all(|&slot| slot == 0),
        "black frame"
    );
    assert!(shutdown[1..].iter().all(|packet| packet[112] == 0x40));
    assert_quiet(&new_receiver).await;
}

#[tokio::test]
async fn e131_cadence_requires_three_identical_sends() {
    let e131 = config_for(json!({
        "id": "sacn", "host": "127.0.0.1", "protocol": "e131", "pixel_count": 1
    }));
    let (backend, id) = connect(&e131).await;
    assert_eq!(
        backend
            .output_cadence(&id)
            .map(OutputCadence::min_identical_sends),
        Some(3)
    );
    for protocol in ["artnet", "ddp"] {
        let config = config_for(json!({
            "id": "other", "host": "127.0.0.1", "protocol": protocol, "pixel_count": 1
        }));
        let (backend, id) = connect(&config).await;
        assert_eq!(
            backend
                .output_cadence(&id)
                .map(OutputCadence::min_identical_sends),
            Some(1),
            "{protocol} keeps plain suppression"
        );
    }
}

#[tokio::test]
async fn cadence_follows_render_loop_with_keepalive() {
    let config = config_for(json!({
        "id": "free", "host": "127.0.0.1", "protocol": "ddp", "pixel_count": 1
    }));
    let (backend, id) = connect(&config).await;
    let cadence = backend.output_cadence(&id).expect("cadence");
    assert_eq!(cadence.min_interval(), None, "no driver-side frame cap");
    assert_eq!(cadence.max_frame_silence(), Some(KEEPALIVE_INTERVAL));
    assert_eq!(KEEPALIVE_INTERVAL, Duration::from_millis(800));
    assert_eq!(backend.target_fps(&id), None);

    let capped = config_for(json!({
        "id": "capped", "host": "127.0.0.1", "protocol": "e131", "pixel_count": 1,
        "max_fps": 40
    }));
    let (backend, id) = connect(&capped).await;
    let cadence = backend.output_cadence(&id).expect("cadence");
    let interval = cadence.min_interval().expect("capped device paces output");
    assert!(interval.abs_diff(Duration::from_millis(25)) < Duration::from_micros(1));
    assert_eq!(cadence.target_fps(), 40);
    assert_eq!(cadence.max_frame_silence(), Some(KEEPALIVE_INTERVAL));
    assert_eq!(backend.target_fps(&id), Some(40));
}

// ── Authorization by the live config ────────────────────────────────────

fn assert_not_configured(error: &DeviceError) {
    assert!(matches!(error, DeviceError::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn unconfigured_devices_cannot_connect() {
    let rig = Rig::new();
    assert_not_configured(
        &rig.backend
            .connect(&DeviceId::new())
            .await
            .expect_err("nothing configures this id"),
    );
}

#[tokio::test]
async fn a_removed_device_cannot_be_reached_from_retained_metadata() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 1
    }));
    let rig = Rig::new();
    let retained = rig.connected(&config).await;
    let id = retained.info.id;
    rig.backend
        .write_colors(&id, &[[7, 7, 7]])
        .await
        .expect("write");
    recv(&receiver).await;

    // The user deletes the entry; the rescan disconnects the device, but the
    // registry keeps its last discovery metadata.
    rig.config.clear();
    rig.backend.disconnect(&id).await.expect("departure");
    recv(&receiver).await;

    // Identify re-adopts the retained record and connects. Both steps must
    // refuse, and not one packet may reach the old receiver.
    assert_not_configured(
        &rig.backend
            .adopt_device(&retained)
            .expect_err("adoption must check the live config"),
    );
    assert_not_configured(
        &rig.backend
            .connect(&id)
            .await
            .expect_err("connect must check the live config"),
    );
    assert!(rig.backend.frame_sink(&id).is_none());
    assert_quiet(&receiver).await;
}

#[tokio::test]
async fn an_adopted_device_removed_before_connect_is_refused() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 1
    }));
    let rig = Rig::new();
    rig.config.set(&config);
    let device = first_device(&config);
    rig.backend.adopt_device(&device).expect("adopt");

    rig.config.clear();
    assert_not_configured(
        &rig.backend
            .connect(&device.info.id)
            .await
            .expect_err("entry is gone"),
    );
    assert_quiet(&receiver).await;
}

#[tokio::test]
async fn a_disabled_driver_reaches_nothing() {
    let config = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "protocol": "ddp", "pixel_count": 1
    }));
    let rig = Rig::new();
    rig.config.set(&config);
    rig.config.disable();
    let device = first_device(&config);
    assert_not_configured(
        &rig.backend
            .adopt_device(&device)
            .expect_err("disabled driver"),
    );
    assert_not_configured(
        &rig.backend
            .connect(&device.info.id)
            .await
            .expect_err("disabled driver"),
    );
}

#[tokio::test]
async fn metadata_neither_authorizes_nor_redirects_a_send() {
    let (configured, configured_addr) = loopback_receiver().await;
    let (elsewhere, elsewhere_addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "port": configured_addr.port(), "protocol": "ddp",
        "pixel_count": 1
    }));
    let rig = Rig::new();
    rig.config.set(&config);

    let mut device = first_device(&config);
    device.metadata.insert(
        METADATA_DEVICE_CONFIG.to_owned(),
        json!({
            "id": "roof", "host": "127.0.0.1", "port": elsewhere_addr.port(), "protocol": "ddp",
            "pixel_count": 1
        })
        .to_string(),
    );
    rig.backend.adopt_device(&device).expect("configured");
    rig.backend.connect(&device.info.id).await.expect("connect");
    rig.backend
        .write_colors(&device.info.id, &[[1, 2, 3]])
        .await
        .expect("write");
    recv(&configured).await;
    assert_quiet(&elsewhere).await;

    device.metadata.clear();
    rig.backend
        .adopt_device(&device)
        .expect("config alone authorizes adoption");

    let unconfigured = first_device(&config_for(json!({
        "id": "other", "host": "127.0.0.1", "port": elsewhere_addr.port(), "protocol": "ddp",
        "pixel_count": 1
    })));
    assert_not_configured(
        &rig.backend
            .adopt_device(&unconfigured)
            .expect_err("metadata alone authorizes nothing"),
    );
    assert_quiet(&elsewhere).await;
}

/// A config that moves on its own: each read takes the next entry, and the
/// last entry stays.
struct SteppingConfig(StdMutex<VecDeque<DriverConfigEntry>>);

impl DriverConfigSource for SteppingConfig {
    fn current(&self) -> DriverConfigEntry {
        let mut entries = self.0.lock().expect("config lock");
        if entries.len() > 1 {
            entries.pop_front().expect("more than one entry")
        } else {
            entries.front().cloned().expect("one entry")
        }
    }
}

#[tokio::test]
async fn a_config_edit_during_connect_publishes_no_session() {
    let (receiver, addr) = loopback_receiver().await;
    let config = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 1
    }));
    let moved = config_for(json!({
        "id": "roof", "host": "127.0.0.1", "port": addr.port(), "protocol": "ddp",
        "pixel_count": 2
    }));
    let backend = PixelBackend::new(
        Arc::new(SteppingConfig(StdMutex::new(VecDeque::from([
            entry_for(&config),
            entry_for(&moved),
        ])))),
        installation(),
    );
    let id = first_device(&config).info.id;

    let error = backend.connect(&id).await.expect_err("the config moved");
    assert!(
        matches!(error, DeviceError::ConnectionFailed { .. }),
        "{error:?}"
    );
    assert!(backend.frame_sink(&id).is_none());
    assert_quiet(&receiver).await;

    backend
        .connect(&id)
        .await
        .expect("the retry sees one config");
    assert!(backend.frame_sink(&id).is_some());
}

/// Records one frame, plus shutdown frames, of every packet type the
/// backend emits, as text2pcap hex dumps for Wireshark dissector checks.
///
/// Receivers bind ephemeral 127.0.0.1 ports. Run with
/// `PIXEL_WIRE_CAPTURE_DIR=<dir> cargo test -p hypercolor-driver-pixel
/// --test backend_tests -- --ignored wire_capture_fixture`, then wrap each
/// dump with `text2pcap -u <port>,<port>` on the protocol's standard port
/// and read it with `tshark -r`.
#[tokio::test]
#[ignore = "manual fixture for tshark dissector checks"]
async fn wire_capture_fixture() {
    use std::fmt::Write as _;

    let dir = std::path::PathBuf::from(
        std::env::var_os("PIXEL_WIRE_CAPTURE_DIR").expect("set PIXEL_WIRE_CAPTURE_DIR"),
    );
    std::fs::create_dir_all(&dir).expect("create capture dir");
    let devices = [
        (
            "e131",
            json!({ "id": "cap-e131", "protocol": "e131", "pixel_count": 200,
                    "sync_universe": 9, "source_name": "Hypercolor capture" }),
            // 2 universes + sync, then black frame + sync, then 3 x 2 terminated.
            3 + 3 + 6,
        ),
        (
            "artnet",
            json!({ "id": "cap-artnet", "protocol": "artnet", "pixel_count": 171,
                    "start_universe": 3 }),
            2 + 2,
        ),
        (
            "ddp",
            json!({ "id": "cap-ddp", "protocol": "ddp", "pixel_count": 600 }),
            2 + 2,
        ),
    ];
    for (name, mut device, expected_packets) in devices {
        let (receiver, addr) = loopback_receiver().await;
        device["host"] = json!("127.0.0.1");
        device["port"] = json!(addr.port());
        let pixels =
            usize::try_from(device["pixel_count"].as_u64().expect("pixel count")).expect("fits");
        let (backend, id) = connect(&config_for(device)).await;
        backend
            .write_colors(&id, &gradient(pixels))
            .await
            .expect("write");
        backend.disconnect(&id).await.expect("disconnect");

        let mut dump = String::new();
        for packet in recv_n(&receiver, expected_packets).await {
            for (line, chunk) in packet.chunks(16).enumerate() {
                write!(dump, "{:06x}", line * 16).expect("write to String");
                for byte in chunk {
                    write!(dump, " {byte:02x}").expect("write to String");
                }
                dump.push('\n');
            }
            dump.push('\n');
        }
        assert_quiet(&receiver).await;
        std::fs::write(dir.join(format!("{name}.txt")), dump).expect("write dump");
    }
}
