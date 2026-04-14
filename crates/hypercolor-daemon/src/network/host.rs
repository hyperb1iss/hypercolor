use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::{
    DriverCredentialStore, DriverDiscoveryState, DriverHost, DriverRuntimeActions,
    DriverTrackedDevice,
};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::DisconnectReason;
use hypercolor_types::spatial::SpatialLayout;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{self, DiscoveryRuntime};
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::runtime_state;
use crate::scene_transactions::SceneTransactionQueue;

/// Daemon-owned host adapter passed to built-in drivers.
#[derive(Clone)]
pub struct DaemonDriverHost {
    device_registry: DeviceRegistry,
    backend_manager: Arc<Mutex<BackendManager>>,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    event_bus: Arc<HypercolorBus>,
    spatial_engine: Arc<RwLock<SpatialEngine>>,
    scene_manager: Arc<RwLock<SceneManager>>,
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
    scene_transactions: SceneTransactionQueue,
}

impl DaemonDriverHost {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        device_registry: DeviceRegistry,
        backend_manager: Arc<Mutex<BackendManager>>,
        lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
        reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
        event_bus: Arc<HypercolorBus>,
        spatial_engine: Arc<RwLock<SpatialEngine>>,
        scene_manager: Arc<RwLock<SceneManager>>,
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
        scene_transactions: SceneTransactionQueue,
    ) -> Self {
        Self {
            device_registry,
            backend_manager,
            lifecycle_manager,
            reconnect_tasks,
            event_bus,
            spatial_engine,
            scene_manager,
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
            scene_transactions,
        }
    }

    #[must_use]
    pub fn discovery_runtime(&self) -> DiscoveryRuntime {
        DiscoveryRuntime {
            device_registry: self.device_registry.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            lifecycle_manager: Arc::clone(&self.lifecycle_manager),
            reconnect_tasks: Arc::clone(&self.reconnect_tasks),
            event_bus: Arc::clone(&self.event_bus),
            spatial_engine: Arc::clone(&self.spatial_engine),
            scene_manager: Arc::clone(&self.scene_manager),
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
            scene_transactions: self.scene_transactions.clone(),
            task_spawner: tokio::runtime::Handle::current(),
        }
    }

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

#[async_trait]
impl DriverDiscoveryState for DaemonDriverHost {
    async fn tracked_devices(&self, backend_id: &str) -> Vec<DriverTrackedDevice> {
        let mut tracked_devices = Vec::new();

        for tracked in self.device_registry.list().await {
            let metadata = self
                .device_registry
                .metadata_for_id(&tracked.info.id)
                .await
                .unwrap_or_default();
            if discovery::backend_id_for_device(&tracked.info.family, Some(&metadata)) != backend_id
            {
                continue;
            }
            let fingerprint = self
                .device_registry
                .fingerprint_for_id(&tracked.info.id)
                .await;

            tracked_devices.push(DriverTrackedDevice {
                fingerprint,
                metadata,
                current_state: tracked.state,
                info: tracked.info,
            });
        }

        tracked_devices
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        match (driver_id, key) {
            ("wled", "probe_ips") => {
                let cached = runtime_state::load_wled_probe_ips(&self.runtime_state_path)?;
                Ok(Some(
                    serde_json::to_value(cached)
                        .context("failed to serialize cached WLED probe IPs")?,
                ))
            }
            ("wled", "probe_targets") => {
                let cached = runtime_state::load_wled_probe_targets(&self.runtime_state_path)?;
                Ok(Some(serde_json::to_value(cached).context(
                    "failed to serialize cached WLED probe targets",
                )?))
            }
            _ => Ok(None),
        }
    }
}

impl DriverHost for DaemonDriverHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        self
    }
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
