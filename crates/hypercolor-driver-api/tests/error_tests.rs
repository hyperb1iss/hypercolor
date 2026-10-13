use std::time::Duration;

use hypercolor_driver_api::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryStatus, DeviceLifecyclePolicy,
    DeviceWriteOutcome, DriverError, ErrorRecoverability,
};
use hypercolor_types::device::DeviceError;

#[test]
fn driver_error_recoverability_is_typed() {
    assert_eq!(
        DriverError::Timeout {
            after: Duration::from_secs(1),
        }
        .recoverability(),
        ErrorRecoverability::Retry
    );
    assert_eq!(
        DriverError::Configuration {
            message: "missing endpoint".to_owned(),
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );
    assert_eq!(
        DriverError::discovery("mDNS socket closed").recoverability(),
        ErrorRecoverability::Retry
    );
    assert_eq!(
        DriverError::pairing("bridge button not pressed").recoverability(),
        ErrorRecoverability::Retry
    );
}

#[test]
fn anyhow_converts_to_backend_construction_error() {
    let error = DriverError::from(anyhow::anyhow!("socket unavailable"));

    assert!(matches!(error, DriverError::BackendConstruction { .. }));
    assert!(error.to_string().contains("socket unavailable"));
}

#[test]
fn lifecycle_policy_owns_typed_connect_retry_decisions() {
    let timeout = DeviceError::Timeout {
        after: Duration::from_secs(1),
    };
    let reconnect = DeviceError::connection("fixture", "connection refused");
    let permanent = DeviceError::PermissionDenied {
        device: "fixture".to_owned(),
        detail: "access denied".to_owned(),
    };

    assert!(DeviceLifecyclePolicy::default().should_retry_connect_failure(&timeout));
    assert!(
        !DeviceLifecyclePolicy::default()
            .without_connect_timeout_retry()
            .should_retry_connect_failure(&timeout)
    );
    assert!(DeviceLifecyclePolicy::default().should_retry_connect_failure(&reconnect));
    assert!(!DeviceLifecyclePolicy::default().should_retry_connect_failure(&permanent));
}

#[test]
fn delivery_ack_preserves_typed_device_error() {
    let error = DeviceError::Timeout {
        after: Duration::from_millis(25),
    };
    let ack = DeviceDeliveryAck::from_write_result(
        DeviceDeliveryId {
            queue_generation: 4,
            sequence: 9,
        },
        3,
        Duration::from_millis(25),
        Err::<DeviceWriteOutcome, _>(error.clone()),
    );

    assert_eq!(ack.error, Some(error));
}

#[test]
fn delivery_ack_marks_only_transient_failures_transient() {
    let id = DeviceDeliveryId {
        queue_generation: 4,
        sequence: 10,
    };
    let error = DeviceError::write("fixture", "hid write reported a short count");

    let transient =
        DeviceDeliveryAck::failed_transient(id, true, Duration::from_millis(2), error.clone());
    assert!(transient.transient);
    assert_eq!(transient.error, Some(error.clone()));
    assert_eq!(
        transient,
        DeviceDeliveryAck {
            transient: true,
            ..DeviceDeliveryAck::failed(id, true, Duration::from_millis(2), error.clone())
        }
    );

    assert!(!DeviceDeliveryAck::failed(id, true, Duration::ZERO, error.clone()).transient);
    assert!(!DeviceDeliveryAck::rejected(id, error.clone()).transient);
    assert!(!DeviceDeliveryAck::completed(id, 3, Duration::ZERO).transient);
    assert!(
        !DeviceDeliveryAck::from_write_result(
            id,
            3,
            Duration::ZERO,
            Err::<DeviceWriteOutcome, _>(error)
        )
        .transient
    );
}

#[test]
fn delivery_ack_reports_a_declined_write_as_neither_completed_nor_failed() {
    let id = DeviceDeliveryId {
        queue_generation: 4,
        sequence: 11,
    };
    let ack = DeviceDeliveryAck::from_write_result(
        id,
        675,
        Duration::from_millis(3),
        Ok(DeviceWriteOutcome::Declined),
    );

    assert_eq!(
        ack,
        DeviceDeliveryAck::declined(id, Duration::from_millis(3))
    );
    assert_eq!(ack.status, DeviceDeliveryStatus::Declined);
    assert!(ack.transport_started, "the payload reached the device");
    assert_eq!(ack.completed_payload_bytes, 0, "nothing was delivered");
    assert_eq!(ack.error, None, "a decline is not a transport failure");
    assert!(!ack.transient);
    assert!(!DeviceWriteOutcome::Declined.is_sent());
}
