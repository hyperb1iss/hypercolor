//! Govee network driver.

use std::collections::HashMap;
use std::net::IpAddr;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::support::{activate_if_requested, disconnect_after_unpair};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, DiscoveryCapability, DiscoveryRequest,
    DiscoveryResult, DriverDescriptor, DriverDiscoveredDevice, DriverHost, DriverTrackedDevice,
    DriverTransport, NetworkDriverFactory, PairDeviceOutcome, PairDeviceRequest, PairDeviceStatus,
    PairingCapability, PairingDescriptor, PairingFieldDescriptor, PairingFlowKind,
    TrackedDeviceCtx,
};
use hypercolor_types::config::GoveeConfig;
use serde_json::json;

pub mod backend;
pub mod capabilities;
pub mod cloud;
pub mod lan;

use backend::GoveeBackend;
use cloud::CloudClient;
use lan::discovery::{GoveeKnownDevice, GoveeLanScanner};

pub use capabilities::{
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, known_sku_count, profile_for_sku,
};
pub use lan::discovery::{GoveeLanDevice, build_device_info, parse_scan_response};

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
    cloud_base_url: Option<String>,
}

impl GoveeDriverFactory {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            cloud_base_url: None,
        }
    }

    #[must_use]
    pub fn with_cloud_base_url(config: GoveeConfig, cloud_base_url: impl Into<String>) -> Self {
        Self {
            config,
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
}

impl NetworkDriverFactory for GoveeDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, _host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(GoveeBackend::new(self.config.clone()))))
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(self)
    }
}

#[async_trait]
impl DiscoveryCapability for GoveeDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResult> {
        let tracked_devices = host.discovery_state().tracked_devices("govee").await;
        let known_devices =
            resolve_govee_probe_devices_from_sources(&self.config, &tracked_devices);
        let mut scanner = GoveeLanScanner::new(known_devices, request.timeout);
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
            Ok(None) => Some(DeviceAuthSummary {
                state: DeviceAuthState::Open,
                can_pair: true,
                descriptor: Some(govee_pairing_descriptor()),
                last_error: None,
            }),
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
    let mut known_devices: HashMap<IpAddr, GoveeKnownDevice> = config
        .known_ips
        .iter()
        .copied()
        .map(GoveeKnownDevice::from_ip)
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

        let known = GoveeKnownDevice {
            ip,
            sku: tracked.metadata.get("sku").cloned(),
            mac: tracked.metadata.get("mac").cloned(),
        };
        known_devices
            .entry(ip)
            .and_modify(|existing| {
                if existing.sku.is_none() {
                    existing.sku.clone_from(&known.sku);
                }
                if existing.mac.is_none() {
                    existing.mac.clone_from(&known.mac);
                }
            })
            .or_insert(known);
    }

    let mut resolved: Vec<_> = known_devices.into_values().collect();
    resolved.sort_by_key(|device| device.ip);
    resolved
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
