//! Shared device discovery runtime for daemon startup and API-triggered scans.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::wled::{WledKnownTarget, WledScanner};
use hypercolor_core::device::{
    AsyncWriteFailure, BackendIo, BackendManager, DeviceLifecycleManager, DeviceRegistry,
    DiscoveryConnectBehavior, DiscoveryOrchestrator, DiscoveryProgress, LifecycleAction,
    ScannerScanReport, SmBusScanner, UsbProtocolConfigStore, UsbScanner,
};
use hypercolor_core::spatial::{SpatialEngine, generate_positions};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    DeviceError, DeviceFamily, DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint,
    DeviceUserSettings,
};
use hypercolor_types::event::{DeviceRef, DisconnectReason, HypercolorEvent, ZoneRef};
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::{DeviceSettingsStore, StoredDeviceSettings};
use crate::layout_auto_exclusions;
use crate::logical_devices::{self, LogicalDevice};
use crate::runtime_state;

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

    /// Active spatial layout used by the render loop.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Persisted layout store shared with the runtime/API.
    pub layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,

    /// Persistent path for the layout store.
    pub layouts_path: PathBuf,

    /// Layout-specific exclusions that block discovery-driven auto-layout reconciliation.
    pub layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,

    /// Logical device segmentation store.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Attachment template registry used to derive dynamic hardware topology.
    pub attachment_registry: Arc<RwLock<AttachmentRegistry>>,

    /// Saved attachment bindings keyed by physical device ID.
    pub attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,

    /// Persisted global and per-device output settings.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

    /// Persistent JSON file for startup runtime session state.
    pub runtime_state_path: PathBuf,

    /// Shared per-device USB protocol configuration store.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Shared "scan in progress" lock flag.
    pub in_progress: Arc<AtomicBool>,

    /// Main daemon runtime handle for detached background work.
    pub task_spawner: Handle,
}

/// Discovery backends currently implemented in runtime scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscoveryBackend {
    Wled,
    Usb,
    SmBus,
}

impl DiscoveryBackend {
    /// Stable backend identifier used in request/response payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wled => "wled",
            Self::Usb => "usb",
            Self::SmBus => "smbus",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "wled" => Some(Self::Wled),
            "usb" => Some(Self::Usb),
            "smbus" => Some(Self::SmBus),
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

/// Resolve the IPs that the WLED scanner should probe during discovery.
///
/// This merges explicit config with IP metadata cached from previous WLED
/// discoveries so a transient mDNS miss does not immediately orphan a device
/// that was recently reachable over HTTP.
pub async fn resolve_wled_probe_ips(
    device_registry: &DeviceRegistry,
    config: &HypercolorConfig,
    runtime_state_path: &std::path::Path,
) -> Vec<IpAddr> {
    let mut known_ips: HashSet<IpAddr> = config.wled.known_ips.iter().copied().collect();

    match runtime_state::load_wled_probe_ips(runtime_state_path) {
        Ok(cached_ips) => {
            known_ips.extend(cached_ips);
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "failed to load cached WLED probe IPs; ignoring persisted runtime cache"
            );
        }
    }

    known_ips.extend(runtime_state::collect_wled_probe_ips(device_registry).await);

    let mut resolved: Vec<IpAddr> = known_ips.into_iter().collect();
    resolved.sort_unstable();
    resolved
}

