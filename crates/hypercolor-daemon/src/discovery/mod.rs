//! Shared device discovery runtime for daemon startup and API-triggered scans.

mod auto_layout;
mod device_helpers;
mod lifecycle;
mod scan;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::SpatialLayout;
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::scene_transactions::SceneTransactionQueue;

pub use auto_layout::{
    append_auto_layout_zones_for_device, reconcile_auto_layout_zones_for_device,
    sync_active_layout_connectivity, sync_active_layout_for_renderable_devices,
};
pub(crate) use device_helpers::{apply_persisted_device_settings, sync_registry_state};
pub(crate) use lifecycle::execute_lifecycle_actions;
pub(crate) use lifecycle::handle_async_write_failures;
pub use lifecycle::{
    UserEnabledStateResult, activate_pairable_device, apply_user_enabled_state,
    disconnect_tracked_device, release_renderable_devices, shutdown_renderable_devices,
};
pub use scan::{DiscoveryScanResult, execute_discovery_scan, execute_discovery_scan_if_idle};

const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
const MIN_DISCOVERY_TIMEOUT_MS: u64 = 100;
const MAX_DISCOVERY_TIMEOUT_MS: u64 = 60_000;

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

    /// Scene manager used to keep primary-group layouts aligned with the
    /// active spatial layout.
    pub scene_manager: Arc<RwLock<SceneManager>>,

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

    /// Frame-boundary scene changes mirrored into the render thread.
    pub scene_transactions: SceneTransactionQueue,

    /// Persistent JSON file for startup runtime session state.
    pub runtime_state_path: PathBuf,

    /// Shared per-device USB protocol configuration store.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Shared encrypted credential store for network device auth.
    pub credential_store: Arc<CredentialStore>,

    /// Shared "scan in progress" lock flag.
    pub in_progress: Arc<AtomicBool>,

    /// Main daemon runtime handle for detached background work.
    pub task_spawner: Handle,
}

/// Discovery backends currently implemented in runtime scans.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiscoveryBackend {
    Network(String),
    Usb,
    SmBus,
    Blocks,
}

impl DiscoveryBackend {
    /// Create a network-backed discovery target.
    #[must_use]
    pub fn network(id: impl Into<String>) -> Self {
        Self::Network(id.into())
    }

