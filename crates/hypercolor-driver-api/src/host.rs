use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DeviceState};

use crate::DriverControlHost;

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
