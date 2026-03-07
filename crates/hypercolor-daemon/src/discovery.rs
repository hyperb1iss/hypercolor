//! Shared device discovery runtime for daemon startup and API-triggered scans.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::openrgb::{OpenRgbScanner, ScannerConfig as OpenRgbScannerConfig};
use hypercolor_core::device::wled::WledScanner;
use hypercolor_core::device::{
    AsyncWriteFailure, BackendManager, DeviceLifecycleManager, DeviceRegistry,
    DiscoveryOrchestrator, LifecycleAction, ScannerScanReport, UsbScanner,
};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{DeviceFamily, DeviceId};
use hypercolor_types::event::{DeviceRef, DisconnectReason, HypercolorEvent};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::logical_devices::{self, LogicalDevice};

const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
const MIN_DISCOVERY_TIMEOUT_MS: u64 = 100;
const MAX_DISCOVERY_TIMEOUT_MS: u64 = 60_000;

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

/// Per-scanner diagnostics for one discovery scan.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryScannerResult {
    /// Scanner display name.
    pub scanner: String,

    /// Scanner runtime in milliseconds.
    pub duration_ms: u64,

    /// Devices returned by this scanner.
    pub discovered: usize,

    /// `"ok"` or `"error"`.
    pub status: String,

    /// Error message for failed scanners.
    pub error: Option<String>,
}

/// Shared runtime dependencies needed for discovery + lifecycle orchestration.
#[derive(Clone)]
pub struct DiscoveryRuntime {
    /// Device registry used for discovery merge and state sync.
    pub device_registry: DeviceRegistry,

    /// Backend manager used to connect/disconnect and map devices.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Pure lifecycle state/action manager.
    pub lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,

    /// Background reconnect tasks keyed by device ID.
    pub reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,

    /// Event bus for discovery/lifecycle events.
    pub event_bus: Arc<HypercolorBus>,

    /// Logical device segmentation store.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Shared "scan in progress" lock flag.
    pub in_progress: Arc<AtomicBool>,
}

/// Discovery backends currently implemented in runtime scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscoveryBackend {
    Wled,
    OpenRgb,
    Usb,
}

impl DiscoveryBackend {
    /// Stable backend identifier used in request/response payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wled => "wled",
            Self::OpenRgb => "openrgb",
            Self::Usb => "usb",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "wled" => Some(Self::Wled),
            "openrgb" => Some(Self::OpenRgb),
            "usb" => Some(Self::Usb),
            _ => None,
        }
    }
}

/// Default timeout used when callers do not provide one.
#[must_use]
pub const fn default_timeout() -> Duration {
    Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS)
}

/// Clamp API-provided timeout values to a safe operational range.
#[must_use]
pub fn normalize_timeout_ms(timeout_ms: Option<u64>) -> Duration {
    let raw = timeout_ms.unwrap_or(DEFAULT_DISCOVERY_TIMEOUT_MS);
    Duration::from_millis(raw.clamp(MIN_DISCOVERY_TIMEOUT_MS, MAX_DISCOVERY_TIMEOUT_MS))
}

