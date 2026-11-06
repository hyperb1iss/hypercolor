//! Discovery and device probing (Spec 93 §3 and §9.4).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};
use tracing::{debug, warn};

use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};

use crate::WIZ_DRIVER_ID;
use crate::protocol::{
    DEFAULT_MIN_DIMMING, DEFAULT_PORT, DISCOVERY_REQUEST, GET_MODEL_CONFIG, GET_SYSTEM_CONFIG,
    ModelConfig, ModuleClass, ProtocolError, RegistrationResult, SystemConfig, canonical_mac,
    decode_reply, error_code, reply_result,
};
use crate::transport::{RequestPolicy, TransportError, bind_for, request};

/// Metadata keys published for each device.
pub mod metadata_key {
    pub const IP: &str = "ip";
    pub const PORT: &str = "port";
    pub const MAC: &str = "mac";
    pub const MODULE_NAME: &str = "module_name";
    pub const MODULE_CLASS: &str = "module_class";
    pub const FIRMWARE: &str = "firmware";
    pub const MIN_DIMMING: &str = "min_dimming";
}

const MIN_COLLECTION_WINDOW: Duration = Duration::from_millis(300);
const MAX_COLLECTION_WINDOW: Duration = Duration::from_secs(2);
const RECEIVE_BUFFER_LEN: usize = 2048;
/// Most devices one scan admits from registration replies. WiZ homes run
/// from a handful to a few hundred bulbs; 512 matches the LIFX driver's
/// bound and caps what unsolicited replies can make the scanner probe.
pub const MAX_DISCOVERY_CANDIDATES: usize = 512;
/// Probes in flight at once, each one socket and two short requests.
pub const MAX_CONCURRENT_PROBES: usize = 32;

/// Where and how a scan looks for devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizScanOptions {
    /// Devices probed directly (configured and remembered).
    pub unicast_targets: Vec<SocketAddr>,
    /// Discovery registration targets. Empty disables broadcast entirely.
    pub broadcast_targets: Vec<SocketAddr>,
    /// Overall scan budget from the host.
    pub timeout: Duration,
    /// Per-request retry policy for probes.
    pub request_policy: RequestPolicy,
}

/// Every IPv4 broadcast address worth sending discovery to: the limited
/// broadcast address plus each up, non-loopback interface's directed
/// broadcast address.
#[must_use]
pub fn broadcast_targets() -> Vec<SocketAddr> {
    let mut targets = vec![SocketAddr::from((Ipv4Addr::BROADCAST, DEFAULT_PORT))];
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            for interface in interfaces {
                if interface.is_loopback() {
                    continue;
                }
                if let if_addrs::IfAddr::V4(v4) = interface.addr
                    && let Some(broadcast) = v4.broadcast
                {
                    let target = SocketAddr::from((broadcast, DEFAULT_PORT));
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
        }
        Err(error) => debug!(%error, "WiZ could not list interfaces for broadcast"),
    }
    targets
}

/// Everything a probe learns about one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizDeviceProfile {
    pub addr: SocketAddr,
    /// MAC address, lowercase and colon separated.
    pub mac: String,
    pub module_name: Option<String>,
    pub firmware: Option<String>,
    pub class: ModuleClass,
    pub min_dimming: u8,
}

