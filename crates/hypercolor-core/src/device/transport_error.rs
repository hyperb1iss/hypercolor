use std::time::Duration;

use anyhow::Error;
use hypercolor_hal::transport::TransportError;
use hypercolor_types::device::{DeviceError, DeviceId};

#[derive(Clone, Copy)]
pub(super) enum DeviceTransportOperation {
    Connect,
    Disconnect,
    Write,
}

pub(super) fn map_hal_transport_error(
    device_id: DeviceId,
    backend_id: &'static str,
    operation: DeviceTransportOperation,
    error: &Error,
) -> DeviceError {
    let transport_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TransportError>());

    match transport_error {
        Some(TransportError::NotFound { detail }) => operation.not_found(device_id, detail),
        Some(TransportError::Timeout { timeout_ms }) => DeviceError::Timeout {
            after: Duration::from_millis(*timeout_ms),
        },
        Some(TransportError::Disconnected { .. } | TransportError::Closed) => {
            DeviceError::Disconnected {
                device: device_id.to_string(),
            }
        }
        Some(TransportError::PermissionDenied { detail }) => DeviceError::PermissionDenied {
            device: device_id.to_string(),
            detail: detail.clone(),
        },
        Some(TransportError::UnsupportedTransfer { .. }) => DeviceError::Unsupported {
            backend: backend_id.to_owned(),
            operation: operation.description(),
        },
        Some(TransportError::IoError { .. }) | None => operation.fallback(device_id, error),
    }
}

impl DeviceTransportOperation {
    const fn description(self) -> &'static str {
        match self {
            Self::Connect => "transport connection",
            Self::Disconnect => "transport disconnect",
            Self::Write => "transport write",
        }
    }

    fn fallback(self, device_id: DeviceId, error: &Error) -> DeviceError {
        match self {
            Self::Connect => DeviceError::connection(device_id, error),
            Self::Disconnect => DeviceError::connection(device_id, error),
            Self::Write => DeviceError::write(device_id, error),
        }
    }

    fn not_found(self, device_id: DeviceId, detail: &str) -> DeviceError {
        match self {
            Self::Connect => DeviceError::NotFound {
                device: format!("{device_id} ({detail})"),
            },
            Self::Disconnect | Self::Write => DeviceError::Disconnected {
                device: device_id.to_string(),
            },
        }
    }
}
