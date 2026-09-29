use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use anyhow::anyhow;
use async_trait::async_trait;
use hypercolor_driver_api::DeviceDeliveryStatus;
use hypercolor_hal::display::DisplayEncodeError;
use hypercolor_hal::protocol::{
    ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus, TransferType,
};
use hypercolor_hal::registry::{TransportLifecycleHints, UsbTransportFuture, UsbTransportKind};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceOrigin, DeviceTopologyHint,
    DisplayFrameFormat, DisplayFramePayload, SegmentInfo,
};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::time::timeout;

use super::*;
use crate::device::BackendManager;

static USB_ACTOR_METRICS_TEST_LOCK: LazyLock<AsyncMutex<()>> =
    LazyLock::new(|| AsyncMutex::new(()));

fn unreachable_test_transport(_request: UsbTransportOpenRequest) -> UsbTransportFuture {
    Box::pin(async {
        Err(TransportError::IoError {
            detail: "test transport factory should not run".to_owned(),
        })
    })
}

fn background_driver_transport() -> TransportType {
    TransportType::DriverUsb {
        binding: UsbTransportBinding {
            id: "test/background-native",
            kind: UsbTransportKind::Midi,
            lifecycle: TransportLifecycleHints {
                connect_timeout: Some(Duration::from_secs(30)),
                connect_execution: TransportConnectExecution::Background,
                retry_on_connect_timeout: false,
            },
            open: unreachable_test_transport,
        },
    }
}

/// A 320x320 JPEG display segment, the shape the display gate keys on.
fn test_display_segment() -> SegmentInfo {
    SegmentInfo {
        name: "Display".to_owned(),
        led_count: 0,
        topology: DeviceTopologyHint::Display {
            width: 320,
            height: 320,
            circular: true,
            format: DisplayFrameFormat::Jpeg,
        },
        color_format: hypercolor_types::device::DeviceColorFormat::Rgb,
        layout_hint: None,
    }
}

fn temporary_control_test_device(supports_direct: bool, led_count: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "USB Test Strip".to_owned(),
        vendor: "Hypercolor".to_owned(),
        family: DeviceFamily::new_static("test", "Test"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", USB_OUTPUT_BACKEND_ID, ConnectionType::Usb),
        segments: if led_count == 0 {
            Vec::new()
        } else {
            vec![SegmentInfo {
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
    info.segments.clear();
    assert!(!backend.supports_temporary_direct_control(&info));
}

#[test]
fn driver_transport_lifecycle_policy_uses_binding_hints() {
    let policy = lifecycle_policy_for_transport(background_driver_transport());

    assert!(policy.connect_execution().is_background());
    assert_eq!(policy.connect_timeout(), Duration::from_secs(30));
    assert!(!policy.retry_on_connect_timeout());
}

#[tokio::test]
async fn driver_transport_policy_rejects_timeout_retry_when_declared() {
    let device_id = DeviceId::new();
    let transport = RecordingTransport::default()
        .with_failed_primary_send_attempt(1, InjectedPrimaryFailure::Timeout);
    let error = UsbBackend::run_commands(&FairnessProtocol, &transport, &[test_command(0x42)])
        .await
        .expect_err("injected transport timeout should fail the command");
    let error = map_hal_transport_error(
        device_id,
        USB_OUTPUT_BACKEND_ID,
        DeviceTransportOperation::Connect,
        &error,
    );
    let policy = lifecycle_policy_for_transport(background_driver_transport());

    assert!(matches!(
        &error,
        DeviceError::Timeout { after } if *after == Duration::from_millis(25)
    ));
    assert!(!policy.should_retry_connect_failure(&error));
}

#[test]
fn transport_not_found_recoverability_depends_on_operation() {
    use hypercolor_types::device::ErrorRecoverability;

    let device_id = DeviceId::new();
    let error = anyhow!(TransportError::NotFound {
        detail: "device removed".to_owned(),
    });

    let connect_error = map_hal_transport_error(
        device_id,
        USB_OUTPUT_BACKEND_ID,
        DeviceTransportOperation::Connect,
        &error,
    );
    assert!(matches!(connect_error, DeviceError::NotFound { .. }));
    assert_eq!(
        connect_error.recoverability(),
        ErrorRecoverability::Permanent
    );

    let write_error = map_hal_transport_error(
        device_id,
        USB_OUTPUT_BACKEND_ID,
        DeviceTransportOperation::Write,
        &error,
    );
    assert_eq!(
        write_error,
        DeviceError::Disconnected {
            device: device_id.to_string(),
        }
    );
    assert_eq!(write_error.recoverability(), ErrorRecoverability::Reconnect);

    let disconnect_error = map_hal_transport_error(
        device_id,
        USB_OUTPUT_BACKEND_ID,
        DeviceTransportOperation::Disconnect,
        &error,
    );
    assert_eq!(disconnect_error, write_error);
}

#[test]
fn transport_not_ready_is_retryable_during_connect() {
    use hypercolor_types::device::ErrorRecoverability;

    let device_id = DeviceId::new();
    let error = anyhow!(TransportError::NotReady {
        detail: "Windows MIDI endpoint is still enumerating".to_owned(),
    });

    let connect_error = map_hal_transport_error(
        device_id,
        USB_OUTPUT_BACKEND_ID,
        DeviceTransportOperation::Connect,
        &error,
    );

    assert!(matches!(
        connect_error,
        DeviceError::ConnectionFailed { .. }
    ));
    assert_eq!(
        connect_error.recoverability(),
        ErrorRecoverability::Reconnect
    );
    let policy = lifecycle_policy_for_transport(background_driver_transport());
    assert!(policy.should_retry_connect_failure(&connect_error));
}

#[test]
fn standard_usb_lifecycle_policy_uses_default_connect_behavior() {
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
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1])),
    )))));

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

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1])),
    )))));

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
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD2])),
    )))));

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
    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1])),
    )))));

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

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1])),
    )))));

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

    display_tx.send_replace(Some(Arc::new(UsbDisplayPayload::untracked(Arc::new(
        OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1])),
    )))));

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
        TransportError::UnsupportedPlatform {
            transport: "SMBus",
            platform: hypercolor_hal::transport::TransportPlatform::MacOs,
        },
        TransportError::UnsupportedTransfer {
            transport: "test".to_owned(),
            transfer_type: TransferType::Primary,
        },
        TransportError::Disconnected {
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

    let prose_only_disconnect = anyhow!(TransportError::IoError {
        detail: "hidraw device disconnected".to_owned(),
    })
    .context("USB frame write failed");
    assert_eq!(
        UsbBackend::classify_frame_write_error(&prose_only_disconnect),
        actor::FrameWriteDisposition::Transient
    );

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

#[tokio::test]
async fn actor_shutdown_rejects_pending_tracked_frame() {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let delivery_id = DeviceDeliveryId {
        queue_generation: 17,
        sequence: 3,
    };
    let (frame, delivery_rx) =
        UsbFramePayload::tracked(delivery_id, Arc::new(vec![[0x11, 0x22, 0x33]]));
    frame_tx.send_replace(Some(Arc::new(frame)));

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("shutdown command should queue before actor start");

    let actor = UsbBackend::spawn_device_actor(
        DeviceId::new(),
        "pending-shutdown-test-device",
        Arc::new(FairnessProtocol),
        Arc::new(RecordingTransport::default()),
        Arc::new(AtomicBool::new(true)),
        Arc::new(Mutex::new(())),
        frame_tx,
        frame_rx,
        display_tx,
        display_rx,
        command_rx,
        Arc::new(Mutex::new(None)),
    );

    response_rx
        .await
        .expect("shutdown response should arrive")
        .expect("shutdown should succeed");
    let ack = timeout(Duration::from_secs(1), delivery_rx)
        .await
        .expect("pending delivery should be rejected without hanging")
        .expect("delivery acknowledgement channel should stay open");
    assert_eq!(ack.id, delivery_id);
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(!ack.transport_started);
    actor.await.expect("actor wrapper should join");
}

#[tokio::test]
async fn fatal_control_exit_rejects_pending_tracked_frame() {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let delivery_id = DeviceDeliveryId {
        queue_generation: 23,
        sequence: 9,
    };
    let (frame, delivery_rx) =
        UsbFramePayload::tracked(delivery_id, Arc::new(vec![[0x44, 0x55, 0x66]]));
    frame_tx.send_replace(Some(Arc::new(frame)));

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::SetBrightness {
            brightness: 128,
            response_tx,
        })
        .expect("unsupported brightness command should queue before actor start");
    let last_async_error = Arc::new(Mutex::new(None));
    let actor = UsbBackend::spawn_device_actor(
        DeviceId::new(),
        "pending-control-failure-test-device",
        Arc::new(FairnessProtocol),
        Arc::new(RecordingTransport::default()),
        Arc::new(AtomicBool::new(true)),
        Arc::new(Mutex::new(())),
        frame_tx,
        frame_rx,
        display_tx,
        display_rx,
        command_rx,
        Arc::clone(&last_async_error),
    );

    let control_error = response_rx
        .await
        .expect("control response should arrive")
        .expect_err("unsupported brightness should fail the actor");
    assert!(matches!(
        &control_error,
        DeviceError::WriteError { detail, .. }
            if detail.contains("does not support brightness")
    ));
    let ack = timeout(Duration::from_secs(1), delivery_rx)
        .await
        .expect("pending delivery should be rejected without hanging")
        .expect("delivery acknowledgement channel should stay open");
    assert_eq!(ack.id, delivery_id);
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(!ack.transport_started);
    actor.await.expect("actor wrapper should join");
    assert!(
        last_async_error
            .lock()
            .expect("async error lock should remain available")
            .as_ref()
            .is_some_and(|error| error.to_string().contains("does not support brightness"))
    );
}

#[tokio::test]
async fn brightness_actor_response_preserves_timeout_duration() {
    let device_id = DeviceId::new();
    let protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let transport: Arc<dyn Transport> = Arc::new(
        RecordingTransport::default()
            .with_failed_primary_send_attempt(1, InjectedPrimaryFailure::Timeout),
    );
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicBool::new(true));
    let lifecycle_gate = Arc::new(Mutex::new(()));
    let last_async_error = Arc::new(Mutex::new(None));
    let actor_task = UsbBackend::spawn_device_actor(
        device_id,
        "brightness-timeout-test-device",
        Arc::clone(&protocol),
        transport,
        Arc::clone(&active),
        Arc::clone(&lifecycle_gate),
        frame_tx.clone(),
        frame_rx,
        display_tx.clone(),
        display_rx,
        command_rx,
        Arc::clone(&last_async_error),
    );
    let mut device = UsbDevice {
        protocol,
        transport_name: "recording-test",
        target_fps: None,
        resolved_led_count: 1,
        frame_tx,
        display_tx,
        command_tx,
        actor_task: Some(actor_task),
        active,
        lifecycle_gate,
        last_async_error,
        info_template: temporary_control_test_device(true, 1),
        frame_diagnostics_emitted: false,
        non_black_frame_diagnostics_emitted: false,
    };

    let error = device
        .set_brightness(device_id, 128)
        .await
        .expect_err("injected brightness timeout should cross the actor response");
    assert_eq!(
        error,
        DeviceError::Timeout {
            after: Duration::from_millis(25),
        }
    );
    if let Some(actor_task) = device.actor_task.take() {
        actor_task.await.expect("actor wrapper should join");
    }
}

