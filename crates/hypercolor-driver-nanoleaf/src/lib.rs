use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
pub use hypercolor_core::device::nanoleaf::NanoleafKnownDevice;
use hypercolor_core::device::nanoleaf::{
    DEFAULT_NANOLEAF_API_PORT, NanoleafBackend, NanoleafScanner, pair_device_with_status,
};
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
use hypercolor_types::config::NanoleafConfig;

const NANOLEAF_PAIRING_INSTRUCTIONS: &[&str] = &[
    "Hold the Nanoleaf power button for 5-7 seconds.",
    "Wait for the controller to enter pairing mode.",
    "Click Pair Device.",
];

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("nanoleaf", "Nanoleaf", DriverTransport::Network, true, true);

#[derive(Clone)]
pub struct NanoleafDriverFactory {
    credential_store: Arc<CredentialStore>,
    config: NanoleafConfig,
    mdns_enabled: bool,
}

impl NanoleafDriverFactory {
    #[must_use]
    pub fn new(
        credential_store: Arc<CredentialStore>,
        config: NanoleafConfig,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            credential_store,
            config,
            mdns_enabled,
        }
    }
}

impl NetworkDriverFactory for NanoleafDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, _host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(NanoleafBackend::with_mdns_enabled(
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
impl DiscoveryCapability for NanoleafDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResult> {
        let tracked_devices = host.discovery_state().tracked_devices("nanoleaf").await;
        let known_devices =
            resolve_nanoleaf_probe_devices_from_sources(&self.config, &tracked_devices);
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

#[async_trait]
impl PairingCapability for NanoleafDriverFactory {
    async fn auth_summary(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let last_error = device
            .metadata
            .and_then(|values| values.get("auth_error").cloned());
        let configured =
            nanoleaf_credentials_present(&self.credential_store, device.metadata).await;

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
        if nanoleaf_credentials_present(&self.credential_store, device.metadata).await {
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
        clear_nanoleaf_credentials(&self.credential_store, device.metadata).await?;
        let disconnected = disconnect_after_unpair(host, device.device_id, "nanoleaf").await;

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
