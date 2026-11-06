//! Twinkly network driver for Hypercolor (Spec 92).
//!
//! Finds Twinkly lights with the UDP discovery datagram on port 5555 (or
//! unicast to configured addresses), reads each device's gestalt over the
//! local REST API, logs in with the challenge and verify flow, reads the
//! LED layout for spatial positions, switches the device to real-time mode,
//! and streams per-LED frames over the UDP real-time protocol on port 7777.
//! The real-time protocol version (1, 2, or 3) follows the firmware, and
//! the LED profile (RGB or RGBW) sets the bytes per LED.

pub mod api;
pub mod backend;
pub mod config;
pub mod layout;
pub mod protocol;
pub mod scanner;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveryBatch, DiscoveryCapability,
    DiscoveryRequest, DriverConfigProvider, DriverConfigView, DriverControlProvider,
    DriverDescriptor, DriverError, DriverHost, DriverModule, DriverPresentationProvider,
    DriverTrackedDevice, OutputBinding, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::network::validate_ip;
use hypercolor_driver_support::{control_apply, control_surface};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::{ControlValue, IpText};
use hypercolor_types::controls::{
    ActionConfirmation, ActionConfirmationLevel, ApplyControlChangesResponse, ApplyImpact,
    ControlActionDescriptor, ControlActionResult, ControlActionStatus, ControlAvailabilityExpr,
    ControlChange, ControlFieldDescriptor, ControlGroupKind, ControlOwner, ControlSurfaceDocument,
    ControlValueMap, ControlValueType,
};
use hypercolor_types::device::{DeviceClassHint, DriverPresentation, DriverTransportKind};
use hypercolor_types::identity::BackendId;

pub use api::{TwinklyClient, TwinklyEndpoint};
pub use backend::{ContentionPolicy, TwinklyBackend};
pub use config::TwinklyConfig;
pub use protocol::{DISCOVERY_PORT, HTTP_PORT, LedProfile, RT_PORT, RtProtocol};
pub use scanner::{
    TwinklyDeviceProfile, TwinklyScanOptions, TwinklyScanner, broadcast_targets, discovered_device,
};

/// Driver id, fingerprint namespace, and device family slug.
pub const TWINKLY_DRIVER_ID: &str = "twinkly";

pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    TWINKLY_DRIVER_ID,
    "Twinkly",
    DriverTransportKind::Network,
    true,
    false,
)
.experimental();

const FIELD_DEVICE_IPS: &str = "device_ips";
const FIELD_BROADCAST_DISCOVERY: &str = "broadcast_discovery";
const FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_MODEL: &str = "model";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_STATE: &str = "state";
const DEVICE_ACTION_REFRESH_LAYOUT: &str = "refresh_layout";

/// Twinkly driver module.
#[derive(Debug, Clone, Default)]
pub struct TwinklyDriverModule;

impl TwinklyDriverModule {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for TwinklyDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("Twinkly backend ID must be valid"),
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

impl DeviceBackendFactory for TwinklyDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        let config = config.parse_settings::<TwinklyConfig>()?;
        Ok(Arc::new(TwinklyBackend::new(config)))
    }
}

