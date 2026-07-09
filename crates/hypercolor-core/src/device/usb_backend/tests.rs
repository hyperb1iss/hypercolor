use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use anyhow::anyhow;
use async_trait::async_trait;
use hypercolor_driver_api::DeviceDeliveryStatus;
use hypercolor_hal::protocol::{
    ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus, TransferType,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceOrigin, DeviceTopologyHint,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

use super::*;

static USB_ACTOR_METRICS_TEST_LOCK: LazyLock<AsyncMutex<()>> =
    LazyLock::new(|| AsyncMutex::new(()));

fn temporary_control_test_device(supports_direct: bool, led_count: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "USB Test Strip".to_owned(),
        vendor: "Hypercolor".to_owned(),
        family: DeviceFamily::new_static("test", "Test"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", USB_OUTPUT_BACKEND_ID, ConnectionType::Usb),
        zones: if led_count == 0 {
            Vec::new()
        } else {
            vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count,
                topology: DeviceTopologyHint::Strip,
                color_format: hypercolor_types::device::DeviceColorFormat::Rgb,
                layout_hint: None,
            }]
        },
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct,
            ..DeviceCapabilities::default()
        },
    }
}

#[test]
fn usb_backend_supports_temporary_direct_control_for_led_devices() {
    let backend = UsbBackend::new();
    let mut info = temporary_control_test_device(true, 8);

    assert!(backend.supports_temporary_direct_control(&info));

    info.capabilities.supports_direct = false;
    assert!(!backend.supports_temporary_direct_control(&info));

    info.capabilities.supports_direct = true;
    info.zones.clear();
    assert!(!backend.supports_temporary_direct_control(&info));
}

#[test]
fn usb_midi_lifecycle_policy_runs_connect_in_background_without_timeout_retry() {
    let policy = lifecycle_policy_for_transport(TransportType::UsbMidi {
        midi_interface: 2,
        display_interface: 0,
        display_endpoint: 0x01,
    });

    assert!(policy.connect_execution().is_background());
    assert_eq!(policy.connect_timeout(), Duration::from_secs(30));
    assert!(!policy.retry_on_connect_timeout());
}

#[test]
fn usb_non_midi_lifecycle_policy_uses_default_connect_behavior() {
    let policy = lifecycle_policy_for_transport(TransportType::UsbHid { interface: 0 });

    assert_eq!(policy, DeviceLifecyclePolicy::default());
}

#[tokio::test]
async fn display_branch_services_pending_led_frame_before_display_frame() {
    let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
    let before = usb_actor_metrics_snapshot();
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [0x11, 0x22, 0x33],
    ])))));
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
    })));

    let transport =
        Arc::new(RecordingTransport::default().with_send_delay(Duration::from_millis(5)));
    let actor_protocol: Arc<dyn Protocol> = Arc::new(FairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "fairness-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let writes = wait_for_writes(&transport, 2).await;
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should still be open");

    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");

    assert_eq!(writes, vec![vec![0x11], vec![0xD1]]);

    let after = usb_actor_metrics_snapshot();
    assert!(after.display_frames_total > before.display_frames_total);
    assert!(
        after.display_frames_delayed_for_led_total > before.display_frames_delayed_for_led_total
    );
    assert!(after.display_led_priority_wait_total_us > before.display_led_priority_wait_total_us);
    assert!(after.display_led_priority_wait_max_us >= before.display_led_priority_wait_max_us);
}

#[tokio::test]
async fn display_load_services_new_led_before_next_display_frame() {
    let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
    let before = usb_actor_metrics_snapshot();
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
    })));

    let transport =
        Arc::new(RecordingTransport::default().with_send_delay(Duration::from_millis(5)));
    let actor_protocol: Arc<dyn Protocol> = Arc::new(FairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "display-load-fairness-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let writes = wait_for_writes(&transport, 1).await;
    assert_eq!(writes, vec![vec![0xD1]]);

    frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [0x22, 0x33, 0x44],
    ])))));
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD2]))),
    })));

    let writes = wait_for_writes(&transport, 3).await;
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should still be open");

    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");

    assert_eq!(writes, vec![vec![0xD1], vec![0x22], vec![0xD2]]);

    let after = usb_actor_metrics_snapshot();
    assert!(after.display_frames_total >= before.display_frames_total + 2);
    assert!(
        after.display_frames_delayed_for_led_total > before.display_frames_delayed_for_led_total
    );
}

