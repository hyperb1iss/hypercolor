use std::fs;
use std::os::unix::fs as unix_fs;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use hypercolor_core::device::smbus_scanner::{
    dram_capable_pci_id, resolve_parent_pci_id_from_sysfs_path,
};
use hypercolor_core::device::{
    DeviceBackend, DiscoveredDevice, DiscoveryConnectBehavior, SmBusBackend, SmBusScanner,
    TransportScanner,
};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo,
};
use tempfile::tempdir;

#[derive(Clone)]
struct StaticScanner {
    devices: Vec<DiscoveredDevice>,
}

impl StaticScanner {
    fn new(devices: Vec<DiscoveredDevice>) -> Self {
        Self { devices }
    }
}

#[async_trait::async_trait]
impl TransportScanner for StaticScanner {
    fn name(&self) -> &'static str {
        "static-smbus-test"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        Ok(self.devices.clone())
    }
}

struct ScriptedTransport {
    send_receive_results: StdMutex<Vec<Result<Vec<u8>, TransportError>>>,
    send_results: StdMutex<Vec<Result<(), TransportError>>>,
    sent_packets: Arc<StdMutex<Vec<Vec<u8>>>>,
    close_count: Arc<AtomicUsize>,
}

impl ScriptedTransport {
    fn new(
        send_receive_results: Vec<Result<Vec<u8>, TransportError>>,
        send_results: Vec<Result<(), TransportError>>,
        sent_packets: Arc<StdMutex<Vec<Vec<u8>>>>,
        close_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            send_receive_results: StdMutex::new(send_receive_results),
            send_results: StdMutex::new(send_results),
            sent_packets,
            close_count,
        }
    }
}

#[async_trait::async_trait]
impl Transport for ScriptedTransport {
    fn name(&self) -> &'static str {
        "Scripted SMBus"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.sent_packets
            .lock()
            .expect("sent packet lock should not be poisoned")
            .push(data.to_vec());

        let mut results = self
            .send_results
            .lock()
            .expect("send results lock should not be poisoned");
        if results.is_empty() {
            return Ok(());
        }

        results.remove(0)
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn send_receive(
        &self,
        data: &[u8],
        _timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.sent_packets
            .lock()
            .expect("sent packet lock should not be poisoned")
            .push(data.to_vec());

        let mut results = self
            .send_receive_results
            .lock()
            .expect("send/receive results lock should not be poisoned");
        if results.is_empty() {
            return Err(TransportError::IoError {
                detail: "unexpected send_receive call".to_owned(),
            });
        }

        results.remove(0)
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn smbus_scanner_name_is_stable() {
    let scanner = SmBusScanner::new();
    assert_eq!(scanner.name(), "SMBus HAL");
}

#[tokio::test]
async fn smbus_scanner_ignores_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());

    let devices = scanner.scan().await.expect("scan should succeed");
    assert!(devices.is_empty());
}

#[tokio::test]
async fn smbus_scanner_ignores_non_device_i2c_nodes() {
    let tempdir = tempdir().expect("tempdir should create");
    let fake_bus = tempdir.path().join("i2c-0");
    fs::write(&fake_bus, b"not a real i2c bus").expect("fake i2c node should write");

    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());
    let devices = scanner.scan().await.expect("scan should succeed");

    assert!(devices.is_empty());
}

#[test]
fn smbus_backend_info_reports_hal_transport() {
    let backend = SmBusBackend::new();
    let info = backend.info();

    assert_eq!(info.id, "smbus");
    assert_eq!(info.name, "SMBus (HAL)");
}

#[tokio::test]
async fn smbus_backend_discover_is_empty_on_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut backend = SmBusBackend::with_scanner(SmBusScanner::with_dev_root(tempdir.path()));

    let devices = backend.discover().await.expect("discover should succeed");
    assert!(devices.is_empty());
}

#[test]
fn smbus_scanner_walks_up_sysfs_tree_for_parent_pci_id() {
    let tempdir = tempdir().expect("tempdir should create");
    let pci_root = tempdir.path().join("0000:00:15.0");
    let adapter_root = pci_root.join("i2c_designware.0").join("i2c-0");

    fs::create_dir_all(&adapter_root).expect("adapter tree should create");
    fs::write(pci_root.join("vendor"), "0x8086\n").expect("vendor file should write");
    fs::write(pci_root.join("device"), "0x7A4C\n").expect("device file should write");

    let sysfs_entry = tempdir.path().join("i2c-0-device");
    unix_fs::symlink(&adapter_root, &sysfs_entry).expect("symlink should create");

    let pci_id =
        resolve_parent_pci_id_from_sysfs_path(&sysfs_entry).expect("pci id should resolve");
    assert_eq!(pci_id, (0x8086, 0x7A4C));
}

