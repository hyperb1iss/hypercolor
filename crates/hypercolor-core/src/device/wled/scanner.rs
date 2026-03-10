//! mDNS-based discovery scanner for WLED devices.
//!
//! Implements [`TransportScanner`] to discover WLED controllers on the
//! local network via `_wled._tcp.local.` service browsing.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::device::discovery::{DiscoveredDevice, TransportScanner};
use crate::types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceInfo, DeviceTopologyHint, ZoneInfo,
};

/// mDNS service type for WLED devices.
const WLED_SERVICE_TYPE: &str = "_wled._tcp.local.";

/// Default scan timeout for mDNS browsing.
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(5);
/// Best-effort wait for the mDNS daemon to acknowledge shutdown.
const MDNS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

// ── WledScanner ─────────────────────────────────────────────────────────

/// mDNS-based transport scanner for WLED devices.
///
/// Browses for `_wled._tcp.local.` services, resolves their addresses,
/// and optionally enriches via the WLED JSON API over HTTP.
pub struct WledScanner {
    /// How long to browse before returning results.
    scan_timeout: Duration,
    /// Explicit IPs to probe over HTTP before or alongside mDNS.
    known_ips: Vec<IpAddr>,
    /// Whether to browse mDNS in addition to known-IP probing.
    mdns_enabled: bool,
}

impl WledScanner {
    /// Create a new scanner with the default 5-second timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scan_timeout: DEFAULT_SCAN_TIMEOUT,
            known_ips: Vec::new(),
            mdns_enabled: true,
        }
    }

    /// Create a scanner with a custom timeout.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            scan_timeout: timeout,
            known_ips: Vec::new(),
            mdns_enabled: true,
        }
    }

    /// Create a scanner that probes known IPs and optionally browses mDNS.
    #[must_use]
    pub fn with_known_ips(known_ips: Vec<IpAddr>, mdns_enabled: bool, timeout: Duration) -> Self {
        Self {
            scan_timeout: timeout,
            known_ips,
            mdns_enabled,
        }
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
            family: DeviceFamily::Wled,
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: u32::from(led_count),
                topology: DeviceTopologyHint::Strip,
                color_format,
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
            name,
            family: DeviceFamily::Wled,
            fingerprint,
            info: device_info,
            metadata,
        }
    }

    async fn collect_mdns_candidates(&self) -> Result<HashMap<IpAddr, Option<String>>> {
        if !self.mdns_enabled {
            return Ok(HashMap::new());
        }

        let daemon = ServiceDaemon::new().context("Failed to create mDNS daemon")?;
        let receiver = daemon
            .browse(WLED_SERVICE_TYPE)
            .context("Failed to start mDNS browse")?;

        let mut candidates = HashMap::new();
        let deadline = tokio::time::Instant::now() + self.scan_timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, async {
                receiver
                    .recv_async()
                    .await
                    .map_err(|e| anyhow::anyhow!("mDNS recv error: {e}"))
            })
            .await
            {
                Ok(Ok(ServiceEvent::ServiceResolved(info))) => {
                    let Some(&ip) = info.get_addresses().iter().next() else {
                        debug!("mDNS resolved service with no addresses, skipping");
                        continue;
                    };

                    let hostname = info.get_hostname().trim_end_matches('.').to_owned();
                    info!(ip = %ip, hostname = %hostname, "Found WLED device via mDNS");
                    candidates.entry(ip).or_insert(Some(hostname));
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    warn!(error = %e, "mDNS browse error");
                    break;
                }
                Err(_) => {
                    break;
                }
            }
        }

        // Drain the shutdown status receiver so `mdns-sd` does not log an
        // internal error when it reports daemon exit on a dropped channel.
        match daemon.shutdown() {
            Ok(receiver) => {
                match tokio::time::timeout(MDNS_SHUTDOWN_TIMEOUT, receiver.recv_async()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        debug!(error = %error, "mDNS daemon shutdown channel closed");
                    }
                    Err(_) => {
                        debug!(
                            timeout_ms = MDNS_SHUTDOWN_TIMEOUT.as_millis(),
                            "timed out waiting for mDNS daemon shutdown"
                        );
                    }
                }
            }
            Err(error) => {
                debug!(error = %error, "failed to request mDNS daemon shutdown");
            }
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
        let mut candidates: HashMap<IpAddr, Option<String>> = self
            .known_ips
            .iter()
            .copied()
            .map(|ip| (ip, None))
            .collect();
        let known_ip_set = candidates
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut enrichment_tasks = JoinSet::new();

        for ip in known_ip_set.iter().copied() {
            enrichment_tasks.spawn(async move { (ip, enrich_via_http(ip).await) });
        }

        for (ip, hostname) in self.collect_mdns_candidates().await? {
            candidates.entry(ip).or_insert(hostname);
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
        for (ip, hostname) in candidates {
            let wled_info = match enriched.remove(&ip) {
                Some(Ok(info)) => Some(info),
                Some(Err(error)) => {
                    let host_label = hostname.as_deref().unwrap_or("<unknown>");
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

            if wled_info.is_none() && hostname.is_none() {
                debug!(
                    ip = %ip,
                    "Skipping stale WLED probe candidate without mDNS hostname or HTTP enrichment"
                );
                continue;
            }

            let host_label = hostname.unwrap_or_else(|| ip.to_string());

            discovered.push(Self::build_discovered(&host_label, ip, wled_info.as_ref()));
        }

        info!(count = discovered.len(), "WLED mDNS scan complete");
        Ok(discovered)
    }
}

/// Fetch `/json/info` from a WLED device over HTTP.
async fn enrich_via_http(ip: std::net::IpAddr) -> Result<super::backend::WledDeviceInfo> {
    super::fetch_wled_info(ip).await
}
