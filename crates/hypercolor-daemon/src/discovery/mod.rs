//! Shared device discovery runtime for daemon startup and API-triggered scans.

mod device_helpers;
mod lifecycle;
mod scan;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use tokio::runtime::Handle;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::domain::layout::LayoutContext;
use crate::logical_devices::LogicalDevice;

pub(crate) use device_helpers::{
    apply_persisted_device_settings, desired_connect_behavior, sync_registry_state,
};
pub use hypercolor_types::api::devices::{DiscoveryScanResult, DiscoveryScannerResult};
pub(crate) use lifecycle::execute_lifecycle_actions;
pub(crate) use lifecycle::handle_async_write_failures;
pub use lifecycle::{
    UserEnabledStateResult, activate_pairable_device, apply_user_enabled_state,
    disconnect_tracked_device, release_renderable_devices, release_renderable_network_devices,
    shutdown_renderable_devices,
};
pub use scan::{
    execute_discovery_scan, execute_discovery_scan_if_idle, execute_discovery_scan_or_enqueue,
    schedule_discovery_scan,
};

const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
const MIN_DISCOVERY_TIMEOUT_MS: u64 = 100;
const MAX_DISCOVERY_TIMEOUT_MS: u64 = 60_000;

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

    /// Narrow layout authority used for identity and discovery convergence.
    pub layout: LayoutContext,

    /// Logical device segmentation store.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Attachment template registry used to derive dynamic hardware topology.
    pub attachment_registry: Arc<RwLock<ComponentRegistry>>,

    /// Saved attachment bindings keyed by physical device ID.
    pub attachment_profiles: Arc<RwLock<ComponentProfileStore>>,

    /// Persisted global and per-device output settings.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

    /// Persistent JSON file for startup runtime session state.
    pub runtime_state_path: PathBuf,

    /// Persistent portable identity overlay in the machine-local state tier.
    pub device_aliases_path: PathBuf,

    /// Shared per-device USB protocol configuration store.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Shared encrypted credential store for driver device auth.
    pub credential_store: Arc<CredentialStore>,

    /// Shared "scan in progress" lock flag.
    pub in_progress: Arc<AtomicBool>,

    /// Coalesced background scans that must run after the current owner exits.
    pub pending_scans: Arc<StdMutex<PendingDiscoveryScans>>,

    /// Main daemon runtime handle for detached background work.
    pub task_spawner: Handle,
}

/// Work-conserving queue for background discovery requests.
#[derive(Debug, Default)]
pub struct PendingDiscoveryScans {
    targets: HashSet<DiscoveryTarget>,
    timeout: Duration,
    config: Option<Arc<HypercolorConfig>>,
}

impl PendingDiscoveryScans {
    fn merge(
        &mut self,
        targets: Vec<DiscoveryTarget>,
        timeout: Duration,
        config: Arc<HypercolorConfig>,
    ) {
        self.targets.extend(targets);
        self.timeout = self.timeout.max(timeout);
        self.config = Some(config);
    }

    fn take(&mut self) -> Option<PendingDiscoveryScan> {
        let config = self.config.take()?;
        let mut targets = self.targets.drain().collect::<Vec<_>>();
        targets.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        let timeout = std::mem::take(&mut self.timeout);
        Some(PendingDiscoveryScan {
            targets,
            timeout,
            config,
        })
    }

    fn is_empty(&self) -> bool {
        self.config.is_none()
    }
}

struct PendingDiscoveryScan {
    targets: Vec<DiscoveryTarget>,
    timeout: Duration,
    config: Arc<HypercolorConfig>,
}

/// Opaque discovery target resolved from registered driver modules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscoveryTarget {
    id: String,
    preserves_renderable_on_miss: bool,
}

