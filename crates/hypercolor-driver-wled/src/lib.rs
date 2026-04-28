use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::net::IpAddr;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_core::device::wled::{
    WledBackend, WledDeviceInfo, WledKnownTarget, WledProtocol, WledScanner,
};
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ControlApplyTarget, DiscoveryCapability, DiscoveryRequest, DiscoveryResult,
    DriverConfigProvider, DriverConfigView, DriverControlProvider, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverRuntimeCacheProvider, DriverTrackedDevice,
    DriverTransport, NetworkDriverFactory, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlActionResult, ControlAvailability, ControlAvailabilityExpr, ControlAvailabilityState,
    ControlChange, ControlEnumOption, ControlFieldDescriptor, ControlGroupDescriptor,
    ControlGroupKind, ControlOwner, ControlPersistence, ControlSurfaceDocument,
    ControlSurfaceScope, ControlValue, ControlValueMap, ControlValueType, ControlVisibility,
};
use hypercolor_types::device::DeviceId;
use serde::{Deserialize, Serialize};

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("wled", "WLED", DriverTransport::Network, true, false);

const FIELD_KNOWN_IPS: &str = "known_ips";
const FIELD_DEFAULT_PROTOCOL: &str = "default_protocol";
const FIELD_REALTIME_HTTP_ENABLED: &str = "realtime_http_enabled";
const FIELD_DEDUP_THRESHOLD: &str = "dedup_threshold";
const DEVICE_FIELD_PROTOCOL: &str = "protocol";
const DEVICE_FIELD_IP: &str = "ip";
const DEVICE_FIELD_HOSTNAME: &str = "hostname";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_DEDUP_THRESHOLD: &str = "dedup_threshold";
const DEVICE_FIELD_RGBW: &str = "rgbw";

/// Default protocol for WLED realtime streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WledProtocolConfig {
    /// Distributed Display Protocol.
    #[default]
    Ddp,
    /// E1.31 / sACN output.
    E131,
}

/// WLED driver configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledConfig {
    /// IPs that are always probed during WLED discovery.
    #[serde(default)]
    pub known_ips: Vec<IpAddr>,

    /// Default realtime transport for newly connected WLED devices.
    #[serde(default)]
    pub default_protocol: WledProtocolConfig,

    /// Whether startup/shutdown should toggle WLED realtime mode over HTTP.
    #[serde(default = "bool_true")]
    pub realtime_http_enabled: bool,

    /// Fuzzy frame dedup threshold (0 disables deduplication).
    #[serde(default = "default_dedup_threshold")]
    pub dedup_threshold: u8,
}

impl Default for WledConfig {
    fn default() -> Self {
        Self {
            known_ips: Vec::new(),
            default_protocol: WledProtocolConfig::default(),
            realtime_http_enabled: true,
            dedup_threshold: default_dedup_threshold(),
        }
    }
}

const fn bool_true() -> bool {
    true
}

const fn default_dedup_threshold() -> u8 {
    2
}

#[derive(Clone)]
pub struct WledDriverFactory {
    mdns_enabled: bool,
}

impl WledDriverFactory {
    #[must_use]
    pub const fn new(mdns_enabled: bool) -> Self {
        Self { mdns_enabled }
    }
}

impl NetworkDriverFactory for WledDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(build_wled_backend(
            &config.parse_settings::<WledConfig>()?,
            self.mdns_enabled,
            host,
        )?)))
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

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DiscoveryCapability for WledDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = config.parse_settings::<WledConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let cached_probe_ips = load_cached_probe_ips(host)?;
        let cached_targets = load_cached_probe_targets(host)?;
        let known_targets = resolve_wled_probe_targets_from_sources(
            &config,
            &tracked_devices,
            &cached_probe_ips,
            &cached_targets,
        );
        let mut scanner =
            WledScanner::with_known_targets(known_targets, request.mdns_enabled, request.timeout);
        let devices = scanner
            .scan()
            .await?
            .into_iter()
            .map(DriverDiscoveredDevice::from)
            .collect();

        Ok(DiscoveryResult { devices })
    }
}

