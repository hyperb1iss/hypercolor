//! Govee network driver.

use std::collections::HashMap;
use std::net::IpAddr;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverTrackedDevice, DriverTransport, NetworkDriverFactory,
};
use hypercolor_types::config::GoveeConfig;

pub mod backend;
pub mod capabilities;
pub mod lan;

use backend::GoveeBackend;
use lan::discovery::{GoveeKnownDevice, GoveeLanScanner};

pub use capabilities::{
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, profile_for_sku,
};
pub use lan::discovery::{GoveeLanDevice, build_device_info, parse_scan_response};

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("govee", "Govee", DriverTransport::Network, true, false);

#[derive(Clone)]
pub struct GoveeDriverFactory {
    config: GoveeConfig,
}

impl GoveeDriverFactory {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self { config }
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
