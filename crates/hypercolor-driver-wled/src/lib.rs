use std::collections::HashSet;
use std::net::IpAddr;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypercolor_core::device::wled::{
    WledBackend, WledDeviceInfo, WledKnownTarget, WledProtocol, WledScanner,
};
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_api::validation::validate_ip;
use hypercolor_driver_api::{
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigView, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverTrackedDevice, DriverTransport, NetworkDriverFactory,
};
use hypercolor_types::config::{WledConfig, WledProtocolConfig};
use hypercolor_types::device::DeviceId;

pub static DESCRIPTOR: DriverDescriptor =
    DriverDescriptor::new("wled", "WLED", DriverTransport::Network, true, false);

#[derive(Clone)]
pub struct WledDriverFactory {
    mdns_enabled: bool,
}

impl WledDriverFactory {
    #[must_use]
    pub const fn new(mdns_enabled: bool) -> Self {
        Self { mdns_enabled }
    }
}

impl NetworkDriverFactory for WledDriverFactory {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(build_wled_backend(
            &config.parse_settings::<WledConfig>()?,
            self.mdns_enabled,
            host,
        )?)))
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
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = config.parse_settings::<WledConfig>()?;
        let tracked_devices = host.discovery_state().tracked_devices("wled").await;
        let cached_probe_ips = load_cached_probe_ips(host)?;
        let cached_targets = load_cached_probe_targets(host)?;
        let known_targets = resolve_wled_probe_targets_from_sources(
            &config,
            &tracked_devices,
            &cached_probe_ips,
            &cached_targets,
        );
        let mut scanner =
            WledScanner::with_known_targets(known_targets, request.mdns_enabled, request.timeout);
        let devices = scanner
            .scan()
            .await?
            .into_iter()
            .map(DriverDiscoveredDevice::from)
            .collect();

        Ok(DiscoveryResult { devices })
    }
}

/// Build the runtime WLED backend using config and cached discovery hints.
///
/// # Errors
///
/// Returns an error if cached probe data cannot be parsed.
pub fn build_wled_backend(
    config: &WledConfig,
    mdns_enabled: bool,
    host: &dyn DriverHost,
) -> Result<WledBackend> {
    let mut known_ips: HashSet<_> = config.known_ips.iter().copied().collect();
    known_ips.extend(load_cached_probe_ips(host)?);

    let mut resolved_known_ips: Vec<_> = known_ips.into_iter().collect();
    resolved_known_ips.sort_unstable();

    let mut backend = WledBackend::with_mdns_fallback(resolved_known_ips, mdns_enabled);
    for target in load_cached_probe_targets(host)? {
        let Some((device_id, ip, info)) = cached_wled_backend_seed(&target) else {
            continue;
        };
        backend.remember_device(device_id, ip, info);
    }
    let protocol = match config.default_protocol {
        WledProtocolConfig::Ddp => WledProtocol::Ddp,
        WledProtocolConfig::E131 => WledProtocol::E131,
    };
    backend.set_protocol(protocol);
    backend.set_realtime_http_enabled(config.realtime_http_enabled);
    backend.set_dedup_threshold(config.dedup_threshold);
    Ok(backend)
}

/// Merge WLED probe IPs from config, tracked devices, and cached discovery.
#[must_use]
pub fn resolve_wled_probe_ips_from_sources(
    config: &WledConfig,
    tracked_devices: &[DriverTrackedDevice],
    cached_probe_ips: &[IpAddr],
    cached_targets: &[WledKnownTarget],
) -> Vec<IpAddr> {
    let mut known_ips: HashSet<IpAddr> = config.known_ips.iter().copied().collect();
    known_ips.extend(cached_probe_ips.iter().copied());
    known_ips.extend(cached_targets.iter().map(|target| target.ip));

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
        known_ips.insert(ip);
    }

    let mut resolved: Vec<IpAddr> = known_ips.into_iter().collect();
    resolved.sort_unstable();
    resolved
}

/// Merge WLED probe targets from config, tracked devices, and cached discovery.
#[must_use]
pub fn resolve_wled_probe_targets_from_sources(
    config: &WledConfig,
    tracked_devices: &[DriverTrackedDevice],
    cached_probe_ips: &[IpAddr],
    cached_targets: &[WledKnownTarget],
) -> Vec<WledKnownTarget> {
    let mut known_targets: std::collections::HashMap<IpAddr, WledKnownTarget> = config
        .known_ips
        .iter()
        .copied()
        .map(WledKnownTarget::from_ip)
        .map(|target| (target.ip, target))
        .collect();

    for ip in cached_probe_ips {
        known_targets
            .entry(*ip)
            .or_insert_with(|| WledKnownTarget::from_ip(*ip));
    }

    for target in cached_targets {
        known_targets
            .entry(target.ip)
            .and_modify(|existing| existing.merge_from(target))
            .or_insert_with(|| target.clone());
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

        let rgbw = tracked.info.zones.first().map(|zone| {
            matches!(
                zone.color_format,
                hypercolor_types::device::DeviceColorFormat::Rgbw
            )
        });
        let target = WledKnownTarget {
            ip,
            hostname: tracked.metadata.get("hostname").cloned(),
            fingerprint: tracked.fingerprint.clone(),
            name: Some(tracked.info.name.clone()),
            led_count: Some(tracked.info.total_led_count()),
            firmware_version: tracked.info.firmware_version.clone(),
            max_fps: Some(tracked.info.capabilities.max_fps),
            rgbw,
        };

        known_targets
            .entry(ip)
            .and_modify(|existing| existing.merge_from(&target))
            .or_insert(target);
    }

    let mut resolved: Vec<WledKnownTarget> = known_targets.into_values().collect();
    resolved.sort_by_key(|target| target.ip);
    resolved
}

fn load_cached_probe_ips(host: &dyn DriverHost) -> Result<Vec<IpAddr>> {
    host.discovery_state()
        .load_cached_json("wled", "probe_ips")?
        .map(serde_json::from_value)
        .transpose()
        .context("failed to parse cached WLED probe IPs")
        .map(Option::unwrap_or_default)
}

fn load_cached_probe_targets(host: &dyn DriverHost) -> Result<Vec<WledKnownTarget>> {
    host.discovery_state()
        .load_cached_json("wled", "probe_targets")?
        .map(serde_json::from_value)
        .transpose()
        .context("failed to parse cached WLED probe targets")
        .map(Option::unwrap_or_default)
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
