//! Background discovery worker — periodic device scans plus startup recovery retries.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use tracing::{debug, info, warn};

use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::manager::BackendRoutingDebugSnapshot;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::spatial::SpatialLayout;

use crate::discovery::{self, DiscoveryRuntime, DiscoveryTarget};
use crate::network::DaemonDriverHost;

const STARTUP_DRIVER_RECOVERY_ATTEMPTS: usize = 3;
const STARTUP_DRIVER_RECOVERY_INTERVAL_SECS: u64 = 5;

#[derive(Clone)]
pub(super) struct DiscoveryWorkerContext {
    pub(super) discovery: DiscoveryRuntime,
    pub(super) config_manager: Arc<ConfigManager>,
    pub(super) driver_host: Arc<DaemonDriverHost>,
    pub(super) driver_registry: Arc<DriverModuleRegistry>,
}

impl DiscoveryWorkerContext {
    fn runtime(&self) -> discovery::DiscoveryRuntime {
        self.discovery.clone()
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

        if discovery::execute_discovery_scan_or_enqueue(
            self.runtime(),
            Arc::clone(&self.driver_registry),
            Arc::clone(&self.driver_host),
            config,
            targets,
            discovery::default_timeout(),
        )
        .await
        .is_none()
        {
            debug!("{busy_log}");
        }
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
            "Queued periodic discovery scan behind active discovery",
        )
        .await;
    }

    pub(super) async fn run_usb_hotplug_scan(&self) {
        self.run_scan_if_idle(
            Arc::clone(&self.config_manager.get()),
            vec![DiscoveryTarget::usb()],
            "Queued USB hotplug scan behind active discovery",
        )
        .await;
    }

    pub(super) async fn run_startup_driver_recovery_scans(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());

        for attempt in 1..=STARTUP_DRIVER_RECOVERY_ATTEMPTS {
            let unmapped_by_driver = self
                .active_layout_unmapped_driver_targets(&latest_config)
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
                max_attempts = STARTUP_DRIVER_RECOVERY_ATTEMPTS,
                retry_after_secs = STARTUP_DRIVER_RECOVERY_INTERVAL_SECS,
                drivers = ?drivers,
                unmapped_layout_device_ids = ?unmapped_layout_device_ids,
                "Active layout still has unmapped driver targets after startup scan; retrying discovery"
            );

            tokio::time::sleep(std::time::Duration::from_secs(
                STARTUP_DRIVER_RECOVERY_INTERVAL_SECS,
            ))
            .await;

            self.run_scan_if_idle(
                Arc::clone(&latest_config),
                targets,
                "Queued startup driver recovery scan behind active discovery",
            )
            .await;
        }

        let unmapped_by_driver = self
            .active_layout_unmapped_driver_targets(&latest_config)
            .await;
        if !unmapped_by_driver.is_empty() {
            let drivers = unmapped_by_driver.keys().cloned().collect::<Vec<_>>();
            let unmapped_layout_device_ids = unmapped_by_driver
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>();
            warn!(
                retry_attempts = STARTUP_DRIVER_RECOVERY_ATTEMPTS,
                drivers = ?drivers,
                unmapped_layout_device_ids = ?unmapped_layout_device_ids,
                scan_interval_secs = latest_config.discovery.scan_interval_secs.max(1),
                "Startup recovery scans exhausted; active layout still has unmapped driver targets"
            );
        }
    }

    async fn active_layout_unmapped_driver_targets(
        &self,
        config: &HypercolorConfig,
    ) -> BTreeMap<String, Vec<String>> {
        let layout = {
            let spatial = self.discovery.spatial_engine.snapshot();
            spatial.layout().as_ref().clone()
        };
        let routing = {
            let manager = self.discovery.backend_manager.lock().await;
            manager.routing_snapshot()
        };
        let driver_ids = self
            .driver_registry
            .discovery_drivers()
            .into_iter()
            .filter_map(|driver| {
                let descriptor = driver.module_descriptor();
                crate::network::module_enabled(config, &descriptor).then_some(descriptor.id)
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