/// Resolve and validate requested discovery backends against configuration.
///
/// Returns backend identifiers in a deterministic order with duplicates removed.
///
/// # Errors
///
/// Returns an error when an unknown backend is requested or when a requested
/// backend is disabled by configuration.
pub fn resolve_backends(
    requested: Option<&[String]>,
    config: &HypercolorConfig,
) -> Result<Vec<DiscoveryBackend>, String> {
    let includes_all = requested.is_some_and(|raw| {
        raw.iter()
            .any(|item| item.trim().eq_ignore_ascii_case("all"))
    });
    let explicit_request = requested.is_some_and(|raw| !raw.is_empty()) && !includes_all;
    let mut candidates: Vec<String> = match requested {
        Some(raw) if !raw.is_empty() => raw.to_vec(),
        _ => vec!["wled".to_owned(), "openrgb".to_owned(), "usb".to_owned()],
    };

    if candidates
        .iter()
        .any(|item| item.trim().eq_ignore_ascii_case("all"))
    {
        candidates = vec!["wled".to_owned(), "openrgb".to_owned(), "usb".to_owned()];
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for candidate in candidates {
        let normalized = candidate.trim().to_ascii_lowercase();
        let backend = DiscoveryBackend::parse(&normalized).ok_or_else(|| {
            format!(
                "Unknown discovery backend '{candidate}'. Supported backends: wled, openrgb, usb"
            )
        })?;

        if !seen.insert(backend) {
            continue;
        }

        match backend {
            DiscoveryBackend::Wled => {
                if !config.discovery.wled_scan {
                    if explicit_request {
                        return Err(
                            "Discovery backend 'wled' is disabled by config (discovery.wled_scan=false)"
                                .to_owned(),
                        );
                    }
                    continue;
                }
                if !config.discovery.mdns_enabled {
                    if explicit_request {
                        return Err(
                            "Discovery backend 'wled' requires discovery.mdns_enabled=true"
                                .to_owned(),
                        );
                    }
                    continue;
                }
            }
            DiscoveryBackend::OpenRgb | DiscoveryBackend::Usb => {}
        }

        out.push(backend);
    }

    Ok(out)
}

/// Render backend enum values as stable string identifiers.
#[must_use]
pub fn backend_names(backends: &[DiscoveryBackend]) -> Vec<String> {
    backends
        .iter()
        .map(|backend| backend.as_str().to_owned())
        .collect()
}

/// Execute one full discovery scan and publish discovery events.
///
/// This function assumes the caller already set `in_progress=true`. It always
/// resets that flag on exit.
#[allow(clippy::too_many_lines)]
pub async fn execute_discovery_scan(
    runtime: DiscoveryRuntime,
    config: Arc<HypercolorConfig>,
    backends: Vec<DiscoveryBackend>,
    timeout: Duration,
) -> DiscoveryScanResult {
    let _flag_guard = DiscoveryFlagGuard {
        flag: Arc::clone(&runtime.in_progress),
    };
    let backend_names = backend_names(&backends);
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);

    runtime
        .event_bus
        .publish(HypercolorEvent::DeviceDiscoveryStarted {
            backends: backend_names.clone(),
        });

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
            DiscoveryBackend::Wled => {
                orchestrator.add_scanner(Box::new(WledScanner::with_timeout(timeout)));
            }
            DiscoveryBackend::OpenRgb => {
                let probe_timeout =
                    timeout.clamp(Duration::from_millis(250), Duration::from_secs(2));
                orchestrator.add_scanner(Box::new(OpenRgbScanner::new(OpenRgbScannerConfig {
                    host: config.discovery.openrgb_host.clone(),
                    port: config.discovery.openrgb_port,
                    probe_timeout,
                })));
            }
            DiscoveryBackend::Usb => {
                orchestrator.add_scanner(Box::new(UsbScanner::new()));
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

    let report = orchestrator.full_scan().await;
    let mut found = Vec::new();
    let mut new_devices = Vec::new();
    let mut reappeared_devices = Vec::new();

    let seen_ids: HashSet<DeviceId> = report
        .new_devices
        .iter()
        .chain(report.reappeared_devices.iter())
        .copied()
        .collect();

    for id in &report.new_devices {
        let Some(tracked) = runtime.device_registry.get(id).await else {
            continue;
        };

        let backend = backend_id_for_family(&tracked.info.family);
        let fingerprint = runtime.device_registry.fingerprint_for_id(id).await;
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_discovered(*id, &tracked.info, &backend, fingerprint.as_ref())
        };
        ensure_default_logical_for_device(
            &runtime,
            *id,
            &tracked.info.name,
            tracked.info.total_led_count(),
        )
        .await;
        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(&runtime, *id).await;

        let device_ref = device_ref_for_tracked(&tracked.info.family, &tracked.info);
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDiscovered {
                device_id: device_ref.id.clone(),
                name: device_ref.name.clone(),
                backend: device_ref.backend.clone(),
                led_count: device_ref.led_count,
                address: None,
            });

        found.push(device_ref.clone());
        new_devices.push(device_ref);
    }

    for id in &report.reappeared_devices {
        let Some(tracked) = runtime.device_registry.get(id).await else {
            continue;
        };

        let backend = backend_id_for_family(&tracked.info.family);
        let fingerprint = runtime.device_registry.fingerprint_for_id(id).await;
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_discovered(*id, &tracked.info, &backend, fingerprint.as_ref())
        };
        ensure_default_logical_for_device(
            &runtime,
            *id,
            &tracked.info.name,
            tracked.info.total_led_count(),
        )
        .await;
        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(&runtime, *id).await;

        let device_ref = device_ref_for_tracked(&tracked.info.family, &tracked.info);
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDiscovered {
                device_id: device_ref.id.clone(),
                name: device_ref.name.clone(),
                backend: device_ref.backend.clone(),
                led_count: device_ref.led_count,
                address: None,
            });

        found.push(device_ref.clone());
        reappeared_devices.push(device_ref);
    }

    let mut vanished_ids: HashSet<DeviceId> = report.vanished_devices.iter().copied().collect();
    let lifecycle_tracked_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.tracked_device_ids()
    };
    for id in lifecycle_tracked_ids {
        if !seen_ids.contains(&id) {
            vanished_ids.insert(id);
        }
    }

    let mut vanished_ids: Vec<DeviceId> = vanished_ids.into_iter().collect();
    vanished_ids.sort_by_key(DeviceId::as_uuid);

    let mut vanished_devices = Vec::new();
    for id in vanished_ids {
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

#[allow(clippy::too_many_lines)]
async fn execute_lifecycle_actions(runtime: DiscoveryRuntime, actions: Vec<LifecycleAction>) {
    let mut pending: VecDeque<LifecycleAction> = actions.into();

    while let Some(action) = pending.pop_front() {
        match action {
            LifecycleAction::Connect {
                device_id,
                backend_id,
                layout_device_id,
            } => {
                let result = {
                    let mut manager = runtime.backend_manager.lock().await;
                    manager
                        .connect_device(&backend_id, device_id, &layout_device_id)
                        .await
                };

                let (follow_up, connected) = match result {
                    Ok(()) => {
                        if let Err(error) =
                            refresh_connected_device_info(&runtime, &backend_id, device_id).await
                        {
                            let device_label = device_log_label(&runtime, device_id).await;
                            warn!(
                                device = %device_label,
                                device_id = %device_id,
                                backend_id = %backend_id,
                                error = %error,
                                error_chain = %format_error_chain(&error),
                                "failed to refresh device metadata after connect"
                            );
                        }
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        (lifecycle.on_connected(device_id), true)
                    }
                    Err(error) => {
                        let device_label = device_log_label(&runtime, device_id).await;
                        warn!(
                            device = %device_label,
                            device_id = %device_id,
                            backend_id = %backend_id,
                            layout_device_id = %layout_device_id,
                            error = %error,
                            error_chain = %format_error_chain(&error),
                            "lifecycle connect action failed"
                        );
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        (lifecycle.on_connect_failed(device_id), false)
                    }
                };

                match follow_up {
                    Ok(next_actions) => {
                        if connected {
                            sync_logical_mappings_for_device(
                                &runtime,
                                device_id,
                                &backend_id,
                                &layout_device_id,
                            )
                            .await;
                        }
                        pending.extend(next_actions);
                        sync_registry_state(&runtime, device_id).await;
                    }
                    Err(error) => {
                        let device_label = device_log_label(&runtime, device_id).await;
                        warn!(
                            device = %device_label,
                            device_id = %device_id,
                            error = %error,
                            "lifecycle state update failed after connect"
                        );
                    }
                }
            }
            LifecycleAction::Disconnect {
                device_id,
                backend_id,
            } => {
                let layout_device_id = {
                    let lifecycle = runtime.lifecycle_manager.lock().await;
                    lifecycle
                        .layout_device_id_for(device_id)
                        .map(ToOwned::to_owned)
                };

                let Some(layout_device_id) = layout_device_id else {
                    warn!(
                        device_id = %device_id,
                        backend_id = %backend_id,
                        "missing lifecycle layout id during disconnect action"
                    );
                    continue;
                };

                let result = {
                    let mut manager = runtime.backend_manager.lock().await;
                    manager
                        .disconnect_device(&backend_id, device_id, &layout_device_id)
                        .await
                };
                if let Err(error) = result {
                    warn!(
                        device_id = %device_id,
                        backend_id = %backend_id,
                        error = %error,
                        "lifecycle disconnect action failed"
                    );
                }
            }
            LifecycleAction::Map {
                layout_device_id,
                backend_id,
                device_id,
            } => {
                let mut manager = runtime.backend_manager.lock().await;
                manager.map_device(layout_device_id, backend_id, device_id);
            }
            LifecycleAction::Unmap { layout_device_id } => {
                let mut manager = runtime.backend_manager.lock().await;
                manager.unmap_device(&layout_device_id);
            }
            LifecycleAction::SpawnReconnect { device_id, delay } => {
                spawn_reconnect_task(&runtime, device_id, delay);
            }
            LifecycleAction::CancelReconnect { device_id } => {
                cancel_reconnect_task(&runtime, device_id);
            }
        }
    }
}

pub(crate) async fn handle_async_write_failures(
    runtime: &DiscoveryRuntime,
    failures: Vec<AsyncWriteFailure>,
) {
    let mut handled = HashSet::new();

    for failure in failures {
        if !handled.insert(failure.device_id) {
            continue;
        }

        let should_handle = {
            let lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle
                .state(failure.device_id)
                .is_some_and(|state| state.is_renderable())
        };

        if !should_handle {
            continue;
        }

        warn!(
            backend_id = %failure.backend_id,
            device_id = %failure.device_id,
            error = %failure.error,
            "async device write failed; entering reconnect flow"
        );

        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_comm_error(failure.device_id)
        };

        match actions {
            Ok(actions) => {
                execute_lifecycle_actions(runtime.clone(), actions).await;
                sync_registry_state(runtime, failure.device_id).await;
            }
            Err(error) => {
                warn!(
                    backend_id = %failure.backend_id,
                    device_id = %failure.device_id,
                    error = %error,
                    "failed to transition lifecycle after async device write error"
                );
            }
        }
    }
}

