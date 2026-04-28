//! Driver-facing host boundary for modular Hypercolor network drivers.
//!
//! This crate defines the stable capability surface between the daemon-owned
//! runtime and network driver implementations. Drivers should depend on these
//! traits and shared request/response types instead of reaching into daemon
//! internals directly.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ApplyImpact, ControlActionResult, ControlChange,
    ControlSurfaceDocument, ControlSurfaceEvent, ControlValueMap,
};
use hypercolor_types::device::{
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DriverCapabilitySet,
    DriverModuleDescriptor, DriverModuleKind, DriverTransportKind,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub mod backend;
pub mod discovery;
pub mod net;
pub mod validation;

pub use backend::{BackendInfo, DeviceBackend, HealthStatus};
pub use discovery::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
pub use net::{CredentialStore, Credentials, MdnsBrowser, MdnsService};

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

    /// Convert this driver-facing descriptor into the host-wide module
    /// descriptor used by registry introspection.
    #[must_use]
    pub fn module_descriptor(&self) -> DriverModuleDescriptor {
        DriverModuleDescriptor {
            id: self.id.to_owned(),
            display_name: self.display_name.to_owned(),
            vendor_name: None,
            module_kind: DriverModuleKind::Network,
            transports: vec![self.transport.into()],
            capabilities: DriverCapabilitySet {
                config: false,
                discovery: self.supports_discovery,
                pairing: self.supports_pairing,
                backend_factory: true,
                protocol_catalog: false,
                runtime_cache: false,
                credentials: self.supports_pairing,
                presentation: false,
                controls: false,
            },
            api_schema_version: self.schema_version,
            config_version: 1,
            default_enabled: true,
        }
    }
}

impl From<DriverTransport> for DriverTransportKind {
    fn from(value: DriverTransport) -> Self {
        match value {
            DriverTransport::Network => Self::Network,
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

/// Target for a driver-owned control operation.
#[derive(Debug, Clone, Copy)]
pub enum ControlApplyTarget<'a> {
    /// Driver-module level controls.
    Driver {
        /// Driver module identifier.
        driver_id: &'a str,
        /// Current resolved driver config.
        config: DriverConfigView<'a>,
    },

    /// Controls for one tracked device.
    Device {
        /// Tracked device context.
        device: &'a TrackedDeviceCtx<'a>,
    },
}

/// Driver-normalized control changes that are safe to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedControlChanges {
    /// Changes accepted by validation.
    pub changes: Vec<ControlChange>,

    /// Dynamic impacts validation expects the apply step to perform.
    pub impacts: Vec<ApplyImpact>,
}

impl ValidatedControlChanges {
    /// Construct a validated bundle with no precomputed impacts.
    #[must_use]
    pub fn new(changes: Vec<ControlChange>) -> Self {
        Self {
            changes,
            impacts: Vec::new(),
        }
    }
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

/// Host persistence for driver-scoped control values.
#[async_trait]
pub trait DriverControlStore: Send + Sync {
    /// Load the current typed values for a driver surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    async fn load_driver_values(&self, driver_id: &str) -> Result<ControlValueMap>;

    /// Persist typed values for a driver surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    async fn save_driver_values(&self, driver_id: &str, values: ControlValueMap) -> Result<()>;
}

/// Host persistence for device-scoped control values.
#[async_trait]
pub trait DeviceControlStore: Send + Sync {
    /// Load the current typed values for a device surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap>;

    /// Persist typed values for a device surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()>;
}

/// Dynamic lifecycle operations available to control apply transactions.
#[async_trait]
pub trait DriverLifecycleActions: Send + Sync {
    /// Reconnect a device after a control change.
    ///
    /// # Errors
    ///
    /// Returns an error if reconnect scheduling fails.
    async fn reconnect_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool>;