#[tokio::test]
async fn stored_actor_disconnect_survives_duplicate_connect_and_output_paths() {
    let device_id = DeviceId::new();
    let backend = backend_with_stored_actor_error(
        device_id,
        DeviceError::Disconnected {
            device: device_id.to_string(),
        },
    );
    let expected = DeviceError::Disconnected {
        device: device_id.to_string(),
    };

    assert_eq!(
        backend
            .connect(&device_id)
            .await
            .expect_err("duplicate connect should report the actor failure"),
        expected
    );
    assert_eq!(
        backend
            .write_colors(&device_id, &[[1, 2, 3]])
            .await
            .expect_err("write should report the actor failure"),
        expected
    );
    assert_eq!(
        backend
            .write_display_payload_owned(
                &device_id,
                Arc::new(OwnedDisplayFramePayload::jpeg(1, 1, Arc::new(vec![0xFF]),)),
            )
            .await
            .expect_err("display write should report the actor failure"),
        expected
    );
    assert_eq!(
        backend
            .set_brightness(&device_id, 128)
            .await
            .expect_err("brightness should report the actor failure"),
        expected
    );
}

fn backend_with_stored_actor_error(device_id: DeviceId, error: DeviceError) -> UsbBackend {
    let backend = UsbBackend::new();
    let mut info = temporary_control_test_device(true, 1);
    info.id = device_id;
    info.segments.push(test_display_segment());
    info.sync_display_capabilities();
    info.capabilities.supports_brightness = true;
    let protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let (frame_tx, _frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, _display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicBool::new(false));
    let lifecycle_gate = Arc::new(Mutex::new(()));
    let last_async_error = Arc::new(Mutex::new(Some(error)));
    let device = UsbDevice {
        protocol: Arc::clone(&protocol),
        transport_name: "recording-test",
        target_fps: None,
        resolved_led_count: 1,
        frame_tx,
        display_tx,
        command_tx,
        actor_task: None,
        active,
        lifecycle_gate,
        last_async_error,
        info_template: info.clone(),
        frame_diagnostics_emitted: false,
        non_black_frame_diagnostics_emitted: false,
    };
    let frame_sink = device.frame_sink(device_id);
    let display_sink = Some(device.display_sink(device_id));
    backend
        .connected
        .write()
        .expect("connected device map should remain available")
        .insert(
            device_id,
            Arc::new(ConnectedUsbDevice {
                device: tokio::sync::Mutex::new(device),
                info_template: info,
                protocol,
                target_fps: None,
                frame_sink,
                display_sink,
            }),
        );
    backend
}