/// Resolve the WLED targets that discovery should probe, including cached
/// identity hints for friendly fallback labels when HTTP enrichment fails.
pub async fn resolve_wled_probe_targets(
    device_registry: &DeviceRegistry,
    config: &HypercolorConfig,
    runtime_state_path: &std::path::Path,
) -> Vec<WledKnownTarget> {
    let mut known_targets: HashMap<IpAddr, WledKnownTarget> = config
        .wled
        .known_ips
        .iter()
        .copied()
        .map(WledKnownTarget::from_ip)
        .map(|target| (target.ip, target))
        .collect();

    match runtime_state::load_wled_probe_ips(runtime_state_path) {
        Ok(cached_ips) => {
            for ip in cached_ips {
                known_targets
                    .entry(ip)
                    .or_insert_with(|| WledKnownTarget::from_ip(ip));
            }
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "failed to load cached WLED probe IPs; ignoring persisted runtime cache"
            );
        }
    }

    match runtime_state::load_wled_probe_targets(runtime_state_path) {
        Ok(cached_targets) => {
            for target in cached_targets {
                known_targets
                    .entry(target.ip)
                    .and_modify(|existing| existing.merge_from(&target))
                    .or_insert(target);
            }
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "failed to load cached WLED probe targets; ignoring persisted runtime cache"
            );
        }
    }

    for target in runtime_state::collect_wled_probe_targets(device_registry).await {
        known_targets
            .entry(target.ip)
            .and_modify(|existing| existing.merge_from(&target))
            .or_insert(target);
    }

    let mut resolved: Vec<WledKnownTarget> = known_targets.into_values().collect();
    resolved.sort_by_key(|target| target.ip);
    resolved
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
        _ => vec!["wled".to_owned(), "usb".to_owned(), "smbus".to_owned()],
    };

    if candidates
        .iter()
        .any(|item| item.trim().eq_ignore_ascii_case("all"))
    {
        candidates = vec!["wled".to_owned(), "usb".to_owned(), "smbus".to_owned()];
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for candidate in candidates {
        let normalized = candidate.trim().to_ascii_lowercase();
        let backend = DiscoveryBackend::parse(&normalized).ok_or_else(|| {
            format!("Unknown discovery backend '{candidate}'. Supported backends: wled, usb, smbus")
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
            }
            DiscoveryBackend::Usb | DiscoveryBackend::SmBus => {}
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
    let scanned_backend_ids = backend_names.iter().cloned().collect::<HashSet<_>>();
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
            DiscoveryBackend::Wled => {
                let known_targets = resolve_wled_probe_targets(
                    &runtime.device_registry,
                    config.as_ref(),
                    &runtime.runtime_state_path,
                )
                .await;
                orchestrator.add_scanner(Box::new(WledScanner::with_known_targets(
                    known_targets,
                    config.discovery.mdns_enabled,
                    timeout,
                )));
            }
            DiscoveryBackend::Usb => {
                orchestrator.add_scanner(Box::new(UsbScanner::new()));
            }
            DiscoveryBackend::SmBus => {
                orchestrator.add_scanner(Box::new(SmBusScanner::new()));
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
        let metadata = runtime
            .device_registry
            .metadata_for_id(&tracked.info.id)
            .await;
        let backend_id = backend_id_for_device(&tracked.info.family, metadata.as_ref());
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

    let metadata = runtime.device_registry.metadata_for_id(&device_id).await;
    let backend = backend_id_for_device(&tracked_before.info.family, metadata.as_ref());
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
    let metadata = runtime.device_registry.metadata_for_id(&device_id).await;
    let device_ref = device_ref_for_tracked(
        &tracked_after.info.family,
        &tracked_after.info,
        metadata.as_ref(),
    );

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEnabledStateResult {
    /// Lifecycle transition ran and registry state was synced.
    Applied,
    /// Device exists in the registry but has no lifecycle entry to drive.
    MissingLifecycle,
}

/// Apply a user-requested enabled/disabled state transition to a tracked device.
///
/// This routes through the lifecycle executor so disable operations disconnect
/// hardware and tear down routing instead of only flipping registry state.
pub async fn apply_user_enabled_state(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    enabled: bool,
) -> anyhow::Result<UserEnabledStateResult> {
    let should_activate = if enabled {
        let Some(tracked) = runtime.device_registry.get(&device_id).await else {
            return Ok(UserEnabledStateResult::MissingLifecycle);
        };
        let metadata = runtime.device_registry.metadata_for_id(&device_id).await;
        let backend = backend_id_for_device(&tracked.info.family, metadata.as_ref());
        let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;

        desired_connect_behavior(
            runtime,
            device_id,
            &tracked.info,
            &backend,
            fingerprint.as_ref(),
            tracked.connect_behavior,
            true,
        )
        .await
        .should_auto_connect()
    } else {
        false
    };

    let actions = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        let mut transition = if enabled {
            lifecycle.on_user_enable(device_id)
        } else {
            lifecycle.on_user_disable(device_id)
        };

        if enabled
            && !should_activate
            && let Ok(actions) = transition.as_mut()
        {
            actions.clear();
        }

        match transition {
            Ok(actions) => actions,
            Err(DeviceError::NotFound { .. }) => {
                return Ok(UserEnabledStateResult::MissingLifecycle);
            }
            Err(error) => return Err(error.into()),
        }
    };

    execute_lifecycle_actions(runtime.clone(), actions).await;
    sync_registry_state(runtime, device_id).await;

    if !enabled {
        sync_active_layout_for_renderable_devices(runtime, None).await;
    }

    Ok(UserEnabledStateResult::Applied)
}

/// Clear and disconnect every renderable device during daemon shutdown.
pub async fn shutdown_renderable_devices(runtime: &DiscoveryRuntime) -> usize {
    let tracked_device_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle
            .tracked_device_ids()
            .into_iter()
            .filter(|device_id| {
                lifecycle
                    .state(*device_id)
                    .is_some_and(|state| state.is_renderable())
            })
            .collect::<Vec<_>>()
    };

    let mut disconnected = 0_usize;

    for device_id in tracked_device_ids {
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_user_disable(device_id)
        };

        match actions {
            Ok(actions) => {
                execute_lifecycle_actions(runtime.clone(), actions).await;
                sync_registry_state(runtime, device_id).await;
                disconnected = disconnected.saturating_add(1);
            }
            Err(error) => {
                let device_label = device_log_label(runtime, device_id).await;
                warn!(
                    device = %device_label,
                    device_id = %device_id,
                    error = %error,
                    "failed to disable device during daemon shutdown cleanup"
                );
            }
        }
    }

    disconnected
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
                let result =
                    connect_backend_device(&runtime, &backend_id, device_id, &layout_device_id)
                        .await;

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
                        if connected {
                            let connected_only = HashSet::from([device_id]);
                            sync_active_layout_for_renderable_devices(
                                &runtime,
                                Some(&connected_only),
                            )
                            .await;
                            publish_device_connected(&runtime, &backend_id, device_id).await;
                        }
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

                let Some(_layout_device_id) = layout_device_id else {
                    warn!(
                        device_id = %device_id,
                        backend_id = %backend_id,
                        "missing lifecycle layout id during disconnect action"
                    );
                    continue;
                };

                let result = { disconnect_backend_device(&runtime, &backend_id, device_id).await };
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
    let task = runtime.task_spawner.spawn(async move {
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
            connect_backend_device(&runtime_for_task, &backend_id, device_id, &layout_device_id)
                .await
        };
        let reconnected = connect_result.is_ok();

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
                if reconnected {
                    let reconnect_only = HashSet::from([device_id]);
                    sync_active_layout_for_renderable_devices(
                        &runtime_for_task,
                        Some(&reconnect_only),
                    )
                    .await;
                    publish_device_connected(&runtime_for_task, &backend_id, device_id).await;
                }
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

async fn apply_persisted_device_settings(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
) -> DeviceUserSettings {
    let fallback_settings = runtime
        .device_registry
        .get(&device_id)
        .await
        .map_or_else(DeviceUserSettings::default, |tracked| tracked.user_settings);
    let key = runtime
        .device_registry
        .fingerprint_for_id(&device_id)
        .await
        .map_or_else(
            || device_id.to_string(),
            |fingerprint| fingerprint.to_string(),
        );
    let persisted_settings = {
        let store = runtime.device_settings.read().await;
        store
            .device_settings_for_key(&key)
            .map_or(fallback_settings, stored_device_settings_to_user_settings)
    };

    let _ = runtime
        .device_registry
        .replace_user_settings(&device_id, persisted_settings.clone())
        .await;

    let mut manager = runtime.backend_manager.lock().await;
    manager.set_device_output_brightness(device_id, persisted_settings.brightness);
    persisted_settings
}

fn stored_device_settings_to_user_settings(settings: StoredDeviceSettings) -> DeviceUserSettings {
    DeviceUserSettings {
        name: settings.name,
        enabled: !settings.disabled,
        brightness: settings.brightness.clamp(0.0, 1.0),
    }
}

async fn refresh_connected_device_info(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    let maybe_info = backend_io(runtime, backend_id)
        .await?
        .connected_device_info(device_id)
        .await?;

    if let Some(info) = maybe_info {
        let _ = runtime.device_registry.update_info(&device_id, info).await;
    }

    Ok(())
}

async fn backend_io(runtime: &DiscoveryRuntime, backend_id: &str) -> anyhow::Result<BackendIo> {
    let manager = runtime.backend_manager.lock().await;
    manager
        .backend_io(backend_id)
        .with_context(|| format!("backend '{backend_id}' is not registered"))
}

async fn apply_dynamic_usb_protocol_config(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) {
    if backend_id != "usb" {
        return;
    }

    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    };

    if tracked.info.family != DeviceFamily::PrismRgb
        || tracked.info.model.as_deref() != Some("prism_s")
    {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    }

    let prism_s_config = {
        let registry = runtime.attachment_registry.read().await;
        let profiles = runtime.attachment_profiles.read().await;
        profiles.prism_s_config_for_device(&tracked.info, &registry)
    };

    if let Some(config) = prism_s_config {
        runtime
            .usb_protocol_configs
            .set_prism_s_config(device_id, config)
            .await;
    } else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
    }
}

async fn connect_backend_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
) -> anyhow::Result<()> {
    apply_dynamic_usb_protocol_config(runtime, backend_id, device_id).await;
    let io = backend_io(runtime, backend_id).await?;
    let target_fps = io.connect_with_refresh(device_id).await?;

    let mut manager = runtime.backend_manager.lock().await;
    manager.set_cached_target_fps(backend_id, device_id, target_fps);
    manager.map_device(
        layout_device_id.to_owned(),
        backend_id.to_owned(),
        device_id,
    );
    Ok(())
}

