pub mod backend;
mod scanner;
mod streaming;
mod topology;
mod types;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::support::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_port_from_metadata,
    push_lookup_key,
};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, ControlApplyTarget, DeviceAuthState, DeviceAuthSummary,
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigProvider, DriverConfigView,
    DriverControlProvider, DriverCredentialStore, DriverDescriptor, DriverDiscoveredDevice,
    DriverHost, DriverTrackedDevice, DriverTransport, NetworkDriverFactory, PairDeviceOutcome,
    PairDeviceRequest, PairDeviceStatus, PairingCapability, PairingDescriptor, PairingFlowKind,
    TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlActionResult, ControlAvailabilityExpr, ControlChange, ControlFieldDescriptor,
    ControlGroupDescriptor, ControlGroupKind, ControlOwner, ControlPersistence,
    ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueMap, ControlValueType,
    ControlVisibility,
};
use reqwest::StatusCode;
use serde::Deserialize;

use self::types::NanoleafPanelLayoutResponse;

pub use backend::{NanoleafBackend, NanoleafConfig};
pub use scanner::{NanoleafKnownDevice, NanoleafScanner};
pub use streaming::{
    DEFAULT_NANOLEAF_API_PORT, DEFAULT_NANOLEAF_STREAM_PORT, NanoleafStreamSession,
    encode_frame_into,
};
pub use topology::NanoleafShapeType;
pub use types::{NanoleafDeviceInfo, NanoleafDiscoveredDevice, NanoleafPanelLayout};
#[doc(hidden)]
pub use types::{build_device_info, panel_ids_from_layout};

const NANOLEAF_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Hold the Nanoleaf power button for 5-7 seconds.",
    "Wait for the controller to enter pairing mode.",
    "Click Pair Device.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("nanoleaf", "Nanoleaf", DriverTransport::Network, true, true);

const FIELD_DEVICE_IPS: &str = "device_ips";
const FIELD_TRANSITION_TIME: &str = "transition_time";

static NANOLEAF_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
});

fn nanoleaf_http_client() -> Result<&'static reqwest::Client> {
    NANOLEAF_HTTP_CLIENT
        .as_ref()
        .map_err(|error| anyhow!("failed to build shared Nanoleaf HTTP client: {error}"))
}

/// Result of a successful Nanoleaf pairing attempt.
#[derive(Debug, Clone)]
pub struct NanoleafPairResult {
    pub auth_token: String,
    pub device_key: String,
    pub name: String,
    pub model: String,
    pub firmware_version: String,
    pub serial_no: String,
}

