//! Tests for the device backend system.
//!
//! All tests use mock backend implementations — no real hardware is required.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::Mutex;

use hypercolor_core::device::{
    BackendInfo, DeviceBackend, DevicePlugin, DeviceRegistry, DeviceStateMachine, DiscoveredDevice,
    DiscoveryConnectBehavior, DiscoveryOrchestrator, ReconnectPolicy, TransportScanner,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceHandle, DeviceId, DeviceIdentifier, DeviceInfo,
    DeviceOrigin, DeviceState, DeviceTopologyHint, DeviceUserSettings, ZoneInfo,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

/// Build a synthetic [`DeviceInfo`] for testing.
fn mock_device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.to_owned(),
        vendor: "MockVendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("test", "test", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Zone 1".to_owned(),
            led_count: 30,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("1.0.0".to_owned()),
        capabilities: DeviceCapabilities::default(),
    }
}

fn asus_dram_device_info(address: u16) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: format!("ASUS Aura DRAM (SMBus 0x{address:02X})"),
        vendor: "ASUS".to_owned(),
        family: DeviceFamily::new_static("asus", "ASUS"),
        model: Some("asus_aura_smbus_dram".to_owned()),
        connection_type: ConnectionType::SmBus,
        origin: DeviceOrigin::native("asus", "smbus", ConnectionType::SmBus)
            .with_protocol_id("asus/aura-smbus"),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("AUDA0-E6K5-0101".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 8,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn asus_dram_metadata(address: u16) -> HashMap<String, String> {
    HashMap::from([
        ("bus_path".to_owned(), "/dev/i2c-9".to_owned()),
        ("smbus_address".to_owned(), format!("0x{address:02X}")),
        ("controller_kind".to_owned(), "dram".to_owned()),
        ("firmware_name".to_owned(), "AUDA0-E6K5-0101".to_owned()),
    ])
}

// ── Mock Backend ─────────────────────────────────────────────────────────

/// A mock device backend that tracks calls for test assertions.
struct MockBackend {
    /// Devices this backend will "discover".
    discoverable: Vec<DeviceInfo>,

    /// Set of currently connected device IDs.
    connected: Arc<Mutex<Vec<DeviceId>>>,

    /// Total number of `write_colors` calls.
    write_count: Arc<AtomicU32>,

    /// Last colors written, per device.
    last_colors: Arc<Mutex<HashMap<DeviceId, Vec<[u8; 3]>>>>,

    /// If true, `connect` will fail with an error.
    fail_connect: Arc<AtomicBool>,

    /// If true, `write_colors` will fail with an error.
    fail_write: Arc<AtomicBool>,
}

impl MockBackend {
    fn new(discoverable: Vec<DeviceInfo>) -> Self {
        Self {
            discoverable,
            connected: Arc::new(Mutex::new(Vec::new())),
            write_count: Arc::new(AtomicU32::new(0)),
            last_colors: Arc::new(Mutex::new(HashMap::new())),
            fail_connect: Arc::new(AtomicBool::new(false)),
            fail_write: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for MockBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Mock Backend".to_owned(),
            description: "A test-only backend with no real hardware".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(self.discoverable.clone())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if self.fail_connect.load(Ordering::Relaxed) {
            bail!("mock connect failure for device {id}");
        }

        let mut connected = self.connected.lock().await;
        if connected.contains(id) {
            bail!("device {id} is already connected");
        }
        connected.push(*id);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let mut connected = self.connected.lock().await;
        if let Some(pos) = connected.iter().position(|d| d == id) {
            connected.remove(pos);
            Ok(())
        } else {
            bail!("device {id} is not connected");
        }
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if self.fail_write.load(Ordering::Relaxed) {
            bail!("mock write failure for device {id}");
        }

        let connected = self.connected.lock().await;
        if !connected.contains(id) {
            bail!("cannot write to disconnected device {id}");
        }
        drop(connected);

        self.write_count.fetch_add(1, Ordering::Relaxed);

        let mut last = self.last_colors.lock().await;
        last.insert(*id, colors.to_vec());
        Ok(())
    }
}

// ── Mock Plugin ──────────────────────────────────────────────────────────

struct MockPlugin;

impl DevicePlugin for MockPlugin {
    fn name(&self) -> &'static str {
        "Mock Plugin"
    }

    fn build(&self) -> Box<dyn DeviceBackend> {
        Box::new(MockBackend::new(vec![mock_device_info("Plugin Device")]))
    }
}

// ── Mock Scanner ─────────────────────────────────────────────────────────

struct MockScanner {
    name: String,
    devices: Vec<DiscoveredDevice>,
    should_fail: bool,
}

impl MockScanner {
    fn new(name: &str, devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            name: name.to_owned(),
            devices,
            should_fail: false,
        }
    }

    fn failing(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            devices: Vec::new(),
            should_fail: true,
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for MockScanner {
    fn name(&self) -> &str {
        &self.name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        if self.should_fail {
            bail!("mock scanner '{name}' failed", name = self.name);
        }
        Ok(self.devices.clone())
    }
}

/// Scanner that sleeps before returning devices, used for parallelism tests.
struct DelayedScanner {
    name: String,
    delay: Duration,
    devices: Vec<DiscoveredDevice>,
}

impl DelayedScanner {
    fn new(name: &str, delay: Duration, devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            name: name.to_owned(),
            delay,
            devices,
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for DelayedScanner {
    fn name(&self) -> &str {
        &self.name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        tokio::time::sleep(self.delay).await;
        Ok(self.devices.clone())
    }
}

/// Build a [`DiscoveredDevice`] for scanner tests.
fn mock_discovered(name: &str, fingerprint: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: DeviceFingerprint(fingerprint.to_owned()),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: mock_device_info(name),
        metadata: HashMap::new(),
    }
}

// ── DeviceBackend trait tests ────────────────────────────────────────────

#[tokio::test]
async fn backend_info_returns_metadata() {
    let backend = MockBackend::new(vec![]);
    let info = backend.info();
    assert_eq!(info.id, "mock");
    assert_eq!(info.name, "Mock Backend");
    assert!(!info.description.is_empty());
}

#[tokio::test]
async fn backend_discover_returns_devices() {
    let d1 = mock_device_info("LED Strip A");
    let d2 = mock_device_info("LED Strip B");
    let mut backend = MockBackend::new(vec![d1.clone(), d2.clone()]);

    let discovered = backend.discover().await.expect("discover should succeed");
    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].name, "LED Strip A");
    assert_eq!(discovered[1].name, "LED Strip B");
}

#[tokio::test]
async fn backend_connect_and_disconnect() {
    let device = mock_device_info("Test Device");
    let id = device.id;
    let mut backend = MockBackend::new(vec![device]);

    // Connect
    backend.connect(&id).await.expect("connect should succeed");

    // Verify connected
    let connected = backend.connected.lock().await;
    assert!(connected.contains(&id));
    drop(connected);

    // Disconnect
    backend
        .disconnect(&id)
        .await
        .expect("disconnect should succeed");

    // Verify disconnected
    let connected = backend.connected.lock().await;
    assert!(!connected.contains(&id));
}

#[tokio::test]
async fn backend_double_connect_fails() {
    let device = mock_device_info("Test Device");
    let id = device.id;
    let mut backend = MockBackend::new(vec![device]);

    backend.connect(&id).await.expect("first connect succeeds");
    let result = backend.connect(&id).await;
    assert!(result.is_err(), "double connect should fail");
}

#[tokio::test]
async fn backend_disconnect_unknown_fails() {
    let mut backend = MockBackend::new(vec![]);
    let unknown_id = DeviceId::new();

    let result = backend.disconnect(&unknown_id).await;
    assert!(result.is_err(), "disconnecting unknown device should fail");
}

#[tokio::test]
async fn backend_write_colors_succeeds() {
    let device = mock_device_info("RGB Strip");
    let id = device.id;
    let mut backend = MockBackend::new(vec![device]);

    backend.connect(&id).await.expect("connect succeeds");

    let colors: Vec<[u8; 3]> = vec![[255, 0, 0]; 30];
    backend
        .write_colors(&id, &colors)
        .await
        .expect("write should succeed");

    assert_eq!(backend.write_count.load(Ordering::Relaxed), 1);

    let last = backend.last_colors.lock().await;
    let written = last.get(&id).expect("colors should be stored");
    assert_eq!(written.len(), 30);
    assert_eq!(written[0], [255, 0, 0]);
}

#[tokio::test]
async fn backend_write_to_disconnected_fails() {
    let mut backend = MockBackend::new(vec![]);
    let id = DeviceId::new();

    let colors: Vec<[u8; 3]> = vec![[0, 255, 0]; 10];
    let result = backend.write_colors(&id, &colors).await;
    assert!(
        result.is_err(),
        "writing to disconnected device should fail"
    );
}

#[tokio::test]
async fn backend_connect_failure_propagates() {
    let device = mock_device_info("Flaky Device");
    let id = device.id;
    let mut backend = MockBackend::new(vec![device]);

    backend.fail_connect.store(true, Ordering::Relaxed);
    let result = backend.connect(&id).await;
    assert!(
        result.is_err(),
        "connect should fail when fail_connect is set"
    );
}

#[tokio::test]
async fn backend_write_failure_propagates() {
    let device = mock_device_info("Unreliable Strip");
    let id = device.id;
    let mut backend = MockBackend::new(vec![device]);

    backend.connect(&id).await.expect("connect succeeds");
    backend.fail_write.store(true, Ordering::Relaxed);

    let colors: Vec<[u8; 3]> = vec![[0, 0, 255]; 30];
    let result = backend.write_colors(&id, &colors).await;
    assert!(result.is_err(), "write should fail when fail_write is set");
}

// ── DevicePlugin trait tests ─────────────────────────────────────────────

#[tokio::test]
async fn plugin_lifecycle() {
    let plugin = MockPlugin;

    assert_eq!(plugin.name(), "Mock Plugin");
    plugin.ready().expect("ready should succeed");

    let mut backend = plugin.build();
    let info = backend.info();
    assert_eq!(info.id, "mock");

    let discovered = backend.discover().await.expect("discover succeeds");
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].name, "Plugin Device");

    plugin.teardown();
}

