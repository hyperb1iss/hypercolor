//! mDNS-based discovery for WLED devices.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tracing::{debug, warn};

use hypercolor_driver_api::{
    DiscoveredDevice, DiscoveryBatch, DiscoveryConnectBehavior, DiscoveryObservation,
};
use hypercolor_driver_support::discovery::{
    DEFAULT_PROBE_CONCURRENCY, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
use hypercolor_driver_support::{MdnsBrowser, MdnsScope, MdnsService};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, FingerprintNamespace,
    SegmentInfo,
};
use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};

/// mDNS service type for WLED devices.
const WLED_SERVICE_TYPE: &str = "_wled._tcp.local.";

/// Default scan timeout for mDNS browsing.
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(5);

/// One HTTP probe at worst: the client's 5 s timeout, plus a moment so a
/// probe's own timeout lands before the scan deadline.
const PROBE_ROUND: Duration = Duration::from_secs(6);

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
    /// Forward-compatible driver-owned fields.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl WledKnownTarget {
    /// Create a bare probe target from an IP address.
    #[must_use]
    pub fn from_ip(ip: IpAddr) -> Self {
        Self {
            ip,
            hostname: None,
            fingerprint: None,
            extra: BTreeMap::new(),
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
        for (key, value) in &other.extra {
            self.extra
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    /// Refresh fields observed during a positive discovery without dropping extensions.
    pub fn refresh_from(&mut self, other: &Self) {
        if other.hostname.is_some() {
            self.hostname.clone_from(&other.hostname);
        }
        if other.fingerprint.is_some() {
            self.fingerprint.clone_from(&other.fingerprint);
        }
        self.extra.extend(other.extra.clone());
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
    /// Interfaces to browse mDNS on, beside known-IP probing, if any.
    mdns: Option<MdnsScope>,
}

impl WledScanner {
    /// Create a new scanner with the default 5-second timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scan_timeout: DEFAULT_SCAN_TIMEOUT,
            known_targets: Vec::new(),
            mdns: Some(MdnsScope::Network),
        }
    }

    /// Create a scanner with a custom timeout.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            scan_timeout: timeout,
            known_targets: Vec::new(),
            mdns: Some(MdnsScope::Network),
        }
    }

    /// Create a scanner with persisted known-device hints.
    #[must_use]
    pub fn with_known_targets(
        known_targets: Vec<WledKnownTarget>,
        mdns: Option<MdnsScope>,
        timeout: Duration,
    ) -> Self {
        Self {
            scan_timeout: timeout,
            known_targets,
            mdns,
        }
    }

    /// Create a scanner that probes known IPs and optionally browses mDNS.
    #[must_use]
    pub fn with_known_ips(
        known_ips: Vec<IpAddr>,
        mdns: Option<MdnsScope>,
        timeout: Duration,
    ) -> Self {
        Self::with_known_targets(
            known_ips
                .into_iter()
                .map(WledKnownTarget::from_ip)
                .collect(),
            mdns,
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

        let fingerprint_key = if mac.is_empty() { hostname } else { &mac };
        let fingerprint =
            DeviceFingerprint::mint(FingerprintNamespace::Net, "wled", fingerprint_key);
        let device_id = fingerprint.stable_device_id();

        let device_info = DeviceInfo {
            id: device_id,
            name: name.clone(),
            vendor: "WLED".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            segments: vec![SegmentInfo {
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
            if !info.mac.is_empty() {
                metadata.insert("mac".to_owned(), info.mac.clone());
            }
        }

        DiscoveredDevice {
            fingerprint,
            connect_behavior: if wled_info.is_some() {
                DiscoveryConnectBehavior::AutoConnect
            } else {
                DiscoveryConnectBehavior::Deferred
            },
            info: device_info,
            metadata,
            // The device just reported its own MAC; the constructor refuses
            // the unstable ones (locally administered, multicast, all-zero).
            claim: PortableIdentityClaim::mac_address(&mac, NetworkAttachment::Peer(ip)),
        }
    }

    /// What a candidate without a working HTTP probe amounts to.
    ///
    /// Only mDNS can vouch for it: a device that advertised itself is alive,
    /// and a known address nothing vouched for is simply gone.
    fn unprobed_observation(
        ip: IpAddr,
        target: WledKnownTarget,
        advertised: bool,
    ) -> Option<DiscoveryObservation> {
        if !advertised {
            debug!(ip = %ip, "Skipping stale WLED probe candidate without HTTP enrichment");
            return None;
        }
        if let Some(fingerprint) = target.fingerprint {
            return Some(DiscoveryObservation::Present(fingerprint));
        }
        let hostname = target.hostname.as_deref()?;
        debug!(
            ip = %ip,
            hostname = %hostname,
            "Using mDNS-only WLED placeholder until HTTP enrichment succeeds"
        );
        Some(DiscoveryObservation::Device(Self::build_discovered(
            hostname, ip, None,
        )))
    }

    async fn browse_mdns(&self, known_ips: &[IpAddr]) -> Result<ScanOutcome<MdnsService>> {
        let Some(scope) = self.mdns else {
            return Ok(ScanOutcome::default());
        };
        MdnsBrowser::scan(scope, WLED_SERVICE_TYPE, self.scan_timeout, known_ips)
            .await
            .context("failed to browse WLED mDNS services")
    }
}

impl Default for WledScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl WledScanner {
    /// Discover configured and mDNS-visible WLED devices.
    ///
    /// A known address that fails its HTTP probe and is not advertised is
    /// simply gone. An advertised one is alive, so it renews presence when
    /// the host tracks it and surfaces as a deferred placeholder when not.
    ///
    /// # Errors
    ///
    /// Returns an error when mDNS cannot run and no known device gives the
    /// scan anything else to look at.
    pub async fn scan(&mut self) -> Result<DiscoveryBatch> {
        let started = Instant::now();
        let mut candidates: HashMap<IpAddr, WledKnownTarget> = self
            .known_targets
            .iter()
            .cloned()
            .map(|target| (target.ip, target))
            .collect();
        let known_ips: Vec<IpAddr> = candidates.keys().copied().collect();
        let budget = ProbeBudget {
            deadline: started + self.scan_timeout + PROBE_ROUND,
            in_flight: DEFAULT_PROBE_CONCURRENCY,
        };

        // Known devices are probed while mDNS browses, so a long browse
        // never delays them.
        let (known_probes, browse) = tokio::join!(
            probe_candidates(known_ips.iter().copied(), budget, enrich_candidate),
            self.browse_mdns(&known_ips),
        );
        let browse = browse?;
        let mut gaps = known_probes.gaps;
        gaps.extend(browse.gaps);
        let mut enriched: HashMap<IpAddr, _> = known_probes.found.into_iter().collect();

        let mut mdns_seen_ips = HashSet::new();
        let mut advertised_only = Vec::new();
        for service in browse.found {
            debug!(
                ip = %service.host,
                hostname = %service.name,
                "Found WLED device via mDNS"
            );
            mdns_seen_ips.insert(service.host);
            let entry = candidates.entry(service.host).or_insert_with(|| {
                advertised_only.push(service.host);
                WledKnownTarget::from_ip(service.host)
            });
            if entry.hostname.is_none() {
                entry.hostname = Some(service.name);
            }
        }

        let advertised_probes = probe_candidates(advertised_only, budget, enrich_candidate).await;
        gaps.extend(advertised_probes.gaps);
        enriched.extend(advertised_probes.found);

        let mut observations = Vec::with_capacity(candidates.len());
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
                observations.push(DiscoveryObservation::Device(Self::build_discovered(
                    &host_label,
                    ip,
                    Some(info),
                )));
                continue;
            }

            observations.extend(Self::unprobed_observation(
                ip,
                target,
                mdns_seen_ips.contains(&ip),
            ));
        }

        debug!(count = observations.len(), "WLED mDNS scan complete");
        Ok(DiscoveryBatch::new(observations, gaps))
    }
}

