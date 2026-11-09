use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DeviceState};

use crate::{DeviceInputSink, DiscoveredDevice, DriverControlHost};

/// Read-only tracked-device view passed into pairing and auth-summary logic.
#[derive(Debug, Clone, Copy)]
pub struct TrackedDeviceCtx<'a> {
    pub device_id: DeviceId,
    pub info: &'a DeviceInfo,
    pub metadata: Option<&'a HashMap<String, String>>,
    pub current_state: &'a DeviceState,
}

/// Snapshot of a tracked device exposed to discovery-capable drivers.
#[derive(Debug, Clone)]
pub struct DriverTrackedDevice {
    pub info: DeviceInfo,
    pub metadata: HashMap<String, String>,
    pub fingerprint: Option<DeviceFingerprint>,
    pub current_state: DeviceState,
}

/// Driver-facing credential store abstraction.
#[async_trait]
pub trait DriverCredentialStore: Send + Sync {
    /// Retrieve a JSON credential payload for one driver-scoped key.
    ///
    /// # Errors
    ///
    /// Returns an error if the credential store is unavailable.
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>>;

    /// Persist a JSON credential payload for one driver-scoped key.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence fails.
    async fn set_json(&self, driver_id: &str, key: &str, value: serde_json::Value) -> Result<()>;

    /// Remove any credential payload for one driver-scoped key.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence fails.
    async fn remove(&self, driver_id: &str, key: &str) -> Result<()>;
}

/// Narrow lifecycle actions exposed to drivers.
#[async_trait]
pub trait DriverRuntimeActions: Send + Sync {
    /// Publish an optional refreshed discovery snapshot and schedule a reconnect.
    ///
    /// # Errors
    ///
    /// Returns an error if the host cannot schedule a reconnect.
    async fn request_reconnect(
        &self,
        _device_id: DeviceId,
        _backend_id: &str,
        _updated: Option<DiscoveredDevice>,
    ) -> Result<bool> {
        anyhow::bail!("host does not support driver-requested reconnects")
    }

    /// Ask the host to rediscover one driver's devices soon.
    ///
    /// Drivers whose transport pushes arrival and departure events call this
    /// so a device is adopted or retired without waiting for the periodic
    /// scan. The host merges overlapping requests into its pending scans.
    ///
    /// # Errors
    ///
    /// Returns an error if the host cannot schedule the scan.
    async fn request_discovery(&self, _driver_id: &str) -> Result<()> {
        anyhow::bail!("host does not support driver-requested discovery")
    }

    /// Best-effort immediate activation after pairing.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime activation fails.
    async fn activate_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool>;

    /// Best-effort disconnect after credential removal.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime disconnection fails.
    async fn disconnect_device(
        &self,
        device_id: DeviceId,
        backend_id: &str,
        will_retry: bool,
    ) -> Result<bool>;
}

/// Discovery-oriented host state exposed to drivers.
#[async_trait]
pub trait DriverDiscoveryState: Send + Sync {
    /// Return tracked devices previously seen for one driver module.
    async fn tracked_devices(&self, driver_id: &str) -> Vec<DriverTrackedDevice>;

    /// Load a driver-scoped cached JSON payload, if available.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache cannot be loaded.
    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>>;
}

/// Live, read-only view of one driver's configuration.
///
/// The host scopes each source to one driver, so a driver can only ever read
/// its own entry. A driver acting on devices that exist only because config
/// names them reads this at the moment it acts instead of trusting anything
/// it was told earlier.
pub trait DriverConfigSource: Send + Sync {
    /// This driver's entry as it is right now. A driver the host holds no
    /// entry for reads as [`DriverConfigEntry::default`], the same entry its
    /// backend is built from.
    fn current(&self) -> DriverConfigEntry;
}

/// Stable identity of one Hypercolor installation.
///
/// Drivers mix it into identities that must be unique per installation, such
/// as protocol sender ids, so two hosts with the same config never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallationId(Arc<str>);

impl InstallationId {
    /// Wrap the identity the host persists for this installation.
    #[must_use]
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    /// The identity as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Host services one driver's backend is built with.
///
/// A host cannot build a backend without supplying `config`,
/// `installation_id`, and `runtime`, so no driver needs a fallback for their
/// absence. `device_input` stays optional because only hardware with input
/// surfaces consumes it, and a host may have no input routing at all.
#[derive(Clone)]
pub struct DriverContext {
    /// Live view of this driver's configuration.
    pub config: Arc<dyn DriverConfigSource>,
    /// Identity of this installation.
    pub installation_id: InstallationId,
    /// Runtime actions a backend may retain for asynchronous lifecycle
    /// notifications. This handle must not keep the backend graph alive.
    pub runtime: Arc<dyn DriverRuntimeActions>,
    /// Sink for input from driver-owned devices, when the host routes input.
    pub device_input: Option<Arc<dyn DeviceInputSink>>,
}

/// Supplies the [`DriverContext`] each output backend is built with.
pub trait DriverContextProvider: Send + Sync {
    /// Build the context for one driver, scoped to that driver's config.
    fn driver_context(&self, driver_id: &str) -> DriverContext;
}

/// Host capabilities exposed to drivers.
pub trait DriverHost: Send + Sync {
    /// Access the shared credential store.
    fn credentials(&self) -> &dyn DriverCredentialStore;

    /// Access limited runtime lifecycle actions.
    fn runtime(&self) -> &dyn DriverRuntimeActions;

    /// Access discovery-oriented tracked state and caches.
    fn discovery_state(&self) -> &dyn DriverDiscoveryState;

    /// Access control-surface host services when the daemon supports them.
    fn control_host(&self) -> Option<&dyn DriverControlHost> {
        None
    }
}