fn backend_with_display_actor(
    device_id: DeviceId,
    transport: Arc<RecordingTransport>,
) -> Arc<UsbBackend> {
    let backend = Arc::new(UsbBackend::new());
    let mut info = temporary_control_test_device(true, 1);
    info.id = device_id;
    info.segments.push(test_display_segment());
    info.sync_display_capabilities();
    let protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
    let actor_transport: Arc<dyn Transport> = transport;
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicBool::new(true));
    let lifecycle_gate = Arc::new(Mutex::new(()));
    let last_async_error = Arc::new(Mutex::new(None));
    let actor_task = UsbBackend::spawn_device_actor(
        device_id,
        "coordinator-display-test-device",
        Arc::clone(&protocol),
        actor_transport,
        Arc::clone(&active),
        Arc::clone(&lifecycle_gate),
        frame_tx.clone(),
        frame_rx,
        display_tx.clone(),
        display_rx,
        command_rx,
        Arc::clone(&last_async_error),
    );
    let device = UsbDevice {
        protocol: Arc::clone(&protocol),
        transport_name: "recording-test",
        target_fps: None,
        resolved_led_count: 1,
        frame_tx,
        display_tx,
        command_tx,
        actor_task: Some(actor_task),
        active,
        lifecycle_gate,
        last_async_error,
        info_template: info.clone(),
        frame_diagnostics_emitted: false,
        non_black_frame_diagnostics_emitted: false,
    };
    let frame_sink = device.frame_sink(device_id);
    let display_sink = Some(device.display_sink(device_id));
    backend
        .connected
        .write()
        .expect("connected device map should remain available")
        .insert(
            device_id,
            Arc::new(ConnectedUsbDevice {
                device: tokio::sync::Mutex::new(device),
                info_template: info,
                protocol,
                target_fps: None,
                frame_sink,
                display_sink,
            }),
        );
    backend
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coordinator_generation_gate_survives_cancelled_usb_display_waiter() {
    let device_id = DeviceId::new();
    let old_transport =
        Arc::new(RecordingTransport::default().with_bulk_send_delay(Duration::from_millis(200)));
    let new_transport = Arc::new(RecordingTransport::default());
    let old_backend = backend_with_display_actor(device_id, Arc::clone(&old_transport));
    let new_backend = backend_with_display_actor(device_id, Arc::clone(&new_transport));
    let mut manager = BackendManager::new();
    manager.register_backend(old_backend);
    let old_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("old USB display lane should exist");

    let old_write = tokio::spawn({
        let old_lane = old_lane.clone();
        async move {
            old_lane
                .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                    1,
                    1,
                    Arc::new(vec![0xD1]),
                )))
                .await
        }
    });
    wait_for_bulk_send_attempts(&old_transport, 1).await;

    manager.register_backend(new_backend.clone());
    let new_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("replacement USB display lane should exist");
    assert_ne!(old_lane.queue_generation(), new_lane.queue_generation());
    let new_write = tokio::spawn(async move {
        new_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD2]),
            )))
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        new_transport.bulk_send_attempts(),
        0,
        "replacement transport must wait for the retired generation's terminal ack"
    );
    old_write.abort();
    assert!(
        old_write
            .await
            .expect_err("old delivery waiter should be cancelled")
            .is_cancelled()
    );
    assert_eq!(
        wait_for_writes(&old_transport, 1).await,
        vec![vec![0xD1]],
        "old physical transport should finish after its waiter is cancelled"
    );
    timeout(Duration::from_secs(2), new_write)
        .await
        .expect("replacement USB delivery should finish")
        .expect("replacement USB delivery task should join")
        .expect("replacement USB transport should complete");
    assert_eq!(new_transport.writes(), vec![vec![0xD2]]);

    new_backend
        .disconnect(&device_id)
        .await
        .expect("replacement USB actor should shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_retains_old_usb_actor_through_terminal_delivery() {
    let device_id = DeviceId::new();
    let old_transport =
        Arc::new(RecordingTransport::default().with_bulk_send_delay(Duration::from_millis(200)));
    let new_transport = Arc::new(RecordingTransport::default());
    let old_backend = backend_with_display_actor(device_id, Arc::clone(&old_transport));
    let new_backend = backend_with_display_actor(device_id, Arc::clone(&new_transport));
    let mut manager = BackendManager::new();
    manager.register_backend(old_backend);
    let old_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("old USB display lane should exist");
    let old_write = tokio::spawn(async move {
        old_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD1]),
            )))
            .await
    });
    wait_for_bulk_send_attempts(&old_transport, 1).await;

    manager.register_backend(new_backend.clone());
    let new_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("replacement USB display lane should exist");
    let new_write = tokio::spawn(async move {
        new_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD2]),
            )))
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(new_transport.bulk_send_attempts(), 0);

    timeout(Duration::from_secs(1), old_write)
        .await
        .expect("old physical delivery should terminate")
        .expect("old delivery task should join")
        .expect("old delivery should retain its actual terminal success");
    timeout(Duration::from_secs(1), new_write)
        .await
        .expect("replacement delivery should follow old terminal completion")
        .expect("replacement delivery task should join")
        .expect("replacement delivery should complete");
    assert_eq!(old_transport.writes(), vec![vec![0xD1]]);
    assert_eq!(new_transport.writes(), vec![vec![0xD2]]);
    assert_eq!(
        manager.display_delivery_supervisor_statistics().in_flight,
        0
    );

    new_backend
        .disconnect(&device_id)
        .await
        .expect("replacement USB actor should shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_replaced_usb_failure_reaches_original_lifecycle_fence() {
    let device_id = DeviceId::new();
    let old_transport = Arc::new(
        RecordingTransport::default()
            .with_failed_transfer_type(TransferType::Bulk)
            .with_failed_transfer_delay(Duration::from_millis(200)),
    );
    let new_transport = Arc::new(RecordingTransport::default());
    let old_backend = backend_with_display_actor(device_id, Arc::clone(&old_transport));
    let new_backend = backend_with_display_actor(device_id, Arc::clone(&new_transport));
    let mut manager = BackendManager::new();
    manager.register_backend(old_backend);
    let old_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("old USB display lane should exist");
    let old_generation = old_lane.queue_generation();

    let old_write = tokio::spawn({
        let old_lane = old_lane.clone();
        async move {
            old_lane
                .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                    1,
                    1,
                    Arc::new(vec![0xD1]),
                )))
                .await
        }
    });
    wait_for_bulk_send_attempts(&old_transport, 1).await;

    manager.register_backend(new_backend.clone());
    let new_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("replacement USB display lane should exist");
    let new_write = tokio::spawn(async move {
        new_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD2]),
            )))
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        new_transport.bulk_send_attempts(),
        0,
        "replacement transport must wait for the old physical failure"
    );
    old_write.abort();
    assert!(
        old_write
            .await
            .expect_err("old delivery waiter should be cancelled")
            .is_cancelled()
    );

    let failure = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(failure) = manager.async_write_failures().into_iter().next() {
                return failure;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("late old-generation failure should reach the manager authority");
    assert_eq!(failure.delivery_id.queue_generation, old_generation);
    assert_eq!(failure.delivery_id.sequence, 1);
    assert!(matches!(failure.error, DeviceError::WriteError { .. }));
    assert!(
        failure.try_acknowledge(),
        "daemon lifecycle fencing must be able to claim the late failure"
    );

    timeout(Duration::from_secs(2), new_write)
        .await
        .expect("replacement USB delivery should finish")
        .expect("replacement USB delivery task should join")
        .expect("replacement USB transport should complete");
    assert_eq!(new_transport.writes(), vec![vec![0xD2]]);
    assert!(manager.async_write_failures().is_empty());

    new_backend
        .disconnect(&device_id)
        .await
        .expect("replacement USB actor should shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_replacement_does_not_accumulate_cancelled_display_transactions() {
    let device_id = DeviceId::new();
    let release_old_transport = Arc::new(Notify::new());
    let old_transport = Arc::new(
        RecordingTransport::default().with_bulk_send_release(Arc::clone(&release_old_transport)),
    );
    let old_backend = backend_with_display_actor(device_id, Arc::clone(&old_transport));
    let mut manager = BackendManager::new();
    manager.register_backend(old_backend);
    let old_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("old USB display lane should exist");
    let old_write = tokio::spawn(async move {
        old_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD1]),
            )))
            .await
    });
    wait_for_bulk_send_attempts(&old_transport, 1).await;
    old_write.abort();
    assert!(
        old_write
            .await
            .expect_err("old delivery waiter should be cancelled")
            .is_cancelled()
    );

    for payload_byte in 0xD2..=0xDD {
        let transport = Arc::new(RecordingTransport::default());
        let backend = backend_with_display_actor(device_id, transport);
        manager.register_backend(backend);
        let lane = manager
            .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
            .expect("replacement USB display lane should exist");
        let waiter = tokio::spawn(async move {
            lane.write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![payload_byte]),
            )))
            .await
        });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;

        let supervision = manager.display_delivery_supervisor_statistics();
        assert_eq!(supervision.in_flight, 1);
        assert_eq!(
            supervision.retained_generations, 2,
            "only the physical delivery and current generation should remain owned"
        );
    }

    release_old_transport.notify_waiters();
    assert_eq!(wait_for_writes(&old_transport, 1).await, vec![vec![0xD1]]);
    timeout(Duration::from_secs(1), async {
        loop {
            let supervision = manager.display_delivery_supervisor_statistics();
            if supervision.in_flight == 0 && supervision.retained_generations == 1 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal completion should drain the retired generation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actor_panic_terminates_cancelled_retired_display_delivery() {
    let device_id = DeviceId::new();
    let release_old_transport = Arc::new(Notify::new());
    let old_transport = Arc::new(
        RecordingTransport::default()
            .with_bulk_send_release(Arc::clone(&release_old_transport))
            .with_parallel_transfer_lanes()
            .with_panicked_transfer_type(TransferType::Bulk),
    );
    let new_transport = Arc::new(RecordingTransport::default());
    let old_backend = backend_with_display_actor(device_id, Arc::clone(&old_transport));
    let new_backend = backend_with_display_actor(device_id, Arc::clone(&new_transport));
    let mut manager = BackendManager::new();
    manager.register_backend(old_backend);
    let old_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("old USB display lane should exist");
    let old_write = tokio::spawn(async move {
        old_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD1]),
            )))
            .await
    });
    wait_for_bulk_send_attempts(&old_transport, 1).await;

    manager.register_backend(new_backend.clone());
    let new_lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("replacement USB display lane should exist");
    let new_write = tokio::spawn(async move {
        new_lane
            .write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD2]),
            )))
            .await
    });
    old_write.abort();
    assert!(
        old_write
            .await
            .expect_err("old delivery waiter should be cancelled")
            .is_cancelled()
    );
    release_old_transport.notify_waiters();

    let failure = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(failure) = manager.async_write_failures().into_iter().next() {
                return failure;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor panic should terminate the retired delivery through supervision");
    assert!(failure.is_from_retired_generation());
    assert!(failure.try_acknowledge());
    timeout(Duration::from_secs(1), new_write)
        .await
        .expect("replacement delivery should be released after actor cleanup")
        .expect("replacement delivery task should join")
        .expect("replacement delivery should complete");
    assert_eq!(new_transport.writes(), vec![vec![0xD2]]);
    assert!(manager.async_write_failures().is_empty());
    assert_eq!(
        manager.display_delivery_supervisor_statistics().in_flight,
        0
    );

    new_backend
        .disconnect(&device_id)
        .await
        .expect("replacement USB actor should shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hung_display_transport_has_bounded_actor_shutdown() {
    let device_id = DeviceId::new();
    let transport_release = Arc::new(Notify::new());
    let transport = Arc::new(
        RecordingTransport::default().with_bulk_send_release(Arc::clone(&transport_release)),
    );
    let backend = backend_with_display_actor(device_id, Arc::clone(&transport));
    let mut manager = BackendManager::new();
    manager.register_backend(backend.clone());
    let lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("USB display lane should exist");
    let write = tokio::spawn(async move {
        lane.write(Arc::new(OwnedDisplayFramePayload::jpeg(
            1,
            1,
            Arc::new(vec![0xD1]),
        )))
        .await
    });
    wait_for_bulk_send_attempts(&transport, 1).await;

    let shutdown_error = timeout(
        USB_ACTOR_SHUTDOWN_TIMEOUT + Duration::from_secs(1),
        backend.disconnect(&device_id),
    )
    .await
    .expect("hung USB actor shutdown should remain bounded")
    .expect_err("hung USB actor shutdown should report its timeout");
    assert!(matches!(
        shutdown_error,
        DeviceError::Timeout { after } if after == USB_ACTOR_SHUTDOWN_TIMEOUT
    ));
    let write_error = timeout(Duration::from_secs(1), write)
        .await
        .expect("hung display delivery should terminate during bounded shutdown")
        .expect("display delivery task should join")
        .expect_err("display delivery should report shutdown timeout");
    assert!(matches!(
        write_error,
        DeviceError::Timeout { after } if after == USB_ACTOR_SHUTDOWN_TIMEOUT
    ));
    let failure = manager
        .async_write_failures()
        .into_iter()
        .next()
        .expect("bounded shutdown failure should reach lifecycle fencing");
    assert!(failure.try_acknowledge());
    assert_eq!(
        manager.display_delivery_supervisor_statistics().in_flight,
        0
    );
}

#[tokio::test]
async fn cancelled_shutdown_aborts_the_taken_actor_task() {
    let device_id = DeviceId::new();
    let command_received = Arc::new(Notify::new());
    let actor_dropped = Arc::new(AtomicBool::new(false));
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let actor_task = tokio::spawn({
        let command_received = Arc::clone(&command_received);
        let actor_dropped = Arc::clone(&actor_dropped);
        async move {
            let _drop_signal = ActorDropSignal(actor_dropped);
            let Some(UsbDeviceCommand::Shutdown { response_tx, .. }) = command_rx.recv().await
            else {
                panic!("shutdown command should reach the actor");
            };
            let _response_tx = response_tx;
            command_received.notify_one();
            std::future::pending::<()>().await;
        }
    });
    let mut device = usb_device_with_actor_task(device_id, command_tx, actor_task);
    let shutdown_task = tokio::spawn(async move { device.shutdown(device_id).await });

    timeout(Duration::from_secs(1), command_received.notified())
        .await
        .expect("shutdown should take the actor handle and send its command");
    shutdown_task.abort();
    assert!(
        shutdown_task
            .await
            .expect_err("shutdown caller should be cancelled")
            .is_cancelled()
    );
    wait_for_actor_drop(&actor_dropped).await;
}

#[tokio::test]
async fn shutdown_response_loss_still_joins_a_failing_actor() {
    let device_id = DeviceId::new();
    let response_dropped = Arc::new(Notify::new());
    let actor_release = Arc::new(Notify::new());
    let actor_dropped = Arc::new(AtomicBool::new(false));
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let actor_task = tokio::spawn({
        let response_dropped = Arc::clone(&response_dropped);
        let actor_release = Arc::clone(&actor_release);
        let actor_dropped = Arc::clone(&actor_dropped);
        async move {
            let _drop_signal = ActorDropSignal(actor_dropped);
            let Some(UsbDeviceCommand::Shutdown { response_tx, .. }) = command_rx.recv().await
            else {
                panic!("shutdown command should reach the actor");
            };
            drop(response_tx);
            response_dropped.notify_one();
            actor_release.notified().await;
            panic!("injected actor failure after shutdown response loss");
        }
    });
    let mut device = usb_device_with_actor_task(device_id, command_tx, actor_task);
    let mut shutdown_task = tokio::spawn(async move { (device.shutdown(device_id).await, device) });

    timeout(Duration::from_secs(1), response_dropped.notified())
        .await
        .expect("actor should drop the shutdown response channel");
    assert!(
        timeout(Duration::from_millis(50), &mut shutdown_task)
            .await
            .is_err(),
        "response loss must not return before actor cleanup"
    );

    actor_release.notify_one();
    let (shutdown_result, device) = timeout(Duration::from_secs(1), shutdown_task)
        .await
        .expect("shutdown should finish after the failing actor terminates")
        .expect("shutdown task should join");
    assert_eq!(
        shutdown_result,
        Err(DeviceError::Disconnected {
            device: device_id.to_string(),
        })
    );
    assert!(device.actor_task.is_none());
    assert!(actor_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn usb_display_transport_failure_keeps_its_queue_generation() {
    let device_id = DeviceId::new();
    let transport =
        Arc::new(RecordingTransport::default().with_failed_transfer_type(TransferType::Bulk));
    let backend = backend_with_display_actor(device_id, transport);
    let mut manager = BackendManager::new();
    manager.register_backend(backend.clone());
    let lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("USB display lane should exist");
    let queue_generation = lane.queue_generation();

    let error = lane
        .write(Arc::new(OwnedDisplayFramePayload::jpeg(
            1,
            1,
            Arc::new(vec![0xD1]),
        )))
        .await
        .expect_err("injected USB transport failure should reach the coordinator");
    assert!(matches!(error, DeviceError::WriteError { .. }));
    let statistics = lane.statistics();
    assert_eq!(statistics.transport_started, 1);
    assert_eq!(statistics.transport_completed, 0);
    assert_eq!(statistics.transport_failed, 1);
    let failures = manager.async_write_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].delivery_id.queue_generation, queue_generation);
    assert_eq!(failures[0].delivery_id.sequence, 1);
    assert!(failures[0].is_current());

    backend
        .disconnect(&device_id)
        .await
        .expect("USB actor should shut down");
}

#[tokio::test]
async fn cancelled_usb_display_waiter_keeps_terminal_failure_generation() {
    let device_id = DeviceId::new();
    let transport = Arc::new(
        RecordingTransport::default()
            .with_failed_transfer_type(TransferType::Bulk)
            .with_failed_transfer_delay(Duration::from_millis(200)),
    );
    let backend = backend_with_display_actor(device_id, Arc::clone(&transport));
    let mut manager = BackendManager::new();
    manager.register_backend(backend.clone());
    let lane = manager
        .display_output_lane(USB_OUTPUT_BACKEND_ID, device_id)
        .expect("USB display lane should exist");
    let queue_generation = lane.queue_generation();

    let delivery = tokio::spawn({
        let lane = lane.clone();
        async move {
            lane.write(Arc::new(OwnedDisplayFramePayload::jpeg(
                1,
                1,
                Arc::new(vec![0xD1]),
            )))
            .await
        }
    });
    wait_for_bulk_send_attempts(&transport, 1).await;
    delivery.abort();
    assert!(
        delivery
            .await
            .expect_err("display delivery waiter should be cancelled")
            .is_cancelled()
    );

    let failure = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(failure) = manager.async_write_failures().into_iter().next() {
                return failure;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal USB failure should reach the coordinator");
    assert_eq!(failure.delivery_id.queue_generation, queue_generation);
    assert_eq!(failure.delivery_id.sequence, 1);
    assert!(failure.is_current());
    assert!(matches!(failure.error, DeviceError::WriteError { .. }));
    let statistics = lane.statistics();
    assert_eq!(statistics.transport_started, 1);
    assert_eq!(statistics.transport_completed, 0);
    assert_eq!(statistics.transport_failed, 1);

    backend
        .disconnect(&device_id)
        .await
        .expect("USB actor should shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_gate_prevents_post_cleanup_tracked_publication() {
    let (frame_tx, _frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let active = Arc::new(AtomicBool::new(true));
    let lifecycle_gate = Arc::new(Mutex::new(()));
    let device_id = DeviceId::new();
    let sink = UsbFrameSink {
        device_id,
        frame_tx: frame_tx.clone(),
        active: Arc::clone(&active),
        lifecycle_gate: Arc::clone(&lifecycle_gate),
        last_async_error: Arc::new(Mutex::new(None)),
    };
    let delivery_id = DeviceDeliveryId {
        queue_generation: 31,
        sequence: 4,
    };

    let gate = lock_lifecycle_gate(&lifecycle_gate);
    let delivery = tokio::spawn(async move {
        sink.deliver_colors_shared(delivery_id, Arc::new(vec![[1, 2, 3]]))
            .await
    });
    active.store(false, Ordering::Release);
    if let Some(pending) = frame_tx.send_replace(None) {
        pending.reject_pending(DeviceError::Disconnected {
            device: device_id.to_string(),
        });
    }
    drop(gate);

    let ack = timeout(Duration::from_secs(1), delivery)
        .await
        .expect("publication blocked across cleanup should not hang")
        .expect("delivery task should join");
    assert_eq!(ack.id, delivery_id);
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(!ack.transport_started);
    assert!(frame_tx.borrow().is_none());
}

struct ActorDropSignal(Arc<AtomicBool>);

impl Drop for ActorDropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn usb_device_with_actor_task(
    device_id: DeviceId,
    command_tx: mpsc::UnboundedSender<UsbDeviceCommand>,
    actor_task: JoinHandle<()>,
) -> UsbDevice {
    let (frame_tx, _frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (display_tx, _display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let mut info = temporary_control_test_device(true, 1);
    info.id = device_id;

    UsbDevice {
        protocol: Arc::new(FairnessProtocol),
        transport_name: "shutdown-test",
        target_fps: None,
        resolved_led_count: 1,
        frame_tx,
        display_tx,
        command_tx,
        actor_task: Some(actor_task),
        active: Arc::new(AtomicBool::new(true)),
        lifecycle_gate: Arc::new(Mutex::new(())),
        last_async_error: Arc::new(Mutex::new(None)),
        info_template: info,
        frame_diagnostics_emitted: false,
        non_black_frame_diagnostics_emitted: false,
    }
}

async fn wait_for_actor_drop(actor_dropped: &AtomicBool) {
    timeout(Duration::from_secs(1), async {
        while !actor_dropped.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled shutdown should abort and drop the actor task");
}

/// An L-Wireless pair whose TX stopped taking commands: every TX write
/// runs out its budget, while the RX still takes the partner reset.
#[derive(Default)]
struct StalledWirelessTx {
    sends: Mutex<Vec<(TransferType, Vec<u8>)>>,
}

#[async_trait]
impl Transport for StalledWirelessTx {
    fn name(&self) -> &'static str {
        "stalled L-Wireless pair"
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        self.sends
            .lock()
            .expect("send log lock should not be poisoned")
            .push((transfer_type, data.to_vec()));
        if transfer_type == TransferType::Companion {
            Ok(())
        } else {
            Err(TransportError::Timeout { timeout_ms: 1000 })
        }
    }

    async fn receive(&self, _timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        Err(TransportError::Timeout { timeout_ms: 20 })
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

/// Handle the test keeps while the watcher owns the pair.
struct SharedStalledWirelessTx(Arc<StalledWirelessTx>);

#[async_trait]
impl Transport for SharedStalledWirelessTx {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.0.send(data).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        self.0.send_with_type(data, transfer_type).await
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        self.0.receive(timeout).await
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        self.0.close().await
    }
}

/// A TX that stops taking commands mid-stream used to read as a transient
/// frame timeout: the actor wrote into the dead endpoint frame after frame
/// until upkeep failed. The controller's transport now names the wedge,
/// resets the TX through the RX, and the actor stops on the first stall.
#[tokio::test]
async fn a_wedged_lianli_tx_ends_the_actor_and_is_reset_through_the_rx() {
    use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;
    use hypercolor_hal::drivers::lianli::wireless::transport::{
        WirelessControllerTransport, partner_reset_packet,
    };

    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let pair = Arc::new(StalledWirelessTx::default());
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedStalledWirelessTx(Arc::clone(&pair))),
        "usb-backend-test-wedged-tx",
    ));
    let protocol: Arc<dyn Protocol> = Arc::new(WirelessControllerProtocol::new());

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "wedged-lianli-tx-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let frame_id = DeviceDeliveryId {
        queue_generation: 3,
        sequence: 1,
    };
    let (frame, ack_rx) = UsbFramePayload::tracked(frame_id, Arc::new(vec![[0x40, 0x10, 0x80]]));
    frame_tx.send_replace(Some(Arc::new(frame)));

    let ack = timeout(Duration::from_secs(1), ack_rx)
        .await
        .expect("the stalled frame is acknowledged")
        .expect("acknowledgement channel stays open");
    assert_eq!(ack.status, DeviceDeliveryStatus::Failed);
    assert!(
        matches!(ack.error, Some(DeviceError::Disconnected { .. })),
        "a wedge is a disconnect, not a retryable timeout: {:?}",
        ack.error
    );

    let result = timeout(Duration::from_secs(1), actor)
        .await
        .expect("the actor stops on the first stall instead of writing on")
        .expect("actor task joins");
    assert!(result.is_err(), "the session ends with the wedge");

    let sends = pair
        .sends
        .lock()
        .expect("send log lock should not be poisoned")
        .clone();
    assert_eq!(
        sends.len(),
        2,
        "one stalled TX write, then the reset: {sends:?}"
    );
    assert_eq!(sends[0].0, TransferType::Primary);
    assert_eq!(sends[1], (TransferType::Companion, partner_reset_packet()));
}

/// L-Wireless stand-in for actor pacing tests. The TX rebuilds each RGB
/// transfer from its envelope slices and decodes its pixels; the fan
/// cluster takes it `apply_delay` after its last envelope arrives, unless
/// the radio is dead; the RX answers a table poll with the tag the cluster
/// last took. Time is tokio's, so a paused runtime drives it.
struct EchoingWirelessPair {
    master: [u8; 6],
    apply_delay: Duration,
    state: Mutex<EchoingWirelessState>,
}

#[derive(Default)]
struct EchoingWirelessState {
    rf_dead: bool,
    /// Fail the next TX write with an I/O error, once.
    fail_next_tx: bool,
    envelope: Vec<u8>,
    transfer: Option<EchoingTransfer>,
    pending: std::collections::VecDeque<(tokio::time::Instant, [u8; 4], [u8; 4])>,
    applied: [u8; 4],
    /// Pixel hash of every transfer the fans took.
    shown: Vec<[u8; 4]>,
    rx_replies: std::collections::VecDeque<Vec<u8>>,
    /// The cluster's clock, advanced by every report the RX hands over.
    clock: u32,
    /// Every transfer the TX took whole: when, and its pixel hash.
    transfers: Vec<(tokio::time::Instant, [u8; 4])>,
    sends: Vec<(TransferType, Vec<u8>)>,
}

/// A transfer the stand-in TX is rebuilding.
struct EchoingTransfer {
    tag: [u8; 4],
    envelopes: u8,
    compressed_len: usize,
    leds: usize,
    data: Vec<u8>,
}

/// The stand-in's fan cluster: a radio MAC of its own per controller, since
/// the TX reset budget is kept per cluster for the whole process.
const fn echoing_cluster(master: [u8; 6]) -> [u8; 6] {
    [0x11, 0x11, 0x11, 0x11, master[4], master[5]]
}
const ECHOING_LEDS: usize = 78;

impl EchoingWirelessPair {
    fn new(master: [u8; 6], apply_delay: Duration) -> Self {
        Self {
            master,
            apply_delay,
            state: Mutex::new(EchoingWirelessState {
                applied: [0xF0, 0, 0, 1],
                ..EchoingWirelessState::default()
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, EchoingWirelessState> {
        self.state
            .lock()
            .expect("echoing pair lock should not be poisoned")
    }

    fn table(master: [u8; 6], state: &mut EchoingWirelessState, pages: u8) -> Vec<u8> {
        let now = tokio::time::Instant::now();
        while let Some((at, tag, content)) = state.pending.front().copied() {
            if at > now {
                break;
            }
            state.applied = tag;
            state.shown.push(content);
            state.pending.pop_front();
        }
        let mut reply = vec![0_u8; 448 * usize::from(pages.max(1))];
        reply[..4].copy_from_slice(&[0x10, 1, 0x80, 0]);
        let record = &mut reply[4..46];
        record[0..6].copy_from_slice(&echoing_cluster(master));
        record[6..12].copy_from_slice(&master);
        record[12] = 8;
        record[13] = 3;
        state.clock = state.clock.wrapping_add(1);
        record[14..18].copy_from_slice(&state.clock.to_be_bytes());
        record[19] = 3;
        record[20..24].copy_from_slice(&state.applied);
        record[24..27].copy_from_slice(&[28, 28, 28]);
        record[36..39].copy_from_slice(&[128, 128, 128]);
        record[41] = 0x1C;
        reply
    }

    fn take(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        let mut state = self.state();
        if transfer_type == TransferType::Companion {
            state.sends.push((transfer_type, data.to_vec()));
            if data[0] == 0x10 && data[2..4] != [0x04, 0x30] {
                let reply = Self::table(self.master, &mut state, data[1]);
                state.rx_replies.push_back(reply);
            }
            return Ok(());
        }
        if std::mem::take(&mut state.fail_next_tx) {
            return Err(TransportError::IoError {
                detail: "injected TX write fault".to_owned(),
            });
        }
        state.sends.push((transfer_type, data.to_vec()));
        if data[0] != 0x10 {
            return Ok(());
        }
        if data[1] == 0 {
            state.envelope.clear();
        }
        state.envelope.extend_from_slice(&data[4..]);
        if data[1] != 3 || state.envelope.len() != 240 {
            return Ok(());
        }
        let envelope = std::mem::take(&mut state.envelope);
        if envelope[0] != 0x12 || envelope[1] != 0x20 {
            return Ok(());
        }
        let mut tag = [0_u8; 4];
        tag.copy_from_slice(&envelope[14..18]);
        let (index, total) = (envelope[18], envelope[19]);
        if index == 0 {
            let length =
                u32::from_be_bytes([envelope[20], envelope[21], envelope[22], envelope[23]]);
            state.transfer = Some(EchoingTransfer {
                tag,
                envelopes: total,
                compressed_len: usize::try_from(length).expect("length fits"),
                leds: usize::from(envelope[27]),
                data: Vec::new(),
            });
            return Ok(());
        }
        let Some(mut transfer) = state.transfer.take() else {
            return Ok(());
        };
        if transfer.tag != tag {
            return Ok(());
        }
        transfer.data.extend_from_slice(&envelope[20..240]);
        if index + 1 < transfer.envelopes {
            state.transfer = Some(transfer);
            return Ok(());
        }
        let EchoingTransfer {
            compressed_len: length,
            leds,
            mut data,
            ..
        } = transfer;
        data.truncate(length);
        let raw = hypercolor_hal::drivers::lianli::wireless::tinyuz::decompress(&data, leds * 3)
            .expect("a transfer decodes");
        let content = hypercolor_hal::drivers::lianli::wireless::frame::effect_index_for(&raw);
        let now = tokio::time::Instant::now();
        state.transfers.push((now, content));
        if !state.rf_dead {
            state
                .pending
                .push_back((now + self.apply_delay, tag, content));
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for EchoingWirelessPair {
    fn name(&self) -> &'static str {
        "echoing L-Wireless pair"
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        self.take(data, transfer_type)
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        let reply = (transfer_type == TransferType::Companion)
            .then(|| self.state().rx_replies.pop_front())
            .flatten();
        reply.ok_or(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.send_with_type(data, transfer_type).await?;
        self.receive_with_type(timeout, transfer_type).await
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

/// Handle the test keeps while the watcher owns the pair.
struct SharedEchoingWirelessPair(Arc<EchoingWirelessPair>);

#[async_trait]
impl Transport for SharedEchoingWirelessPair {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.0.send(data).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        self.0.send_with_type(data, transfer_type).await
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        self.0.receive(timeout).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.0.receive_with_type(timeout, transfer_type).await
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.0
            .send_receive_with_type(data, timeout, transfer_type)
            .await
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        self.0.close().await
    }
}

/// The wireless protocol on tokio's clock, already past discovery: it knows
/// the controller `master` and one three-fan cluster.
fn paced_wireless_protocol(
    master: [u8; 6],
) -> Arc<hypercolor_hal::drivers::lianli::WirelessControllerProtocol> {
    use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;

    let protocol =
        WirelessControllerProtocol::with_clock(Arc::new(|| tokio::time::Instant::now().into_std()));
    let mut reply = vec![0_u8; 64];
    reply[0] = 0x11;
    reply[1..7].copy_from_slice(&master);
    reply[7..13].copy_from_slice(&[0, 0x0A, 0xE7, 0x4B, 0x00, 0x10]);
    protocol.parse_response(&reply).expect("MAC reply parses");
    let mut state = EchoingWirelessState {
        applied: [0xF0, 0, 0, 1],
        ..EchoingWirelessState::default()
    };
    protocol
        .parse_response(&EchoingWirelessPair::table(master, &mut state, 2))
        .expect("table parses");
    Arc::new(protocol)
}

fn wireless_frame(index: u8) -> Arc<UsbFramePayload> {
    Arc::new(UsbFramePayload::untracked(Arc::new(vec![
        [
            index, 0x20, 0x40
        ];
        ECHOING_LEDS
    ])))
}

/// Pixel hash of `wireless_frame(index)`.
fn wireless_content(index: u8) -> [u8; 4] {
    let raw: Vec<u8> = [index, 0x20, 0x40].repeat(ECHOING_LEDS);
    hypercolor_hal::drivers::lianli::wireless::frame::effect_index_for(&raw)
}

/// A frame that arrives while the window is full is held, and the frame
/// pump sends it as soon as an echo frees room, with no later frame to
/// carry it.
#[tokio::test(start_paused = true)]
async fn the_lianli_frame_pump_releases_a_frame_held_for_an_echo() {
    use hypercolor_hal::drivers::lianli::wireless::transport::WirelessControllerTransport;

    let master = [0xA0, 0x71, 0xAE, 0x72, 0xC0, 0x01];
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let pair = Arc::new(EchoingWirelessPair::new(master, Duration::from_millis(40)));
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedEchoingWirelessPair(Arc::clone(&pair))),
        "usb-backend-test-lianli-held-frame",
    ));
    let protocol: Arc<dyn Protocol> = paced_wireless_protocol(master);

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "lianli-held-frame-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    // Two frames fill the initial window of two; the third is held.
    for frame in 1..=3 {
        frame_tx.send_replace(Some(wireless_frame(frame)));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(900)).await;

    {
        let state = pair.state();
        let contents: Vec<[u8; 4]> = state
            .transfers
            .iter()
            .map(|(_, content)| *content)
            .collect();
        assert_eq!(
            &contents[..3],
            &[
                wireless_content(1),
                wireless_content(2),
                wireless_content(3)
            ],
            "the third frame went out once, after an echo freed room"
        );
        let held_for = state.transfers[2].0 - state.transfers[1].0;
        assert!(
            held_for >= Duration::from_millis(40),
            "the third frame waited for an echo: {held_for:?}"
        );
        assert_eq!(
            state.shown.last(),
            Some(&wireless_content(3)),
            "the fans show the newest frame"
        );
    }

    drop(frame_tx);
    let result = timeout(Duration::from_secs(1), actor)
        .await
        .expect("the actor stops once its frame source is gone")
        .expect("actor task joins");
    assert!(result.is_ok(), "{result:?}");
}

/// Delivery acknowledgements report what the transport did: a frame the
/// protocol writes is completed, a frame it holds while the window is full
/// is suppressed, not reported as delivered.
#[tokio::test(start_paused = true)]
async fn a_lianli_frame_held_for_an_echo_is_acknowledged_as_suppressed() {
    use hypercolor_hal::drivers::lianli::wireless::transport::WirelessControllerTransport;

    let master = [0xA0, 0x71, 0xAE, 0x72, 0xC0, 0x04];
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let pair = Arc::new(EchoingWirelessPair::new(master, Duration::from_millis(40)));
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedEchoingWirelessPair(Arc::clone(&pair))),
        "usb-backend-test-lianli-held-ack",
    ));
    let protocol: Arc<dyn Protocol> = paced_wireless_protocol(master);

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "lianli-held-ack-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let colors = |index: u8| Arc::new(vec![[index, 0x20, 0x40]; ECHOING_LEDS]);
    // The initial window of two takes two frames, each written whole.
    for sequence in 1..=2_u8 {
        let id = DeviceDeliveryId {
            queue_generation: 9,
            sequence: u64::from(sequence),
        };
        let (frame, ack) = UsbFramePayload::tracked(id, colors(sequence));
        frame_tx.send_replace(Some(Arc::new(frame)));
        let ack = timeout(Duration::from_secs(1), ack)
            .await
            .expect("the frame is acknowledged")
            .expect("acknowledgement channel stays open");
        assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
        assert!(ack.transport_started);
        assert_eq!(
            ack.completed_payload_bytes,
            u64::try_from(ECHOING_LEDS * 3).expect("small")
        );
    }

    let held_id = DeviceDeliveryId {
        queue_generation: 9,
        sequence: 3,
    };
    let (held, held_ack) = UsbFramePayload::tracked(held_id, colors(3));
    frame_tx.send_replace(Some(Arc::new(held)));
    let held_ack = timeout(Duration::from_secs(1), held_ack)
        .await
        .expect("the held frame is acknowledged")
        .expect("acknowledgement channel stays open");
    assert_eq!(
        held_ack.status,
        DeviceDeliveryStatus::SuppressedCadence,
        "the window was full; nothing was written for the frame"
    );
    assert!(!held_ack.transport_started);
    assert_eq!(held_ack.completed_payload_bytes, 0);

    drop(frame_tx);
    let result = timeout(Duration::from_secs(1), actor)
        .await
        .expect("the actor stops once its frame source is gone")
        .expect("actor task joins");
    assert!(result.is_ok(), "{result:?}");
}

/// A frame the pump releases is frame traffic: a transient write fault on
/// it is logged and paced again, the way a render frame's would be, and
/// never ends the session the way a failed keepalive does.
#[tokio::test(start_paused = true)]
async fn a_transient_fault_on_a_pumped_lianli_frame_keeps_the_actor_alive() {
    use hypercolor_hal::drivers::lianli::wireless::transport::WirelessControllerTransport;

    let master = [0xA0, 0x71, 0xAE, 0x72, 0xC0, 0x02];
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let pair = Arc::new(EchoingWirelessPair::new(master, Duration::from_millis(40)));
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedEchoingWirelessPair(Arc::clone(&pair))),
        "usb-backend-test-lianli-pump-fault",
    ));
    let protocol: Arc<dyn Protocol> = paced_wireless_protocol(master);

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "lianli-pump-fault-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    // Two frames fill the initial window of two.
    for frame in 1..=2 {
        frame_tx.send_replace(Some(wireless_frame(frame)));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The third frame is held, so the next TX write is the pump releasing
    // it once an echo frees room.
    pair.state().fail_next_tx = true;
    frame_tx.send_replace(Some(wireless_frame(3)));
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(
        !actor.is_finished(),
        "a transient fault does not end the session"
    );
    assert!(
        !pair.state().fail_next_tx,
        "the fault was injected into a pumped write"
    );
    assert_eq!(
        pair.state().shown.last(),
        Some(&wireless_content(3)),
        "the frame is sent again and reaches the fans"
    );

    drop(frame_tx);
    let result = timeout(Duration::from_secs(1), actor)
        .await
        .expect("the actor stops once its frame source is gone")
        .expect("actor task joins");
    assert!(result.is_ok(), "{result:?}");
}

/// A TX that takes every write while its fans stop confirming frames is
/// reset through the RX once the stall bound passes, and the actor ends
/// instead of streaming into a radio that delivers nothing.
#[tokio::test(start_paused = true)]
async fn lianli_fans_that_stop_confirming_end_the_actor_through_the_wedge_path() {
    use hypercolor_hal::drivers::lianli::wireless::health::{
        ControllerHealth, DongleHalf, WedgeCause, controller_health,
    };
    use hypercolor_hal::drivers::lianli::wireless::pacing::ECHO_STALL;
    use hypercolor_hal::drivers::lianli::wireless::transport::{
        WirelessControllerTransport, partner_reset_packet,
    };

    let master = [0xA0, 0x71, 0xAE, 0x72, 0xC0, 0x03];
    let controller = "usb-backend-test-lianli-undelivered";
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let pair = Arc::new(EchoingWirelessPair::new(master, Duration::from_millis(10)));
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedEchoingWirelessPair(Arc::clone(&pair))),
        controller,
    ));
    let protocol: Arc<dyn Protocol> = paced_wireless_protocol(master);

    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "lianli-undelivered-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let mut frame = 0_u8;
    for _ in 0..30 {
        frame = frame.wrapping_add(1);
        frame_tx.send_replace(Some(wireless_frame(frame)));
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    let confirmed = pair.state().transfers.len();
    assert!(
        confirmed > 20,
        "the radio worked first: {confirmed} transfers"
    );

    pair.state().rf_dead = true;
    let failed_at = tokio::time::Instant::now();
    while !actor.is_finished() {
        assert!(
            failed_at.elapsed() < ECHO_STALL + Duration::from_secs(5),
            "the actor must not stream into a dead radio forever"
        );
        frame = frame.wrapping_add(1);
        frame_tx.send_replace(Some(wireless_frame(frame)));
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    let ended_after = failed_at.elapsed();
    let result = actor.await.expect("actor task joins");
    assert!(result.is_err(), "the session ends with the verdict");
    assert!(
        ended_after + Duration::from_millis(100) >= ECHO_STALL,
        "the verdict waits out the stall bound: {ended_after:?}"
    );

    let state = pair.state();
    let resets: Vec<&(TransferType, Vec<u8>)> = state
        .sends
        .iter()
        .filter(|(_, data)| data[0] == 0x15)
        .collect();
    assert_eq!(
        resets,
        vec![&(TransferType::Companion, partner_reset_packet())],
        "one reset, written to the RX"
    );
    let after_failure = state
        .transfers
        .iter()
        .filter(|(at, _)| *at >= failed_at)
        .count();
    assert!(
        after_failure <= 8,
        "a bounded number of resends, not {after_failure} transfers"
    );
    drop(state);
    match controller_health(controller) {
        ControllerHealth::Wedged(wedge) => {
            assert_eq!(wedge.half, DongleHalf::Tx);
            assert_eq!(wedge.cause, WedgeCause::Undelivered);
        }
        ControllerHealth::Healthy => panic!("the stall is recorded as a TX wedge"),
    }
}

/// One actor session over `pair` for the controller `controller`, fed a
/// changing frame every 33 ms until the actor ends or `bound` passes.
/// Returns how it ended and the writes it made.
async fn run_lianli_session(
    master: [u8; 6],
    controller: &'static str,
    pair: &Arc<EchoingWirelessPair>,
    bound: Duration,
) -> (Option<anyhow::Result<()>>, Vec<(TransferType, Vec<u8>)>) {
    use hypercolor_hal::drivers::lianli::wireless::transport::WirelessControllerTransport;

    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(WirelessControllerTransport::new(
        Box::new(SharedEchoingWirelessPair(Arc::clone(pair))),
        controller,
    ));
    let protocol: Arc<dyn Protocol> = paced_wireless_protocol(master);
    let sends_before = pair.state().sends.len();
    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "lianli-recovery-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));
    let started = tokio::time::Instant::now();
    let mut frame = 0_u8;
    while !actor.is_finished() && started.elapsed() < bound {
        frame = frame.wrapping_add(1);
        frame_tx.send_replace(Some(wireless_frame(frame)));
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    let result = if actor.is_finished() {
        Some(actor.await.expect("actor task joins"))
    } else {
        actor.abort();
        None
    };
    let sends = pair.state().sends[sends_before..].to_vec();
    (result, sends)
}

/// Fans that stop confirming past the TX-reset budget: the next session
/// sends no reset but rests the cluster's RGB with upkeep going on, then
/// ends the actor as a disconnect asking for a fresh connect, which the
/// lifecycle turns into a reconnect. When the fans confirm again, their
/// recovery budget starts over.
#[tokio::test(start_paused = true)]
async fn lianli_fans_past_their_reset_budget_rest_then_end_the_actor_for_a_fresh_connect() {
    use hypercolor_hal::drivers::lianli::wireless::pacing::{
        MAX_RESETS_WITHOUT_DELIVERY, RECONNECT_RESTS, reconnects_without_delivery,
        resets_without_delivery,
    };

    let master = [0xA0, 0x71, 0xAE, 0x72, 0xC0, 0x05];
    let cluster = echoing_cluster(master);
    let controller = "usb-backend-test-lianli-fan-stall";
    let pair = Arc::new(EchoingWirelessPair::new(master, Duration::from_millis(10)));
    pair.state().rf_dead = true;
    let is_reset = |(_, data): &(TransferType, Vec<u8>)| data.first() == Some(&0x15);

    for session in 0..MAX_RESETS_WITHOUT_DELIVERY {
        let (result, sends) =
            run_lianli_session(master, controller, &pair, Duration::from_secs(15)).await;
        assert!(
            matches!(result, Some(Err(_))),
            "session {session} ends with the TX reset"
        );
        assert_eq!(sends.iter().filter(|send| is_reset(send)).count(), 1);
    }
    assert_eq!(
        resets_without_delivery(cluster),
        MAX_RESETS_WITHOUT_DELIVERY
    );

    let started = tokio::time::Instant::now();
    let (result, sends) =
        run_lianli_session(master, controller, &pair, Duration::from_mins(1)).await;
    let ended_after = started.elapsed();
    let error = result
        .expect("the session ends by itself")
        .expect_err("it ends as a disconnect");
    let chain = format!("{error:#}");
    assert!(
        chain.contains("reconnecting to run the connect sequence again"),
        "the reason names the fresh connect: {chain}"
    );
    assert_eq!(
        UsbBackend::classify_frame_write_error(&error),
        actor::FrameWriteDisposition::Fatal,
        "a disconnect, so the lifecycle reconnects"
    );
    assert!(
        !sends.iter().any(is_reset),
        "past the budget, no further TX reset"
    );
    assert!(
        ended_after >= RECONNECT_RESTS[0],
        "the rest comes first: ended after {ended_after:?}"
    );
    let upkeep = sends
        .iter()
        .filter(|(transfer, _)| *transfer == TransferType::Primary)
        .count();
    assert!(
        upkeep >= 8 * 8,
        "fan-speed and clock upkeep carry on through the rest: {upkeep} TX writes"
    );
    assert_eq!(reconnects_without_delivery(cluster), 1);

    // The fans answer again on the fresh connect.
    pair.state().rf_dead = false;
    let (result, _) = run_lianli_session(master, controller, &pair, Duration::from_secs(5)).await;
    assert!(result.is_none(), "the recovered session streams on");
    assert_eq!(resets_without_delivery(cluster), 0);
    assert_eq!(reconnects_without_delivery(cluster), 0);
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
    match failure {
        InjectedPrimaryFailure::Timeout => assert!(matches!(
            first_ack.error,
            Some(DeviceError::Timeout { after }) if after == Duration::from_millis(25)
        )),
        InjectedPrimaryFailure::Io => assert!(matches!(
            first_ack.error,
            Some(DeviceError::WriteError { .. })
        )),
    }

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

async fn wait_for_bulk_send_attempts(transport: &RecordingTransport, count: usize) {
    timeout(Duration::from_secs(1), async {
        loop {
            if transport.bulk_send_attempts() >= count {
                return;
            }

            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bulk transport send attempt should arrive before timeout");
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

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> std::result::Result<(), DisplayEncodeError> {
        commands.clear();
        commands.push(test_command(payload.data.first().copied().unwrap_or(0xD1)));
        Ok(())
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
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

/// Writes one command per frame and reports writing only part of it, as a
/// protocol does that sends only the zones a frame changed.
struct PartlyWritingProtocol;

impl Protocol for PartlyWritingProtocol {
    fn name(&self) -> &'static str {
        "partly-writing-test"
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

    fn written_frame_bytes(&self) -> Option<usize> {
        Some(18)
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::default()
    }

    fn total_leds(&self) -> u32 {
        10
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

/// A frame the protocol writes only part of is acknowledged with the bytes
/// actually written, not the whole payload.
#[tokio::test]
async fn a_partly_written_frame_is_acknowledged_with_the_bytes_written() {
    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (_command_tx, command_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(RecordingTransport::default());
    let protocol: Arc<dyn Protocol> = Arc::new(PartlyWritingProtocol);
    let actor = tokio::spawn(UsbBackend::test_run_device_actor(
        DeviceId::new(),
        "partly-writing-test-device",
        protocol,
        transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let id = DeviceDeliveryId {
        queue_generation: 11,
        sequence: 1,
    };
    let (frame, ack) = UsbFramePayload::tracked(id, Arc::new(vec![[1, 2, 3]; 10]));
    frame_tx.send_replace(Some(Arc::new(frame)));
    let ack = timeout(Duration::from_secs(1), ack)
        .await
        .expect("the frame is acknowledged")
        .expect("acknowledgement channel stays open");
    assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
    assert_eq!(
        ack.completed_payload_bytes, 18,
        "only the bytes the protocol wrote are counted"
    );

    drop(frame_tx);
    let result = timeout(Duration::from_secs(1), actor)
        .await
        .expect("the actor stops once its frame source is gone")
        .expect("actor task joins");
    assert!(result.is_ok(), "{result:?}");
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

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        Some(vec![test_command(brightness)])
    }

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> std::result::Result<(), DisplayEncodeError> {
        commands.clear();
        commands.push(test_command_with_transfer(
            payload.data.first().copied().unwrap_or(0xD1),
            TransferType::Bulk,
        ));
        Ok(())
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
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
    panicked_transfer_type: Option<TransferType>,
    failed_transfer_delay: Duration,
    bulk_send_release: Option<Arc<Notify>>,
    primary_send_attempts: AtomicUsize,
    bulk_send_attempts: AtomicUsize,
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

    fn with_bulk_send_delay(mut self, send_delay: Duration) -> Self {
        self.bulk_send_delay = Some(send_delay);
        self
    }

    fn with_bulk_send_release(mut self, release: Arc<Notify>) -> Self {
        self.bulk_send_release = Some(release);
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

    const fn with_panicked_transfer_type(mut self, transfer_type: TransferType) -> Self {
        self.panicked_transfer_type = Some(transfer_type);
        self
    }

    const fn with_failed_transfer_delay(mut self, delay: Duration) -> Self {
        self.failed_transfer_delay = delay;
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

    fn bulk_send_attempts(&self) -> usize {
        self.bulk_send_attempts.load(Ordering::Relaxed)
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
            TransferType::HidReport | TransferType::Companion => self.send_delay,
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
        if transfer_type == TransferType::Bulk {
            self.bulk_send_attempts.fetch_add(1, Ordering::Relaxed);
            if let Some(release) = &self.bulk_send_release {
                release.notified().await;
            }
        }
        assert_ne!(
            self.panicked_transfer_type,
            Some(transfer_type),
            "injected {transfer_type:?} transport panic"
        );
        if self.failed_transfer_type == Some(transfer_type) {
            if !self.failed_transfer_delay.is_zero() {
                tokio::time::sleep(self.failed_transfer_delay).await;
            }
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
        ..Default::default()
    }
}

/// One transport read as the actor asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoggedRead {
    timeout: Duration,
    capacity: Option<usize>,
}

/// Answers scripted reports in order and records how each read was requested.
struct ResponsePlanTransport {
    reports: Mutex<Vec<Vec<u8>>>,
    reads: Mutex<Vec<LoggedRead>>,
    sends: Mutex<Vec<Vec<u8>>>,
}

impl ResponsePlanTransport {
    fn new(reports: Vec<Vec<u8>>) -> Self {
        Self {
            reports: Mutex::new(reports),
            reads: Mutex::new(Vec::new()),
            sends: Mutex::new(Vec::new()),
        }
    }

    fn next_report(&self, timeout: Duration, capacity: Option<usize>) -> Vec<u8> {
        self.reads
            .lock()
            .expect("read log should not be poisoned")
            .push(LoggedRead { timeout, capacity });

        let mut reports = self.reports.lock().expect("reports should not be poisoned");
        if reports.is_empty() {
            Vec::new()
        } else {
            reports.remove(0)
        }
    }

    fn reads(&self) -> Vec<LoggedRead> {
        self.reads
            .lock()
            .expect("read log should not be poisoned")
            .clone()
    }

    fn sends(&self) -> Vec<Vec<u8>> {
        self.sends
            .lock()
            .expect("send log should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl Transport for ResponsePlanTransport {
    fn name(&self) -> &'static str {
        "response-plan-test"
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.sends
            .lock()
            .expect("send log should not be poisoned")
            .push(data.to_vec());
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        Ok(self.next_report(timeout, None))
    }

    async fn receive_logical(
        &self,
        timeout: Duration,
        _transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        Ok(self.next_report(timeout, capacity))
    }

    async fn send_receive_logical(
        &self,
        data: &[u8],
        timeout: Duration,
        _transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        Ok(self.next_report(timeout, capacity))
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

const PLAN_PROTOCOL_TIMEOUT: Duration = Duration::from_millis(150);

/// Records every report handed to `parse_response`, in arrival order, and
/// answers scripted statuses so retry paths can be driven.
struct ReportRecordingProtocol {
    seen: Mutex<Vec<Vec<u8>>>,
    statuses: Mutex<Vec<ResponseStatus>>,
}

impl ReportRecordingProtocol {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
        }
    }

    /// Answer these statuses in order, then `Ok` forever after.
    fn with_statuses(statuses: Vec<ResponseStatus>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            statuses: Mutex::new(statuses),
        }
    }

    fn seen(&self) -> Vec<Vec<u8>> {
        self.seen
            .lock()
            .expect("report log should not be poisoned")
            .clone()
    }
}

impl Protocol for ReportRecordingProtocol {
    fn name(&self) -> &'static str {
        "report-recording-test"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn parse_response(&self, data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        self.seen
            .lock()
            .expect("report log should not be poisoned")
            .push(data.to_vec());

        let mut statuses = self
            .statuses
            .lock()
            .expect("statuses should not be poisoned");
        let status = if statuses.is_empty() {
            ResponseStatus::Ok
        } else {
            statuses.remove(0)
        };
        Ok(ProtocolResponse {
            status,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        PLAN_PROTOCOL_TIMEOUT
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::default()
    }

    fn total_leds(&self) -> u32 {
        0
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

fn responding_command(byte: u8) -> ProtocolCommand {
    ProtocolCommand {
        data: vec![byte],
        expects_response: true,
        ..Default::default()
    }
}

/// A command that answers with two reports must have both read, or the
/// second one is still queued when the next command reads its reply.
#[tokio::test]
async fn response_count_reads_every_report_and_keeps_the_next_command_in_sync() {
    let protocol = ReportRecordingProtocol::new();
    let transport = ResponsePlanTransport::new(vec![
        b"1.0.5".to_vec(),
        b"Mar 14 2026".to_vec(),
        b"next-command-reply".to_vec(),
    ]);

    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0xA6).with_response_count(2),
            responding_command(0xAA),
        ],
    )
    .await
    .expect("both commands should run");

    assert_eq!(
        protocol.seen(),
        vec![
            b"1.0.5".to_vec(),
            b"Mar 14 2026".to_vec(),
            b"next-command-reply".to_vec(),
        ],
        "reports must reach parse_response in arrival order"
    );
    assert_eq!(transport.reads().len(), 3, "two reports plus one");
    assert_eq!(
        transport.sends(),
        vec![vec![0xA6], vec![0xAA]],
        "the extra report is a read, not a resend"
    );
}

#[tokio::test]
async fn a_command_response_timeout_overrides_the_protocol_budget() {
    let protocol = ReportRecordingProtocol::new();
    let transport = ResponsePlanTransport::new(vec![vec![0x01], vec![0x02], vec![0x03]]);

    let init_timeout = Duration::from_secs(3);
    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0x3E)
                .with_response_timeout(init_timeout)
                .with_response_count(2),
            responding_command(0x46),
        ],
    )
    .await
    .expect("commands should run");

    let timeouts: Vec<Duration> = transport.reads().into_iter().map(|r| r.timeout).collect();
    assert_eq!(
        timeouts,
        vec![init_timeout, init_timeout, PLAN_PROTOCOL_TIMEOUT],
        "the override covers every report of its own command and nothing after"
    );
}

#[tokio::test]
async fn response_len_reaches_the_transport_as_a_read_capacity() {
    let protocol = ReportRecordingProtocol::new();
    let transport = ResponsePlanTransport::new(vec![vec![0x01; 8], vec![0x02; 8]]);

    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0xA0).with_response_capacity(508),
            responding_command(0xA1),
        ],
    )
    .await
    .expect("commands should run");

    let capacities: Vec<Option<usize>> =
        transport.reads().into_iter().map(|r| r.capacity).collect();
    assert_eq!(
        capacities,
        vec![Some(508), None],
        "capacity travels per command, and a command without one reads at the transport default"
    );
}

/// A retry partway through a multi-report command must not leave the rest of
/// that attempt's reports queued: the resend would read one as its own reply
/// and every read after it would be answering the wrong command.
#[tokio::test]
async fn a_retry_discards_the_reports_left_over_from_the_failed_attempt() {
    let protocol =
        ReportRecordingProtocol::with_statuses(vec![ResponseStatus::Busy, ResponseStatus::Ok]);
    let transport = ResponsePlanTransport::new(vec![
        b"busy".to_vec(),
        b"stale-date".to_vec(),
        b"version".to_vec(),
        b"date".to_vec(),
        b"next-command-reply".to_vec(),
    ]);

    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0xA6).with_response_count(2),
            responding_command(0xAA),
        ],
    )
    .await
    .expect("the retried command should succeed on its second attempt");

    assert_eq!(
        transport.sends(),
        vec![vec![0xA6], vec![0xA6], vec![0xAA]],
        "the busy command is resent once"
    );
    assert_eq!(
        protocol.seen(),
        vec![
            b"busy".to_vec(),
            b"version".to_vec(),
            b"date".to_vec(),
            b"next-command-reply".to_vec(),
        ],
        "the abandoned attempt's second report is discarded, not parsed as \
         the resend's reply"
    );
}

/// Answers each read from a script of results, so a scripted timeout can
/// stand in for a device that stays quiet.
struct ScriptedReadTransport {
    reads: Mutex<Vec<std::result::Result<Vec<u8>, TransportError>>>,
    sends: Mutex<Vec<Vec<u8>>>,
}

impl ScriptedReadTransport {
    fn new(reads: Vec<std::result::Result<Vec<u8>, TransportError>>) -> Self {
        Self {
            reads: Mutex::new(reads),
            sends: Mutex::new(Vec::new()),
        }
    }

    fn next_read(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        let mut reads = self.reads.lock().expect("reads should not be poisoned");
        if reads.is_empty() {
            Err(TransportError::Timeout {
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            })
        } else {
            reads.remove(0)
        }
    }

    fn sends(&self) -> Vec<Vec<u8>> {
        self.sends
            .lock()
            .expect("send log should not be poisoned")
            .clone()
    }

    fn reads_left(&self) -> usize {
        self.reads
            .lock()
            .expect("reads should not be poisoned")
            .len()
    }
}

#[async_trait]
impl Transport for ScriptedReadTransport {
    fn name(&self) -> &'static str {
        "scripted-read-test"
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.sends
            .lock()
            .expect("send log should not be poisoned")
            .push(data.to_vec());
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        self.next_read(timeout)
    }

    async fn receive_logical(
        &self,
        timeout: Duration,
        _transfer_type: TransferType,
        _capacity: Option<usize>,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.next_read(timeout)
    }

    async fn send_receive_logical(
        &self,
        data: &[u8],
        timeout: Duration,
        _transfer_type: TransferType,
        _capacity: Option<usize>,
    ) -> std::result::Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.next_read(timeout)
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

/// A status packet the firmware sends most of the time is an optional reply:
/// its absence completes the command instead of failing the batch.
#[tokio::test]
async fn an_optional_reply_that_never_arrives_completes_the_command() {
    let protocol = ReportRecordingProtocol::new();
    let transport = ScriptedReadTransport::new(vec![
        Err(TransportError::Timeout { timeout_ms: 1 }),
        Ok(b"next-command-reply".to_vec()),
    ]);

    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0x65).with_optional_response(),
            responding_command(0xAA),
        ],
    )
    .await
    .expect("a quiet device does not fail an optional read");

    assert_eq!(
        transport.sends(),
        vec![vec![0x65], vec![0xAA]],
        "the quiet reply is not retried, the batch moves on"
    );
    assert_eq!(protocol.seen(), vec![b"next-command-reply".to_vec()]);
}

/// A trailing report some units skip is the same shape: the reports that did
/// arrive are parsed, the missing one ends the command.
#[tokio::test]
async fn an_optional_trailing_report_ends_the_command_after_the_reports_that_came() {
    let protocol = ReportRecordingProtocol::new();
    let transport = ScriptedReadTransport::new(vec![
        Ok(b"version".to_vec()),
        Err(TransportError::Timeout { timeout_ms: 1 }),
        Ok(b"next-command-reply".to_vec()),
    ]);

    UsbBackend::run_commands(
        &protocol,
        &transport,
        &[
            responding_command(0xA6)
                .with_response_count(2)
                .with_optional_response(),
            responding_command(0xAA),
        ],
    )
    .await
    .expect("a missing second report is tolerated");

    assert_eq!(
        protocol.seen(),
        vec![b"version".to_vec(), b"next-command-reply".to_vec()]
    );
}

/// The default plan keeps the old contract: a required reply that never comes
/// fails the batch.
#[tokio::test]
async fn a_required_reply_that_never_arrives_fails_the_batch() {
    let protocol = ReportRecordingProtocol::new();
    let transport =
        ScriptedReadTransport::new(vec![Err(TransportError::Timeout { timeout_ms: 1 })]);

    let error = UsbBackend::run_commands(&protocol, &transport, &[responding_command(0x3C)])
        .await
        .expect_err("a required reply is part of the contract");

    assert!(
        format!("{error:#}").contains("timeout"),
        "the failure names the timeout: {error:#}"
    );
}

/// Parses every report as malformed, standing in for a device whose first
/// report of a multi-report command is garbage.
struct MalformedReportProtocol;

impl Protocol for MalformedReportProtocol {
    fn name(&self) -> &'static str {
        "malformed-report-test"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn parse_response(&self, _data: &[u8]) -> std::result::Result<ProtocolResponse, ProtocolError> {
        Err(ProtocolError::MalformedResponse {
            detail: "scripted".to_owned(),
        })
    }

    fn response_timeout(&self) -> Duration {
        PLAN_PROTOCOL_TIMEOUT
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities::default()
    }

    fn total_leds(&self) -> u32 {
        0
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

/// A parse failure aborts the batch, but the reports still queued for the
/// failed command must not be left for the next session to misread.
#[tokio::test]
async fn a_parse_failure_drains_the_reports_still_queued_for_the_command() {
    let transport =
        ScriptedReadTransport::new(vec![Ok(b"garbage".to_vec()), Ok(b"stale-date".to_vec())]);

    UsbBackend::run_commands(
        &MalformedReportProtocol,
        &transport,
        &[responding_command(0xA6).with_response_count(2)],
    )
    .await
    .expect_err("a malformed report fails the command");

    assert_eq!(
        transport.reads_left(),
        0,
        "the second report was drained before the failure propagated"
    );
}

/// Refuses every send with a timeout, standing in for an OUT transfer that
/// never completes.
struct SendTimeoutTransport;

#[async_trait]
impl Transport for SendTimeoutTransport {
    fn name(&self) -> &'static str {
        "send-timeout-test"
    }

    async fn send(&self, _data: &[u8]) -> std::result::Result<(), TransportError> {
        Err(TransportError::Timeout { timeout_ms: 1 })
    }

    async fn receive(&self, _timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        Ok(vec![0x01])
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

/// An optional reply forgives a quiet device, not a failed send.
#[tokio::test]
async fn an_optional_reply_does_not_forgive_a_send_that_timed_out() {
    let protocol = ReportRecordingProtocol::new();

    UsbBackend::run_commands(
        &protocol,
        &SendTimeoutTransport,
        &[responding_command(0x65).with_optional_response()],
    )
    .await
    .expect_err("the frame never reached the wire");

    assert!(protocol.seen().is_empty(), "nothing was read as a reply");
}

/// Push 2 stand-in for actor tests: applies palette-indexed LED MIDI,
/// answers palette reads, and can stall like a wedged MIDI endpoint.
struct FakePush2Transport {
    device: Mutex<FakePush2Device>,
    stalled: AtomicBool,
    backlog: Mutex<Vec<Vec<u8>>>,
    pending_reply: Mutex<Option<Vec<u8>>>,
    sends_while_stalled: AtomicUsize,
}

struct FakePush2Device {
    palette: [[u8; 4]; 128],
    led_index: HashMap<(u8, u8), u8>,
    lit: HashMap<(u8, u8), [u8; 4]>,
}

const FAKE_PUSH2_PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
const FAKE_KERNEL_BUFFER_BYTES: usize = 4096;

impl FakePush2Device {
    fn process(&mut self, message: &[u8]) -> Option<Vec<u8>> {
        match message.first().copied() {
            Some(status @ (0x90 | 0xB0)) => {
                let key = (status, message[1]);
                self.led_index.insert(key, message[2]);
                self.lit.insert(key, self.palette[usize::from(message[2])]);
                None
            }
            Some(0xF0) if message == [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7] => Some(vec![
                0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, 0x01, 0x00,
                0x2F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF7,
            ]),
            Some(0xF0) if message.len() >= 8 && message[..6] == FAKE_PUSH2_PREFIX => {
                let args = &message[7..message.len() - 1];
                match message[6] {
                    0x03 => {
                        let mut entry = [0_u8; 4];
                        for (channel, value) in entry.iter_mut().enumerate() {
                            *value = args[1 + channel * 2] | (args[2 + channel * 2] << 7);
                        }
                        self.palette[usize::from(args[0])] = entry;
                        None
                    }
                    0x04 => {
                        let mut reply = FAKE_PUSH2_PREFIX.to_vec();
                        reply.extend_from_slice(&[0x04, args[0]]);
                        for value in self.palette[usize::from(args[0])] {
                            reply.extend_from_slice(&[value & 0x7F, value >> 7]);
                        }
                        reply.push(0xF7);
                        Some(reply)
                    }
                    0x05 => {
                        for (key, index) in &self.led_index {
                            self.lit.insert(*key, self.palette[usize::from(*index)]);
                        }
                        None
                    }
                    0x0A => {
                        let mut reply = FAKE_PUSH2_PREFIX.to_vec();
                        reply.extend_from_slice(&[0x0A, args[0], 0xF7]);
                        Some(reply)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl FakePush2Transport {
    fn new() -> Self {
        Self {
            device: Mutex::new(FakePush2Device {
                palette: [[0; 4]; 128],
                led_index: HashMap::new(),
                lit: HashMap::new(),
            }),
            stalled: AtomicBool::new(false),
            backlog: Mutex::new(Vec::new()),
            pending_reply: Mutex::new(None),
            sends_while_stalled: AtomicUsize::new(0),
        }
    }

    fn stall(&self) {
        self.stalled.store(true, Ordering::SeqCst);
    }

    fn unstall(&self) {
        let backlog = std::mem::take(&mut *self.backlog.lock().expect("backlog lock"));
        let mut device = self.device.lock().expect("device lock");
        for message in backlog {
            let _ = device.process(&message);
        }
        self.stalled.store(false, Ordering::SeqCst);
    }

    fn backlog_bytes(&self) -> usize {
        self.backlog
            .lock()
            .expect("backlog lock")
            .iter()
            .map(Vec::len)
            .sum()
    }

    fn pads_show(&self, frame: &[[u8; 3]]) -> bool {
        let device = self.device.lock().expect("device lock");
        (0..64_u8).all(|pad| {
            device
                .lit
                .get(&(0x90, 36 + pad))
                .is_some_and(|lit| lit[..3] == frame[usize::from(pad)])
        })
    }
}

#[async_trait]
impl Transport for FakePush2Transport {
    fn name(&self) -> &'static str {
        "fake-push2"
    }

    fn supports_parallel_transfer_lanes(&self) -> bool {
        true
    }

    async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> std::result::Result<(), TransportError> {
        if transfer_type != TransferType::Primary {
            return Ok(());
        }
        if self.stalled.load(Ordering::SeqCst) {
            self.sends_while_stalled.fetch_add(1, Ordering::SeqCst);
            let mut backlog = self.backlog.lock().expect("backlog lock");
            let queued: usize = backlog.iter().map(Vec::len).sum();
            if queued + data.len() > FAKE_KERNEL_BUFFER_BYTES {
                return Err(TransportError::Timeout { timeout_ms: 1000 });
            }
            backlog.push(data.to_vec());
            return Ok(());
        }
        let reply = self.device.lock().expect("device lock").process(data);
        *self.pending_reply.lock().expect("reply lock") = reply;
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> std::result::Result<Vec<u8>, TransportError> {
        let reply = self.pending_reply.lock().expect("reply lock").take();
        if let Some(reply) = reply {
            return Ok(reply);
        }
        // A wedged endpoint never answers: the actor waits out the full
        // response budget, exactly as it would on hardware.
        tokio::time::sleep(timeout).await;
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

fn push2_test_frame(seed: u32) -> Vec<[u8; 3]> {
    (0..160_u32)
        .map(|led| {
            let red = u8::try_from((led * 7 + seed) % 256).expect("wrapped to a byte");
            let green = u8::try_from((led * 13 + seed * 3) % 256).expect("wrapped to a byte");
            [red, green, 128]
        })
        .collect()
}

async fn publish_until(
    frame_tx: &watch::Sender<Option<Arc<UsbFramePayload>>>,
    clock: &Mutex<std::time::Instant>,
    frame: &[[u8; 3]],
    mut done: impl FnMut() -> bool,
) {
    timeout(Duration::from_secs(5), async {
        while !done() {
            *clock.lock().expect("clock lock") += Duration::from_millis(100);
            frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(
                frame.to_vec(),
            )))));
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("Push 2 fake should converge before the timeout");
}

// Paused time lets the actor wait out real 1 s response budgets for every
// unanswered probe without the test taking that long.
#[tokio::test(start_paused = true)]
async fn push2_midi_stall_never_blocks_frame_publication_or_tears_down_the_actor() {
    let clock = Arc::new(Mutex::new(std::time::Instant::now()));
    let protocol_clock = Arc::clone(&clock);
    let protocol = Arc::new(hypercolor_hal::drivers::push2::Push2Protocol::with_clock(
        Arc::new(move || *protocol_clock.lock().expect("clock lock")),
    ));
    let transport = Arc::new(FakePush2Transport::new());
    UsbBackend::run_commands(
        protocol.as_ref(),
        transport.as_ref(),
        &protocol.init_sequence(),
    )
    .await
    .expect("init should complete against a healthy fake");

    let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
    let (_display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let actor_protocol: Arc<dyn Protocol> = protocol.clone();
    let actor_transport: Arc<dyn Transport> = transport.clone();
    let actor = tokio::spawn(UsbBackend::test_run_parallel_device_actor(
        DeviceId::new(),
        "push2-stall-test-device",
        actor_protocol,
        actor_transport,
        frame_rx,
        display_rx,
        command_rx,
    ));

    let first = push2_test_frame(0);
    publish_until(&frame_tx, &clock, &first, || transport.pads_show(&first)).await;

    // The endpoint wedges. The render side keeps publishing at full rate and
    // never waits on the device: the watch slot only ever holds the newest
    // frame, whatever the actor is doing.
    transport.stall();
    let publish_started = std::time::Instant::now();
    for seed in 1..=2_000 {
        frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(
            push2_test_frame(seed),
        )))));
        if seed % 100 == 0 {
            tokio::task::yield_now().await;
        }
    }
    let publish_elapsed = publish_started.elapsed();
    for seed in 0..30 {
        *clock.lock().expect("clock lock") += Duration::from_secs(1);
        frame_tx.send_replace(Some(Arc::new(UsbFramePayload::untracked(Arc::new(
            push2_test_frame(3_000 + seed),
        )))));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Guards the architecture rather than the driver: the render side hands
    // frames to a latest-value slot and must never start awaiting the device.
    assert!(
        publish_elapsed < Duration::from_millis(250),
        "publishing 2000 frames into a stalled device took {publish_elapsed:?}"
    );
    assert!(
        !actor.is_finished(),
        "a stalled endpoint must not end the actor"
    );
    let stalled_sends = transport.sends_while_stalled.load(Ordering::SeqCst);
    // One batch went out before the missing acknowledgement was noticed,
    // then only backed-off probes.
    assert!(
        (2..=160).contains(&stalled_sends),
        "{stalled_sends} sends reached the stalled endpoint"
    );
    assert!(transport.backlog_bytes() <= 512 + 30 * 9);

    // The endpoint recovers: the next probe is answered, the lane resyncs,
    // and the device lands on the newest frame without a reconnect.
    transport.unstall();
    let last = push2_test_frame(9_999);
    publish_until(&frame_tx, &clock, &last, || transport.pads_show(&last)).await;
    assert!(!actor.is_finished());

    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(UsbDeviceCommand::Shutdown {
            led_count: 0,
            response_tx,
        })
        .expect("actor command channel should remain open");
    response_rx
        .await
        .expect("shutdown response should be delivered")
        .expect("shutdown should succeed");
    actor
        .await
        .expect("actor task should join")
        .expect("actor should exit cleanly");
}
