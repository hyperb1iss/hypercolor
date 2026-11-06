//! Backend integration tests over pseudo-terminal pairs (Spec 91 §9, §10).
//!
//! Every port these tests open is a pty slave the test created itself, or a
//! path inside a fresh temporary directory that does not exist. No real
//! serial device is ever touched.

#![cfg(unix)]

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use hypercolor_driver_adalight::protocol::{AwaWhite, FrameEncoder, FrameLayout, Framing};
use hypercolor_driver_adalight::{
    AdalightBackend, AdalightConfig, ColorOrder, DISCONNECT_BUDGET, KEEPALIVE_INTERVAL,
    MAX_BLACKOUT_FRAMES, configured_devices, output_cadence_for,
};
use hypercolor_driver_api::{
    DeviceBackend, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, DriverConfigSource,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{DeviceError, DeviceId};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPort, SerialStream};

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const FAKE_PORT: &str = "/nonexistent/adalight-test-port";

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

/// A live driver config the test edits while the backend runs, standing in
/// for the daemon's config manager.
#[derive(Default)]
struct TestConfig {
    devices: RwLock<Vec<Value>>,
    disabled: RwLock<bool>,
}

impl TestConfig {
    /// Insert `device`, replacing any entry with the same id.
    fn upsert(&self, device: Value) {
        let mut devices = self.devices.write().expect("config lock");
        devices.retain(|existing| existing["id"] != device["id"]);
        devices.push(device);
    }

    fn remove(&self, id: &str) {
        self.devices
            .write()
            .expect("config lock")
            .retain(|existing| existing["id"] != id);
    }

    fn set_disabled(&self, disabled: bool) {
        *self.disabled.write().expect("config lock") = disabled;
    }
}

impl DriverConfigSource for TestConfig {
    fn driver_config(&self, driver_id: &str) -> Option<DriverConfigEntry> {
        if driver_id != "adalight" {
            return None;
        }
        let devices = Value::Array(self.devices.read().expect("config lock").clone());
        let settings = [("devices".to_owned(), devices)].into_iter().collect();
        Some(if *self.disabled.read().expect("config lock") {
            DriverConfigEntry::disabled(settings)
        } else {
            DriverConfigEntry::enabled(settings)
        })
    }
}

/// A backend authorized by a [`TestConfig`] the test controls.
struct Rig {
    config: Arc<TestConfig>,
    backend: AdalightBackend,
}

impl Rig {
    fn new() -> Self {
        let config = Arc::new(TestConfig::default());
        let backend = AdalightBackend::new(Arc::clone(&config) as Arc<dyn DriverConfigSource>);
        Self { config, backend }
    }

    /// Configure `device` and return the discovery result a scan would
    /// produce for it.
    fn discovered(&self, device: Value) -> DiscoveredDevice {
        self.config.upsert(device.clone());
        discovered(device)
    }

    async fn connected(&self, device: Value) -> DeviceId {
        let discovered = self.discovered(device);
        self.backend.adopt_device(&discovered).expect("adopt");
        self.backend
            .connect(&discovered.info.id)
            .await
            .expect("connect to pty slave");
        discovered.info.id
    }
}

fn discovered(device: Value) -> DiscoveredDevice {
    let config: AdalightConfig =
        serde_json::from_value(json!({ "devices": [device] })).expect("config");
    configured_devices(&config)
        .expect("valid config")
        .pop()
        .expect("one device")
}

async fn read_frame(master: &mut SerialStream, len: usize) -> Vec<u8> {
    let mut frame = vec![0; len];
    tokio::time::timeout(READ_TIMEOUT, master.read_exact(&mut frame))
        .await
        .expect("frame should arrive")
        .expect("read from pty master");
    frame
}

async fn assert_silent(master: &mut SerialStream, message: &str) {
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_millis(200), master.read(&mut byte)).await;
    assert!(!matches!(read, Ok(Ok(1))), "{message}");
}

fn expected_frame(framing: Framing, order: ColorOrder, colors: &[[u8; 3]], leds: u32) -> Vec<u8> {
    let layout = FrameLayout::new(framing, leds).expect("layout");
    FrameEncoder::new(layout, order).encode(colors).to_vec()
}

