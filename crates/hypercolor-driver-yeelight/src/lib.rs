//! Yeelight network driver for Hypercolor (Spec 94).
//!
//! Finds Yeelight lights with the SSDP-style search on UDP port 1982
//! (multicast, or unicast to configured addresses), reads everything it
//! needs from the search reply, and drives each light over its TCP control
//! port. Lights that support music mode connect back to Hypercolor and
//! take one `set_scene` per frame with no command quota; the rest stay
//! inside the documented 60 commands per minute. Every light needs "LAN
//! Control" switched on in the Yeelight app.

pub mod backend;
pub mod config;
pub mod protocol;
pub mod scanner;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveryBatch, DiscoveryCapability,
    DiscoveryObservation, DiscoveryRequest, DriverConfigProvider, DriverConfigView,
    DriverControlProvider, DriverDescriptor, DriverError, DriverHost, DriverModule,
    DriverPresentationProvider, DriverTrackedDevice, OutputBinding, TrackedDeviceCtx,
    ValidatedControlChanges,
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

pub use backend::{StreamMode, YeelightBackend};
pub use config::YeelightConfig;
pub use protocol::{CONTROL_PORT, Capabilities, Command, LightState, SEARCH_PORT};
pub use scanner::{YeelightDeviceProfile, YeelightScanOptions, YeelightScanner, discovered_device};

/// Driver id, fingerprint namespace, and device family slug.
pub const YEELIGHT_DRIVER_ID: &str = "yeelight";

pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    YEELIGHT_DRIVER_ID,
    "Yeelight",
    DriverTransportKind::Network,
    true,
    false,
)
.experimental();

const FIELD_DEVICE_IPS: &str = "device_ips";
const FIELD_MULTICAST_DISCOVERY: &str = "multicast_discovery";
const FIELD_MUSIC_MODE: &str = "music_mode";
const FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_MODEL: &str = "model";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_STATE: &str = "state";

/// Yeelight driver module.
#[derive(Debug, Clone, Default)]
pub struct YeelightDriverModule;

impl YeelightDriverModule {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for YeelightDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("Yeelight backend ID must be valid"),
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

impl DeviceBackendFactory for YeelightDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        let config = config.parse_settings::<YeelightConfig>()?;
        Ok(Arc::new(YeelightBackend::new(config)))
    }
}

impl DriverPresentationProvider for YeelightDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Yeelight".to_owned(),
            short_label: Some("Yeelight".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: Some([241, 250, 140]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for YeelightDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<DiscoveryBatch, DriverError> {
        let config = config.parse_settings::<YeelightConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let scanner = YeelightScanner::new(YeelightScanOptions {
            unicast_targets: resolve_yeelight_probe_targets(&config, &tracked_devices),
            multicast: config.multicast_discovery,
            timeout: request.timeout,
        });
        let outcome = scanner.scan().await.map_err(DriverError::discovery)?;
        let observations = outcome
            .profiles
            .iter()
            .map(|profile| {
                let max_fps = if config.music_mode && profile.capabilities().music {
                    config.max_fps
                } else {
                    1
                };
                DiscoveryObservation::Device(discovered_device(profile, max_fps, config.music_mode))
            })
            .collect();
        Ok(DiscoveryBatch::new(observations, outcome.gaps))
    }
}

impl DriverConfigProvider for YeelightDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(yeelight_config_settings(&YeelightConfig::default()))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<YeelightConfig>()?;
        validate_yeelight_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

fn validate_yeelight_config(config: &YeelightConfig) -> Result<()> {
    for ip in &config.device_ips {
        validate_ip(*ip).map_err(|error| anyhow!("invalid Yeelight device IP {ip}: {error}"))?;
    }
    config.validate_ranges().map_err(|error| anyhow!(error))
}

fn yeelight_config_settings(config: &YeelightConfig) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            FIELD_DEVICE_IPS.to_owned(),
            serde_json::json!(config.device_ips),
        ),
        (
            FIELD_MULTICAST_DISCOVERY.to_owned(),
            serde_json::json!(config.multicast_discovery),
        ),
        (
            FIELD_MUSIC_MODE.to_owned(),
            serde_json::json!(config.music_mode),
        ),
        (FIELD_MAX_FPS.to_owned(), serde_json::json!(config.max_fps)),
    ])
}

