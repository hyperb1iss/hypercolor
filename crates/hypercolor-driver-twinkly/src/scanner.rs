//! Discovery and device probing (Spec 92 §3 and §10.4).
//!
//! A scan sends the discovery datagram to each broadcast target, collects
//! the replies, adds the configured and previously seen addresses, and
//! reads every device's gestalt and firmware version over REST. None of
//! this logs in: a login would sign out whatever session the device holds,
//! including an active Hypercolor stream, so authentication waits for
//! connect.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};
use tracing::{debug, warn};

use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorSpace, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};

use crate::TWINKLY_DRIVER_ID;
use crate::api::{DEFAULT_REQUEST_TIMEOUT, Gestalt, TwinklyClient, TwinklyEndpoint};
use crate::layout::strip_segment;
use crate::protocol::{
    DISCOVERY_PORT, DISCOVERY_REQUEST, FirmwareVersion, HTTP_PORT, LedProfile, RT_PORT, RtProtocol,
    decode_discovery_reply,
};

/// Metadata keys published for each device.
pub mod metadata_key {
    pub const IP: &str = "ip";
    pub const HTTP_PORT: &str = "http_port";
    pub const RT_PORT: &str = "rt_port";
    pub const MAC: &str = "mac";
    pub const PRODUCT_CODE: &str = "product_code";
    pub const HARDWARE_VERSION: &str = "hardware_version";
    pub const FW_FAMILY: &str = "fw_family";
    pub const FIRMWARE: &str = "firmware";
    pub const LED_PROFILE: &str = "led_profile";
    pub const RT_PROTOCOL: &str = "rt_protocol";
    pub const LAYOUT_SOURCE: &str = "layout_source";
    pub const GESTALT_FRAME_RATE: &str = "gestalt_frame_rate";
}

/// Bounds for how long a scan listens for discovery replies. Devices answer
/// within milliseconds; the rest of the host's budget is left for probing.
const MIN_COLLECTION_WINDOW: Duration = Duration::from_millis(300);
const MAX_COLLECTION_WINDOW: Duration = Duration::from_secs(2);
const RECEIVE_BUFFER_LEN: usize = 512;

/// Where and how a scan looks for devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwinklyScanOptions {
    /// Devices probed over REST directly (configured and remembered).
    pub endpoints: Vec<TwinklyEndpoint>,
    /// Discovery datagram targets. Empty disables broadcast entirely.
    pub broadcast_targets: Vec<SocketAddr>,
    /// Overall scan budget from the host.
    pub timeout: Duration,
    /// REST port assumed for devices found by the discovery datagram.
    pub http_port: u16,
    /// Real-time port assumed for devices found by the discovery datagram.
    pub rt_port: u16,
}

impl TwinklyScanOptions {
    /// Options on the standard ports.
    #[must_use]
    pub const fn new(
        endpoints: Vec<TwinklyEndpoint>,
        broadcast_targets: Vec<SocketAddr>,
        timeout: Duration,
    ) -> Self {
        Self {
            endpoints,
            broadcast_targets,
            timeout,
            http_port: HTTP_PORT,
            rt_port: RT_PORT,
        }
    }
}

/// Every IPv4 broadcast address worth sending discovery to: the limited
/// broadcast address plus each up, non-loopback interface's directed
/// broadcast address.
#[must_use]
pub fn broadcast_targets() -> Vec<SocketAddr> {
    let mut targets = vec![SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))];
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            for interface in interfaces {
                if interface.is_loopback() {
                    continue;
                }
                if let if_addrs::IfAddr::V4(v4) = interface.addr
                    && let Some(broadcast) = v4.broadcast
                {
                    let target = SocketAddr::from((broadcast, DISCOVERY_PORT));
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
        }
        Err(error) => debug!(%error, "Twinkly could not list interfaces for broadcast"),
    }
    targets
}

