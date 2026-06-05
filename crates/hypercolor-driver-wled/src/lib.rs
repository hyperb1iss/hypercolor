//! WLED network driver for Hypercolor.
//!
//! Discovers WLED controllers (ESP8266/ESP32) via mDNS and known-IP probing and
//! streams real-time pixel data using DDP (default) or E1.31/sACN. No authentication
//! is required. Both RGB and RGBW formats are supported; per-device protocol selection
//! overrides the driver-level default.

pub mod backend;
mod ddp;
mod e131;
mod scanner;

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_driver_api::control_apply;
use hypercolor_driver_api::control_surface;
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ControlApplyTarget, DiscoveryCapability, DiscoveryRequest, DiscoveryResult,
    DriverConfigProvider, DriverConfigView, DriverControlProvider, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverModule, DriverPresentationProvider,
    DriverRuntimeCacheProvider, DriverTrackedDevice, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_api::{DeviceBackend, TransportScanner};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ApplyImpact, ControlChange, ControlEnumOption,
    ControlFieldDescriptor, ControlGroupKind, ControlSurfaceDocument, ControlValue,
    ControlValueMap, ControlValueType,
};
use hypercolor_types::device::{
    DeviceClassHint, DeviceId, DriverPresentation, DriverTransportKind,
};
use serde::{Deserialize, Serialize};

pub use backend::{
    WledBackend, WledColorFormat, WledDevice, WledDeviceInfo, WledLiveReceiverConfig, WledProtocol,
    WledSegmentInfo,
};
pub use ddp::{DdpPacket, DdpSequence, build_ddp_frame};
pub use e131::{
    E131_PIXELS_PER_UNIVERSE_RGB, E131_PIXELS_PER_UNIVERSE_RGBW, E131Packet, E131SequenceTracker,
    universes_needed,
};
pub use scanner::{WledKnownTarget, WledScanner};

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("wled", "WLED", DriverTransportKind::Network, true, false);

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

const PROTOCOL_DESCRIPTION: &str =
    "Realtime pixel transport. DDP is preferred; use E1.31 only for sACN workflows.";
const DEDUP_THRESHOLD_DESCRIPTION: &str = "UDP frame suppression tolerance. Keep the default unless network load is causing trouble; 0 disables it.";

static WLED_INFO_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
});

fn wled_info_http_client() -> Result<&'static reqwest::Client> {
    WLED_INFO_HTTP_CLIENT
        .as_ref()
        .map_err(|error| anyhow!("Failed to build shared WLED HTTP client: {error}"))
}

async fn fetch_wled_info(ip: IpAddr) -> Result<backend::WledDeviceInfo> {
    let url = format!("http://{ip}/json/info");
    let client = wled_info_http_client()?;

    let resp = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("HTTP request to {url} failed"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse JSON from {url}"))?;

    backend::parse_wled_info(&json)
}

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
pub struct WledDriverModule {
    mdns_enabled: bool,
}

impl WledDriverModule {
    #[must_use]
    pub const fn new(mdns_enabled: bool) -> Self {
        Self { mdns_enabled }
    }
}

impl DriverModule for WledDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_output_backend(
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

    fn has_output_backend(&self) -> bool {
        true
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

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

impl DriverPresentationProvider for WledDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "WLED".to_owned(),
            short_label: Some("WLED".to_owned()),
            accent_rgb: Some([255, 106, 193]),
            secondary_rgb: Some([128, 255, 234]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for WledDriverModule {
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

impl DriverConfigProvider for WledDriverModule {
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
impl DriverControlProvider for WledDriverModule {
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

                let values = wled_config_values(&config.parse_settings::<WledConfig>()?);
                control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes)
                    .await
            }
            ControlApplyTarget::Device { device } => {
                if device.info.driver_id() != DESCRIPTOR.id {
                    bail!(
                        "WLED controls cannot apply to device owned by '{}'",
                        device.info.driver_id()
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
                let values = wled_effective_device_values(&driver_values, &existing_device_values);
                let (values, previous_revision, revision) = control_apply::apply_value_changes(
                    values,
                    &changes.changes,
                    |values| wled_device_control_revision(device, values),
                    |change| {
                        if change.field_id == DEVICE_FIELD_PROTOCOL {
                            wled_protocol_control_value(Some(&change.value))
                        } else {
                            change.value.clone()
                        }
                    },
                );
                control_host
                    .device_config_store()
                    .save_device_values(device.device_id, values.clone())
                    .await?;

                Ok(control_apply::apply_response(
                    format!("driver:{}:device:{}", DESCRIPTOR.id, device.device_id),
                    previous_revision,
                    revision,
                    changes,
                    values,
                ))
            }
        }
    }
}

#[must_use]
pub fn wled_driver_control_surface(config: &WledConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = wled_driver_control_fields();
    document.values = wled_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    control_surface::mark_fields_available(&mut document);
    document
}

#[must_use]
pub fn wled_device_control_surface(
    device: &TrackedDeviceCtx<'_>,
    driver_values: &ControlValueMap,
    device_values: &ControlValueMap,
) -> ControlSurfaceDocument {
    let mut document = control_surface::device_surface(DESCRIPTOR.id, device.device_id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
        control_surface::group(
            "diagnostics",
            "Diagnostics",
            ControlGroupKind::Diagnostics,
            20,
        ),
    ]);
    document.fields.extend(wled_device_control_fields());
    document.values = wled_effective_device_values(driver_values, device_values);

    if let Some(metadata) = device.metadata {
        control_surface::push_metadata_value(
            &mut document,
            DESCRIPTOR.id,
            metadata,
            DEVICE_FIELD_IP,
            "IP Address",
            "connection",
            ControlValueType::IpAddress,
            ControlValue::IpAddress,
            0,
        );
        control_surface::push_metadata_value(
            &mut document,
            DESCRIPTOR.id,
            metadata,
            DEVICE_FIELD_HOSTNAME,
            "Hostname",
            "connection",
            control_surface::string_value_type(Some(255)),
            ControlValue::String,
            10,
        );
    }

    if let Some(firmware_version) = &device.info.firmware_version {
        control_surface::push_readonly_value(
            &mut document,
            DESCRIPTOR.id,
            DEVICE_FIELD_FIRMWARE_VERSION,
            "Firmware",
            "diagnostics",
            control_surface::string_value_type(Some(80)),
            ControlValue::String(firmware_version.clone()),
            20,
        );
    }

    control_surface::push_readonly_value(
        &mut document,
        DESCRIPTOR.id,
        DEVICE_FIELD_LED_COUNT,
        "LED Count",
        "diagnostics",
        control_surface::integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.total_led_count())),
        30,
    );
    control_surface::push_readonly_value(
        &mut document,
        DESCRIPTOR.id,
        DEVICE_FIELD_MAX_FPS,
        "Max FPS",
        "diagnostics",
        control_surface::integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.capabilities.max_fps)),
        40,
    );

    if let Some(rgbw) = device.info.zones.first().map(|zone| {
        matches!(
            zone.color_format,
            hypercolor_types::device::DeviceColorFormat::Rgbw
        )
    }) {
        control_surface::push_readonly_value(
            &mut document,
            DESCRIPTOR.id,
            DEVICE_FIELD_RGBW,
            "RGBW",
            "diagnostics",
            ControlValueType::Bool,
            ControlValue::Bool(rgbw),
            50,
        );
    }

    control_surface::mark_fields_available(&mut document);
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
            if device.info.driver_id() != DESCRIPTOR.id {
                bail!(
                    "WLED controls cannot validate device owned by '{}'",
                    device.info.driver_id()
                );
            }
            wled_device_control_fields()
        }
    };

    control_apply::validate_control_changes("WLED", fields, changes, |change| {
        if change.field_id == FIELD_KNOWN_IPS {
            control_surface::validate_control_ip_list("WLED known IP", &change.value)?;
        }
        Ok(())
    })
}

