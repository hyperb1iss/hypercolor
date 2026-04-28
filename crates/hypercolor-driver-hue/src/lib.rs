pub mod backend;
mod bridge;
mod color;
mod scanner;
mod streaming;
mod types;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::support::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_port_from_metadata,
    push_lookup_key,
};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, ControlApplyTarget, DeviceAuthState, DeviceAuthSummary,
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigProvider, DriverConfigView,
    DriverControlProvider, DriverCredentialStore, DriverDescriptor, DriverDiscoveredDevice,
    DriverHost, DriverModule, DriverPresentationProvider, DriverTrackedDevice, DriverTransport,
    PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus, PairingCapability, PairingDescriptor,
    PairingFlowKind, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_api::{DeviceBackend, TransportScanner};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlActionResult, ControlAvailability, ControlAvailabilityExpr, ControlAvailabilityState,
    ControlChange, ControlFieldDescriptor, ControlGroupDescriptor, ControlGroupKind, ControlOwner,
    ControlPersistence, ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueMap,
    ControlValueType, ControlVisibility,
};
use hypercolor_types::device::{DeviceClassHint, DriverPresentation};

pub use backend::{HueBackend, HueConfig};
pub use bridge::{DEFAULT_HUE_API_PORT, DEFAULT_HUE_STREAM_PORT, HueBridgeClient, HueNupnpBridge};
pub use color::{CieXyb, ColorGamut, GAMUT_A, GAMUT_B, GAMUT_C, rgb_to_cie_xyb};
pub use scanner::{HueKnownBridge, HueScanner};
pub use streaming::{HueStreamSession, encode_packet_into};
pub use types::{
    HueBridgeIdentity, HueChannel, HueChannelMember, HueDiscoveredBridge, HueEntertainmentConfig,
    HueEntertainmentType, HueLight, HuePairResult, HuePosition, build_device_info,
    choose_entertainment_config,
};

const HUE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Press the link button on the Hue Bridge.",
    "Return here within 30 seconds.",
    "Click Pair Bridge.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, true, true);

const FIELD_BRIDGE_IPS: &str = "bridge_ips";
const FIELD_USE_CIE_XY: &str = "use_cie_xy";
const DEVICE_FIELD_IP: &str = "ip";
const DEVICE_FIELD_API_PORT: &str = "api_port";
const DEVICE_FIELD_BRIDGE_ID: &str = "bridge_id";
const DEVICE_FIELD_BRIDGE_NAME: &str = "bridge_name";
const DEVICE_FIELD_ENTERTAINMENT_CONFIG_ID: &str = "entertainment_config_id";
const DEVICE_FIELD_ENTERTAINMENT_CONFIG_NAME: &str = "entertainment_config_name";
const DEVICE_FIELD_MODEL: &str = "model";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_STATE: &str = "state";

#[derive(Clone)]
pub struct HueDriverModule {
    credential_store: Arc<CredentialStore>,
    mdns_enabled: bool,
}

impl HueDriverModule {
    #[must_use]
    pub fn new(credential_store: Arc<CredentialStore>, mdns_enabled: bool) -> Self {
        Self {
            credential_store,
            mdns_enabled,
        }
    }
}

impl DriverModule for HueDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_output_backend(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(HueBackend::with_mdns_enabled(
            config.parse_settings::<HueConfig>()?,
            Arc::clone(&self.credential_store),
            self.mdns_enabled,
        ))))
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
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

impl DriverPresentationProvider for HueDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Philips Hue".to_owned(),
            short_label: Some("Hue".to_owned()),
            accent_rgb: Some([241, 250, 140]),
            secondary_rgb: Some([225, 53, 255]),
            icon: Some("bridge".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for HueDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = config.parse_settings::<HueConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let known_bridges = resolve_hue_probe_bridges_from_sources(&config, &tracked_devices);
        let mut scanner = HueScanner::with_options(
            known_bridges,
            Arc::clone(&self.credential_store),
            request.timeout,
            request.mdns_enabled,
            config.entertainment_config.clone(),
        );
        let devices = scanner
            .scan()
            .await?
            .into_iter()
            .map(DriverDiscoveredDevice::from)
            .collect();

        Ok(DiscoveryResult { devices })
    }
}