/// Everything a scan learns about one device without logging in.
#[derive(Debug, Clone, PartialEq)]
pub struct TwinklyDeviceProfile {
    pub endpoint: TwinklyEndpoint,
    /// MAC address, lowercase and colon separated.
    pub mac: String,
    pub device_name: Option<String>,
    pub product_name: Option<String>,
    pub product_code: Option<String>,
    pub hardware_version: Option<String>,
    pub fw_family: Option<String>,
    pub firmware: FirmwareVersion,
    pub led_count: u32,
    pub led_profile: LedProfile,
    pub frame_rate: Option<f64>,
}

impl TwinklyDeviceProfile {
    /// Build a profile from a gestalt and firmware version string.
    ///
    /// # Errors
    ///
    /// Returns an error when the MAC, firmware, or LED profile is unusable.
    pub fn from_gestalt(
        endpoint: TwinklyEndpoint,
        gestalt: &Gestalt,
        firmware: &str,
    ) -> Result<Self> {
        let mac = canonical_mac(&gestalt.mac)
            .ok_or_else(|| anyhow!("Twinkly gestalt carries an invalid MAC '{}'", gestalt.mac))?;
        let firmware = firmware
            .parse::<FirmwareVersion>()
            .map_err(|error| anyhow!("{error}"))?;
        let led_profile = match (gestalt.led_profile.as_deref(), gestalt.bytes_per_led) {
            (Some(raw), _) => LedProfile::parse(raw).map_err(|error| anyhow!("{error}"))?,
            (None, Some(4)) => LedProfile::Rgbw,
            (None, _) => LedProfile::Rgb,
        };
        Ok(Self {
            endpoint,
            mac,
            device_name: non_empty(gestalt.device_name.as_deref()),
            product_name: non_empty(gestalt.product_name.as_deref()),
            product_code: non_empty(gestalt.product_code.as_deref()),
            hardware_version: non_empty(gestalt.hardware_version.as_deref()),
            fw_family: non_empty(gestalt.fw_family.as_deref()),
            firmware,
            led_count: gestalt.number_of_led,
            led_profile,
            frame_rate: gestalt.frame_rate,
        })
    }

    /// Real-time datagram format this device takes (Spec 92 §6.4).
    #[must_use]
    pub fn rt_protocol(&self) -> RtProtocol {
        RtProtocol::for_device(self.fw_family.as_deref(), self.firmware)
    }

