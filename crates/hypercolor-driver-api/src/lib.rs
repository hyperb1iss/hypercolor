//! Driver-facing host boundary for modular Hypercolor network drivers.
//!
//! This crate defines the stable capability surface between the daemon-owned
//! runtime and network driver implementations. Drivers should depend on these
//! traits and shared request/response types instead of reaching into daemon
//! internals directly.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::{DeviceBackend, DiscoveryConnectBehavior};
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DeviceState};
use serde::{Deserialize, Serialize};

/// Stable transport category for a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverTransport {
    /// A driver that communicates with devices over IP networking.
    Network,
}

/// Static metadata about a modular driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverDescriptor {
    /// Stable machine-readable ID, for example `wled` or `hue`.
    pub id: &'static str,
    /// Human-readable driver name for logs and UI.
    pub display_name: &'static str,
    /// Transport class used by this driver.
    pub transport: DriverTransport,
    /// Whether the driver contributes discovery support.
    pub supports_discovery: bool,
    /// Whether the driver contributes pairing support.
    pub supports_pairing: bool,
}

impl DriverDescriptor {
    /// Create a new static descriptor.
    #[must_use]
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        transport: DriverTransport,
        supports_discovery: bool,
        supports_pairing: bool,
    ) -> Self {
        Self {
            id,
            display_name,
            transport,
            supports_discovery,
            supports_pairing,
        }
    }
}

/// Summary of whether a device needs authentication before it can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthState {
    /// Device does not require credentials.
    Open,
    /// Device requires credentials and none are stored.
    Required,
    /// Credentials are present and should be used for connections.
    Configured,
    /// Credentials exist, but the driver knows they are invalid or stale.
    Error,
}

/// How the UI or CLI should present a pairing flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingFlowKind {
    /// User must perform a physical action, then confirm.
    PhysicalAction,
    /// User must submit one or more credentials.
    CredentialsForm,
}

/// Descriptor for one pairing form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingFieldDescriptor {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Backend-provided pairing UI/CLI descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingDescriptor {
    pub kind: PairingFlowKind,
    pub title: String,
    pub instructions: Vec<String>,
    pub action_label: String,
    #[serde(default)]
    pub fields: Vec<PairingFieldDescriptor>,
}

/// Driver-owned authentication summary for one tracked device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAuthSummary {
    pub state: DeviceAuthState,
    pub can_pair: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<PairingDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Generic pairing request submitted by the daemon API or CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PairDeviceRequest {
    /// Driver-defined values for credential-based flows.
    #[serde(default)]
    pub values: HashMap<String, String>,
    /// Whether to attempt immediate post-pair activation.
    #[serde(default = "bool_true")]
    pub activate_after_pair: bool,
}

/// High-level result category for a pairing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairDeviceStatus {
    Paired,
    ActionRequired,
    AlreadyPaired,
    InvalidInput,
}

/// Driver-owned result of a pairing action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairDeviceOutcome {
    pub status: PairDeviceStatus,
    pub message: String,
    pub auth_state: DeviceAuthState,
    pub activated: bool,
}

/// Driver-owned result of clearing pairing credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearPairingOutcome {
    pub message: String,
    pub auth_state: DeviceAuthState,
    pub disconnected: bool,
}

/// Discovery request normalized by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
}

/// Driver-produced discovery payload for one tracked device.
#[derive(Debug, Clone)]
pub struct DriverDiscoveredDevice {
    pub info: DeviceInfo,
    pub fingerprint: DeviceFingerprint,
    pub metadata: HashMap<String, String>,
    pub connect_behavior: DiscoveryConnectBehavior,
}

/// Discovery result for one driver execution.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    pub devices: Vec<DriverDiscoveredDevice>,
}

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
    /// Retrieve a JSON credential payload for one key.
    ///
    /// # Errors
    ///
    /// Returns an error if the credential store is unavailable.
    async fn get_json(&self, key: &str) -> Result<Option<serde_json::Value>>;

    /// Persist a JSON credential payload for one key.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence fails.
    async fn set_json(&self, key: &str, value: serde_json::Value) -> Result<()>;

    /// Remove any credential payload for one key.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence fails.
    async fn remove(&self, key: &str) -> Result<()>;
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
    /// Return tracked devices previously seen for one backend.
    async fn tracked_devices(&self, backend_id: &str) -> Vec<DriverTrackedDevice>;

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
}

/// Driver capability for device discovery.
#[async_trait]
pub trait DiscoveryCapability: Send + Sync {
    /// Discover reachable devices for this driver.
    ///
    /// # Errors
    ///
    /// Returns an error if discovery fails.
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResult>;
}

/// Driver capability for pairing and auth summaries.
#[async_trait]
pub trait PairingCapability: Send + Sync {
    /// Summarize auth state for one tracked device.
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary>;

    /// Pair a tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the pair flow fails unexpectedly.
    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome>;

    /// Clear stored credentials for a tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the credential clear flow fails.
    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome>;
}

/// Factory and capability root for one modular network driver.
pub trait NetworkDriverFactory: Send + Sync {
    /// Static metadata about the driver.
    fn descriptor(&self) -> &'static DriverDescriptor;

    /// Build the optional runtime backend used for color output.
    ///
    /// Returning `Ok(None)` allows capability-only drivers, though built-in
    /// Hypercolor network drivers are expected to contribute a backend.
    ///
    /// # Errors
    ///
    /// Returns an error if backend construction fails.
    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>>;

    /// Discovery capability, if supported.
    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        None
    }

    /// Pairing capability, if supported.
    fn pairing(&self) -> Option<&dyn PairingCapability> {
        None
    }
}

const fn bool_true() -> bool {
    true
}