impl DriverConfigProvider for HueDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(BTreeMap::from([
            (FIELD_BRIDGE_IPS.to_owned(), serde_json::json!([])),
            (FIELD_USE_CIE_XY.to_owned(), serde_json::json!(true)),
        ]))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<HueConfig>()?;
        for ip in config.bridge_ips {
            validate_ip(ip).with_context(|| format!("invalid Hue bridge IP: {ip}"))?;
        }
        Ok(())
    }
}

#[async_trait]
impl DriverControlProvider for HueDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(hue_driver_control_surface(
            &config.parse_settings::<HueConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(hue_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        validate_hue_driver_changes(target, changes)
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        let ControlApplyTarget::Driver { driver_id, config } = target else {
            bail!("Hue controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Hue controls cannot apply to driver '{driver_id}'");
        }

        let control_host = host
            .control_host()
            .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
        let mut values = hue_config_values(&config.parse_settings::<HueConfig>()?);
        for change in &changes.changes {
            values.insert(change.field_id.clone(), change.value.clone());
        }
        control_host
            .driver_config_store()
            .save_driver_values(DESCRIPTOR.id, values.clone())
            .await?;

        Ok(hue_apply_response(
            format!("driver:{}", DESCRIPTOR.id),
            changes,
            values,
        ))
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        bail!("unknown Hue control action: {action_id}")
    }
}

#[must_use]
pub fn hue_driver_control_surface(config: &HueConfig) -> ControlSurfaceDocument {
    let mut document = ControlSurfaceDocument::empty(
        format!("driver:{}", DESCRIPTOR.id),
        ControlSurfaceScope::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
    );
    document.groups.push(ControlGroupDescriptor {
        id: "connection".to_owned(),
        label: "Connection".to_owned(),
        description: None,
        kind: ControlGroupKind::Connection,
        ordering: 0,
    });
    document.groups.push(ControlGroupDescriptor {
        id: "output".to_owned(),
        label: "Output".to_owned(),
        description: None,
        kind: ControlGroupKind::Output,
        ordering: 10,
    });
    document.fields = hue_driver_control_fields();
    document.values = hue_config_values(config);
    document.revision = hue_control_revision(&document.values);
    document
}

#[must_use]
pub fn hue_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
    let mut document = ControlSurfaceDocument::empty(
        format!("driver:{}:device:{}", DESCRIPTOR.id, device.device_id),
        ControlSurfaceScope::Device {
            device_id: device.device_id,
            driver_id: DESCRIPTOR.id.to_owned(),
        },
    );
    document.groups.extend([
        ControlGroupDescriptor {
            id: "connection".to_owned(),
            label: "Connection".to_owned(),
            description: None,
            kind: ControlGroupKind::Connection,
            ordering: 0,
        },
        ControlGroupDescriptor {
            id: "entertainment".to_owned(),
            label: "Entertainment".to_owned(),
            description: None,
            kind: ControlGroupKind::Output,
            ordering: 10,
        },
        ControlGroupDescriptor {
            id: "diagnostics".to_owned(),
            label: "Diagnostics".to_owned(),
            description: None,
            kind: ControlGroupKind::Diagnostics,
            ordering: 20,
        },
    ]);

    if let Some(metadata) = device.metadata {
        push_hue_metadata_field(
            &mut document,
            metadata,
            DEVICE_FIELD_IP,
            "IP Address",
            "connection",
            ControlValueType::IpAddress,
            ControlValue::IpAddress,
            0,
        );
        if let Some(api_port) = metadata
            .get(DEVICE_FIELD_API_PORT)
            .and_then(|raw| raw.parse::<i64>().ok())
        {
            push_hue_readonly_field(
                &mut document,
                DEVICE_FIELD_API_PORT,
                "API Port",
                "connection",
                integer_value_type(0, Some(i64::from(u16::MAX))),
                ControlValue::Integer(api_port),
                10,
            );
        }
        push_hue_metadata_field(
            &mut document,
            metadata,
            DEVICE_FIELD_BRIDGE_ID,
            "Bridge ID",
            "connection",
            string_value_type(255),
            ControlValue::String,
            20,
        );
        push_hue_metadata_field(
            &mut document,
            metadata,
            DEVICE_FIELD_BRIDGE_NAME,
            "Bridge Name",
            "connection",
            string_value_type(255),
            ControlValue::String,
            30,
        );
        push_hue_metadata_field(
            &mut document,
            metadata,
            DEVICE_FIELD_ENTERTAINMENT_CONFIG_ID,
            "Entertainment Config ID",
            "entertainment",
            string_value_type(255),
            ControlValue::String,
            0,
        );
        push_hue_metadata_field(
            &mut document,
            metadata,
            DEVICE_FIELD_ENTERTAINMENT_CONFIG_NAME,
            "Entertainment Config",
            "entertainment",
            string_value_type(255),
            ControlValue::String,
            10,
        );
    }

    if let Some(model) = &device.info.model {
        push_hue_readonly_field(
            &mut document,
            DEVICE_FIELD_MODEL,
            "Model",
            "diagnostics",
            string_value_type(80),
            ControlValue::String(model.clone()),
            0,
        );
    }
    if let Some(firmware_version) = &device.info.firmware_version {
        push_hue_readonly_field(
            &mut document,
            DEVICE_FIELD_FIRMWARE_VERSION,
            "Firmware",
            "diagnostics",
            string_value_type(80),
            ControlValue::String(firmware_version.clone()),
            10,
        );
    }
    push_hue_readonly_field(
        &mut document,
        DEVICE_FIELD_LED_COUNT,
        "LED Count",
        "diagnostics",
        integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.total_led_count())),
        20,
    );
    push_hue_readonly_field(
        &mut document,
        DEVICE_FIELD_MAX_FPS,
        "Max FPS",
        "diagnostics",
        integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.capabilities.max_fps)),
        30,
    );
    push_hue_readonly_field(
        &mut document,
        DEVICE_FIELD_STATE,
        "State",
        "diagnostics",
        string_value_type(32),
        ControlValue::String(device.current_state.to_string()),
        40,
    );

    document.availability = document
        .fields
        .iter()
        .map(|field| {
            (
                field.id.clone(),
                ControlAvailability {
                    state: ControlAvailabilityState::Available,
                    reason: None,
                },
            )
        })
        .collect();
    document.revision = hue_control_revision(&document.values);
    document
}