/// Unicast search targets from configured IPs and previously tracked
/// devices, deduplicated and sorted.
#[must_use]
pub fn resolve_yeelight_probe_targets(
    config: &YeelightConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<SocketAddr> {
    let mut targets: Vec<SocketAddr> = config
        .device_ips
        .iter()
        .filter(|ip| validate_ip(**ip).is_ok())
        .map(|ip| SocketAddr::new(*ip, SEARCH_PORT))
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
        let port =
            network_port_from_metadata(Some(&tracked.metadata), scanner::metadata_key::SEARCH_PORT)
                .unwrap_or(SEARCH_PORT);
        targets.push(SocketAddr::new(ip, port));
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

#[async_trait]
impl DriverControlProvider for YeelightDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(yeelight_driver_control_surface(
            &config.parse_settings::<YeelightConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(yeelight_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let ControlApplyTarget::Driver { driver_id, .. } = target else {
            bail!("Yeelight controls cannot validate device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Yeelight controls cannot validate driver '{driver_id}'");
        }
        control_apply::validate_control_changes(
            "Yeelight",
            yeelight_driver_control_fields(),
            changes,
            |change| {
                if change.field_id == FIELD_DEVICE_IPS {
                    control_surface::validate_control_ip_list("Yeelight device IP", &change.value)?;
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
            bail!("Yeelight controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Yeelight controls cannot apply to driver '{driver_id}'");
        }
        let values = yeelight_config_values(&config.parse_settings::<YeelightConfig>()?);
        control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes).await
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        bail!("unknown Yeelight control action: {action_id}");
    }
}

/// Driver-level control surface.
#[must_use]
pub fn yeelight_driver_control_surface(config: &YeelightConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = yeelight_driver_control_fields();
    document.values = yeelight_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    document
}

fn yeelight_driver_control_fields() -> Vec<ControlFieldDescriptor> {
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
            FIELD_MULTICAST_DISCOVERY,
            "Multicast Discovery",
            Some("Find lights with the SSDP search on the local network"),
            Some("connection"),
            ControlValueType::Bool,
            ApplyImpact::DiscoveryRescan,
            10,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_MUSIC_MODE,
            "Music Mode",
            Some("Stream without the one-command-per-second quota on lights that support it"),
            Some("output"),
            ControlValueType::Bool,
            ApplyImpact::BackendRebind,
            20,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_MAX_FPS,
            "Frame Rate",
            Some("Music-mode color updates per second sent to each light"),
            Some("output"),
            control_surface::integer_value_type(1, Some(i64::from(config::MAX_FPS_LIMIT))),
            ApplyImpact::BackendRebind,
            30,
        ),
    ]
}

fn yeelight_config_values(config: &YeelightConfig) -> ControlValueMap {
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
            FIELD_MULTICAST_DISCOVERY.to_owned(),
            ControlValue::Bool(config.multicast_discovery),
        ),
        (
            FIELD_MUSIC_MODE.to_owned(),
            ControlValue::Bool(config.music_mode),
        ),
        (
            FIELD_MAX_FPS.to_owned(),
            ControlValue::Int(i64::from(config.max_fps)),
        ),
    ])
}

/// Device-level control surface: diagnostics.
#[must_use]
pub fn yeelight_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
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
            (scanner::metadata_key::ID, "Yeelight ID", "diagnostics", 10),
            (
                scanner::metadata_key::CLASS,
                "Light Class",
                "diagnostics",
                20,
            ),
            (
                scanner::metadata_key::MUSIC_MODE,
                "Music Mode",
                "diagnostics",
                30,
            ),
            ("stream_mode", "Stream Mode", "diagnostics", 40),
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
    document.revision = yeelight_device_control_revision(device, &document.values);
    document
}

fn yeelight_device_control_revision(
    device: &TrackedDeviceCtx<'_>,
    values: &ControlValueMap,
) -> u64 {
    let mut payload = Vec::new();
    payload.extend_from_slice(device.device_id.to_string().as_bytes());
    payload.extend_from_slice(device.info.name.as_bytes());
    payload.extend_from_slice(&device.info.total_led_count().to_le_bytes());
    payload.extend_from_slice(&device.info.capabilities.max_fps.to_le_bytes());
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    control_surface::extend_value_map_revision(&mut payload, values);
    control_surface::revision_hash(&payload)
}
