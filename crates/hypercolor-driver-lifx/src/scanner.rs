//! Discovery and device probing (Spec 88 §4 and §11.4).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::time::Instant;
use tracing::debug;

use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_driver_support::discovery::{
    CandidateAdmission, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
use hypercolor_types::device::DeviceFingerprint;
use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};

use crate::encoder::OutputPlan;
use crate::products::{LIFX_VENDOR_ID, ProductKind, lookup_product};
use crate::protocol::{DEFAULT_PORT, Message, Serial, State, message_type};
use crate::topology::{LifxDeviceProfile, LifxTopology, TileGeometry, fingerprint_for_serial};
use crate::transport::{DiscoveryRound, LifxClient, RequestPolicy, ServiceReply};

/// Metadata keys published for each device.
pub mod metadata_key {
    pub const IP: &str = "ip";
    pub const PORT: &str = "port";
    pub const SERIAL: &str = "serial";
    pub const VENDOR_ID: &str = "vendor_id";
    pub const PRODUCT_ID: &str = "product_id";
    pub const FIRMWARE: &str = "firmware";
    pub const DEVICE_CLASS: &str = "device_class";
    pub const ZONES: &str = "zones";
    pub const TILES: &str = "tiles";
}

/// Bounds for how long a scan listens for StateService replies. Lights
/// answer within milliseconds, so the rest of the host's timeout is left
/// for probing.
const MIN_COLLECTION_WINDOW: Duration = Duration::from_millis(300);
const MAX_COLLECTION_WINDOW: Duration = Duration::from_secs(2);

/// Distinct serials one scan will probe. A large LIFX home runs dozens to a
/// couple of hundred lights, so 512 leaves headroom while bounding the work
/// a burst of forged StateService replies can cause. A scan that sees more
/// reports partial coverage, so lights it never probed stay tracked.
pub const MAX_DISCOVERY_CANDIDATES: usize = 512;

/// Probes in flight at once, each holding one UDP socket. A healthy probe
/// is four round trips, so 32 in flight clears 512 lights in well under a
/// second while keeping descriptor and port use small.
pub const MAX_CONCURRENT_PROBES: NonZeroUsize = NonZeroUsize::new(32).expect("32 is not zero");

/// Requests in a healthy probe: version, firmware, label, and one topology
/// read.
const PROBE_REQUESTS: u32 = 4;
/// Longest probe round the scan waits for, whatever retry policy a caller
/// passes.
const MAX_PROBE_ROUND: Duration = Duration::from_secs(10);

/// Where a tracked light last proved itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedRoute {
    /// The light's last verified endpoint.
    pub addr: SocketAddr,
    /// The fingerprint the host tracks it under.
    pub fingerprint: DeviceFingerprint,
}

/// Where and how a scan looks for devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifxScanOptions {
    /// Unicast GetService targets (configured and remembered devices).
    pub unicast_targets: Vec<SocketAddr>,
    /// Broadcast GetService targets. Empty disables broadcast entirely.
    pub broadcast_targets: Vec<SocketAddr>,
    /// Verified routes of the lights the host tracks.
    pub tracked: HashMap<Serial, TrackedRoute>,
    /// Overall scan budget from the host.
    pub timeout: Duration,
    /// Per-request retry policy for probes.
    pub request_policy: RequestPolicy,
    /// Distinct serials the scan will probe before reporting partial
    /// coverage.
    pub max_candidates: usize,
    /// Probes in flight at once.
    pub max_concurrent_probes: NonZeroUsize,
}

impl Default for LifxScanOptions {
    fn default() -> Self {
        Self {
            unicast_targets: Vec::new(),
            broadcast_targets: Vec::new(),
            tracked: HashMap::new(),
            timeout: Duration::from_secs(2),
            request_policy: RequestPolicy::default(),
            max_candidates: MAX_DISCOVERY_CANDIDATES,
            max_concurrent_probes: MAX_CONCURRENT_PROBES,
        }
    }
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
        Err(error) => debug!(%error, "LIFX could not list interfaces for broadcast"),
    }
    targets
}

/// LIFX discovery scanner.
pub struct LifxScanner {
    options: LifxScanOptions,
}

impl LifxScanner {
    #[must_use]
    pub const fn new(options: LifxScanOptions) -> Self {
        Self { options }
    }

