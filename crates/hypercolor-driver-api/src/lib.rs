//! Driver-facing host boundary for modular Hypercolor network drivers.
//!
//! This crate defines the stable capability surface between the daemon-owned
//! runtime and network driver implementations. Drivers should depend on these
//! traits and shared request/response types instead of reaching into daemon
//! internals directly.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypercolor_core::device::{DeviceBackend, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DeviceState};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub mod validation;

/// Current driver API schema version. Bump this on any breaking change to
/// the [`DriverHost`] trait, [`DriverDescriptor`] fields, or related types.
pub const DRIVER_API_SCHEMA_VERSION: u32 = 1;

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
    /// Schema version of the driver API contract this driver implements.
    /// The host rejects load if this does not match [`DRIVER_API_SCHEMA_VERSION`].
    pub schema_version: u32,
}

impl DriverDescriptor {
    /// Create a new static descriptor tagged with the current
    /// [`DRIVER_API_SCHEMA_VERSION`].
    #[must_use]
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        transport: DriverTransport,
        supports_discovery: bool,
        supports_pairing: bool,
    ) -> Self {
        Self::with_schema_version(
            id,
            display_name,
            transport,
            supports_discovery,
            supports_pairing,
            DRIVER_API_SCHEMA_VERSION,
        )
    }

    /// Create a new static descriptor with an explicit schema version.
    ///
    /// Out-of-tree drivers should prefer [`DriverDescriptor::new`] so they
    /// automatically pick up the current schema version at compile time.
    /// This constructor exists so the host can synthesise descriptors at
    /// other versions in tests and version-mismatch error paths.
    #[must_use]
    pub const fn with_schema_version(
        id: &'static str,
        display_name: &'static str,
        transport: DriverTransport,
        supports_discovery: bool,
        supports_pairing: bool,
        schema_version: u32,
    ) -> Self {
        Self {
            id,
            display_name,
            transport,
            supports_discovery,
            supports_pairing,
            schema_version,
        }
    }
}

/// Summary of whether a device needs authentication before it can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PairingFlowKind {
    /// User must perform a physical action, then confirm.
    PhysicalAction,
    /// User must submit one or more credentials.
    CredentialsForm,
}

/// Descriptor for one pairing form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingFieldDescriptor {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Backend-provided pairing UI/CLI descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingDescriptor {
    pub kind: PairingFlowKind,
    pub title: String,
    pub instructions: Vec<String>,
    pub action_label: String,
    #[serde(default)]
    pub fields: Vec<PairingFieldDescriptor>,
}

/// Driver-owned authentication summary for one tracked device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

impl From<DiscoveredDevice> for DriverDiscoveredDevice {
    fn from(device: DiscoveredDevice) -> Self {
        Self {
            info: device.info,
            fingerprint: device.fingerprint,
            metadata: device.metadata,
            connect_behavior: device.connect_behavior,
        }
    }
}

/// Discovery result for one driver execution.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    pub devices: Vec<DriverDiscoveredDevice>,
}

/// Read-only resolved config for one driver.
#[derive(Debug, Clone, Copy)]
pub struct DriverConfigView<'a> {
    pub driver_id: &'a str,
    pub entry: &'a DriverConfigEntry,
}

impl DriverConfigView<'_> {
    /// Whether the host should activate this driver.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.entry.enabled
    }

    /// Deserialize this driver's settings into a typed private config.
    ///
    /// # Errors
    ///
    /// Returns an error when the settings payload does not match `T`.
    pub fn parse_settings<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let settings = serde_json::Value::Object(
            self.entry
                .settings
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        serde_json::from_value(settings)
            .with_context(|| format!("invalid config for driver '{}'", self.driver_id))
    }
}

/// Optional driver-owned configuration metadata and validation.
pub trait DriverConfigProvider: Send + Sync {
    /// Default config entry for this driver.
    fn default_config(&self) -> DriverConfigEntry;

    /// Validate a resolved config entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver cannot accept the config payload.
    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()>;
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
        config: DriverConfigView<'_>,
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

    /// Config capability, if the driver exposes host-readable defaults or validation.
    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        None
    }

    /// Build the optional runtime backend used for color output.
    ///
    /// Returning `Ok(None)` allows capability-only drivers, though built-in
    /// Hypercolor network drivers are expected to contribute a backend.
    ///
    /// # Errors
    ///
    /// Returns an error if backend construction fails.
    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>>;

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

/// Shared helper utilities for network drivers.
pub mod support {
    use std::collections::HashMap;
    use std::net::IpAddr;

    use tracing::warn;

    use crate::DriverHost;
    use crate::validation::{validate_ip, validate_port};
    use hypercolor_types::device::DeviceId;

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

    /// Extract a trimmed metadata value if present and non-empty.
    #[must_use]
    pub fn metadata_value<'a>(
        metadata: Option<&'a HashMap<String, String>>,
        key: &str,
    ) -> Option<&'a str> {
        metadata
            .and_then(|values| values.get(key))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Parse a routable network IP from the standard `ip` metadata key.
    ///
    /// Returns `None` if the key is missing, unparseable, or points at a
    /// non-routable address such as loopback, multicast, or broadcast. See
    /// [`crate::validation::validate_ip`] for the full list of rejected ranges.
    #[must_use]
    pub fn network_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
        metadata
            .and_then(|values| values.get("ip"))
            .and_then(|value| value.parse::<IpAddr>().ok())
            .and_then(|ip| validate_ip(ip).ok())
    }

    /// Parse a validated port from a metadata key.
    ///
    /// Returns `None` if the key is missing, unparseable, or fails
    /// [`crate::validation::validate_port`] (port 0 or privileged ports).
    #[must_use]
    pub fn network_port_from_metadata(
        metadata: Option<&HashMap<String, String>>,
        key: &str,
    ) -> Option<u16> {
        metadata
            .and_then(|values| values.get(key))
            .and_then(|value| value.parse::<u16>().ok())
            .and_then(|port| validate_port(port).ok())
    }

    /// Push a credential lookup key if it is not already present.
    pub fn push_lookup_key(keys: &mut Vec<String>, key: String) {
        if !keys.iter().any(|existing| existing == &key) {
            keys.push(key);
        }
    }
}
