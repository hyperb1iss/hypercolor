use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use hypercolor_core::device::{
    BlocksScanner, DiscoveredDevice, DiscoveryOrchestrator, DiscoveryProgress, SmBusScanner,
    TransportScanner, UsbScanner,
};
use hypercolor_driver_api::{
    DiscoveryRequest, DriverConfigView, DriverDiscoveredDevice, NetworkDriverFactory,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::event::{DeviceRef, DisconnectReason, HypercolorEvent};
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::auto_layout::sync_active_layout_for_renderable_devices;
use super::device_helpers::{
    apply_persisted_device_settings, desired_connect_behavior, device_log_label,
    device_ref_for_tracked, sync_registry_state,
};
use super::lifecycle::execute_lifecycle_actions;
use super::{DiscoveryBackend, DiscoveryRuntime, DiscoveryScannerResult};
use crate::network::{self, DaemonDriverHost};

use hypercolor_core::device::ScannerScanReport;

/// Detailed discovery scan result for reverse-engineering workflows.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryScanResult {
    /// Backends that were scanned.
    pub backends: Vec<String>,

    /// Effective timeout used for the scan.
    pub timeout_ms: u64,

    /// Newly discovered devices.
    pub new_devices: Vec<DeviceRef>,

    /// Previously known devices observed again.
    pub reappeared_devices: Vec<DeviceRef>,

    /// Device IDs that were not observed in this scan.
    pub vanished_devices: Vec<String>,

    /// Total known devices in the registry after merge.
    pub total_known: usize,

    /// End-to-end scan duration.
    pub duration_ms: u64,

    /// Per-scanner diagnostics.
    pub scanners: Vec<DiscoveryScannerResult>,
}

/// Execute a discovery scan only when no other scan currently owns the
/// shared in-progress slot.
///
/// Returns `None` when another caller is already scanning.
pub async fn execute_discovery_scan_if_idle(
    runtime: DiscoveryRuntime,
    driver_registry: Arc<DriverModuleRegistry>,
    driver_host: Arc<DaemonDriverHost>,
    config: Arc<HypercolorConfig>,
    backends: Vec<DiscoveryBackend>,
    timeout: Duration,
) -> Option<DiscoveryScanResult> {
    if runtime
        .in_progress
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return None;
    }

    Some(
        execute_discovery_scan(
            runtime,
            driver_registry,
            driver_host,
            config,
            backends,
            timeout,
        )
        .await,
    )
}

struct NetworkDriverScanner {
    driver: Arc<dyn NetworkDriverFactory>,
    driver_id: String,
    config: DriverConfigEntry,
    host: Arc<DaemonDriverHost>,
    request: DiscoveryRequest,
}