    /// Discover and probe devices.
    ///
    /// A light that answers but fails its probe has not proven where it is:
    /// a StateService reply is unverified, and anyone on the LAN can send
    /// one that names a tracked serial. Only a reply from the light's last
    /// verified endpoint counts as presence. Any other reply is an unknown
    /// that keeps the scan partial, so it can neither prove a dead light
    /// present nor have a live one retired.
    ///
    /// # Errors
    ///
    /// Returns an error when no discovery pass with targets could start. An
    /// address family with no targets is simply skipped, and everything
    /// short of that is reported as a coverage gap.
    pub async fn scan(&self) -> Result<ScanOutcome<LifxDeviceProfile>> {
        let started = Instant::now();
        let window = (self.options.timeout / 2).clamp(MIN_COLLECTION_WINDOW, MAX_COLLECTION_WINDOW);
        let (replies, gaps) = self.collect_services(window).await?;

        let policy = self.options.request_policy;
        let round = policy
            .reply_timeout
            .saturating_mul(policy.attempts.max(1).saturating_mul(PROBE_REQUESTS))
            .min(MAX_PROBE_ROUND);
        let budget = ProbeBudget {
            deadline: started + self.options.timeout.max(window + round),
            in_flight: self.options.max_concurrent_probes,
        };
        let mut outcome = probe_candidates(replies, budget, |reply: ServiceReply| {
            let verified = self
                .options
                .tracked
                .get(&reply.serial)
                .filter(|route| route.addr == reply.addr)
                .map(|route| route.fingerprint.clone());
            async move {
                match probe_at(reply.addr, reply.serial, policy).await {
                    Ok(Some(profile)) => Ok(Probed::Described(profile)),
                    Ok(None) => {
                        debug!(serial = %reply.serial, addr = %reply.addr, "LIFX device is not a light");
                        Ok(Probed::Nothing)
                    }
                    Err(error) => match verified {
                        Some(fingerprint) => Ok(Probed::Present(fingerprint)),
                        None => Err(error),
                    },
                }
            }
        })
        .await;

        outcome.gaps.extend(gaps);
        outcome.found.sort_by_key(|profile| profile.serial);
        Ok(outcome)
    }