async fn disconnect_backend_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    backend_io(runtime, backend_id)
        .await?
        .disconnect(device_id)
        .await?;

    let mut manager = runtime.backend_manager.lock().await;
    let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);
    runtime.usb_protocol_configs.remove_device(device_id).await;
    Ok(())
}

async fn ensure_default_logical_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    physical_layout_id: &str,
    device_name: &str,
    led_count: u32,
) {
    let mut logical_store = runtime.logical_devices.write().await;
    logical_devices::ensure_default_logical_device(
        &mut logical_store,
        device_id,
        physical_layout_id,
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
    ensure_default_logical_for_device(
        runtime,
        device_id,
        fallback_layout_id,
        &tracked.info.name,
        total_leds,
    )
    .await;

    let (logical_entries, legacy_default_ids) = {
        let logical_store = runtime.logical_devices.read().await;
        let legacy_ids = logical_devices::legacy_default_ids_for_physical(
            &logical_store,
            device_id,
            fallback_layout_id,
        );
        let entries = logical_devices::list_for_physical(&logical_store, device_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .collect::<Vec<_>>();
        (entries, legacy_ids)
    };

    let mut manager = runtime.backend_manager.lock().await;
    let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);

    let fallback = hypercolor_core::device::SegmentRange::new(
        0,
        usize::try_from(total_leds).unwrap_or_default(),
    );

    if logical_entries.is_empty() {
        map_device_with_zone_segments(
            &mut manager,
            fallback_layout_id.to_owned(),
            backend_id.to_owned(),
            device_id,
            Some(fallback),
            &tracked.info,
        );
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
            &tracked.info,
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
        map_device_with_zone_segments(
            &mut manager,
            logical.id,
            backend_id.to_owned(),
            device_id,
            Some(hypercolor_core::device::SegmentRange::new(start, length)),
            &tracked.info,
        );
    }

    if default_enabled {
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
            &tracked.info,
        );
    }

    for legacy_id in legacy_default_ids {
        map_device_with_zone_segments(
            &mut manager,
            legacy_id,
            backend_id.to_owned(),
            device_id,
            Some(fallback),
            &tracked.info,
        );
    }
}

