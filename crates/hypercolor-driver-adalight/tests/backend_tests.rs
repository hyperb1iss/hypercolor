//! Backend integration tests over pseudo-terminal pairs (Spec 91 §9).
//!
//! Every port these tests open is a pty slave the test created itself, or a
//! path inside a fresh temporary directory that does not exist. No real
//! serial device is ever touched.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_driver_adalight::protocol::{AwaWhite, FrameEncoder, FrameLayout, Framing};
use hypercolor_driver_adalight::{
    AdalightBackend, AdalightConfig, BLACKOUT_FRAMES, ColorOrder, KEEPALIVE_INTERVAL,
    configured_devices, output_cadence_for,
};
use hypercolor_driver_api::{DeviceBackend, DeviceWriteOutcome, DiscoveredDevice};
use hypercolor_types::device::{DeviceError, DeviceId};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPort, SerialStream};

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A pty pair whose slave path is free for the backend to open.
struct Pty {
    master: SerialStream,
    slave_path: String,
}

fn pty() -> Pty {
    let (master, slave) = SerialStream::pair().expect("pseudo-terminal pair");
    let slave_path = slave.name().expect("pty slave has a path");
    drop(slave);
    Pty { master, slave_path }
}

fn discovered(device: serde_json::Value) -> DiscoveredDevice {
    let config: AdalightConfig =
        serde_json::from_value(json!({ "devices": [device] })).expect("config");
    configured_devices(&config)
        .expect("valid config")
        .pop()
        .expect("one device")
}

async fn connected(backend: &AdalightBackend, device: serde_json::Value) -> DeviceId {
    let discovered = discovered(device);
    backend.adopt_device(&discovered).expect("adopt");
    backend
        .connect(&discovered.info.id)
        .await
        .expect("connect to pty slave");
    discovered.info.id
}

async fn read_frame(master: &mut SerialStream, len: usize) -> Vec<u8> {
    let mut frame = vec![0; len];
    tokio::time::timeout(READ_TIMEOUT, master.read_exact(&mut frame))
        .await
        .expect("frame should arrive")
        .expect("read from pty master");
    frame
}

fn expected_frame(framing: Framing, order: ColorOrder, colors: &[[u8; 3]], leds: u32) -> Vec<u8> {
    let layout = FrameLayout::new(framing, leds).expect("layout");
    FrameEncoder::new(layout, order).encode(colors).to_vec()
}

#[tokio::test]
async fn ada_frames_reach_the_port_byte_for_byte() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "desk", "port": pty.slave_path, "led_count": 3 }),
    )
    .await;

    // Newline, carriage return, XON, and XOFF bytes must pass through
    // untouched, which proves the port is in raw mode.
    let colors = [[0x0A, 0x0D, 0x11], [0x13, 0x41, 0xFF], [0x00, 0x7F, 0x80]];
    backend.write_colors(&id, &colors).await.expect("write");

    let frame = read_frame(&mut pty.master, 6 + 9).await;
    assert_eq!(
        frame,
        [
            0x41, 0x64, 0x61, 0x00, 0x02, 0x57, // "Ada", count field 2
            0x0A, 0x0D, 0x11, 0x13, 0x41, 0xFF, 0x00, 0x7F, 0x80,
        ]
    );
}

#[tokio::test]
async fn awa_frames_carry_order_white_and_trailer() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({
            "id": "tv", "port": pty.slave_path, "protocol": "awa", "led_count": 2,
            "color_order": "grb", "white": "host"
        }),
    )
    .await;

    let colors = [[200, 100, 50], [1, 2, 3]];
    let outcome = backend
        .write_colors_shared_outcome(&id, Arc::new(colors.to_vec()))
        .await
        .expect("write");
    assert_eq!(outcome, DeviceWriteOutcome::Sent);

    let expected = expected_frame(Framing::Awa(AwaWhite::Host), ColorOrder::Grb, &colors, 2);
    assert_eq!(&expected[..6], b"AWa\x00\x01\x54");
    assert_eq!(read_frame(&mut pty.master, expected.len()).await, expected);
}

