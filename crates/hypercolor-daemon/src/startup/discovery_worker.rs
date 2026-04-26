//! Background discovery worker — periodic device scans plus startup recovery retries.

use std::collections::{HashMap, HashSet};
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
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{self, DiscoveryBackend};
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::network::DaemonDriverHost;
use crate::scene_transactions::SceneTransactionQueue;
use hypercolor_core::scene::SceneManager;

const STARTUP_WLED_RECOVERY_ATTEMPTS: usize = 3;
const STARTUP_WLED_RECOVERY_INTERVAL_SECS: u64 = 5;

#[derive(Clone)]
pub(super) struct DiscoveryWorkerContext {
    pub(super) device_registry: DeviceRegistry,
    pub(super) backend_manager: Arc<Mutex<BackendManager>>,
    pub(super) lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    pub(super) reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    pub(super) event_bus: Arc<HypercolorBus>,
    pub(super) config_manager: Arc<ConfigManager>,
    pub(super) driver_host: Arc<DaemonDriverHost>,
    pub(super) driver_registry: Arc<DriverRegistry>,
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
        backends: Vec<DiscoveryBackend>,
        busy_log: &'static str,
    ) {
        if backends.is_empty() {
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
            backends,
            discovery::default_timeout(),
        )
        .await;
    }

    pub(super) async fn run_periodic_scan(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());
        let backends = match discovery::resolve_backends(
            None,
            &latest_config,
            self.driver_registry.as_ref(),
        ) {
            Ok(backends) => backends,
            Err(error) => {
                warn!(
                    error = %error,
                    "Periodic discovery backend resolution failed; skipping interval"
                );
                return;
            }
        };

        self.run_scan_if_idle(
            latest_config,
            backends,
            "Skipping periodic discovery scan; scan already in progress",
        )
        .await;
    }

    pub(super) async fn run_usb_hotplug_scan(&self) {
        self.run_scan_if_idle(
            Arc::clone(&self.config_manager.get()),
            vec![DiscoveryBackend::Usb],
            "Skipping USB hotplug scan; discovery already in progress",
        )
        .await;
    }

    pub(super) async fn run_startup_wled_recovery_scans(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());
        if !should_retry_unmapped_wled_targets(&latest_config) {
            return;
        }

        for attempt in 1..=STARTUP_WLED_RECOVERY_ATTEMPTS {
            let unmapped = self.active_layout_unmapped_wled_targets().await;
            if unmapped.is_empty() {
                return;
            }

            info!(
                attempt,
                max_attempts = STARTUP_WLED_RECOVERY_ATTEMPTS,
                retry_after_secs = STARTUP_WLED_RECOVERY_INTERVAL_SECS,
                unmapped_layout_device_ids = ?unmapped,
                "Active layout still has unmapped WLED targets after startup scan; retrying discovery"
            );

            tokio::time::sleep(std::time::Duration::from_secs(
                STARTUP_WLED_RECOVERY_INTERVAL_SECS,
            ))
            .await;

            self.run_scan_if_idle(
                Arc::clone(&latest_config),
                vec![DiscoveryBackend::network("wled")],
                "Skipping startup WLED recovery scan; discovery already in progress",
            )
            .await;
        }

        let unmapped = self.active_layout_unmapped_wled_targets().await;
        if !unmapped.is_empty() {
            warn!(
                retry_attempts = STARTUP_WLED_RECOVERY_ATTEMPTS,
                unmapped_layout_device_ids = ?unmapped,
                scan_interval_secs = latest_config.discovery.scan_interval_secs.max(1),
                "Startup recovery scans exhausted; active layout still has unmapped WLED targets"
            );
        }
    }

    async fn active_layout_unmapped_wled_targets(&self) -> Vec<String> {
        let layout = {
            let spatial = self.spatial_engine.read().await;
            spatial.layout().as_ref().clone()
        };
        let routing = {
            let manager = self.backend_manager.lock().await;
            manager.routing_snapshot()
        };

        collect_unmapped_prefixed_layout_targets(&layout, &routing, "wled:")
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

fn should_retry_unmapped_wled_targets(config: &HypercolorConfig) -> bool {
    let wled_enabled = config.drivers.get("wled").is_none_or(|entry| entry.enabled);
    let has_known_ips = config
        .drivers
        .get("wled")
        .and_then(|entry| entry.settings.get("known_ips"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|ips| !ips.is_empty());

    wled_enabled && config.discovery.mdns_enabled && !has_known_ips
}
