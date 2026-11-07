//! Discovery by the SSDP-style search (Spec 94 §3 and §8.4).
//!
//! The search reply carries everything the driver needs (control address,
//! id, model, supported methods), so a scan never opens a control
//! connection: each device allows only four.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::debug;

use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_driver_support::discovery::{
    Admission, CandidateAdmission, KnownTarget, ScanOutcome,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::portable::PortableIdentityClaim;

use crate::YEELIGHT_DRIVER_ID;
use crate::protocol::{
    CONTROL_PORT, Capabilities, SEARCH_MULTICAST, SEARCH_PORT, SEARCH_REQUEST, SearchReply,
    decode_search_reply,
};

/// Metadata keys published for each device.
pub mod metadata_key {
    pub const IP: &str = "ip";
    pub const PORT: &str = "port";
    pub const SEARCH_PORT: &str = "search_port";
    pub const ID: &str = "yeelight_id";
    pub const MODEL: &str = "model";
    pub const FIRMWARE: &str = "firmware";
    pub const SUPPORT: &str = "support";
    pub const CLASS: &str = "light_class";
    pub const MUSIC_MODE: &str = "music_mode";
}

const MIN_COLLECTION_WINDOW: Duration = Duration::from_millis(500);
const MAX_COLLECTION_WINDOW: Duration = Duration::from_secs(3);
const RECEIVE_BUFFER_LEN: usize = 2048;
/// Most lights one search admits. Yeelight homes run from a handful to a
/// few dozen lights; 256 leaves headroom while capping how many invented
/// identities one scan can hand the device registry.
pub const MAX_SEARCH_REPLIES: usize = 256;
/// Searches are UDP; repeat a few times inside the window.
const SEARCH_ROUNDS: u32 = 3;

/// Where a scan sends the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YeelightScanOptions {
    /// Search targets probed directly (configured and remembered devices,
    /// on port 1982).
    pub unicast_targets: Vec<SocketAddr>,
    /// Also send the search to the multicast group.
    pub multicast: bool,
    /// Overall scan budget from the host.
    pub timeout: Duration,
}

/// Everything a search reply says about one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YeelightDeviceProfile {
    pub reply: SearchReply,
    /// Port the device answered the search from.
    pub search_port: u16,
}

