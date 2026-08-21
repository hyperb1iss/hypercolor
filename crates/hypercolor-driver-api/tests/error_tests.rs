use std::time::Duration;

use hypercolor_driver_api::{DeviceLifecyclePolicy, DriverError, ErrorRecoverability};
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