    /// Stable device id minted from the MAC.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        fingerprint_for_mac(&self.mac).stable_device_id()
    }

    /// Display name: the device name, else the product code, else the MAC.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.device_name.clone().unwrap_or_else(|| {
            let suffix: String = self.mac.chars().filter(char::is_ascii_hexdigit).collect();
            let tail = &suffix[suffix.len().saturating_sub(6)..];
            format!("Twinkly {}", tail.to_ascii_uppercase())
        })
    }

    /// Model name: the product code with the product name as fallback.
    #[must_use]
    pub fn model_name(&self) -> String {
        self.product_code
            .clone()
            .or_else(|| self.product_name.clone())
            .unwrap_or_else(|| "Twinkly".to_owned())
    }

    /// Build the `DeviceInfo` the host tracks, with explicit segments.
    #[must_use]
    pub fn device_info_with_segments(
        &self,
        segments: Vec<SegmentInfo>,
        max_fps: u32,
    ) -> DeviceInfo {
        let led_count = segments.iter().map(|segment| segment.led_count).sum();
        DeviceInfo {
            id: self.device_id(),
            name: self.display_name(),
            vendor: "Twinkly".to_owned(),
            family: DeviceFamily::new_static(TWINKLY_DRIVER_ID, "Twinkly"),
            model: Some(self.model_name()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(
                TWINKLY_DRIVER_ID,
                TWINKLY_DRIVER_ID,
                ConnectionType::Network,
            ),
            segments,
            firmware_version: Some(self.firmware.to_string()),
            capabilities: DeviceCapabilities {
                led_count,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps,
                color_space: DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        }
    }

    /// `DeviceInfo` before the layout is known: one strip in wiring order.
    #[must_use]
    pub fn device_info(&self, max_fps: u32) -> DeviceInfo {
        self.device_info_with_segments(vec![strip_segment(self.led_count)], max_fps)
    }
}

/// Fingerprint for a device MAC.
#[must_use]
pub fn fingerprint_for_mac(mac: &str) -> DeviceFingerprint {
    DeviceFingerprint::mint(FingerprintNamespace::Net, TWINKLY_DRIVER_ID, mac)
}

/// Lowercase, colon-separated MAC, or `None` when the text is not six
/// hexadecimal octets.
#[must_use]
pub fn canonical_mac(raw: &str) -> Option<String> {
    let hex: String = raw
        .chars()
        .filter(|character| !matches!(character, ':' | '-' | '.'))
        .collect();
    if hex.len() != 12 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    let octets: Vec<&str> = (0..6)
        .map(|index| &lower[index * 2..index * 2 + 2])
        .collect();
    Some(octets.join(":"))
}

fn non_empty(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Twinkly discovery scanner.
pub struct TwinklyScanner {
    options: TwinklyScanOptions,
}

impl TwinklyScanner {
    #[must_use]
    pub const fn new(options: TwinklyScanOptions) -> Self {
        Self { options }
    }

    /// Discover and probe devices.
    ///
    /// # Errors
    ///
    /// Returns an error when the discovery socket cannot be bound.
    pub async fn scan_profiles(&self) -> Result<Vec<TwinklyDeviceProfile>> {
        let mut endpoints = self.options.endpoints.clone();
        if !self.options.broadcast_targets.is_empty() {
            let window =
                (self.options.timeout / 2).clamp(MIN_COLLECTION_WINDOW, MAX_COLLECTION_WINDOW);
            for ip in collect_discovery_replies(&self.options.broadcast_targets, window).await? {
                endpoints.push(TwinklyEndpoint {
                    ip,
                    http_port: self.options.http_port,
                    rt_port: self.options.rt_port,
                });
            }
        }
        endpoints.sort_unstable();
        endpoints.dedup_by_key(|endpoint| endpoint.http_addr());

        let request_timeout =
            (self.options.timeout / 2).clamp(Duration::from_millis(250), DEFAULT_REQUEST_TIMEOUT);
        let mut tasks = JoinSet::new();
        for endpoint in endpoints {
            tasks.spawn(async move { (endpoint, probe_endpoint(endpoint, request_timeout).await) });
        }

        let mut profiles: Vec<TwinklyDeviceProfile> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((_, Ok(profile))) => {
                    if !profiles.iter().any(|known| known.mac == profile.mac) {
                        profiles.push(profile);
                    }
                }
                Ok((endpoint, Err(error))) => {
                    warn!(%endpoint, error = %error, "Twinkly device probe failed");
                }
                Err(error) => warn!(error = %error, "Twinkly probe task failed"),
            }
        }
        profiles.sort_by(|left, right| left.mac.cmp(&right.mac));
        Ok(profiles)
    }
}

/// Send the discovery datagram to every target and collect the addresses
/// of devices that answer within `window`.
///
/// # Errors
///
/// Returns an error when the socket cannot be bound or configured.
pub async fn collect_discovery_replies(
    targets: &[SocketAddr],
    window: Duration,
) -> Result<Vec<IpAddr>> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .context("failed to bind the Twinkly discovery socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable broadcast on the Twinkly discovery socket")?;
    for target in targets.iter().filter(|target| target.is_ipv4()) {
        if let Err(error) = socket.send_to(&DISCOVERY_REQUEST, target).await {
            debug!(%target, %error, "Twinkly discovery send failed");
        }
    }

    let deadline = Instant::now() + window;
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    let mut found = Vec::new();
    loop {
        match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            Err(_) => break,
            Ok(Err(error)) => {
                debug!(%error, "Twinkly discovery receive failed");
                break;
            }
            Ok(Ok((len, source))) => match decode_discovery_reply(&buffer[..len]) {
                Ok(reply) => {
                    debug!(%source, device_id = %reply.device_id, "Twinkly discovery reply");
                    let ip = source.ip();
                    if !found.contains(&ip) {
                        found.push(ip);
                    }
                }
                Err(error) => debug!(%source, %error, "ignoring non-Twinkly discovery reply"),
            },
        }
    }
    Ok(found)
}