async fn desired_connect_behavior(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    device_info: &DeviceInfo,
    backend_id: &str,
    fingerprint: Option<&DeviceFingerprint>,
    discovered_behavior: DiscoveryConnectBehavior,
    user_enabled: bool,
) -> DiscoveryConnectBehavior {
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(backend_id, device_info, fingerprint);
    ensure_default_logical_for_device(
        runtime,
        device_id,
        &layout_device_id,
        &device_info.name,
        device_info.total_led_count(),
    )
    .await;

    if !user_enabled || !discovered_behavior.should_auto_connect() {
        return DiscoveryConnectBehavior::Deferred;
    }

    if active_layout_targets_enabled_device(runtime, device_id, &layout_device_id).await {
        DiscoveryConnectBehavior::AutoConnect
    } else {
        DiscoveryConnectBehavior::Deferred
    }
}

async fn active_layout_targets_enabled_device(
    runtime: &DiscoveryRuntime,
    physical_id: DeviceId,
    layout_device_id: &str,
) -> bool {
    let candidate_ids = {
        let logical_store = runtime.logical_devices.read().await;
        let mut candidates = logical_devices::list_for_physical(&logical_store, physical_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.id)
            .collect::<HashSet<_>>();

        let default_enabled = logical_store
            .get(layout_device_id)
            .is_none_or(|entry| entry.enabled);
        if default_enabled {
            candidates.insert(layout_device_id.to_owned());
            candidates.extend(logical_devices::legacy_default_ids_for_physical(
                &logical_store,
                physical_id,
                layout_device_id,
            ));
            candidates.insert(physical_id.to_string());
            candidates.insert(format!("device:{physical_id}"));
        }

        candidates
    };

    let spatial = runtime.spatial_engine.read().await;
    spatial
        .layout()
        .zones
        .iter()
        .any(|zone| candidate_ids.contains(&zone.device_id))
}

#[doc(hidden)]
pub async fn sync_active_layout_connectivity(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    let tracked_devices = runtime.device_registry.list().await;

    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
            continue;
        }

        let metadata = runtime.device_registry.metadata_for_id(&device_id).await;
        let backend = backend_id_for_device(&tracked.info.family, metadata.as_ref());
        let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
        let connect_behavior = desired_connect_behavior(
            runtime,
            device_id,
            &tracked.info,
            &backend,
            fingerprint.as_ref(),
            tracked.connect_behavior,
            tracked.user_settings.enabled,
        )
        .await;

        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_discovered_with_behavior(
                device_id,
                &tracked.info,
                &backend,
                fingerprint.as_ref(),
                connect_behavior,
            )
        };
        if actions.is_empty() {
            continue;
        }

        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(runtime, device_id).await;
    }

    sync_active_layout_for_renderable_devices(runtime, limit_to_devices).await;
}