#[tokio::test]
async fn short_frames_are_padded_to_the_configured_count() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "desk", "port": pty.slave_path, "protocol": "awa", "led_count": 4 }),
    )
    .await;

    backend
        .write_colors(&id, &[[9, 8, 7]])
        .await
        .expect("write");
    let expected = expected_frame(
        Framing::Awa(AwaWhite::Off),
        ColorOrder::Rgb,
        &[[9, 8, 7], [0, 0, 0], [0, 0, 0], [0, 0, 0]],
        4,
    );
    assert_eq!(read_frame(&mut pty.master, expected.len()).await, expected);
}

#[tokio::test]
async fn disconnect_blacks_out_the_strip_and_closes_the_port() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "desk", "port": pty.slave_path, "led_count": 2 }),
    )
    .await;

    backend
        .write_colors(&id, &[[255, 255, 255], [255, 255, 255]])
        .await
        .expect("write");
    read_frame(&mut pty.master, 12).await;

    backend.disconnect(&id).await.expect("disconnect");
    assert!(!backend.is_connected(&id));
    let black = [0x41, 0x64, 0x61, 0x00, 0x01, 0x54, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        read_frame(&mut pty.master, 12 * BLACKOUT_FRAMES).await,
        black.repeat(BLACKOUT_FRAMES),
        "disconnect sends the blackout frames"
    );

    let error = backend
        .write_colors(&id, &[[1, 1, 1]])
        .await
        .expect_err("writes after disconnect fail");
    assert!(matches!(error, DeviceError::Disconnected { .. }));
    assert!(matches!(
        backend.disconnect(&id).await,
        Err(DeviceError::Disconnected { .. })
    ));
}

#[tokio::test]
async fn disconnect_without_frames_sends_nothing() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "desk", "port": pty.slave_path, "led_count": 2 }),
    )
    .await;
    backend.disconnect(&id).await.expect("disconnect");

    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_millis(200), pty.master.read(&mut byte)).await;
    assert!(
        !matches!(read, Ok(Ok(1))),
        "a device that never lit should not receive a blackout frame"
    );
}

#[tokio::test]
async fn connect_waits_out_the_startup_delay_and_tolerates_a_greeting() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let discovered = discovered(json!({
        "id": "uno", "port": pty.slave_path, "led_count": 1, "startup_delay_ms": 300
    }));
    backend.adopt_device(&discovered).expect("adopt");
    let id = discovered.info.id;

    let started = Instant::now();
    let connect = backend.connect(&id);
    let greet = async {
        // The board "boots" and prints its hello while the host waits.
        tokio::time::sleep(Duration::from_millis(100)).await;
        pty.master.write_all(b"Ada\n").await.expect("greeting");
    };
    let (connected, ()) = tokio::join!(connect, greet);
    connected.expect("connect");
    assert!(started.elapsed() >= Duration::from_millis(300));

    backend
        .write_colors(&id, &[[1, 2, 3]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 9).await,
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55, 1, 2, 3]
    );
}

#[tokio::test]
async fn connect_twice_keeps_one_session() {
    let mut pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "desk", "port": pty.slave_path, "led_count": 1 }),
    )
    .await;
    backend
        .connect(&id)
        .await
        .expect("second connect is a no-op");
    backend
        .write_colors(&id, &[[4, 5, 6]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 9).await,
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55, 4, 5, 6]
    );
}