fn spawn_reconnect_task(runtime: &DiscoveryRuntime, device_id: DeviceId, delay: Duration) {
    debug!(
        device_id = %device_id,
        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        "scheduled reconnect attempt"
    );
    let runtime_for_task = runtime.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(delay).await;

        // Remove our own handle before executing follow-up logic so reschedules
        // do not fight with this running task.
        runtime_for_task
            .reconnect_tasks
            .lock()
            .expect("reconnect task map lock poisoned")
            .remove(&device_id);

        let connect_action = {
            let lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_reconnect_attempt(device_id)
        };
        let Some(LifecycleAction::Connect {
            backend_id,
            layout_device_id,
            ..
        }) = connect_action
        else {
            return;
        };

        debug!(
            device_id = %device_id,
            backend_id = %backend_id,
            layout_device_id = %layout_device_id,
            "starting reconnect attempt"
        );

        let connect_result = {
            let mut manager = runtime_for_task.backend_manager.lock().await;
            manager
                .connect_device(&backend_id, device_id, &layout_device_id)
                .await
        };

        let follow_up = if let Err(error) = connect_result {
            let device_label = device_log_label(&runtime_for_task, device_id).await;
            warn!(
                device = %device_label,
                device_id = %device_id,
                backend_id = %backend_id,
                layout_device_id = %layout_device_id,
                error = %error,
                error_chain = %format_error_chain(&error),
                "reconnect attempt failed"
            );
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_reconnect_failed(device_id)
        } else {
            sync_logical_mappings_for_device(
                &runtime_for_task,
                device_id,
                &backend_id,
                &layout_device_id,
            )
            .await;
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_connected(device_id)
        };

        match follow_up {
            Ok(actions) => {
                execute_lifecycle_actions(runtime_for_task.clone(), actions).await;
                sync_registry_state(&runtime_for_task, device_id).await;
            }
            Err(error) => {
                let device_label = device_log_label(&runtime_for_task, device_id).await;
                warn!(
                    device = %device_label,
                    device_id = %device_id,
                    error = %error,
                    "failed to update lifecycle state after reconnect attempt"
                );
            }
        }
    });

    let mut tasks = runtime
        .reconnect_tasks
        .lock()
        .expect("reconnect task map lock poisoned");
    if let Some(existing) = tasks.insert(device_id, task) {
        existing.abort();
    }
}

