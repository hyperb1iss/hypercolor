//! WiZ network driver for Hypercolor (Spec 93).
//!
//! Finds WiZ (Signify) bulbs, strips, and fixtures with the `registration`
//! broadcast on UDP port 38899 (or unicast to configured addresses), reads
//! each device's system and model configuration, and streams one color per
//! frame as a fire-and-forget `setPilot` datagram: hue and saturation in
//! `r`, `g`, `b`, brightness in `dimming`. Homes set to "Only verified
//! controls" need the home signing key, which signs every `setPilot` with
//! HMAC-SHA256.

pub mod backend;
pub mod config;
pub mod protocol;
pub mod scanner;
pub mod transport;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryCapability,
    DiscoveryRequest, DriverConfigProvider, DriverConfigView, DriverControlProvider,
    DriverDescriptor, DriverError, DriverHost, DriverModule, DriverPresentationProvider,
    DriverTrackedDevice, OutputBinding, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::network::{network_port_from_metadata, validate_ip};
use hypercolor_driver_support::{control_apply, control_surface};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::{ControlValue, IpText};
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ApplyImpact, ControlActionResult, ControlChange,
    ControlFieldDescriptor, ControlGroupKind, ControlSurfaceDocument, ControlValueMap,
    ControlValueType,
};
use hypercolor_types::device::{DeviceClassHint, DriverPresentation, DriverTransportKind};
use hypercolor_types::identity::BackendId;

pub use backend::WizBackend;
pub use config::WizConfig;
pub use protocol::{DEFAULT_PORT, ModuleClass, Pilot, SigningKey};
pub use scanner::{
    WizDeviceProfile, WizScanOptions, WizScanner, broadcast_targets, discovered_device,
};
pub use transport::RequestPolicy;

/// Driver id, fingerprint namespace, and device family slug.
pub const WIZ_DRIVER_ID: &str = "wiz";

pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    WIZ_DRIVER_ID,
    "WiZ",
    DriverTransportKind::Network,
    true,
    false,
);

const FIELD_DEVICE_IPS: &str = "device_ips";
const FIELD_BROADCAST_DISCOVERY: &str = "broadcast_discovery";
const FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_MODEL: &str = "model";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_STATE: &str = "state";

/// WiZ driver module.
#[derive(Debug, Clone, Default)]
pub struct WizDriverModule;

impl WizDriverModule {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for WizDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("WiZ backend ID must be valid"),
            factory: self,
        }
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        Some(self)
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        Some(self)
    }
}

impl DeviceBackendFactory for WizDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        let config = config.parse_settings::<WizConfig>()?;
        Ok(Arc::new(WizBackend::new(config)))
    }
}

impl DriverPresentationProvider for WizDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "WiZ".to_owned(),
            short_label: Some("WiZ".to_owned()),
            accent_rgb: Some([241, 250, 140]),
            secondary_rgb: Some([128, 255, 234]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for WizDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        let config = config.parse_settings::<WizConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let scanner = WizScanner::new(WizScanOptions {
            unicast_targets: resolve_wiz_probe_targets(&config, &tracked_devices),
            broadcast_targets: if config.broadcast_discovery {
                broadcast_targets()
            } else {
                Vec::new()
            },
            timeout: request.timeout,
            request_policy: RequestPolicy::default(),
        });
        let profiles = scanner
            .scan_profiles()
            .await
            .map_err(DriverError::discovery)?;
        Ok(profiles
            .iter()
            .map(|profile| discovered_device(profile, config.max_fps))
            .collect())
    }
}

impl DriverConfigProvider for WizDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(wiz_config_settings(&WizConfig::default()))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<WizConfig>()?;
        validate_wiz_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

fn validate_wiz_config(config: &WizConfig) -> Result<()> {
    for ip in &config.device_ips {
        validate_ip(*ip).map_err(|error| anyhow!("invalid WiZ device IP {ip}: {error}"))?;
    }
    config.validate_ranges().map_err(|error| anyhow!(error))
}