#[tokio::test]
async fn a_stalled_port_fails_the_write_instead_of_queueing() {
    let pty = pty();
    let backend = AdalightBackend::new();
    let id = connected(
        &backend,
        json!({ "id": "big", "port": pty.slave_path, "protocol": "awa", "led_count": 4096 }),
    )
    .await;

    // Nobody reads the master, so the pty fills and the write must give up
    // within its bound rather than block the output worker forever.
    let colors = vec![[0x55, 0x55, 0x55]; 4096];
    let started = Instant::now();
    let mut failure = None;
    for _ in 0..64 {
        if let Err(error) = backend.write_colors(&id, &colors).await {
            failure = Some(error);
            break;
        }
    }
    let error = failure.expect("a full pty should stall the write");
    assert!(matches!(error, DeviceError::WriteError { .. }), "{error:?}");
    assert!(error.to_string().contains("stalled"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(pty);
}

#[tokio::test]
async fn missing_ports_fail_to_connect_with_a_retryable_error() {
    let directory = tempfile::tempdir().expect("temp dir");
    let missing = directory.path().join("no-such-port");
    let backend = AdalightBackend::new();
    let discovered = discovered(json!({
        "id": "ghost", "port": missing.to_string_lossy(), "led_count": 1
    }));
    backend.adopt_device(&discovered).expect("adopt");

    let error = backend
        .connect(&discovered.info.id)
        .await
        .expect_err("missing port");
    assert!(
        matches!(error, DeviceError::ConnectionFailed { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("no-such-port"));
    assert!(!backend.is_connected(&discovered.info.id));
}

#[tokio::test]
async fn unadopted_devices_cannot_connect() {
    let backend = AdalightBackend::new();
    let id = DeviceId::new();
    assert!(matches!(
        backend.connect(&id).await,
        Err(DeviceError::NotAdopted { .. })
    ));
}

#[test]
fn cadence_paces_to_the_line_and_refreshes_idle_strips() {
    let backend = AdalightBackend::new();
    let discovered = discovered(
        json!({ "id": "desk", "port": "/nonexistent/adalight-test-port", "led_count": 300 }),
    );
    backend.adopt_device(&discovered).expect("adopt");
    let id = discovered.info.id;

    let cadence = backend.output_cadence(&id).expect("cadence");
    // 906 bytes at 115200 baud occupy the line for 78.646 ms.
    assert_eq!(cadence.min_interval(), Some(Duration::from_micros(78_646)));
    assert_eq!(cadence.max_frame_silence(), Some(KEEPALIVE_INTERVAL));
    assert_eq!(cadence.target_fps(), 12);
    assert_eq!(backend.target_fps(&id), Some(12));

    let plan = backend.adopted_plan(&id).expect("plan");
    assert_eq!(output_cadence_for(&plan), cadence);
}

#[test]
fn startup_delay_extends_the_connect_timeout() {
    let backend = AdalightBackend::new();
    let discovered = discovered(json!({
        "id": "uno", "port": "/nonexistent/adalight-test-port", "led_count": 10, "startup_delay_ms": 2000
    }));
    backend.adopt_device(&discovered).expect("adopt");
    let policy = backend.lifecycle_policy(&discovered.info);
    assert_eq!(policy.connect_timeout(), Duration::from_secs(7));
}

#[test]
fn adoption_requires_the_config_metadata() {
    let backend = AdalightBackend::new();
    let mut discovered = discovered(
        json!({ "id": "desk", "port": "/nonexistent/adalight-test-port", "led_count": 1 }),
    );
    discovered.metadata.clear();
    assert!(matches!(
        backend.adopt_device(&discovered),
        Err(DeviceError::NotAdopted { .. })
    ));

    let mut tampered = self::discovered(
        json!({ "id": "desk", "port": "/nonexistent/adalight-test-port", "led_count": 1 }),
    );
    tampered.metadata.insert(
        hypercolor_driver_adalight::METADATA_DEVICE_CONFIG.to_owned(),
        json!({ "id": "desk", "port": "/nonexistent/adalight-test-port", "led_count": 0 })
            .to_string(),
    );
    assert!(matches!(
        backend.adopt_device(&tampered),
        Err(DeviceError::ProtocolError { .. })
    ));
    assert!(backend.adopted_plan(&tampered.info.id).is_none());
}