// ── DeviceRegistry tests ─────────────────────────────────────────────────

#[tokio::test]
async fn registry_add_and_get() {
    let registry = DeviceRegistry::new();
    let device = mock_device_info("Shelf LEDs");
    let id = device.id;

    registry.add(device.clone()).await;

    let tracked = registry.get(&id).await.expect("device should exist");
    assert_eq!(tracked.info.name, "Shelf LEDs");
    assert_eq!(tracked.state, DeviceState::Known);
}

#[tokio::test]
async fn registry_add_returns_existing_id() {
    let registry = DeviceRegistry::new();
    let device = mock_device_info("Desk Strip");
    let id = device.id;

    let first_id = registry.add(device.clone()).await;
    assert_eq!(first_id, id);

    // Adding the same device again should return the same ID
    let mut updated = device;
    updated.name = "Desk Strip (updated)".to_owned();
    // Use same DeviceId so fingerprint matches
    let second_id = registry.add(updated).await;
    assert_eq!(second_id, id);

    // Verify metadata was updated
    let tracked = registry.get(&id).await.expect("device should exist");
    assert_eq!(tracked.info.name, "Desk Strip (updated)");
}

#[tokio::test]
async fn registry_add_with_fingerprint_reuses_existing_device() {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned());

    let first = mock_device_info("Desk Strip");
    let first_id = registry
        .add_with_fingerprint(first, fingerprint.clone())
        .await;

    let second = mock_device_info("Desk Strip Updated");
    let second_id = registry.add_with_fingerprint(second, fingerprint).await;

    assert_eq!(
        first_id, second_id,
        "same fingerprint should map to same logical device id"
    );
    let tracked = registry
        .get(&first_id)
        .await
        .expect("device should still exist");
    assert_eq!(tracked.info.name, "Desk Strip Updated");
}