#[doc(hidden)]
#[allow(
    clippy::too_many_lines,
    reason = "layout reconciliation keeps the full discovery-driven repair flow in one place"
)]
pub async fn sync_active_layout_for_renderable_devices(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    let mut layout = {
        let spatial = runtime.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let excluded_layout_device_ids = {
        let store = runtime.layout_auto_exclusions.read().await;
        store.get(&layout.id).cloned().unwrap_or_default()
    };

    let inactive_ids = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .connected_devices_without_layout_targets(&layout)
            .into_iter()
            .map(|(_, device_id)| device_id)
            .collect::<HashSet<_>>()
    };

    let tracked_devices = runtime.device_registry.list().await;
    let logical_store = runtime.logical_devices.read().await.clone();
    let canonical_layout_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        tracked_devices
            .iter()
            .map(|tracked| {
                let device_id = tracked.info.id;
                let layout_id = lifecycle
                    .layout_device_id_for(device_id)
                    .map_or_else(|| format!("device:{device_id}"), ToOwned::to_owned);
                (device_id, layout_id)
            })
            .collect::<HashMap<_, _>>()
    };

    let mut repaired_devices = Vec::new();
    let mut repaired_zone_count = 0_usize;
    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        if !tracked.state.is_renderable() {
            continue;
        }
        if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
            continue;
        }

        let Some(layout_device_id) = canonical_layout_ids.get(&device_id) else {
            continue;
        };
        let default_enabled = logical_store
            .get(layout_device_id)
            .is_none_or(|entry| entry.enabled);
        if !default_enabled {
            if inactive_ids.contains(&device_id) {
                debug!(
                    device_id = %device_id,
                    device_name = %tracked.info.name,
                    layout_device_id = %layout_device_id,
                    "skipping auto-layout sync because the default logical device is disabled"
                );
            }
            continue;
        }
        if excluded_layout_device_ids.contains(layout_device_id) {
            if inactive_ids.contains(&device_id) {
                debug!(
                    device_id = %device_id,
                    device_name = %tracked.info.name,
                    layout_device_id = %layout_device_id,
                    layout_id = %layout.id,
                    "skipping auto-layout sync because the device is excluded from the active layout"
                );
            }
            continue;
        }

        let repaired =
            reconcile_auto_layout_zones_for_device(&mut layout, layout_device_id, &tracked.info);
        if repaired > 0 {
            repaired_zone_count = repaired_zone_count.saturating_add(repaired);
            repaired_devices.push(format!("{} ({device_id})", tracked.info.name));
        }

        if inactive_ids.contains(&device_id) {
            debug!(
                device_id = %device_id,
                device_name = %tracked.info.name,
                layout_device_id = %layout_device_id,
                zone_count = tracked.info.zones.len(),
                total_leds = tracked.info.total_led_count(),
                "leaving layout-inactive device out of the active layout until it is explicitly mapped"
            );
        }
    }

    if repaired_devices.is_empty() {
        return;
    }

    {
        let mut spatial = runtime.spatial_engine.write().await;
        spatial.update_layout(layout.clone());
    }

    let layouts_snapshot = {
        let mut layouts = runtime.layouts.write().await;
        layouts.insert(layout.id.clone(), layout.clone());
        layouts.clone()
    };
    if let Err(error) = crate::layout_store::save(&runtime.layouts_path, &layouts_snapshot) {
        warn!(
            path = %runtime.layouts_path.display(),
            %error,
            "failed to persist auto-updated layout store"
        );
    }

    info!(
        layout_id = %layout.id,
        layout_name = %layout.name,
        repaired_device_count = repaired_devices.len(),
        repaired_zone_count,
        repaired_devices = ?repaired_devices,
        "reconciled existing auto-layout zones in the active layout"
    );
}

