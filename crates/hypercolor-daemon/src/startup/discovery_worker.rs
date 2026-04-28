//! Background discovery worker — periodic device scans plus startup recovery retries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::manager::BackendRoutingDebugSnapshot;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{self, DiscoveryTarget};
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::network::DaemonDriverHost;
use crate::scene_transactions::SceneTransactionQueue;
use hypercolor_core::scene::SceneManager;

const STARTUP_NETWORK_RECOVERY_ATTEMPTS: usize = 3;
const STARTUP_NETWORK_RECOVERY_INTERVAL_SECS: u64 = 5;

#[derive(Clone)]
pub(super) struct DiscoveryWorkerContext {
    pub(super) device_registry: DeviceRegistry,
    pub(super) backend_manager: Arc<Mutex<BackendManager>>,
    pub(super) lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    pub(super) reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    pub(super) event_bus: Arc<HypercolorBus>,
    pub(super) config_manager: Arc<ConfigManager>,
    pub(super) driver_host: Arc<DaemonDriverHost>,
    pub(super) driver_registry: Arc<DriverModuleRegistry>,
    pub(super) spatial_engine: Arc<RwLock<SpatialEngine>>,
    pub(super) scene_manager: Arc<RwLock<SceneManager>>,
    pub(super) layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    pub(super) layouts_path: PathBuf,
    pub(super) layout_auto_exclusions:
        Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    pub(super) logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    pub(super) attachment_registry: Arc<RwLock<AttachmentRegistry>>,
    pub(super) attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,
    pub(super) device_settings: Arc<RwLock<DeviceSettingsStore>>,
    pub(super) runtime_state_path: PathBuf,
    pub(super) usb_protocol_configs: UsbProtocolConfigStore,
    pub(super) credential_store: Arc<CredentialStore>,
    pub(super) in_progress: Arc<AtomicBool>,
    pub(super) scene_transactions: SceneTransactionQueue,
}

impl DiscoveryWorkerContext {
    fn runtime(&self) -> discovery::DiscoveryRuntime {
        discovery::DiscoveryRuntime {
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
            in_progress: Arc::clone(&self.in_progress),
            scene_transactions: self.scene_transactions.clone(),
            task_spawner: tokio::runtime::Handle::current(),
        }
    }

    pub(super) async fn run_scan_if_idle(
        &self,
        config: Arc<HypercolorConfig>,
        targets: Vec<DiscoveryTarget>,
        busy_log: &'static str,
    ) {
        if targets.is_empty() {
            return;
        }

        if self
            .in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("{busy_log}");
            return;
        }

