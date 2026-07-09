use std::collections::VecDeque;
use std::fs;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use hypercolor_core::device::{
    DeviceBackend, DeviceLifecyclePolicy, DiscoveredDevice, DiscoveryConnectBehavior, SmBusBackend,
    SmBusScanner, TransportScanner,
};
use hypercolor_hal::transport::smbus::{SmBusBusArbiter, SmBusOperation, decode_operations};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
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

struct SmBusConcurrencyProbe {
    measuring: AtomicBool,
    active_transactions: AtomicUsize,
    max_active_transactions: AtomicUsize,
    active_waits: AtomicUsize,
    max_active_waits: AtomicUsize,
    wait_barrier: tokio::sync::Barrier,
}

struct SmBusTransactionRelease(Option<tokio::sync::oneshot::Sender<()>>);

impl SmBusTransactionRelease {
    fn release(mut self) {
        self.0
            .take()
            .expect("transaction release should remain available")
            .send(())
            .expect("blocking transaction should still be alive");
    }
}

impl Drop for SmBusTransactionRelease {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

impl SmBusConcurrencyProbe {
    fn new() -> Self {
        Self {
            measuring: AtomicBool::new(false),
            active_transactions: AtomicUsize::new(0),
            max_active_transactions: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            max_active_waits: AtomicUsize::new(0),
            wait_barrier: tokio::sync::Barrier::new(2),
        }
    }

    fn begin(&self) {
        self.active_transactions.store(0, Ordering::SeqCst);
        self.max_active_transactions.store(0, Ordering::SeqCst);
        self.active_waits.store(0, Ordering::SeqCst);
        self.max_active_waits.store(0, Ordering::SeqCst);
        self.measuring.store(true, Ordering::SeqCst);
    }

    fn finish(&self) {
        self.measuring.store(false, Ordering::SeqCst);
    }
}

struct ConcurrentSmBusTransport {
    bus_arbiter: SmBusBusArbiter,
    probe: Arc<SmBusConcurrencyProbe>,
    responses: StdMutex<VecDeque<Vec<u8>>>,
}

impl ConcurrentSmBusTransport {
    fn new(bus_arbiter: SmBusBusArbiter, probe: Arc<SmBusConcurrencyProbe>) -> Self {
        Self {
            bus_arbiter,
            probe,
            responses: StdMutex::new(VecDeque::from([
                dram_firmware_response(),
                dram_config_response(8),
            ])),
        }
    }