fn wled_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_KNOWN_IPS,
            "Known IPs",
            None,
            Some("connection"),
            control_surface::ip_list_value_type(64),
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_DEFAULT_PROTOCOL,
            "Default Streaming Protocol",
            Some(PROTOCOL_DESCRIPTION),
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
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_REALTIME_HTTP_ENABLED,
            "Realtime HTTP",
            None,
            Some("output"),
            ControlValueType::Bool,
            ApplyImpact::BackendRebind,
            20,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_DEDUP_THRESHOLD,
            "Frame Dedup Tolerance",
            Some(DEDUP_THRESHOLD_DESCRIPTION),
            Some("output"),
            control_surface::integer_value_type(0, Some(i64::from(u8::MAX))),
            ApplyImpact::Live,
            30,
        ),
    ]
}

fn wled_device_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![control_surface::device_config_field(
        DESCRIPTOR.id,
        DEVICE_FIELD_PROTOCOL,
        "Streaming Protocol",
        Some(PROTOCOL_DESCRIPTION),
        "output",
        ControlValueType::Enum {
            options: vec![
                ControlEnumOption::new("ddp", "DDP"),
                ControlEnumOption::new("e131", "E1.31"),
            ],
        },
        ApplyImpact::DeviceReconnect,
        0,
    )]
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
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    control_surface::extend_value_map_revision(&mut payload, values);
    control_surface::revision_hash(&payload)
}

fn wled_effective_device_values(
    driver_values: &ControlValueMap,
    device_values: &ControlValueMap,
) -> ControlValueMap {
    let mut values = ControlValueMap::from([(
        DEVICE_FIELD_PROTOCOL.to_owned(),
        wled_protocol_control_value(driver_values.get(FIELD_DEFAULT_PROTOCOL)),
    )]);
    for (key, value) in device_values {
        match key.as_str() {
            DEVICE_FIELD_PROTOCOL => {
                values.insert(key.clone(), wled_protocol_control_value(Some(value)));
            }
            DEVICE_FIELD_DEDUP_THRESHOLD => {}
            _ => {
                values.insert(key.clone(), value.clone());
            }
        }
    }
    values
}

fn wled_protocol_control_value(value: Option<&ControlValue>) -> ControlValue {
    match value {
        Some(ControlValue::Enum(protocol) | ControlValue::String(protocol))
            if matches!(protocol.as_str(), "ddp" | "e131") =>
        {
            ControlValue::Enum(protocol.clone())
        }
        _ => ControlValue::Enum("ddp".to_owned()),
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

#[async_trait]
impl DriverRuntimeCacheProvider for WledDriverModule {
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