#[test]
fn smbus_scanner_matches_openrgb_dram_bus_allowlist() {
    assert!(dram_capable_pci_id(0x8086, 0x7A23));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4C));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4D));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4E));
    assert!(!dram_capable_pci_id(0x10DE, 0x2783));
}

#[tokio::test]
async fn smbus_backend_reinitializes_transport_after_write_failure() {
    let device_id = DeviceId::new();
    let scanner = StaticScanner::new(vec![discovered_smbus_device(device_id)]);
    let open_count = Arc::new(AtomicUsize::new(0));
    let first_packets = Arc::new(StdMutex::new(Vec::<Vec<u8>>::new()));
    let second_packets = Arc::new(StdMutex::new(Vec::<Vec<u8>>::new()));
    let close_count = Arc::new(AtomicUsize::new(0));

    let mut backend = SmBusBackend::with_scanner_and_transport_factory(scanner, {
        let open_count = Arc::clone(&open_count);
        let first_packets = Arc::clone(&first_packets);
        let second_packets = Arc::clone(&second_packets);
        let close_count = Arc::clone(&close_count);

        move |bus_path, address| {
            assert_eq!(bus_path, "/dev/i2c-9");
            assert_eq!(address, 0x71);

            let open_index = open_count.fetch_add(1, Ordering::SeqCst);
            match open_index {
                0 => Ok(Box::new(ScriptedTransport::new(
                    vec![Ok(dram_firmware_response()), Ok(dram_config_response(8))],
                    vec![
                        Ok(()),
                        Err(TransportError::IoError {
                            detail: "simulated frame write failure".to_owned(),
                        }),
                    ],
                    Arc::clone(&first_packets),
                    Arc::clone(&close_count),
                ))),
                1 => Ok(Box::new(ScriptedTransport::new(
                    vec![Ok(dram_firmware_response()), Ok(dram_config_response(8))],
                    vec![Ok(()), Ok(())],
                    Arc::clone(&second_packets),
                    Arc::clone(&close_count),
                ))),
                other => bail!("unexpected extra SMBus transport open #{other}"),
            }
        }
    });

    let devices = backend.discover().await.expect("discover should succeed");
    assert_eq!(devices.len(), 1);

    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");
    backend
        .write_colors(&device_id, &[[0x10, 0x20, 0x30]; 8])
        .await
        .expect("write should recover after one transport reinitialize");
    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");

    assert_eq!(open_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        first_packets
            .lock()
            .expect("first packet log lock should not be poisoned")
            .len(),
        4,
        "first transport should see init traffic plus the failed frame write"
    );
    assert_eq!(
        second_packets
            .lock()
            .expect("second packet log lock should not be poisoned")
            .len(),
        4,
        "replacement transport should rerun init and then accept the retried frame"
    );
    assert_eq!(
        close_count.load(Ordering::SeqCst),
        2,
        "recovery should close the stale transport and disconnect should close the replacement"
    );
}

fn discovered_smbus_device(device_id: DeviceId) -> DiscoveredDevice {
    DiscoveredDevice {
        connection_type: ConnectionType::SmBus,
        name: "ASUS Aura DRAM (SMBus 0x71)".to_owned(),
        family: DeviceFamily::Asus,
        fingerprint: DeviceFingerprint("smbus:/dev/i2c-9:71".to_owned()),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: DeviceInfo {
            id: device_id,
            name: "ASUS Aura DRAM (SMBus 0x71)".to_owned(),
            vendor: "ASUS".to_owned(),
            family: DeviceFamily::Asus,
            model: Some("asus_aura_smbus_dram".to_owned()),
            connection_type: ConnectionType::SmBus,
            zones: vec![ZoneInfo {
                name: "Lighting".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("AUDA0-E6K5-0101".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 8,
                supports_direct: true,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
                features: DeviceFeatures::default(),
            },
        },
        metadata: [
            ("backend_id".to_owned(), "smbus".to_owned()),
            ("bus_path".to_owned(), "/dev/i2c-9".to_owned()),
            ("smbus_address".to_owned(), "0x71".to_owned()),
            ("controller_kind".to_owned(), "dram".to_owned()),
            ("firmware_name".to_owned(), "AUDA0-E6K5-0101".to_owned()),
        ]
        .into_iter()
        .collect(),
    }
}

fn dram_firmware_response() -> Vec<u8> {
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUDA0-E6K5-0101");
    firmware.to_vec()
}

fn dram_config_response(led_count: u8) -> Vec<u8> {
    let mut config = [0_u8; 64];
    config[0x02] = led_count;
    config[0x13] = 0x0E;
    config.to_vec()
}
