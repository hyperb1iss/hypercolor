use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::DeviceBackend;
use hypercolor_core::device::TransportScanner;
use hypercolor_core::device::wled::{
    WledBackend, WledDeviceInfo, WledKnownTarget, WledProtocol, WledScanner,
};
use hypercolor_driver_api::{
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverDescriptor, DriverHost,
    DriverTransport, NetworkDriverFactory,
};
use hypercolor_types::config::{HypercolorConfig, WledProtocolConfig};
use hypercolor_types::device::DeviceId;
use tracing::warn;

use super::DaemonDriverHost;
use crate::runtime_state;

pub(crate) static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("wled", "WLED", DriverTransport::Network, true, false);

#[derive(Clone)]
pub(crate) struct WledDriverFactory {
    host: Arc<DaemonDriverHost>,
    config: HypercolorConfig,
    runtime_state_path: PathBuf,
}

impl WledDriverFactory {
    pub(crate) fn new(
        host: Arc<DaemonDriverHost>,
        config: HypercolorConfig,
        runtime_state_path: PathBuf,
    ) -> Self {
        Self {
            host,
            config,
            runtime_state_path,
        }
    }
}

impl NetworkDriverFactory for WledDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(&self, host: &dyn DriverHost) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        Ok(Some(Box::new(build_wled_backend(
            &self.config,
            &self.runtime_state_path,
        ))))
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait]
impl DiscoveryCapability for WledDriverFactory {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResult> {
        let _ = host;
        let runtime = self.host.discovery_runtime();
        let known_targets = crate::discovery::resolve_wled_probe_targets(
            &runtime.device_registry,
            &self.config.wled,
            &runtime.runtime_state_path,
        )
        .await;
        let mut scanner =
            WledScanner::with_known_targets(known_targets, request.mdns_enabled, request.timeout);
        let devices = scanner
            .scan()
            .await?
            .into_iter()
            .map(super::into_driver_discovered)
            .collect();

        Ok(DiscoveryResult { devices })
    }
}

pub fn build_wled_backend(config: &HypercolorConfig, runtime_state_path: &Path) -> WledBackend {
    let mut known_ips: HashSet<_> = config.wled.known_ips.iter().copied().collect();
    match runtime_state::load_wled_probe_ips(runtime_state_path) {
        Ok(cached_ips) => {
            known_ips.extend(cached_ips);
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "Failed to load cached WLED probe IPs; falling back to config only"
            );
        }
    }

    let mut resolved_known_ips: Vec<_> = known_ips.into_iter().collect();
    resolved_known_ips.sort_unstable();

    let mut backend =
        WledBackend::with_mdns_fallback(resolved_known_ips, config.discovery.mdns_enabled);
    match runtime_state::load_wled_probe_targets(runtime_state_path) {
        Ok(cached_targets) => {
            for target in cached_targets {
                let Some((device_id, ip, info)) = cached_wled_backend_seed(&target) else {
                    continue;
                };
                backend.remember_device(device_id, ip, info);
            }
        }
        Err(error) => {
            warn!(
                path = %runtime_state_path.display(),
                %error,
                "Failed to load cached WLED identity hints; backend will rely on fresh probing"
            );
        }
    }
    let protocol = match config.wled.default_protocol {
        WledProtocolConfig::Ddp => WledProtocol::Ddp,
        WledProtocolConfig::E131 => WledProtocol::E131,
    };
    backend.set_protocol(protocol);
    backend.set_realtime_http_enabled(config.wled.realtime_http_enabled);
    backend.set_dedup_threshold(config.wled.dedup_threshold);
    backend
}

fn cached_wled_backend_seed(
    target: &WledKnownTarget,
) -> Option<(DeviceId, IpAddr, WledDeviceInfo)> {
    let fingerprint = target.fingerprint.clone()?;
    let name = target.name.clone()?;
    let led_count = target.led_count?;
    let fps = target
        .max_fps
        .map_or(60, |value| u8::try_from(value).unwrap_or(u8::MAX));

    Some((
        fingerprint.stable_device_id(),
        target.ip,
        WledDeviceInfo {
            firmware_version: target
                .firmware_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            build_id: 0,
            mac: fingerprint
                .0
                .strip_prefix("net:")
                .filter(|value| !value.starts_with("wled:"))
                .unwrap_or_default()
                .to_owned(),
            name,
            led_count: u16::try_from(led_count).unwrap_or(u16::MAX),
            rgbw: target.rgbw.unwrap_or(false),
            max_segments: 1,
            fps,
            power_draw_ma: 0,
            max_power_ma: 0,
            free_heap: 0,
            uptime_secs: 0,
            arch: "unknown".to_owned(),
            is_wifi: true,
            effect_count: 0,
            palette_count: 0,
        },
    ))
}
