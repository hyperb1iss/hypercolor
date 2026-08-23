use std::time::Duration;

use hypercolor_driver_api::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceLifecyclePolicy, DeviceWriteOutcome, DriverError,
    ErrorRecoverability,
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