fn assert_not_configured(error: &DeviceError) {
    assert!(matches!(error, DeviceError::NotFound { .. }), "{error:?}");
}

// ── Frames on the wire ──────────────────────────────────────────────────

#[tokio::test]
async fn ada_frames_reach_the_port_byte_for_byte() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 3 }))
        .await;

    // Newline, carriage return, XON, and XOFF bytes must pass through
    // untouched, which proves the port is in raw mode.
    let colors = [[0x0A, 0x0D, 0x11], [0x13, 0x41, 0xFF], [0x00, 0x7F, 0x80]];
    rig.backend.write_colors(&id, &colors).await.expect("write");

    assert_eq!(
        read_frame(&mut pty.master, 6 + 9).await,
        [
            0x41, 0x64, 0x61, 0x00, 0x02, 0x57, // "Ada", count field 2
            0x0A, 0x0D, 0x11, 0x13, 0x41, 0xFF, 0x00, 0x7F, 0x80,
        ]
    );
}

#[tokio::test]
async fn awa_frames_carry_order_white_and_trailer() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({
            "id": "tv", "port": pty.slave_path, "protocol": "awa", "led_count": 2,
            "color_order": "grb", "white": "host"
        }))
        .await;

    let colors = [[200, 100, 50], [1, 2, 3]];
    let outcome = rig
        .backend
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
    let rig = Rig::new();
    let id = rig
        .connected(json!({
            "id": "desk", "port": pty.slave_path, "protocol": "awa", "led_count": 4
        }))
        .await;

    rig.backend
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

// ── Connect and disconnect ──────────────────────────────────────────────

#[tokio::test]
async fn disconnect_blacks_out_the_strip_and_closes_the_port() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 2 }))
        .await;

    rig.backend
        .write_colors(&id, &[[255, 255, 255], [255, 255, 255]])
        .await
        .expect("write");
    read_frame(&mut pty.master, 12).await;

    rig.backend.disconnect(&id).await.expect("disconnect");
    assert!(!rig.backend.is_connected(&id));
    let black = [0x41, 0x64, 0x61, 0x00, 0x01, 0x54, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        read_frame(&mut pty.master, 12 * MAX_BLACKOUT_FRAMES).await,
        black.repeat(MAX_BLACKOUT_FRAMES),
        "a short frame gets every blackout copy"
    );

    let error = rig
        .backend
        .write_colors(&id, &[[1, 1, 1]])
        .await
        .expect_err("writes after disconnect fail");
    assert!(matches!(error, DeviceError::Disconnected { .. }));
    assert!(matches!(
        rig.backend.disconnect(&id).await,
        Err(DeviceError::Disconnected { .. })
    ));
}

#[tokio::test]
async fn disconnect_without_frames_sends_nothing() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 2 }))
        .await;
    rig.backend.disconnect(&id).await.expect("disconnect");
    assert_silent(
        &mut pty.master,
        "a device that never lit should not receive a blackout frame",
    )
    .await;
}

#[tokio::test]
async fn connect_waits_out_the_startup_delay_and_tolerates_a_greeting() {
    let mut pty = pty();
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "uno", "port": pty.slave_path, "led_count": 1, "startup_delay_ms": 300
    }));
    rig.backend.adopt_device(&discovered).expect("adopt");
    let id = discovered.info.id;

    let started = Instant::now();
    let connect = rig.backend.connect(&id);
    let greet = async {
        // The board "boots" and prints its hello while the host waits.
        tokio::time::sleep(Duration::from_millis(100)).await;
        pty.master.write_all(b"Ada\n").await.expect("greeting");
    };
    let (connected, ()) = tokio::join!(connect, greet);
    connected.expect("connect");
    assert!(started.elapsed() >= Duration::from_millis(300));

    rig.backend
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
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 1 }))
        .await;
    rig.backend
        .connect(&id)
        .await
        .expect("second connect is a no-op");
    rig.backend
        .write_colors(&id, &[[4, 5, 6]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 9).await,
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55, 4, 5, 6]
    );
}

