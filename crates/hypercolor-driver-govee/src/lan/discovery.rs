use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_driver_support::discovery::{
    Admission, CandidateAdmission, KnownTarget, ScanOutcome,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, FingerprintNamespace,
    SegmentInfo,
};
use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::debug;

use crate::capabilities::{
    GoveeCapabilities, SkuFamily, SkuProfile, fallback_profile, profile_for_sku,
};
use crate::lan::protocol::{DEVICE_PORT, LISTEN_PORT, LanCommand, MULTICAST_ADDR, encode_command};

/// Unknown MACs one scan admits. Homes run from a few lights to a few dozen;
/// 256 leaves headroom while capping how many invented identities one scan
/// can hand the device registry.
pub const MAX_SCAN_RESPONSES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoveeKnownDevice {
    pub ip: IpAddr,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl GoveeKnownDevice {
    #[must_use]
    pub fn from_ip(ip: IpAddr) -> Self {
        Self {
            ip,
            sku: None,
            mac: None,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoveeLanDevice {
    pub ip: IpAddr,
    pub sku: String,
    pub mac: String,
    pub name: String,
    pub firmware_version: Option<String>,
}

/// Admission that always keeps a slot for `known` devices: by MAC when one is
/// known, else by the configured address.
#[must_use]
pub fn scan_admission(
    known: &[GoveeKnownDevice],
    limit: usize,
) -> CandidateAdmission<String, DiscoveredDevice> {
    CandidateAdmission::new(
        limit,
        known.iter().map(|device| match device.mac.as_deref() {
            Some(mac) => KnownTarget::Identity(normalize_mac(mac)),
            None => KnownTarget::Address(device.ip),
        }),
    )
}

/// Offer one datagram from `source`. Each MAC reports its latest reply, and
/// anything that does not parse is ignored.
pub fn offer_scan_response(
    admission: &mut CandidateAdmission<String, DiscoveredDevice>,
    bytes: &[u8],
    source: IpAddr,
) -> Option<Admission> {
    match parse_scan_response(bytes, source) {
        Ok(device) => {
            let mac = device.mac.clone();
            Some(admission.offer_latest(source, mac, build_discovered_device(device)))
        }
        Err(error) => {
            debug!(ip = %source, error = %error, "ignored invalid Govee scan response");
            None
        }
    }
}

pub struct GoveeLanScanner {
    known_devices: Vec<GoveeKnownDevice>,
    timeout: Duration,
}

impl GoveeLanScanner {
    #[must_use]
    pub fn new(known_devices: Vec<GoveeKnownDevice>, timeout: Duration) -> Self {
        Self {
            known_devices,
            timeout,
        }
    }
}

impl GoveeLanScanner {
    /// Discover multicast-visible and configured Govee LAN devices.
    ///
    /// # Errors
    ///
    /// Returns an error when the scan socket cannot be bound.
    pub async fn scan(&mut self) -> Result<ScanOutcome<DiscoveredDevice>> {
        let socket = bind_scan_socket().await?;
        let scan = encode_command(&LanCommand::Scan)?;
        let mut gaps = Vec::new();
        if let Err(error) = socket.send_to(&scan, MULTICAST_ADDR).await {
            debug!(error = %error, "failed to send Govee multicast scan");
            gaps.push(CoverageGap::SendFailed {
                target: MULTICAST_ADDR.to_owned(),
            });
        }

        for known in &self.known_devices {
            let target = SocketAddr::new(known.ip, DEVICE_PORT);
            if let Err(error) = socket.send_to(&scan, target).await {
                debug!(ip = %known.ip, error = %error, "failed to send Govee known-IP scan");
                gaps.push(CoverageGap::SendFailed {
                    target: target.to_string(),
                });
            }
        }

        let deadline = Instant::now() + self.timeout;
        let mut buf = [0_u8; 4096];
        let mut admission = scan_admission(&self.known_devices, MAX_SCAN_RESPONSES);

        loop {
            let (len, source) = match timeout_at(deadline, socket.recv_from(&mut buf)).await {
                Err(_) => break,
                Ok(Err(error)) => {
                    debug!(error = %error, "Govee scan receive failed");
                    gaps.extend(CoverageGap::from_receive_error(&error));
                    break;
                }
                Ok(Ok(received)) => received,
            };
            offer_scan_response(&mut admission, &buf[..len], source.ip());
        }

        let mut outcome = admission.finish();
        outcome.gaps.append(&mut gaps);
        Ok(outcome)
    }
}

pub fn parse_scan_response(bytes: &[u8], source_ip: IpAddr) -> Result<GoveeLanDevice> {
    let payload: ScanEnvelope =
        serde_json::from_slice(bytes).context("failed to parse Govee scan response JSON")?;
    let data = payload
        .msg
        .data
        .context("Govee scan response missing data")?;
    let sku = data.sku.context("Govee scan response missing sku")?;
    let mac = data
        .device
        .or(data.mac)
        .context("Govee scan response missing device MAC")?;
    let ip = data
        .ip
        .and_then(|value| value.parse().ok())
        .unwrap_or(source_ip);
    let profile = profile_for_sku(&sku).unwrap_or_else(|| fallback_profile(&sku));
    let firmware_version = data.wifi_version_soft.or(data.ble_version_soft);

    Ok(GoveeLanDevice {
        ip,
        sku,
        mac: normalize_mac(&mac),
        name: profile.name.to_owned(),
        firmware_version,
    })
}

pub fn build_device_info(device: &GoveeLanDevice) -> DeviceInfo {
    let profile = profile_for_sku(&device.sku).unwrap_or_else(|| fallback_profile(&device.sku));
    let fingerprint = fingerprint_for_mac(&device.mac);
    let led_count = profile_led_count(&profile);
    let topology = topology_for_family(profile.family);
    let max_fps = if profile
        .capabilities
        .contains(GoveeCapabilities::RAZER_STREAMING)
    {
        25
    } else {
        10
    };

    DeviceInfo {
        id: fingerprint.stable_device_id(),
        name: device.name.clone(),
        vendor: "Govee".to_owned(),
        family: DeviceFamily::new_static("govee", "Govee"),
        model: Some(device.sku.clone()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("govee", "govee", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count,
            topology,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: device.firmware_version.clone(),
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: true,
            supports_brightness: profile.capabilities.contains(GoveeCapabilities::BRIGHTNESS),
            has_display: false,
            display_resolution: None,
            max_fps,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn build_discovered_device(device: GoveeLanDevice) -> DiscoveredDevice {
    let info = build_device_info(&device);
    let mut metadata = HashMap::from([
        ("ip".to_owned(), device.ip.to_string()),
        ("sku".to_owned(), device.sku.clone()),
        ("mac".to_owned(), device.mac.clone()),
    ]);
    if let Some(version) = &device.firmware_version {
        metadata.insert("firmware".to_owned(), version.clone());
    }

    DiscoveredDevice {
        fingerprint: fingerprint_for_mac(&device.mac),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        // The device answered a LAN probe with its own MAC, so the peer
        // address is real attachment evidence.
        claim: PortableIdentityClaim::mac_address(&device.mac, NetworkAttachment::Peer(device.ip)),
        metadata,
    }
}

pub(crate) fn profile_led_count(profile: &SkuProfile) -> u32 {
    u32::from(
        profile
            .razer_led_count
            .or(profile.lan_segment_count)
            .unwrap_or(1),
    )
}

pub(crate) fn topology_for_family(family: SkuFamily) -> DeviceTopologyHint {
    match family {
        SkuFamily::Bulb => DeviceTopologyHint::Point,
        SkuFamily::RgbicBar => DeviceTopologyHint::Matrix { rows: 2, cols: 1 },
        _ => DeviceTopologyHint::Strip,
    }
}

fn fingerprint_for_mac(mac: &str) -> DeviceFingerprint {
    DeviceFingerprint::mint(FingerprintNamespace::Net, "govee", &normalize_mac(mac))
}

fn normalize_mac(mac: &str) -> String {
    mac.chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase()
}

async fn bind_scan_socket() -> Result<UdpSocket> {
    match UdpSocket::bind(("0.0.0.0", LISTEN_PORT)).await {
        Ok(socket) => Ok(socket),
        Err(error) => {
            debug!(error = %error, port = LISTEN_PORT, "Govee scan listen port unavailable; falling back to ephemeral port");
            UdpSocket::bind(("0.0.0.0", 0))
                .await
                .context("failed to bind fallback Govee scan socket")
        }
    }
}

#[derive(Deserialize)]
struct ScanEnvelope {
    msg: ScanMessage,
}

#[derive(Deserialize)]
struct ScanMessage {
    data: Option<ScanData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanData {
    ip: Option<String>,
    device: Option<String>,
    mac: Option<String>,
    sku: Option<String>,
    wifi_version_soft: Option<String>,
    ble_version_soft: Option<String>,
}