#[tokio::test]
async fn parallel_transfer_lanes_do_not_wait_for_pending_led_frame_before_display() {
    let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
    let before = usb_actor_metrics_snapshot();
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [0x11, 0x22, 0x33],
    ])))));
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
    })));

    let transport = Arc::new(
        RecordingTransport::default()
            .with_parallel_transfer_lanes()
            .with_primary_send_delay(Duration::from_millis(200)),
    );
    let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();

    let actor = tokio::spawn(UsbBackend::test_run_parallel_device_actor(
        DeviceId::new(),
        "parallel-fairness-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let writes = wait_for_writes(&transport, 1).await;
    assert_eq!(writes, vec![vec![0xD1]]);

    let writes = wait_for_writes(&transport, 2).await;
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should still be open");

    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");

    assert_eq!(writes, vec![vec![0xD1], vec![0x11]]);

    let after = usb_actor_metrics_snapshot();
    assert!(after.display_frames_total > before.display_frames_total);
    assert_eq!(
        after.display_frames_delayed_for_led_total,
        before.display_frames_delayed_for_led_total
    );
}

#[tokio::test]
async fn display_write_failure_does_not_stop_single_lane_led_actor() {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
    })));

    let transport =
        Arc::new(RecordingTransport::default().with_failed_transfer_type(TransferType::Bulk));
    let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "display-failure-single-lane-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    tokio::time::sleep(Duration::from_millis(20)).await;
    frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [0x22, 0x33, 0x44],
    ])))));

    assert_eq!(wait_for_writes(&transport, 1).await, vec![vec![0x22]]);

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should stay open after display failure");
    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");
}

#[tokio::test]
async fn parallel_display_write_failure_does_not_stop_control_lane() {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
        payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
    })));

    let transport = Arc::new(
        RecordingTransport::default()
            .with_parallel_transfer_lanes()
            .with_failed_transfer_type(TransferType::Bulk),
    );
    let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();

    let actor = tokio::spawn(UsbBackend::test_run_parallel_device_actor(
        DeviceId::new(),
        "display-failure-parallel-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    tokio::time::sleep(Duration::from_millis(20)).await;
    frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [0x33, 0x44, 0x55],
    ])))));

    assert_eq!(wait_for_writes(&transport, 1).await, vec![vec![0x33]]);

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("control command channel should stay open after display failure");
    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");
}

#[test]
fn transient_and_fatal_frame_write_errors_classify_transport_liveness() {
    let transient_errors = [
        TransportError::Timeout { timeout_ms: 25 },
        TransportError::IoError {
            detail: "temporary bus contention".to_owned(),
        },
    ];
    for error in transient_errors {
        let error = anyhow!(error).context("USB frame write failed");
        assert_eq!(
            UsbBackend::classify_frame_write_error(&error),
            actor::FrameWriteDisposition::Transient
        );
    }

    let fatal_errors = [
        TransportError::NotFound {
            detail: "device removed".to_owned(),
        },
        TransportError::PermissionDenied {
            detail: "access revoked".to_owned(),
        },
        TransportError::Closed,
        TransportError::UnsupportedTransfer {
            transport: "test".to_owned(),
            transfer_type: TransferType::Primary,
        },
        TransportError::IoError {
            detail: "hidraw device disconnected".to_owned(),
        },
    ];
    for error in fatal_errors {
        let error = anyhow!(error).context("USB frame write failed");
        assert_eq!(
            UsbBackend::classify_frame_write_error(&error),
            actor::FrameWriteDisposition::Fatal
        );
    }

    assert_eq!(
        UsbBackend::classify_frame_write_error(&anyhow!("protocol encoding failed")),
        actor::FrameWriteDisposition::Fatal
    );
}

#[tokio::test]
async fn single_lane_actor_survives_transient_io_frame_failure() {
    assert_transient_frame_failure_survival(false, InjectedPrimaryFailure::Io).await;
}

#[tokio::test]
async fn parallel_actor_survives_transient_timeout_frame_failure() {
    assert_transient_frame_failure_survival(true, InjectedPrimaryFailure::Timeout).await;
}