#[tokio::test]
async fn reconnecting_right_after_disconnect_reopens_the_port() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 1 }))
        .await;
    rig.backend
        .write_colors(&id, &[[1, 1, 1]])
        .await
        .expect("write");
    read_frame(&mut pty.master, 9).await;

    // The exclusive open would fail if the old descriptor were still held.
    rig.backend.disconnect(&id).await.expect("disconnect");
    read_frame(&mut pty.master, 9 * MAX_BLACKOUT_FRAMES).await;
    rig.backend.connect(&id).await.expect("reconnect");
    rig.backend
        .write_colors(&id, &[[2, 2, 2]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 9).await,
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55, 2, 2, 2]
    );
}

#[tokio::test]
async fn a_changed_plan_replaces_the_live_session() {
    let mut pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({ "id": "desk", "port": pty.slave_path, "led_count": 1 }))
        .await;

    // A config edit re-adopts the same id with a longer strip.
    let edited = rig.discovered(json!({ "id": "desk", "port": pty.slave_path, "led_count": 2 }));
    assert_eq!(edited.info.id, id);
    rig.backend.adopt_device(&edited).expect("re-adopt");
    rig.backend
        .connect(&id)
        .await
        .expect("connect with the new plan");

    rig.backend
        .write_colors(&id, &[[3, 3, 3], [4, 4, 4]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 12).await,
        [0x41, 0x64, 0x61, 0x00, 0x01, 0x54, 3, 3, 3, 4, 4, 4]
    );
}

#[tokio::test]
async fn a_stalled_port_fails_the_write_and_disconnect_fits_its_budget() {
    let pty = pty();
    let rig = Rig::new();
    let id = rig
        .connected(json!({
            "id": "big", "port": pty.slave_path, "protocol": "awa", "led_count": 4096
        }))
        .await;

    // Nobody reads the master, so the pty fills and the write must give up
    // within its bound rather than block the output worker forever.
    let colors = vec![[0x55, 0x55, 0x55]; 4096];
    let started = Instant::now();
    let mut failure = None;
    for _ in 0..64 {
        if let Err(error) = rig.backend.write_colors(&id, &colors).await {
            failure = Some(error);
            break;
        }
    }
    let error = failure.expect("a full pty should stall the write");
    assert!(matches!(error, DeviceError::WriteError { .. }), "{error:?}");
    assert!(error.to_string().contains("stalled"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));

    // Disconnecting a wedged port must still finish inside the daemon's
    // two-second disconnect timeout.
    let started = Instant::now();
    rig.backend.disconnect(&id).await.expect("disconnect");
    assert!(
        started.elapsed() < DISCONNECT_BUDGET + Duration::from_millis(150),
        "disconnect took {:?}",
        started.elapsed()
    );
    drop(pty);
}