impl DriverConfigProvider for WledDriverFactory {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(BTreeMap::from([
            (FIELD_KNOWN_IPS.to_owned(), serde_json::json!([])),
            (FIELD_DEFAULT_PROTOCOL.to_owned(), serde_json::json!("ddp")),
            (
                FIELD_REALTIME_HTTP_ENABLED.to_owned(),
                serde_json::json!(true),
            ),
            (
                FIELD_DEDUP_THRESHOLD.to_owned(),
                serde_json::json!(default_dedup_threshold()),
            ),
        ]))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<WledConfig>()?;
        for ip in config.known_ips {
            validate_ip(ip).with_context(|| format!("invalid WLED known IP: {ip}"))?;
        }
        Ok(())
    }
}

#[async_trait]
impl DriverControlProvider for WledDriverFactory {
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        let _ = host;
        Ok(Some(wled_driver_control_surface(
            &config.parse_settings::<WledConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        let (driver_values, device_values) = if let Some(control_host) = host.control_host() {
            let driver_values = control_host
                .driver_config_store()
                .load_driver_values(DESCRIPTOR.id)
                .await
                .unwrap_or_else(|_| wled_config_values(&WledConfig::default()));
            let device_values = control_host
                .device_config_store()
                .load_device_values(device.device_id)
                .await
                .unwrap_or_default();
            (driver_values, device_values)
        } else {
            (
                wled_config_values(&WledConfig::default()),
                ControlValueMap::new(),
            )
        };

        Ok(Some(wled_device_control_surface(
            device,
            &driver_values,
            &device_values,
        )))
    }

    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let _ = host;
        validate_wled_driver_changes(target, changes)
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        match target {
            ControlApplyTarget::Driver { driver_id, config } => {
                if *driver_id != DESCRIPTOR.id {
                    bail!("WLED controls cannot apply to driver '{driver_id}'");
                }

                let control_host = host
                    .control_host()
                    .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
                let mut values = wled_config_values(&config.parse_settings::<WledConfig>()?);
                for change in &changes.changes {
                    values.insert(change.field_id.clone(), change.value.clone());
                }
                control_host
                    .driver_config_store()
                    .save_driver_values(DESCRIPTOR.id, values.clone())
                    .await?;

                Ok(wled_apply_response(
                    format!("driver:{}", DESCRIPTOR.id),
                    changes,
                    values,
                ))
            }
            ControlApplyTarget::Device { device } => {
                if device.info.origin.driver_id != DESCRIPTOR.id {
                    bail!(
                        "WLED controls cannot apply to device owned by '{}'",
                        device.info.origin.driver_id
                    );
                }

                let control_host = host
                    .control_host()
                    .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
                let driver_values = control_host
                    .driver_config_store()
                    .load_driver_values(DESCRIPTOR.id)
                    .await
                    .unwrap_or_else(|_| wled_config_values(&WledConfig::default()));
                let existing_device_values = control_host
                    .device_config_store()
                    .load_device_values(device.device_id)
                    .await?;
                let mut values =
                    wled_effective_device_values(&driver_values, &existing_device_values);
                for change in &changes.changes {
                    values.insert(change.field_id.clone(), change.value.clone());
                }
                control_host
                    .device_config_store()
                    .save_device_values(device.device_id, values.clone())
                    .await?;
                if changes.impacts.contains(&ApplyImpact::DeviceReconnect) {
                    control_host
                        .lifecycle()
                        .reconnect_device(device.device_id, device.info.backend_id())
                        .await?;
                }

                Ok(wled_apply_response(
                    format!("driver:{}:device:{}", DESCRIPTOR.id, device.device_id),
                    changes,
                    values,
                ))
            }
        }
    }

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        let _ = (host, target, input);
        bail!("unknown WLED control action: {action_id}")
    }
}

#[must_use]
pub fn wled_driver_control_surface(config: &WledConfig) -> ControlSurfaceDocument {
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
    document.fields = wled_driver_control_fields();
    document.values = wled_config_values(config);
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
    document
}

