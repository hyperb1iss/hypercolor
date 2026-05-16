//! Govee network driver for Hypercolor.
//!
//! Targets Govee LED strips, panels, and bulbs over two transports: local-area UDP
//! (Govee LAN protocol on port 4003, with optional Razer-streaming for compatible SKUs)
//! and the Govee Developer API v1 for cloud inventory enrichment and fallback. A
//! per-SKU capability database maps model numbers to LED counts, topology, and protocol
//! flags. Pairing stores a Govee account API key validated against the cloud.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_driver_api::support::{activate_if_requested, disconnect_after_unpair};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, ControlApplyTarget, CredentialStore, DeviceAuthState, DeviceAuthSummary,
    DeviceBackend, DiscoveryCapability, DiscoveryConnectBehavior, DiscoveryRequest,
    DiscoveryResult, DriverConfigProvider, DriverConfigView, DriverControlProvider,
    DriverDescriptor, DriverDiscoveredDevice, DriverHost, DriverModule, DriverPresentationProvider,
    DriverRuntimeCacheProvider, DriverTrackedDevice, PairDeviceOutcome, PairDeviceRequest,
    PairDeviceStatus, PairingCapability, PairingDescriptor, PairingFieldDescriptor,
    PairingFlowKind, TrackedDeviceCtx, TransportScanner, ValidatedControlChanges,
};
use hypercolor_types::config::{DriverConfigEntry, GoveeConfig};
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlAvailability, ControlAvailabilityExpr, ControlAvailabilityState, ControlChange,
    ControlFieldDescriptor, ControlGroupDescriptor, ControlGroupKind, ControlOwner,
    ControlPersistence, ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueMap,
    ControlValueType, ControlVisibility,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceInfo, DeviceOrigin, DriverPresentation,
    DriverTransportKind, ZoneInfo,
};
use serde_json::json;
use tracing::warn;

pub mod backend;
pub mod capabilities;
pub mod cloud;
pub mod lan;

use backend::GoveeBackend;
use cloud::{CloudClient, V1Device};
use lan::discovery::{GoveeLanScanner, profile_led_count, topology_for_family};

pub use capabilities::{
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, known_cloud_sku_count,
    known_sku_count, profile_for_sku,
};
pub use lan::discovery::{
    GoveeKnownDevice, GoveeLanDevice, build_device_info, parse_scan_response,
};

const GOVEE_ACCOUNT_CREDENTIAL_KEY: &str = "account";
const FIELD_KNOWN_IPS: &str = "known_ips";
const FIELD_POWER_OFF_ON_DISCONNECT: &str = "power_off_on_disconnect";
const FIELD_LAN_STATE_FPS: &str = "lan_state_fps";
const FIELD_RAZER_FPS: &str = "razer_fps";
const DEVICE_FIELD_IP: &str = "ip";
const DEVICE_FIELD_SKU: &str = "sku";
const DEVICE_FIELD_MAC: &str = "mac";
const DEVICE_FIELD_CLOUD_DEVICE_ID: &str = "cloud_device_id";
const DEVICE_FIELD_CLOUD_CONTROLLABLE: &str = "cloud_controllable";
const DEVICE_FIELD_CLOUD_RETRIEVABLE: &str = "cloud_retrievable";
const DEVICE_FIELD_CLOUD_SUPPORT_CMDS: &str = "cloud_support_cmds";
const DEVICE_FIELD_FIRMWARE_VERSION: &str = "firmware_version";
const DEVICE_FIELD_LED_COUNT: &str = "led_count";
const DEVICE_FIELD_MAX_FPS: &str = "max_fps";
const DEVICE_FIELD_RAZER_STREAMING: &str = "razer_streaming";
const GOVEE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Open the Govee Home app.",
    "Go to Profile, Settings, Apply for API Key.",
    "Paste the API key here to validate it and unlock cloud fallback.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("govee", "Govee", DriverTransportKind::Network, true, true);

#[derive(Clone)]
pub struct GoveeDriverModule {
    config: GoveeConfig,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
}

