use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::{Result, bail};
use async_trait::async_trait;
use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    BackendRebindActions, DeviceControlStore, DriverConfigView, DriverControlHost,
    DriverControlStore, DriverCredentialStore, DriverDiscoveryState, DriverHost,
    DriverLifecycleActions, DriverRuntimeActions, DriverTrackedDevice,
};
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::controls::{ControlSurfaceEvent, ControlValue, ControlValueMap};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use hypercolor_types::spatial::SpatialLayout;
use serde_json::{Number, Value};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::warn;

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
    driver_registry: Arc<DriverRegistry>,
    discovery_in_progress: Arc<AtomicBool>,
    scene_transactions: SceneTransactionQueue,
    config_manager: Option<Arc<ConfigManager>>,
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
        driver_registry: Arc<DriverRegistry>,
        discovery_in_progress: Arc<AtomicBool>,
        scene_transactions: SceneTransactionQueue,
        config_manager: Option<Arc<ConfigManager>>,
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
            driver_registry,
            discovery_in_progress,
            scene_transactions,
            config_manager,
        }
    }

    #[must_use]
    pub fn with_config_manager(&self, config_manager: Option<Arc<ConfigManager>>) -> Self {
        let mut host = self.clone();
        host.config_manager = config_manager;
        host
    }

    #[must_use]
    pub fn with_driver_registry(&self, driver_registry: Arc<DriverRegistry>) -> Self {
        let mut host = self.clone();
        host.driver_registry = driver_registry;
        host
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

    fn current_config(&self) -> Arc<HypercolorConfig> {
        self.config_manager.as_ref().map_or_else(
            || Arc::new(HypercolorConfig::default()),
            |manager| Arc::clone(&manager.get()),
        )
    }

    async fn device_control_settings_key(&self, device_id: DeviceId) -> String {
        self.device_registry
            .fingerprint_for_id(&device_id)
            .await
            .map_or_else(
                || device_id.to_string(),
                |fingerprint| fingerprint.to_string(),
            )
    }
}

#[async_trait]
impl DriverCredentialStore for DaemonDriverHost {
    async fn get_json(&self, key: &str) -> Result<Option<Value>> {
        Ok(self.credential_store.get_json(key).await)
    }