impl NetworkDriverScanner {
    fn new(
        driver: Arc<dyn NetworkDriverFactory>,
        driver_id: String,
        config: DriverConfigEntry,
        host: Arc<DaemonDriverHost>,
        request: DiscoveryRequest,
    ) -> Self {
        Self {
            driver,
            driver_id,
            config,
            host,
            request,
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for NetworkDriverScanner {
    fn name(&self) -> &str {
        self.driver.descriptor().display_name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let Some(capability) = self.driver.discovery() else {
            return Ok(Vec::new());
        };
        let config = DriverConfigView {
            driver_id: &self.driver_id,
            entry: &self.config,
        };
        let result = capability
            .discover(self.host.as_ref(), &self.request, config)
            .await?;
        Ok(result
            .devices
            .into_iter()
            .map(driver_discovered_to_device)
            .collect())
    }
}

fn driver_discovered_to_device(device: DriverDiscoveredDevice) -> DiscoveredDevice {
    DiscoveredDevice {
        connection_type: device.info.connection_type,
        origin: device.info.origin.clone(),
        name: device.info.name.clone(),
        family: device.info.family.clone(),
        fingerprint: device.fingerprint,
        connect_behavior: device.connect_behavior,
        info: device.info,
        metadata: device.metadata,
    }
}

/// Execute one full discovery scan and publish discovery events.
///
/// This function assumes the caller already set `in_progress=true`. It always
/// resets that flag on exit.
#[allow(clippy::too_many_lines)]
pub async fn execute_discovery_scan(
    runtime: DiscoveryRuntime,
    driver_registry: Arc<DriverModuleRegistry>,
    driver_host: Arc<DaemonDriverHost>,
    config: Arc<HypercolorConfig>,
    backends: Vec<DiscoveryBackend>,
    timeout: Duration,
) -> DiscoveryScanResult {
    let _flag_guard = super::DiscoveryFlagGuard {
        flag: Arc::clone(&runtime.in_progress),
    };
    let backend_names = super::backend_names(&backends);
    let scanned_backend_ids = backend_names.iter().cloned().collect::<HashSet<_>>();
    let transient_miss_backend_ids = backends
        .iter()
        .filter(|backend| backend.preserves_renderable_on_discovery_miss())
        .map(|backend| backend.as_str().to_owned())
        .collect::<HashSet<_>>();
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);

    runtime
        .event_bus
        .publish(HypercolorEvent::DeviceDiscoveryStarted {
            backends: backend_names.clone(),
        });
    info!(
        backends = ?backend_names,
        timeout_ms,
        "starting discovery scan"
    );

    if backends.is_empty() {
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDiscoveryCompleted {
                found: Vec::new(),
                duration_ms: 0,
            });
        return DiscoveryScanResult {
            backends: backend_names,
            timeout_ms,
            new_devices: Vec::new(),
            reappeared_devices: Vec::new(),
            vanished_devices: Vec::new(),
            total_known: runtime.device_registry.len().await,
            duration_ms: 0,
            scanners: Vec::new(),
        };
    }

    let mut orchestrator = DiscoveryOrchestrator::new(runtime.device_registry.clone());
    for backend in backends {
        match backend {
            DiscoveryBackend::Network(driver_id) => {
                let Some(driver) = driver_registry.get(&driver_id) else {
                    warn!(driver_id, "skipping unknown network discovery driver");
                    continue;
                };
                if driver.discovery().is_none() {
                    warn!(
                        driver_id,
                        "skipping network driver without discovery capability"
                    );
                    continue;
                }
                let driver_config = network::driver_config_entry(&config, &driver_id);
                orchestrator.add_scanner(Box::new(NetworkDriverScanner::new(
                    driver,
                    driver_id,
                    driver_config,
                    Arc::clone(&driver_host),
                    DiscoveryRequest {
                        timeout,
                        mdns_enabled: config.discovery.mdns_enabled,
                    },
                )));
            }
            DiscoveryBackend::Usb => {
                orchestrator.add_scanner(Box::new(UsbScanner::with_enabled_driver_ids(
                    network::enabled_hal_driver_ids(&config),
                )));
            }
            DiscoveryBackend::SmBus => {
                orchestrator.add_scanner(Box::new(SmBusScanner::new()));
            }
            DiscoveryBackend::Blocks => {
                let socket_path = config.discovery.blocks_socket_path.as_ref().map_or_else(
                    hypercolor_core::device::BlocksBackend::default_socket_path,
                    std::path::PathBuf::from,
                );
                orchestrator.add_scanner(Box::new(BlocksScanner::new(socket_path)));
            }
        }
    }

    if orchestrator.scanner_count() == 0 {
        warn!("Discovery scan requested with zero active scanners");
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDiscoveryCompleted {
                found: Vec::new(),
                duration_ms: 0,
            });
        return DiscoveryScanResult {
            backends: backend_names,
            timeout_ms,
            new_devices: Vec::new(),
            reappeared_devices: Vec::new(),
            vanished_devices: Vec::new(),
            total_known: runtime.device_registry.len().await,
            duration_ms: 0,
            scanners: Vec::new(),
        };
    }

    let incremental = Arc::new(Mutex::new(IncrementalDiscoveryState::default()));
    let incremental_for_progress = Arc::clone(&incremental);
    let runtime_for_progress = runtime.clone();
    let report = orchestrator
        .full_scan_with_progress(|progress: DiscoveryProgress| {
            let runtime = runtime_for_progress.clone();
            let incremental = Arc::clone(&incremental_for_progress);
            async move {
                process_discovery_progress(&runtime, progress, &incremental).await;
            }
        })
        .await;

    let IncrementalDiscoveryState {
        found,
        new_devices,
        reappeared_devices,
    } = incremental.lock().await.clone();

    let seen_ids: HashSet<DeviceId> = report
        .new_devices
        .iter()
        .chain(report.reappeared_devices.iter())
        .copied()
        .collect();

