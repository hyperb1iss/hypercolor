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
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{DeviceId, DriverTransportKind};
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

/// Discovery target kind used by the scanner builder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum DiscoveryTargetKind {
    Driver,
    Usb,
    SmBus,
    Blocks,
}

/// Opaque discovery target resolved from driver modules and host transports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscoveryTarget {
    id: String,
    kind: DiscoveryTargetKind,
    preserves_renderable_on_miss: bool,
}

impl DiscoveryTarget {
    /// Create a driver-backed discovery target.
    #[must_use]
    pub fn driver(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: DiscoveryTargetKind::Driver,
            preserves_renderable_on_miss: false,
        }
    }

    /// Create the host USB discovery target.
    #[must_use]
    pub fn usb() -> Self {
        Self {
            id: "usb".to_owned(),
            kind: DiscoveryTargetKind::Usb,
            preserves_renderable_on_miss: false,
        }
    }

    /// Create the host SMBus discovery target.
    #[must_use]
    pub fn smbus() -> Self {
        Self {
            id: "smbus".to_owned(),
            kind: DiscoveryTargetKind::SmBus,
            preserves_renderable_on_miss: true,
        }
    }

    /// Create the host Blocks bridge discovery target.
    #[must_use]
    pub fn blocks() -> Self {
        Self {
            id: "blocks".to_owned(),
            kind: DiscoveryTargetKind::Blocks,
            preserves_renderable_on_miss: false,
        }
    }

    /// Stable discovery target identifier used in request/response payloads.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }

    /// Whether a missed device should remain renderable after a clean scan.
    #[must_use]
    pub fn preserves_renderable_on_discovery_miss(&self) -> bool {
        self.preserves_renderable_on_miss
    }

    pub(super) const fn kind(&self) -> &DiscoveryTargetKind {
        &self.kind
    }

    fn parse(raw: &str, registry: &DriverModuleRegistry) -> Option<Self> {
        match raw {
            "usb" => Some(Self::usb()),
            "smbus" => Some(Self::smbus()),
            "blocks" => Some(Self::blocks()),
            _ => registry
                .get(raw)
                .filter(|driver| driver.discovery().is_some())
                .map(|_| Self::driver(raw)),
        }
    }

    /// All discovery targets compiled into this daemon binary.
    fn all(registry: &DriverModuleRegistry) -> Vec<Self> {
        let mut targets = registry
            .discovery_drivers()
            .into_iter()
            .map(|driver| Self::driver(driver.descriptor().id))
            .collect::<Vec<_>>();
        targets.extend([Self::usb(), Self::smbus(), Self::blocks()]);
        targets
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

/// Resolve and validate requested discovery targets against configuration.
///
/// Returns target identifiers in a deterministic order with duplicates removed.
///
/// # Errors
///
/// Returns an error when an unknown target is requested or when a requested
/// target is disabled by configuration.
pub fn resolve_targets(
    requested: Option<&[String]>,
    config: &HypercolorConfig,
    driver_registry: &DriverModuleRegistry,
) -> Result<Vec<DiscoveryTarget>, String> {
    let includes_all = requested.is_some_and(|raw| {
        raw.iter()
            .any(|item| item.trim().eq_ignore_ascii_case("all"))
    });
    let explicit_request = requested.is_some_and(|raw| !raw.is_empty()) && !includes_all;
    let compiled_targets = DiscoveryTarget::all(driver_registry);
    let all_targets: Vec<String> = compiled_targets
        .iter()
        .map(|target| target.as_str().to_owned())
        .collect();
    let mut candidates: Vec<String> = match requested {
        Some(raw) if !raw.is_empty() => raw.to_vec(),
        _ => all_targets.clone(),
    };

    if candidates
        .iter()
        .any(|item| item.trim().eq_ignore_ascii_case("all"))
    {
        candidates = all_targets;
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for candidate in candidates {
        let normalized = candidate.trim().to_ascii_lowercase();
        let supported: Vec<&str> = compiled_targets
            .iter()
            .map(DiscoveryTarget::as_str)
            .collect();
        let target = DiscoveryTarget::parse(&normalized, driver_registry).ok_or_else(|| {
            format!(
                "Unknown discovery target '{candidate}'. Supported targets: {}",
                supported.join(", ")
            )
        })?;

        if !seen.insert(target.clone()) {
            continue;
        }

        match target.kind() {
            DiscoveryTargetKind::Driver => {
                let driver_id = target.as_str();
                let enabled = driver_registry.get(driver_id).is_some_and(|driver| {
                    crate::network::module_enabled(config, &driver.module_descriptor())
                });
                if !enabled {
                    if explicit_request {
                        let config_flag = crate::network::driver_config_flag(driver_id);
                        return Err(format!(
                            "Discovery target '{driver_id}' is disabled by config ({config_flag}=false)"
                        ));
                    }
                    continue;
                }
            }
            DiscoveryTargetKind::Blocks => {
                if !config.discovery.blocks_scan {
                    if explicit_request {
                        return Err(
                            "Discovery target 'blocks' is disabled by config (discovery.blocks_scan=false)"
                                .to_owned(),
                        );
                    }
                    continue;
                }
            }
            DiscoveryTargetKind::Usb => {
                if crate::network::enabled_hal_driver_ids(config).is_empty() {
                    if explicit_request {
                        return Err(
                            "Discovery target 'usb' has no enabled HAL driver modules".to_owned()
                        );
                    }
                    continue;
                }
            }
            DiscoveryTargetKind::SmBus => {
                if crate::network::enabled_hal_driver_ids_for_transport(
                    config,
                    &DriverTransportKind::Smbus,
                )
                .is_empty()
                {
                    if explicit_request {
                        return Err(
                            "Discovery target 'smbus' has no enabled SMBus HAL driver modules"
                                .to_owned(),
                        );
                    }
                    continue;
                }
            }
        }

        out.push(target);
    }

    Ok(out)
}

/// Render discovery targets as stable string identifiers.
#[must_use]
pub fn target_names(targets: &[DiscoveryTarget]) -> Vec<String> {
    targets
        .iter()
        .map(|target| target.as_str().to_owned())
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
    use super::{DiscoveryTarget, default_timeout, normalize_timeout_ms, resolve_targets};
    use crate::api::AppState;
    use hypercolor_driver_api::{
        DeviceBackend, DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigView,
        DriverDescriptor, DriverModule, DriverTransport,
    };
    use hypercolor_network::DriverModuleRegistry;
    use hypercolor_types::config::HypercolorConfig;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
        DeviceOrigin, DeviceTopologyHint, DriverModuleDescriptor, ZoneInfo,
    };

    fn builtin_registry() -> AppState {
        AppState::new()
    }

    struct DefaultDisabledDiscoveryDriver;

    static DEFAULT_DISABLED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "default-disabled",
        "Default Disabled",
        DriverTransport::Network,
        true,
        false,
    );

    impl DriverModule for DefaultDisabledDiscoveryDriver {
        fn descriptor(&self) -> &'static DriverDescriptor {
            &DEFAULT_DISABLED_DESCRIPTOR
        }

        fn module_descriptor(&self) -> DriverModuleDescriptor {
            let mut descriptor = self.descriptor().module_descriptor();
            descriptor.default_enabled = false;
            descriptor
        }

        fn build_output_backend(
            &self,
            host: &dyn hypercolor_driver_api::DriverHost,
            config: DriverConfigView<'_>,
        ) -> anyhow::Result<Option<Box<dyn DeviceBackend>>> {
            let _ = (host, config);
            Ok(None)
        }

        fn has_output_backend(&self) -> bool {
            false
        }

        fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
            Some(self)
        }
    }

    #[async_trait::async_trait]
    impl DiscoveryCapability for DefaultDisabledDiscoveryDriver {
        async fn discover(
            &self,
            host: &dyn hypercolor_driver_api::DriverHost,
            request: &DiscoveryRequest,
            config: DriverConfigView<'_>,
        ) -> anyhow::Result<DiscoveryResult> {
            let _ = (host, request, config);
            Ok(DiscoveryResult {
                devices: Vec::new(),
            })
        }
    }

    fn expected_default_targets(state: &AppState) -> Vec<DiscoveryTarget> {
        let mut targets = state
            .driver_registry
            .discovery_drivers()
            .into_iter()
            .map(|driver| DiscoveryTarget::driver(driver.descriptor().id))
            .collect::<Vec<_>>();
        targets.extend([
            DiscoveryTarget::usb(),
            DiscoveryTarget::smbus(),
            DiscoveryTarget::blocks(),
        ]);
        targets
    }

    fn device_info_with_origin(origin: DeviceOrigin) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Test Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::named("test"),
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
    fn resolve_targets_defaults_to_all() {
        let state = builtin_registry();
        let cfg = HypercolorConfig::default();
        let resolved = resolve_targets(None, &cfg, state.driver_registry.as_ref())
            .expect("default targets should resolve");
        assert_eq!(resolved, expected_default_targets(&state));
    }

    #[test]
    fn resolve_targets_rejects_unknown_values() {
        let state = builtin_registry();
        let cfg = HypercolorConfig::default();
        let requested = vec!["unknown".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("unknown must fail");
        assert!(error.contains("Unknown discovery target"));
    }

    #[test]
    fn resolve_targets_rejects_disabled_wled() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("wled")
            .expect("wled config should exist")
            .enabled = false;
        let requested = vec!["wled".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("wled must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_targets_honors_driver_default_enabled_flag() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(DefaultDisabledDiscoveryDriver)
            .expect("driver should register");
        let cfg = HypercolorConfig::default();

        let resolved =
            resolve_targets(None, &cfg, &registry).expect("default targets should resolve");

        assert!(
            !resolved
                .iter()
                .any(|target| target.as_str() == "default-disabled")
        );
    }

    #[test]
    fn resolve_targets_rejects_explicit_default_disabled_driver() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(DefaultDisabledDiscoveryDriver)
            .expect("driver should register");
        let cfg = HypercolorConfig::default();
        let requested = vec!["default-disabled".to_owned()];

        let error = resolve_targets(Some(&requested), &cfg, &registry)
            .expect_err("default-disabled driver must fail");

        assert!(error.contains("drivers.default-disabled.enabled=false"));
    }

    #[test]
    fn resolve_targets_rejects_disabled_nanoleaf() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("nanoleaf")
            .expect("nanoleaf config should exist")
            .enabled = false;
        let requested = vec!["nanoleaf".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("nanoleaf must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_targets_rejects_disabled_hue() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers
            .get_mut("hue")
            .expect("hue config should exist")
            .enabled = false;
        let requested = vec!["hue".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("hue must fail");
        assert!(error.contains("disabled"));
    }

    #[test]
    fn resolve_targets_rejects_disabled_smbus_hal_driver() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "asus".to_owned(),
            hypercolor_types::config::DriverConfigEntry::disabled(
                std::collections::BTreeMap::default(),
            ),
        );
        let requested = vec!["smbus".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("smbus must fail when all SMBus HAL modules are disabled");
        assert!(error.contains("no enabled SMBus HAL driver modules"));
    }

    #[test]
    fn discovery_target_transient_miss_policy_is_target_owned() {
        assert!(DiscoveryTarget::smbus().preserves_renderable_on_discovery_miss());
        assert!(!DiscoveryTarget::usb().preserves_renderable_on_discovery_miss());
        assert!(!DiscoveryTarget::blocks().preserves_renderable_on_discovery_miss());
        assert!(!DiscoveryTarget::driver("wled").preserves_renderable_on_discovery_miss());
    }

    #[test]
    fn resolve_targets_keeps_wled_when_mdns_is_disabled() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.discovery.mdns_enabled = false;

        let resolved = resolve_targets(None, &cfg, state.driver_registry.as_ref())
            .expect("wled should still resolve");
        assert_eq!(resolved, expected_default_targets(&state));
    }

    #[test]
    fn output_backend_id_for_device_uses_device_origin() {
        let info =
            device_info_with_origin(DeviceOrigin::native("ableton", "usb", ConnectionType::Usb));

        assert_eq!(info.output_backend_id(), "usb");
    }
}
