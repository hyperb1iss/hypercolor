//! Built-in network driver registry and host adapters.
//!
//! This module keeps the daemon-facing network-driver boundary in one place
//! while the protocol-specific implementations are still being extracted into
//! dedicated driver crates.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
#[cfg(feature = "hue")]
use hypercolor_core::device::hue::{DEFAULT_HUE_API_PORT, HueBackend, HueBridgeClient};
#[cfg(feature = "nanoleaf")]
use hypercolor_core::device::nanoleaf::{
    DEFAULT_NANOLEAF_API_PORT, NanoleafBackend, pair_device_with_status,
};
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_core::device::wled::{WledBackend, WledDeviceInfo, WledKnownTarget, WledProtocol};
use hypercolor_core::device::{
    BackendManager, DeviceBackend, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::spatial::SpatialEngine;
#[cfg(any(feature = "hue", feature = "nanoleaf"))]
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, PairDeviceOutcome, PairDeviceRequest,
    PairDeviceStatus, PairingCapability, PairingDescriptor, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_driver_api::{
    DriverCredentialStore, DriverDescriptor, DriverHost, DriverRuntimeActions, DriverTransport,
    NetworkDriverFactory,
};
use hypercolor_network::DriverRegistry;
#[cfg(feature = "hue")]
use hypercolor_types::config::HueConfig;
#[cfg(feature = "nanoleaf")]
use hypercolor_types::config::NanoleafConfig;
use hypercolor_types::config::{HypercolorConfig, WledProtocolConfig};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::DisconnectReason;
use hypercolor_types::spatial::SpatialLayout;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{self, DiscoveryRuntime};
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::runtime_state;

#[cfg(feature = "hue")]
const HUE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Press the link button on the Hue Bridge.",
    "Return here within 30 seconds.",
    "Click Pair Bridge.",
];

#[cfg(feature = "nanoleaf")]
const NANOLEAF_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Hold the Nanoleaf power button for 5-7 seconds.",
    "Wait for the controller to enter pairing mode.",
    "Click Pair Device.",
];

static WLED_DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("wled", "WLED", DriverTransport::Network, false, false);

#[cfg(feature = "hue")]
static HUE_DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, false, true);

#[cfg(feature = "nanoleaf")]
static NANOLEAF_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "nanoleaf",
    "Nanoleaf",
    DriverTransport::Network,
    false,
    true,
);

/// Daemon-owned host adapter passed to built-in drivers.
#[derive(Clone)]
pub struct DaemonDriverHost {
    device_registry: DeviceRegistry,
    backend_manager: Arc<Mutex<BackendManager>>,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    event_bus: Arc<HypercolorBus>,
    spatial_engine: Arc<RwLock<SpatialEngine>>,
    layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    layouts_path: PathBuf,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    attachment_registry: Arc<RwLock<AttachmentRegistry>>,
    attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,
    device_settings: Arc<RwLock<DeviceSettingsStore>>,
    runtime_state_path: PathBuf,
    usb_protocol_configs: UsbProtocolConfigStore,
    credential_store: Arc<CredentialStore>,
    discovery_in_progress: Arc<AtomicBool>,
}

