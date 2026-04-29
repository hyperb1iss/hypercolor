//! mDNS-based discovery scanner for WLED devices.
//!
//! Implements [`TransportScanner`] to discover WLED controllers on the
//! local network via `_wled._tcp.local.` service browsing.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use hypercolor_driver_api::MdnsBrowser;
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

/// mDNS service type for WLED devices.
const WLED_SERVICE_TYPE: &str = "_wled._tcp.local.";

/// Default scan timeout for mDNS browsing.
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(5);

/// Persistable scanner hint for known WLED devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WledKnownTarget {
    /// Target IP address to probe.
    pub ip: IpAddr,
    /// Last-known mDNS hostname, if available.
    #[serde(default)]
    pub hostname: Option<String>,
    /// Stable device fingerprint from a previous successful discovery.
    #[serde(default)]
    pub fingerprint: Option<DeviceFingerprint>,
    /// Last-known friendly WLED display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Last-known LED count.
    #[serde(default)]
    pub led_count: Option<u32>,
    /// Last-known firmware version.
    #[serde(default)]
    pub firmware_version: Option<String>,
    /// Last-known max FPS capability.
    #[serde(default)]
    pub max_fps: Option<u32>,
    /// Whether the device reported RGBW support.
    #[serde(default)]
    pub rgbw: Option<bool>,
}

impl WledKnownTarget {
    /// Create a bare probe target from an IP address.
    #[must_use]
    pub fn from_ip(ip: IpAddr) -> Self {
        Self {
            ip,
            hostname: None,
            fingerprint: None,
            name: None,
            led_count: None,
            firmware_version: None,
            max_fps: None,
            rgbw: None,
        }
    }

    /// Merge richer cached identity fields from another target.
    pub fn merge_from(&mut self, other: &Self) {
        if self.hostname.is_none() {
            self.hostname.clone_from(&other.hostname);
        }
        if self.fingerprint.is_none() {
            self.fingerprint.clone_from(&other.fingerprint);
        }
        if self.name.is_none() {
            self.name.clone_from(&other.name);
        }
        if self.led_count.is_none() {
            self.led_count = other.led_count;
        }
        if self.firmware_version.is_none() {
            self.firmware_version.clone_from(&other.firmware_version);
        }
        if self.max_fps.is_none() {
            self.max_fps = other.max_fps;
        }
        if self.rgbw.is_none() {
            self.rgbw = other.rgbw;
        }
    }
}

// ── WledScanner ─────────────────────────────────────────────────────────

/// mDNS-based transport scanner for WLED devices.
///
/// Browses for `_wled._tcp.local.` services, resolves their addresses,
/// and optionally enriches via the WLED JSON API over HTTP.
pub struct WledScanner {
    /// How long to browse before returning results.
    scan_timeout: Duration,
    /// Explicit IPs to probe over HTTP before or alongside mDNS.
    known_targets: Vec<WledKnownTarget>,
    /// Whether to browse mDNS in addition to known-IP probing.
    mdns_enabled: bool,
}