#[doc(hidden)]
#[must_use]
pub fn append_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let eligible_zones = device_info
        .zones
        .iter()
        .filter(|zone| {
            zone.led_count > 0 && !matches!(zone.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    if eligible_zones.is_empty() {
        return 0;
    }

    let existing_device_count = layout
        .zones
        .iter()
        .map(|zone| zone.device_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let slot_center = auto_layout_slot_center(existing_device_count);

    for (index, zone_info) in eligible_zones.iter().enumerate() {
        let override_spec = auto_layout_override(layout_device_id, zone_info);
        let topology = override_spec.as_ref().map_or_else(
            || spatial_topology_for_zone(zone_info),
            |spec| spec.topology.clone(),
        );
        let (position, default_size) = auto_layout_geometry(
            slot_center,
            index,
            eligible_zones.len(),
            &zone_info.topology,
        );
        let size = override_spec
            .as_ref()
            .and_then(|spec| spec.size)
            .unwrap_or(default_size);
        let zone_id = unique_auto_zone_id(layout, layout_device_id, &zone_info.name);
        let zone_name = if eligible_zones.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, zone_info.name)
        };

        layout.zones.push(DeviceZone {
            id: zone_id,
            name: zone_name,
            device_id: layout_device_id.to_owned(),
            zone_name: Some(zone_info.name.clone()),
            group_id: None,
            position,
            size,
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: topology.clone(),
            led_positions: generate_positions(&topology),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: override_spec
                .as_ref()
                .and_then(|spec| spec.shape.clone())
                .or_else(|| auto_layout_shape(&zone_info.topology)),
            shape_preset: None,
            attachment: None,
        });
    }

    eligible_zones.len()
}

#[doc(hidden)]
#[must_use]
pub fn reconcile_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let auto_zone_prefix = format!("auto-{}-", sanitize_auto_layout_component(layout_device_id));
    let eligible_zones = device_info
        .zones
        .iter()
        .filter(|zone| {
            zone.led_count > 0 && !matches!(zone.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    let expected_zone_names = eligible_zones
        .iter()
        .map(|zone| zone.name.as_str())
        .collect::<HashSet<_>>();
    let before_len = layout.zones.len();
    layout.zones.retain(|zone| {
        if zone.device_id != layout_device_id || !zone.id.starts_with(&auto_zone_prefix) {
            return true;
        }

        zone.zone_name
            .as_deref()
            .is_some_and(|zone_name| expected_zone_names.contains(zone_name))
    });

    let mut repaired = before_len.saturating_sub(layout.zones.len());
    if eligible_zones.is_empty() {
        return repaired;
    }

    for (index, zone_info) in eligible_zones.iter().enumerate() {
        let override_spec = auto_layout_override(layout_device_id, zone_info);
        let expected_topology = override_spec.as_ref().map_or_else(
            || spatial_topology_for_zone(zone_info),
            |spec| spec.topology.clone(),
        );

        let (_, default_size) = auto_layout_geometry(
            NormalizedPosition::new(0.5, 0.5),
            index,
            eligible_zones.len(),
            &zone_info.topology,
        );
        let expected_size = override_spec
            .as_ref()
            .and_then(|spec| spec.size)
            .unwrap_or(default_size);
        let expected_name = if eligible_zones.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, zone_info.name)
        };
        let expected_positions = generate_positions(&expected_topology);
        let expected_shape = override_spec
            .as_ref()
            .and_then(|spec| spec.shape.clone())
            .or_else(|| auto_layout_shape(&zone_info.topology));

        for zone in layout.zones.iter_mut().filter(|zone| {
            zone.device_id == layout_device_id
                && zone.zone_name.as_deref() == Some(zone_info.name.as_str())
                && zone.id.starts_with(&auto_zone_prefix)
        }) {
            let mut changed = false;

            if zone.name != expected_name {
                zone.name.clone_from(&expected_name);
                changed = true;
            }
            if zone.topology != expected_topology {
                zone.topology = expected_topology.clone();
                changed = true;
            }
            if zone.led_positions != expected_positions {
                zone.led_positions.clone_from(&expected_positions);
                changed = true;
            }
            if zone.shape != expected_shape {
                zone.shape.clone_from(&expected_shape);
                changed = true;
            }
            if zone.size != expected_size {
                zone.size = expected_size;
                changed = true;
            }

            if changed {
                repaired = repaired.saturating_add(1);
            }
        }
    }

    repaired
}

fn auto_layout_slot_center(slot_index: usize) -> NormalizedPosition {
    const COLUMNS: usize = 3;
    const LEFT_X: f32 = 0.18;
    const TOP_Y: f32 = 0.18;
    const X_SPACING: f32 = 0.32;
    const Y_SPACING: f32 = 0.22;
    let column = slot_index % COLUMNS;
    let row = slot_index / COLUMNS;

    let column_f32 = f32::from(u16::try_from(column).unwrap_or(u16::MAX));
    let row_f32 = f32::from(u16::try_from(row).unwrap_or(u16::MAX));
    NormalizedPosition::new(
        (LEFT_X + X_SPACING * column_f32).clamp(0.12, 0.88),
        (TOP_Y + Y_SPACING * row_f32).clamp(0.14, 0.86),
    )
}

fn auto_layout_geometry(
    slot_center: NormalizedPosition,
    zone_index: usize,
    zone_count: usize,
    topology: &DeviceTopologyHint,
) -> (NormalizedPosition, NormalizedPosition) {
    let slot_width = 0.26;
    let slot_height = 0.18;
    let zone_count_f32 = f32::from(u16::try_from(zone_count.max(1)).unwrap_or(u16::MAX));
    let zone_index_f32 = f32::from(u16::try_from(zone_index).unwrap_or(u16::MAX));
    let steps = zone_count.saturating_sub(1);
    let steps_f32 = f32::from(u16::try_from(steps).unwrap_or(u16::MAX));
    let step = if zone_count <= 1 {
        0.0
    } else {
        (slot_height / zone_count_f32).min(0.08)
    };
    let offset = if zone_count <= 1 {
        0.0
    } else {
        -step * steps_f32 / 2.0 + step * zone_index_f32
    };
    let position = NormalizedPosition::new(slot_center.x, (slot_center.y + offset).clamp(0.1, 0.9));

    let size = match topology {
        DeviceTopologyHint::Strip | DeviceTopologyHint::Custom => {
            NormalizedPosition::new(slot_width, (slot_height / zone_count_f32).clamp(0.05, 0.1))
        }
        DeviceTopologyHint::Matrix { rows, cols } => {
            let rows_f32 = f32::from(u16::try_from(*rows).unwrap_or(u16::MAX));
            let cols_f32 = f32::from(u16::try_from(*cols).unwrap_or(u16::MAX));
            let aspect = if rows_f32 <= 0.0 {
                1.0
            } else {
                cols_f32 / rows_f32
            };
            let width = 0.18_f32.clamp(0.12, slot_width);
            // Dense multi-zone devices cannot always preserve the preferred minimum matrix height.
            let max_height = slot_height / zone_count_f32;
            let min_height = max_height.min(0.08);
            let height = (width / aspect).clamp(min_height, max_height);
            NormalizedPosition::new(width, height)
        }
        DeviceTopologyHint::Ring { .. } => {
            let diameter = (0.16 / zone_count_f32.max(1.0)).clamp(0.08, 0.16);
            NormalizedPosition::new(diameter, diameter)
        }
        DeviceTopologyHint::Point => NormalizedPosition::new(0.08, 0.08),
        DeviceTopologyHint::Display { .. } => NormalizedPosition::new(0.18, 0.12),
    };

    (position, size)
}

#[derive(Clone)]
struct AutoLayoutOverride {
    topology: LedTopology,
    size: Option<NormalizedPosition>,
    shape: Option<ZoneShape>,
}

fn auto_layout_override(
    layout_device_id: &str,
    zone_info: &hypercolor_types::device::ZoneInfo,
) -> Option<AutoLayoutOverride> {
    if layout_device_id.starts_with("usb:1532:056f:") && zone_info.led_count == 10 {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    6,
                    2,
                    &[
                        (1, 0),
                        (2, 0),
                        (3, 0),
                        (4, 0),
                        (0, 1),
                        (1, 1),
                        (2, 1),
                        (3, 1),
                        (4, 1),
                        (5, 1),
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.2, 0.08)),
            shape: Some(ZoneShape::Rectangle),
        });
    }

    if layout_device_id.starts_with("usb:1532:0099:") && zone_info.led_count == 11 {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    7,
                    8,
                    &[
                        (3, 5),
                        (3, 1),
                        (1, 1),
                        (0, 2),
                        (0, 3),
                        (0, 4),
                        (2, 6),
                        (4, 6),
                        (5, 3),
                        (6, 2),
                        (6, 1),
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.16, 0.18)),
            shape: Some(ZoneShape::Rectangle),
        });
    }

    None
}