    async fn collect_services(
        &self,
        window: Duration,
    ) -> Result<(Vec<ServiceReply>, Vec<CoverageGap>)> {
        let (v4_unicast, v6_unicast): (Vec<SocketAddr>, Vec<SocketAddr>) = self
            .options
            .unicast_targets
            .iter()
            .copied()
            .partition(SocketAddr::is_ipv4);
        let v4_broadcast: Vec<SocketAddr> = self
            .options
            .broadcast_targets
            .iter()
            .copied()
            .filter(SocketAddr::is_ipv4)
            .collect();
        let v4_targets: Vec<SocketAddr> = v4_unicast.iter().chain(&v4_broadcast).copied().collect();

        let admission = || {
            CandidateAdmission::new(self.options.max_candidates)
                .reserving(self.options.tracked.keys().copied())
        };
        let (v4, v6) = tokio::join!(
            discovery_pass("IPv4", &v4_targets, &v4_broadcast, window, admission()),
            discovery_pass("IPv6", &v6_unicast, &[], window, admission()),
        );
        let passes = [("IPv4", v4), ("IPv6", v6)];
        let any_ran = passes.iter().any(|(_, pass)| matches!(pass, Ok(Some(_))));

        let mut collected = admission();
        let mut gaps = Vec::new();
        for (family, pass) in passes {
            let round = match pass {
                Ok(Some(round)) => round,
                Ok(None) => continue,
                Err(error) if any_ran => {
                    gaps.push(CoverageGap::SourceFailed {
                        source_name: format!("LIFX {family} discovery"),
                        reason: format!("{error:#}"),
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            gaps.extend(round_gaps(&round, &v4_broadcast));
            for reply in round.replies {
                collected.offer(reply.serial, reply);
            }
        }
        let mut outcome = collected.finish();
        gaps.append(&mut outcome.gaps);
        Ok((outcome.found, gaps))
    }
}

/// Run one address family's GetService round, or skip it when it has no
/// targets. `broadcast` holds the subset of `targets` that are broadcast
/// addresses.
async fn discovery_pass(
    family: &'static str,
    targets: &[SocketAddr],
    broadcast: &[SocketAddr],
    window: Duration,
    admission: CandidateAdmission<Serial, ServiceReply>,
) -> Result<Option<DiscoveryRound>> {
    let Some(first) = targets.first() else {
        return Ok(None);
    };
    let mut client = LifxClient::bind_for(first)
        .await
        .with_context(|| format!("failed to bind the LIFX {family} discovery socket"))?;
    if !broadcast.is_empty() {
        client
            .enable_broadcast()
            .with_context(|| format!("failed to enable broadcast on the LIFX {family} socket"))?;
    }
    client
        .discover(targets, window, admission)
        .await
        .with_context(|| format!("LIFX {family} discovery failed"))
        .map(Some)
}

/// What a discovery round could have missed.
///
/// - any unicast send was rejected: configured and tracked lights are
///   probed by unicast, so that light was never asked;
/// - every broadcast send was rejected: nothing on the local networks was
///   asked (a single failed interface is tolerated, because each tracked
///   light also gets a unicast probe);
/// - the round's own gaps: a socket failure or refused replies.
#[must_use]
pub fn round_gaps(round: &DiscoveryRound, broadcast: &[SocketAddr]) -> Vec<CoverageGap> {
    let (broadcast_failed, unicast_failed): (Vec<&SocketAddr>, Vec<&SocketAddr>) = round
        .failed_targets
        .iter()
        .partition(|target| broadcast.contains(target));
    let mut gaps: Vec<CoverageGap> = unicast_failed
        .into_iter()
        .map(|target| CoverageGap::SendFailed {
            target: target.to_string(),
        })
        .collect();
    if !broadcast.is_empty() && broadcast_failed.len() == broadcast.len() {
        gaps.extend(
            broadcast_failed
                .into_iter()
                .map(|target| CoverageGap::SendFailed {
                    target: target.to_string(),
                }),
        );
    }
    gaps.extend(round.gaps.iter().cloned());
    gaps
}

/// Probe one device at a known address and serial.
///
/// Returns `Ok(None)` for devices that are not LIFX lights (switches, other
/// vendors).
///
/// # Errors
///
/// Returns an error when the socket cannot be bound or a required reply
/// never arrives.
pub async fn probe_at(
    addr: SocketAddr,
    serial: Serial,
    policy: RequestPolicy,
) -> Result<Option<LifxDeviceProfile>> {
    let mut client = LifxClient::bind_for(&addr)
        .await
        .with_context(|| format!("failed to bind LIFX probe socket for {addr}"))?;
    probe_device(&mut client, addr, serial, policy).await
}

/// Probe a device with an existing client.
///
/// # Errors
///
/// See [`probe_at`].
pub async fn probe_device(
    client: &mut LifxClient,
    addr: SocketAddr,
    serial: Serial,
    policy: RequestPolicy,
) -> Result<Option<LifxDeviceProfile>> {
    let State::Version(version) = client
        .request(
            addr,
            serial,
            &Message::GetVersion,
            &[message_type::STATE_VERSION],
            policy,
        )
        .await?
    else {
        bail!("LIFX device {serial} sent an unexpected GetVersion reply");
    };
    if version.vendor != LIFX_VENDOR_ID {
        debug!(%serial, vendor = version.vendor, "skipping non-LIFX vendor");
        return Ok(None);
    }
    let product = lookup_product(version.product);
    if product.is_some_and(|product| product.kind == ProductKind::NonLight) {
        return Ok(None);
    }

    let State::HostFirmware(firmware) = client
        .request(
            addr,
            serial,
            &Message::GetHostFirmware,
            &[message_type::STATE_HOST_FIRMWARE],
            policy,
        )
        .await?
    else {
        bail!("LIFX device {serial} sent an unexpected GetHostFirmware reply");
    };
    let firmware = firmware.version();

    let label = match client
        .request(
            addr,
            serial,
            &Message::GetLabel,
            &[message_type::STATE_LABEL],
            policy,
        )
        .await
    {
        Ok(State::Label(label)) => label,
        Ok(_) => String::new(),
        Err(error) => {
            debug!(%serial, error = %error, "LIFX label unavailable");
            String::new()
        }
    };

    let topology = match product.map(|product| product.kind) {
        Some(ProductKind::Matrix) => probe_matrix(client, addr, serial, policy, product).await?,
        Some(ProductKind::Multizone) => {
            let extended =
                product.is_some_and(|product| product.supports_extended_multizone(firmware));
            probe_multizone(client, addr, serial, policy, extended).await?
        }
        _ => LifxTopology::SingleZone,
    };

    Ok(Some(LifxDeviceProfile {
        serial,
        addr,
        label,
        vendor_id: version.vendor,
        product_id: version.product,
        product,
        firmware,
        topology,
    }))
}

async fn probe_matrix(
    client: &mut LifxClient,
    addr: SocketAddr,
    serial: Serial,
    policy: RequestPolicy,
    product: Option<&'static crate::products::LifxProduct>,
) -> Result<LifxTopology> {
    let State::DeviceChain(chain) = client
        .request(
            addr,
            serial,
            &Message::GetDeviceChain,
            &[message_type::STATE_DEVICE_CHAIN],
            policy,
        )
        .await?
    else {
        bail!("LIFX device {serial} sent an unexpected GetDeviceChain reply");
    };
    if chain.tiles.is_empty() {
        bail!("LIFX matrix device {serial} reports an empty device chain");
    }
    Ok(LifxTopology::Matrix {
        tiles: chain.tiles.iter().map(TileGeometry::from).collect(),
        quirk: product.map_or(crate::products::MatrixQuirk::None, |product| product.quirk),
        uplight: product.and_then(|product| product.uplight),
    })
}

async fn probe_multizone(
    client: &mut LifxClient,
    addr: SocketAddr,
    serial: Serial,
    policy: RequestPolicy,
    extended: bool,
) -> Result<LifxTopology> {
    let zones = if extended {
        match client
            .request(
                addr,
                serial,
                &Message::GetExtendedColorZones,
                &[message_type::STATE_EXTENDED_COLOR_ZONES],
                policy,
            )
            .await?
        {
            State::ExtendedColorZones(state) => state.zones_count,
            _ => bail!("LIFX device {serial} sent an unexpected GetExtendedColorZones reply"),
        }
    } else {
        match client
            .request(
                addr,
                serial,
                &Message::GetColorZones {
                    start_index: 0,
                    end_index: u8::MAX,
                },
                &[message_type::STATE_ZONE, message_type::STATE_MULTI_ZONE],
                policy,
            )
            .await?
        {
            State::Zone(state) => u16::from(state.zones_count),
            State::MultiZone(state) => u16::from(state.zones_count),
            _ => bail!("LIFX device {serial} sent an unexpected GetColorZones reply"),
        }
    };
    if zones == 0 {
        bail!("LIFX multizone device {serial} reports zero zones");
    }
    Ok(LifxTopology::Multizone { zones, extended })
}

/// Frames per second a profile reaches under a message budget when every
/// frame is as cheap as its device class allows.
#[must_use]
pub fn max_fps_for(topology: &LifxTopology, max_messages_per_second: u16) -> u32 {
    let messages = OutputPlan::from_topology(topology).best_case_messages();
    (u32::from(max_messages_per_second) / messages).max(1)
}

/// Metadata published for a probed device.
#[must_use]
pub fn profile_metadata(profile: &LifxDeviceProfile) -> HashMap<String, String> {
    let mut metadata = HashMap::from([
        (metadata_key::IP.to_owned(), profile.addr.ip().to_string()),
        (
            metadata_key::PORT.to_owned(),
            profile.addr.port().to_string(),
        ),
        (metadata_key::SERIAL.to_owned(), profile.serial.to_string()),
        (
            metadata_key::VENDOR_ID.to_owned(),
            profile.vendor_id.to_string(),
        ),
        (
            metadata_key::PRODUCT_ID.to_owned(),
            profile.product_id.to_string(),
        ),
        (
            metadata_key::FIRMWARE.to_owned(),
            profile.firmware.to_string(),
        ),
        (
            metadata_key::DEVICE_CLASS.to_owned(),
            profile.topology.class_name().to_owned(),
        ),
        (
            metadata_key::ZONES.to_owned(),
            profile.topology.led_count().to_string(),
        ),
    ]);
    if let LifxTopology::Matrix { tiles, .. } = &profile.topology {
        metadata.insert(metadata_key::TILES.to_owned(), tiles.len().to_string());
    }
    metadata
}

/// Generic discovery record for a probed device.
#[must_use]
pub fn discovered_device(
    profile: &LifxDeviceProfile,
    max_messages_per_second: u16,
) -> DiscoveredDevice {
    let max_fps = max_fps_for(&profile.topology, max_messages_per_second);
    DiscoveredDevice {
        fingerprint: fingerprint_for_serial(profile.serial),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: profile.device_info(max_fps),
        metadata: profile_metadata(profile),
        claim: PortableIdentityClaim::mac_address(
            &profile.serial.to_string(),
            NetworkAttachment::Peer(profile.addr.ip()),
        ),
    }
}

/// Address and serial recorded in discovery metadata.
///
/// # Errors
///
/// Returns an error when the metadata lacks a parseable IP or serial.
pub fn route_from_metadata(metadata: &HashMap<String, String>) -> Result<(SocketAddr, Serial)> {
    let ip = metadata
        .get(metadata_key::IP)
        .ok_or_else(|| anyhow!("LIFX metadata is missing the device IP"))?
        .parse::<IpAddr>()
        .context("LIFX metadata carries an invalid device IP")?;
    let port = metadata
        .get(metadata_key::PORT)
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT);
    let serial = metadata
        .get(metadata_key::SERIAL)
        .ok_or_else(|| anyhow!("LIFX metadata is missing the device serial"))?
        .parse::<Serial>()
        .map_err(|error| anyhow!("{error}"))?;
    Ok((SocketAddr::new(ip, port), serial))
}
