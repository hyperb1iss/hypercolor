//! Backend I/O handles that can outlive the manager lock.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::DiscoveredDevice;
use hypercolor_types::device::{DeviceId, DeviceInfo, OwnedDisplayFramePayload};
use tracing::debug;

use crate::device::traits::{DeviceDisplaySink, DeviceFrameSink, OutputCadence};

use super::BackendHandle;

/// Lightweight handle for backend I/O that can outlive the manager lock.
///
/// Clone this from [`super::BackendManager::backend_io`] while holding the
/// manager briefly, then perform the awaited backend call after releasing the
/// outer manager mutex.
#[derive(Clone)]
pub struct BackendIo {
    backend_id: String,
    backend: BackendHandle,
}

impl BackendIo {
    pub(super) const fn new(backend_id: String, backend: BackendHandle) -> Self {
        Self {
            backend_id,
            backend,
        }
    }

    /// Connect a device, retrying once after cleanup and backend discovery refresh.
    ///
    /// Returns the backend's preferred output cadence for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails both before and
    /// after discovery refresh.
    pub async fn connect_with_refresh(&self, device_id: DeviceId) -> Result<OutputCadence> {
        self.connect_with_refresh_inner(device_id, None).await
    }

    /// Connect a device, applying timeout only to backend operations after
    /// this handle acquires the backend lock.
    ///
    /// Returns the backend's preferred output cadence for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails or times out.
    pub async fn connect_with_refresh_timeout(
        &self,
        device_id: DeviceId,
        timeout: Duration,
    ) -> Result<OutputCadence> {
        self.connect_with_refresh_inner(device_id, Some(timeout))
            .await
    }

    async fn connect_with_refresh_inner(
        &self,
        device_id: DeviceId,
        timeout: Option<Duration>,
    ) -> Result<OutputCadence> {
        let mut backend = self.backend.lock().await;

        if let Err(initial_error) = run_backend_operation(
            timeout,
            &self.backend_id,
            device_id,
            "connect",
            backend.connect(&device_id),
        )
        .await
        {
            let initial_message = initial_error.to_string();
            if is_backend_operation_timeout(&initial_error) {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "backend connect timed out; preserving discovery state for reconnect"
                );
                return Err(initial_error);
            } else if is_missing_discovery_descriptor(&initial_message) {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "backend discovery state missing; refreshing before connect retry"
                );
            } else {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "initial connect failed; refreshing backend discovery state and retrying"
                );

                match run_backend_operation(
                    timeout,
                    &self.backend_id,
                    device_id,
                    "disconnect cleanup",
                    backend.disconnect(&device_id),
                )
                .await
                {
                    Ok(()) => debug!(
                        backend_id = %self.backend_id,
                        %device_id,
                        "best-effort cleanup after failed connect completed"
                    ),
                    Err(cleanup_error) => debug!(
                        backend_id = %self.backend_id,
                        %device_id,
                        error = %cleanup_error,
                        "best-effort cleanup after failed connect could not release an existing session"
                    ),
                }
            }

            run_backend_operation(
                timeout,
                &self.backend_id,
                device_id,
                "discovery refresh",
                backend.discover(),
            )
            .await
            .with_context(|| {
                format!(
                    "backend '{}' discovery refresh failed after initial connect failure for device {device_id}: {initial_message}",
                    self.backend_id
                )
            })?;

            if let Err(retry_error) = run_backend_operation(
                timeout,
                &self.backend_id,
                device_id,
                "connect retry",
                backend.connect(&device_id),
            )
            .await
            {
                let retry_message = retry_error.to_string();
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %retry_message,
                    "connect still failing after discovery refresh"
                );
                return Err(retry_error).with_context(|| {
                    format!(
                        "failed to connect device {device_id} using backend '{}' after discovery refresh (initial error: {initial_message})",
                        self.backend_id
                    )
                });
            }

            debug!(
                backend_id = %self.backend_id,
                %device_id,
                "connect succeeded after discovery refresh"
            );
        }

        Ok(backend.output_cadence(&device_id).unwrap_or_default())
    }

    /// Prime the backend's discovery cache from a scanner result.
    pub async fn remember_discovered_device(&self, discovered: &DiscoveredDevice) {
        let mut backend = self.backend.lock().await;
        backend.remember_discovered_device(discovered);
    }

    /// Fetch refreshed metadata for a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata retrieval fails.
    pub async fn connected_device_info(&self, device_id: DeviceId) -> Result<Option<DeviceInfo>> {
        let backend = self.backend.lock().await;
        backend
            .connected_device_info(&device_id)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch connected device metadata for {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }

    /// Clone the hot-path frame sink for a connected device, if the backend exposes one.
    pub async fn frame_sink(&self, device_id: DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        let backend = self.backend.lock().await;
        backend.frame_sink(&device_id)
    }

    /// Clone the hot-path display sink for a connected device, if the backend exposes one.
    pub async fn display_sink(&self, device_id: DeviceId) -> Option<Arc<dyn DeviceDisplaySink>> {
        let backend = self.backend.lock().await;
        backend.display_sink(&device_id)
    }

    /// Whether this backend can briefly connect an idle device for direct control.
    pub async fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        let backend = self.backend.lock().await;
        backend.supports_temporary_direct_control(info)
    }

    /// Whether this backend consumes host-managed attachment profiles.
    pub async fn supports_host_attachment_profiles(&self, info: &DeviceInfo) -> bool {
        let backend = self.backend.lock().await;
        backend.supports_host_attachment_profiles(info)
    }

    /// Disconnect a device from the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend disconnect call fails.
    pub async fn disconnect(&self, device_id: DeviceId) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend.disconnect(&device_id).await.with_context(|| {
            format!(
                "failed to disconnect device {device_id} using backend '{}'",
                self.backend_id
            )
        })
    }

    /// Write immediate LED colors directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend write fails.
    pub async fn write_colors(&self, device_id: DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_colors(&device_id, colors)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} colors to device {device_id} using backend '{}'",
                    colors.len(),
                    self.backend_id
                )
            })
    }

    /// Set hardware brightness directly on the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend brightness write fails.
    pub async fn set_brightness(&self, device_id: DeviceId, brightness: u8) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .set_brightness(&device_id, brightness)
            .await
            .with_context(|| {
                format!(
                    "failed to set brightness {brightness} on device {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }

    /// Write immediate display bytes directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame(&self, device_id: DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    jpeg_data.len(),
                    self.backend_id
                )
            })
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame_owned(
        &self,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let byte_len = jpeg_data.len();
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame_owned(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    byte_len, self.backend_id
                )
            })
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_payload_owned(
        &self,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        let byte_len = payload.data.len();
        let format = payload.format;
        let mut backend = self.backend.lock().await;
        backend
            .write_display_payload_owned(&device_id, payload)
            .await
            .with_context(|| {
                format!(
                    "failed to write {byte_len} {format} display bytes to device {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }
}

fn is_missing_discovery_descriptor(message: &str) -> bool {
    message.contains(" has no pending ") && message.contains(" descriptor; run discover()")
}

fn is_backend_operation_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("transport timeout after")
            || message.contains(" timed out after ") && message.contains(" using backend ")
    })
}

async fn run_backend_operation<T, F>(
    timeout: Option<Duration>,
    backend_id: &str,
    device_id: DeviceId,
    operation: &'static str,
    future: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let Some(timeout) = timeout else {
        return future.await;
    };

    let Ok(result) = tokio::time::timeout(timeout, future).await else {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        bail!(
            "device {operation} timed out after {timeout_ms}ms using backend '{backend_id}' for device {device_id}"
        );
    };

    result
}
