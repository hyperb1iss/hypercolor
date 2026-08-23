use hypercolor_driver_api::DriverHost;
use hypercolor_types::device::DeviceId;
use tracing::warn;

/// Best-effort immediate activation after pairing.
pub async fn activate_if_requested(
    host: &dyn DriverHost,
    activate_after_pair: bool,
    device_id: DeviceId,
    backend_id: &str,
) -> bool {
    if !activate_after_pair {
        return false;
    }

    match host.runtime().activate_device(device_id, backend_id).await {
        Ok(activated) => activated,
        Err(error) => {
            warn!(
                error = %error,
                device_id = %device_id,
                backend_id = %backend_id,
                "paired device activation failed"
            );
            false
        }
    }
}

/// Best-effort disconnect after credentials are removed.
pub async fn disconnect_after_unpair(
    host: &dyn DriverHost,
    device_id: DeviceId,
    backend_id: &str,
) -> bool {
    match host
        .runtime()
        .disconnect_device(device_id, backend_id, false)
        .await
    {
        Ok(disconnected) => disconnected,
        Err(error) => {
            warn!(
                error = %error,
                device_id = %device_id,
                backend_id = %backend_id,
                "paired device disconnect failed"
            );
            false
        }
    }
}