async fn assert_transient_frame_failure_survival(
    parallel_transfer_lanes: bool,
    failure: InjectedPrimaryFailure,
) {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    let transport = RecordingTransport::default()
        .with_failed_primary_send_attempt(1, failure)
        .with_parallel_transfer_lanes_if(parallel_transfer_lanes);
    let transport = Arc::new(transport);
    let actor_protocol: Arc<dyn Protocol> = Arc::new(FairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport.clone();
    let device_id = DeviceId::new();

    let actor = if parallel_transfer_lanes {
        tokio::spawn(UsbBackend::test_run_parallel_device_actor(
            device_id,
            "transient-frame-failure-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ))
    } else {
        tokio::spawn(UsbBackend::test_run_device_actor(
            device_id,
            "transient-frame-failure-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ))
    };

    let first_id = DeviceDeliveryId {
        queue_generation: 7,
        sequence: 1,
    };
    let (first_frame, first_ack_rx) =
        UsbFramePayload::tracked(first_id, Arc::new(vec![[0x11, 0x22, 0x33]]));
    frame_tx.send_replace(Some(Arc::new(first_frame)));
    wait_for_primary_send_attempts(&transport, 1).await;
    assert!(transport.writes().is_empty());
    let first_ack = timeout(Duration::from_secs(1), first_ack_rx)
        .await
        .expect("failed transport acknowledgement should arrive")
        .expect("failed transport acknowledgement channel should stay open");
    assert_eq!(first_ack.id, first_id);
    assert_eq!(first_ack.status, DeviceDeliveryStatus::Failed);
    assert!(first_ack.transport_started);
    assert_eq!(first_ack.completed_payload_bytes, 0);

    let second_id = DeviceDeliveryId {
        queue_generation: 7,
        sequence: 2,
    };
    let (second_frame, second_ack_rx) =
        UsbFramePayload::tracked(second_id, Arc::new(vec![[0x22, 0x33, 0x44]]));
    frame_tx.send_replace(Some(Arc::new(second_frame)));
    assert_eq!(wait_for_writes(&transport, 1).await, vec![vec![0x22]]);
    let second_ack = timeout(Duration::from_secs(1), second_ack_rx)
        .await
        .expect("completed transport acknowledgement should arrive")
        .expect("completed transport acknowledgement channel should stay open");
    assert_eq!(second_ack.id, second_id);
    assert_eq!(second_ack.status, DeviceDeliveryStatus::Completed);
    assert!(second_ack.transport_started);
    assert_eq!(second_ack.completed_payload_bytes, 3);
    assert!(!actor.is_finished());

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should remain open after transient frame failure");
    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");
}

async fn wait_for_writes(transport: &RecordingTransport, count: usize) -> Vec<Vec<u8>> {
    timeout(Duration::from_secs(1), async {
        loop {
            let writes = transport.writes();
            if writes.len() >= count {
                return writes;
            }

            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("transport writes should arrive before timeout")
}

async fn wait_for_primary_send_attempts(transport: &RecordingTransport, count: usize) {
    timeout(Duration::from_secs(1), async {
        loop {
            if transport.primary_send_attempts() >= count {
                return;
            }

            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("transport send attempt should arrive before timeout");
}

struct FairnessProtocol;

impl Protocol for FairnessProtocol {
    fn name(&self) -> &'static str {
        "fairness-test"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        vec![test_command(colors.first().map_or(0x11, |color| color[0]))]
    }

    fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        Some(vec![test_command(
            jpeg_data.first().copied().unwrap_or(0xD1),
        )])
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::default()
    }

    fn total_leds(&self) -> u32 {
        1
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

struct ParallelFairnessProtocol;

impl Protocol for ParallelFairnessProtocol {
    fn name(&self) -> &'static str {
        "parallel-fairness-test"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        vec![test_command_with_transfer(
            colors.first().map_or(0x11, |color| color[0]),
            TransferType::Primary,
        )]
    }

    fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        Some(vec![test_command_with_transfer(
            jpeg_data.first().copied().unwrap_or(0xD1),
            TransferType::Bulk,
        )])
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::default()
    }

    fn total_leds(&self) -> u32 {
        1
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

#[derive(Clone, Copy)]
enum InjectedPrimaryFailure {
    Io,
    Timeout,
}

#[derive(Default)]
struct RecordingTransport {
    writes: Mutex<Vec<Vec<u8>>>,
    send_delay: Duration,
    primary_send_delay: Option<Duration>,
    bulk_send_delay: Option<Duration>,
    parallel_transfer_lanes: bool,
    failed_transfer_type: Option<TransferType>,
    primary_send_attempts: AtomicUsize,
    failed_primary_send_attempt: Option<usize>,
    failed_primary_send_error: Option<InjectedPrimaryFailure>,
}

impl RecordingTransport {
    fn with_send_delay(mut self, send_delay: Duration) -> Self {
        self.send_delay = send_delay;
        self
    }

    fn with_primary_send_delay(mut self, send_delay: Duration) -> Self {
        self.primary_send_delay = Some(send_delay);
        self
    }

    fn with_parallel_transfer_lanes(mut self) -> Self {
        self.parallel_transfer_lanes = true;
        self
    }

    const fn with_parallel_transfer_lanes_if(mut self, enabled: bool) -> Self {
        self.parallel_transfer_lanes = enabled;
        self
    }

    const fn with_failed_transfer_type(mut self, transfer_type: TransferType) -> Self {
        self.failed_transfer_type = Some(transfer_type);
        self
    }

    const fn with_failed_primary_send_attempt(
        mut self,
        attempt: usize,
        error: InjectedPrimaryFailure,
    ) -> Self {
        self.failed_primary_send_attempt = Some(attempt);
        self.failed_primary_send_error = Some(error);
        self
    }

    fn writes(&self) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .expect("recording transport mutex should not be poisoned")
            .clone()
    }

    fn primary_send_attempts(&self) -> usize {
        self.primary_send_attempts.load(Ordering::Relaxed)
    }

    async fn record_send(&self, data: &[u8], send_delay: Duration) {
        if !send_delay.is_zero() {
            tokio::time::sleep(send_delay).await;
        }
        self.writes
            .lock()
            .expect("recording transport mutex should not be poisoned")
            .push(data.to_vec());
    }

    fn send_delay_for(&self, transfer_type: TransferType) -> Duration {
        match transfer_type {
            TransferType::Primary => self.primary_send_delay.unwrap_or(self.send_delay),
            TransferType::Bulk => self.bulk_send_delay.unwrap_or(self.send_delay),
            TransferType::HidReport => self.send_delay,
        }
    }
}

#[async_trait]
impl Transport for RecordingTransport {
    fn name(&self) -> &'static str {
        "recording-test"
    }

    fn supports_parallel_transfer_lanes(&self) -> bool {
        self.parallel_transfer_lanes
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.record_send(data, self.send_delay).await;
        Ok(())
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        if self.failed_transfer_type == Some(transfer_type) {
            return Err(TransportError::IoError {
                detail: format!("injected {transfer_type:?} failure"),
            });
        }
        if transfer_type == TransferType::Primary {
            let attempt = self.primary_send_attempts.fetch_add(1, Ordering::Relaxed) + 1;
            if self.failed_primary_send_attempt == Some(attempt) {
                return Err(match self.failed_primary_send_error {
                    Some(InjectedPrimaryFailure::Io) => TransportError::IoError {
                        detail: format!("injected primary send failure on attempt {attempt}"),
                    },
                    Some(InjectedPrimaryFailure::Timeout) => {
                        TransportError::Timeout { timeout_ms: 25 }
                    }
                    None => TransportError::IoError {
                        detail: "injected primary send failure".to_owned(),
                    },
                });
            }
        }
        self.record_send(data, self.send_delay_for(transfer_type))
            .await;
        Ok(())
    }

    async fn receive(&self, _timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        Ok(Vec::new())
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

fn test_command(byte: u8) -> ProtocolCommand {
    test_command_with_transfer(byte, TransferType::Primary)
}

fn test_command_with_transfer(byte: u8, transfer_type: TransferType) -> ProtocolCommand {
    ProtocolCommand {
        data: vec![byte],
        expects_response: false,
        response_delay: Duration::ZERO,
        post_delay: Duration::ZERO,
        transfer_type,
    }
}