impl WledScanner {
    /// Create a new scanner with the default 5-second timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scan_timeout: DEFAULT_SCAN_TIMEOUT,
            known_targets: Vec::new(),
            mdns_enabled: true,
        }
    }

    /// Create a scanner with a custom timeout.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            scan_timeout: timeout,
            known_targets: Vec::new(),
            mdns_enabled: true,
        }
    }

    /// Create a scanner with persisted known-device hints.
    #[must_use]
    pub fn with_known_targets(
        known_targets: Vec<WledKnownTarget>,
        mdns_enabled: bool,
        timeout: Duration,
    ) -> Self {
        Self {
            scan_timeout: timeout,
            known_targets,
            mdns_enabled,
        }
    }

    /// Create a scanner that probes known IPs and optionally browses mDNS.
    #[must_use]
    pub fn with_known_ips(known_ips: Vec<IpAddr>, mdns_enabled: bool, timeout: Duration) -> Self {
        Self::with_known_targets(
            known_ips
                .into_iter()
                .map(WledKnownTarget::from_ip)
                .collect(),
            mdns_enabled,
            timeout,
        )
    }

    /// Build a `DiscoveredDevice` from mDNS service info.
    fn build_discovered(
        hostname: &str,
        ip: std::net::IpAddr,
        wled_info: Option<&super::backend::WledDeviceInfo>,
    ) -> DiscoveredDevice {
        let (name, led_count, firmware_version, rgbw, mac) = match wled_info {
            Some(info) => (
                info.name.clone(),
                info.led_count,
                Some(info.firmware_version.clone()),
                info.rgbw,
                info.mac.clone(),
            ),
            None => (hostname.to_owned(), 0, None, false, String::new()),
        };

        let color_format = if rgbw {
            DeviceColorFormat::Rgbw
        } else {
            DeviceColorFormat::Rgb
        };

        // Use MAC address for fingerprint if available, else hostname
        let fingerprint_key = if mac.is_empty() {
            format!("net:wled:{hostname}")
        } else {
            format!("net:{mac}")
        };
        let fingerprint = DeviceFingerprint(fingerprint_key);
        let device_id = fingerprint.stable_device_id();

        let device_info = DeviceInfo {
            id: device_id,
            name: name.clone(),
            vendor: "WLED".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: u32::from(led_count),
                topology: DeviceTopologyHint::Strip,
                color_format,
                layout_hint: None,
            }],
            firmware_version,
            capabilities: DeviceCapabilities {
                led_count: u32::from(led_count),
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: wled_info
                    .map_or(60, super::backend::WledDeviceInfo::negotiated_target_fps),
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        };

        let mut metadata = HashMap::new();
        metadata.insert("ip".to_owned(), ip.to_string());
        metadata.insert("hostname".to_owned(), hostname.to_owned());
        if let Some(info) = wled_info {
            metadata.insert("arch".to_owned(), info.arch.clone());
            metadata.insert("firmware".to_owned(), info.firmware_version.clone());
        }

        DiscoveredDevice {
            connection_type: ConnectionType::Network,
            origin: device_info.origin.clone(),
            name,
            family: DeviceFamily::new_static("wled", "WLED"),
            fingerprint,
            connect_behavior: if wled_info.is_some() {
                DiscoveryConnectBehavior::AutoConnect
            } else {
                DiscoveryConnectBehavior::Deferred
            },
            info: device_info,
            metadata,
        }
    }

    fn build_discovered_from_known_target(target: &WledKnownTarget) -> Option<DiscoveredDevice> {
        let has_cached_identity = target.fingerprint.is_some()
            || target.name.is_some()
            || target.led_count.is_some()
            || target.firmware_version.is_some()
            || target.max_fps.is_some()
            || target.rgbw.is_some();
        if !has_cached_identity {
            return None;
        }

        let name = target.name.clone().or_else(|| target.hostname.clone())?;
        let fingerprint = target.fingerprint.clone().or_else(|| {
            target
                .hostname
                .as_ref()
                .map(|hostname| DeviceFingerprint(format!("net:wled:{hostname}")))
        })?;
        let color_format = if target.rgbw.unwrap_or(false) {
            DeviceColorFormat::Rgbw
        } else {
            DeviceColorFormat::Rgb
        };
        let led_count = target.led_count.unwrap_or(0);
        let device_id = fingerprint.stable_device_id();

        let device_info = DeviceInfo {
            id: device_id,
            name: name.clone(),
            vendor: "WLED".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count,
                topology: DeviceTopologyHint::Strip,
                color_format,
                layout_hint: None,
            }],
            firmware_version: target.firmware_version.clone(),
            capabilities: DeviceCapabilities {
                led_count,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: target.max_fps.unwrap_or(60),
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        };

        let mut metadata = HashMap::new();
        metadata.insert("ip".to_owned(), target.ip.to_string());
        if let Some(hostname) = &target.hostname {
            metadata.insert("hostname".to_owned(), hostname.clone());
        }

        Some(DiscoveredDevice {
            connection_type: ConnectionType::Network,
            origin: device_info.origin.clone(),
            name,
            family: DeviceFamily::new_static("wled", "WLED"),
            fingerprint,
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info: device_info,
            metadata,
        })
    }

    async fn collect_mdns_candidates(&self) -> Result<HashMap<IpAddr, Option<String>>> {
        if !self.mdns_enabled {
            return Ok(HashMap::new());
        }

        let browser = MdnsBrowser::new()?;
        let services = browser
            .browse(WLED_SERVICE_TYPE, self.scan_timeout)
            .await
            .context("failed to browse WLED mDNS services")?;
        let mut candidates = HashMap::new();
        for service in services {
            info!(
                ip = %service.host,
                hostname = %service.name,
                "Found WLED device via mDNS"
            );
            candidates.entry(service.host).or_insert(Some(service.name));
        }
        Ok(candidates)
    }
}

