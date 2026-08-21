use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::{Result, bail};
use async_trait::async_trait;
use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    BackendRebindActions, DeviceControlStore, DriverConfigView, DriverControlHost,
    DriverControlStore, DriverCredentialStore, DriverDiscoveryState, DriverHost,
    DriverLifecycleActions, DriverRuntimeActions, DriverTrackedDevice,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::controls::{ControlSurfaceEvent, ControlValue, ControlValueMap};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use hypercolor_types::spatial::SpatialLayout;
use serde_json::{Number, Value};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{self, DiscoveryRuntime};
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::driver_inventory::DriverInventoryStore;
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::scene_transactions::SceneTransactionQueue;

/// Daemon-owned host adapter passed to built-in drivers.
#[derive(Clone)]
pub struct DaemonDriverHost {
    device_registry: DeviceRegistry,
    backend_manager: Arc<Mutex<BackendManager>>,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    event_bus: Arc<HypercolorBus>,
    spatial_engine: SpatialService,
    scene_manager: SceneService,
    layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    layouts_path: PathBuf,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    attachment_registry: Arc<RwLock<ComponentRegistry>>,
    attachment_profiles: Arc<RwLock<ComponentProfileStore>>,
    device_settings: Arc<RwLock<DeviceSettingsStore>>,
    runtime_state_path: PathBuf,
    device_aliases_path: PathBuf,
    driver_inventory: Arc<DriverInventoryStore>,
    usb_protocol_configs: UsbProtocolConfigStore,
    credential_store: Arc<CredentialStore>,
    driver_registry: Arc<DriverModuleRegistry>,
    discovery_in_progress: Arc<AtomicBool>,
    pending_discovery_scans: Arc<StdMutex<crate::discovery::PendingDiscoveryScans>>,
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
        spatial_engine: SpatialService,
        scene_manager: SceneService,
        layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
        layouts_path: PathBuf,
        layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
        logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
        attachment_registry: Arc<RwLock<ComponentRegistry>>,
        attachment_profiles: Arc<RwLock<ComponentProfileStore>>,
        device_settings: Arc<RwLock<DeviceSettingsStore>>,
        runtime_state_path: PathBuf,
        device_aliases_path: PathBuf,
        driver_inventory: Arc<DriverInventoryStore>,
        usb_protocol_configs: UsbProtocolConfigStore,
        credential_store: Arc<CredentialStore>,
        driver_registry: Arc<DriverModuleRegistry>,
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
            device_aliases_path,
            driver_inventory,
            usb_protocol_configs,
            credential_store,
            driver_registry,
            discovery_in_progress,
            pending_discovery_scans: Arc::default(),
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
    pub fn with_driver_registry(&self, driver_registry: Arc<DriverModuleRegistry>) -> Self {
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
            spatial_engine: self.spatial_engine.clone(),
            scene_manager: self.scene_manager.clone(),
            layouts: Arc::clone(&self.layouts),
            layouts_path: self.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&self.layout_auto_exclusions),
            logical_devices: Arc::clone(&self.logical_devices),
            attachment_registry: Arc::clone(&self.attachment_registry),
            attachment_profiles: Arc::clone(&self.attachment_profiles),
            device_settings: Arc::clone(&self.device_settings),
            runtime_state_path: self.runtime_state_path.clone(),
            device_aliases_path: self.device_aliases_path.clone(),
            usb_protocol_configs: self.usb_protocol_configs.clone(),
            credential_store: Arc::clone(&self.credential_store),
            in_progress: Arc::clone(&self.discovery_in_progress),
            pending_scans: Arc::clone(&self.pending_discovery_scans),
            scene_transactions: self.scene_transactions.clone(),
            task_spawner: tokio::runtime::Handle::current(),
        }
    }

    #[must_use]
    pub fn credential_store(&self) -> Arc<CredentialStore> {
        Arc::clone(&self.credential_store)
    }

    #[must_use]
    pub fn driver_inventory(&self) -> Arc<DriverInventoryStore> {
        Arc::clone(&self.driver_inventory)
    }

    fn current_config(&self) -> Arc<HypercolorConfig> {
        self.current_config_snapshot()
            .unwrap_or_else(|| Arc::new(HypercolorConfig::default()))
    }

    #[must_use]
    pub(crate) fn current_config_snapshot(&self) -> Option<Arc<HypercolorConfig>> {
        self.config_manager
            .as_ref()
            .map(|manager| Arc::clone(&manager.get()))
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
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        Ok(self.credential_store.get_driver_json(driver_id, key).await)
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.credential_store
            .store_driver_json(driver_id, key, value)
            .await
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        self.credential_store.remove_driver(driver_id, key).await
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
            if tracked.info.driver_id() != driver_id {
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
        Ok(self.driver_inventory.load_cached_json(driver_id, key))
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
        let mut entry = manager
            .get()
            .drivers
            .get(driver_id)
            .cloned()
            .unwrap_or_default();
        for (key, value) in values {
            entry
                .settings
                .insert(key, control_value_to_config_json(value));
        }
        if let Some(driver) = self.driver_registry.get(driver_id)
            && let Some(provider) = driver.config()
        {
            provider.validate_config(&entry)?;
        }
        // Targeted read-modify-write so a concurrent writer (e.g. the
        // capture restore-token sink) is never clobbered wholesale.
        manager.modify(|config| {
            config.drivers.insert(driver_id.to_owned(), entry);
        });
        manager.save()
    }
}

#[async_trait]
impl DeviceControlStore for DaemonDriverHost {
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap> {
        let key = self.device_control_settings_key(device_id).await;
        let store = self.device_settings.read().await;
        store.driver_control_values_for_key(&key)
    }

    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()> {
        let key = self.device_control_settings_key(device_id).await;
        {
            let mut store = self.device_settings.write().await;
            store.set_driver_control_values(&key, values)?;
            store.save()?;
        }
        // Driver control rows live in their own map, not the per-device
        // settings rows a `Some` key names, so this hint is store-scoped.
        self.event_bus
            .publish(HypercolorEvent::DeviceSettingsChanged { key: None });
        Ok(())
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
        let runtime = self.discovery_runtime();
        let driver_registry = Arc::clone(&self.driver_registry);
        let driver_host = Arc::new(self.clone());
        let config = self.current_config();
        let targets = match discovery::rescan_targets_for_driver(
            &driver_id,
            config.as_ref(),
            &driver_registry,
        ) {
            Ok(targets) => targets,
            Err(error) => {
                warn!(driver_id, error = %error, "Skipped driver control rescan");
                return Ok(());
            }
        };

        discovery::schedule_discovery_scan(
            runtime,
            driver_registry,
            driver_host,
            config,
            targets,
            discovery::default_timeout(),
        );

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
        let descriptor = driver.module_descriptor();
        if !super::module_enabled(&config, &descriptor) || !descriptor.capabilities.output_backend {
            return Ok(());
        }

        let enabled_driver_ids = super::enabled_driver_module_ids(&self.driver_registry, &config);
        let finalized = self
            .driver_registry
            .finalize_output_bindings(&enabled_driver_ids)?;
        let Some(backend_id) = driver.output().backend_id().cloned() else {
            return Ok(());
        };
        let Some(provider) = finalized.provider(&backend_id) else {
            return Ok(());
        };
        let provider_driver_id = provider.driver_id();
        let config_entry = super::driver_config_entry(&config, provider_driver_id);
        let config_view = DriverConfigView {
            driver_id: provider_driver_id,
            entry: &config_entry,
        };
        let backend = provider.build(self, config_view)?;

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
        ControlValue::Null | ControlValue::Unknown => Value::Null,
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