fn validate_hue_driver_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    let ControlApplyTarget::Driver { driver_id, .. } = target else {
        bail!("Hue controls cannot validate device targets");
    };
    if *driver_id != DESCRIPTOR.id {
        bail!("Hue controls cannot validate driver '{driver_id}'");
    }

    let fields = hue_driver_control_fields()
        .into_iter()
        .map(|field| (field.id.clone(), field))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut impacts = Vec::new();

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            bail!("duplicate Hue control field: {}", change.field_id);
        }
        let field = fields
            .get(&change.field_id)
            .ok_or_else(|| anyhow!("unknown Hue control field: {}", change.field_id))?;
        field
            .value_type
            .validate_value(&change.value)
            .with_context(|| format!("invalid Hue control field: {}", change.field_id))?;
        if change.field_id == FIELD_BRIDGE_IPS {
            validate_control_ip_list("Hue bridge IP", &change.value)?;
        }
        push_unique_impact(&mut impacts, field.apply_impact.clone());
    }

    Ok(ValidatedControlChanges {
        changes: changes.to_vec(),
        impacts,
    })
}

fn validate_control_ip_list(label: &str, value: &ControlValue) -> Result<()> {
    let ControlValue::List(values) = value else {
        return Ok(());
    };
    for value in values {
        if let ControlValue::IpAddress(raw) = value {
            let ip = raw
                .parse::<IpAddr>()
                .with_context(|| format!("invalid {label}: {raw}"))?;
            validate_ip(ip).with_context(|| format!("invalid {label}: {ip}"))?;
        }
    }
    Ok(())
}