fn cancel_reconnect_task(runtime: &DiscoveryRuntime, device_id: DeviceId) {
    let mut tasks = runtime
        .reconnect_tasks
        .lock()
        .expect("reconnect task map lock poisoned");
    if let Some(handle) = tasks.remove(&device_id) {
        handle.abort();
    }
}

async fn sync_registry_state(runtime: &DiscoveryRuntime, device_id: DeviceId) {
    let state = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.state(device_id)
    };
    if let Some(state) = state {
        let _ = runtime.device_registry.set_state(&device_id, state).await;
    }
}

async fn refresh_connected_device_info(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    let maybe_info = {
        let manager = runtime.backend_manager.lock().await;
        manager.connected_device_info(backend_id, device_id).await?
    };

    if let Some(info) = maybe_info {
        let _ = runtime.device_registry.update_info(&device_id, info).await;
    }

    Ok(())
}

async fn ensure_default_logical_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    device_name: &str,
    led_count: u32,
) {
    let physical_layout_id = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle
            .layout_device_id_for(device_id)
            .map_or_else(|| format!("device:{device_id}"), ToOwned::to_owned)
    };

    let mut logical_store = runtime.logical_devices.write().await;
    logical_devices::ensure_default_logical_device(
        &mut logical_store,
        device_id,
        &physical_layout_id,
        device_name,
        led_count,
    );
}