impl GoveeDriverModule {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            credential_store: None,
            cloud_base_url: None,
        }
    }

    #[must_use]
    pub fn with_credential_store(
        config: GoveeConfig,
        credential_store: Arc<CredentialStore>,
    ) -> Self {
        Self {
            config,
            credential_store: Some(credential_store),
            cloud_base_url: None,
        }
    }

    #[must_use]
    pub fn with_cloud_base_url(config: GoveeConfig, cloud_base_url: impl Into<String>) -> Self {
        Self {
            config,
            credential_store: None,
            cloud_base_url: Some(cloud_base_url.into()),
        }
    }

    fn cloud_client(&self, api_key: impl Into<String>) -> Result<CloudClient> {
        let api_key = api_key.into();
        match &self.cloud_base_url {
            Some(base_url) => CloudClient::with_base_url(api_key, base_url),
            None => CloudClient::new(api_key),
        }
    }

    fn resolved_config(&self, config: DriverConfigView<'_>) -> Result<GoveeConfig> {
        if config.entry.settings.is_empty() {
            return Ok(self.config.clone());
        }
        config.parse_settings()
    }
}

impl DriverModule for GoveeDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let mut backend = GoveeBackend::new(self.resolved_config(config)?);
        if let Some(credential_store) = &self.credential_store {
            backend = backend.with_credential_store(Arc::clone(credential_store));
        }
        if let Some(base_url) = &self.cloud_base_url {
            backend = backend.with_cloud_base_url(base_url.clone());
        }
        for device in load_cached_probe_devices(host)? {
            let (Some(sku), Some(mac)) = (device.sku, device.mac) else {
                continue;
            };
            let profile = profile_for_sku(&sku).unwrap_or_else(|| fallback_profile(&sku));
            backend.remember_device(GoveeLanDevice {
                ip: device.ip,
                sku,
                mac,
                name: profile.name.to_owned(),
                firmware_version: None,
            });
        }

        Ok(Some(Box::new(backend)))
    }

    fn has_output_backend(&self) -> bool {
        true
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
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

impl DriverPresentationProvider for GoveeDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Govee".to_owned(),
            short_label: Some("Govee".to_owned()),
            accent_rgb: Some([80, 250, 123]),
            secondary_rgb: Some([255, 106, 193]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Light),
        }
    }
}

impl DriverConfigProvider for GoveeDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(govee_config_settings(&self.config))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<GoveeConfig>()?;
        validate_govee_config(&config)
    }
}

