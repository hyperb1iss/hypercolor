//! Govee network driver.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{DeviceBackend, DiscoveryConnectBehavior, TransportScanner};
use hypercolor_driver_api::support::{activate_if_requested, disconnect_after_unpair};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, DiscoveryCapability, DiscoveryRequest,
    DiscoveryResult, DriverConfigView, DriverDescriptor, DriverDiscoveredDevice, DriverHost,
    DriverRuntimeCacheProvider, DriverTrackedDevice, DriverTransport, NetworkDriverFactory,
    PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus, PairingCapability, PairingDescriptor,
    PairingFieldDescriptor, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, ZoneInfo,
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
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, known_sku_count, profile_for_sku,
};
pub use lan::discovery::{
    GoveeKnownDevice, GoveeLanDevice, build_device_info, parse_scan_response,
};

const GOVEE_ACCOUNT_CREDENTIAL_KEY: &str = "govee:account";
const GOVEE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Open the Govee Home app.",
    "Go to Profile, Settings, Apply for API Key.",
    "Paste the API key here to validate it and unlock cloud fallback.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("govee", "Govee", DriverTransport::Network, true, true);

#[derive(Clone)]
pub struct GoveeDriverFactory {
    config: GoveeConfig,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
}

impl GoveeDriverFactory {
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

impl NetworkDriverFactory for GoveeDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(
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

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DiscoveryCapability for GoveeDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = self.resolved_config(config)?;
        let tracked_devices = host.discovery_state().tracked_devices("govee").await;
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
impl DriverRuntimeCacheProvider for GoveeDriverFactory {
    async fn snapshot(
        &self,
        host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        let tracked_devices = host.discovery_state().tracked_devices("govee").await;
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
impl PairingCapability for GoveeDriverFactory {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        _device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        match host
            .credentials()
            .get_json(GOVEE_ACCOUNT_CREDENTIAL_KEY)
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
            .get_json(GOVEE_ACCOUNT_CREDENTIAL_KEY)
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
            .set_json(GOVEE_ACCOUNT_CREDENTIAL_KEY, json!({ "api_key": api_key }))
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
            .remove(GOVEE_ACCOUNT_CREDENTIAL_KEY)
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
        family: DeviceFamily::Govee,
        model: Some(device.model.clone()),
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count,
            topology: topology_for_family(profile.family),
            color_format: DeviceColorFormat::Rgb,
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
        ("backend_id".to_owned(), "govee".to_owned()),
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
        .get_json(GOVEE_ACCOUNT_CREDENTIAL_KEY)
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