#[tokio::test]
async fn registry_reuses_renderable_asus_dram_when_smbus_address_changes() {
    let registry = DeviceRegistry::new();
    let first = asus_dram_device_info(0x71);
    let first_fingerprint = DeviceFingerprint("smbus:/dev/i2c-9:71".to_owned());
    let first_id = registry
        .add_with_fingerprint_and_metadata(first, first_fingerprint, asus_dram_metadata(0x71))
        .await;
    assert!(registry.set_state(&first_id, DeviceState::Connected).await);

    let second = asus_dram_device_info(0x73);
    let second_fingerprint = DeviceFingerprint("smbus:/dev/i2c-9:73".to_owned());
    let second_id = registry
        .add_with_fingerprint_and_metadata(
            second,
            second_fingerprint.clone(),
            asus_dram_metadata(0x73),
        )
        .await;

    assert_eq!(second_id, first_id);
    assert_eq!(registry.list().await.len(), 1);
    assert_eq!(
        registry.fingerprint_for_id(&first_id).await,
        Some(second_fingerprint)
    );
    let tracked = registry.get(&first_id).await.expect("device should exist");
    assert_eq!(tracked.info.name, "ASUS Aura DRAM (SMBus 0x73)");
    assert_eq!(tracked.state, DeviceState::Connected);
}

#[tokio::test]
async fn registry_keeps_asus_dram_address_change_separate_when_ambiguous() {
    let registry = DeviceRegistry::new();
    let mut existing_ids = Vec::new();
    for address in [0x71, 0x72] {
        let info = asus_dram_device_info(address);
        let id = registry
            .add_with_fingerprint_and_metadata(
                info,
                DeviceFingerprint(format!("smbus:/dev/i2c-9:{address:02x}")),
                asus_dram_metadata(address),
            )
            .await;
        existing_ids.push(id);
    }
    for id in existing_ids {
        assert!(registry.set_state(&id, DeviceState::Connected).await);
    }

    let discovered = asus_dram_device_info(0x73);
    let discovered_id = discovered.id;
    let returned_id = registry
        .add_with_fingerprint_and_metadata(
            discovered,
            DeviceFingerprint("smbus:/dev/i2c-9:73".to_owned()),
            asus_dram_metadata(0x73),
        )
        .await;

    assert_eq!(returned_id, discovered_id);
    assert_eq!(registry.list().await.len(), 3);
}