/// Attempt to pair with a Nanoleaf device.
///
/// Returns `Ok(None)` when the device is not in pairing mode.
///
/// # Errors
///
/// Returns an error if the pairing request fails or the device-info fetch after
/// pairing is malformed.
pub async fn pair_device_with_status(
    ip: IpAddr,
    api_port: u16,
) -> Result<Option<NanoleafPairResult>> {
    let url = format!("http://{ip}:{api_port}/api/v1/new");
    let client = nanoleaf_http_client()?;
    let response = client
        .post(&url)
        .send()
        .await
        .with_context(|| format!("Nanoleaf pairing request to {url} failed"))?;
    if matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
    ) {
        return Ok(None);
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("Nanoleaf pairing request to {url} failed"))?;
    let body: NanoleafPairResponse = response
        .json()
        .await
        .with_context(|| format!("failed to decode Nanoleaf pairing response from {url}"))?;
    let Some(auth_token) = body.auth_token else {
        return Ok(None);
    };

    let info = fetch_device_info(ip, api_port, &auth_token).await?;
    Ok(Some(NanoleafPairResult {
        auth_token,
        device_key: info.serial_no.clone(),
        name: info.name,
        model: info.model,
        firmware_version: info.firmware_version,
        serial_no: info.serial_no,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NanoleafPairResponse {
    auth_token: Option<String>,
}

async fn fetch_device_info(
    ip: IpAddr,
    api_port: u16,
    auth_token: &str,
) -> Result<NanoleafDeviceInfo> {
    let url = format!("http://{ip}:{api_port}/api/v1/{auth_token}");
    let client = nanoleaf_http_client()?;
    let response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Nanoleaf device-info request to {url} failed"))?;

    response
        .json()
        .await
        .with_context(|| format!("failed to decode Nanoleaf device-info response from {url}"))
}

async fn fetch_panel_layout(
    ip: IpAddr,
    api_port: u16,
    auth_token: &str,
) -> Result<NanoleafPanelLayoutResponse> {
    let url = format!("http://{ip}:{api_port}/api/v1/{auth_token}/panelLayout/layout");
    let client = nanoleaf_http_client()?;
    let response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Nanoleaf panel-layout request to {url} failed"))?;
    let decoded: NanoleafPanelLayoutResponse = response
        .json()
        .await
        .with_context(|| format!("failed to decode Nanoleaf panel-layout response from {url}"))?;
    Ok(decoded)
}

#[derive(Clone)]
pub struct NanoleafDriverFactory {
    credential_store: Arc<CredentialStore>,
    mdns_enabled: bool,
}

impl NanoleafDriverFactory {
    #[must_use]
    pub fn new(credential_store: Arc<CredentialStore>, mdns_enabled: bool) -> Self {
        Self {
            credential_store,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for NanoleafDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(NanoleafBackend::with_mdns_enabled(
            config.parse_settings::<NanoleafConfig>()?,
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
}

#[async_trait]
impl DiscoveryCapability for NanoleafDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = config.parse_settings::<NanoleafConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices(DESCRIPTOR.id).await;
        let known_devices = resolve_nanoleaf_probe_devices_from_sources(&config, &tracked_devices);
        let mut scanner = NanoleafScanner::with_options(
            known_devices,
            Arc::clone(&self.credential_store),
            request.timeout,
            request.mdns_enabled,
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

impl DriverConfigProvider for NanoleafDriverFactory {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(BTreeMap::from([
            (FIELD_DEVICE_IPS.to_owned(), serde_json::json!([])),
            (FIELD_TRANSITION_TIME.to_owned(), serde_json::json!(1)),
        ]))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<NanoleafConfig>()?;
        for ip in config.device_ips {
            validate_ip(ip).with_context(|| format!("invalid Nanoleaf device IP: {ip}"))?;
        }
        Ok(())
    }
}

#[async_trait]
impl DriverControlProvider for NanoleafDriverFactory {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(nanoleaf_driver_control_surface(
            &config.parse_settings::<NanoleafConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        _device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(None)
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        validate_nanoleaf_driver_changes(target, changes)
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        let ControlApplyTarget::Driver { driver_id, config } = target else {
            bail!("Nanoleaf controls cannot apply to device targets");
        };
        if *driver_id != DESCRIPTOR.id {
            bail!("Nanoleaf controls cannot apply to driver '{driver_id}'");
        }

        let control_host = host
            .control_host()
            .ok_or_else(|| anyhow!("driver control host services are unavailable"))?;
        let mut values = nanoleaf_config_values(&config.parse_settings::<NanoleafConfig>()?);
        for change in &changes.changes {
            values.insert(change.field_id.clone(), change.value.clone());
        }
        control_host
            .driver_config_store()
            .save_driver_values(DESCRIPTOR.id, values.clone())
            .await?;

        Ok(nanoleaf_apply_response(
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
        bail!("unknown Nanoleaf control action: {action_id}")
    }
}

#[must_use]
pub fn nanoleaf_driver_control_surface(config: &NanoleafConfig) -> ControlSurfaceDocument {
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
    document.fields = nanoleaf_driver_control_fields();
    document.values = nanoleaf_config_values(config);
    document.revision = nanoleaf_control_revision(&document.values);
    document
}

fn validate_nanoleaf_driver_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    let ControlApplyTarget::Driver { driver_id, .. } = target else {
        bail!("Nanoleaf controls cannot validate device targets");
    };
    if *driver_id != DESCRIPTOR.id {
        bail!("Nanoleaf controls cannot validate driver '{driver_id}'");
    }

    let fields = nanoleaf_driver_control_fields()
        .into_iter()
        .map(|field| (field.id.clone(), field))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut impacts = Vec::new();

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            bail!("duplicate Nanoleaf control field: {}", change.field_id);
        }
        let field = fields
            .get(&change.field_id)
            .ok_or_else(|| anyhow!("unknown Nanoleaf control field: {}", change.field_id))?;
        field
            .value_type
            .validate_value(&change.value)
            .with_context(|| format!("invalid Nanoleaf control field: {}", change.field_id))?;
        if change.field_id == FIELD_DEVICE_IPS {
            validate_control_ip_list("Nanoleaf device IP", &change.value)?;
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

fn nanoleaf_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        nanoleaf_driver_field(
            FIELD_DEVICE_IPS,
            "Device IPs",
            Some("connection"),
            ControlValueType::List {
                item_type: Box::new(ControlValueType::IpAddress),
                min_items: None,
                max_items: Some(64),
            },
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        nanoleaf_driver_field(
            FIELD_TRANSITION_TIME,
            "Transition Time",
            Some("output"),
            ControlValueType::Integer {
                min: Some(0),
                max: Some(i64::from(u16::MAX)),
                step: Some(1),
            },
            ApplyImpact::BackendRebind,
            10,
        ),
    ]
}

fn nanoleaf_driver_field(
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

fn nanoleaf_config_values(config: &NanoleafConfig) -> ControlValueMap {
    ControlValueMap::from([
        (
            FIELD_DEVICE_IPS.to_owned(),
            ControlValue::List(
                config
                    .device_ips
                    .iter()
                    .map(|ip| ControlValue::IpAddress(ip.to_string()))
                    .collect(),
            ),
        ),
        (
            FIELD_TRANSITION_TIME.to_owned(),
            ControlValue::Integer(i64::from(config.transition_time)),
        ),
    ])
}

fn nanoleaf_apply_response(
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

fn nanoleaf_control_revision(values: &ControlValueMap) -> u64 {
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
impl PairingCapability for NanoleafDriverFactory {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured = nanoleaf_credentials_present(host.credentials(), device.metadata)
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
            descriptor: Some(nanoleaf_pairing_descriptor()),
            last_error,
        })
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        if nanoleaf_credentials_present(host.credentials(), device.metadata)
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
                "Nanoleaf credentials are already configured and the device was activated."
            } else {
                "Nanoleaf credentials are already configured."
            };
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::AlreadyPaired,
                message: message.to_owned(),
                auth_state: DeviceAuthState::Configured,
                activated,
            });
        }

        let Some(device_ip) = pairing_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Nanoleaf device is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let api_port = network_port_from_metadata(device.metadata, "api_port")
            .unwrap_or(DEFAULT_NANOLEAF_API_PORT);

        match pair_nanoleaf_device_at_ip(&self.credential_store, device_ip, api_port).await? {
            Some(_) => {
                let activated = activate_if_requested(
                    host,
                    request.activate_after_pair,
                    device.device_id,
                    DESCRIPTOR.id,
                )
                .await;
                let message = if activated {
                    "Nanoleaf device paired and activated."
                } else {
                    "Nanoleaf device paired. Credentials are stored."
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
                message: "Hold the Nanoleaf power button for 5-7 seconds, then retry pairing."
                    .to_owned(),
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
        clear_nanoleaf_credentials(host.credentials(), device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, DESCRIPTOR.id).await;

        Ok(ClearPairingOutcome {
            message: "Nanoleaf credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

/// Merge Nanoleaf probe hints from config and tracked devices.
#[must_use]
pub fn resolve_nanoleaf_probe_devices_from_sources(
    config: &NanoleafConfig,
    tracked_devices: &[DriverTrackedDevice],
) -> Vec<NanoleafKnownDevice> {
    let mut known_devices: HashMap<IpAddr, NanoleafKnownDevice> = config
        .device_ips
        .iter()
        .copied()
        .map(NanoleafKnownDevice::from_ip)
        .map(|device| (device.ip, device))
        .collect();

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

        let port = network_port_from_metadata(Some(&tracked.metadata), "api_port")
            .unwrap_or(DEFAULT_NANOLEAF_API_PORT);
        let device_key = tracked
            .metadata
            .get("device_key")
            .cloned()
            .unwrap_or_else(|| tracked.info.name.to_ascii_lowercase().replace(' ', "-"));

        known_devices
            .entry(ip)
            .and_modify(|existing| {
                if existing.device_id.is_empty() {
                    existing.device_id.clone_from(&device_key);
                }
                existing.port = port;
                if existing.name.is_empty() {
                    existing.name.clone_from(&tracked.info.name);
                }
                if existing.model.is_empty() {
                    existing.model = tracked.info.model.clone().unwrap_or_default();
                }
                if existing.firmware.is_empty() {
                    existing.firmware = tracked.info.firmware_version.clone().unwrap_or_default();
                }
            })
            .or_insert_with(|| NanoleafKnownDevice {
                device_id: device_key,
                ip,
                port,
                name: tracked.info.name.clone(),
                model: tracked.info.model.clone().unwrap_or_default(),
                firmware: tracked.info.firmware_version.clone().unwrap_or_default(),
            });
    }

    let mut resolved: Vec<_> = known_devices.into_values().collect();
    resolved.sort_by_key(|device| device.ip);
    resolved
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNanoleafPairingResult {
    pub device_key: String,
    pub name: String,
}

/// Pair directly against a Nanoleaf IP and persist credentials.
///
/// # Errors
///
/// Returns an error if the Nanoleaf pairing exchange or credential persistence fails.
pub async fn pair_nanoleaf_device_at_ip(
    credential_store: &CredentialStore,
    device_ip: IpAddr,
    api_port: u16,
) -> Result<Option<StoredNanoleafPairingResult>> {
    let Some(pair_result) = pair_device_with_status(device_ip, api_port).await? else {
        return Ok(None);
    };

    let credentials = serde_json::json!({
        "auth_token": pair_result.auth_token,
    });
    credential_store
        .store_json(
            &format!("nanoleaf:{}", pair_result.device_key),
            credentials.clone(),
        )
        .await?;
    credential_store
        .store_json(&format!("nanoleaf:ip:{device_ip}"), credentials)
        .await?;

    Ok(Some(StoredNanoleafPairingResult {
        device_key: pair_result.device_key,
        name: pair_result.name,
    }))
}

fn nanoleaf_credential_keys(metadata: Option<&HashMap<String, String>>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(device_key) = metadata_value(metadata, "device_key") {
        push_lookup_key(&mut keys, format!("nanoleaf:{device_key}"));
    }
    if let Some(ip) = metadata_value(metadata, "ip") {
        push_lookup_key(&mut keys, format!("nanoleaf:ip:{ip}"));
    }
    keys
}

async fn nanoleaf_credentials_present(
    credential_store: &dyn DriverCredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<bool> {
    for key in nanoleaf_credential_keys(metadata) {
        let Some(credentials) = credential_store.get_json(&key).await? else {
            continue;
        };
        if credentials
            .get("auth_token")
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn clear_nanoleaf_credentials(
    credential_store: &dyn DriverCredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<()> {
    for key in nanoleaf_credential_keys(metadata) {
        credential_store.remove(&key).await?;
    }
    Ok(())
}

fn pairing_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
    metadata_value(metadata, "ip").and_then(|value| value.parse::<IpAddr>().ok())
}

fn nanoleaf_pairing_descriptor() -> PairingDescriptor {
    PairingDescriptor {
        kind: PairingFlowKind::PhysicalAction,
        title: "Pair Nanoleaf Device".to_owned(),
        instructions: NANOLEAF_PAIRING_INSTRUCTIONS
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
        action_label: "Pair Device".to_owned(),
        fields: Vec::new(),
    }
}