impl DiscoveryTarget {
    /// Create a driver-backed discovery target.
    #[must_use]
    pub fn driver(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            preserves_renderable_on_miss: id == hypercolor_types::device::SMBUS_OUTPUT_BACKEND_ID,
            id,
        }
    }

    /// Create the USB transport-provider discovery target.
    #[must_use]
    pub fn usb() -> Self {
        Self::driver(hypercolor_types::device::USB_OUTPUT_BACKEND_ID)
    }

    /// Create the SMBus transport-provider discovery target.
    #[must_use]
    pub fn smbus() -> Self {
        Self::driver(hypercolor_types::device::SMBUS_OUTPUT_BACKEND_ID)
    }

    /// Create the Blocks bridge-provider discovery target.
    #[cfg(unix)]
    #[must_use]
    pub fn blocks() -> Self {
        Self::driver(hypercolor_types::device::BLOCKS_OUTPUT_BACKEND_ID)
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

    pub(super) fn matches_device(&self, info: &DeviceInfo) -> bool {
        info.driver_id().eq_ignore_ascii_case(&self.id)
            || info.output_backend_id().eq_ignore_ascii_case(&self.id)
    }

    fn parse(raw: &str, registry: &DriverModuleRegistry) -> Option<Self> {
        registry
            .get(raw)
            .filter(|driver| driver.discovery().is_some())
            .map(|_| Self::driver(raw))
    }

    /// All discovery targets compiled into this daemon binary.
    fn all(registry: &DriverModuleRegistry) -> Vec<Self> {
        registry
            .discovery_drivers()
            .into_iter()
            .map(|driver| Self::driver(driver.descriptor().id))
            .collect()
    }

    /// Transport providers rescanned after the host resumes from sleep.
    #[must_use]
    pub fn session_resume_targets() -> Vec<Self> {
        vec![Self::usb(), Self::smbus()]
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
    let enabled_driver_ids = crate::network::enabled_driver_module_ids(driver_registry, config);
    let finalized = driver_registry
        .finalize_output_bindings(&enabled_driver_ids)
        .map_err(|error| format!("Driver output bindings are invalid: {error}"))?;
    let active_discovery_ids = enabled_driver_ids
        .into_iter()
        .chain(
            finalized
                .providers()
                .iter()
                .map(|provider| provider.driver_id().to_owned()),
        )
        .collect::<HashSet<_>>();

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

        let driver_id = target.as_str();
        if !active_discovery_ids.contains(driver_id) {
            if explicit_request {
                let has_explicit_config = config.drivers.contains_key(driver_id);
                let is_output_provider = driver_registry.get(driver_id).is_some_and(|driver| {
                    matches!(
                        driver.output(),
                        hypercolor_driver_api::OutputBinding::Owned { .. }
                    )
                });
                if is_output_provider && !has_explicit_config {
                    return Err(format!(
                        "Discovery target '{driver_id}' is inactive because no enabled driver selects its output provider"
                    ));
                }
                let config_flag = crate::network::driver_config_flag(driver_id);
                return Err(format!(
                    "Discovery target '{driver_id}' is disabled by config ({config_flag}=false)"
                ));
            }
            continue;
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

/// Resolve the discovery targets needed to rescan one driver module.
///
/// Network drivers usually own discovery directly. HAL catalog drivers share
/// transport-provider discovery, so their rescans map to the provider selected
/// by their output binding.
pub fn rescan_targets_for_driver(
    driver_id: &str,
    config: &HypercolorConfig,
    driver_registry: &DriverModuleRegistry,
) -> Result<Vec<DiscoveryTarget>, String> {
    let normalized = driver_id.trim().to_ascii_lowercase();
    let Some(driver) = driver_registry.get(&normalized) else {
        return Err(format!("Unknown driver module '{driver_id}'"));
    };

    let descriptor = driver.module_descriptor();
    if !crate::network::module_enabled(config, &descriptor) {
        let config_flag = crate::network::driver_config_flag(&normalized);
        return Err(format!(
            "Driver module '{normalized}' is disabled by config ({config_flag}=false)"
        ));
    }

    let target_id = if driver.discovery().is_some() {
        normalized
    } else if let Some(backend_id) = driver.output().backend_id() {
        backend_id.as_str().to_owned()
    } else {
        return Err(format!(
            "Driver module '{normalized}' does not expose discovery or an output provider"
        ));
    };

    resolve_targets(Some(&[target_id]), config, driver_registry)
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryTarget, PendingDiscoveryScans, default_timeout, normalize_timeout_ms,
        rescan_targets_for_driver, resolve_targets,
    };
    use crate::app_state::AppState;
    use hypercolor_driver_api::{
        BackendInfo, DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryCapability,
        DiscoveryRequest, DriverConfigView, DriverDescriptor, DriverError, DriverHost,
        DriverModule, OutputBinding,
    };
    use hypercolor_network::DriverModuleRegistry;
    use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily, DeviceId,
        DeviceInfo, DeviceOrigin, DeviceTopologyHint, DriverModuleDescriptor, DriverTransportKind,
        SegmentInfo,
    };
    use hypercolor_types::identity::BackendId;
    use std::sync::Arc;
    use std::time::Duration;

    fn builtin_registry() -> AppState {
        AppState::new()
    }

    struct TestDriverModule {
        descriptor: &'static DriverDescriptor,
        default_enabled: bool,
    }

    impl TestDriverModule {
        const fn new(descriptor: &'static DriverDescriptor) -> Self {
            Self {
                descriptor,
                default_enabled: true,
            }
        }

        const fn default_disabled(descriptor: &'static DriverDescriptor) -> Self {
            Self {
                descriptor,
                default_enabled: false,
            }
        }
    }

    static ENABLED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "enabled-driver",
        "Enabled Driver",
        DriverTransportKind::Network,
        true,
        true,
    );

    static DEFAULT_DISABLED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "default-disabled",
        "Default Disabled",
        DriverTransportKind::Network,
        true,
        false,
    );

    static SMBUS_MODULE_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "smbus-driver",
        "SMBus Driver",
        DriverTransportKind::Smbus,
        false,
        false,
    );

    static USB_MODULE_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "usb-driver",
        "USB Driver",
        DriverTransportKind::Usb,
        false,
        false,
    );

    static USB_PROVIDER_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "usb",
        "USB Transport Provider",
        DriverTransportKind::Usb,
        true,
        false,
    );

    static SMBUS_PROVIDER_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
        "smbus",
        "SMBus Transport Provider",
        DriverTransportKind::Smbus,
        true,
        false,
    );

    impl DriverModule for TestDriverModule {
        fn descriptor(&self) -> &'static DriverDescriptor {
            self.descriptor
        }

        fn module_descriptor(&self) -> DriverModuleDescriptor {
            let mut descriptor = self.descriptor().module_descriptor();
            descriptor.default_enabled = self.default_enabled;
            descriptor
        }

        fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
            self.descriptor.supports_discovery.then_some(self)
        }

        fn output(&self) -> OutputBinding<'_> {
            match self.descriptor.id {
                "usb-driver" => {
                    OutputBinding::Shared(BackendId::new("usb").expect("valid USB backend ID"))
                }
                "smbus-driver" => {
                    OutputBinding::Shared(BackendId::new("smbus").expect("valid SMBus backend ID"))
                }
                "usb" | "smbus" => OutputBinding::Owned {
                    id: BackendId::new(self.descriptor.id).expect("valid provider backend ID"),
                    factory: self,
                },
                _ => OutputBinding::None,
            }
        }
    }

    impl DeviceBackendFactory for TestDriverModule {
        fn build(
            &self,
            _host: &dyn DriverHost,
            _config: DriverConfigView<'_>,
        ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
            Ok(Arc::new(NoopBackend {
                id: self.descriptor.id,
            }))
        }
    }

    struct NoopBackend {
        id: &'static str,
    }

    #[async_trait::async_trait]
    impl DeviceBackend for NoopBackend {
        fn info(&self) -> BackendInfo {
            BackendInfo {
                id: self.id.to_owned(),
                name: "No-op Test Backend".to_owned(),
                description: "Validates discovery provider selection".to_owned(),
            }
        }

        fn adopt_device(&self, _discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
            Ok(())
        }

        async fn connect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
            Ok(())
        }

        async fn disconnect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
            Ok(())
        }

        async fn write_colors(
            &self,
            _id: &DeviceId,
            _colors: &[[u8; 3]],
        ) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DiscoveryCapability for TestDriverModule {
        async fn discover(
            &self,
            host: &dyn hypercolor_driver_api::DriverHost,
            request: &DiscoveryRequest,
            config: DriverConfigView<'_>,
        ) -> Result<Vec<DiscoveredDevice>, DriverError> {
            let _ = (host, request, config);
            Ok(Vec::new())
        }
    }

    fn expected_default_targets(
        state: &AppState,
        config: &HypercolorConfig,
    ) -> Vec<DiscoveryTarget> {
        let enabled_driver_ids =
            crate::network::enabled_driver_module_ids(state.driver_registry.as_ref(), config);
        let finalized = state
            .driver_registry
            .finalize_output_bindings(&enabled_driver_ids)
            .expect("built-in output bindings should finalize");
        let active_discovery_ids = enabled_driver_ids
            .into_iter()
            .chain(
                finalized
                    .providers()
                    .iter()
                    .map(|provider| provider.driver_id().to_owned()),
            )
            .collect::<std::collections::HashSet<_>>();
        state
            .driver_registry
            .discovery_drivers()
            .into_iter()
            .filter(|driver| active_discovery_ids.contains(driver.descriptor().id))
            .map(|driver| DiscoveryTarget::driver(driver.descriptor().id))
            .collect()
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
            segments: vec![SegmentInfo {
                name: "Main".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
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
    fn pending_scans_coalesce_targets_and_keep_longest_timeout() {
        let mut pending = PendingDiscoveryScans::default();
        let first_config = Arc::new(HypercolorConfig::default());
        let mut latest = HypercolorConfig::default();
        latest.discovery.mdns_enabled = false;
        let latest_config = Arc::new(latest);
        pending.merge(
            vec![DiscoveryTarget::driver("wled")],
            Duration::from_secs(2),
            first_config,
        );
        pending.merge(
            vec![
                DiscoveryTarget::driver("hue"),
                DiscoveryTarget::driver("wled"),
            ],
            Duration::from_secs(5),
            Arc::clone(&latest_config),
        );

        let scan = pending.take().expect("pending scan should exist");

        assert_eq!(
            scan.targets
                .iter()
                .map(DiscoveryTarget::as_str)
                .collect::<Vec<_>>(),
            vec!["hue", "wled"]
        );
        assert_eq!(scan.timeout, Duration::from_secs(5));
        assert!(Arc::ptr_eq(&scan.config, &latest_config));
        assert!(pending.is_empty());
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
        assert_eq!(resolved, expected_default_targets(&state, &cfg));
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
    fn resolve_targets_rejects_disabled_driver_module() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::new(&ENABLED_DESCRIPTOR))
            .expect("driver should register");
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "enabled-driver".to_owned(),
            DriverConfigEntry::disabled(std::collections::BTreeMap::default()),
        );
        let requested = vec!["enabled-driver".to_owned()];

        let error = resolve_targets(Some(&requested), &cfg, &registry)
            .expect_err("disabled driver must fail");

        assert!(error.contains("drivers.enabled-driver.enabled=false"));
    }

    #[test]
    fn resolve_targets_honors_driver_default_enabled_flag() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::default_disabled(
                &DEFAULT_DISABLED_DESCRIPTOR,
            ))
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
            .register(TestDriverModule::default_disabled(
                &DEFAULT_DISABLED_DESCRIPTOR,
            ))
            .expect("driver should register");
        let cfg = HypercolorConfig::default();
        let requested = vec!["default-disabled".to_owned()];

        let error = resolve_targets(Some(&requested), &cfg, &registry)
            .expect_err("default-disabled driver must fail");

        assert!(error.contains("drivers.default-disabled.enabled=false"));
    }

    #[test]
    fn resolve_targets_rejects_disabled_smbus_hal_driver() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::default_disabled(
                &SMBUS_PROVIDER_DESCRIPTOR,
            ))
            .expect("SMBus provider should register");
        registry
            .register(TestDriverModule::new(&SMBUS_MODULE_DESCRIPTOR))
            .expect("driver should register");
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "smbus-driver".to_owned(),
            DriverConfigEntry::disabled(std::collections::BTreeMap::default()),
        );
        let requested = vec!["smbus".to_owned()];
        let error = resolve_targets(Some(&requested), &cfg, &registry)
            .expect_err("smbus must fail when all SMBus HAL modules are disabled");
        assert!(error.contains("no enabled driver selects its output provider"));
    }

    #[test]
    fn resolve_targets_rejects_usb_when_only_smbus_hal_modules_are_enabled() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::default_disabled(&USB_PROVIDER_DESCRIPTOR))
            .expect("USB provider should register");
        registry
            .register(TestDriverModule::default_disabled(
                &SMBUS_PROVIDER_DESCRIPTOR,
            ))
            .expect("SMBus provider should register");
        registry
            .register(TestDriverModule::new(&SMBUS_MODULE_DESCRIPTOR))
            .expect("driver should register");
        let cfg = HypercolorConfig::default();
        let requested = vec!["usb".to_owned()];

        let error = resolve_targets(Some(&requested), &cfg, &registry)
            .expect_err("usb must fail when no USB-family HAL modules are enabled");

        assert!(error.contains("no enabled driver selects its output provider"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_targets_rejects_disabled_blocks_provider() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "blocks".to_owned(),
            DriverConfigEntry::disabled(std::collections::BTreeMap::default()),
        );
        let requested = vec!["blocks".to_owned()];

        let error = resolve_targets(Some(&requested), &cfg, state.driver_registry.as_ref())
            .expect_err("blocks must fail when disabled");

        assert!(error.contains("drivers.blocks.enabled=false"));
    }

    #[test]
    fn discovery_target_transient_miss_policy_is_target_owned() {
        assert!(DiscoveryTarget::smbus().preserves_renderable_on_discovery_miss());
        assert!(!DiscoveryTarget::usb().preserves_renderable_on_discovery_miss());
        #[cfg(unix)]
        assert!(!DiscoveryTarget::blocks().preserves_renderable_on_discovery_miss());
        assert!(
            !DiscoveryTarget::driver("network-driver").preserves_renderable_on_discovery_miss()
        );
    }

    #[test]
    fn resolve_targets_ignores_mdns_when_building_target_list() {
        let state = builtin_registry();
        let mut cfg = HypercolorConfig::default();
        cfg.discovery.mdns_enabled = false;

        let resolved = resolve_targets(None, &cfg, state.driver_registry.as_ref())
            .expect("default targets should still resolve");
        assert_eq!(resolved, expected_default_targets(&state, &cfg));
    }

    #[test]
    fn output_backend_id_for_device_uses_device_origin() {
        let info = device_info_with_origin(DeviceOrigin::native(
            "fixture-driver",
            "usb",
            ConnectionType::Usb,
        ));

        assert_eq!(info.output_backend_id(), "usb");
    }

    #[test]
    fn discovery_targets_match_devices_by_target_kind() {
        let shared_backend_device = device_info_with_origin(DeviceOrigin::native(
            "network-driver",
            "shared-network",
            ConnectionType::Network,
        ));
        assert!(DiscoveryTarget::driver("network-driver").matches_device(&shared_backend_device));
        assert!(
            DiscoveryTarget::driver("shared-network").matches_device(&shared_backend_device),
            "provider discovery targets should scope devices by output route"
        );

        let usb_device = device_info_with_origin(DeviceOrigin::native(
            "fixture-driver",
            "usb",
            ConnectionType::Usb,
        ));
        assert!(DiscoveryTarget::usb().matches_device(&usb_device));
        assert!(
            !DiscoveryTarget::smbus().matches_device(&usb_device),
            "transport provider targets should scope by output route"
        );
    }

    #[test]
    fn rescan_targets_for_network_driver_use_driver_discovery() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::new(&ENABLED_DESCRIPTOR))
            .expect("driver should register");
        let cfg = HypercolorConfig::default();

        let targets = rescan_targets_for_driver("enabled-driver", &cfg, &registry)
            .expect("network discovery driver should resolve");

        assert_eq!(
            targets
                .iter()
                .map(DiscoveryTarget::as_str)
                .collect::<Vec<_>>(),
            vec!["enabled-driver"]
        );
    }

    #[test]
    fn rescan_targets_for_hal_driver_use_output_provider() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::default_disabled(&USB_PROVIDER_DESCRIPTOR))
            .expect("USB provider should register");
        registry
            .register(TestDriverModule::new(&USB_MODULE_DESCRIPTOR))
            .expect("driver should register");
        let cfg = HypercolorConfig::default();

        let targets = rescan_targets_for_driver("usb-driver", &cfg, &registry)
            .expect("HAL driver should resolve through its USB provider");

        assert_eq!(
            targets
                .iter()
                .map(DiscoveryTarget::as_str)
                .collect::<Vec<_>>(),
            vec!["usb"]
        );
    }

    #[test]
    fn rescan_targets_reject_disabled_hal_driver() {
        let mut registry = DriverModuleRegistry::new();
        registry
            .register(TestDriverModule::default_disabled(&USB_PROVIDER_DESCRIPTOR))
            .expect("USB provider should register");
        registry
            .register(TestDriverModule::new(&USB_MODULE_DESCRIPTOR))
            .expect("driver should register");
        let mut cfg = HypercolorConfig::default();
        cfg.drivers.insert(
            "usb-driver".to_owned(),
            DriverConfigEntry::disabled(std::collections::BTreeMap::default()),
        );

        let error = rescan_targets_for_driver("usb-driver", &cfg, &registry)
            .expect_err("disabled driver rescans should fail");

        assert!(error.contains("drivers.usb-driver.enabled=false"));
    }
}