impl DriverPresentationProvider for TwinklyDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Twinkly".to_owned(),
            short_label: Some("Twinkly".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: Some([225, 53, 255]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for TwinklyDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<DiscoveryBatch, DriverError> {
        let config = config.parse_settings::<TwinklyConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let scanner = TwinklyScanner::new(TwinklyScanOptions::new(
            resolve_twinkly_probe_targets(&config, &tracked_devices),
            if config.broadcast_discovery {
                broadcast_targets()
            } else {
                Vec::new()
            },
            request.timeout,
        ));
        let outcome = scanner.scan().await.map_err(DriverError::discovery)?;
        Ok(outcome.into_batch(|profile| discovered_device(&profile, config.max_fps)))
    }
}

impl DriverConfigProvider for TwinklyDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(twinkly_config_settings(&TwinklyConfig::default()))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<TwinklyConfig>()?;
        validate_twinkly_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

fn validate_twinkly_config(config: &TwinklyConfig) -> Result<()> {
    for ip in &config.device_ips {
        validate_ip(*ip).map_err(|error| anyhow!("invalid Twinkly device IP {ip}: {error}"))?;
    }
    config.validate_ranges().map_err(|error| anyhow!(error))
}

fn twinkly_config_settings(config: &TwinklyConfig) -> BTreeMap<String, serde_json::Value> {
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

/// REST endpoints probed directly: configured IPs on the standard ports
/// plus every previously tracked device at its recorded endpoint,
/// deduplicated and sorted.
#[must_use]
pub fn resolve_twinkly_probe_targets(
    config: &TwinklyConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<TwinklyEndpoint> {
    let mut targets: Vec<TwinklyEndpoint> = config
        .device_ips
        .iter()
        .filter(|ip| validate_ip(**ip).is_ok())
        .map(|ip| TwinklyEndpoint::new(*ip))
        .collect();
    for tracked in tracked_devices {
        let Ok(endpoint) = scanner::endpoint_from_metadata(&tracked.metadata) else {
            continue;
        };
        if validate_ip(endpoint.ip).is_err() {
            continue;
        }
        targets.push(endpoint);
    }
    targets.sort_unstable();
    targets.dedup_by_key(|endpoint| endpoint.http_addr());
    targets
}

#[async_trait]
impl DriverControlProvider for TwinklyDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(twinkly_driver_control_surface(
            &config.parse_settings::<TwinklyConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(twinkly_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let ControlApplyTarget::Driver { driver_id, .. } = target else {
            bail!("Twinkly controls cannot validate device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Twinkly controls cannot validate driver '{driver_id}'");
        }
        control_apply::validate_control_changes(
            "Twinkly",
            twinkly_driver_control_fields(),
            changes,
            |change| {
                if change.field_id == FIELD_DEVICE_IPS {
                    control_surface::validate_control_ip_list("Twinkly device IP", &change.value)?;
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
            bail!("Twinkly controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Twinkly controls cannot apply to driver '{driver_id}'");
        }
        let values = twinkly_config_values(&config.parse_settings::<TwinklyConfig>()?);
        control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes).await
    }

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        if action_id != DEVICE_ACTION_REFRESH_LAYOUT {
            bail!("unknown Twinkly control action: {action_id}");
        }
        if !input.is_empty() {
            bail!("Twinkly refresh layout does not accept input");
        }
        let ControlApplyTarget::Device { device } = target else {
            bail!("Twinkly refresh layout requires a device target");
        };
        if device.info.driver_id() != DESCRIPTOR.id {
            bail!(
                "Twinkly refresh layout cannot target device owned by '{}'",
                device.info.driver_id()
            );
        }
        let control_host = host
            .control_host()
            .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
        let scheduled = control_host
            .lifecycle()
            .reconnect_device(device.device_id, device.info.output_backend_id())
            .await?;
        let surface = twinkly_device_control_surface(device);
        Ok(ControlActionResult {
            surface_id: surface.surface_id,
            action_id: action_id.to_owned(),
            status: ControlActionStatus::Accepted,
            result: Some(ControlValue::Bool(scheduled)),
            revision: surface.revision,
        })
    }
}

/// Driver-level control surface.
#[must_use]
pub fn twinkly_driver_control_surface(config: &TwinklyConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = twinkly_driver_control_fields();
    document.values = twinkly_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    document
}

fn twinkly_driver_control_fields() -> Vec<ControlFieldDescriptor> {
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
            Some("Real-time frames per second sent to each light"),
            Some("output"),
            control_surface::integer_value_type(1, Some(i64::from(config::MAX_FPS_LIMIT))),
            ApplyImpact::BackendRebind,
            20,
        ),
    ]
}

fn twinkly_config_values(config: &TwinklyConfig) -> ControlValueMap {
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

/// Device-level control surface: diagnostics plus the layout refresh.
#[must_use]
pub fn twinkly_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
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
                scanner::metadata_key::PRODUCT_CODE,
                "Product Code",
                "diagnostics",
                20,
            ),
            (
                scanner::metadata_key::HARDWARE_VERSION,
                "Hardware Version",
                "diagnostics",
                30,
            ),
            (
                scanner::metadata_key::LED_PROFILE,
                "LED Profile",
                "diagnostics",
                40,
            ),
            (
                scanner::metadata_key::RT_PROTOCOL,
                "Real-time Protocol",
                "diagnostics",
                50,
            ),
            (
                scanner::metadata_key::LAYOUT_SOURCE,
                "Layout",
                "diagnostics",
                55,
            ),
            (
                scanner::metadata_key::GESTALT_FRAME_RATE,
                "Reported Frame Rate",
                "diagnostics",
                58,
            ),
            (backend::CONTROL_STATE_KEY, "Control", "diagnostics", 59),
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
        "LEDs",
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
    document.actions.push(ControlActionDescriptor {
        id: DEVICE_ACTION_REFRESH_LAYOUT.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
        group_id: Some("diagnostics".to_owned()),
        label: "Refresh Layout".to_owned(),
        description: Some(
            "Reconnect and re-read the LED layout after mapping the lights in the Twinkly app"
                .to_owned(),
        ),
        input_fields: Vec::new(),
        result_type: Some(ControlValueType::Bool),
        confirmation: Some(ActionConfirmation {
            level: ActionConfirmationLevel::Normal,
            message: "Refresh layout will reconnect this Twinkly device.".to_owned(),
        }),
        apply_impact: ApplyImpact::DeviceReconnect,
        availability: ControlAvailabilityExpr::Always,
        ordering: 110,
    });

    control_surface::mark_fields_available(&mut document);
    control_surface::mark_actions_available(&mut document);
    document.revision = twinkly_device_control_revision(device, &document.values);
    document
}

fn twinkly_device_control_revision(device: &TrackedDeviceCtx<'_>, values: &ControlValueMap) -> u64 {
    let mut payload = Vec::new();
    payload.extend_from_slice(device.device_id.to_string().as_bytes());
    payload.extend_from_slice(device.info.name.as_bytes());
    payload.extend_from_slice(&device.info.total_led_count().to_le_bytes());
    payload.extend_from_slice(&device.info.capabilities.max_fps.to_le_bytes());
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    control_surface::extend_value_map_revision(&mut payload, values);
    control_surface::revision_hash(&payload)
}