#[must_use]
pub fn wled_device_control_surface(
    device: &TrackedDeviceCtx<'_>,
    driver_values: &ControlValueMap,
    device_values: &ControlValueMap,
) -> ControlSurfaceDocument {
    let mut document = ControlSurfaceDocument::empty(
        format!("driver:{}:device:{}", DESCRIPTOR.id, device.device_id),
        ControlSurfaceScope::Device {
            device_id: device.device_id,
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
    document.groups.push(ControlGroupDescriptor {
        id: "diagnostics".to_owned(),
        label: "Diagnostics".to_owned(),
        description: None,
        kind: ControlGroupKind::Diagnostics,
        ordering: 20,
    });
    document.fields.extend(wled_device_control_fields());
    document.values = wled_effective_device_values(driver_values, device_values);

    if let Some(metadata) = device.metadata {
        if let Some(ip) = metadata.get("ip") {
            document.fields.push(wled_device_readonly_field(
                DEVICE_FIELD_IP,
                "IP Address",
                "connection",
                ControlValueType::IpAddress,
                0,
            ));
            document.values.insert(
                DEVICE_FIELD_IP.to_owned(),
                ControlValue::IpAddress(ip.clone()),
            );
        }
        if let Some(hostname) = metadata.get("hostname") {
            document.fields.push(wled_device_readonly_field(
                DEVICE_FIELD_HOSTNAME,
                "Hostname",
                "connection",
                ControlValueType::String {
                    min_len: None,
                    max_len: Some(255),
                    pattern: None,
                },
                10,
            ));
            document.values.insert(
                DEVICE_FIELD_HOSTNAME.to_owned(),
                ControlValue::String(hostname.clone()),
            );
        }
    }

    if let Some(firmware_version) = &device.info.firmware_version {
        document.fields.push(wled_device_readonly_field(
            DEVICE_FIELD_FIRMWARE_VERSION,
            "Firmware",
            "diagnostics",
            ControlValueType::String {
                min_len: None,
                max_len: Some(80),
                pattern: None,
            },
            20,
        ));
        document.values.insert(
            DEVICE_FIELD_FIRMWARE_VERSION.to_owned(),
            ControlValue::String(firmware_version.clone()),
        );
    }

    document.fields.extend([
        wled_device_readonly_field(
            DEVICE_FIELD_LED_COUNT,
            "LED Count",
            "diagnostics",
            ControlValueType::Integer {
                min: Some(0),
                max: None,
                step: Some(1),
            },
            30,
        ),
        wled_device_readonly_field(
            DEVICE_FIELD_MAX_FPS,
            "Max FPS",
            "diagnostics",
            ControlValueType::Integer {
                min: Some(0),
                max: None,
                step: Some(1),
            },
            40,
        ),
    ]);
    document.values.extend([
        (
            DEVICE_FIELD_LED_COUNT.to_owned(),
            ControlValue::Integer(i64::from(device.info.total_led_count())),
        ),
        (
            DEVICE_FIELD_MAX_FPS.to_owned(),
            ControlValue::Integer(i64::from(device.info.capabilities.max_fps)),
        ),
    ]);

    if let Some(rgbw) = device.info.zones.first().map(|zone| {
        matches!(
            zone.color_format,
            hypercolor_types::device::DeviceColorFormat::Rgbw
        )
    }) {
        document.fields.push(wled_device_readonly_field(
            DEVICE_FIELD_RGBW,
            "RGBW",
            "diagnostics",
            ControlValueType::Bool,
            50,
        ));
        document
            .values
            .insert(DEVICE_FIELD_RGBW.to_owned(), ControlValue::Bool(rgbw));
    }

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
    document.revision = wled_device_control_revision(device, &document.values);
    document
}

fn validate_wled_driver_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    let fields = match target {
        ControlApplyTarget::Driver { driver_id, .. } => {
            if *driver_id != DESCRIPTOR.id {
                bail!("WLED controls cannot validate driver '{driver_id}'");
            }
            wled_driver_control_fields()
        }
        ControlApplyTarget::Device { device } => {
            if device.info.origin.driver_id != DESCRIPTOR.id {
                bail!(
                    "WLED controls cannot validate device owned by '{}'",
                    device.info.origin.driver_id
                );
            }
            wled_device_control_fields()
        }
    };

    let fields = fields
        .into_iter()
        .map(|field| (field.id.clone(), field))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut impacts = Vec::new();

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            bail!("duplicate WLED control field: {}", change.field_id);
        }
        let field = fields
            .get(&change.field_id)
            .ok_or_else(|| anyhow!("unknown WLED control field: {}", change.field_id))?;
        field
            .value_type
            .validate_value(&change.value)
            .with_context(|| format!("invalid WLED control field: {}", change.field_id))?;
        if change.field_id == FIELD_KNOWN_IPS {
            validate_control_ip_list("WLED known IP", &change.value)?;
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

fn wled_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        wled_driver_field(
            FIELD_KNOWN_IPS,
            "Known IPs",
            Some("connection"),
            ControlValueType::List {
                item_type: Box::new(ControlValueType::IpAddress),
                min_items: None,
                max_items: Some(64),
            },
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        wled_driver_field(
            FIELD_DEFAULT_PROTOCOL,
            "Default Protocol",
            Some("output"),
            ControlValueType::Enum {
                options: vec![
                    ControlEnumOption::new("ddp", "DDP"),
                    ControlEnumOption::new("e131", "E1.31"),
                ],
            },
            ApplyImpact::BackendRebind,
            10,
        ),
        wled_driver_field(
            FIELD_REALTIME_HTTP_ENABLED,
            "Realtime HTTP",
            Some("output"),
            ControlValueType::Bool,
            ApplyImpact::BackendRebind,
            20,
        ),
        wled_driver_field(
            FIELD_DEDUP_THRESHOLD,
            "Dedup Threshold",
            Some("output"),
            ControlValueType::Integer {
                min: Some(0),
                max: Some(i64::from(u8::MAX)),
                step: Some(1),
            },
            ApplyImpact::Live,
            30,
        ),
    ]
}

fn wled_driver_field(
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

fn wled_device_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        wled_device_config_field(
            DEVICE_FIELD_PROTOCOL,
            "Protocol",
            ControlValueType::Enum {
                options: vec![
                    ControlEnumOption::new("ddp", "DDP"),
                    ControlEnumOption::new("e131", "E1.31"),
                ],
            },
            0,
        ),
        wled_device_config_field(
            DEVICE_FIELD_DEDUP_THRESHOLD,
            "Dedup Threshold",
            ControlValueType::Integer {
                min: Some(0),
                max: Some(i64::from(u8::MAX)),
                step: Some(1),
            },
            10,
        ),
    ]
}

