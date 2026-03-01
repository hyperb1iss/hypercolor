//! Core device backend and plugin traits.
//!
//! Every hardware protocol (WLED/DDP, USB HID, `OpenRGB` gRPC, Philips Hue)
//! implements [`DeviceBackend`] for communication and [`DevicePlugin`] for
//! lifecycle registration with the engine.

use anyhow::Result;
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
    /// Examples: `"wled"`, `"hid"`, `"openrgb"`, `"hue"`.
    pub id: String,

    /// Human-readable backend name for logging and UI display.
    ///
    /// Examples: `"WLED (DDP)"`, `"USB HID (PrismRGB)"`, `"OpenRGB (gRPC)"`.
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

    /// Establish a connection to a specific device.
    ///
    /// For USB HID: opens the HID device file, runs initialization sequence.
    /// For WLED: verifies reachability via HTTP `/json/info`, caches metadata.
    /// For OpenRGB: registers as a client for the specified controller.
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
    /// the device file. For WLED: no action needed (stateless UDP). For
    /// OpenRGB: releases the controller. For Hue: tears down Entertainment.
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
///    (hidapi available, network reachable, `OpenRGB` bridge process running).
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