fn hue_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        hue_driver_field(
            FIELD_BRIDGE_IPS,
            "Bridge IPs",
            Some("connection"),
            ControlValueType::List {
                item_type: Box::new(ControlValueType::IpAddress),
                min_items: None,
                max_items: Some(32),
            },
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        hue_driver_field(
            FIELD_USE_CIE_XY,
            "CIE xy Streaming",
            Some("output"),
            ControlValueType::Bool,
            ApplyImpact::BackendRebind,
            10,
        ),
    ]
}

fn hue_driver_field(
    id: &str,
    label: &str,
    group_id: Option<&str>,
    value_type: ControlValueType,
    apply_impact: ApplyImpact,
    ordering: i32,
) -> ControlFieldDescriptor {
    ControlFieldDescriptor {
        id: id.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
        group_id: group_id.map(str::to_owned),
        label: label.to_owned(),
        description: None,
        value_type,
        default_value: None,
        access: ControlAccess::ReadWrite,
        persistence: ControlPersistence::DriverConfig,
        apply_impact,
        visibility: ControlVisibility::Standard,
        availability: ControlAvailabilityExpr::Always,
        ordering,
    }
}

fn push_hue_metadata_field(
    document: &mut ControlSurfaceDocument,
    metadata: &HashMap<String, String>,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    value: impl FnOnce(String) -> ControlValue,
    ordering: i32,
) {
    let Some(raw) = metadata.get(id).filter(|value| !value.is_empty()).cloned() else {
        return;
    };
    push_hue_readonly_field(
        document,
        id,
        label,
        group_id,
        value_type,
        value(raw),
        ordering,
    );
}

fn push_hue_readonly_field(
    document: &mut ControlSurfaceDocument,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    value: ControlValue,
    ordering: i32,
) {
    document.fields.push(ControlFieldDescriptor {
        id: id.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
        group_id: Some(group_id.to_owned()),
        label: label.to_owned(),
        description: None,
        value_type,
        default_value: None,
        access: ControlAccess::ReadOnly,
        persistence: ControlPersistence::RuntimeOnly,
        apply_impact: ApplyImpact::None,
        visibility: ControlVisibility::Diagnostics,
        availability: ControlAvailabilityExpr::Always,
        ordering,
    });
    document.values.insert(id.to_owned(), value);
}

const fn integer_value_type(min: i64, max: Option<i64>) -> ControlValueType {
    ControlValueType::Integer {
        min: Some(min),
        max,
        step: Some(1),
    }
}

const fn string_value_type(max_len: u16) -> ControlValueType {
    ControlValueType::String {
        min_len: None,
        max_len: Some(max_len),
        pattern: None,
    }
}

fn hue_config_values(config: &HueConfig) -> ControlValueMap {
    ControlValueMap::from([
        (
            FIELD_BRIDGE_IPS.to_owned(),
            ControlValue::List(
                config
                    .bridge_ips
                    .iter()
                    .map(|ip| ControlValue::IpAddress(ip.to_string()))
                    .collect(),
            ),
        ),
        (
            FIELD_USE_CIE_XY.to_owned(),
            ControlValue::Bool(config.use_cie_xy),
        ),
    ])
}

fn hue_apply_response(
    surface_id: String,
    changes: ValidatedControlChanges,
    values: ControlValueMap,
) -> ApplyControlChangesResponse {
    ApplyControlChangesResponse {
        surface_id,
        previous_revision: 0,
        revision: 0,
        accepted: changes
            .changes
            .into_iter()
            .map(|change| AppliedControlChange {
                field_id: change.field_id,
                value: change.value,
            })
            .collect(),
        rejected: Vec::new(),
        impacts: changes.impacts,
        values,
    }
}