fn wled_device_readonly_field(
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    ordering: i32,
) -> ControlFieldDescriptor {
    ControlFieldDescriptor {
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
    }
}

fn wled_device_config_field(
    id: &str,
    label: &str,
    value_type: ControlValueType,
    ordering: i32,
) -> ControlFieldDescriptor {
    ControlFieldDescriptor {
        id: id.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: DESCRIPTOR.id.to_owned(),
        },
        group_id: Some("output".to_owned()),
        label: label.to_owned(),
        description: None,
        value_type,
        default_value: None,
        access: ControlAccess::ReadWrite,
        persistence: ControlPersistence::DeviceConfig,
        apply_impact: ApplyImpact::DeviceReconnect,
        visibility: ControlVisibility::Standard,
        availability: ControlAvailabilityExpr::Always,
        ordering,
    }
}

fn wled_device_control_revision(device: &TrackedDeviceCtx<'_>, values: &ControlValueMap) -> u64 {
    let mut payload = Vec::new();
    payload.extend_from_slice(device.device_id.to_string().as_bytes());
    payload.extend_from_slice(device.info.name.as_bytes());
    payload.extend_from_slice(&device.info.total_led_count().to_le_bytes());
    payload.extend_from_slice(&device.info.capabilities.max_fps.to_le_bytes());
    if let Some(firmware_version) = &device.info.firmware_version {
        payload.extend_from_slice(firmware_version.as_bytes());
    }
    if let Some(metadata) = device.metadata {
        let mut metadata_entries = metadata.iter().collect::<Vec<_>>();
        metadata_entries.sort_by_key(|(key, _)| key.as_str());
        for (key, value) in metadata_entries {
            payload.extend_from_slice(key.as_bytes());
            payload.extend_from_slice(value.as_bytes());
        }
    }
    for (key, value) in values {
        payload.extend_from_slice(key.as_bytes());
        payload.extend_from_slice(format!("{value:?}").as_bytes());
    }
    payload.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn wled_effective_device_values(
    driver_values: &ControlValueMap,
    device_values: &ControlValueMap,
) -> ControlValueMap {
    let mut values = ControlValueMap::from([
        (
            DEVICE_FIELD_PROTOCOL.to_owned(),
            driver_values
                .get(FIELD_DEFAULT_PROTOCOL)
                .cloned()
                .unwrap_or_else(|| ControlValue::Enum("ddp".to_owned())),
        ),
        (
            DEVICE_FIELD_DEDUP_THRESHOLD.to_owned(),
            driver_values
                .get(FIELD_DEDUP_THRESHOLD)
                .cloned()
                .unwrap_or(ControlValue::Integer(i64::from(default_dedup_threshold()))),
        ),
    ]);
    for (key, value) in device_values {
        values.insert(key.clone(), value.clone());
    }
    values
}

fn wled_apply_response(
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

fn wled_config_values(config: &WledConfig) -> ControlValueMap {
    ControlValueMap::from([
        (
            FIELD_KNOWN_IPS.to_owned(),
            ControlValue::List(
                config
                    .known_ips
                    .iter()
                    .map(|ip| ControlValue::IpAddress(ip.to_string()))
                    .collect(),
            ),
        ),
        (
            FIELD_DEFAULT_PROTOCOL.to_owned(),
            ControlValue::Enum(
                match config.default_protocol {
                    WledProtocolConfig::Ddp => "ddp",
                    WledProtocolConfig::E131 => "e131",
                }
                .to_owned(),
            ),
        ),
        (
            FIELD_REALTIME_HTTP_ENABLED.to_owned(),
            ControlValue::Bool(config.realtime_http_enabled),
        ),
        (
            FIELD_DEDUP_THRESHOLD.to_owned(),
            ControlValue::Integer(i64::from(config.dedup_threshold)),
        ),
    ])
}

fn push_unique_impact(impacts: &mut Vec<ApplyImpact>, impact: ApplyImpact) {
    if !impacts.contains(&impact) {
        impacts.push(impact);
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for WledDriverFactory {
    async fn snapshot(
        &self,
        host: &dyn DriverHost,
    ) -> Result<std::collections::BTreeMap<String, serde_json::Value>> {
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let probe_ips =
            resolve_wled_probe_ips_from_sources(&WledConfig::default(), &tracked_devices, &[], &[]);
        let probe_targets = resolve_wled_probe_targets_from_sources(
            &WledConfig::default(),
            &tracked_devices,
            &[],
            &[],
        );

        Ok(std::collections::BTreeMap::from([
            (
                "probe_ips".to_owned(),
                serde_json::to_value(probe_ips).context("failed to serialize WLED probe IPs")?,
            ),
            (
                "probe_targets".to_owned(),
                serde_json::to_value(probe_targets)
                    .context("failed to serialize WLED probe targets")?,
            ),
        ]))
    }
}

/// Build the runtime WLED backend using config and cached discovery hints.
///
/// # Errors
///
/// Returns an error if cached probe data cannot be parsed.
pub fn build_wled_backend(
    config: &WledConfig,
    mdns_enabled: bool,
    host: &dyn DriverHost,
) -> Result<WledBackend> {
    let mut known_ips: HashSet<_> = config.known_ips.iter().copied().collect();
    known_ips.extend(load_cached_probe_ips(host)?);

    let mut resolved_known_ips: Vec<_> = known_ips.into_iter().collect();
    resolved_known_ips.sort_unstable();

    let mut backend = WledBackend::with_mdns_fallback(resolved_known_ips, mdns_enabled);
    for target in load_cached_probe_targets(host)? {
        let Some((device_id, ip, info)) = cached_wled_backend_seed(&target) else {
            continue;
        };
        backend.remember_device(device_id, ip, info);
    }
    let protocol = match config.default_protocol {
        WledProtocolConfig::Ddp => WledProtocol::Ddp,
        WledProtocolConfig::E131 => WledProtocol::E131,
    };
    backend.set_protocol(protocol);
    backend.set_realtime_http_enabled(config.realtime_http_enabled);
    backend.set_dedup_threshold(config.dedup_threshold);
    Ok(backend)
}

/// Merge WLED probe IPs from config, tracked devices, and cached discovery.
#[must_use]
pub fn resolve_wled_probe_ips_from_sources(
    config: &WledConfig,
    tracked_devices: &[DriverTrackedDevice],
    cached_probe_ips: &[IpAddr],
    cached_targets: &[WledKnownTarget],
) -> Vec<IpAddr> {
    let mut known_ips: HashSet<IpAddr> = config.known_ips.iter().copied().collect();
    known_ips.extend(cached_probe_ips.iter().copied());
    known_ips.extend(cached_targets.iter().map(|target| target.ip));

    for tracked in tracked_devices {
        let Some(ip_raw) = tracked.metadata.get("ip") else {
            continue;
        };
        let Ok(ip) = ip_raw.parse::<IpAddr>() else {
            continue;
        };
        let Ok(ip) = validate_ip(ip) else {
            continue;
        };
        known_ips.insert(ip);
    }

    let mut resolved: Vec<IpAddr> = known_ips.into_iter().collect();
    resolved.sort_unstable();
    resolved
}

/// Merge WLED probe targets from config, tracked devices, and cached discovery.
#[must_use]
pub fn resolve_wled_probe_targets_from_sources(
    config: &WledConfig,
    tracked_devices: &[DriverTrackedDevice],
    cached_probe_ips: &[IpAddr],
    cached_targets: &[WledKnownTarget],
) -> Vec<WledKnownTarget> {
    let mut known_targets: std::collections::HashMap<IpAddr, WledKnownTarget> = config
        .known_ips
        .iter()
        .copied()
        .map(WledKnownTarget::from_ip)
        .map(|target| (target.ip, target))
        .collect();

    for ip in cached_probe_ips {
        known_targets
            .entry(*ip)
            .or_insert_with(|| WledKnownTarget::from_ip(*ip));
    }

    for target in cached_targets {
        known_targets
            .entry(target.ip)
            .and_modify(|existing| existing.merge_from(target))
            .or_insert_with(|| target.clone());
    }

    for tracked in tracked_devices {
        let Some(ip_raw) = tracked.metadata.get("ip") else {
            continue;
        };
        let Ok(ip) = ip_raw.parse::<IpAddr>() else {
            continue;
        };
        let Ok(ip) = validate_ip(ip) else {
            continue;
        };

        let rgbw = tracked.info.zones.first().map(|zone| {
            matches!(
                zone.color_format,
                hypercolor_types::device::DeviceColorFormat::Rgbw
            )
        });
        let target = WledKnownTarget {
            ip,
            hostname: tracked.metadata.get("hostname").cloned(),
            fingerprint: tracked.fingerprint.clone(),
            name: Some(tracked.info.name.clone()),
            led_count: Some(tracked.info.total_led_count()),
            firmware_version: tracked.info.firmware_version.clone(),
            max_fps: Some(tracked.info.capabilities.max_fps),
            rgbw,
        };

        known_targets
            .entry(ip)
            .and_modify(|existing| existing.merge_from(&target))
            .or_insert(target);
    }

    let mut resolved: Vec<WledKnownTarget> = known_targets.into_values().collect();
    resolved.sort_by_key(|target| target.ip);
    resolved
}

fn load_cached_probe_ips(host: &dyn DriverHost) -> Result<Vec<IpAddr>> {
    host.discovery_state()
        .load_cached_json(DESCRIPTOR.id, "probe_ips")?
        .map(serde_json::from_value)
        .transpose()
        .context("failed to parse cached WLED probe IPs")
        .map(Option::unwrap_or_default)
}

fn load_cached_probe_targets(host: &dyn DriverHost) -> Result<Vec<WledKnownTarget>> {
    host.discovery_state()
        .load_cached_json(DESCRIPTOR.id, "probe_targets")?
        .map(serde_json::from_value)
        .transpose()
        .context("failed to parse cached WLED probe targets")
        .map(Option::unwrap_or_default)
}

fn cached_wled_backend_seed(
    target: &WledKnownTarget,
) -> Option<(DeviceId, IpAddr, WledDeviceInfo)> {
    let fingerprint = target.fingerprint.clone()?;
    let name = target.name.clone()?;
    let led_count = target.led_count?;
    let fps = target
        .max_fps
        .map_or(60, |value| u8::try_from(value).unwrap_or(u8::MAX));

    Some((
        fingerprint.stable_device_id(),
        target.ip,
        WledDeviceInfo {
            firmware_version: target
                .firmware_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            build_id: 0,
            mac: fingerprint
                .0
                .strip_prefix("net:")
                .filter(|value| !value.starts_with("wled:"))
                .unwrap_or_default()
                .to_owned(),
            name,
            led_count: u16::try_from(led_count).unwrap_or(u16::MAX),
            rgbw: target.rgbw.unwrap_or(false),
            max_segments: 1,
            fps,
            power_draw_ma: 0,
            max_power_ma: 0,
            free_heap: 0,
            uptime_secs: 0,
            arch: "unknown".to_owned(),
            is_wifi: true,
            effect_count: 0,
            palette_count: 0,
        },
    ))
}