impl DaemonDriverHost {
    /// Create a host adapter from the daemon's shared runtime state.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        device_registry: DeviceRegistry,
        backend_manager: Arc<Mutex<BackendManager>>,
        lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
        reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
        event_bus: Arc<HypercolorBus>,
        spatial_engine: Arc<RwLock<SpatialEngine>>,
        layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
        layouts_path: PathBuf,
        layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
        logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
        attachment_registry: Arc<RwLock<AttachmentRegistry>>,
        attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,
        device_settings: Arc<RwLock<DeviceSettingsStore>>,
        runtime_state_path: PathBuf,
        usb_protocol_configs: UsbProtocolConfigStore,
        credential_store: Arc<CredentialStore>,
        discovery_in_progress: Arc<AtomicBool>,
    ) -> Self {
        Self {
            device_registry,
            backend_manager,
            lifecycle_manager,
            reconnect_tasks,
            event_bus,
            spatial_engine,
            layouts,
            layouts_path,
            layout_auto_exclusions,
            logical_devices,
            attachment_registry,
            attachment_profiles,
            device_settings,
            runtime_state_path,
            usb_protocol_configs,
            credential_store,
            discovery_in_progress,
        }
    }

    /// Build a discovery runtime view for the current Tokio context.
    #[must_use]
    pub fn discovery_runtime(&self) -> DiscoveryRuntime {
        DiscoveryRuntime {
            device_registry: self.device_registry.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            lifecycle_manager: Arc::clone(&self.lifecycle_manager),
            reconnect_tasks: Arc::clone(&self.reconnect_tasks),
            event_bus: Arc::clone(&self.event_bus),
            spatial_engine: Arc::clone(&self.spatial_engine),
            layouts: Arc::clone(&self.layouts),
            layouts_path: self.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&self.layout_auto_exclusions),
            logical_devices: Arc::clone(&self.logical_devices),
            attachment_registry: Arc::clone(&self.attachment_registry),
            attachment_profiles: Arc::clone(&self.attachment_profiles),
            device_settings: Arc::clone(&self.device_settings),
            runtime_state_path: self.runtime_state_path.clone(),
            usb_protocol_configs: self.usb_protocol_configs.clone(),
            credential_store: Arc::clone(&self.credential_store),
            in_progress: Arc::clone(&self.discovery_in_progress),
            task_spawner: tokio::runtime::Handle::current(),
        }
    }

    /// Access the raw credential store for built-in backend construction.
    #[must_use]
    pub fn credential_store(&self) -> Arc<CredentialStore> {
        Arc::clone(&self.credential_store)
    }
}

#[async_trait]
impl DriverCredentialStore for DaemonDriverHost {
    async fn get_json(&self, key: &str) -> Result<Option<Value>> {
        let Some(credentials) = self.credential_store.get(key).await else {
            return Ok(None);
        };

        let value = match credentials {
            Credentials::HueBridge {
                api_key,
                client_key,
            } => serde_json::json!({
                "api_key": api_key,
                "client_key": client_key,
            }),
            Credentials::Nanoleaf { auth_token } => serde_json::json!({
                "auth_token": auth_token,
            }),
            Credentials::Wled {
                username,
                password,
                token,
            } => serde_json::json!({
                "username": username,
                "password": password,
                "token": token,
            }),
            Credentials::Custom { data, .. } => data,
        };

        Ok(Some(value))
    }

    async fn set_json(&self, key: &str, value: Value) -> Result<()> {
        let credentials = credentials_from_json(key, value)?;
        self.credential_store.store(key, credentials).await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.credential_store.remove(key).await
    }
}

#[async_trait]
impl DriverRuntimeActions for DaemonDriverHost {
    async fn activate_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let runtime = self.discovery_runtime();
        discovery::activate_pairable_device(&runtime, device_id, backend_id).await
    }

    async fn disconnect_device(
        &self,
        device_id: DeviceId,
        backend_id: &str,
        will_retry: bool,
    ) -> Result<bool> {
        let _ = backend_id;
        let runtime = self.discovery_runtime();
        discovery::disconnect_tracked_device(
            &runtime,
            device_id,
            DisconnectReason::User,
            will_retry,
        )
        .await
    }
}

impl DriverHost for DaemonDriverHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }
}

/// Build the daemon's compiled-in network driver registry.
///
/// # Errors
///
/// Returns an error if a built-in driver registration collides.
pub fn build_builtin_driver_registry(
    config: &HypercolorConfig,
    host: Arc<DaemonDriverHost>,
    runtime_state_path: PathBuf,
) -> Result<DriverRegistry> {
    let mut registry = DriverRegistry::new();
    registry.register(WledDriverFactory::new(config.clone(), runtime_state_path))?;
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &host;

    #[cfg(feature = "hue")]
    registry.register(HueDriverFactory::new(
        Arc::clone(&host),
        config.hue.clone(),
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(NanoleafDriverFactory::new(
        host,
        config.nanoleaf.clone(),
        config.discovery.mdns_enabled,
    ))?;

    Ok(registry)
}

#[derive(Clone)]
struct WledDriverFactory {
    config: HypercolorConfig,
    runtime_state_path: PathBuf,
}

impl WledDriverFactory {
    fn new(config: HypercolorConfig, runtime_state_path: PathBuf) -> Self {
        Self {
            config,
            runtime_state_path,
        }
    }
}

impl NetworkDriverFactory for WledDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &WLED_DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(build_wled_backend(
            &self.config,
            &self.runtime_state_path,
        ))))
    }
}

