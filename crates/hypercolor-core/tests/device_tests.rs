//! Tests for the device backend system.
//!
//! All tests use mock backend implementations — no real hardware is required.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::Mutex;

use hypercolor_core::device::{
    BackendInfo, DeviceBackend, DevicePlugin, DeviceRegistry, DiscoveredDevice,
    DiscoveryOrchestrator, TransportScanner,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

/// Build a synthetic [`DeviceInfo`] for testing.
fn mock_device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.to_owned(),
        vendor: "MockVendor".to_owned(),
        family: DeviceFamily::Wled,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Zone 1".to_owned(),
            led_count: 30,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".to_owned()),
        capabilities: DeviceCapabilities::default(),
    }
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
        connection_type: ConnectionType::Network,
        name: name.to_owned(),
        family: DeviceFamily::Wled,
        fingerprint: DeviceFingerprint(fingerprint.to_owned()),
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

// ── DiscoveryOrchestrator tests ──────────────────────────────────────────

#[tokio::test]
async fn orchestrator_full_scan_empty() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let report = orchestrator.full_scan().await;
    assert_eq!(report.new_devices.len(), 0);
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
        connection_type: ConnectionType::Network,
        name: "Known Device".to_owned(),
        family: DeviceFamily::Wled,
        fingerprint,
        info: existing,
        metadata: HashMap::new(),
    };

    orchestrator.add_scanner(Box::new(MockScanner::new("mDNS", vec![rediscovered])));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.reappeared_devices.len(), 1);
    assert_eq!(report.reappeared_devices[0], existing_id);
    assert_eq!(report.new_devices.len(), 0);
}

#[tokio::test]
async fn orchestrator_provides_registry_access() {
    let registry = DeviceRegistry::new();
    registry.add(mock_device_info("Pre-existing")).await;

    let orchestrator = DiscoveryOrchestrator::new(registry);

    let count = orchestrator.registry().len().await;
    assert_eq!(count, 1);
}
