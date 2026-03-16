use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::DeviceBackend;
use hypercolor_core::device::nanoleaf::{
    DEFAULT_NANOLEAF_API_PORT, NanoleafBackend, pair_device_with_status,
};
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, DriverDescriptor, DriverHost,
    DriverTransport, NetworkDriverFactory, PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus,
    PairingCapability, PairingDescriptor, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_types::config::NanoleafConfig;

use super::host::DaemonDriverHost;
use super::pairing::{
    activate_if_requested, disconnect_after_unpair, metadata_value, network_ip_from_metadata,
    push_lookup_key,
};

const NANOLEAF_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Hold the Nanoleaf power button for 5-7 seconds.",
    "Wait for the controller to enter pairing mode.",
    "Click Pair Device.",
];

pub(crate) static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "nanoleaf",
    "Nanoleaf",
    DriverTransport::Network,
    false,
    true,
);

#[derive(Clone)]
pub(crate) struct NanoleafDriverFactory {
    host: Arc<DaemonDriverHost>,
    config: NanoleafConfig,
    mdns_enabled: bool,
}

impl NanoleafDriverFactory {
    pub(crate) fn new(
        host: Arc<DaemonDriverHost>,
        config: NanoleafConfig,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            host,
            config,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for NanoleafDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(NanoleafBackend::with_mdns_enabled(
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
impl PairingCapability for NanoleafDriverFactory {
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
            nanoleaf_credentials_present(&self.host.credential_store(), device.metadata).await;

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
        if nanoleaf_credentials_present(&self.host.credential_store(), device.metadata).await {
            let activated = activate_if_requested(
                host,
                request.activate_after_pair,
                device.device_id,
                "nanoleaf",
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

        let Some(device_ip) = network_ip_from_metadata(device.metadata) else {
            return Ok(PairDeviceOutcome {
                status: PairDeviceStatus::InvalidInput,
                message: "Nanoleaf device is missing network address metadata".to_owned(),
                auth_state: DeviceAuthState::Required,
                activated: false,
            });
        };
        let api_port = device
            .metadata
            .and_then(|values| values.get("api_port"))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_NANOLEAF_API_PORT);

        match pair_nanoleaf_device_at_ip(&self.host.credential_store(), device_ip, api_port).await?
        {
            Some(_) => {
                let activated = activate_if_requested(
                    host,
                    request.activate_after_pair,
                    device.device_id,
                    "nanoleaf",
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
        clear_nanoleaf_credentials(&self.host.credential_store(), device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "nanoleaf").await;

        Ok(ClearPairingOutcome {
            message: "Nanoleaf credentials removed.".to_owned(),
            auth_state: DeviceAuthState::Required,
            disconnected,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNanoleafPairingResult {
    pub device_key: String,
    pub name: String,
}

pub async fn pair_nanoleaf_device_at_ip(
    credential_store: &CredentialStore,
    device_ip: IpAddr,
    api_port: u16,
) -> Result<Option<StoredNanoleafPairingResult>> {
    let Some(pair_result) = pair_device_with_status(device_ip, api_port).await? else {
        return Ok(None);
    };

    let credentials = Credentials::Nanoleaf {
        auth_token: pair_result.auth_token,
    };
    credential_store
        .store(
            &format!("nanoleaf:{}", pair_result.device_key),
            credentials.clone(),
        )
        .await?;
    credential_store
        .store(&format!("nanoleaf:ip:{device_ip}"), credentials)
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
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> bool {
    for key in nanoleaf_credential_keys(metadata) {
        if matches!(
            credential_store.get(&key).await,
            Some(Credentials::Nanoleaf { .. })
        ) {
            return true;
        }
    }
    false
}

async fn clear_nanoleaf_credentials(
    credential_store: &CredentialStore,
    metadata: Option<&HashMap<String, String>>,
) -> Result<()> {
    for key in nanoleaf_credential_keys(metadata) {
        credential_store.remove(&key).await?;
    }
    Ok(())
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