#[cfg(feature = "hue")]
#[derive(Clone)]
struct HueDriverFactory {
    host: Arc<DaemonDriverHost>,
    config: HueConfig,
    mdns_enabled: bool,
}

#[cfg(feature = "hue")]
impl HueDriverFactory {
    fn new(host: Arc<DaemonDriverHost>, config: HueConfig, mdns_enabled: bool) -> Self {
        Self {
            host,
            config,
            mdns_enabled,
        }
    }
}

#[cfg(feature = "hue")]
impl NetworkDriverFactory for HueDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &HUE_DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(HueBackend::with_mdns_enabled(
            self.config.clone(),
            self.host.credential_store(),
            self.mdns_enabled,
        ))))
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
    }
}

#[cfg(feature = "hue")]
#[async_trait]
impl PairingCapability for HueDriverFactory {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let _ = host;
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured =
            hue_credentials_present(&self.host.credential_store, device.metadata).await;

        Some(DeviceAuthSummary {
            state: if last_error.is_some() {
                DeviceAuthState::Error
            } else if configured {
                DeviceAuthState::Configured
            } else {
                DeviceAuthState::Required
            },
            can_pair: true,
            descriptor: Some(hue_pairing_descriptor()),
            last_error,
        })
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        if hue_credentials_present(&self.host.credential_store, device.metadata).await {
            let activated =
                activate_if_requested(host, request.activate_after_pair, device.device_id, "hue")
                    .await;
            let message = if activated {
                "Hue bridge credentials are already configured and the device was activated."
            } else {
                "Hue bridge credentials are already configured."
            };
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::AlreadyPaired,
                message: message.to_owned(),
                auth_state: DeviceAuthState::Configured,
                activated,
            });
        }

        let Some(bridge_ip) = network_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Hue bridge is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let bridge_port = device
            .metadata
            .and_then(|values| values.get("api_port"))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_HUE_API_PORT);

        match pair_hue_bridge_at_ip(&self.host.credential_store, bridge_ip, bridge_port).await? {
            Some(_) => {
                let activated = activate_if_requested(
                    host,
                    request.activate_after_pair,
                    device.device_id,
                    "hue",
                )
                .await;
                let message = if activated {
                    "Hue bridge paired and activated."
                } else {
                    "Hue bridge paired. Credentials are stored."
                };
                Ok(PairDeviceOutcome {
                    status: PairDeviceStatus::Paired,
                    message: message.to_owned(),
                    auth_state: DeviceAuthState::Configured,
                    activated,
                })
            }
            None => Ok(PairDeviceOutcome {
                status: PairDeviceStatus::ActionRequired,
                message: "Press the Hue bridge link button, then retry pairing.".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            }),
        }
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome> {
        clear_hue_credentials(&self.host.credential_store, device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "hue").await;

        Ok(ClearPairingOutcome {
            message: "Hue bridge credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

#[cfg(feature = "nanoleaf")]
#[derive(Clone)]
struct NanoleafDriverFactory {
    host: Arc<DaemonDriverHost>,
    config: NanoleafConfig,
    mdns_enabled: bool,
}

#[cfg(feature = "nanoleaf")]
impl NanoleafDriverFactory {
    fn new(host: Arc<DaemonDriverHost>, config: NanoleafConfig, mdns_enabled: bool) -> Self {
        Self {
            host,
            config,
            mdns_enabled,
        }
    }
}

#[cfg(feature = "nanoleaf")]
impl NetworkDriverFactory for NanoleafDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &NANOLEAF_DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(NanoleafBackend::with_mdns_enabled(
            self.config.clone(),
            self.host.credential_store(),
            self.mdns_enabled,
        ))))
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
    }
}

