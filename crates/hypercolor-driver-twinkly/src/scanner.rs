//! Discovery and device probing (Spec 92 §3 and §10.4).
//!
//! A scan sends the discovery datagram to each broadcast target, collects
//! the replies, adds the configured and previously seen addresses, and
//! reads every device's gestalt and firmware version over REST. None of
//! this logs in: a login would sign out whatever session the device holds,
//! including an active Hypercolor stream, so authentication waits for
//! connect.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::debug;

use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_driver_support::discovery::{
    CandidateAdmission, KnownTarget, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
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
    decode_discovery_reply, validate_led_count,
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
/// Most devices one scan admits from discovery replies. A large holiday
/// display runs a few dozen controllers, so 128 leaves headroom while
/// capping how much probing unsolicited replies can cause.
pub const MAX_DISCOVERY_CANDIDATES: usize = 128;
/// Probes in flight at once. Each probe is two short unauthenticated HTTP
/// requests on its own client; 16 at a time finishes a full candidate list
/// in eight rounds.
pub const MAX_CONCURRENT_PROBES: NonZeroUsize = NonZeroUsize::new(16).expect("16 is not zero");

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
        let fw_family = non_empty(gestalt.fw_family.as_deref());
        let led_count = usize::try_from(gestalt.number_of_led).unwrap_or(usize::MAX);
        validate_led_count(
            RtProtocol::for_device(fw_family.as_deref(), firmware),
            led_profile,
            led_count,
        )
        .map_err(|error| anyhow!("{error}"))?;
        Ok(Self {
            endpoint,
            mac,
            device_name: non_empty(gestalt.device_name.as_deref()),
            product_name: non_empty(gestalt.product_name.as_deref()),
            product_code: non_empty(gestalt.product_code.as_deref()),
            hardware_version: non_empty(gestalt.hardware_version.as_deref()),
            fw_family,
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
    /// A configured or remembered endpoint that fails its probe is simply
    /// gone. One that answered the discovery datagram is alive, so its
    /// failed probe leaves a coverage gap.
    ///
    /// # Errors
    ///
    /// Returns an error when the discovery socket cannot be bound.
    pub async fn scan(&self) -> Result<ScanOutcome<TwinklyDeviceProfile>> {
        let started = Instant::now();
        let window = (self.options.timeout / 2).clamp(MIN_COLLECTION_WINDOW, MAX_COLLECTION_WINDOW);
        let request_timeout =
            (self.options.timeout / 2).clamp(Duration::from_millis(250), DEFAULT_REQUEST_TIMEOUT);

        // Known endpoints stay first, so the deadline falls on unsolicited
        // ones.
        let mut endpoints = self.options.endpoints.clone();
        endpoints.sort_unstable();
        endpoints.dedup_by_key(|endpoint| endpoint.http_addr());

        let mut gaps = Vec::new();
        let mut answered = HashSet::new();
        if !self.options.broadcast_targets.is_empty() {
            let admission = CandidateAdmission::new(
                MAX_DISCOVERY_CANDIDATES,
                endpoints
                    .iter()
                    .map(|endpoint| KnownTarget::Address(endpoint.ip)),
            );
            let replies =
                collect_discovery_replies(&self.options.broadcast_targets, window, admission)
                    .await?;
            gaps = replies.gaps;
            let known: HashSet<SocketAddr> =
                endpoints.iter().map(TwinklyEndpoint::http_addr).collect();
            for ip in replies.found {
                answered.insert(ip);
                let endpoint = TwinklyEndpoint {
                    ip,
                    http_port: self.options.http_port,
                    rt_port: self.options.rt_port,
                };
                if !known.contains(&endpoint.http_addr()) {
                    endpoints.push(endpoint);
                }
            }
        }

        // Probes share the scan budget; at least one full probe round runs
        // even when the host's timeout is shorter than that.
        let budget = ProbeBudget {
            deadline: started + self.options.timeout.max(window + request_timeout * 2),
            in_flight: MAX_CONCURRENT_PROBES,
        };
        let answered = Arc::new(answered);
        let mut outcome = probe_candidates(endpoints, budget, |endpoint: TwinklyEndpoint| {
            let answered = Arc::clone(&answered);
            async move {
                match probe_endpoint(endpoint, request_timeout).await {
                    Ok(profile) => Ok(Probed::Described(profile)),
                    Err(error) if answered.contains(&endpoint.ip) => Err(error),
                    Err(error) => {
                        debug!(%endpoint, error = %format!("{error:#}"), "Twinkly device probe failed");
                        Ok(Probed::Nothing)
                    }
                }
            }
        })
        .await;

        outcome.gaps.extend(gaps);
        outcome
            .found
            .sort_by(|left, right| left.mac.cmp(&right.mac));
        outcome.found.dedup_by(|left, right| left.mac == right.mac);
        Ok(outcome)
    }
}

/// Send the discovery datagram to every target and collect the addresses
/// of devices that answer within `window`, as far as `admission` lets them.
///
/// # Errors
///
/// Returns an error when the socket cannot be bound or configured.
pub async fn collect_discovery_replies(
    targets: &[SocketAddr],
    window: Duration,
    mut admission: CandidateAdmission<IpAddr, IpAddr>,
) -> Result<ScanOutcome<IpAddr>> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .context("failed to bind the Twinkly discovery socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable broadcast on the Twinkly discovery socket")?;
    let targets: Vec<&SocketAddr> = targets.iter().filter(|target| target.is_ipv4()).collect();
    let mut failed_sends = Vec::new();
    for target in &targets {
        if let Err(error) = socket.send_to(&DISCOVERY_REQUEST, target).await {
            debug!(%target, %error, "Twinkly discovery send failed");
            failed_sends.push(CoverageGap::SendFailed {
                target: target.to_string(),
            });
        }
    }
    let mut gaps = Vec::new();
    // One failed interface is tolerated: known devices are probed directly.
    if failed_sends.len() == targets.len() {
        gaps.append(&mut failed_sends);
    }

    let deadline = Instant::now() + window;
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    loop {
        match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            Err(_) => break,
            Ok(Err(error)) => {
                debug!(%error, "Twinkly discovery receive failed");
                gaps.extend(CoverageGap::from_receive_error(&error));
                break;
            }
            Ok(Ok((len, source))) => match decode_discovery_reply(&buffer[..len]) {
                Ok(reply) => {
                    debug!(%source, device_id = %reply.device_id, "Twinkly discovery reply");
                    admission.offer(source.ip(), source.ip(), source.ip());
                }
                Err(error) => debug!(%source, %error, "ignoring non-Twinkly discovery reply"),
            },
        }
    }
    let mut replies = admission.finish();
    replies.gaps.append(&mut gaps);
    Ok(replies)
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