fn hue_control_revision(values: &ControlValueMap) -> u64 {
    values
        .iter()
        .flat_map(|(key, value)| [key.as_bytes(), format!("{value:?}").as_bytes()].concat())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn push_unique_impact(impacts: &mut Vec<ApplyImpact>, impact: ApplyImpact) {
    if !impacts.contains(&impact) {
        impacts.push(impact);
    }
}

#[async_trait]
impl PairingCapability for HueDriverModule {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured = hue_credentials_present(host.credentials(), device.metadata)
            .await
            .unwrap_or_default();

        Some(DeviceAuthSummary {
            state: if last_error.is_some() {
                DeviceAuthState::Error
            } else if configured {
                DeviceAuthState::Configured
            } else {
                DeviceAuthState::Required
            },
            can_pair: true,
            descriptor: Some(hue_pairing_descriptor()),
            last_error,
        })
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        if hue_credentials_present(host.credentials(), device.metadata)
            .await
            .unwrap_or_default()
        {
            let activated = activate_if_requested(
                host,
                request.activate_after_pair,
                device.device_id,
                DESCRIPTOR.id,
            )
            .await;
            let message = if activated {
                "Hue bridge credentials are already configured and the device was activated."
            } else {
                "Hue bridge credentials are already configured."
            };
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::AlreadyPaired,
                message: message.to_owned(),
                auth_state: DeviceAuthState::Configured,
                activated,
            });
        }

        let Some(bridge_ip) = pairing_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Hue bridge is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let bridge_port =
            network_port_from_metadata(device.metadata, "api_port").unwrap_or(DEFAULT_HUE_API_PORT);

        match pair_hue_bridge_at_ip(&self.credential_store, bridge_ip, bridge_port).await? {
            Some(_) => {
                let activated = activate_if_requested(
                    host,
                    request.activate_after_pair,
                    device.device_id,
                    DESCRIPTOR.id,
                )
                .await;
                let message = if activated {
                    "Hue bridge paired and activated."
                } else {
                    "Hue bridge paired. Credentials are stored."
                };
                Ok(PairDeviceOutcome {
                    status: PairDeviceStatus::Paired,
                    message: message.to_owned(),
                    auth_state: DeviceAuthState::Configured,
                    activated,
                })
            }
            None => Ok(PairDeviceOutcome {
                status: PairDeviceStatus::ActionRequired,
                message: "Press the Hue bridge link button, then retry pairing.".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            }),
        }
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome> {
        clear_hue_credentials(host.credentials(), device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, DESCRIPTOR.id).await;

        Ok(ClearPairingOutcome {
            message: "Hue bridge credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

/// Merge Hue bridge probe hints from config and tracked devices.
#[must_use]
pub fn resolve_hue_probe_bridges_from_sources(
    config: &HueConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<HueKnownBridge> {
    let mut known_bridges: HashMap<IpAddr, HueKnownBridge> = config
        .bridge_ips
        .iter()
        .copied()
        .map(HueKnownBridge::from_ip)
        .map(|bridge| (bridge.ip, bridge))
        .collect();

    for tracked in tracked_devices {
        let Some(ip) = tracked
            .metadata
            .get("ip")
            .and_then(|value| value.parse::<IpAddr>().ok())
            .and_then(|addr| validate_ip(addr).ok())
        else {
            continue;
        };

        let api_port = network_port_from_metadata(Some(&tracked.metadata), "api_port")
            .unwrap_or(DEFAULT_HUE_API_PORT);
        let bridge_id = tracked
            .metadata
            .get("bridge_id")
            .cloned()
            .unwrap_or_default();
        let model_id = tracked
            .metadata
            .get("model_id")
            .cloned()
            .or_else(|| tracked.info.model.clone())
            .unwrap_or_default();
        let sw_version = tracked
            .metadata
            .get("sw_version")
            .cloned()
            .or_else(|| tracked.info.firmware_version.clone())
            .unwrap_or_default();

        known_bridges
            .entry(ip)
            .and_modify(|existing| {
                if existing.bridge_id.is_empty() {
                    existing.bridge_id.clone_from(&bridge_id);
                }
                if existing.api_port == 0 {
                    existing.api_port = api_port;
                }
                if existing.name.is_empty() {
                    existing.name.clone_from(&tracked.info.name);
                }
                if existing.model_id.is_empty() {
                    existing.model_id.clone_from(&model_id);
                }
                if existing.sw_version.is_empty() {
                    existing.sw_version.clone_from(&sw_version);
                }
            })
            .or_insert_with(|| HueKnownBridge {
                bridge_id,
                ip,
                api_port,
                name: tracked.info.name.clone(),
                model_id,
                sw_version,
            });
    }

    let mut resolved: Vec<_> = known_bridges.into_values().collect();
    resolved.sort_by_key(|bridge| bridge.ip);
    resolved
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHuePairingResult {
    pub device_key: Option<String>,
    pub name: Option<String>,
}

/// Pair directly against a Hue bridge IP and persist credentials.
///
/// # Errors
///
/// Returns an error if the Hue pairing exchange or credential persistence fails.
pub async fn pair_hue_bridge_at_ip(
    credential_store: &CredentialStore,
    bridge_ip: IpAddr,
    bridge_port: u16,
) -> Result<Option<StoredHuePairingResult>> {
    let client = HueBridgeClient::with_port(bridge_ip, bridge_port);
    let Some(pair_result) = client.pair_with_status("hypercolor").await? else {
        return Ok(None);
    };

    let bridge_identity = client.bridge_identity().await.ok();
    let credentials = serde_json::json!({
        "api_key": pair_result.api_key,
        "client_key": pair_result.client_key,
    });

    if let Some(identity) = bridge_identity.as_ref() {
        credential_store
            .store_driver_json(DESCRIPTOR.id, &identity.bridge_id, credentials.clone())
            .await?;
    }
    credential_store
        .store_driver_json(DESCRIPTOR.id, &format!("ip:{bridge_ip}"), credentials)
        .await?;

    Ok(Some(StoredHuePairingResult {
        device_key: bridge_identity
            .as_ref()
            .map(|identity| identity.bridge_id.clone()),
        name: bridge_identity
            .as_ref()
            .map(|identity| identity.name.clone()),
    }))
}

fn hue_credential_keys(metadata: Option<&HashMap<String, String>>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(bridge_id) = metadata_value(metadata, "bridge_id") {
        push_lookup_key(&mut keys, bridge_id.to_owned());
    }
    if let Some(ip) = metadata_value(metadata, "ip") {
        push_lookup_key(&mut keys, format!("ip:{ip}"));
    }
    keys
}

async fn hue_credentials_present(
    credential_store: &dyn DriverCredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<bool> {
    for key in hue_credential_keys(metadata) {
        let Some(credentials) = credential_store.get_json(DESCRIPTOR.id, &key).await? else {
            continue;
        };
        if credentials
            .get("api_key")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && credentials
                .get("client_key")
                .and_then(serde_json::Value::as_str)
                .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn clear_hue_credentials(
    credential_store: &dyn DriverCredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<()> {
    for key in hue_credential_keys(metadata) {
        credential_store.remove(DESCRIPTOR.id, &key).await?;
    }
    Ok(())
}

fn pairing_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
    metadata_value(metadata, "ip").and_then(|value| value.parse::<IpAddr>().ok())
}

fn hue_pairing_descriptor() -> PairingDescriptor {
    PairingDescriptor {
        kind: PairingFlowKind::PhysicalAction,
        title: "Pair Hue Bridge".to_owned(),
        instructions: HUE_PAIRING_INSTRUCTIONS
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
        action_label: "Pair Bridge".to_owned(),
        fields: Vec::new(),
    }
}
