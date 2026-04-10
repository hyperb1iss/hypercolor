//! Core device backend and plugin traits.
//!
//! Every hardware protocol (WLED/DDP, USB HID, Philips Hue)
//! implements [`DeviceBackend`] for communication and [`DevicePlugin`] for
//! lifecycle registration with the engine.

use std::sync::Arc;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::types::device::{DeviceId, DeviceInfo};

// ── BackendInfo ──────────────────────────────────────────────────────────

/// Static metadata describing a device backend.
///
/// Returned by [`DeviceBackend::info`] so the engine, CLI, and web UI
/// can display backend status without needing a live connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    /// Unique backend identifier used in configuration and feature gating.
    ///
    /// Examples: `"wled"`, `"hid"`, `"hue"`.
    pub id: String,

    /// Human-readable backend name for logging and UI display.
    ///
    /// Examples: `"WLED (DDP)"`, `"USB HID (PrismRGB)"`, `"Philips Hue"`.
    pub name: String,

    /// Short description of what this backend supports.
    pub description: String,
}

// ── DeviceBackend ────────────────────────────────────────────────────────

/// Core device communication trait.
///
/// Implementors provide hardware-specific discovery, connection management,
/// and color writes for a single transport protocol. Each backend manages
/// one or more physical devices over the same transport (e.g., all WLED
/// devices over UDP, all USB HID controllers over hidapi).
///
/// # Lifecycle
///
/// ```text
/// discover() -> connect() -> write_colors() ... write_colors() -> disconnect()
///     ^                          |                                      |
///     |                          +--- error (device lost) --------------+
///     +--- re-discover after reconnect backoff ---+
/// ```
///
/// # Thread safety
///
/// Backends are `Send + Sync` and called from the tokio runtime. Long-running
/// I/O (USB HID packet trains) must be dispatched to a dedicated task or
/// thread internally — `write_colors` returns immediately after queuing.
#[async_trait::async_trait]
pub trait DeviceBackend: Send + Sync {
    /// Static metadata about this backend.
    fn info(&self) -> BackendInfo;

    /// Scan for devices reachable via this backend's transport.
    ///
    /// Called at startup (full scan), on manual rescan, and after hot-plug
    /// events that hint a device may have appeared. Returns all devices
    /// currently reachable. The [`DiscoveryOrchestrator`](super::DiscoveryOrchestrator)
    /// handles deduplication across backends.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport is unavailable or the scan fails
    /// (e.g., USB subsystem inaccessible, network unreachable).
    async fn discover(&mut self) -> Result<Vec<DeviceInfo>>;

