//! Loopback integration tests for the pixel controller backend.
//!
//! Every test binds its receiver on 127.0.0.1 and configures the device to
//! stream there. Nothing leaves the host: no LAN, multicast, or broadcast.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryId, DeviceDeliveryObserver, DeviceDeliveryStatus,
    DeviceWriteOutcome, DiscoveredDevice,
};
use hypercolor_driver_pixel::{
    KEEPALIVE_INTERVAL, METADATA_DEVICE_CONFIG, PixelBackend, PixelConfig, configured_devices,
    derived_cid,
};
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

fn config_for(device: serde_json::Value) -> PixelConfig {
    serde_json::from_value(json!({ "devices": [device] })).expect("config fixture")
}

/// Discover, adopt, and connect the single device in `config`.
async fn connect(config: &PixelConfig) -> (PixelBackend, DeviceId) {
    let discovered = configured_devices(config).expect("config should resolve");
    let device = discovered.into_iter().next().expect("one device");
    let backend = PixelBackend::new();
    backend.adopt_device(&device).expect("adopt");
    backend.connect(&device.info.id).await.expect("connect");
    (backend, device.info.id)
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
            assert_eq!(&packet[22..38], derived_cid("falcon").as_bytes());
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

#[tokio::test]
async fn connect_requires_adoption() {
    let backend = PixelBackend::new();
    let id = DeviceId::new();
    assert!(matches!(
        backend.connect(&id).await,
        Err(DeviceError::NotAdopted { .. })
    ));
}

#[tokio::test]
async fn adopt_rejects_devices_without_pixel_metadata() {
    let config = config_for(json!({
        "id": "meta", "host": "127.0.0.1", "protocol": "ddp", "pixel_count": 1
    }));
    let mut device: DiscoveredDevice = configured_devices(&config)
        .expect("resolve")
        .into_iter()
        .next()
        .expect("one device");
    device.metadata.remove(METADATA_DEVICE_CONFIG);
    let backend = PixelBackend::new();
    assert!(matches!(
        backend.adopt_device(&device),
        Err(DeviceError::NotAdopted { .. })
    ));

    device
        .metadata
        .insert(METADATA_DEVICE_CONFIG.to_owned(), "{\"id\":1}".to_owned());
    assert!(matches!(
        backend.adopt_device(&device),
        Err(DeviceError::ProtocolError { .. })
    ));
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
        let (backend, id) = connect(&config_for(device)).await;
        let pixels = backend.adopted_plan(&id).expect("adopted").total_pixels() as usize;
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