        let _ = discovery::execute_discovery_scan(
            self.runtime(),
            Arc::clone(&self.driver_registry),
            Arc::clone(&self.driver_host),
            config,
            targets,
            discovery::default_timeout(),
        )
        .await;
    }

    pub(super) async fn run_periodic_scan(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());
        let targets =
            match discovery::resolve_targets(None, &latest_config, self.driver_registry.as_ref()) {
                Ok(targets) => targets,
                Err(error) => {
                    warn!(
                        error = %error,
                        "Periodic discovery target resolution failed; skipping interval"
                    );
                    return;
                }
            };

        self.run_scan_if_idle(
            latest_config,
            targets,
            "Skipping periodic discovery scan; scan already in progress",
        )
        .await;
    }

    pub(super) async fn run_usb_hotplug_scan(&self) {
        self.run_scan_if_idle(
            Arc::clone(&self.config_manager.get()),
            vec![DiscoveryTarget::usb()],
            "Skipping USB hotplug scan; discovery already in progress",
        )
        .await;
    }

    pub(super) async fn run_startup_network_recovery_scans(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());

        for attempt in 1..=STARTUP_NETWORK_RECOVERY_ATTEMPTS {
            let unmapped_by_driver = self
                .active_layout_unmapped_network_targets(&latest_config)
                .await;
            if unmapped_by_driver.is_empty() {
                return;
            }
            let targets = unmapped_by_driver
                .keys()
                .cloned()
                .map(DiscoveryTarget::driver)
                .collect::<Vec<_>>();
            let drivers = discovery::target_names(&targets);
            let unmapped_layout_device_ids = unmapped_by_driver
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>();

            info!(
                attempt,
                max_attempts = STARTUP_NETWORK_RECOVERY_ATTEMPTS,
                retry_after_secs = STARTUP_NETWORK_RECOVERY_INTERVAL_SECS,
                drivers = ?drivers,
                unmapped_layout_device_ids = ?unmapped_layout_device_ids,
                "Active layout still has unmapped network targets after startup scan; retrying discovery"
            );

            tokio::time::sleep(std::time::Duration::from_secs(
                STARTUP_NETWORK_RECOVERY_INTERVAL_SECS,
            ))
            .await;

            self.run_scan_if_idle(
                Arc::clone(&latest_config),
                targets,
                "Skipping startup network recovery scan; discovery already in progress",
            )
            .await;
        }

        let unmapped_by_driver = self
            .active_layout_unmapped_network_targets(&latest_config)
            .await;
        if !unmapped_by_driver.is_empty() {
            let drivers = unmapped_by_driver.keys().cloned().collect::<Vec<_>>();
            let unmapped_layout_device_ids = unmapped_by_driver
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>();
            warn!(
                retry_attempts = STARTUP_NETWORK_RECOVERY_ATTEMPTS,
                drivers = ?drivers,
                unmapped_layout_device_ids = ?unmapped_layout_device_ids,
                scan_interval_secs = latest_config.discovery.scan_interval_secs.max(1),
                "Startup recovery scans exhausted; active layout still has unmapped network targets"
            );
        }
    }

    async fn active_layout_unmapped_network_targets(
        &self,
        config: &HypercolorConfig,
    ) -> BTreeMap<String, Vec<String>> {
        let layout = {
            let spatial = self.spatial_engine.read().await;
            spatial.layout().as_ref().clone()
        };
        let routing = {
            let manager = self.backend_manager.lock().await;
            manager.routing_snapshot()
        };
        let driver_ids = self
            .driver_registry
            .discovery_drivers()
            .into_iter()
            .filter_map(|driver| {
                let id = driver.descriptor().id;
                crate::network::driver_enabled(config, id).then(|| id.to_owned())
            })
            .collect::<Vec<_>>();

        collect_unmapped_driver_layout_targets(&layout, &routing, &driver_ids)
    }
}

#[doc(hidden)]
#[must_use]
pub fn collect_unmapped_prefixed_layout_targets(
    layout: &SpatialLayout,
    routing: &BackendRoutingDebugSnapshot,
    prefix: &str,
) -> Vec<String> {
    let mapped_ids: HashSet<&str> = routing
        .mappings
        .iter()
        .map(|entry| entry.layout_device_id.as_str())
        .collect();

    let mut unmapped = layout
        .zones
        .iter()
        .filter_map(|zone| {
            let layout_device_id = zone.device_id.as_str();
            (layout_device_id.starts_with(prefix) && !mapped_ids.contains(layout_device_id))
                .then(|| zone.device_id.clone())
        })
        .collect::<Vec<_>>();

    unmapped.sort();
    unmapped.dedup();
    unmapped
}

#[doc(hidden)]
#[must_use]
pub fn collect_unmapped_driver_layout_targets(
    layout: &SpatialLayout,
    routing: &BackendRoutingDebugSnapshot,
    driver_ids: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mapped_ids: HashSet<&str> = routing
        .mappings
        .iter()
        .map(|entry| entry.layout_device_id.as_str())
        .collect();
    let driver_ids = driver_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut unmapped = BTreeMap::<String, Vec<String>>::new();

    for zone in &layout.zones {
        let layout_device_id = zone.device_id.as_str();
        let Some((driver_id, _)) = layout_device_id.split_once(':') else {
            continue;
        };
        if driver_ids.contains(driver_id) && !mapped_ids.contains(layout_device_id) {
            unmapped
                .entry(driver_id.to_owned())
                .or_default()
                .push(zone.device_id.clone());
        }
    }

    for targets in unmapped.values_mut() {
        targets.sort();
        targets.dedup();
    }

    unmapped
}