async fn sync_logical_mappings_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    backend_id: &str,
    fallback_layout_id: &str,
) {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return;
    };

    let total_leds = tracked.info.total_led_count();
    ensure_default_logical_for_device(runtime, device_id, &tracked.info.name, total_leds).await;

    let logical_entries = {
        let logical_store = runtime.logical_devices.read().await;
        logical_devices::list_for_physical(&logical_store, device_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .collect::<Vec<_>>()
    };

    let mut manager = runtime.backend_manager.lock().await;
    let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);

    let fallback = hypercolor_core::device::SegmentRange::new(
        0,
        usize::try_from(total_leds).unwrap_or_default(),
    );

    if logical_entries.is_empty() {
        manager.map_device_with_segment(
            fallback_layout_id.to_owned(),
            backend_id.to_owned(),
            device_id,
            Some(fallback),
        );
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
        );
        return;
    }

    let mut default_enabled = false;
    for logical in logical_entries {
        let start = usize::try_from(logical.led_start).unwrap_or_default();
        let length = usize::try_from(logical.led_count).unwrap_or_default();
        if logical.id == fallback_layout_id {
            default_enabled = true;
        }
        manager.map_device_with_segment(
            logical.id,
            backend_id.to_owned(),
            device_id,
            Some(hypercolor_core::device::SegmentRange::new(start, length)),
        );
    }

    if default_enabled {
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
        );
    }
}

fn map_physical_device_alias(
    manager: &mut BackendManager,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
    segment: hypercolor_core::device::SegmentRange,
) {
    let physical_alias = device_id.to_string();
    if physical_alias == layout_device_id {
        return;
    }

    manager.map_device_with_segment(
        physical_alias,
        backend_id.to_owned(),
        device_id,
        Some(segment),
    );
}

async fn device_log_label(runtime: &DiscoveryRuntime, device_id: DeviceId) -> String {
    runtime.device_registry.get(&device_id).await.map_or_else(
        || device_id.to_string(),
        |tracked| format!("{} ({device_id})", tracked.info.name),
    )
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | caused_by: ")
}

fn backend_id_for_family(family: &DeviceFamily) -> String {
    match family {
        DeviceFamily::OpenRgb => "openrgb".to_owned(),
        DeviceFamily::Wled => "wled".to_owned(),
        DeviceFamily::Hue => "hue".to_owned(),
        DeviceFamily::Razer
        | DeviceFamily::Corsair
        | DeviceFamily::Dygma
        | DeviceFamily::LianLi
        | DeviceFamily::PrismRgb => "usb".to_owned(),
        DeviceFamily::Custom(name) => name.to_ascii_lowercase(),
    }
}

fn device_ref_for_tracked(
    family: &DeviceFamily,
    info: &hypercolor_types::device::DeviceInfo,
) -> DeviceRef {
    DeviceRef {
        id: info.id.to_string(),
        name: info.name.clone(),
        backend: backend_id_for_family(family),
        led_count: info.total_led_count(),
    }
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

struct DiscoveryFlagGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for DiscoveryFlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{DiscoveryBackend, default_timeout, normalize_timeout_ms, resolve_backends};
    use hypercolor_types::config::HypercolorConfig;

    #[test]
    fn default_timeout_is_ten_seconds() {
        assert_eq!(default_timeout().as_millis(), 10_000);
    }

    #[test]
    fn timeout_normalization_clamps_values() {
        assert_eq!(normalize_timeout_ms(Some(1)).as_millis(), 100);
        assert_eq!(normalize_timeout_ms(Some(65_000)).as_millis(), 60_000);
        assert_eq!(normalize_timeout_ms(None).as_millis(), 10_000);
    }

    #[test]
    fn resolve_backends_defaults_to_wled_and_openrgb() {
        let cfg = HypercolorConfig::default();
        let resolved = resolve_backends(None, &cfg).expect("default backends should resolve");
        assert_eq!(
            resolved,
            vec![
                DiscoveryBackend::Wled,
                DiscoveryBackend::OpenRgb,
                DiscoveryBackend::Usb
            ]
        );
    }

    #[test]
    fn resolve_backends_rejects_unknown_values() {
        let cfg = HypercolorConfig::default();
        let requested = vec!["unknown".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg).expect_err("unknown must fail");
        assert!(error.contains("Unknown discovery backend"));
    }

    #[test]
    fn resolve_backends_rejects_disabled_wled() {
        let mut cfg = HypercolorConfig::default();
        cfg.discovery.wled_scan = false;
        let requested = vec!["wled".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg).expect_err("wled must fail");
        assert!(error.contains("disabled"));
    }
}