impl Default for WledScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TransportScanner for WledScanner {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "WLED mDNS"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let mut candidates: HashMap<IpAddr, WledKnownTarget> = self
            .known_targets
            .iter()
            .cloned()
            .map(|target| (target.ip, target))
            .collect();
        let mut mdns_seen_ips = HashSet::new();
        let known_ip_set = candidates
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut enrichment_tasks = JoinSet::new();

        for ip in known_ip_set.iter().copied() {
            enrichment_tasks.spawn(async move { (ip, enrich_via_http(ip).await) });
        }

        for (ip, hostname) in self.collect_mdns_candidates().await? {
            mdns_seen_ips.insert(ip);
            let entry = candidates
                .entry(ip)
                .or_insert_with(|| WledKnownTarget::from_ip(ip));
            if entry.hostname.is_none() {
                entry.hostname = hostname;
            }
            if !known_ip_set.contains(&ip) {
                enrichment_tasks.spawn(async move { (ip, enrich_via_http(ip).await) });
            }
        }

        let mut enriched = HashMap::new();
        while let Some(task) = enrichment_tasks.join_next().await {
            match task {
                Ok((ip, result)) => {
                    enriched.insert(ip, result);
                }
                Err(error) => {
                    warn!(error = %error, "WLED enrichment task failed");
                }
            }
        }

        let mut discovered: Vec<DiscoveredDevice> = Vec::with_capacity(candidates.len());
        for (ip, target) in candidates {
            let wled_info = match enriched.remove(&ip) {
                Some(Ok(info)) => Some(info),
                Some(Err(error)) => {
                    let host_label = target.hostname.as_deref().unwrap_or("<unknown>");
                    warn!(
                        ip = %ip,
                        hostname = %host_label,
                        error = %error,
                        "WLED candidate found but HTTP enrichment failed"
                    );
                    None
                }
                None => None,
            };

            if let Some(info) = wled_info.as_ref() {
                let host_label = target.hostname.clone().unwrap_or_else(|| ip.to_string());
                discovered.push(Self::build_discovered(&host_label, ip, Some(info)));
                continue;
            }

            if mdns_seen_ips.contains(&ip)
                && let Some(cached) = Self::build_discovered_from_known_target(&target)
            {
                discovered.push(cached);
                continue;
            }

            if mdns_seen_ips.contains(&ip)
                && let Some(hostname) = target.hostname.as_deref()
            {
                debug!(
                    ip = %ip,
                    hostname = %hostname,
                    "Using mDNS-only WLED placeholder until HTTP enrichment succeeds"
                );
                discovered.push(Self::build_discovered(hostname, ip, None));
                continue;
            }

            debug!(
                ip = %ip,
                "Skipping stale WLED probe candidate without cached identity or HTTP enrichment"
            );
        }

        info!(count = discovered.len(), "WLED mDNS scan complete");
        Ok(discovered)
    }
}

/// Fetch `/json/info` from a WLED device over HTTP.
async fn enrich_via_http(ip: std::net::IpAddr) -> Result<super::backend::WledDeviceInfo> {
    super::fetch_wled_info(ip).await
}