/// Fetch `/json/info` from a WLED device over HTTP, keeping the outcome for
/// the scan to weigh against what mDNS says.
async fn enrich_candidate(
    ip: IpAddr,
) -> Result<Probed<(IpAddr, Result<super::backend::WledDeviceInfo>)>, Infallible> {
    Ok(Probed::Described((ip, super::fetch_wled_info(ip).await)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 7));

    fn target(fingerprint: Option<&str>, hostname: Option<&str>) -> WledKnownTarget {
        WledKnownTarget {
            hostname: hostname.map(ToOwned::to_owned),
            fingerprint: fingerprint.map(|raw| DeviceFingerprint::from_persisted(raw.to_owned())),
            ..WledKnownTarget::from_ip(IP)
        }
    }

    #[test]
    fn an_advertised_known_device_renews_presence_without_a_record() {
        let observation = WledScanner::unprobed_observation(
            IP,
            target(Some("net:wled:aabbccddeeff"), Some("wled.local")),
            true,
        );
        assert!(matches!(
            observation,
            Some(DiscoveryObservation::Present(fingerprint))
                if fingerprint.as_str() == "net:wled:aabbccddeeff"
        ));
    }

    #[test]
    fn an_advertised_stranger_becomes_a_deferred_placeholder() {
        let observation =
            WledScanner::unprobed_observation(IP, target(None, Some("wled.local")), true);
        let Some(DiscoveryObservation::Device(device)) = observation else {
            panic!("an advertised device with no identity surfaces as a placeholder");
        };
        assert_eq!(device.connect_behavior, DiscoveryConnectBehavior::Deferred);
    }

    #[test]
    fn a_silent_address_nothing_advertised_is_gone() {
        let observation = WledScanner::unprobed_observation(
            IP,
            target(Some("net:wled:aabbccddeeff"), Some("wled.local")),
            false,
        );
        assert!(observation.is_none());
    }
}
