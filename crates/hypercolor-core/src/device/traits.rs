//! Core device backend and plugin traits.
//!
//! Device backends implement [`DeviceBackend`] for communication and
//! [`DevicePlugin`] for lifecycle registration with the engine.

use anyhow::Result;

pub use hypercolor_driver_api::{BackendInfo, DeviceBackend, DeviceFrameSink, HealthStatus};

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