#[tokio::test]
async fn registry_add_with_fingerprint_preserves_renderable_runtime_shape_when_rediscovery_is_blank()
 {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("usb:1b1c:0c3f:corsair-hub".to_owned());

    let connected = DeviceInfo {
        id: DeviceId::new(),
        name: "Corsair Hub".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: Some("icue_link_system_hub".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "iCUE LINK H-Series AIO".to_owned(),
                led_count: 20,
                topology: DeviceTopologyHint::Ring { count: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "iCUE LINK Cooler Pump LCD".to_owned(),
                led_count: 24,
                topology: DeviceTopologyHint::Ring { count: 24 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: Some("2.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 44,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let device_id = registry
        .add_with_fingerprint(connected, fingerprint.clone())
        .await;
    assert!(
        registry.set_state(&device_id, DeviceState::Connected).await,
        "device state should update"
    );

    let rediscovered = DeviceInfo {
        id: DeviceId::new(),
        name: "Corsair Hub (rediscovered)".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: Some("icue_link_system_hub".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", "usb", ConnectionType::Usb),
        zones: Vec::new(),
        firmware_version: Some("2.2.0".to_owned()),
        capabilities: DeviceCapabilities::default(),
    };

    let rediscovered_id = registry
        .add_with_fingerprint(rediscovered, fingerprint)
        .await;

    assert_eq!(rediscovered_id, device_id);

    let tracked = registry
        .get(&device_id)
        .await
        .expect("device should still exist");
    assert_eq!(tracked.info.name, "Corsair Hub (rediscovered)");
    assert_eq!(tracked.info.zones.len(), 2);
    assert_eq!(tracked.info.total_led_count(), 44);
    assert_eq!(tracked.info.capabilities.led_count, 44);
    assert_eq!(tracked.info.capabilities.max_fps, 30);
    assert_eq!(tracked.state, DeviceState::Connected);
}

#[tokio::test]
async fn registry_generation_advances_on_mutation() {
    let registry = DeviceRegistry::new();
    assert_eq!(registry.generation(), 0);

    let device = mock_device_info("Generation Test");
    let id = device.id;
    registry.add(device).await;
    let after_add = registry.generation();
    assert!(after_add > 0, "add should advance generation");

    assert!(registry.set_state(&id, DeviceState::Active).await);
    let after_state = registry.generation();
    assert!(
        after_state > after_add,
        "state changes should advance generation"
    );

    registry
        .update_user_settings(&id, None, None, Some(0.5))
        .await
        .expect("device should exist");
    let after_settings = registry.generation();
    assert!(
        after_settings > after_state,
        "user setting updates should advance generation"
    );

    registry
        .remove(&id)
        .await
        .expect("device should be removed");
    assert!(
        registry.generation() > after_settings,
        "removal should advance generation"
    );
}

#[tokio::test]
async fn registry_fingerprint_lookup_round_trips_device_id() {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("net:12:34:56:78:9a:bc".to_owned());
    let info = mock_device_info("Roundtrip Device");

    let id = registry
        .add_with_fingerprint(info, fingerprint.clone())
        .await;

    assert_eq!(
        registry
            .fingerprint_for_id(&id)
            .await
            .expect("fingerprint should exist"),
        fingerprint
    );
}

#[tokio::test]
async fn registry_preserves_scanner_metadata() {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned());
    let info = mock_device_info("Metadata Device");
    let mut metadata = HashMap::new();
    metadata.insert("ip".to_owned(), "192.168.1.42".to_owned());
    metadata.insert("hostname".to_owned(), "wled-desk".to_owned());

    let id = registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata.clone())
        .await;

    assert_eq!(
        registry
            .metadata_for_id(&id)
            .await
            .expect("metadata should exist"),
        metadata
    );
}

#[tokio::test]
async fn registry_add_discovered_persists_explicit_origin() {
    let registry = DeviceRegistry::new();
    let mut discovered = mock_discovered("Origin Device", "net:origin-device");
    discovered.info.origin = DeviceOrigin::native("wled", "wled-alt", ConnectionType::Network);

    let id = registry.add_discovered(discovered).await;
    let tracked = registry
        .get(&id)
        .await
        .expect("discovered device should be tracked");

    assert_eq!(tracked.info.origin.driver_id, "wled");
    assert_eq!(tracked.info.origin.backend_id, "wled-alt");
}

#[tokio::test]
async fn registry_remove() {
    let registry = DeviceRegistry::new();
    let device = mock_device_info("Temporary Device");
    let id = device.id;

    registry.add(device).await;
    assert!(registry.contains(&id).await);

    let removed = registry.remove(&id).await;
    assert!(removed.is_some());
    assert!(!registry.contains(&id).await);
}

#[tokio::test]
async fn registry_remove_unknown_returns_none() {
    let registry = DeviceRegistry::new();
    let unknown_id = DeviceId::new();

    let result = registry.remove(&unknown_id).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn registry_list() {
    let registry = DeviceRegistry::new();

    registry.add(mock_device_info("Device A")).await;
    registry.add(mock_device_info("Device B")).await;
    registry.add(mock_device_info("Device C")).await;

    let all = registry.list().await;
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn registry_set_state() {
    let registry = DeviceRegistry::new();
    let device = mock_device_info("Stateful Device");
    let id = device.id;

    registry.add(device).await;

    let updated = registry.set_state(&id, DeviceState::Connected).await;
    assert!(updated);

    let tracked = registry.get(&id).await.expect("device should exist");
    assert_eq!(tracked.state, DeviceState::Connected);
}

#[tokio::test]
async fn registry_set_state_unknown_returns_false() {
    let registry = DeviceRegistry::new();
    let unknown_id = DeviceId::new();

    let updated = registry.set_state(&unknown_id, DeviceState::Active).await;
    assert!(!updated);
}

#[tokio::test]
async fn registry_update_info_preserves_id_and_state() {
    let registry = DeviceRegistry::new();
    let device = mock_device_info("Original Device");
    let id = device.id;

    registry.add(device).await;
    registry.set_state(&id, DeviceState::Connected).await;

    let refreshed = DeviceInfo {
        id: DeviceId::new(),
        name: "Refreshed Device".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: Some("iCUE LINK".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "Pump Ring".to_owned(),
                led_count: 24,
                topology: DeviceTopologyHint::Ring { count: 24 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Radiator Fans".to_owned(),
                led_count: 102,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: Some("2.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 126,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let updated = registry
        .update_info(&id, refreshed)
        .await
        .expect("tracked device should update");
    assert_eq!(updated.info.id, id);
    assert_eq!(updated.info.name, "Refreshed Device");
    assert_eq!(updated.state, DeviceState::Connected);

    let tracked = registry.get(&id).await.expect("device should exist");
    assert_eq!(tracked.info.id, id);
    assert_eq!(tracked.info.vendor, "Corsair");
    assert_eq!(tracked.info.zones.len(), 2);
    assert_eq!(tracked.info.capabilities.led_count, 126);
    assert_eq!(tracked.state, DeviceState::Connected);
}

#[tokio::test]
async fn registry_list_by_state() {
    let registry = DeviceRegistry::new();

    let d1 = mock_device_info("Active Strip");
    let d2 = mock_device_info("Known Strip");
    let d3 = mock_device_info("Also Active");
    let id1 = d1.id;
    let id3 = d3.id;

    registry.add(d1).await;
    registry.add(d2).await;
    registry.add(d3).await;

    registry.set_state(&id1, DeviceState::Active).await;
    registry.set_state(&id3, DeviceState::Active).await;

    let active = registry.list_by_state(&DeviceState::Active).await;
    assert_eq!(active.len(), 2);

    let known = registry.list_by_state(&DeviceState::Known).await;
    assert_eq!(known.len(), 1);
}

#[tokio::test]
async fn registry_update_user_settings_tracks_name_enabled_and_brightness_without_changing_state() {
    let registry = DeviceRegistry::new();
    let info = mock_device_info("Desk Strip");
    let id = info.id;
    registry.add(info).await;

    let updated = registry
        .update_user_settings(&id, Some("Desk Glow".to_owned()), Some(false), Some(0.35))
        .await
        .expect("device should update");

    assert_eq!(updated.info.name, "Desk Glow");
    assert_eq!(updated.state, DeviceState::Known);
    assert_eq!(updated.user_settings.name.as_deref(), Some("Desk Glow"));
    assert!(!updated.user_settings.enabled);
    assert!((updated.user_settings.brightness - 0.35).abs() < f32::EPSILON);
}

#[tokio::test]
async fn registry_replace_user_settings_reapplies_name_override_on_metadata_refresh() {
    let registry = DeviceRegistry::new();
    let original = mock_device_info("Original Name");
    let id = original.id;
    registry.add(original).await;

    registry
        .replace_user_settings(
            &id,
            DeviceUserSettings {
                name: Some("Override Name".to_owned()),
                enabled: true,
                brightness: 0.6,
            },
        )
        .await
        .expect("settings should update");

    let mut refreshed = mock_device_info("Rediscovered Name");
    refreshed.id = id;
    let updated = registry
        .update_info(&id, refreshed)
        .await
        .expect("metadata should refresh");

    assert_eq!(updated.info.name, "Override Name");
    assert_eq!(updated.user_settings.name.as_deref(), Some("Override Name"));
    assert!((updated.user_settings.brightness - 0.6).abs() < f32::EPSILON);
}

#[tokio::test]
async fn registry_len_and_is_empty() {
    let registry = DeviceRegistry::new();
    assert!(registry.is_empty().await);
    assert_eq!(registry.len().await, 0);

    registry.add(mock_device_info("First")).await;
    assert!(!registry.is_empty().await);
    assert_eq!(registry.len().await, 1);
}

#[tokio::test]
async fn registry_is_thread_safe() {
    let registry = DeviceRegistry::new();
    let r1 = registry.clone();
    let r2 = registry.clone();

    // Spawn concurrent add operations
    let h1 = tokio::spawn(async move {
        for i in 0..10 {
            r1.add(mock_device_info(&format!("Thread1-Device-{i}")))
                .await;
        }
    });

    let h2 = tokio::spawn(async move {
        for i in 0..10 {
            r2.add(mock_device_info(&format!("Thread2-Device-{i}")))
                .await;
        }
    });

    h1.await.expect("task 1 should complete");
    h2.await.expect("task 2 should complete");

    assert_eq!(registry.len().await, 20);
}

// ── DeviceStateMachine tests ─────────────────────────────────────────────

fn sample_identifier() -> DeviceIdentifier {
    DeviceIdentifier::Network {
        mac_address: "AA:BB:CC:DD:EE:FF".to_owned(),
        last_ip: None,
        mdns_hostname: Some("wled-test".to_owned()),
    }
}

#[test]
fn state_machine_transitions_connected_to_active() {
    let identifier = sample_identifier();
    let mut sm = DeviceStateMachine::new(identifier.clone());

    let handle = DeviceHandle::new(identifier, "mock");
    sm.on_connected(handle)
        .expect("connect transition should work");
    assert_eq!(*sm.state(), DeviceState::Connected);
    assert!(sm.handle().is_some());

    sm.on_frame_success()
        .expect("first frame transition should work");
    assert_eq!(*sm.state(), DeviceState::Active);

    let debug = sm.debug_snapshot();
    assert_eq!(debug.state, "Active");
    assert_eq!(debug.transition_count, 2);
}

#[test]
fn state_machine_rejects_invalid_transition() {
    let mut sm = DeviceStateMachine::new(sample_identifier());
    let err = sm
        .on_frame_success()
        .expect_err("Known -> Active should be invalid");
    assert!(
        matches!(err, DeviceError::InvalidTransition { .. }),
        "expected invalid transition error"
    );
}

#[test]
fn state_machine_enters_reconnecting_on_comm_error() {
    let identifier = sample_identifier();
    let mut sm = DeviceStateMachine::new(identifier.clone());

    sm.on_connected(DeviceHandle::new(identifier, "mock"))
        .expect("connect should work");
    sm.on_frame_success()
        .expect("first frame should transition to active");
    assert_eq!(*sm.state(), DeviceState::Active);

    sm.on_comm_error().expect("comm error should transition");

    assert_eq!(*sm.state(), DeviceState::Reconnecting);
    assert!(sm.handle().is_none());
    assert_eq!(
        sm.reconnect_status()
            .expect("reconnect status should exist")
            .attempt,
        0
    );
}

#[test]
fn state_machine_connect_failure_enters_reconnecting() {
    let policy = ReconnectPolicy {
        initial_delay: Duration::from_millis(25),
        max_delay: Duration::from_secs(2),
        backoff_factor: 2.0,
        max_attempts: Some(4),
        jitter: 0.0,
    };
    let mut sm = DeviceStateMachine::with_policy(sample_identifier(), policy);

    let delay = sm
        .on_connect_failed()
        .expect("failed connect should transition to reconnecting");
    assert_eq!(delay, Duration::from_millis(25));
    assert_eq!(*sm.state(), DeviceState::Reconnecting);
    assert_eq!(
        sm.reconnect_status()
            .expect("reconnect status should exist")
            .attempt,
        0
    );
}

#[test]
fn state_machine_connect_abandoned_returns_to_known() {
    let policy = ReconnectPolicy {
        initial_delay: Duration::from_millis(25),
        max_delay: Duration::from_secs(2),
        backoff_factor: 2.0,
        max_attempts: Some(4),
        jitter: 0.0,
    };
    let mut sm = DeviceStateMachine::with_policy(sample_identifier(), policy);

    sm.on_connect_failed()
        .expect("failed connect should transition to reconnecting");
    sm.on_connect_abandoned();

    assert_eq!(*sm.state(), DeviceState::Known);
    assert!(sm.reconnect_status().is_none());
}

#[test]
fn state_machine_reconnect_exhaustion_returns_known() {
    let identifier = sample_identifier();
    let policy = ReconnectPolicy {
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(100),
        backoff_factor: 2.0,
        max_attempts: Some(1),
        jitter: 0.0,
    };
    let mut sm = DeviceStateMachine::with_policy(identifier.clone(), policy);

    sm.on_connected(DeviceHandle::new(identifier, "mock"))
        .expect("connect should work");
    sm.on_comm_error().expect("comm error should transition");

    let next = sm.on_reconnect_failed();
    assert!(next.is_none(), "max attempts should exhaust immediately");
    assert_eq!(*sm.state(), DeviceState::Known);
    assert!(sm.reconnect_status().is_none());
}

#[test]
fn state_machine_backoff_progresses_with_jitter_and_cap() {
    let identifier = sample_identifier();
    let policy = ReconnectPolicy {
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        backoff_factor: 2.0,
        max_attempts: Some(8),
        jitter: 0.1,
    };
    let mut sm = DeviceStateMachine::with_policy(identifier.clone(), policy);

    sm.on_connected(DeviceHandle::new(identifier, "mock"))
        .expect("connect should work");
    sm.on_comm_error().expect("comm error should transition");

    let status = sm
        .reconnect_status()
        .expect("reconnect status should exist after comm error");
    assert_eq!(status.attempt, 0);
    assert_eq!(status.next_retry, Duration::from_millis(100));

    // Deterministic jitter alternates -, +, -, + with the current algorithm.
    let d1 = sm
        .on_reconnect_failed()
        .expect("attempt one should schedule retry");
    let d2 = sm
        .on_reconnect_failed()
        .expect("attempt two should schedule retry");
    let d3 = sm
        .on_reconnect_failed()
        .expect("attempt three should schedule retry");

    assert_eq!(d1, Duration::from_millis(180));
    assert_eq!(d2, Duration::from_millis(396));
    assert_eq!(d3, Duration::from_millis(450));
    assert_eq!(*sm.state(), DeviceState::Reconnecting);
}

#[test]
fn state_machine_user_disable_and_enable() {
    let mut sm = DeviceStateMachine::new(sample_identifier());

    sm.on_user_disable();
    assert_eq!(*sm.state(), DeviceState::Disabled);

    sm.on_user_enable();
    assert_eq!(*sm.state(), DeviceState::Known);
}

#[test]
fn state_machine_hot_unplug_clears_handle() {
    let identifier = sample_identifier();
    let mut sm = DeviceStateMachine::new(identifier.clone());
    sm.on_connected(DeviceHandle::new(identifier, "mock"))
        .expect("connect should work");
    assert!(sm.handle().is_some());

    sm.on_hot_unplug();
    assert_eq!(*sm.state(), DeviceState::Known);
    assert!(sm.handle().is_none());
}

#[test]
fn state_machine_hot_unplug_from_any_state_returns_known() {
    let id = sample_identifier();

    let mut known = DeviceStateMachine::new(id.clone());
    known.on_hot_unplug();
    assert_eq!(*known.state(), DeviceState::Known);

    let mut connected = DeviceStateMachine::new(id.clone());
    connected
        .on_connected(DeviceHandle::new(id.clone(), "mock"))
        .expect("connect should work");
    connected.on_hot_unplug();
    assert_eq!(*connected.state(), DeviceState::Known);
    assert!(connected.handle().is_none());

    let mut active = DeviceStateMachine::new(id.clone());
    active
        .on_connected(DeviceHandle::new(id.clone(), "mock"))
        .expect("connect should work");
    active
        .on_frame_success()
        .expect("first frame should transition to active");
    active.on_hot_unplug();
    assert_eq!(*active.state(), DeviceState::Known);
    assert!(active.handle().is_none());

    let mut reconnecting = DeviceStateMachine::new(id.clone());
    reconnecting
        .on_connected(DeviceHandle::new(id.clone(), "mock"))
        .expect("connect should work");
    reconnecting
        .on_comm_error()
        .expect("comm error should transition to reconnecting");
    reconnecting.on_hot_unplug();
    assert_eq!(*reconnecting.state(), DeviceState::Known);
    assert!(reconnecting.handle().is_none());

    let mut disabled = DeviceStateMachine::new(id);
    disabled.on_user_disable();
    disabled.on_hot_unplug();
    assert_eq!(*disabled.state(), DeviceState::Known);
}

#[test]
fn state_machine_invalid_transitions_return_device_error() {
    let id = sample_identifier();

    let mut known = DeviceStateMachine::new(id.clone());
    assert!(matches!(
        known.on_frame_success(),
        Err(DeviceError::InvalidTransition { .. })
    ));

    let mut active = DeviceStateMachine::new(id.clone());
    active
        .on_connected(DeviceHandle::new(id.clone(), "mock"))
        .expect("connect should work");
    active
        .on_frame_success()
        .expect("first frame should transition to active");
    assert!(matches!(
        active.on_connected(DeviceHandle::new(id.clone(), "mock")),
        Err(DeviceError::InvalidTransition { .. })
    ));

    let mut reconnecting = DeviceStateMachine::new(id.clone());
    reconnecting
        .on_connected(DeviceHandle::new(id.clone(), "mock"))
        .expect("connect should work");
    reconnecting
        .on_comm_error()
        .expect("comm error should transition");
    assert!(matches!(
        reconnecting.on_frame_success(),
        Err(DeviceError::InvalidTransition { .. })
    ));

    let mut disabled = DeviceStateMachine::new(id);
    disabled.on_user_disable();
    assert!(matches!(
        disabled.on_connected(DeviceHandle::new(sample_identifier(), "mock")),
        Err(DeviceError::InvalidTransition { .. })
    ));
}

// ── DiscoveryOrchestrator tests ──────────────────────────────────────────

#[tokio::test]
async fn orchestrator_full_scan_empty() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let report = orchestrator.full_scan().await;
    assert_eq!(report.new_devices.len(), 0);
    assert_eq!(report.reappeared_devices.len(), 0);
    assert_eq!(report.vanished_devices.len(), 0);
    assert_eq!(report.total_known, 0);
}

#[tokio::test]
async fn orchestrator_registers_scanners() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    assert_eq!(orchestrator.scanner_count(), 0);

    orchestrator.add_scanner(Box::new(MockScanner::new("USB", vec![])));
    orchestrator.add_scanner(Box::new(MockScanner::new("mDNS", vec![])));

    assert_eq!(orchestrator.scanner_count(), 2);
}

#[tokio::test]
async fn orchestrator_discovers_new_devices() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let d1 = mock_discovered("WLED Kitchen", "net:aa:bb:cc:dd:ee:01");
    let d2 = mock_discovered("WLED Bedroom", "net:aa:bb:cc:dd:ee:02");

    orchestrator.add_scanner(Box::new(MockScanner::new("mDNS", vec![d1, d2])));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.new_devices.len(), 2);
    assert_eq!(report.total_known, 2);
}

#[tokio::test]
async fn orchestrator_deduplicates_across_scanners() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    // Same fingerprint from two different scanners
    let d1 = mock_discovered("WLED Strip (mDNS)", "net:aa:bb:cc:dd:ee:ff");
    let d2 = mock_discovered("WLED Strip (UDP)", "net:aa:bb:cc:dd:ee:ff");

    orchestrator.add_scanner(Box::new(MockScanner::new("mDNS", vec![d1])));
    orchestrator.add_scanner(Box::new(MockScanner::new("UDP", vec![d2])));

    let report = orchestrator.full_scan().await;

    // Only one device should be registered despite two reports
    assert_eq!(report.total_known, 1);
}

#[tokio::test]
async fn orchestrator_handles_scanner_failure_gracefully() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let good_device = mock_discovered("Healthy Device", "net:11:22:33:44:55:66");

    orchestrator.add_scanner(Box::new(MockScanner::failing("Broken Scanner")));
    orchestrator.add_scanner(Box::new(MockScanner::new(
        "Good Scanner",
        vec![good_device],
    )));

    let report = orchestrator.full_scan().await;

    // The healthy scanner's device should still be registered
    assert_eq!(report.new_devices.len(), 1);
    assert_eq!(report.total_known, 1);
    assert_eq!(report.scanner_reports.len(), 2);
    assert!(
        report
            .scanner_reports
            .iter()
            .any(|r| r.error.is_some() && r.scanner == "Broken Scanner")
    );
}

#[tokio::test]
async fn orchestrator_scans_transports_in_parallel() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let delay = Duration::from_millis(220);
    orchestrator.add_scanner(Box::new(DelayedScanner::new(
        "Scanner A",
        delay,
        vec![mock_discovered("A", "net:parallel:a")],
    )));
    orchestrator.add_scanner(Box::new(DelayedScanner::new(
        "Scanner B",
        delay,
        vec![mock_discovered("B", "net:parallel:b")],
    )));

    let started = Instant::now();
    let report = orchestrator.full_scan().await;
    let elapsed = started.elapsed();

    assert_eq!(report.total_known, 2);
    assert!(
        elapsed < Duration::from_millis(400),
        "expected parallel scan completion, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn orchestrator_reports_progress_as_scanners_finish() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);
    let progress = Arc::new(StdMutex::new(Vec::<Duration>::new()));
    let fast_delay = Duration::from_millis(40);
    let slow_delay = Duration::from_millis(220);

    orchestrator.add_scanner(Box::new(DelayedScanner::new(
        "Fast",
        fast_delay,
        vec![mock_discovered("Fast Device", "net:progress:fast")],
    )));
    orchestrator.add_scanner(Box::new(DelayedScanner::new(
        "Slow",
        slow_delay,
        vec![mock_discovered("Slow Device", "net:progress:slow")],
    )));

    let started = Instant::now();
    let progress_for_callback = Arc::clone(&progress);
    let report = orchestrator
        .full_scan_with_progress(|delta| {
            let progress = Arc::clone(&progress_for_callback);
            let elapsed = started.elapsed();
            async move {
                if !(delta.new_devices.is_empty() && delta.reappeared_devices.is_empty()) {
                    progress
                        .lock()
                        .expect("progress timings should not be poisoned")
                        .push(elapsed);
                }
            }
        })
        .await;

    let timings = progress
        .lock()
        .expect("progress timings should not be poisoned")
        .clone();
    assert_eq!(report.total_known, 2);
    assert_eq!(timings.len(), 2);
    assert!(
        timings[0] < Duration::from_millis(140),
        "expected first progress callback before slow scanner finished, timings={timings:?}"
    );
    assert!(
        timings[1] >= Duration::from_millis(180),
        "expected second progress callback after slow scanner completed, timings={timings:?}"
    );
}

#[tokio::test]
async fn orchestrator_tracks_reappeared_devices() {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("net:re:ap:pe:ar:ed".to_owned());

    // Pre-populate the registry with a known device
    let existing = mock_device_info("Known Device");
    let existing_id = registry
        .add_with_fingerprint(existing.clone(), fingerprint.clone())
        .await;

    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    // Scanner rediscovers the same device (same DeviceId in info)
    let rediscovered = DiscoveredDevice {
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: existing,
        metadata: HashMap::new(),
    };

    orchestrator.add_scanner(Box::new(MockScanner::new("mDNS", vec![rediscovered])));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.reappeared_devices.len(), 1);
    assert_eq!(report.reappeared_devices[0], existing_id);
    assert_eq!(report.new_devices.len(), 0);
    assert_eq!(report.vanished_devices.len(), 0);
}

#[tokio::test]
async fn orchestrator_tracks_vanished_devices() {
    let registry = DeviceRegistry::new();

    let keep_fingerprint = DeviceFingerprint("net:keep:device".to_owned());
    let vanished_fingerprint = DeviceFingerprint("net:gone:device".to_owned());

    let keep = mock_device_info("Keep");
    let vanished = mock_device_info("Gone");

    let keep_id = registry
        .add_with_fingerprint(keep.clone(), keep_fingerprint.clone())
        .await;
    let vanished_id = registry
        .add_with_fingerprint(vanished, vanished_fingerprint)
        .await;

    let mut orchestrator = DiscoveryOrchestrator::new(registry);
    orchestrator.add_scanner(Box::new(MockScanner::new(
        "mDNS",
        vec![DiscoveredDevice {
            fingerprint: keep_fingerprint,
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info: keep,
            metadata: HashMap::new(),
        }],
    )));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.reappeared_devices, vec![keep_id]);
    assert_eq!(report.vanished_devices, vec![vanished_id]);
}

#[tokio::test]
async fn orchestrator_reappeared_device_keeps_stable_id_when_scanner_emits_new_id() {
    let registry = DeviceRegistry::new();
    let fingerprint = DeviceFingerprint("net:stable:id".to_owned());

    let existing = mock_device_info("Stable");
    let existing_id = registry
        .add_with_fingerprint(existing, fingerprint.clone())
        .await;

    let mut rediscovered = mock_device_info("Stable");
    rediscovered.id = DeviceId::new(); // scanner emits a fresh ID

    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());
    orchestrator.add_scanner(Box::new(MockScanner::new(
        "mDNS",
        vec![DiscoveredDevice {
            fingerprint,
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info: rediscovered,
            metadata: HashMap::new(),
        }],
    )));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.reappeared_devices, vec![existing_id]);

    let tracked = registry
        .get(&existing_id)
        .await
        .expect("stable registry entry should remain");
    assert_eq!(tracked.info.id, existing_id);
}

#[tokio::test]
async fn orchestrator_provides_registry_access() {
    let registry = DeviceRegistry::new();
    registry.add(mock_device_info("Pre-existing")).await;

    let orchestrator = DiscoveryOrchestrator::new(registry);

    let count = orchestrator.registry().len().await;
    assert_eq!(count, 1);
}
