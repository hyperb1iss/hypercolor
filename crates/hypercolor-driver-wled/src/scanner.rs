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
    CoverageGap, DiscoveredDevice, DiscoveryBatch, DiscoveryConnectBehavior, DiscoveryObservation,
};
use hypercolor_driver_support::discovery::{
    DEFAULT_PROBE_CONCURRENCY, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
use hypercolor_driver_support::http::HttpError;
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
    tracked: HashMap<IpAddr, DeviceFingerprint>,
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
            tracked: HashMap::new(),
            mdns: Some(MdnsScope::Network),
        }
    }

    /// Create a scanner with a custom timeout.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            scan_timeout: timeout,
            known_targets: Vec::new(),
            tracked: HashMap::new(),
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
            tracked: HashMap::new(),
            mdns,
        }
    }

    /// Name the devices the host tracks now. A cached fingerprint may belong
    /// to a device this run has not registered, so only these can be renewed.
    #[must_use]
    pub fn with_tracked(mut self, tracked: HashMap<IpAddr, DeviceFingerprint>) -> Self {
        self.tracked = tracked;
        self
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
    /// and a known address nothing vouched for is simply gone. An advertised
    /// device the host tracks renews its presence. Any other surfaces as a
    /// deferred placeholder, under its cached identity when a previous run
    /// left one, so it stays visible until HTTP answers.
    fn unprobed_observation(
        ip: IpAddr,
        target: WledKnownTarget,
        advertised: bool,
        tracked: Option<&DeviceFingerprint>,
    ) -> Option<DiscoveryObservation> {
        if !advertised {
            debug!(ip = %ip, "Skipping stale WLED probe candidate without HTTP enrichment");
            return None;
        }
        if let Some(fingerprint) = tracked {
            return Some(DiscoveryObservation::Present(fingerprint.clone()));
        }
        let hostname = target.hostname.as_deref()?;
        debug!(
            ip = %ip,
            hostname = %hostname,
            "Using mDNS-only WLED placeholder until HTTP enrichment succeeds"
        );
        let mut placeholder = Self::build_discovered(hostname, ip, None);
        if let Some(fingerprint) = target.fingerprint {
            placeholder.info.id = fingerprint.stable_device_id();
            placeholder.fingerprint = fingerprint;
        }
        Some(DiscoveryObservation::Device(placeholder))
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
    /// simply gone, unless a controller answered and could not be read, which
    /// holds the device tracked there. An advertised one is alive, so it
    /// renews presence when the host tracks it and surfaces as a deferred
    /// placeholder when not.
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
            let advertised = mdns_seen_ips.contains(&ip);
            let tracked = self.tracked.get(&ip);
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
                    if !advertised
                        && controller_answered(&error)
                        && let Some(fingerprint) = tracked
                    {
                        gaps.push(CoverageGap::DeviceUnverified {
                            fingerprint: fingerprint.clone(),
                            reason: format!("{error:#}"),
                        });
                    }
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

            observations.extend(Self::unprobed_observation(ip, target, advertised, tracked));
        }

        debug!(count = observations.len(), "WLED mDNS scan complete");
        Ok(DiscoveryBatch::new(observations, gaps))
    }
}

/// Whether a failed probe came from a live controller: a server error, or a
/// reply it could not make sense of. Silence is not that, and neither is a 4xx
/// from whatever host now holds the address.
fn controller_answered(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<HttpError>())
        .is_none_or(|http| match http {
            HttpError::Status(status) => status.is_server_error(),
            HttpError::Json(_) | HttpError::BodyTooLarge { .. } => true,
            HttpError::ClientBuild(_) | HttpError::Transport(_) | HttpError::DeadlineExceeded => {
                false
            }
        })
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 7));
    const CACHED: &str = "net:wled:aabbccddeeff";

    fn target(fingerprint: Option<&str>, hostname: Option<&str>) -> WledKnownTarget {
        WledKnownTarget {
            hostname: hostname.map(ToOwned::to_owned),
            fingerprint: fingerprint.map(|raw| DeviceFingerprint::from_persisted(raw.to_owned())),
            ..WledKnownTarget::from_ip(IP)
        }
    }

    #[test]
    fn an_advertised_tracked_device_renews_presence_without_a_record() {
        let tracked = DeviceFingerprint::from_persisted(CACHED.to_owned());
        let observation = WledScanner::unprobed_observation(
            IP,
            target(Some(CACHED), Some("wled.local")),
            true,
            Some(&tracked),
        );
        assert!(matches!(
            observation,
            Some(DiscoveryObservation::Present(fingerprint)) if fingerprint == tracked
        ));
    }

    #[test]
    fn an_advertised_device_this_run_never_registered_stays_visible_under_its_cached_identity() {
        let observation = WledScanner::unprobed_observation(
            IP,
            target(Some(CACHED), Some("wled.local")),
            true,
            None,
        );
        let Some(DiscoveryObservation::Device(device)) = observation else {
            panic!("a fresh registry would discard presence, so this must be a description");
        };
        assert_eq!(device.fingerprint.as_str(), CACHED);
        assert_eq!(device.info.id, device.fingerprint.stable_device_id());
        assert_eq!(device.connect_behavior, DiscoveryConnectBehavior::Deferred);
        assert!(device.claim.is_none(), "a cached string proves nothing");
    }

    #[test]
    fn an_advertised_stranger_becomes_a_deferred_placeholder() {
        let observation =
            WledScanner::unprobed_observation(IP, target(None, Some("wled.local")), true, None);
        let Some(DiscoveryObservation::Device(device)) = observation else {
            panic!("an advertised device with no identity surfaces as a placeholder");
        };
        assert_eq!(device.connect_behavior, DiscoveryConnectBehavior::Deferred);
    }

    #[test]
    fn a_silent_address_nothing_advertised_is_gone() {
        let tracked = DeviceFingerprint::from_persisted(CACHED.to_owned());
        let observation = WledScanner::unprobed_observation(
            IP,
            target(Some(CACHED), Some("wled.local")),
            false,
            Some(&tracked),
        );
        assert!(observation.is_none());
    }

    /// The error a probe of a server answering `reply` ends in.
    async fn probe_error(reply: &'static str) -> anyhow::Error {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 512];
            let _ = stream.read(&mut buffer).await;
            let _ = stream.write_all(reply.as_bytes()).await;
        });
        http_probe(addr).await
    }

    async fn http_probe(addr: std::net::SocketAddr) -> anyhow::Error {
        let client = crate::wled_info_http_client().expect("client");
        crate::fetch_json(
            client,
            &format!("http://{addr}/json/info"),
            Duration::from_secs(2),
        )
        .await
        .expect_err("no reply here is valid JSON")
    }

    #[tokio::test]
    async fn a_server_error_or_garbage_comes_from_a_live_controller() {
        let server_error =
            probe_error("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
        assert!(controller_answered(&server_error.await));
        let garbage = probe_error("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nnope");
        assert!(controller_answered(&garbage.await));
        assert!(
            controller_answered(&anyhow::anyhow!("reply is missing the LED count")),
            "JSON that is not WLED's still came from something alive"
        );
    }

    #[tokio::test]
    async fn silence_and_a_foreign_host_are_not_a_live_controller() {
        let not_found = probe_error("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        assert!(!controller_answered(&not_found.await));

        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = closed.local_addr().expect("address");
        drop(closed);
        assert!(!controller_answered(&http_probe(addr).await));
    }
}
