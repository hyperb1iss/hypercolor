use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
pub use hypercolor_core::device::hue::HueKnownBridge;
use hypercolor_core::device::hue::{DEFAULT_HUE_API_PORT, HueBackend, HueBridgeClient, HueScanner};
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::support::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_port_from_metadata,
    push_lookup_key,
};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, DiscoveryCapability, DiscoveryRequest,
    DiscoveryResult, DriverDescriptor, DriverDiscoveredDevice, DriverHost, DriverTrackedDevice,
    DriverTransport, NetworkDriverFactory, PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus,
    PairingCapability, PairingDescriptor, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_types::config::HueConfig;

const HUE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Press the link button on the Hue Bridge.",
    "Return here within 30 seconds.",
    "Click Pair Bridge.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, true, true);

#[derive(Clone)]
pub struct HueDriverFactory {
    credential_store: Arc<CredentialStore>,
    config: HueConfig,
    mdns_enabled: bool,
}

impl HueDriverFactory {
    #[must_use]
    pub fn new(
        credential_store: Arc<CredentialStore>,
        config: HueConfig,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            credential_store,
            config,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for HueDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, _host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(HueBackend::with_mdns_enabled(
            self.config.clone(),
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
}

#[async_trait]
impl DiscoveryCapability for HueDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResult> {
        let tracked_devices = host.discovery_state().tracked_devices("hue").await;
        let known_bridges = resolve_hue_probe_bridges_from_sources(&self.config, &tracked_devices);
        let mut scanner = HueScanner::with_options(
            known_bridges,
            Arc::clone(&self.credential_store),
            request.timeout,
            request.mdns_enabled,
            self.config.entertainment_config.clone(),
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
impl PairingCapability for HueDriverFactory {
    async fn auth_summary(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured = hue_credentials_present(&self.credential_store, device.metadata).await;

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
        if hue_credentials_present(&self.credential_store, device.metadata).await {
            let activated =
                activate_if_requested(host, request.activate_after_pair, device.device_id, "hue")
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
                    "hue",
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
        clear_hue_credentials(&self.credential_store, device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "hue").await;

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
    let credentials = Credentials::HueBridge {
        api_key: pair_result.api_key,
        client_key: pair_result.client_key,
    };

    if let Some(identity) = bridge_identity.as_ref() {
        credential_store
            .store(&format!("hue:{}", identity.bridge_id), credentials.clone())
            .await?;
    }
    credential_store
        .store(&format!("hue:ip:{bridge_ip}"), credentials)
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
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> bool {
    for key in hue_credential_keys(metadata) {
        if matches!(
            credential_store.get(&key).await,
            Some(Credentials::HueBridge { .. })
        ) {
            return true;
        }
    }
    false
}

async fn clear_hue_credentials(
    credential_store: &CredentialStore,
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