#[cfg(feature = "nanoleaf")]
#[async_trait]
impl PairingCapability for NanoleafDriverFactory {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let _ = host;
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured =
            nanoleaf_credentials_present(&self.host.credential_store, device.metadata).await;

        Some(DeviceAuthSummary {
            state: if last_error.is_some() {
                DeviceAuthState::Error
            } else if configured {
                DeviceAuthState::Configured
            } else {
                DeviceAuthState::Required
            },
            can_pair: true,
            descriptor: Some(nanoleaf_pairing_descriptor()),
            last_error,
        })
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        if nanoleaf_credentials_present(&self.host.credential_store, device.metadata).await {
            let activated = activate_if_requested(
                host,
                request.activate_after_pair,
                device.device_id,
                "nanoleaf",
            )
            .await;
            let message = if activated {
                "Nanoleaf credentials are already configured and the device was activated."
            } else {
                "Nanoleaf credentials are already configured."
            };
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::AlreadyPaired,
                message: message.to_owned(),
                auth_state: DeviceAuthState::Configured,
                activated,
            });
        }

        let Some(device_ip) = network_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Nanoleaf device is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let api_port = device
            .metadata
            .and_then(|values| values.get("api_port"))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_NANOLEAF_API_PORT);

        match pair_nanoleaf_device_at_ip(&self.host.credential_store, device_ip, api_port).await? {
            Some(_) => {
                let activated = activate_if_requested(
                    host,
                    request.activate_after_pair,
                    device.device_id,
                    "nanoleaf",
                )
                .await;
                let message = if activated {
                    "Nanoleaf device paired and activated."
                } else {
                    "Nanoleaf device paired. Credentials are stored."
                };
                Ok(PairDeviceOutcome {
                    status: PairDeviceStatus::Paired,
                    message: message.to_owned(),
                    auth_state: DeviceAuthState::Configured,
                    activated,
                })
            }
            None => Ok(PairDeviceOutcome {
                status: PairDeviceStatus::ActionRequired,
                message: "Hold the Nanoleaf power button for 5-7 seconds, then retry pairing."
                    .to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            }),
        }
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome> {
        clear_nanoleaf_credentials(&self.host.credential_store, device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "nanoleaf").await;

        Ok(ClearPairingOutcome {
            message: "Nanoleaf credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

#[cfg(feature = "hue")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHuePairingResult {
    pub device_key: Option<String>,
    pub name: Option<String>,
}

#[cfg(feature = "nanoleaf")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNanoleafPairingResult {
    pub device_key: String,
    pub name: String,
}

#[cfg(feature = "hue")]
pub async fn pair_hue_bridge_at_ip(
    credential_store: &CredentialStore,
    bridge_ip: IpAddr,
    bridge_port: u16,
) -> Result<Option<StoredHuePairingResult>> {
    let client = HueBridgeClient::with_port(bridge_ip, bridge_port);
    let Some(pair_result) = client.pair_with_status("hypercolor").await? else {
        return Ok(None);
    };

    let bridge_identity = client.bridge_identity().await.ok();
    let credentials = Credentials::HueBridge {
        api_key: pair_result.api_key,
        client_key: pair_result.client_key,
    };

    if let Some(identity) = bridge_identity.as_ref() {
        credential_store
            .store(&format!("hue:{}", identity.bridge_id), credentials.clone())
            .await?;
    }
    credential_store
        .store(&format!("hue:ip:{bridge_ip}"), credentials)
        .await?;

    Ok(Some(StoredHuePairingResult {
        device_key: bridge_identity
            .as_ref()
            .map(|identity| identity.bridge_id.clone()),
        name: bridge_identity
            .as_ref()
            .map(|identity| identity.name.clone()),
    }))
}

#[cfg(feature = "nanoleaf")]
pub async fn pair_nanoleaf_device_at_ip(
    credential_store: &CredentialStore,
    device_ip: IpAddr,
    api_port: u16,
) -> Result<Option<StoredNanoleafPairingResult>> {
    let Some(pair_result) = pair_device_with_status(device_ip, api_port).await? else {
        return Ok(None);
    };

    let credentials = Credentials::Nanoleaf {
        auth_token: pair_result.auth_token,
    };
    credential_store
        .store(
            &format!("nanoleaf:{}", pair_result.device_key),
            credentials.clone(),
        )
        .await?;
    credential_store
        .store(&format!("nanoleaf:ip:{device_ip}"), credentials)
        .await?;

    Ok(Some(StoredNanoleafPairingResult {
        device_key: pair_result.device_key,
        name: pair_result.name,
    }))
}

pub fn build_wled_backend(config: &HypercolorConfig, runtime_state_path: &Path) -> WledBackend {
    let mut known_ips: HashSet<_> = config.wled.known_ips.iter().copied().collect();
    match runtime_state::load_wled_probe_ips(runtime_state_path) {
        Ok(cached_ips) => {
            known_ips.extend(cached_ips);
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "Failed to load cached WLED probe IPs; falling back to config only"
            );
        }
    }

    let mut resolved_known_ips: Vec<_> = known_ips.into_iter().collect();
    resolved_known_ips.sort_unstable();

    let mut backend =
        WledBackend::with_mdns_fallback(resolved_known_ips, config.discovery.mdns_enabled);
    match runtime_state::load_wled_probe_targets(runtime_state_path) {
        Ok(cached_targets) => {
            for target in cached_targets {
                let Some((device_id, ip, info)) = cached_wled_backend_seed(&target) else {
                    continue;
                };
                backend.remember_device(device_id, ip, info);
            }
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "Failed to load cached WLED identity hints; backend will rely on fresh probing"
            );
        }
    }
    let protocol = match config.wled.default_protocol {
        WledProtocolConfig::Ddp => WledProtocol::Ddp,
        WledProtocolConfig::E131 => WledProtocol::E131,
    };
    backend.set_protocol(protocol);
    backend.set_realtime_http_enabled(config.wled.realtime_http_enabled);
    backend.set_dedup_threshold(config.wled.dedup_threshold);
    backend
}