    async fn execute_operations(&self, data: &[u8]) -> Result<(), TransportError> {
        for operation in decode_operations(data)? {
            if let SmBusOperation::Delay { duration } = operation {
                if self.probe.measuring.load(Ordering::SeqCst) {
                    let active = self.probe.active_waits.fetch_add(1, Ordering::SeqCst) + 1;
                    self.probe
                        .max_active_waits
                        .fetch_max(active, Ordering::SeqCst);
                    self.probe.wait_barrier.wait().await;
                    tokio::time::sleep(duration).await;
                    self.probe.active_waits.fetch_sub(1, Ordering::SeqCst);
                } else {
                    tokio::time::sleep(duration).await;
                }
            } else {
                let _transaction = self.bus_arbiter.acquire_transaction().await;
                if self.probe.measuring.load(Ordering::SeqCst) {
                    let active = self
                        .probe
                        .active_transactions
                        .fetch_add(1, Ordering::SeqCst)
                        + 1;
                    self.probe
                        .max_active_transactions
                        .fetch_max(active, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    self.probe
                        .active_transactions
                        .fetch_sub(1, Ordering::SeqCst);
                }
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl Transport for ConcurrentSmBusTransport {
    fn name(&self) -> &'static str {
        "Concurrent SMBus"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.execute_operations(data).await
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
        self.execute_operations(data).await?;
        self.responses
            .lock()
            .expect("response lock should not be poisoned")
            .pop_front()
            .ok_or_else(|| TransportError::IoError {
                detail: "unexpected SMBus response request".to_owned(),
            })
    }

    async fn close(&self) -> Result<(), TransportError> {
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

#[test]
fn smbus_backend_lifecycle_policy_runs_connect_in_background() {
    let backend = SmBusBackend::new();
    let info = discovered_smbus_device(DeviceId::new()).info;
    let policy = backend.lifecycle_policy(&info);

    assert!(policy.connect_execution().is_background());
    assert_eq!(
        policy.connect_timeout(),
        DeviceLifecyclePolicy::DEFAULT_CONNECT_TIMEOUT
    );
    assert!(policy.retry_on_connect_timeout());
}

#[tokio::test]
async fn smbus_backend_discover_is_empty_on_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut backend = SmBusBackend::with_scanner(SmBusScanner::with_dev_root(tempdir.path()));

    let devices = backend.discover().await.expect("discover should succeed");
    assert!(devices.is_empty());
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

        move |bus_path, address, _bus_arbiter| {
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
        .connect(&device_id)
        .await
        .expect("duplicate connect should be idempotent");
    assert_eq!(
        open_count.load(Ordering::SeqCst),
        1,
        "duplicate connect should preserve the active device and its frame sinks"
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smbus_device_sinks_overlap_waits_without_overlapping_bus_transactions() {
    let first_id = DeviceId::new();
    let second_id = DeviceId::new();
    let scanner = StaticScanner::new(vec![
        discovered_smbus_device_at(first_id, "/dev/i2c-9", 0x71),
        discovered_smbus_device_at(second_id, "/dev/i2c-9", 0x73),
    ]);
    let probe = Arc::new(SmBusConcurrencyProbe::new());
    let mut backend = SmBusBackend::with_scanner_and_transport_factory(scanner, {
        let probe = Arc::clone(&probe);
        move |bus_path, address, bus_arbiter| {
            assert_eq!(bus_path, "/dev/i2c-9");
            assert!(matches!(address, 0x71 | 0x73));
            Ok(Box::new(ConcurrentSmBusTransport::new(
                bus_arbiter,
                Arc::clone(&probe),
            )))
        }
    });

    let devices = backend.discover().await.expect("discover should succeed");
    assert_eq!(devices.len(), 2);
    backend
        .connect(&first_id)
        .await
        .expect("first connect should succeed");
    backend
        .connect(&second_id)
        .await
        .expect("second connect should succeed");
    let first_sink = backend
        .frame_sink(&first_id)
        .expect("first device should expose a frame sink");
    let second_sink = backend
        .frame_sink(&second_id)
        .expect("second device should expose a frame sink");

    probe.begin();
    let writes = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            first_sink.write_colors_shared(Arc::new(vec![[0x10, 0x20, 0x30]; 8])),
            second_sink.write_colors_shared(Arc::new(vec![[0x40, 0x50, 0x60]; 8])),
        )
    })
    .await
    .expect("independent device waits should overlap without deadlocking");
    writes.0.expect("first frame write should succeed");
    writes.1.expect("second frame write should succeed");

    assert_eq!(probe.max_active_transactions.load(Ordering::SeqCst), 1);
    assert_eq!(probe.max_active_waits.load(Ordering::SeqCst), 2);

    probe.finish();
    backend
        .disconnect(&first_id)
        .await
        .expect("first disconnect should succeed");
    let stale_sink_error = first_sink
        .write_colors_shared(Arc::new(vec![[0, 0, 0]; 8]))
        .await
        .expect_err("stale sink should reject writes after disconnect");
    assert!(stale_sink_error.to_string().contains("disconnected"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smbus_bus_arbiter_retains_ownership_after_waiter_cancellation() {
    let arbiter = SmBusBusArbiter::default();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release = SmBusTransactionRelease(Some(release_tx));
    let first_arbiter = arbiter.clone();
    let first = tokio::spawn(async move {
        first_arbiter
            .run_blocking(move || {
                started_tx.send(()).map_err(|()| TransportError::IoError {
                    detail: "failed to signal transaction start".to_owned(),
                })?;
                release_rx
                    .blocking_recv()
                    .map_err(|_| TransportError::IoError {
                        detail: "failed to receive transaction release".to_owned(),
                    })
            })
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .expect("first transaction should enter its blocking operation")
        .expect("transaction start sender should remain available");
    first.abort();
    assert!(
        first
            .await
            .expect_err("cancelled waiter should stop")
            .is_cancelled()
    );

    let second = arbiter.run_blocking(|| Ok(()));
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "cancelled waiter must not release an in-progress bus transaction"
    );

    release.release();
    tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .expect("second transaction should proceed after the first completes")
        .expect("second transaction should succeed");
}

fn discovered_smbus_device(device_id: DeviceId) -> DiscoveredDevice {
    discovered_smbus_device_at(device_id, "/dev/i2c-9", 0x71)
}

fn discovered_smbus_device_at(
    device_id: DeviceId,
    bus_path: &str,
    address: u16,
) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: DeviceFingerprint(format!("smbus:{bus_path}:{address:02x}")),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: DeviceInfo {
            id: device_id,
            name: format!("ASUS Aura DRAM (SMBus 0x{address:02X})"),
            vendor: "ASUS".to_owned(),
            family: DeviceFamily::new_static("asus", "ASUS"),
            model: Some("asus_aura_smbus_dram".to_owned()),
            connection_type: ConnectionType::SmBus,
            origin: DeviceOrigin::native("asus", "smbus", ConnectionType::SmBus)
                .with_protocol_id("asus/aura-smbus"),
            zones: vec![ZoneInfo {
                name: "Lighting".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: Some("AUDA0-E6K5-0101".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 8,
                supports_direct: true,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: [
            ("bus_path".to_owned(), bus_path.to_owned()),
            ("smbus_address".to_owned(), format!("0x{address:02X}")),
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
