//! LIFX LAN network driver for Hypercolor (Spec 88).
//!
//! Discovers LIFX lights with a broadcast or unicast GetService, reads each
//! device's product, firmware, label, and zone topology, and streams frames
//! as unacknowledged Set messages: SetColor for single-zone bulbs,
//! SetColorZones or SetExtendedColorZones for strips and beams, and Set64
//! (with CopyFrameBuffer above 64 zones) for Tile chains, Candles, Ceilings,
//! and the other matrix products. Output stays within the per-device
//! message budget by reporting a matching cadence to the host's
//! latest-value output queue.

pub mod backend;
pub mod color;
pub mod config;
pub mod encoder;
pub mod pacer;
pub mod products;
pub mod protocol;
pub mod scanner;
pub mod topology;
pub mod transport;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveryBatch, DiscoveryCapability,
    DiscoveryRequest, DriverConfigProvider, DriverConfigView, DriverContext, DriverControlProvider,
    DriverDescriptor, DriverError, DriverHost, DriverModule, DriverPresentationProvider,
    DriverTrackedDevice, OutputBinding, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::network::{network_port_from_metadata, validate_ip};
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

pub use backend::LifxBackend;
pub use color::{Hsbk, HsbkEncoder};
pub use config::LifxConfig;
pub use protocol::{DEFAULT_PORT, Serial};
pub use scanner::{
    LifxScanOptions, LifxScanner, TrackedRoute, broadcast_targets, discovered_device,
};
pub use topology::{LifxDeviceProfile, LifxTopology};
pub use transport::RequestPolicy;

pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    topology::LIFX_DRIVER_ID,
    "LIFX",
    DriverTransportKind::Network,
    true,
    false,
)
.experimental();

const FIELD_DEVICE_IPS: &str = "device_ips";
const FIELD_BROADCAST_DISCOVERY: &str = "broadcast_discovery";
const FIELD_MAX_MESSAGES_PER_SECOND: &str = "max_messages_per_second";
const FIELD_TRANSITION_MS: &str = "transition_ms";
const FIELD_WHITE_KELVIN: &str = "white_kelvin";
const DEVICE_FIELD_MODEL: &str = "model";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_STATE: &str = "state";
const DEVICE_ACTION_REFRESH_TOPOLOGY: &str = "refresh_topology";

/// LIFX driver module.
#[derive(Debug, Clone, Default)]
pub struct LifxDriverModule;

impl LifxDriverModule {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for LifxDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("LIFX backend ID must be valid"),
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

impl DeviceBackendFactory for LifxDriverModule {
    fn build(
        &self,
        _context: DriverContext,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        let config = config.parse_settings::<LifxConfig>()?;
        Ok(Arc::new(LifxBackend::new(config)))
    }
}

impl DriverPresentationProvider for LifxDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "LIFX".to_owned(),
            short_label: Some("LIFX".to_owned()),
            accent_rgb: Some([225, 53, 255]),
            secondary_rgb: Some([128, 255, 234]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for LifxDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<DiscoveryBatch, DriverError> {
        let config = config.parse_settings::<LifxConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let scanner = LifxScanner::new(LifxScanOptions {
            unicast_targets: resolve_lifx_probe_targets(&config, &tracked_devices),
            broadcast_targets: if config.broadcast_discovery {
                broadcast_targets()
            } else {
                Vec::new()
            },
            tracked: tracked_routes(&tracked_devices),
            timeout: request.timeout,
            ..LifxScanOptions::default()
        });
        let outcome = scanner.scan().await.map_err(DriverError::discovery)?;
        Ok(outcome
            .into_batch(|profile| discovered_device(&profile, config.max_messages_per_second)))
    }
}

impl DriverConfigProvider for LifxDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(lifx_config_settings(&LifxConfig::default()))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<LifxConfig>()?;
        validate_lifx_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

fn validate_lifx_config(config: &LifxConfig) -> Result<()> {
    for ip in &config.device_ips {
        validate_ip(*ip).map_err(|error| anyhow!("invalid LIFX device IP {ip}: {error}"))?;
    }
    config.validate_ranges().map_err(|error| anyhow!(error))
}

fn lifx_config_settings(config: &LifxConfig) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            FIELD_DEVICE_IPS.to_owned(),
            serde_json::json!(config.device_ips),
        ),
        (
            FIELD_BROADCAST_DISCOVERY.to_owned(),
            serde_json::json!(config.broadcast_discovery),
        ),
        (
            FIELD_MAX_MESSAGES_PER_SECOND.to_owned(),
            serde_json::json!(config.max_messages_per_second),
        ),
        (
            FIELD_TRANSITION_MS.to_owned(),
            serde_json::json!(config.transition_ms),
        ),
        (
            FIELD_WHITE_KELVIN.to_owned(),
            serde_json::json!(config.white_kelvin),
        ),
    ])
}

/// Verified routes of the tracked lights, by serial.
#[must_use]
pub fn tracked_routes(tracked_devices: &[DriverTrackedDevice]) -> HashMap<Serial, TrackedRoute> {
    tracked_devices
        .iter()
        .filter_map(|tracked| {
            let fingerprint = tracked.fingerprint.clone()?;
            let (addr, serial) = scanner::route_from_metadata(&tracked.metadata).ok()?;
            Some((serial, TrackedRoute { addr, fingerprint }))
        })
        .collect()
}