fn normalized_grid_positions(
    width: u32,
    height: u32,
    coordinates: &[(u32, u32)],
) -> Vec<NormalizedPosition> {
    let x_divisor = f32::from(u16::try_from(width.saturating_sub(1).max(1)).unwrap_or(u16::MAX));
    let y_divisor = f32::from(u16::try_from(height.saturating_sub(1).max(1)).unwrap_or(u16::MAX));

    coordinates
        .iter()
        .map(|&(x, y)| {
            NormalizedPosition::new(
                f32::from(u16::try_from(x).unwrap_or(u16::MAX)) / x_divisor,
                f32::from(u16::try_from(y).unwrap_or(u16::MAX)) / y_divisor,
            )
        })
        .collect()
}

fn spatial_topology_for_zone(zone_info: &hypercolor_types::device::ZoneInfo) -> LedTopology {
    match zone_info.topology {
        DeviceTopologyHint::Strip
        | DeviceTopologyHint::Custom
        | DeviceTopologyHint::Display { .. } => LedTopology::Strip {
            count: zone_info.led_count,
            direction: StripDirection::LeftToRight,
        },
        DeviceTopologyHint::Matrix { rows, cols } => LedTopology::Matrix {
            width: cols,
            height: rows,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        DeviceTopologyHint::Ring { count } => LedTopology::Ring {
            count,
            start_angle: 0.0,
            direction: Winding::Clockwise,
        },
        DeviceTopologyHint::Point => LedTopology::Point,
    }
}

fn auto_layout_shape(topology: &DeviceTopologyHint) -> Option<ZoneShape> {
    match topology {
        DeviceTopologyHint::Ring { .. } => Some(ZoneShape::Ring),
        DeviceTopologyHint::Point => None,
        DeviceTopologyHint::Strip
        | DeviceTopologyHint::Matrix { .. }
        | DeviceTopologyHint::Custom
        | DeviceTopologyHint::Display { .. } => Some(ZoneShape::Rectangle),
    }
}

fn unique_auto_zone_id(layout: &SpatialLayout, layout_device_id: &str, zone_name: &str) -> String {
    let device_component = sanitize_auto_layout_component(layout_device_id);
    let zone_component = sanitize_auto_layout_component(zone_name);
    let base = format!("auto-{device_component}-{zone_component}");
    if !layout.zones.iter().any(|zone| zone.id == base) {
        return base;
    }

    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !layout.zones.iter().any(|zone| zone.id == candidate) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn sanitize_auto_layout_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_was_dash = false;
    for ch in raw.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch == '_' || ch == ':' || ch.is_ascii_whitespace() {
            Some('-')
        } else {
            None
        };

        let Some(ch) = normalized else {
            continue;
        };

        if ch == '-' {
            if prev_was_dash || out.is_empty() {
                continue;
            }
            prev_was_dash = true;
            out.push(ch);
            continue;
        }

        prev_was_dash = false;
        out.push(ch);
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "zone".to_owned()
    } else {
        out
    }
}

fn map_device_with_zone_segments(
    manager: &mut BackendManager,
    layout_device_id: impl Into<String>,
    backend_id: impl Into<String>,
    device_id: DeviceId,
    segment: Option<hypercolor_core::device::SegmentRange>,
    device_info: &hypercolor_types::device::DeviceInfo,
) {
    let layout_device_id = layout_device_id.into();
    manager.map_device_with_segment(layout_device_id.clone(), backend_id, device_id, segment);
    let _ = manager.set_device_zone_segments(&layout_device_id, device_info);
}

fn map_physical_device_alias(
    manager: &mut BackendManager,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
    segment: hypercolor_core::device::SegmentRange,
    device_info: &hypercolor_types::device::DeviceInfo,
) {
    let physical_alias = device_id.to_string();
    if physical_alias != layout_device_id {
        map_device_with_zone_segments(
            manager,
            physical_alias,
            backend_id.to_owned(),
            device_id,
            Some(segment),
            device_info,
        );
    }

    let legacy_alias = format!("device:{device_id}");
    if legacy_alias != layout_device_id {
        map_device_with_zone_segments(
            manager,
            legacy_alias,
            backend_id.to_owned(),
            device_id,
            Some(segment),
            device_info,
        );
    }
}