impl YeelightDeviceProfile {
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::from_support(&self.reply.support)
    }

    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        fingerprint_for_id(&self.reply.id).stable_device_id()
    }

    /// The name stored on the device, else the model and id tail.
    #[must_use]
    pub fn display_name(&self) -> String {
        if let Some(name) = self
            .reply
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
        // Character-wise, so no byte index ever lands inside a character.
        let digits: Vec<char> = self.reply.id.trim_start_matches("0x").chars().collect();
        let tail: String = digits[digits.len().saturating_sub(6)..].iter().collect();
        format!(
            "Yeelight {} {}",
            self.reply.model.as_deref().unwrap_or("light"),
            tail.to_ascii_uppercase()
        )
    }

    /// Build the `DeviceInfo` the host tracks: one light, one LED.
    #[must_use]
    pub fn device_info(&self, max_fps: u32) -> DeviceInfo {
        DeviceInfo {
            id: self.device_id(),
            name: self.display_name(),
            vendor: "Yeelight".to_owned(),
            family: DeviceFamily::new_static(YEELIGHT_DRIVER_ID, "Yeelight"),
            model: self.reply.model.clone(),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(
                YEELIGHT_DRIVER_ID,
                YEELIGHT_DRIVER_ID,
                ConnectionType::Network,
            ),
            segments: vec![SegmentInfo {
                name: "Light".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: self.reply.fw_ver.clone(),
            capabilities: DeviceCapabilities {
                led_count: 1,
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
}

/// Fingerprint for a Yeelight device id.
#[must_use]
pub fn fingerprint_for_id(id: &str) -> DeviceFingerprint {
    DeviceFingerprint::mint(FingerprintNamespace::Net, YEELIGHT_DRIVER_ID, id)
}

/// Yeelight discovery scanner.
pub struct YeelightScanner {
    options: YeelightScanOptions,
}

impl YeelightScanner {
    #[must_use]
    pub const fn new(options: YeelightScanOptions) -> Self {
        Self { options }
    }

    /// Search for devices and keep the ones Hypercolor can drive.
    ///
    /// # Errors
    ///
    /// Returns an error when the search socket cannot be bound.
    pub async fn scan(&self) -> Result<ScanOutcome<YeelightDeviceProfile>> {
        let mut targets: Vec<SocketAddr> = self
            .options
            .unicast_targets
            .iter()
            .copied()
            .filter(SocketAddr::is_ipv4)
            .collect();
        if self.options.multicast {
            targets.push(SEARCH_MULTICAST);
        }
        if targets.is_empty() {
            return Ok(ScanOutcome::default());
        }
        let window = (self.options.timeout / 2).clamp(MIN_COLLECTION_WINDOW, MAX_COLLECTION_WINDOW);
        let replies = search(&targets, window).await?;
        let mut profiles: Vec<YeelightDeviceProfile> = Vec::new();
        for profile in replies.found {
            if !profile.capabilities().drivable() {
                debug!(id = %profile.reply.id, "skipping Yeelight device without a drivable light");
                continue;
            }
            if !profiles
                .iter()
                .any(|known| known.reply.id == profile.reply.id)
            {
                profiles.push(profile);
            }
        }
        profiles.sort_by(|left, right| left.reply.id.cmp(&right.reply.id));
        Ok(ScanOutcome {
            found: profiles,
            ..replies
        })
    }
}

/// Send the search to every target a few times and collect the replies
/// that arrive within `window`.
///
/// # Errors
///
/// Returns an error when the socket cannot be bound.
pub async fn search(
    targets: &[SocketAddr],
    window: Duration,
) -> Result<ScanOutcome<YeelightDeviceProfile>> {
    search_until(targets, window, None).await
}

/// [`search`], returning early once `limit` distinct devices answered.
///
/// # Errors
///
/// Returns an error when the socket cannot be bound.
pub async fn search_until(
    targets: &[SocketAddr],
    window: Duration,
    limit: Option<usize>,
) -> Result<ScanOutcome<YeelightDeviceProfile>> {
    let limit = limit.unwrap_or(MAX_SEARCH_REPLIES).min(MAX_SEARCH_REPLIES);
    let mut admission = CandidateAdmission::new(
        limit,
        targets
            .iter()
            .map(|target| KnownTarget::Address(target.ip())),
    );
    let mut gaps = Vec::new();
    // A target stays unreached until one round's send to it succeeds.
    let mut unreached = targets.to_vec();
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .context("failed to bind the Yeelight search socket")?;
    let deadline = Instant::now() + window;
    let round_gap = window / (SEARCH_ROUNDS + 1);
    let mut next_round = Instant::now();
    let mut rounds = 0;
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    loop {
        if rounds < SEARCH_ROUNDS && Instant::now() >= next_round {
            for target in targets {
                match socket.send_to(SEARCH_REQUEST.as_bytes(), target).await {
                    Ok(_) => unreached.retain(|other| other != target),
                    Err(error) => debug!(%target, %error, "Yeelight search send failed"),
                }
            }
            rounds += 1;
            next_round = Instant::now() + round_gap;
        }
        let wake = if rounds < SEARCH_ROUNDS {
            next_round.min(deadline)
        } else {
            deadline
        };
        match timeout_at(wake, socket.recv_from(&mut buffer)).await {
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
            Ok(Err(error)) => {
                debug!(%error, "Yeelight search receive failed");
                gaps.extend(CoverageGap::from_receive_error(&error));
                break;
            }
            Ok(Ok((len, source))) => match decode_search_reply(&buffer[..len]) {
                Ok(reply) => {
                    let offered = offer_reply(&mut admission, source, reply);
                    // A targeted lookup (the identity check) is done.
                    if offered == Some(Admission::Added)
                        && limit < MAX_SEARCH_REPLIES
                        && admission.len() >= limit
                    {
                        break;
                    }
                }
                Err(error) => debug!(%source, %error, "ignoring non-Yeelight search reply"),
            },
        }
    }
    gaps.extend(unreached.iter().map(|target| CoverageGap::SendFailed {
        target: target.to_string(),
    }));
    let mut outcome = admission.finish();
    outcome.gaps.append(&mut gaps);
    Ok(outcome)
}

/// Offer one search reply from `source` (Spec 94 §3.3).
///
/// Every check runs before any state changes. A light answers for itself, so
/// a reply naming another control address is not admitted, and neither is an
/// id another address already reported. Each address keeps only its most
/// recent reply: one endpoint cannot hand a scan more than one identity.
pub fn offer_reply(
    admission: &mut CandidateAdmission<IpAddr, YeelightDeviceProfile>,
    source: SocketAddr,
    reply: SearchReply,
) -> Option<Admission> {
    let ip = source.ip();
    if reply.location.ip() != ip {
        debug!(%source, "ignoring a Yeelight reply naming another address");
        return None;
    }
    if admission
        .values()
        .any(|known| known.reply.id == reply.id && known.reply.location.ip() != ip)
    {
        debug!(%source, "ignoring a Yeelight id another address reported");
        return None;
    }
    let profile = YeelightDeviceProfile {
        reply,
        search_port: source.port(),
    };
    Some(admission.offer_latest(ip, ip, profile))
}

/// Metadata published for a device.
#[must_use]
pub fn profile_metadata(
    profile: &YeelightDeviceProfile,
    music_mode: bool,
) -> HashMap<String, String> {
    let capabilities = profile.capabilities();
    let mut metadata = HashMap::from([
        (
            metadata_key::IP.to_owned(),
            profile.reply.location.ip().to_string(),
        ),
        (
            metadata_key::PORT.to_owned(),
            profile.reply.location.port().to_string(),
        ),
        (
            metadata_key::SEARCH_PORT.to_owned(),
            profile.search_port.to_string(),
        ),
        (metadata_key::ID.to_owned(), profile.reply.id.clone()),
        (
            metadata_key::SUPPORT.to_owned(),
            profile.reply.support.join(" "),
        ),
        (
            metadata_key::CLASS.to_owned(),
            capabilities.class_name().to_owned(),
        ),
        (
            metadata_key::MUSIC_MODE.to_owned(),
            if music_mode && capabilities.music {
                "on"
            } else if capabilities.music {
                "off"
            } else {
                "unsupported"
            }
            .to_owned(),
        ),
    ]);
    if let Some(model) = &profile.reply.model {
        metadata.insert(metadata_key::MODEL.to_owned(), model.clone());
    }
    if let Some(firmware) = &profile.reply.fw_ver {
        metadata.insert(metadata_key::FIRMWARE.to_owned(), firmware.clone());
    }
    metadata
}

/// Generic discovery record for a device streamed at `max_fps`.
#[must_use]
pub fn discovered_device(
    profile: &YeelightDeviceProfile,
    max_fps: u32,
    music_mode: bool,
) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: fingerprint_for_id(&profile.reply.id),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: profile.device_info(max_fps),
        metadata: profile_metadata(profile, music_mode),
        claim: PortableIdentityClaim::driver_identifier(
            YEELIGHT_DRIVER_ID,
            &profile.reply.id,
            profile.reply.location.ip(),
        ),
    }
}

/// What the backend needs to reach a device, from discovery metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedRoute {
    pub control: SocketAddr,
    pub search: SocketAddr,
    pub id: String,
    pub support: Vec<String>,
}

/// Route recorded in discovery metadata.
///
/// # Errors
///
/// Returns an error when the metadata lacks a parseable IP or the id.
pub fn route_from_metadata(metadata: &HashMap<String, String>) -> Result<AdoptedRoute> {
    let ip = metadata
        .get(metadata_key::IP)
        .ok_or_else(|| anyhow!("Yeelight metadata is missing the device IP"))?
        .parse::<IpAddr>()
        .context("Yeelight metadata carries an invalid device IP")?;
    let port = |key: &str, default: u16| {
        metadata
            .get(key)
            .and_then(|raw| raw.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or(default)
    };
    let id = metadata
        .get(metadata_key::ID)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("Yeelight metadata is missing the device id"))?
        .clone();
    Ok(AdoptedRoute {
        control: SocketAddr::new(ip, port(metadata_key::PORT, CONTROL_PORT)),
        search: SocketAddr::new(ip, port(metadata_key::SEARCH_PORT, SEARCH_PORT)),
        id,
        support: metadata
            .get(metadata_key::SUPPORT)
            .map(|support| support.split_whitespace().map(ToOwned::to_owned).collect())
            .unwrap_or_default(),
    })
}
