use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use hypercolor_core::config::ConfigManager;
use hypercolor_driver_api::{
    BackendRebindActions, DeviceControlStore, DriverConfigView, DriverControlHost,
    DriverControlStore, DriverCredentialStore, DriverDiscoveryState, DriverHost,
    DriverLifecycleActions, DriverRuntimeActions, DriverTrackedDevice,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{ControlSurfaceEvent, ControlValueMap};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use serde_json::Value;
use tracing::warn;

use crate::discovery::{self, DiscoveryRuntime};
use crate::driver_inventory::DriverInventoryStore;

/// Daemon-owned host adapter passed to built-in drivers.
#[derive(Clone)]
pub struct DaemonDriverHost {
    runtime: DiscoveryRuntime,
    driver_inventory: Arc<DriverInventoryStore>,
    driver_registry: Arc<DriverModuleRegistry>,
    config_manager: Option<Arc<ConfigManager>>,
}

impl DaemonDriverHost {
    #[must_use]
    pub fn new(
        runtime: DiscoveryRuntime,
        driver_inventory: Arc<DriverInventoryStore>,
        driver_registry: Arc<DriverModuleRegistry>,
        config_manager: Option<Arc<ConfigManager>>,
    ) -> Self {
        Self {
            runtime,
            driver_inventory,
            driver_registry,
            config_manager,
        }
    }

    #[must_use]
    pub fn discovery_runtime(&self) -> DiscoveryRuntime {
        self.runtime.clone()
    }

    #[must_use]
    pub fn credential_store(&self) -> Arc<CredentialStore> {
        Arc::clone(&self.runtime.credential_store)
    }

    #[must_use]
    pub fn driver_inventory(&self) -> Arc<DriverInventoryStore> {
        Arc::clone(&self.driver_inventory)
    }

    pub(crate) async fn refresh_driver_inventory(&self) {
        self.driver_inventory
            .refresh(self.driver_registry.as_ref(), self)
            .await;
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
        self.runtime
            .device_registry
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
        Ok(self
            .runtime
            .credential_store
            .get_driver_json(driver_id, key)
            .await)
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.runtime
            .credential_store
            .store_driver_json(driver_id, key, value)
            .await
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        self.runtime
            .credential_store
            .remove_driver(driver_id, key)
            .await
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

        for tracked in self.runtime.device_registry.list().await {
            let metadata = self
                .runtime
                .device_registry
                .metadata_for_id(&tracked.info.id)
                .await
                .unwrap_or_default();
            if tracked.info.driver_id() != driver_id {
                continue;
            }
            let fingerprint = self
                .runtime
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
        let Some(driver) = self.driver_registry.get(driver_id) else {
            bail!("driver '{driver_id}' is not registered");
        };
        let Some(provider) = driver.controls() else {
            return Ok(ControlValueMap::new());
        };
        let config = DriverConfigView { driver_id, entry };
        let Some(document) = provider.driver_surface(self, config).await? else {
            return Ok(ControlValueMap::new());
        };
        document
            .values
            .into_iter()
            .map(|(key, projected)| {
                let Some(value) = entry
                    .settings
                    .get(&key)
                    .filter(|value| ControlValue::is_canonical_wire_candidate(value))
                else {
                    return Ok((key, projected));
                };
                serde_json::from_value(value.clone())
                    .with_context(|| {
                        format!(
                            "invalid persisted control value for driver '{driver_id}' setting '{key}'"
                        )
                    })
                    .map(|persisted| (key, persisted))
            })
            .collect()
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
            let persisted = serde_json::to_value(value)
                .with_context(|| format!("failed to serialize driver control '{key}'"))?;
            entry.settings.insert(key, persisted);
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
        self.runtime
            .device_settings
            .driver_control_values_for_key(&key)
            .await
    }

    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()> {
        let key = self.device_control_settings_key(device_id).await;
        self.runtime
            .device_settings
            .persist_driver_control_values(&key, values)
            .await?;
        // Driver control rows live in their own map, not the per-device
        // settings rows a `Some` key names, so this hint is store-scoped.
        self.runtime
            .event_bus
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
        if !super::module_enabled(&config, &descriptor)
            || !super::module_has_available_transport(&descriptor)
            || !descriptor.capabilities.output_backend
        {
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

        let mut manager = self.runtime.backend_manager.lock().await;
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
        self.runtime
            .event_bus
            .publish(HypercolorEvent::ControlSurfaceChanged(event));
    }
}