/// Read one device's gestalt and firmware version.
///
/// # Errors
///
/// Returns an error when either request fails or the reply is unusable.
pub async fn probe_endpoint(
    endpoint: TwinklyEndpoint,
    request_timeout: Duration,
) -> Result<TwinklyDeviceProfile> {
    let client = TwinklyClient::new(endpoint, request_timeout)?;
    let gestalt = client.gestalt().await?;
    let firmware = client.firmware_version().await?;
    TwinklyDeviceProfile::from_gestalt(endpoint, &gestalt, &firmware.version)
}

/// Metadata published for a probed device.
#[must_use]
pub fn profile_metadata(profile: &TwinklyDeviceProfile) -> HashMap<String, String> {
    let mut metadata = HashMap::from([
        (metadata_key::IP.to_owned(), profile.endpoint.ip.to_string()),
        (
            metadata_key::HTTP_PORT.to_owned(),
            profile.endpoint.http_port.to_string(),
        ),
        (
            metadata_key::RT_PORT.to_owned(),
            profile.endpoint.rt_port.to_string(),
        ),
        (metadata_key::MAC.to_owned(), profile.mac.clone()),
        (
            metadata_key::FIRMWARE.to_owned(),
            profile.firmware.to_string(),
        ),
        (
            metadata_key::LED_PROFILE.to_owned(),
            profile.led_profile.to_string(),
        ),
        (
            metadata_key::RT_PROTOCOL.to_owned(),
            profile.rt_protocol().to_string(),
        ),
    ]);
    let optional = [
        (metadata_key::PRODUCT_CODE, &profile.product_code),
        (metadata_key::HARDWARE_VERSION, &profile.hardware_version),
        (metadata_key::FW_FAMILY, &profile.fw_family),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            metadata.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(rate) = profile.frame_rate.filter(|rate| rate.is_finite()) {
        metadata.insert(
            metadata_key::GESTALT_FRAME_RATE.to_owned(),
            rate.to_string(),
        );
    }
    metadata
}

/// Generic discovery record for a probed device streamed at `max_fps`.
#[must_use]
pub fn discovered_device(profile: &TwinklyDeviceProfile, max_fps: u32) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: fingerprint_for_mac(&profile.mac),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: profile.device_info(max_fps),
        metadata: profile_metadata(profile),
        claim: PortableIdentityClaim::mac_address(
            &profile.mac,
            NetworkAttachment::Peer(profile.endpoint.ip),
        ),
    }
}

/// Endpoint recorded in discovery metadata.
///
/// # Errors
///
/// Returns an error when the metadata lacks a parseable IP.
pub fn endpoint_from_metadata(metadata: &HashMap<String, String>) -> Result<TwinklyEndpoint> {
    let ip = metadata
        .get(metadata_key::IP)
        .ok_or_else(|| anyhow!("Twinkly metadata is missing the device IP"))?
        .parse::<IpAddr>()
        .context("Twinkly metadata carries an invalid device IP")?;
    let port = |key: &str, default: u16| {
        metadata
            .get(key)
            .and_then(|raw| raw.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or(default)
    };
    Ok(TwinklyEndpoint {
        ip,
        http_port: port(metadata_key::HTTP_PORT, HTTP_PORT),
        rt_port: port(metadata_key::RT_PORT, RT_PORT),
    })
}