#[async_trait]
impl DriverControlProvider for GoveeDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(govee_driver_control_surface(
            &self.resolved_config(config)?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(govee_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        validate_govee_driver_changes(target, changes)
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        let ControlApplyTarget::Driver { driver_id, config } = target else {
            bail!("Govee controls do not expose device-scoped apply");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Govee controls cannot apply to driver '{driver_id}'");
        }

        let mut values = govee_control_values(&config.parse_settings::<GoveeConfig>()?);
        let previous_revision = govee_control_revision(&values);
        for change in &changes.changes {
            values.insert(change.field_id.clone(), change.value.clone());
        }
        let revision = govee_control_revision(&values);

        let control_host = host
            .control_host()
            .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
        control_host
            .driver_config_store()
            .save_driver_values(DESCRIPTOR.id, values.clone())
            .await?;

        Ok(govee_apply_response(
            format!("driver:{}", DESCRIPTOR.id),
            previous_revision,
            revision,
            changes,
            values,
        ))
    }
}

#[async_trait]
impl DiscoveryCapability for GoveeDriverModule {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = self.resolved_config(config)?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let cached_devices = load_cached_probe_devices(host)?;
        let known_devices =
            resolve_govee_probe_devices(&config, &tracked_devices, cached_devices.as_slice());
        let mut scanner = GoveeLanScanner::new(known_devices, request.timeout);
        let mut devices: Vec<_> = scanner
            .scan()
            .await?
            .into_iter()
            .map(DriverDiscoveredDevice::from)
            .collect();

        if let Some(api_key) = account_api_key(host).await? {
            match self.cloud_client(api_key)?.list_v1_devices().await {
                Ok(cloud_devices) => merge_cloud_inventory(&mut devices, cloud_devices),
                Err(error) => {
                    warn!(error = %error, "failed to enrich Govee discovery from cloud inventory");
                }
            }
        }

        Ok(DiscoveryResult { devices })
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for GoveeDriverModule {
    async fn snapshot(&self, host: &dyn DriverHost) -> Result<BTreeMap<String, serde_json::Value>> {
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let probe_devices =
            resolve_govee_probe_devices(&GoveeConfig::default(), &tracked_devices, &[]);

        Ok(BTreeMap::from([(
            "probe_devices".to_owned(),
            serde_json::to_value(probe_devices)
                .context("failed to serialize Govee probe devices")?,
        )]))
    }
}

#[async_trait]
impl PairingCapability for GoveeDriverModule {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        _device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        match host
            .credentials()
            .get_json(DESCRIPTOR.id, GOVEE_ACCOUNT_CREDENTIAL_KEY)
            .await
        {
            Ok(Some(_)) => Some(DeviceAuthSummary {
                state: DeviceAuthState::Configured,
                can_pair: false,
                descriptor: None,
                last_error: None,
            }),
            Ok(None) => Some(auth_summary_without_account_key(_device)),
            Err(error) => Some(DeviceAuthSummary {
                state: DeviceAuthState::Error,
                can_pair: true,
                descriptor: Some(govee_pairing_descriptor()),
                last_error: Some(error.to_string()),
            }),
        }
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        if host
            .credentials()
            .get_json(DESCRIPTOR.id, GOVEE_ACCOUNT_CREDENTIAL_KEY)
            .await?
            .is_some()
        {
            let activated =
                activate_if_requested(host, request.activate_after_pair, device.device_id, "govee")
                    .await;
            let message = if activated {
                "Govee API key is already configured and the device was activated."
            } else {
                "Govee API key is already configured."
            };
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::AlreadyPaired,
                message: message.to_owned(),
                auth_state: DeviceAuthState::Configured,
                activated,
            });
        }

        let Some(api_key) = api_key_from_request(request) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Govee pairing requires an API key.".to_owned(),
                auth_state: DeviceAuthState::Open,
                activated: false,
            });
        };

        self.cloud_client(api_key.clone())?
            .list_v1_devices()
            .await?;
        host.credentials()
            .set_json(
                DESCRIPTOR.id,
                GOVEE_ACCOUNT_CREDENTIAL_KEY,
                json!({ "api_key": api_key }),
            )
            .await?;

        let activated =
            activate_if_requested(host, request.activate_after_pair, device.device_id, "govee")
                .await;
        let message = if activated {
            "Govee API key validated, stored, and the device was activated."
        } else {
            "Govee API key validated and stored."
        };

        Ok(PairDeviceOutcome {
            status: PairDeviceStatus::Paired,
            message: message.to_owned(),
            auth_state: DeviceAuthState::Configured,
            activated,
        })
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome> {
        host.credentials()
            .remove(DESCRIPTOR.id, GOVEE_ACCOUNT_CREDENTIAL_KEY)
            .await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "govee").await;

        Ok(ClearPairingOutcome {
            message: "Govee API key removed.".to_owned(),
            auth_state: DeviceAuthState::Open,
            disconnected,
        })
    }
}