#[tokio::test]
async fn missing_ports_fail_to_connect_with_a_retryable_error() {
    let directory = tempfile::tempdir().expect("temp dir");
    let missing = directory.path().join("no-such-port");
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "ghost", "port": missing.to_string_lossy(), "led_count": 1
    }));
    rig.backend.adopt_device(&discovered).expect("adopt");

    let error = rig
        .backend
        .connect(&discovered.info.id)
        .await
        .expect_err("missing port");
    assert!(
        matches!(error, DeviceError::ConnectionFailed { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("no-such-port"));
    assert!(!rig.backend.is_connected(&discovered.info.id));
}

#[tokio::test]
async fn unadopted_devices_cannot_connect() {
    let rig = Rig::new();
    assert!(matches!(
        rig.backend.connect(&DeviceId::new()).await,
        Err(DeviceError::NotAdopted { .. })
    ));
}

// ── Authorization by the live config ────────────────────────────────────

#[tokio::test]
async fn a_removed_entry_cannot_be_reopened_by_identify() {
    let mut pty = pty();
    let rig = Rig::new();

    // Configure the board, discover it, and drive it.
    let retained = rig.discovered(json!({
        "id": "desk", "port": pty.slave_path, "led_count": 1
    }));
    let id = retained.info.id;
    rig.backend.adopt_device(&retained).expect("adopt");
    rig.backend.connect(&id).await.expect("connect");
    rig.backend
        .write_colors(&id, &[[7, 7, 7]])
        .await
        .expect("write");
    read_frame(&mut pty.master, 9).await;

    // The user deletes the entry; the rescan disconnects the device, but
    // the registry keeps its last discovery metadata.
    rig.config.remove("desk");
    rig.backend.disconnect(&id).await.expect("departure");
    read_frame(&mut pty.master, 9 * MAX_BLACKOUT_FRAMES).await;

    // Identify on the retained device re-adopts from that metadata and
    // connects. Both steps must refuse, and nothing may reach the port.
    let error = rig
        .backend
        .adopt_device(&retained)
        .expect_err("adoption must check the live config");
    assert_not_configured(&error);
    assert!(rig.backend.adopted_plan(&id).is_none());
    let error = rig
        .backend
        .connect(&id)
        .await
        .expect_err("connect must check the live config");
    assert!(matches!(error, DeviceError::NotAdopted { .. }), "{error:?}");
    assert!(!rig.backend.is_connected(&id));
    assert_silent(&mut pty.master, "a removed device must never be reopened").await;
}

#[tokio::test]
async fn an_adopted_device_removed_before_connect_is_refused() {
    let mut pty = pty();
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "desk", "port": pty.slave_path, "led_count": 1
    }));
    let id = discovered.info.id;
    rig.backend.adopt_device(&discovered).expect("adopt");

    rig.config.remove("desk");
    let error = rig.backend.connect(&id).await.expect_err("entry is gone");
    assert_not_configured(&error);
    assert!(
        rig.backend.adopted_plan(&id).is_none(),
        "refusal forgets it"
    );
    assert_silent(&mut pty.master, "the port must stay closed").await;
}

#[tokio::test]
async fn an_entry_removed_during_the_startup_delay_is_refused() {
    let mut pty = pty();
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "uno", "port": pty.slave_path, "led_count": 1, "startup_delay_ms": 300
    }));
    let id = discovered.info.id;
    rig.backend.adopt_device(&discovered).expect("adopt");

    let connect = rig.backend.connect(&id);
    let remove = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        rig.config.remove("uno");
    };
    let (connected, ()) = tokio::join!(connect, remove);
    assert_not_configured(&connected.expect_err("entry removed mid-boot"));
    assert!(!rig.backend.is_connected(&id));
    assert_silent(&mut pty.master, "no frame goes to a removed device").await;
}

#[tokio::test]
async fn a_moved_port_is_not_opened_from_stale_metadata() {
    let mut old_port = pty();
    let new_port = pty();
    let rig = Rig::new();
    let stale = rig.discovered(json!({
        "id": "desk", "port": old_port.slave_path, "led_count": 1
    }));
    // The entry now names another port, but the registry still holds the
    // metadata from before the edit.
    rig.config
        .upsert(json!({ "id": "desk", "port": new_port.slave_path, "led_count": 1 }));

    let error = rig
        .backend
        .adopt_device(&stale)
        .expect_err("stale port must not be adopted");
    assert_not_configured(&error);
    assert!(error.to_string().contains("names port"), "{error}");
    assert_silent(&mut old_port.master, "the old port must stay closed").await;
}

#[tokio::test]
async fn a_disabled_driver_opens_nothing() {
    let mut pty = pty();
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "desk", "port": pty.slave_path, "led_count": 1
    }));
    rig.config.set_disabled(true);
    assert_not_configured(
        &rig.backend
            .adopt_device(&discovered)
            .expect_err("disabled driver"),
    );
    assert_silent(&mut pty.master, "a disabled driver must not open ports").await;
}

// ── Lifecycle policy and cadence ────────────────────────────────────────