    async fn set_json(&self, key: &str, value: Value) -> Result<()> {
        self.credential_store.store_json(key, value).await
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
    async fn tracked_devices(&self, driver_id: &str) -> Vec<DriverTrackedDevice> {
        let mut tracked_devices = Vec::new();

        for tracked in self.device_registry.list().await {
            let metadata = self
                .device_registry
                .metadata_for_id(&tracked.info.id)
                .await
                .unwrap_or_default();
            if tracked.info.origin.driver_id != driver_id {
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
        runtime_state::load_driver_cached_json(&self.runtime_state_path, driver_id, key)
            .map_err(Into::into)
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

    fn control_host(&self) -> Option<&dyn DriverControlHost> {
        Some(self)
    }
}

#[async_trait]
impl DriverControlStore for DaemonDriverHost {
    async fn load_driver_values(&self, driver_id: &str) -> Result<ControlValueMap> {
        let Some(manager) = &self.config_manager else {
            bail!("config manager unavailable");
        };
        let config = manager.get();
        let Some(entry) = config.drivers.get(driver_id) else {
            return Ok(ControlValueMap::new());
        };
        Ok(entry
            .settings
            .iter()
            .map(|(key, value)| (key.clone(), config_json_to_control_value(value)))
            .collect())
    }

    async fn save_driver_values(&self, driver_id: &str, values: ControlValueMap) -> Result<()> {
        let Some(manager) = &self.config_manager else {
            bail!("config manager unavailable");
        };
        let current = manager.get();
        let mut config = (**current).clone();
        let entry = config.drivers.entry(driver_id.to_owned()).or_default();
        for (key, value) in values {
            entry
                .settings
                .insert(key, control_value_to_config_json(value));
        }
        if let Some(driver) = self.driver_registry.get(driver_id)
            && let Some(provider) = driver.config()
        {
            provider.validate_config(entry)?;
        }
        manager.update(config);
        manager.save()
    }
}

#[async_trait]
impl DeviceControlStore for DaemonDriverHost {
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap> {
        let key = self.device_control_settings_key(device_id).await;
        let store = self.device_settings.read().await;
        Ok(store.driver_control_values_for_key(&key))
    }

    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()> {
        let key = self.device_control_settings_key(device_id).await;
        let mut store = self.device_settings.write().await;
        store.set_driver_control_values(&key, values);
        store.save()
    }
}

#[async_trait]
impl DriverLifecycleActions for DaemonDriverHost {
    async fn reconnect_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let runtime = self.discovery_runtime();
        let disconnected =
            discovery::disconnect_tracked_device(&runtime, device_id, DisconnectReason::User, true)
                .await?;
        let activated =
            discovery::activate_pairable_device(&runtime, device_id, backend_id).await?;
        Ok(disconnected || activated)
    }

    async fn rescan_driver(&self, driver_id: &str) -> Result<()> {
        let driver_id = driver_id.to_owned();
        let runtime = self.discovery_runtime();
        let task_spawner = runtime.task_spawner.clone();
        let driver_registry = Arc::clone(&self.driver_registry);
        let driver_host = Arc::new(self.clone());
        let config = self.current_config();
        let backends = vec![discovery::DiscoveryBackend::network(driver_id.clone())];

        task_spawner.spawn(async move {
            if discovery::execute_discovery_scan_if_idle(
                runtime,
                driver_registry,
                driver_host,
                config,
                backends,
                discovery::default_timeout(),
            )
            .await
            .is_none()
            {
                warn!(
                    driver_id,
                    "Skipped driver control rescan because discovery is already running"
                );
            }
        });

        Ok(())
    }
}

#[async_trait]
impl BackendRebindActions for DaemonDriverHost {
    async fn rebind_backend(&self, driver_id: &str) -> Result<()> {
        let config = self.current_config();
        let Some(driver) = self.driver_registry.get(driver_id) else {
            return Ok(());
        };
        if !super::module_enabled(&config, &driver.module_descriptor()) {
            return Ok(());
        }

        let config_entry = super::driver_config_entry(&config, driver_id);
        let config_view = DriverConfigView {
            driver_id,
            entry: &config_entry,
        };
        let Some(backend) = driver.build_backend(self, config_view)? else {
            return Ok(());
        };

        let mut manager = self.backend_manager.lock().await;
        manager.register_backend(backend);
        Ok(())
    }
}

impl DriverControlHost for DaemonDriverHost {
    fn driver_config_store(&self) -> &dyn DriverControlStore {
        self
    }

    fn device_config_store(&self) -> &dyn DeviceControlStore {
        self
    }

    fn lifecycle(&self) -> &dyn DriverLifecycleActions {
        self
    }

    fn backend_rebind(&self) -> &dyn BackendRebindActions {
        self
    }

    fn publish_control_event(&self, event: ControlSurfaceEvent) {
        self.event_bus
            .publish(HypercolorEvent::ControlSurfaceChanged(event));
    }
}

fn control_value_to_config_json(value: ControlValue) -> Value {
    match value {
        ControlValue::Null => Value::Null,
        ControlValue::Bool(value) => Value::Bool(value),
        ControlValue::Integer(value) => Value::Number(Number::from(value)),
        ControlValue::Float(value) => Number::from_f64(value).map_or(Value::Null, Value::Number),
        ControlValue::String(value)
        | ControlValue::SecretRef(value)
        | ControlValue::IpAddress(value)
        | ControlValue::MacAddress(value)
        | ControlValue::Enum(value) => Value::String(value),
        ControlValue::ColorRgb(value) => Value::Array(
            value
                .into_iter()
                .map(|channel| Value::Number(Number::from(channel)))
                .collect(),
        ),
        ControlValue::ColorRgba(value) => Value::Array(
            value
                .into_iter()
                .map(|channel| Value::Number(Number::from(channel)))
                .collect(),
        ),
        ControlValue::DurationMs(value) => Value::Number(Number::from(value)),
        ControlValue::Flags(values) => {
            Value::Array(values.into_iter().map(Value::String).collect())
        }
        ControlValue::List(values) => Value::Array(
            values
                .into_iter()
                .map(control_value_to_config_json)
                .collect(),
        ),
        ControlValue::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, control_value_to_config_json(value)))
                .collect(),
        ),
    }
}

fn config_json_to_control_value(value: &Value) -> ControlValue {
    match value {
        Value::Null => ControlValue::Null,
        Value::Bool(value) => ControlValue::Bool(*value),
        Value::Number(value) => value.as_i64().map_or_else(
            || ControlValue::Float(value.as_f64().unwrap_or_default()),
            ControlValue::Integer,
        ),
        Value::String(value) => ControlValue::String(value.clone()),
        Value::Array(values) => ControlValue::List(
            values
                .iter()
                .map(config_json_to_control_value)
                .collect::<Vec<_>>(),
        ),
        Value::Object(values) => ControlValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), config_json_to_control_value(value)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}