fn credentials_from_json(key: &str, value: Value) -> Result<Credentials> {
    let backend_id = key.split(':').next().unwrap_or("custom");
    match backend_id {
        "hue" => {
            let api_key = value
                .get("api_key")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .context("Hue credentials are missing api_key")?;
            let client_key = value
                .get("client_key")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .context("Hue credentials are missing client_key")?;
            Ok(Credentials::HueBridge {
                api_key,
                client_key,
            })
        }
        "nanoleaf" => {
            let auth_token = value
                .get("auth_token")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .context("Nanoleaf credentials are missing auth_token")?;
            Ok(Credentials::Nanoleaf { auth_token })
        }
        "wled" => {
            let username = value
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let password = value
                .get("password")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let token = value
                .get("token")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if username.is_none() && password.is_none() && token.is_none() {
                bail!("WLED credentials require at least one configured field");
            }
            Ok(Credentials::Wled {
                username,
                password,
                token,
            })
        }
        _ => Ok(Credentials::Custom {
            backend_id: backend_id.to_owned(),
            data: value,
        }),
    }
}

#[cfg(any(feature = "hue", feature = "nanoleaf"))]
async fn activate_if_requested(
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

#[cfg(any(feature = "hue", feature = "nanoleaf"))]
async fn disconnect_after_unpair(
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

fn cached_wled_backend_seed(
    target: &WledKnownTarget,
) -> Option<(DeviceId, IpAddr, WledDeviceInfo)> {
    let fingerprint = target.fingerprint.clone()?;
    let name = target.name.clone()?;
    let led_count = target.led_count?;
    let fps = target
        .max_fps
        .map_or(60, |value| u8::try_from(value).unwrap_or(u8::MAX));

    Some((
        fingerprint.stable_device_id(),
        target.ip,
        WledDeviceInfo {
            firmware_version: target
                .firmware_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            build_id: 0,
            mac: fingerprint
                .0
                .strip_prefix("net:")
                .filter(|value| !value.starts_with("wled:"))
                .unwrap_or_default()
                .to_owned(),
            name,
            led_count: u16::try_from(led_count).unwrap_or(u16::MAX),
            rgbw: target.rgbw.unwrap_or(false),
            max_segments: 1,
            fps,
            power_draw_ma: 0,
            max_power_ma: 0,
            free_heap: 0,
            uptime_secs: 0,
            arch: "unknown".to_owned(),
            is_wifi: true,
            effect_count: 0,
            palette_count: 0,
        },
    ))
}

#[cfg(any(feature = "hue", feature = "nanoleaf"))]
fn metadata_value<'a>(metadata: Option<&'a HashMap<String, String>>, key: &str) -> Option<&'a str> {
    metadata
        .and_then(|values| values.get(key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(any(feature = "hue", feature = "nanoleaf"))]
fn network_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
    metadata
        .and_then(|values| values.get("ip"))
        .and_then(|value| value.parse::<IpAddr>().ok())
}

#[cfg(any(feature = "hue", feature = "nanoleaf"))]
fn push_lookup_key(keys: &mut Vec<String>, key: String) {
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}

#[cfg(feature = "hue")]
fn hue_credential_keys(metadata: Option<&HashMap<String, String>>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(bridge_id) = metadata_value(metadata, "bridge_id") {
        push_lookup_key(&mut keys, format!("hue:{bridge_id}"));
    }
    if let Some(ip) = metadata_value(metadata, "ip") {
        push_lookup_key(&mut keys, format!("hue:ip:{ip}"));
    }
    keys
}

#[cfg(feature = "hue")]
async fn hue_credentials_present(
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> bool {
    for key in hue_credential_keys(metadata) {
        if matches!(
            credential_store.get(&key).await,
            Some(Credentials::HueBridge { .. })
        ) {
            return true;
        }
    }
    false
}

#[cfg(feature = "hue")]
async fn clear_hue_credentials(
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<()> {
    for key in hue_credential_keys(metadata) {
        credential_store.remove(&key).await?;
    }
    Ok(())
}

#[cfg(feature = "nanoleaf")]
fn nanoleaf_credential_keys(metadata: Option<&HashMap<String, String>>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(device_key) = metadata_value(metadata, "device_key") {
        push_lookup_key(&mut keys, format!("nanoleaf:{device_key}"));
    }
    if let Some(ip) = metadata_value(metadata, "ip") {
        push_lookup_key(&mut keys, format!("nanoleaf:ip:{ip}"));
    }
    keys
}

#[cfg(feature = "nanoleaf")]
async fn nanoleaf_credentials_present(
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> bool {
    for key in nanoleaf_credential_keys(metadata) {
        if matches!(
            credential_store.get(&key).await,
            Some(Credentials::Nanoleaf { .. })
        ) {
            return true;
        }
    }
    false
}

#[cfg(feature = "nanoleaf")]
async fn clear_nanoleaf_credentials(
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<()> {
    for key in nanoleaf_credential_keys(metadata) {
        credential_store.remove(&key).await?;
    }
    Ok(())
}

#[cfg(feature = "hue")]
fn hue_pairing_descriptor() -> PairingDescriptor {
    PairingDescriptor {
        kind: PairingFlowKind::PhysicalAction,
        title: "Pair Hue Bridge".to_owned(),
        instructions: HUE_PAIRING_INSTRUCTIONS
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
        action_label: "Pair Bridge".to_owned(),
        fields: Vec::new(),
    }
}

#[cfg(feature = "nanoleaf")]
fn nanoleaf_pairing_descriptor() -> PairingDescriptor {
    PairingDescriptor {
        kind: PairingFlowKind::PhysicalAction,
        title: "Pair Nanoleaf Device".to_owned(),
        instructions: NANOLEAF_PAIRING_INSTRUCTIONS
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
        action_label: "Pair Device".to_owned(),
        fields: Vec::new(),
    }
}