/// Unicast discovery targets from configured IPs and previously tracked
/// devices, deduplicated and sorted.
#[must_use]
pub fn resolve_lifx_probe_targets(
    config: &LifxConfig,
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
impl DriverControlProvider for LifxDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(lifx_driver_control_surface(
            &config.parse_settings::<LifxConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(lifx_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let ControlApplyTarget::Driver { driver_id, .. } = target else {
            bail!("LIFX controls cannot validate device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("LIFX controls cannot validate driver '{driver_id}'");
        }
        control_apply::validate_control_changes(
            "LIFX",
            lifx_driver_control_fields(),
            changes,
            |change| {
                if change.field_id == FIELD_DEVICE_IPS {
                    control_surface::validate_control_ip_list("LIFX device IP", &change.value)?;
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
            bail!("LIFX controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("LIFX controls cannot apply to driver '{driver_id}'");
        }
        let values = lifx_config_values(&config.parse_settings::<LifxConfig>()?);
        control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes).await
    }

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        if action_id != DEVICE_ACTION_REFRESH_TOPOLOGY {
            bail!("unknown LIFX control action: {action_id}");
        }
        if !input.is_empty() {
            bail!("LIFX refresh topology does not accept input");
        }
        let ControlApplyTarget::Device { device } = target else {
            bail!("LIFX refresh topology requires a device target");
        };
        if device.info.driver_id() != DESCRIPTOR.id {
            bail!(
                "LIFX refresh topology cannot target device owned by '{}'",
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
        let surface = lifx_device_control_surface(device);
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
pub fn lifx_driver_control_surface(config: &LifxConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = lifx_driver_control_fields();
    document.values = lifx_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    document
}

fn lifx_driver_control_fields() -> Vec<ControlFieldDescriptor> {
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
            FIELD_MAX_MESSAGES_PER_SECOND,
            "Messages Per Second",
            Some("Per-device message budget; LIFX recommends at most 20"),
            Some("output"),
            control_surface::integer_value_type(
                1,
                Some(i64::from(config::MAX_MESSAGES_PER_SECOND_LIMIT)),
            ),
            ApplyImpact::BackendRebind,
            20,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_TRANSITION_MS,
            "Transition (ms)",
            Some("Fade time the light applies to each frame"),
            Some("output"),
            control_surface::integer_value_type(0, Some(i64::from(config::MAX_TRANSITION_MS))),
            ApplyImpact::BackendRebind,
            30,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_WHITE_KELVIN,
            "White Point (K)",
            Some("Kelvin for desaturated colors, clamped to each light's range"),
            Some("output"),
            control_surface::integer_value_type(
                i64::from(color::MIN_CONFIG_KELVIN),
                Some(i64::from(color::MAX_CONFIG_KELVIN)),
            ),
            ApplyImpact::BackendRebind,
            40,
        ),
    ]
}

fn lifx_config_values(config: &LifxConfig) -> ControlValueMap {
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
            FIELD_MAX_MESSAGES_PER_SECOND.to_owned(),
            ControlValue::Int(i64::from(config.max_messages_per_second)),
        ),
        (
            FIELD_TRANSITION_MS.to_owned(),
            ControlValue::Int(i64::from(config.transition_ms)),
        ),
        (
            FIELD_WHITE_KELVIN.to_owned(),
            ControlValue::Int(i64::from(config.white_kelvin)),
        ),
    ])
}

/// Device-level control surface: diagnostics plus the topology refresh.
#[must_use]
pub fn lifx_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
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
            (scanner::metadata_key::PORT, "Port", "connection", 10),
            (scanner::metadata_key::SERIAL, "Serial", "diagnostics", 20),
            (
                scanner::metadata_key::PRODUCT_ID,
                "Product ID",
                "diagnostics",
                30,
            ),
            (
                scanner::metadata_key::DEVICE_CLASS,
                "Device Class",
                "diagnostics",
                40,
            ),
            (scanner::metadata_key::TILES, "Tiles", "diagnostics", 50),
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
    document.actions.push(ControlActionDescriptor {
        id: DEVICE_ACTION_REFRESH_TOPOLOGY.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
        group_id: Some("diagnostics".to_owned()),
        label: "Refresh Topology".to_owned(),
        description: Some("Reconnect and re-read the zones or tile chain".to_owned()),
        input_fields: Vec::new(),
        result_type: Some(ControlValueType::Bool),
        confirmation: Some(ActionConfirmation {
            level: ActionConfirmationLevel::Normal,
            message: "Refresh topology will reconnect this LIFX device.".to_owned(),
        }),
        apply_impact: ApplyImpact::DeviceReconnect,
        availability: ControlAvailabilityExpr::Always,
        ordering: 110,
    });

    control_surface::mark_fields_available(&mut document);
    control_surface::mark_actions_available(&mut document);
    document.revision = lifx_device_control_revision(device, &document.values);
    document
}

fn lifx_device_control_revision(device: &TrackedDeviceCtx<'_>, values: &ControlValueMap) -> u64 {
    let mut payload = Vec::new();
    payload.extend_from_slice(device.device_id.to_string().as_bytes());
    payload.extend_from_slice(device.info.name.as_bytes());
    payload.extend_from_slice(&device.info.total_led_count().to_le_bytes());
    payload.extend_from_slice(&device.info.capabilities.max_fps.to_le_bytes());
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    control_surface::extend_value_map_revision(&mut payload, values);
    control_surface::revision_hash(&payload)
}