    let scan_had_errors = report
        .scanner_reports
        .iter()
        .any(|scanner| scanner.error.is_some());
    let mut scoped_registry_ids = HashSet::new();
    for tracked in runtime.device_registry.list().await {
        let backend_id = tracked.info.backend_id().to_owned();
        if scanned_backend_ids.contains(&backend_id) {
            scoped_registry_ids.insert(tracked.info.id);
        }
    }

    let ignored_out_of_scope = report
        .vanished_devices
        .iter()
        .filter(|id| !scoped_registry_ids.contains(id))
        .count();
    if ignored_out_of_scope > 0 {
        debug!(
            ignored_out_of_scope,
            backends = ?backend_names,
            "ignoring vanished devices outside the active discovery backend scope"
        );
    }

    let mut vanished_ids: HashSet<DeviceId> = if scan_had_errors {
        let failed_scanners = report
            .scanner_reports
            .iter()
            .filter_map(|scanner| scanner.error.as_ref().map(|_| scanner.scanner.clone()))
            .collect::<Vec<_>>();
        warn!(
            backends = ?backend_names,
            failed_scanners = ?failed_scanners,
            "discovery scan was incomplete; preserving existing device mappings"
        );
        HashSet::new()
    } else {
        report
            .vanished_devices
            .iter()
            .copied()
            .filter(|id| scoped_registry_ids.contains(id))
            .collect()
    };
    let lifecycle_tracked_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.tracked_device_ids()
    };
    for id in lifecycle_tracked_ids {
        if !scan_had_errors && scoped_registry_ids.contains(&id) && !seen_ids.contains(&id) {
            vanished_ids.insert(id);
        }
    }
    retain_transient_backend_devices(&runtime, &transient_miss_backend_ids, &mut vanished_ids)
        .await;

    let mut vanished_ids: Vec<DeviceId> = vanished_ids.into_iter().collect();
    vanished_ids.sort_by_key(DeviceId::as_uuid);

    let mut vanished_devices = Vec::new();
    for id in vanished_ids {
        let device_label = device_log_label(&runtime, id).await;
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_device_vanished(id)
        };
        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(&runtime, id).await;

        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDisconnected {
                device_id: id.to_string(),
                reason: DisconnectReason::Timeout,
                will_retry: true,
            });
        info!(
            device = %device_label,
            device_id = %id,
            reason = ?DisconnectReason::Timeout,
            will_retry = true,
            "device disconnected"
        );
        vanished_devices.push(id.to_string());
    }

    let duration_ms = u64::try_from(report.scan_duration.as_millis()).unwrap_or(u64::MAX);
    runtime
        .event_bus
        .publish(HypercolorEvent::DeviceDiscoveryCompleted {
            found: found.clone(),
            duration_ms,
        });

    debug!(
        new_devices = found.len(),
        total_known = report.total_known,
        duration_ms,
        "Discovery scan completed"
    );
    info!(
        backends = ?backend_names,
        found = found.len(),
        vanished = vanished_devices.len(),
        total_known = report.total_known,
        duration_ms,
        "Discovery sweep finished"
    );

    sync_active_layout_for_renderable_devices(&runtime, None).await;
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.enable_unmapped_layout_warnings();
    }

    DiscoveryScanResult {
        backends: backend_names,
        timeout_ms,
        new_devices,
        reappeared_devices,
        vanished_devices,
        total_known: report.total_known,
        duration_ms,
        scanners: map_scanner_reports(&report.scanner_reports),
    }
}

async fn retain_transient_backend_devices(
    runtime: &DiscoveryRuntime,
    transient_miss_backend_ids: &HashSet<String>,
    vanished_ids: &mut HashSet<DeviceId>,
) {
    if vanished_ids.is_empty() || transient_miss_backend_ids.is_empty() {
        return;
    }

    let mut preserved_ids = Vec::new();
    let mut preserved_labels = Vec::new();
    for id in vanished_ids.iter().copied() {
        let Some(tracked) = runtime.device_registry.get(&id).await else {
            continue;
        };
        if !(tracked.state.is_renderable() || tracked.state == DeviceState::Reconnecting) {
            continue;
        }

        if !transient_miss_backend_ids.contains(tracked.info.backend_id()) {
            continue;
        }

        preserved_ids.push(id);
        preserved_labels.push(format!("{} ({id})", tracked.info.name));
    }

    if preserved_ids.is_empty() {
        return;
    }

    for id in preserved_ids {
        vanished_ids.remove(&id);
    }

    debug!(
        preserved_count = preserved_labels.len(),
        devices = ?preserved_labels,
        "preserving connected devices across transient discovery miss"
    );
}