    /// Stable backend identifier used in request/response payloads.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Network(id) => id.as_str(),
            Self::Usb => "usb",
            Self::SmBus => "smbus",
            Self::Blocks => "blocks",
        }
    }

    fn parse(raw: &str, registry: &DriverRegistry) -> Option<Self> {
        match raw {
            "usb" => Some(Self::Usb),
            "smbus" => Some(Self::SmBus),
            "blocks" => Some(Self::Blocks),
            _ => registry
                .get(raw)
                .filter(|driver| driver.discovery().is_some())
                .map(|_| Self::network(raw)),
        }
    }

    /// All backend identifiers compiled into this daemon binary.
    fn all(registry: &DriverRegistry) -> Vec<Self> {
        let mut backends = registry
            .discovery_drivers()
            .into_iter()
            .map(|driver| Self::network(driver.descriptor().id))
            .collect::<Vec<_>>();
        backends.extend([Self::Usb, Self::SmBus, Self::Blocks]);
        backends
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
    driver_registry: &DriverRegistry,
) -> Result<Vec<DiscoveryBackend>, String> {
    let includes_all = requested.is_some_and(|raw| {
        raw.iter()
            .any(|item| item.trim().eq_ignore_ascii_case("all"))
    });
    let explicit_request = requested.is_some_and(|raw| !raw.is_empty()) && !includes_all;
    let compiled_backends = DiscoveryBackend::all(driver_registry);
    let all_backends: Vec<String> = compiled_backends
        .iter()
        .map(|backend| backend.as_str().to_owned())
        .collect();
    let mut candidates: Vec<String> = match requested {
        Some(raw) if !raw.is_empty() => raw.to_vec(),
        _ => all_backends.clone(),
    };

    if candidates
        .iter()
        .any(|item| item.trim().eq_ignore_ascii_case("all"))
    {
        candidates = all_backends;
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for candidate in candidates {
        let normalized = candidate.trim().to_ascii_lowercase();
        let supported: Vec<&str> = compiled_backends
            .iter()
            .map(DiscoveryBackend::as_str)
            .collect();
        let backend = DiscoveryBackend::parse(&normalized, driver_registry).ok_or_else(|| {
            format!(
                "Unknown discovery backend '{candidate}'. Supported backends: {}",
                supported.join(", ")
            )
        })?;

        if !seen.insert(backend.clone()) {
            continue;
        }

        match &backend {
            DiscoveryBackend::Network(driver_id) => {
                if !crate::network::driver_enabled(config, driver_id) {
                    if explicit_request {
                        let config_flag = crate::network::driver_config_flag(driver_id);
                        return Err(format!(
                            "Discovery backend '{driver_id}' is disabled by config ({config_flag}=false)"
                        ));
                    }
                    continue;
                }
            }
            DiscoveryBackend::Blocks => {
                if !config.discovery.blocks_scan {
                    if explicit_request {
                        return Err(
                            "Discovery backend 'blocks' is disabled by config (discovery.blocks_scan=false)"
                                .to_owned(),
                        );
                    }
                    continue;
                }
            }
            DiscoveryBackend::Usb => {
                if crate::network::enabled_hal_driver_ids(config).is_empty() {
                    if explicit_request {
                        return Err(
                            "Discovery backend 'usb' has no enabled HAL driver modules".to_owned()
                        );
                    }
                    continue;
                }
            }
            DiscoveryBackend::SmBus => {
                if !crate::network::hal_driver_enabled(config, "asus") {
                    if explicit_request {
                        return Err(
                            "Discovery backend 'smbus' is disabled by config (drivers.asus.enabled=false)"
                                .to_owned(),
                        );
                    }
                    continue;
                }
            }
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
    use crate::api::AppState;
    use hypercolor_types::config::HypercolorConfig;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
        DeviceOrigin, DeviceTopologyHint, ZoneInfo,
    };

    fn builtin_registry() -> AppState {
        AppState::new()
    }

    fn expected_default_backends(state: &AppState) -> Vec<DiscoveryBackend> {
        let mut backends = state
            .driver_registry
            .discovery_drivers()
            .into_iter()
            .map(|driver| DiscoveryBackend::network(driver.descriptor().id))
            .collect::<Vec<_>>();
        backends.extend([
            DiscoveryBackend::Usb,
            DiscoveryBackend::SmBus,
            DiscoveryBackend::Blocks,
        ]);
        backends
    }

    fn device_info_with_origin(origin: DeviceOrigin) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Test Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("test".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            origin,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

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
    fn resolve_backends_defaults_to_all() {
        let state = builtin_registry();
        let cfg = HypercolorConfig::default();
        let resolved = resolve_backends(None, &cfg, state.driver_registry.as_ref())
            .expect("default backends should resolve");
        assert_eq!(resolved, expected_default_backends(&state));
    }

    #[test]
    fn resolve_backends_rejects_unknown_values() {
        let state = builtin_registry();
        let cfg = HypercolorConfig::default();
        let requested = vec!["unknown".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("unknown must fail");
        assert!(error.contains("Unknown discovery backend"));
    }

    #[test]
    fn resolve_backends_rejects_disabled_wled() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("wled")
            .expect("wled config should exist")
            .enabled = false;
        let requested = vec!["wled".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("wled must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_backends_rejects_disabled_nanoleaf() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("nanoleaf")
            .expect("nanoleaf config should exist")
            .enabled = false;
        let requested = vec!["nanoleaf".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("nanoleaf must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_backends_rejects_disabled_hue() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("hue")
            .expect("hue config should exist")
            .enabled = false;
        let requested = vec!["hue".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("hue must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_backends_rejects_disabled_smbus_hal_driver() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "asus".to_owned(),
            hypercolor_types::config::DriverConfigEntry::disabled(Default::default()),
        );
        let requested = vec!["smbus".to_owned()];
        let error = resolve_backends(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("smbus must fail when ASUS HAL module is disabled");
        assert!(error.contains("drivers.asus.enabled=false"));
    }

    #[test]
    fn resolve_backends_keeps_wled_when_mdns_is_disabled() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.discovery.mdns_enabled = false;

        let resolved = resolve_backends(None, &cfg, state.driver_registry.as_ref())
            .expect("wled should still resolve");
        assert_eq!(resolved, expected_default_backends(&state));
    }

    #[test]
    fn backend_id_for_device_uses_device_origin() {
        let info =
            device_info_with_origin(DeviceOrigin::native("ableton", "usb", ConnectionType::Usb));

        assert_eq!(info.backend_id(), "usb");
    }
}