    /// Request a discovery rescan for one driver.
    ///
    /// # Errors
    ///
    /// Returns an error if rescan scheduling fails.
    async fn rescan_driver(&self, driver_id: &str) -> Result<()>;
}

/// Output backend operations available to control apply transactions.
#[async_trait]
pub trait BackendRebindActions: Send + Sync {
    /// Rebind a driver backend after transport-level controls change.
    ///
    /// # Errors
    ///
    /// Returns an error if rebind scheduling fails.
    async fn rebind_backend(&self, driver_id: &str) -> Result<()>;
}

/// Host services available to driver-owned control providers.
pub trait DriverControlHost: Send + Sync {
    /// Driver-scoped typed value store.
    fn driver_config_store(&self) -> &dyn DriverControlStore;

    /// Device-scoped typed value store.
    fn device_config_store(&self) -> &dyn DeviceControlStore;

    /// Lifecycle actions allowed during apply.
    fn lifecycle(&self) -> &dyn DriverLifecycleActions;

    /// Backend rebind actions allowed during apply.
    fn backend_rebind(&self) -> &dyn BackendRebindActions;

    /// Publish a typed control-surface event.
    fn publish_control_event(&self, event: ControlSurfaceEvent);
}

/// Driver capability for typed dynamic control surfaces.
#[async_trait]
pub trait DriverControlProvider: Send + Sync {
    /// Build the optional driver-scoped control surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface cannot be built.
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>>;

    /// Build the optional device-scoped control surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface cannot be built.
    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>>;

    /// Validate and normalize a batch before mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if validation cannot complete.
    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges>;

    /// Apply a previously validated batch.
    ///
    /// # Errors
    ///
    /// Returns an error if the mutation fails.
    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse>;

    /// Invoke a typed action.
    ///
    /// # Errors
    ///
    /// Returns an error if the action fails.
    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult>;
}

/// Driver capability for persisting discovery/runtime hints between daemon runs.
#[async_trait]
pub trait DriverRuntimeCacheProvider: Send + Sync {
    /// Build a driver-scoped cache snapshot from host state.
    ///
    /// # Errors
    ///
    /// Returns an error if cache serialization fails.
    async fn snapshot(&self, host: &dyn DriverHost) -> Result<BTreeMap<String, serde_json::Value>>;
}

/// Factory and capability root for one modular network driver.
pub trait NetworkDriverFactory: Send + Sync {
    /// Static metadata about the driver.
    fn descriptor(&self) -> &'static DriverDescriptor;

    /// Host-wide module descriptor for this driver factory.
    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.capabilities.config = self.config().is_some();
        descriptor.capabilities.discovery = self.discovery().is_some();
        descriptor.capabilities.pairing = self.pairing().is_some();
        descriptor.capabilities.runtime_cache = self.runtime_cache().is_some();
        descriptor.capabilities.credentials = descriptor.capabilities.pairing;
        descriptor.capabilities.backend_factory = self.has_backend_factory();
        descriptor.capabilities.controls = self.controls().is_some();
        descriptor
    }

    /// Config capability, if the driver exposes host-readable defaults or validation.
    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        None
    }

    /// Whether this driver contributes a runtime backend for color output.
    fn has_backend_factory(&self) -> bool {
        true
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

    /// Control-surface capability, if supported.
    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        None
    }

    /// Runtime cache capability, if supported.
    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
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
    use std::path::PathBuf;

    use anyhow::Result;
    use tracing::warn;

    use crate::CredentialStore;
    use crate::DriverHost;
    use crate::validation::{validate_ip, validate_port};
    use hypercolor_types::device::DeviceId;

    /// Open the daemon's default credential store for native built-in drivers.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured data directory or credential file
    /// cannot be initialized.
    pub fn open_default_credential_store_blocking() -> Result<CredentialStore> {
        CredentialStore::open_blocking(&default_data_dir())
    }

    fn default_data_dir() -> PathBuf {
        const APP_DIR: &str = "hypercolor";

        #[cfg(target_os = "linux")]
        {
            std::env::var("XDG_DATA_HOME")
                .map_or_else(
                    |_| {
                        dirs::home_dir()
                            .expect("HOME must be set")
                            .join(".local/share")
                    },
                    PathBuf::from,
                )
                .join(APP_DIR)
        }

        #[cfg(not(target_os = "linux"))]
        {
            dirs::data_local_dir()
                .expect("data directory must be resolvable")
                .join(APP_DIR)
        }
    }

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