#[derive(Debug, Clone, Default)]
struct IncrementalDiscoveryState {
    found: Vec<DeviceRef>,
    new_devices: Vec<DeviceRef>,
    reappeared_devices: Vec<DeviceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoverySeenKind {
    New,
    Reappeared,
}

async fn process_discovery_progress(
    runtime: &DiscoveryRuntime,
    progress: DiscoveryProgress,
    incremental: &Arc<Mutex<IncrementalDiscoveryState>>,
) {
    for id in progress.new_devices {
        if let Some((device_ref, _)) =
            process_discovered_device(runtime, id, DiscoverySeenKind::New).await
        {
            let mut state = incremental.lock().await;
            state.found.push(device_ref.clone());
            state.new_devices.push(device_ref);
        }
    }

    for id in progress.reappeared_devices {
        let Some((device_ref, should_record)) =
            process_discovered_device(runtime, id, DiscoverySeenKind::Reappeared).await
        else {
            continue;
        };

        let mut state = incremental.lock().await;
        state.found.push(device_ref.clone());
        if should_record {
            state.reappeared_devices.push(device_ref);
        }
    }
}

async fn process_discovered_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    kind: DiscoverySeenKind,
) -> Option<(DeviceRef, bool)> {
    let persisted_settings = apply_persisted_device_settings(runtime, device_id).await;
    let tracked_before = runtime.device_registry.get(&device_id).await?;
    let was_renderable = tracked_before.state.is_renderable();

    let backend = tracked_before.info.backend_id().to_owned();
    let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
    let connect_behavior = desired_connect_behavior(
        runtime,
        device_id,
        &tracked_before.info,
        &backend,
        fingerprint.as_ref(),
        tracked_before.connect_behavior,
        persisted_settings.enabled,
    )
    .await;
    if !connect_behavior.should_auto_connect() {
        debug!(
            device = %tracked_before.info.name,
            device_id = %device_id,
            "deferring auto-connect until discovery/layout state enables the device"
        );
    }
    let mut actions = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(
            device_id,
            &tracked_before.info,
            &backend,
            fingerprint.as_ref(),
            connect_behavior,
        )
    };
    if !persisted_settings.enabled {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        match lifecycle.on_user_disable(device_id) {
            Ok(disable_actions) => actions = disable_actions,
            Err(error) => {
                warn!(
                    device = %tracked_before.info.name,
                    device_id = %device_id,
                    error = %error,
                    kind = ?kind,
                    "failed to reapply persisted disabled state during discovery"
                );
            }
        }
    }
    let had_actions = !actions.is_empty();
    execute_lifecycle_actions(runtime.clone(), actions).await;
    sync_registry_state(runtime, device_id).await;

    let tracked_after = runtime.device_registry.get(&device_id).await?;
    let device_ref = device_ref_for_tracked(&tracked_after.info);

    let should_publish_reappeared = !was_renderable || had_actions;
    let should_publish = match kind {
        DiscoverySeenKind::New => true,
        DiscoverySeenKind::Reappeared => should_publish_reappeared,
    };

    if should_publish {
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDiscovered {
                device_id: device_ref.id.clone(),
                name: device_ref.name.clone(),
                backend: device_ref.backend.clone(),
                led_count: device_ref.led_count,
                address: None,
            });

        let message = match kind {
            DiscoverySeenKind::New => "discovered new device",
            DiscoverySeenKind::Reappeared => "device reappeared",
        };
        info!(
            device = %device_ref.name,
            device_id = %device_ref.id,
            backend = %device_ref.backend,
            led_count = device_ref.led_count,
            "{message}"
        );
    }

    Some((
        device_ref,
        matches!(kind, DiscoverySeenKind::Reappeared) && should_publish_reappeared,
    ))
}

fn map_scanner_reports(reports: &[ScannerScanReport]) -> Vec<DiscoveryScannerResult> {
    reports
        .iter()
        .map(|report| DiscoveryScannerResult {
            scanner: report.scanner.clone(),
            duration_ms: u64::try_from(report.duration.as_millis()).unwrap_or(u64::MAX),
            discovered: report.discovered,
            status: if report.error.is_some() {
                "error".to_owned()
            } else {
                "ok".to_owned()
            },
            error: report.error.clone(),
        })
        .collect()
}