impl WizDeviceProfile {
    /// Build a profile from the configuration replies.
    ///
    /// # Errors
    ///
    /// Returns an error when the MAC is unusable.
    pub fn from_configs(
        addr: SocketAddr,
        system: &SystemConfig,
        model: Option<&ModelConfig>,
    ) -> Result<Self> {
        let mac = canonical_mac(&system.mac).ok_or_else(|| {
            anyhow!(
                "WiZ device at {addr} reports an invalid MAC '{}'",
                system.mac
            )
        })?;
        let module_name = system
            .module_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned);
        Ok(Self {
            addr,
            mac,
            class: ModuleClass::from_module_name(module_name.as_deref()),
            module_name,
            firmware: system
                .fw_version
                .as_deref()
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .map(ToOwned::to_owned),
            min_dimming: model
                .and_then(|model| model.min_dim_level)
                .filter(|level| (1..=100).contains(level))
                .unwrap_or(DEFAULT_MIN_DIMMING),
        })
    }

    /// Stable device id minted from the MAC.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        fingerprint_for_mac(&self.mac).stable_device_id()
    }

    /// Display name. The local API carries no user-assigned name, so the
    /// name is the class plus the MAC tail.
    #[must_use]
    pub fn display_name(&self) -> String {
        let tail: String = self
            .mac
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect::<String>()
            .chars()
            .skip(6)
            .collect();
        let kind = match self.class {
            ModuleClass::Rgb => "Color",
            ModuleClass::TunableWhite => "Tunable White",
            ModuleClass::DimmableWhite => "White",
            ModuleClass::Socket => "Plug",
        };
        format!("WiZ {kind} {}", tail.to_ascii_uppercase())
    }

    /// Build the `DeviceInfo` the host tracks: one light, one LED.
    #[must_use]
    pub fn device_info(&self, max_fps: u32) -> DeviceInfo {
        DeviceInfo {
            id: self.device_id(),
            name: self.display_name(),
            vendor: "WiZ".to_owned(),
            family: DeviceFamily::new_static(WIZ_DRIVER_ID, "WiZ"),
            model: Some(
                self.module_name
                    .clone()
                    .unwrap_or_else(|| "WiZ light".to_owned()),
            ),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(WIZ_DRIVER_ID, WIZ_DRIVER_ID, ConnectionType::Network),
            segments: vec![SegmentInfo {
                name: "Light".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: self.firmware.clone(),
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

/// Fingerprint for a device MAC.
#[must_use]
pub fn fingerprint_for_mac(mac: &str) -> DeviceFingerprint {
    DeviceFingerprint::mint(FingerprintNamespace::Net, WIZ_DRIVER_ID, mac)
}

/// WiZ discovery scanner.
pub struct WizScanner {
    options: WizScanOptions,
}

impl WizScanner {
    #[must_use]
    pub const fn new(options: WizScanOptions) -> Self {
        Self { options }
    }

    /// Discover and probe lights. Smart plugs answer too and are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error when the discovery socket cannot be bound.
    pub async fn scan_profiles(&self) -> Result<Vec<WizDeviceProfile>> {
        let started = Instant::now();
        let window = (self.options.timeout / 2).clamp(MIN_COLLECTION_WINDOW, MAX_COLLECTION_WINDOW);
        let mut targets = self.options.unicast_targets.clone();
        if !self.options.broadcast_targets.is_empty() {
            targets.extend(
                collect_registrations(
                    &self.options.broadcast_targets,
                    window,
                    MAX_DISCOVERY_CANDIDATES,
                )
                .await?,
            );
        }
        targets.sort_unstable();
        targets.dedup();

        // One probe round costs every attempt's reply timeout; the probe
        // phase gets the scan budget, and at least one round.
        let round = self
            .options
            .request_policy
            .reply_timeout
            .saturating_mul(self.options.request_policy.attempts.max(1) * 2);
        let deadline = started + self.options.timeout.max(window + round);
        let policy = self.options.request_policy;
        let mut pending = targets.into_iter();
        let mut tasks = JoinSet::new();
        let spawn_next = |tasks: &mut JoinSet<_>, pending: &mut std::vec::IntoIter<SocketAddr>| {
            if let Some(addr) = pending.next() {
                tasks.spawn(async move { (addr, probe_device(addr, policy).await) });
            }
        };
        for _ in 0..MAX_CONCURRENT_PROBES {
            spawn_next(&mut tasks, &mut pending);
        }

        let mut seen = HashSet::new();
        let mut profiles: Vec<WizDeviceProfile> = Vec::new();
        loop {
            match timeout_at(deadline, tasks.join_next()).await {
                Err(_) => {
                    let unfinished = tasks.len() + pending.len();
                    warn!(
                        unfinished,
                        "WiZ scan budget ran out before every probe finished"
                    );
                    tasks.abort_all();
                    break;
                }
                Ok(None) => break,
                Ok(Some(joined)) => {
                    match joined {
                        Ok((_, Ok(profile))) if !profile.class.is_light() => {
                            debug!(mac = %profile.mac, "skipping WiZ smart plug");
                        }
                        Ok((_, Ok(profile))) => {
                            if seen.insert(profile.mac.clone()) {
                                profiles.push(profile);
                            }
                        }
                        Ok((addr, Err(error))) => {
                            warn!(%addr, error = %error, "WiZ device probe failed");
                        }
                        Err(error) => warn!(error = %error, "WiZ probe task failed"),
                    }
                    spawn_next(&mut tasks, &mut pending);
                }
            }
        }
        profiles.sort_by(|left, right| left.mac.cmp(&right.mac));
        Ok(profiles)
    }
}

/// Send the discovery registration to every target and collect the
/// addresses of devices that answer, admitting at most `limit`.
///
/// A reply counts only when it is a successful registration carrying a
/// well-formed MAC, and each MAC and each source IP is admitted once, so
/// replies from many ports or with invented MACs cannot multiply probes.
///
/// # Errors
///
/// Returns an error when the socket cannot be bound or configured.
pub async fn collect_registrations(
    targets: &[SocketAddr],
    window: Duration,
    limit: usize,
) -> Result<Vec<SocketAddr>> {
    let socket = tokio::net::UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .context("failed to bind the WiZ discovery socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable broadcast on the WiZ discovery socket")?;
    for target in targets.iter().filter(|target| target.is_ipv4()) {
        if let Err(error) = socket.send_to(DISCOVERY_REQUEST.as_bytes(), target).await {
            debug!(%target, %error, "WiZ discovery send failed");
        }
    }

    let deadline = Instant::now() + window;
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    let mut macs = HashSet::new();
    let mut sources = HashSet::new();
    let mut found = Vec::new();
    while found.len() < limit {
        match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            Err(_) => break,
            Ok(Err(error)) => {
                debug!(%error, "WiZ discovery receive failed");
                break;
            }
            Ok(Ok((len, source))) => match registration_mac(&buffer[..len]) {
                Ok(mac) => {
                    if sources.contains(&source.ip()) || !macs.insert(mac.clone()) {
                        continue;
                    }
                    sources.insert(source.ip());
                    debug!(%source, %mac, "WiZ discovery reply");
                    // Replies come from the device's listening port.
                    found.push(source);
                }
                Err(reason) => debug!(%source, %reason, "ignoring WiZ discovery reply"),
            },
        }
    }
    if found.len() >= limit {
        warn!(
            limit,
            "WiZ discovery stopped admitting devices at its limit; add the rest to device_ips"
        );
    }
    Ok(found)
}

/// The canonical MAC of a successful registration reply.
///
/// # Errors
///
/// Returns why the datagram is not one.
pub fn registration_mac(datagram: &[u8]) -> Result<String, String> {
    let reply = decode_reply(datagram).map_err(|error| error.to_string())?;
    let result: RegistrationResult =
        reply_result(&reply, "registration").map_err(|error| error.to_string())?;
    if result.success == Some(false) {
        return Err("registration was not successful".to_owned());
    }
    canonical_mac(&result.mac).ok_or_else(|| format!("invalid MAC '{}'", result.mac))
}

/// Read one device's system and model configuration.
///
/// # Errors
///
/// Returns an error when the system configuration cannot be read.
pub async fn probe_device(addr: SocketAddr, policy: RequestPolicy) -> Result<WizDeviceProfile> {
    let socket = bind_for(&addr)
        .await
        .with_context(|| format!("failed to bind a WiZ probe socket for {addr}"))?;
    let reply = request(&socket, addr, GET_SYSTEM_CONFIG, "getSystemConfig", policy).await?;
    let system: SystemConfig = reply_result(&reply, "getSystemConfig")?;
    let model = match request(&socket, addr, GET_MODEL_CONFIG, "getModelConfig", policy).await {
        Ok(reply) => match reply_result::<ModelConfig>(&reply, "getModelConfig") {
            Ok(model) => Some(model),
            Err(ProtocolError::Device { code, .. }) if code == error_code::METHOD_NOT_FOUND => None,
            Err(error) => {
                debug!(%addr, %error, "WiZ model configuration unreadable; using defaults");
                None
            }
        },
        Err(TransportError::Timeout { .. }) => {
            debug!(%addr, "WiZ device did not answer getModelConfig; using defaults");
            None
        }
        Err(error) => return Err(error.into()),
    };
    WizDeviceProfile::from_configs(addr, &system, model.as_ref())
}

/// Metadata published for a probed device.
#[must_use]
pub fn profile_metadata(profile: &WizDeviceProfile) -> HashMap<String, String> {
    let mut metadata = HashMap::from([
        (metadata_key::IP.to_owned(), profile.addr.ip().to_string()),
        (
            metadata_key::PORT.to_owned(),
            profile.addr.port().to_string(),
        ),
        (metadata_key::MAC.to_owned(), profile.mac.clone()),
        (
            metadata_key::MODULE_CLASS.to_owned(),
            profile.class.to_string(),
        ),
        (
            metadata_key::MIN_DIMMING.to_owned(),
            profile.min_dimming.to_string(),
        ),
    ]);
    if let Some(module_name) = &profile.module_name {
        metadata.insert(metadata_key::MODULE_NAME.to_owned(), module_name.clone());
    }
    if let Some(firmware) = &profile.firmware {
        metadata.insert(metadata_key::FIRMWARE.to_owned(), firmware.clone());
    }
    metadata
}

/// Generic discovery record for a probed device streamed at `max_fps`.
#[must_use]
pub fn discovered_device(profile: &WizDeviceProfile, max_fps: u32) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: fingerprint_for_mac(&profile.mac),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: profile.device_info(max_fps),
        metadata: profile_metadata(profile),
        claim: PortableIdentityClaim::mac_address(
            &profile.mac,
            NetworkAttachment::Peer(profile.addr.ip()),
        ),
    }
}

/// Address recorded in discovery metadata.
///
/// # Errors
///
/// Returns an error when the metadata lacks a parseable IP.
pub fn addr_from_metadata(metadata: &HashMap<String, String>) -> Result<SocketAddr> {
    let ip = metadata
        .get(metadata_key::IP)
        .ok_or_else(|| anyhow!("WiZ metadata is missing the device IP"))?
        .parse::<IpAddr>()
        .context("WiZ metadata carries an invalid device IP")?;
    let port = metadata
        .get(metadata_key::PORT)
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT);
    Ok(SocketAddr::new(ip, port))
}