    /// Return refreshed metadata for a connected device, if available.
    ///
    /// Backends with dynamic topology or capabilities discovered during
    /// connect-time handshakes can override this to surface the final
    /// connected shape back to the registry.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is connected but metadata retrieval
    /// fails. The default implementation reports no refreshed metadata.
    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let _ = id;
        Ok(None)
    }

    /// Establish a connection to a specific device.
    ///
    /// For USB HID: opens the HID device file, runs initialization sequence.
    /// For WLED: verifies reachability via HTTP `/json/info`, caches metadata.
    /// For Hue: authenticates with the bridge, establishes Entertainment stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is not found, permissions are denied,
    /// or the transport-level connection fails.
    async fn connect(&mut self, id: &DeviceId) -> Result<()>;

    /// Cleanly disconnect from a device.
    ///
    /// For USB HID: sends the shutdown color, activates hardware mode, closes
    /// the device file. For WLED: no action needed (stateless UDP). For Hue:
    /// tears down Entertainment.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnect operation fails. Backends should
    /// still clean up internal state even on error.
    async fn disconnect(&mut self, id: &DeviceId) -> Result<()>;

    /// Push LED color data to a connected device.
    ///
    /// This is the hot-path method, called at up to 60fps. Implementations
    /// should not block — if the transport is slower than the frame rate,
    /// queue the frame internally and drop stale ones.
    ///
    /// The `colors` slice contains one RGB triplet per LED, ordered by zone
    /// then by LED index within the zone. The slice length should match the
    /// device's total LED count as reported by [`DeviceInfo::total_led_count`].
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()>;

    /// Push a JPEG-compressed display frame to a connected device, if supported.
    ///
    /// This is used by display-capable devices such as LCD pump heads that
    /// consume image data instead of per-LED color buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected, display output is
    /// unsupported, or the write fails.
    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let _ = (id, jpeg_data);
        bail!(
            "backend '{}' does not support device display output",
            self.info().id
        );
    }

    /// Push an owned JPEG-compressed display frame to a connected device.
    ///
    /// Backends with internal frame queues can override this to keep shared
    /// ownership instead of cloning the payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected, display output is
    /// unsupported, or the write fails.
    async fn write_display_frame_owned(
        &mut self,
        id: &DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        self.write_display_frame(id, jpeg_data.as_slice()).await
    }

    /// Adjust hardware brightness for a connected device, if supported.
    ///
    /// Backends that do not expose device-level brightness should return an
    /// unsupported error from the default implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected, brightness is unsupported,
    /// or the write fails.
    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let _ = (id, brightness);
        bail!(
            "backend '{}' does not support device brightness control",
            self.info().id
        );
    }

    /// Preferred output frame rate for a connected device.
    ///
    /// Backends can override this to inform the manager's per-device output
    /// queue pacing. Returning `None` keeps the manager default.
    #[must_use]
    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        let _ = id;
        None
    }

    /// Non-destructive health probe for a connected device.
    ///
    /// Backends that can cheaply verify connectivity without side effects
    /// (for example an HTTP ping or a cached keepalive timestamp) should
    /// override this. The default implementation assumes the device is
    /// healthy as long as the manager still considers it connected.
    ///
    /// # Errors
    ///
    /// Returns an error only if probing fails unexpectedly. Returning
    /// [`HealthStatus::Degraded`] or [`HealthStatus::Unreachable`] is the
    /// preferred way to signal a known-bad state.
    async fn health_check(&self, id: &DeviceId) -> Result<HealthStatus> {
        let _ = id;
        Ok(HealthStatus::Healthy)
    }
}

// ── HealthStatus ─────────────────────────────────────────────────────────

/// High-level connectivity state reported by [`DeviceBackend::health_check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Device is reachable and behaving normally.
    Healthy,
    /// Device is reachable but exhibiting latency, dropped frames, or
    /// other signs of partial failure.
    Degraded,
    /// Device is currently unreachable. Callers should treat the device
    /// as disconnected even if no explicit disconnect event has fired.
    Unreachable,
}

// ── DevicePlugin ─────────────────────────────────────────────────────────

/// Lifecycle hooks for backend initialization and teardown.
///
/// Plugins are the unit of registration for all extension points. A device
/// backend plugin typically:
///
/// 1. In [`build`](DevicePlugin::build): returns a boxed [`DeviceBackend`]
///    implementation for the engine to register.
/// 2. In [`ready`](DevicePlugin::ready): verifies runtime dependencies
///    (hidapi available, network reachable, credentials present).
/// 3. In [`teardown`](DevicePlugin::teardown): releases any OS-level resources.
pub trait DevicePlugin: Send + Sync {
    /// Human-readable plugin name for logging and display.
    fn name(&self) -> &str;

    /// Construct the backend instance for this plugin.
    ///
    /// Called once during daemon startup. The returned backend is registered
    /// with the engine and used for all subsequent discovery and communication.
    fn build(&self) -> Box<dyn DeviceBackend>;

    /// Verify that runtime dependencies are available.
    ///
    /// Called after all plugins have been built, before the render loop
    /// starts. Return `Err` to indicate the plugin cannot function (missing
    /// library, unreachable service, etc.). The daemon logs the error and
    /// continues without this plugin.
    ///
    /// Default: always ready.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime dependencies are missing or unavailable.
    fn ready(&self) -> Result<()> {
        Ok(())
    }

    /// Clean up resources on daemon shutdown.
    ///
    /// Called during graceful shutdown. Plugins should release any system
    /// resources, close file handles, and perform transport-specific cleanup.
    ///
    /// Default: no-op.
    fn teardown(&self) {}
}