async fn publish_device_connected(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return;
    };

    let zones = build_zone_refs(&tracked.info);
    info!(
        device = %tracked.info.name,
        device_id = %tracked.info.id,
        backend = %backend_id,
        led_count = tracked.info.total_led_count(),
        zones = zones.len(),
        "device connected"
    );
    runtime.event_bus.publish(HypercolorEvent::DeviceConnected {
        device_id: tracked.info.id.to_string(),
        name: tracked.info.name.clone(),
        backend: backend_id.to_owned(),
        led_count: tracked.info.total_led_count(),
        zones,
    });
}

fn build_zone_refs(info: &DeviceInfo) -> Vec<ZoneRef> {
    info.zones
        .iter()
        .map(|zone| ZoneRef {
            zone_id: format!("{}:{}", info.id, zone.name),
            device_id: info.id.to_string(),
            topology: topology_hint_name(&zone.topology).to_owned(),
            led_count: zone.led_count,
        })
        .collect()
}

const fn topology_hint_name(topology: &DeviceTopologyHint) -> &'static str {
    match topology {
        DeviceTopologyHint::Strip => "strip",
        DeviceTopologyHint::Matrix { .. } => "matrix",
        DeviceTopologyHint::Ring { .. } => "ring",
        DeviceTopologyHint::Point => "point",
        DeviceTopologyHint::Display { .. } => "display",
        DeviceTopologyHint::Custom => "custom",
    }
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

pub(crate) fn backend_id_for_family(family: &DeviceFamily) -> String {
    match family {
        DeviceFamily::Wled => "wled".to_owned(),
        DeviceFamily::Hue => "hue".to_owned(),
        DeviceFamily::Razer
        | DeviceFamily::Corsair
        | DeviceFamily::Dygma
        | DeviceFamily::LianLi
        | DeviceFamily::PrismRgb
        | DeviceFamily::Asus => "usb".to_owned(),
        DeviceFamily::Custom(name) => name.to_ascii_lowercase(),
    }
}

pub(crate) fn backend_id_for_device(
    family: &DeviceFamily,
    metadata: Option<&HashMap<String, String>>,
) -> String {
    if let Some(metadata) = metadata {
        if let Some(backend_id) = metadata.get("backend_id")
            && !backend_id.trim().is_empty()
        {
            return backend_id.clone();
        }

        let has_usb_identity = metadata.contains_key("usb_path")
            || (metadata.contains_key("vendor_id") && metadata.contains_key("product_id"));
        if has_usb_identity {
            return "usb".to_owned();
        }
    }

    metadata
        .and_then(|metadata| metadata.get("backend_id"))
        .filter(|backend_id| !backend_id.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| backend_id_for_family(family))
}

fn device_ref_for_tracked(
    family: &DeviceFamily,
    info: &hypercolor_types::device::DeviceInfo,
    metadata: Option<&HashMap<String, String>>,
) -> DeviceRef {
    DeviceRef {
        id: info.id.to_string(),
        name: info.name.clone(),
        backend: backend_id_for_device(family, metadata),
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
    use std::collections::HashMap;

    use super::{
        DiscoveryBackend, backend_id_for_device, default_timeout, normalize_timeout_ms,
        resolve_backends,
    };
    use hypercolor_types::{config::HypercolorConfig, device::DeviceFamily};

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
    fn resolve_backends_defaults_to_wled_usb_and_smbus() {
        let cfg = HypercolorConfig::default();
        let resolved = resolve_backends(None, &cfg).expect("default backends should resolve");
        assert_eq!(
            resolved,
            vec![
                DiscoveryBackend::Wled,
                DiscoveryBackend::Usb,
                DiscoveryBackend::SmBus,
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

    #[test]
    fn resolve_backends_keeps_wled_when_mdns_is_disabled() {
        let mut cfg = HypercolorConfig::default();
        cfg.discovery.mdns_enabled = false;

        let resolved = resolve_backends(None, &cfg).expect("wled should still resolve");
        assert_eq!(
            resolved,
            vec![
                DiscoveryBackend::Wled,
                DiscoveryBackend::Usb,
                DiscoveryBackend::SmBus,
            ]
        );
    }

    #[test]
    fn backend_id_for_device_prefers_scanner_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("backend_id".to_owned(), "smbus".to_owned());

        assert_eq!(
            backend_id_for_device(&DeviceFamily::Asus, Some(&metadata)),
            "smbus"
        );
    }

    #[test]
    fn backend_id_for_device_infers_usb_from_usb_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("vendor_id".to_owned(), "0x2982".to_owned());
        metadata.insert("product_id".to_owned(), "0x1967".to_owned());
        metadata.insert("usb_path".to_owned(), "001-12".to_owned());

        assert_eq!(
            backend_id_for_device(&DeviceFamily::Custom("Ableton".to_owned()), Some(&metadata)),
            "usb"
        );
    }

    #[test]
    fn backend_id_for_device_keeps_custom_fallback_without_usb_metadata() {
        assert_eq!(
            backend_id_for_device(&DeviceFamily::Custom("Ableton".to_owned()), None),
            "ableton"
        );
    }
}
