use std::collections::{HashMap, VecDeque};
use std::fs;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use hypercolor_core::device::{SmBusBackend, SmBusScanner};
use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceDeliveryStatus, DeviceFrameSink, DeviceLifecyclePolicy, DiscoveredDevice,
    DiscoveryConnectBehavior,
};
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use hypercolor_hal::SmBusProbeError;
use hypercolor_hal::drivers::asus::smbus::{AuraSmBusProtocol, decode_ene_transaction};
use hypercolor_hal::protocol::Protocol;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use hypercolor_hal::transport::TransportPlatform;
use hypercolor_hal::transport::smbus::{
    SmBusBusArbiter, SmBusClaim, SmBusOperation, SmBusTransportScope, decode_operations,
};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    SegmentInfo,
};
use tempfile::tempdir;

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

/// Watches device-claimed frame batches the way a physical bus would see them.
#[derive(Default)]
struct SmBusLaneProbe {
    measuring: AtomicBool,
    rendezvous: AtomicBool,
    released: AtomicBool,
    active_frames: AtomicUsize,
    max_active_frames: AtomicUsize,
    operations: StdMutex<HashMap<u16, Vec<SmBusOperation>>>,
}

impl SmBusLaneProbe {
    fn wait_until(condition: impl Fn() -> bool, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !condition() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Hold a batch at its first delay until a partner batch is inside too and
    /// the test releases both. Bounded, so a serialized bus fails rather than
    /// hangs.
    fn hold(&self) {
        if !self.rendezvous.load(Ordering::SeqCst) {
            return;
        }
        Self::wait_until(
            || self.active_frames.load(Ordering::SeqCst) >= 2,
            Duration::from_secs(1),
        );
        Self::wait_until(
            || self.released.load(Ordering::SeqCst),
            Duration::from_secs(2),
        );
    }

    fn take_operations(&self, address: u16) -> Vec<SmBusOperation> {
        self.operations
            .lock()
            .expect("operation log lock should not be poisoned")
            .remove(&address)
            .unwrap_or_default()
    }
}

/// Fake device transport that claims the bus exactly as the Linux transport
/// does for a connected device, then logs what reaches the wire.
struct LaneSmBusTransport {
    address: u16,
    bus_arbiter: SmBusBusArbiter,
    probe: Arc<SmBusLaneProbe>,
    op_lock: tokio::sync::Mutex<()>,
    responses: StdMutex<VecDeque<Vec<u8>>>,
}

impl LaneSmBusTransport {
    fn new(address: u16, bus_arbiter: SmBusBusArbiter, probe: Arc<SmBusLaneProbe>) -> Self {
        Self {
            address,
            bus_arbiter,
            probe,
            op_lock: tokio::sync::Mutex::new(()),
            responses: StdMutex::new(VecDeque::from([
                dram_firmware_response(),
                dram_config_response(8),
            ])),
        }
    }