#[tokio::test]
async fn first_connect_timeout_covers_the_startup_delay_before_adoption() {
    let mut pty = pty();
    let rig = Rig::new();
    let discovered = rig.discovered(json!({
        "id": "uno", "port": pty.slave_path, "led_count": 1, "startup_delay_ms": 6000
    }));
    let id = discovered.info.id;

    // The daemon picks the connect timeout before it adopts the device.
    let policy = rig.backend.lifecycle_policy(&discovered.info);
    assert_eq!(
        policy.connect_timeout(),
        DeviceLifecyclePolicy::DEFAULT_CONNECT_TIMEOUT + Duration::from_secs(6)
    );

    // Then it adopts and connects under that timeout. A default 5 s bound
    // would cancel the 6 s startup delay mid-sleep.
    tokio::time::timeout(policy.connect_timeout(), async {
        rig.backend.adopt_device(&discovered).expect("adopt");
        rig.backend.connect(&id).await
    })
    .await
    .expect("connect must finish inside the policy timeout")
    .expect("connect");

    rig.backend
        .write_colors(&id, &[[5, 5, 5]])
        .await
        .expect("write");
    assert_eq!(
        read_frame(&mut pty.master, 9).await,
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55, 5, 5, 5]
    );
}

#[test]
fn unconfigured_devices_get_the_default_connect_timeout() {
    let rig = Rig::new();
    let discovered = discovered(json!({
        "id": "uno", "port": FAKE_PORT, "led_count": 1, "startup_delay_ms": 2000
    }));
    assert_eq!(
        rig.backend
            .lifecycle_policy(&discovered.info)
            .connect_timeout(),
        DeviceLifecyclePolicy::DEFAULT_CONNECT_TIMEOUT
    );
}

#[test]
fn cadence_paces_to_the_line_and_refreshes_idle_strips() {
    let rig = Rig::new();
    let discovered = rig.discovered(json!({ "id": "desk", "port": FAKE_PORT, "led_count": 300 }));
    rig.backend.adopt_device(&discovered).expect("adopt");
    let id = discovered.info.id;

    let cadence = rig.backend.output_cadence(&id).expect("cadence");
    // 906 bytes at 115200 baud occupy the line for 78.646 ms.
    assert_eq!(cadence.min_interval(), Some(Duration::from_micros(78_646)));
    assert_eq!(cadence.max_frame_silence(), Some(KEEPALIVE_INTERVAL));
    assert_eq!(cadence.target_fps(), 12);
    assert_eq!(rig.backend.target_fps(&id), Some(12));

    let plan = rig.backend.adopted_plan(&id).expect("plan");
    assert_eq!(output_cadence_for(&plan), cadence);
}

#[test]
fn adoption_requires_the_config_metadata() {
    let rig = Rig::new();
    let mut discovered = rig.discovered(json!({ "id": "desk", "port": FAKE_PORT, "led_count": 1 }));
    discovered.metadata.clear();
    assert!(matches!(
        rig.backend.adopt_device(&discovered),
        Err(DeviceError::NotAdopted { .. })
    ));

    let mut tampered = rig.discovered(json!({ "id": "desk", "port": FAKE_PORT, "led_count": 1 }));
    tampered.metadata.insert(
        hypercolor_driver_adalight::METADATA_DEVICE_CONFIG.to_owned(),
        json!({ "id": "desk", "port": FAKE_PORT, "led_count": 0 }).to_string(),
    );
    assert!(matches!(
        rig.backend.adopt_device(&tampered),
        Err(DeviceError::ProtocolError { .. })
    ));
    assert!(rig.backend.adopted_plan(&tampered.info.id).is_none());
}

#[test]
fn a_snapshot_backend_authorizes_only_its_own_entries() {
    let config: AdalightConfig = serde_json::from_value(json!({
        "devices": [{ "id": "desk", "port": FAKE_PORT, "led_count": 1 }]
    }))
    .expect("config");
    let backend = AdalightBackend::from_config(&config).expect("snapshot backend");
    backend
        .adopt_device(&discovered(
            json!({ "id": "desk", "port": FAKE_PORT, "led_count": 1 }),
        ))
        .expect("configured entry adopts");
    assert_not_configured(
        &backend
            .adopt_device(&discovered(
                json!({ "id": "other", "port": FAKE_PORT, "led_count": 1 }),
            ))
            .expect_err("unknown entry"),
    );
}
