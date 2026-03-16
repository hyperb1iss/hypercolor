use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::DeviceBackend;
use hypercolor_core::device::hue::{DEFAULT_HUE_API_PORT, HueBackend, HueBridgeClient};
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, DriverDescriptor, DriverHost,
    DriverTransport, NetworkDriverFactory, PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus,
    PairingCapability, PairingDescriptor, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_types::config::HueConfig;

use super::host::DaemonDriverHost;
use super::pairing::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_ip_from_metadata,
    push_lookup_key,
};

const HUE_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Press the link button on the Hue Bridge.",
    "Return here within 30 seconds.",
    "Click Pair Bridge.",
];

pub(crate) static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, false, true);

#[derive(Clone)]
pub(crate) struct HueDriverFactory {
    host: Arc<DaemonDriverHost>,
    config: HueConfig,
    mdns_enabled: bool,
}

impl HueDriverFactory {
    pub(crate) fn new(host: Arc<DaemonDriverHost>, config: HueConfig, mdns_enabled: bool) -> Self {
        Self {
            host,
            config,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for HueDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(HueBackend::with_mdns_enabled(
            self.config.clone(),
            self.host.credential_store(),
            self.mdns_enabled,
        ))))
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
    }
}

#[async_trait]
impl PairingCapability for HueDriverFactory {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let _ = host;
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured =
            hue_credentials_present(&self.host.credential_store(), device.metadata).await;

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
        if hue_credentials_present(&self.host.credential_store(), device.metadata).await {
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

        let Some(bridge_ip) = network_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Hue bridge is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let bridge_port = device
            .metadata
            .and_then(|values| values.get("api_port"))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_HUE_API_PORT);

        match pair_hue_bridge_at_ip(&self.host.credential_store(), bridge_ip, bridge_port).await? {
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
        clear_hue_credentials(&self.host.credential_store(), device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "hue").await;

        Ok(ClearPairingOutcome {
            message: "Hue bridge credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHuePairingResult {
    pub device_key: Option<String>,
    pub name: Option<String>,
}

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