    async fn execute(&self, data: &[u8]) -> Result<(), TransportError> {
        let operations = decode_operations(data)?;
        let claim = SmBusClaim::for_batch(SmBusTransportScope::Device, self.address, &operations);
        let probe = Arc::clone(&self.probe);
        let address = self.address;
        let _guard = self.op_lock.lock().await;
        self.bus_arbiter
            .run_blocking_claimed(claim, move || {
                if !probe.measuring.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let active = probe.active_frames.fetch_add(1, Ordering::SeqCst) + 1;
                probe.max_active_frames.fetch_max(active, Ordering::SeqCst);
                let mut held = false;
                for operation in operations {
                    if !held && matches!(operation, SmBusOperation::Delay { .. }) {
                        held = true;
                        probe.hold();
                    }
                    probe
                        .operations
                        .lock()
                        .expect("operation log lock should not be poisoned")
                        .entry(address)
                        .or_default()
                        .push(operation);
                }
                probe.active_frames.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
    }
}

#[async_trait::async_trait]
impl Transport for LaneSmBusTransport {
    fn name(&self) -> &'static str {
        "Lane SMBus"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.execute(data).await
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
        self.execute(data).await?;
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

fn reference_dram_frame(colors: &[[u8; 3]]) -> Vec<SmBusOperation> {
    let protocol = AuraSmBusProtocol::new();
    protocol
        .parse_response(&dram_firmware_response())
        .expect("reference firmware should parse");
    protocol
        .parse_response(&dram_config_response(8))
        .expect("reference config should parse");
    let commands = protocol.encode_frame(colors);
    assert_eq!(commands.len(), 1, "a DRAM frame is one transaction");
    decode_ene_transaction(&commands[0].data).expect("reference frame should decode")
}

fn assert_root_scoped_smbus_scan<T: std::fmt::Debug>(result: Result<Vec<T>>) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let devices = result.expect("scan should succeed on supported platforms");
        assert!(devices.is_empty());
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let error = result.expect_err("scan should reject an unsupported platform");
        let error_chain = error.chain().map(ToString::to_string).collect::<Vec<_>>();
        assert!(
            matches!(
                error.downcast_ref::<SmBusProbeError>(),
                Some(SmBusProbeError::Transport(
                    TransportError::UnsupportedPlatform {
                        transport: "SMBus",
                        platform,
                    }
                )) if *platform == TransportPlatform::CURRENT
            ),
            "unexpected SMBus scan error chain: {error_chain:?}"
        );
    }
}

#[tokio::test]
async fn smbus_scanner_ignores_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());

    assert_root_scoped_smbus_scan(scanner.scan().await);
}

#[tokio::test]
async fn smbus_scanner_ignores_non_device_i2c_nodes() {
    let tempdir = tempdir().expect("tempdir should create");
    let fake_bus = tempdir.path().join("i2c-0");
    fs::write(&fake_bus, b"not a real i2c bus").expect("fake i2c node should write");

    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());
    assert_root_scoped_smbus_scan(scanner.scan().await);
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
async fn smbus_discovery_source_is_empty_on_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());

    assert_root_scoped_smbus_scan(scanner.scan().await);
}