#[must_use]
pub fn resolve_govee_probe_devices_from_sources(
    config: &GoveeConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<GoveeKnownDevice> {
    resolve_govee_probe_devices(config, tracked_devices, &[])
}

#[must_use]
pub fn resolve_govee_probe_devices(
    config: &GoveeConfig,
    tracked_devices: &[DriverTrackedDevice],
    cached_devices: &[GoveeKnownDevice],
) -> Vec<GoveeKnownDevice> {
    let mut known_devices: HashMap<IpAddr, GoveeKnownDevice> = config
        .known_ips
        .iter()
        .copied()
        .map(GoveeKnownDevice::from_ip)
        .map(|device| (device.ip, device))
        .collect();

    for cached in cached_devices {
        known_devices
            .entry(cached.ip)
            .and_modify(|existing| merge_known_device(existing, cached))
            .or_insert_with(|| cached.clone());
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

        let known = GoveeKnownDevice {
            ip,
            sku: tracked.metadata.get("sku").cloned(),
            mac: tracked.metadata.get("mac").cloned(),
        };
        known_devices
            .entry(ip)
            .and_modify(|existing| merge_known_device(existing, &known))
            .or_insert(known);
    }

    let mut resolved: Vec<_> = known_devices.into_values().collect();
    resolved.sort_by_key(|device| device.ip);
    resolved
}

fn load_cached_probe_devices(host: &dyn DriverHost) -> Result<Vec<GoveeKnownDevice>> {
    host.discovery_state()
        .load_cached_json("govee", "probe_devices")?
        .map(serde_json::from_value)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(Into::into)
}

fn merge_known_device(existing: &mut GoveeKnownDevice, incoming: &GoveeKnownDevice) {
    if existing.sku.is_none() {
        existing.sku.clone_from(&incoming.sku);
    }
    if existing.mac.is_none() {
        existing.mac.clone_from(&incoming.mac);
    }
}

fn auth_summary_without_account_key(device: &TrackedDeviceCtx<'_>) -> DeviceAuthSummary {
    let cloud_backed = metadata_has(device.metadata, "cloud_device_id");
    let lan_reachable = metadata_has(device.metadata, "ip");
    let cloud_optional = cloud_backed
        || device
            .info
            .model
            .as_deref()
            .and_then(profile_for_sku)
            .is_some_and(|profile| profile.capabilities.contains(GoveeCapabilities::CLOUD));
    let can_pair = cloud_optional;

    DeviceAuthSummary {
        state: if cloud_backed && !lan_reachable {
            DeviceAuthState::Required
        } else {
            DeviceAuthState::Open
        },
        can_pair,
        descriptor: can_pair.then(govee_pairing_descriptor),
        last_error: None,
    }
}

fn metadata_has(metadata: Option<&HashMap<String, String>>, key: &str) -> bool {
    metadata.is_some_and(|values| values.get(key).is_some_and(|value| !value.is_empty()))
}

pub fn merge_cloud_inventory(
    devices: &mut Vec<DriverDiscoveredDevice>,
    cloud_devices: Vec<V1Device>,
) {
    let mut index_by_fingerprint: HashMap<String, usize> = devices
        .iter()
        .enumerate()
        .map(|(index, device)| (device.fingerprint.0.clone(), index))
        .collect();

    for cloud_device in cloud_devices {
        let discovered = build_cloud_discovered_device(cloud_device);
        if let Some(index) = index_by_fingerprint.get(&discovered.fingerprint.0).copied() {
            merge_cloud_metadata(&mut devices[index], discovered.metadata);
        } else {
            index_by_fingerprint.insert(discovered.fingerprint.0.clone(), devices.len());
            devices.push(discovered);
        }
    }
}

#[must_use]
pub fn build_cloud_discovered_device(device: V1Device) -> DriverDiscoveredDevice {
    let profile = profile_for_sku(&device.model).unwrap_or_else(|| fallback_profile(&device.model));
    let mac = normalized_cloud_mac(&device.device);
    let fingerprint = mac.as_ref().map_or_else(
        || DeviceFingerprint(format!("cloud:govee:{}", device.device)),
        |mac| DeviceFingerprint(format!("net:govee:{mac}")),
    );
    let led_count = profile_led_count(&profile);
    let name = if device.device_name.trim().is_empty() {
        profile.name.to_owned()
    } else {
        device.device_name.clone()
    };
    let supports_brightness = profile.capabilities.contains(GoveeCapabilities::BRIGHTNESS)
        || device
            .support_cmds
            .iter()
            .any(|command| command == "brightness");
    let info = DeviceInfo {
        id: fingerprint.stable_device_id(),
        name: name.clone(),
        vendor: "Govee".to_owned(),
        family: DeviceFamily::new_static("govee", "Govee"),
        model: Some(device.model.clone()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("govee", "govee", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count,
            topology: topology_for_family(profile.family),
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: false,
            supports_brightness,
            has_display: false,
            display_resolution: None,
            max_fps: 1,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let mut metadata = HashMap::from([
        ("sku".to_owned(), device.model),
        ("cloud_device_id".to_owned(), device.device),
        (
            "cloud_controllable".to_owned(),
            device.controllable.to_string(),
        ),
        (
            "cloud_retrievable".to_owned(),
            device.retrievable.to_string(),
        ),
    ]);
    if !device.support_cmds.is_empty() {
        metadata.insert(
            "cloud_support_cmds".to_owned(),
            device.support_cmds.join(","),
        );
    }
    if let Some(mac) = mac {
        metadata.insert("mac".to_owned(), mac);
    }

    DriverDiscoveredDevice {
        info,
        fingerprint,
        metadata,
        connect_behavior: DiscoveryConnectBehavior::Deferred,
    }
}

async fn account_api_key(host: &dyn DriverHost) -> Result<Option<String>> {
    Ok(host
        .credentials()
        .get_json(DESCRIPTOR.id, GOVEE_ACCOUNT_CREDENTIAL_KEY)
        .await?
        .and_then(|value| {
            value
                .get("api_key")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .map(ToOwned::to_owned)
        })
        .filter(|value| !value.is_empty()))
}

fn merge_cloud_metadata(
    device: &mut DriverDiscoveredDevice,
    cloud_metadata: HashMap<String, String>,
) {
    for (key, value) in cloud_metadata {
        device.metadata.entry(key).or_insert(value);
    }
}

#[must_use]
pub fn govee_driver_control_surface(config: &GoveeConfig) -> ControlSurfaceDocument {
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
    document.fields = govee_driver_control_fields();
    document.values = govee_control_values(config);
    document.revision = govee_control_revision(&document.values);
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
pub fn govee_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
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
            id: "cloud".to_owned(),
            label: "Cloud".to_owned(),
            description: None,
            kind: ControlGroupKind::Advanced,
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

    push_govee_metadata_field(
        &mut document,
        device,
        DEVICE_FIELD_IP,
        "IP Address",
        "connection",
        ControlValueType::IpAddress,
        ControlValue::IpAddress,
        0,
    );
    push_govee_metadata_field(
        &mut document,
        device,
        DEVICE_FIELD_SKU,
        "SKU",
        "connection",
        string_value_type(),
        ControlValue::String,
        10,
    );
    push_govee_metadata_field(
        &mut document,
        device,
        DEVICE_FIELD_MAC,
        "MAC",
        "connection",
        ControlValueType::MacAddress,
        ControlValue::MacAddress,
        20,
    );
    push_govee_metadata_field(
        &mut document,
        device,
        DEVICE_FIELD_CLOUD_DEVICE_ID,
        "Cloud Device ID",
        "cloud",
        string_value_type(),
        ControlValue::String,
        0,
    );
    push_govee_metadata_bool_field(
        &mut document,
        device,
        DEVICE_FIELD_CLOUD_CONTROLLABLE,
        "Cloud Controllable",
        "cloud",
        10,
    );
    push_govee_metadata_bool_field(
        &mut document,
        device,
        DEVICE_FIELD_CLOUD_RETRIEVABLE,
        "Cloud Retrievable",
        "cloud",
        20,
    );
    push_govee_support_cmds_field(&mut document, device);

    if let Some(version) = &device.info.firmware_version {
        push_govee_readonly_field(
            &mut document,
            DEVICE_FIELD_FIRMWARE_VERSION,
            "Firmware",
            "diagnostics",
            string_value_type(),
            ControlValue::String(version.clone()),
            0,
        );
    }
    push_govee_readonly_field(
        &mut document,
        DEVICE_FIELD_LED_COUNT,
        "LED Count",
        "diagnostics",
        integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.total_led_count())),
        10,
    );
    push_govee_readonly_field(
        &mut document,
        DEVICE_FIELD_MAX_FPS,
        "Max FPS",
        "diagnostics",
        integer_value_type(0, None),
        ControlValue::Integer(i64::from(device.info.capabilities.max_fps)),
        20,
    );
    let sku = device
        .metadata
        .and_then(|metadata| metadata.get(DEVICE_FIELD_SKU))
        .or(device.info.model.as_ref())
        .map(String::as_str);
    if let Some(profile) = sku.and_then(profile_for_sku) {
        push_govee_readonly_field(
            &mut document,
            DEVICE_FIELD_RAZER_STREAMING,
            "Razer Streaming",
            "diagnostics",
            ControlValueType::Bool,
            ControlValue::Bool(
                profile
                    .capabilities
                    .contains(GoveeCapabilities::RAZER_STREAMING),
            ),
            30,
        );
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
    document.revision = govee_control_revision(&document.values);
    document
}

fn validate_govee_driver_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    let ControlApplyTarget::Driver { driver_id, .. } = target else {
        bail!("Govee controls do not expose device-scoped fields");
    };
    if *driver_id != DESCRIPTOR.id {
        bail!("Govee controls cannot validate driver '{driver_id}'");
    }

    let fields = govee_driver_control_fields()
        .into_iter()
        .map(|field| (field.id.clone(), field))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut impacts = Vec::new();

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            bail!("duplicate Govee control field: {}", change.field_id);
        }
        let field = fields
            .get(&change.field_id)
            .ok_or_else(|| anyhow!("unknown Govee control field: {}", change.field_id))?;
        field
            .value_type
            .validate_value(&change.value)
            .with_context(|| format!("invalid Govee control field: {}", change.field_id))?;
        if change.field_id == FIELD_KNOWN_IPS {
            validate_control_ip_list("Govee known IP", &change.value)?;
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

fn validate_govee_config(config: &GoveeConfig) -> Result<()> {
    for ip in &config.known_ips {
        validate_ip(*ip).with_context(|| format!("invalid Govee known IP: {ip}"))?;
    }
    if config.lan_state_fps == 0 {
        bail!("Govee LAN state FPS must be greater than zero");
    }
    if config.razer_fps == 0 {
        bail!("Govee Razer FPS must be greater than zero");
    }
    Ok(())
}

fn govee_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        govee_driver_field(
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
        govee_driver_field(
            FIELD_POWER_OFF_ON_DISCONNECT,
            "Power Off On Disconnect",
            Some("output"),
            ControlValueType::Bool,
            ApplyImpact::BackendRebind,
            10,
        ),
        govee_driver_field(
            FIELD_LAN_STATE_FPS,
            "LAN State FPS",
            Some("output"),
            ControlValueType::Integer {
                min: Some(1),
                max: Some(60),
                step: Some(1),
            },
            ApplyImpact::BackendRebind,
            20,
        ),
        govee_driver_field(
            FIELD_RAZER_FPS,
            "Razer FPS",
            Some("output"),
            ControlValueType::Integer {
                min: Some(1),
                max: Some(60),
                step: Some(1),
            },
            ApplyImpact::BackendRebind,
            30,
        ),
    ]
}

fn govee_driver_field(
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

fn push_govee_metadata_field(
    document: &mut ControlSurfaceDocument,
    device: &TrackedDeviceCtx<'_>,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    value: impl FnOnce(String) -> ControlValue,
    ordering: i32,
) {
    let Some(raw) = device
        .metadata
        .and_then(|metadata| metadata.get(id))
        .filter(|value| !value.is_empty())
        .cloned()
    else {
        return;
    };
    push_govee_readonly_field(
        document,
        id,
        label,
        group_id,
        value_type,
        value(raw),
        ordering,
    );
}

fn push_govee_metadata_bool_field(
    document: &mut ControlSurfaceDocument,
    device: &TrackedDeviceCtx<'_>,
    id: &str,
    label: &str,
    group_id: &str,
    ordering: i32,
) {
    let Some(raw) = device
        .metadata
        .and_then(|metadata| metadata.get(id))
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let value = raw.parse::<bool>().unwrap_or_default();
    push_govee_readonly_field(
        document,
        id,
        label,
        group_id,
        ControlValueType::Bool,
        ControlValue::Bool(value),
        ordering,
    );
}

fn push_govee_support_cmds_field(
    document: &mut ControlSurfaceDocument,
    device: &TrackedDeviceCtx<'_>,
) {
    let Some(raw) = device
        .metadata
        .and_then(|metadata| metadata.get(DEVICE_FIELD_CLOUD_SUPPORT_CMDS))
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let commands = raw
        .split(',')
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| ControlValue::String(command.to_owned()))
        .collect::<Vec<_>>();
    push_govee_readonly_field(
        document,
        DEVICE_FIELD_CLOUD_SUPPORT_CMDS,
        "Cloud Commands",
        "cloud",
        ControlValueType::List {
            item_type: Box::new(string_value_type()),
            min_items: None,
            max_items: Some(64),
        },
        ControlValue::List(commands),
        30,
    );
}

fn push_govee_readonly_field(
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

fn string_value_type() -> ControlValueType {
    ControlValueType::String {
        min_len: None,
        max_len: None,
        pattern: None,
    }
}

fn govee_apply_response(
    surface_id: String,
    previous_revision: u64,
    revision: u64,
    changes: ValidatedControlChanges,
    values: ControlValueMap,
) -> ApplyControlChangesResponse {
    ApplyControlChangesResponse {
        surface_id,
        previous_revision,
        revision,
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

fn govee_config_settings(config: &GoveeConfig) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            FIELD_KNOWN_IPS.to_owned(),
            serde_json::json!(config.known_ips),
        ),
        (
            FIELD_POWER_OFF_ON_DISCONNECT.to_owned(),
            serde_json::json!(config.power_off_on_disconnect),
        ),
        (
            FIELD_LAN_STATE_FPS.to_owned(),
            serde_json::json!(config.lan_state_fps),
        ),
        (
            FIELD_RAZER_FPS.to_owned(),
            serde_json::json!(config.razer_fps),
        ),
    ])
}

fn govee_control_values(config: &GoveeConfig) -> ControlValueMap {
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
            FIELD_POWER_OFF_ON_DISCONNECT.to_owned(),
            ControlValue::Bool(config.power_off_on_disconnect),
        ),
        (
            FIELD_LAN_STATE_FPS.to_owned(),
            ControlValue::Integer(i64::from(config.lan_state_fps)),
        ),
        (
            FIELD_RAZER_FPS.to_owned(),
            ControlValue::Integer(i64::from(config.razer_fps)),
        ),
    ])
}

fn govee_control_revision(values: &ControlValueMap) -> u64 {
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

fn normalized_cloud_mac(device_id: &str) -> Option<String> {
    let normalized = device_id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase();
    (normalized.len() == 12).then_some(normalized)
}

fn api_key_from_request(request: &PairDeviceRequest) -> Option<String> {
    request
        .values
        .get("api_key")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn govee_pairing_descriptor() -> PairingDescriptor {
    PairingDescriptor {
        kind: PairingFlowKind::CredentialsForm,
        title: "Pair Govee Account".to_owned(),
        instructions: GOVEE_PAIRING_INSTRUCTIONS
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
        action_label: "Validate API Key".to_owned(),
        fields: vec![PairingFieldDescriptor {
            key: "api_key".to_owned(),
            label: "Govee API Key".to_owned(),
            secret: true,
            optional: false,
            placeholder: Some("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx".to_owned()),
        }],
    }
}