fn wiz_config_settings(config: &WizConfig) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            FIELD_DEVICE_IPS.to_owned(),
            serde_json::json!(config.device_ips),
        ),
        (
            FIELD_BROADCAST_DISCOVERY.to_owned(),
            serde_json::json!(config.broadcast_discovery),
        ),
        (FIELD_MAX_FPS.to_owned(), serde_json::json!(config.max_fps)),
    ])
}

/// Unicast probe targets from configured IPs and previously tracked
/// devices, deduplicated and sorted.
#[must_use]
pub fn resolve_wiz_probe_targets(
    config: &WizConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<SocketAddr> {
    let mut targets: Vec<SocketAddr> = config
        .device_ips
        .iter()
        .filter(|ip| validate_ip(**ip).is_ok())
        .map(|ip| SocketAddr::new(*ip, DEFAULT_PORT))
        .collect();
    for tracked in tracked_devices {
        let Some(ip) = tracked
            .metadata
            .get(scanner::metadata_key::IP)
            .and_then(|raw| raw.parse().ok())
        else {
            continue;
        };
        if validate_ip(ip).is_err() {
            continue;
        }
        let port = network_port_from_metadata(Some(&tracked.metadata), scanner::metadata_key::PORT)
            .unwrap_or(DEFAULT_PORT);
        targets.push(SocketAddr::new(ip, port));
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

#[async_trait]
impl DriverControlProvider for WizDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(wiz_driver_control_surface(
            &config.parse_settings::<WizConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(wiz_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let ControlApplyTarget::Driver { driver_id, .. } = target else {
            bail!("WiZ controls cannot validate device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("WiZ controls cannot validate driver '{driver_id}'");
        }
        control_apply::validate_control_changes(
            "WiZ",
            wiz_driver_control_fields(),
            changes,
            |change| {
                if change.field_id == FIELD_DEVICE_IPS {
                    control_surface::validate_control_ip_list("WiZ device IP", &change.value)?;
                }
                Ok(())
            },
        )
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        let ControlApplyTarget::Driver { driver_id, config } = target else {
            bail!("WiZ controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("WiZ controls cannot apply to driver '{driver_id}'");
        }
        let values = wiz_config_values(&config.parse_settings::<WizConfig>()?);
        control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes).await
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        bail!("unknown WiZ control action: {action_id}");
    }
}

/// Driver-level control surface.
#[must_use]
pub fn wiz_driver_control_surface(config: &WizConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = wiz_driver_control_fields();
    document.values = wiz_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    document
}

fn wiz_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_DEVICE_IPS,
            "Device IPs",
            Some("Addresses probed directly, for networks that filter broadcast"),
            Some("connection"),
            control_surface::ip_list_value_type(64),
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_BROADCAST_DISCOVERY,
            "Broadcast Discovery",
            Some("Find lights with a UDP broadcast on the local networks"),
            Some("connection"),
            ControlValueType::Bool,
            ApplyImpact::DiscoveryRescan,
            10,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_MAX_FPS,
            "Frame Rate",
            Some("Color updates per second sent to each light"),
            Some("output"),
            control_surface::integer_value_type(1, Some(i64::from(config::MAX_FPS_LIMIT))),
            ApplyImpact::BackendRebind,
            20,
        ),
    ]
}

fn wiz_config_values(config: &WizConfig) -> ControlValueMap {
    ControlValueMap::from([
        (
            FIELD_DEVICE_IPS.to_owned(),
            ControlValue::List(
                config
                    .device_ips
                    .iter()
                    .map(|ip| ControlValue::Ip(IpText::from(*ip)))
                    .collect(),
            ),
        ),
        (
            FIELD_BROADCAST_DISCOVERY.to_owned(),
            ControlValue::Bool(config.broadcast_discovery),
        ),
        (
            FIELD_MAX_FPS.to_owned(),
            ControlValue::Int(i64::from(config.max_fps)),
        ),
    ])
}

/// Device-level control surface: diagnostics.
#[must_use]
pub fn wiz_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
    let mut document = control_surface::device_surface(DESCRIPTOR.id, device.device_id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group(
            "diagnostics",
            "Diagnostics",
            ControlGroupKind::Diagnostics,
            10,
        ),
    ]);

    if let Some(metadata) = device.metadata {
        control_surface::push_metadata_value(
            &mut document,
            DESCRIPTOR.id,
            metadata,
            scanner::metadata_key::IP,
            "IP Address",
            "connection",
            ControlValueType::IpAddress,
            |raw| IpText::new(raw).ok().map(ControlValue::Ip),
            0,
        );
        let text_fields = [
            (scanner::metadata_key::MAC, "MAC Address", "diagnostics", 10),
            (
                scanner::metadata_key::MODULE_NAME,
                "Module",
                "diagnostics",
                20,
            ),
            (
                scanner::metadata_key::MODULE_CLASS,
                "Light Class",
                "diagnostics",
                30,
            ),
            (
                scanner::metadata_key::MIN_DIMMING,
                "Minimum Dimming",
                "diagnostics",
                40,
            ),
        ];
        for (key, label, group, ordering) in text_fields {
            control_surface::push_metadata_value(
                &mut document,
                DESCRIPTOR.id,
                metadata,
                key,
                label,
                group,
                control_surface::string_value_type(Some(64)),
                |raw| Some(ControlValue::Text(raw)),
                ordering,
            );
        }
    }

    if let Some(model) = &device.info.model {
        control_surface::push_readonly_value(
            &mut document,
            DESCRIPTOR.id,
            DEVICE_FIELD_MODEL,
            "Model",
            "diagnostics",
            control_surface::string_value_type(Some(80)),
            ControlValue::Text(model.clone()),
            60,
        );
    }
    if let Some(firmware_version) = &device.info.firmware_version {
        control_surface::push_readonly_value(
            &mut document,
            DESCRIPTOR.id,
            DEVICE_FIELD_FIRMWARE_VERSION,
            "Firmware",
            "diagnostics",
            control_surface::string_value_type(Some(32)),
            ControlValue::Text(firmware_version.clone()),
            70,
        );
    }
    control_surface::push_readonly_value(
        &mut document,
        DESCRIPTOR.id,
        DEVICE_FIELD_LED_COUNT,
        "Zones",
        "diagnostics",
        control_surface::integer_value_type(0, None),
        ControlValue::Int(i64::from(device.info.total_led_count())),
        80,
    );
    control_surface::push_readonly_value(
        &mut document,
        DESCRIPTOR.id,
        DEVICE_FIELD_MAX_FPS,
        "Max FPS",
        "diagnostics",
        control_surface::integer_value_type(0, None),
        ControlValue::Int(i64::from(device.info.capabilities.max_fps)),
        90,
    );
    control_surface::push_readonly_value(
        &mut document,
        DESCRIPTOR.id,
        DEVICE_FIELD_STATE,
        "State",
        "diagnostics",
        control_surface::string_value_type(Some(32)),
        ControlValue::Text(device.current_state.to_string()),
        100,
    );
    control_surface::mark_fields_available(&mut document);
    control_surface::mark_actions_available(&mut document);
    document.revision = wiz_device_control_revision(device, &document.values);
    document
}

fn wiz_device_control_revision(device: &TrackedDeviceCtx<'_>, values: &ControlValueMap) -> u64 {
    let mut payload = Vec::new();
    payload.extend_from_slice(device.device_id.to_string().as_bytes());
    payload.extend_from_slice(device.info.name.as_bytes());
    payload.extend_from_slice(&device.info.total_led_count().to_le_bytes());
    payload.extend_from_slice(&device.info.capabilities.max_fps.to_le_bytes());
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    control_surface::extend_value_map_revision(&mut payload, values);
    control_surface::revision_hash(&payload)
}