#[tokio::test]
async fn smbus_backend_preserves_transport_timeout_without_private_retry() {
    let device_id = DeviceId::new();
    let discovered = discovered_smbus_device(device_id);
    let open_count = Arc::new(AtomicUsize::new(0));
    let first_packets = Arc::new(StdMutex::new(Vec::<Vec<u8>>::new()));
    let close_count = Arc::new(AtomicUsize::new(0));

    let backend = SmBusBackend::with_transport_factory({
        let open_count = Arc::clone(&open_count);
        let first_packets = Arc::clone(&first_packets);
        let close_count = Arc::clone(&close_count);

        move |bus_path, address, _bus_arbiter| {
            assert_eq!(bus_path, "/dev/i2c-9");
            assert_eq!(address, 0x71);

            assert_eq!(open_count.fetch_add(1, Ordering::SeqCst), 0);
            Ok(Box::new(ScriptedTransport::new(
                vec![Ok(dram_firmware_response()), Ok(dram_config_response(8))],
                vec![Ok(()), Err(TransportError::Timeout { timeout_ms: 275 })],
                Arc::clone(&first_packets),
                Arc::clone(&close_count),
            )))
        }
    });

    backend
        .adopt_device(&discovered)
        .expect("backend should adopt discovery descriptor");

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
    let error = backend
        .write_colors(&device_id, &[[0x10, 0x20, 0x30]; 8])
        .await
        .expect_err("transport timeout should cross the backend boundary");
    assert!(matches!(
        error,
        DeviceError::Timeout { after } if after == Duration::from_millis(275)
    ));
    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");

    assert_eq!(open_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        first_packets
            .lock()
            .expect("first packet log lock should not be poisoned")
            .len(),
        4,
        "first transport should see init traffic plus the failed frame write"
    );
    assert_eq!(
        close_count.load(Ordering::SeqCst),
        1,
        "disconnect should close the original transport"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smbus_backend_instances_share_one_physical_bus_arbiter() {
    let first_id = DeviceId::new();
    let second_id = DeviceId::new();
    let discovered = [
        discovered_smbus_device_at(first_id, "/dev/i2c-9", 0x71),
        discovered_smbus_device_at(second_id, "/dev/i2c-9", 0x73),
    ];
    let probe = Arc::new(SmBusConcurrencyProbe::new());
    let make_backend = || {
        let probe = Arc::clone(&probe);
        SmBusBackend::with_transport_factory(move |bus_path, address, bus_arbiter| {
            assert_eq!(bus_path, "/dev/i2c-9");
            assert!(matches!(address, 0x71 | 0x73));
            Ok(Box::new(ConcurrentSmBusTransport::new(
                bus_arbiter,
                Arc::clone(&probe),
            )))
        })
    };
    let first_backend = make_backend();
    let second_backend = make_backend();

    first_backend
        .adopt_device(&discovered[0])
        .expect("first backend should adopt its discovery descriptor");
    second_backend
        .adopt_device(&discovered[1])
        .expect("second backend should adopt its discovery descriptor");
    first_backend
        .connect(&first_id)
        .await
        .expect("first connect should succeed");
    second_backend
        .connect(&second_id)
        .await
        .expect("second connect should succeed");
    let first_sink = first_backend
        .frame_sink(&first_id)
        .expect("first device should expose a frame sink");
    let second_sink = second_backend
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
    first_backend
        .disconnect(&first_id)
        .await
        .expect("first disconnect should succeed");
    second_backend
        .disconnect(&second_id)
        .await
        .expect("second disconnect should succeed");
    let stale_sink_error = first_sink
        .write_colors_shared(Arc::new(vec![[0, 0, 0]; 8]))
        .await
        .expect_err("stale sink should reject writes after disconnect");
    assert!(stale_sink_error.to_string().contains("disconnected"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smbus_frame_writes_overlap_across_devices_while_bus_claims_exclude_them() {
    const BUS_PATH: &str = "/dev/i2c-lane-overlap-test";
    let first_id = DeviceId::new();
    let second_id = DeviceId::new();
    let probe = Arc::new(SmBusLaneProbe::default());
    let backend = SmBusBackend::with_transport_factory({
        let probe = Arc::clone(&probe);
        move |bus_path, address, bus_arbiter| {
            assert_eq!(bus_path, BUS_PATH);
            Ok(Box::new(LaneSmBusTransport::new(
                address,
                bus_arbiter,
                Arc::clone(&probe),
            )))
        }
    });
    for (id, address) in [(first_id, 0x71), (second_id, 0x73)] {
        backend
            .adopt_device(&discovered_smbus_device_at(id, BUS_PATH, address))
            .expect("backend should adopt the DIMM");
        backend.connect(&id).await.expect("DIMM should connect");
    }
    let first_sink = backend
        .frame_sink(&first_id)
        .expect("first DIMM should expose a frame sink");
    let second_sink = backend
        .frame_sink(&second_id)
        .expect("second DIMM should expose a frame sink");
    let first_colors = vec![[0x10, 0x20, 0x30]; 8];
    let second_colors = vec![[0x40, 0x50, 0x60]; 8];
    let bus = SmBusBusArbiter::for_bus(BUS_PATH);

    probe.rendezvous.store(true, Ordering::SeqCst);
    probe.measuring.store(true, Ordering::SeqCst);
    let frames = tokio::spawn({
        let first_sink = Arc::clone(&first_sink);
        let second_sink = Arc::clone(&second_sink);
        let first_colors = Arc::new(first_colors.clone());
        let second_colors = Arc::new(second_colors.clone());
        async move {
            tokio::join!(
                first_sink.write_colors_shared(first_colors),
                second_sink.write_colors_shared(second_colors),
            )
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while probe.active_frames.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("both DIMMs' frames should be on the bus at once");

    let mut discovery = tokio::spawn({
        let bus = bus.clone();
        let probe = Arc::clone(&probe);
        async move {
            bus.run_blocking(move || Ok(probe.active_frames.load(Ordering::SeqCst)))
                .await
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut discovery)
            .await
            .is_err(),
        "a probe must wait for in-flight frame writes"
    );
    probe.released.store(true, Ordering::SeqCst);
    let (first, second) = tokio::time::timeout(Duration::from_secs(2), frames)
        .await
        .expect("frame writes should finish once released")
        .expect("frame task should complete");
    first.expect("first frame write should succeed");
    second.expect("second frame write should succeed");
    let frames_seen_by_probe = tokio::time::timeout(Duration::from_secs(1), discovery)
        .await
        .expect("probe should run after the frames")
        .expect("probe task should complete")
        .expect("probe should succeed");
    assert_eq!(frames_seen_by_probe, 0, "a probe must never share the bus");
    assert_eq!(probe.max_active_frames.load(Ordering::SeqCst), 2);
    assert_eq!(
        probe.take_operations(0x71),
        reference_dram_frame(&first_colors),
        "the first DIMM's transactions must reach the wire whole and in order"
    );
    assert_eq!(
        probe.take_operations(0x73),
        reference_dram_frame(&second_colors),
        "the second DIMM's transactions must reach the wire whole and in order"
    );

    probe.rendezvous.store(false, Ordering::SeqCst);
    let remap = bus.acquire_transaction().await;
    let mut frames = tokio::spawn({
        let first_sink = Arc::clone(&first_sink);
        let second_sink = Arc::clone(&second_sink);
        async move {
            tokio::join!(
                first_sink.write_colors_shared(Arc::new(vec![[1, 2, 3]; 8])),
                second_sink.write_colors_shared(Arc::new(vec![[4, 5, 6]; 8])),
            )
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut frames)
            .await
            .is_err(),
        "frame writes must wait while a remap holds the bus"
    );
    assert_eq!(probe.active_frames.load(Ordering::SeqCst), 0);
    drop(remap);
    let (first, second) = tokio::time::timeout(Duration::from_secs(2), frames)
        .await
        .expect("frame writes should resume after the remap")
        .expect("frame task should complete");
    first.expect("first frame write should succeed after the remap");
    second.expect("second frame write should succeed after the remap");
    probe.measuring.store(false, Ordering::SeqCst);
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

#[derive(Default)]
struct CountingDeliveryObserver {
    started: AtomicUsize,
}

impl DeviceDeliveryObserver for CountingDeliveryObserver {
    fn transport_started(&self, _id: DeviceDeliveryId) {
        self.started.fetch_add(1, Ordering::SeqCst);
    }
}

struct ScriptedDram {
    backend: SmBusBackend,
    device_id: DeviceId,
    sink: Arc<dyn DeviceFrameSink>,
    packets: Arc<StdMutex<Vec<Vec<u8>>>>,
    observer: Arc<CountingDeliveryObserver>,
    sequence: u64,
}

impl ScriptedDram {
    /// Connect one DIMM whose frame sends succeed or fail per `frame_sends`
    /// in order, then succeed.
    async fn connect(frame_sends: Vec<bool>) -> Self {
        let device_id = DeviceId::new();
        let packets = Arc::new(StdMutex::new(Vec::<Vec<u8>>::new()));
        let backend = SmBusBackend::with_transport_factory({
            let packets = Arc::clone(&packets);
            move |_bus_path, _address, _bus_arbiter| {
                let mut send_results = vec![Ok(())];
                send_results.extend(frame_sends.iter().map(|&succeeds| {
                    if succeeds {
                        Ok(())
                    } else {
                        Err(TransportError::IoError {
                            detail: "scripted frame failure".to_owned(),
                        })
                    }
                }));
                Ok(Box::new(ScriptedTransport::new(
                    vec![Ok(dram_firmware_response()), Ok(dram_config_response(8))],
                    send_results,
                    Arc::clone(&packets),
                    Arc::new(AtomicUsize::new(0)),
                )))
            }
        });
        backend
            .adopt_device(&discovered_smbus_device(device_id))
            .expect("backend should adopt the DIMM");
        backend
            .connect(&device_id)
            .await
            .expect("DIMM should connect");
        let sink = backend
            .frame_sink(&device_id)
            .expect("DIMM should expose a frame sink");
        packets
            .lock()
            .expect("packet log lock should not be poisoned")
            .clear();

        Self {
            backend,
            device_id,
            sink,
            packets,
            observer: Arc::new(CountingDeliveryObserver::default()),
            sequence: 0,
        }
    }

    async fn deliver(&mut self, colors: Vec<[u8; 3]>) -> DeviceDeliveryAck {
        self.sequence += 1;
        let observer: Arc<dyn DeviceDeliveryObserver> = self.observer.clone();
        self.sink
            .deliver_colors_shared_observed(
                DeviceDeliveryId {
                    queue_generation: 1,
                    sequence: self.sequence,
                },
                Arc::new(colors),
                observer,
            )
            .await
    }

    fn take_frames(&self) -> Vec<Vec<SmBusOperation>> {
        std::mem::take(
            &mut *self
                .packets
                .lock()
                .expect("packet log lock should not be poisoned"),
        )
        .iter()
        .map(|packet| decode_operations(packet).expect("frame packet should decode"))
        .collect()
    }
}

fn block_writes(operations: &[SmBusOperation]) -> usize {
    operations
        .iter()
        .filter(|operation| matches!(operation, SmBusOperation::WriteBlockData { .. }))
        .count()
}

fn steady_dram_frame() -> Vec<[u8; 3]> {
    vec![[0x10, 0x20, 0x30]; 8]
}

#[tokio::test]
async fn smbus_backend_writes_only_changed_leds_after_a_confirmed_frame() {
    let mut dram = ScriptedDram::connect(Vec::new()).await;

    let first = dram.deliver(steady_dram_frame()).await;
    assert_eq!(first.status, DeviceDeliveryStatus::Completed);
    assert_eq!(first.completed_payload_bytes, 24);
    let frames = dram.take_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(block_writes(&frames[0]), 8);

    let mut changed = steady_dram_frame();
    changed[3] = [0x01, 0x02, 0x03];
    let second = dram.deliver(changed).await;
    assert_eq!(second.status, DeviceDeliveryStatus::Completed);
    assert_eq!(
        second.completed_payload_bytes, 3,
        "the acknowledgement should count only the LED that was written"
    );
    let frames = dram.take_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(block_writes(&frames[0]), 1);
    assert_eq!(dram.observer.started.load(Ordering::SeqCst), 2);

    dram.backend
        .disconnect(&dram.device_id)
        .await
        .expect("disconnect should succeed");
}

#[tokio::test]
async fn smbus_backend_rewrites_every_led_after_a_failed_write() {
    let mut dram = ScriptedDram::connect(vec![true, false]).await;
    let mut changed = steady_dram_frame();
    changed[6] = [0xFF, 0xFF, 0xFF];

    let first = dram.deliver(steady_dram_frame()).await;
    assert_eq!(first.status, DeviceDeliveryStatus::Completed);
    let failed = dram.deliver(changed.clone()).await;
    assert_eq!(failed.status, DeviceDeliveryStatus::Failed);
    let retried = dram.deliver(changed).await;
    assert_eq!(retried.status, DeviceDeliveryStatus::Completed);

    let frames = dram.take_frames();
    assert_eq!(frames.len(), 3);
    assert_eq!(block_writes(&frames[1]), 1, "the failed frame was a delta");
    assert_eq!(
        block_writes(&frames[2]),
        8,
        "a failed write leaves the DIMM's colors unknown, so every LED is rewritten"
    );
    assert_eq!(retried.completed_payload_bytes, 24);

    dram.backend
        .disconnect(&dram.device_id)
        .await
        .expect("disconnect should succeed");
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
        fingerprint: DeviceFingerprint::from_persisted(format!("smbus:{bus_path}:{address:02x}")),
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
            segments: vec![SegmentInfo {
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
        claim: None,
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
