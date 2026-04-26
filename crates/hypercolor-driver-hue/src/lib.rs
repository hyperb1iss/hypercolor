use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_core::device::hue::{DEFAULT_HUE_API_PORT, HueBackend, HueBridgeClient, HueScanner};
pub use hypercolor_core::device::hue::{HueConfig, HueKnownBridge};
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::support::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_port_from_metadata,
    push_lookup_key,
};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, ControlApplyTarget, DeviceAuthState, DeviceAuthSummary,
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigView,
    DriverControlProvider, DriverCredentialStore, DriverDescriptor, DriverDiscoveredDevice,
    DriverHost, DriverTrackedDevice, DriverTransport, NetworkDriverFactory, PairDeviceOutcome,
    PairDeviceRequest, PairDeviceStatus, PairingCapability, PairingDescriptor, PairingFlowKind,
    TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlActionResult, ControlAvailabilityExpr, ControlChange, ControlFieldDescriptor,
    ControlGroupDescriptor, ControlGroupKind, ControlOwner, ControlPersistence,
    ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueMap, ControlValueType,
    ControlVisibility,
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

#[derive(Clone)]
pub struct HueDriverFactory {
    credential_store: Arc<CredentialStore>,
    mdns_enabled: bool,
}

impl HueDriverFactory {
    #[must_use]
    pub fn new(credential_store: Arc<CredentialStore>, mdns_enabled: bool) -> Self {
        Self {
            credential_store,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for HueDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(
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

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }
}

#[async_trait]
impl DiscoveryCapability for HueDriverFactory {
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

#[async_trait]
impl DriverControlProvider for HueDriverFactory {
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
        push_unique_impact(&mut impacts, field.apply_impact.clone());
    }

    Ok(ValidatedControlChanges {
        changes: changes.to_vec(),
        impacts,
    })
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
impl PairingCapability for HueDriverFactory {
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
            .store_json(&format!("hue:{}", identity.bridge_id), credentials.clone())
            .await?;
    }
    credential_store
        .store_json(&format!("hue:ip:{bridge_ip}"), credentials)
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
        push_lookup_key(&mut keys, format!("hue:{bridge_id}"));
    }
    if let Some(ip) = metadata_value(metadata, "ip") {
        push_lookup_key(&mut keys, format!("hue:ip:{ip}"));
    }
    keys
}

async fn hue_credentials_present(
    credential_store: &dyn DriverCredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<bool> {
    for key in hue_credential_keys(metadata) {
        let Some(credentials) = credential_store.get_json(&key).await? else {
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
        credential_store.remove(&key).await?;
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
