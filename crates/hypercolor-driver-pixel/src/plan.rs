//! Output plans: where every channel of a device lands on the wire.
//!
//! A device's pixels are encoded into one contiguous channel stream,
//! segment after segment. The plan maps ranges of that stream to DMX
//! universe slots (E1.31, Art-Net) or DDP byte offsets, and checks that
//! the mapping is valid before a single packet is sent.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::ops::Range;
use std::sync::LazyLock;

use hypercolor_driver_support::network::validate_port;
use hypercolor_pixel_protocols::artnet::ARTNET_MAX_PORT_ADDRESS;
use hypercolor_pixel_protocols::ddp::{
    DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DDP_DTYPE_UNDEFINED, pixel_aligned_chunk_len,
};
use hypercolor_pixel_protocols::dmx::{
    DMX_UNIVERSE_CHANNELS, pixel_aligned_universe_channels, split_into_universes,
};
use hypercolor_pixel_protocols::e131::{
    E131_DEFAULT_PRIORITY, E131_MAX_PRIORITY, E131_MAX_UNIVERSE, E131_MIN_UNIVERSE,
    E131_SOURCE_NAME_MAX_BYTES,
};
use uuid::Uuid;

use crate::color::ColorOrder;
use crate::config::{
    DEFAULT_SOURCE_NAME, MAX_DEVICE_CHANNELS, MAX_DEVICES, MAX_FPS_LIMIT, MAX_NAME_LEN,
    MAX_SEGMENTS, PixelConfig, PixelConfigError, PixelDeviceConfig, PixelProtocol,
    is_unicast_target, is_valid_device_id,
};

/// Namespace for CIDs derived from device ids (UUID v5).
const CID_NAMESPACE: Uuid = Uuid::from_u128(0x93b3_486f_ebaf_41aa_a698_805f_b001_6f41);

/// Fully resolved output plan for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDevicePlan {
    /// Stable device id from config.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Destination address.
    pub target: SocketAddr,
    /// Streaming protocol.
    pub protocol: PixelProtocol,
    /// Wire channel order.
    pub color_order: ColorOrder,
    /// Channels per pixel in the stream.
    pub channels_per_pixel: usize,
    /// Outputs in stream order.
    pub segments: Vec<SegmentPlan>,
    /// Output frame rate cap; 0 follows the render loop.
    pub max_fps: u32,
    /// Protocol-specific routing.
    pub output: OutputPlan,
}

/// One output of a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPlan {
    /// Display name.
    pub name: String,
    /// Pixels on this output.
    pub pixel_count: u32,
}

/// Protocol-specific routing for a device's channel stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputPlan {
    /// E1.31 universes plus source identity.
    E131 {
        /// Universe routing.
        dmx: DmxPlan,
        /// Source identity and synchronization.
        source: E131Source,
    },
    /// Art-Net universes (Port-Addresses).
    ArtNet {
        /// Universe routing.
        dmx: DmxPlan,
    },
    /// DDP byte spans.
    Ddp(DdpPlan),
}

/// Universe routing shared by E1.31 and Art-Net.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmxPlan {
    /// Universes this device sends, ascending.
    pub universes: Vec<UniversePlan>,
    /// Stream ranges copied into universe slots each frame.
    pub runs: Vec<ChannelRun>,
}

/// One universe a device sends every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniversePlan {
    /// Universe number (E1.31) or Port-Address (Art-Net).
    pub universe: u16,
    /// Slots sent: one past the highest slot any run writes. Lower unused
    /// slots are sent as zero because DMX addressing is positional.
    pub channel_count: usize,
}

/// A range of the channel stream that lands in one universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRun {
    /// Index into [`DmxPlan::universes`].
    pub universe_index: usize,
    /// Zero-based slot of the first channel.
    pub first_slot: usize,
    /// Range of the device channel stream.
    pub source: Range<usize>,
}

/// E1.31 source identity for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E131Source {
    /// Data priority, 0-200.
    pub priority: u8,
    /// Source name shown by receivers.
    pub source_name: String,
    /// Sender CID.
    pub cid: Uuid,
    /// Synchronization universe, if frames are synchronized.
    pub sync_universe: Option<u16>,
}

/// DDP routing for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdpPlan {
    /// Byte spans in stream order.
    pub spans: Vec<DdpSpanPlan>,
    /// DDP data type byte.
    pub data_type: u8,
    /// Largest payload per packet that keeps pixels whole.
    pub chunk_len: usize,
}

/// A range of the channel stream placed at a DDP byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdpSpanPlan {
    /// Receiver byte offset of the first channel.
    pub offset: u32,
    /// Range of the device channel stream.
    pub source: Range<usize>,
}

impl PixelDevicePlan {
    /// Total pixels across every output.
    #[must_use]
    pub fn total_pixels(&self) -> u32 {
        self.segments
            .iter()
            .map(|segment| segment.pixel_count)
            .sum()
    }

    /// Length of the encoded channel stream.
    #[must_use]
    pub fn total_channels(&self) -> usize {
        self.total_pixels() as usize * self.channels_per_pixel
    }

    /// Universes this device sends, or empty for DDP.
    #[must_use]
    pub fn universes(&self) -> Vec<u16> {
        match &self.output {
            OutputPlan::E131 { dmx, .. } | OutputPlan::ArtNet { dmx } => {
                dmx.universes.iter().map(|plan| plan.universe).collect()
            }
            OutputPlan::Ddp(_) => Vec::new(),
        }
    }

    /// Compact universe list such as `1-3, 7`, or `None` for DDP.
    #[must_use]
    pub fn universe_summary(&self) -> Option<String> {
        let universes = self.universes();
        if universes.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        let mut start = universes[0];
        let mut end = start;
        for &universe in &universes[1..] {
            if Some(universe) == end.checked_add(1) {
                end = universe;
                continue;
            }
            parts.push(range_label(start, end));
            start = universe;
            end = universe;
        }
        parts.push(range_label(start, end));
        Some(parts.join(", "))
    }
}

fn range_label(start: u16, end: u16) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// Installation id used when the host persists none: a random UUID v4
/// generated once per process, so derived CIDs stay unique to this running
/// Hypercolor and stable for its lifetime.
static PROCESS_INSTALLATION_ID: LazyLock<String> = LazyLock::new(|| Uuid::new_v4().to_string());

/// The per-process fallback installation id (see [`derived_cid`]).
#[must_use]
pub fn fallback_installation_id() -> &'static str {
    &PROCESS_INSTALLATION_ID
}

/// Derive the stable E1.31 CID for a device.
///
/// E1.31 section 5.6 needs a CID unique to each source. The UUID v5 hashes
/// the installation id together with the device id under a fixed Hypercolor
/// namespace, so two installations that reuse a device id get different
/// CIDs. The daemon supplies its persisted instance id, which keeps the CID
/// stable across restarts; a host that persists none gets
/// [`fallback_installation_id`], which is unique per process instead.
#[must_use]
pub fn derived_cid(installation_id: &str, device_id: &str) -> Uuid {
    Uuid::new_v5(
        &CID_NAMESPACE,
        format!("{installation_id}/{device_id}").as_bytes(),
    )
}

/// Validate a whole config and resolve every device's plan.
///
/// Each returned plan already carries the driver-level E1.31 defaults.
///
/// # Errors
///
/// Returns the first validation failure found.
pub fn resolve_config(config: &PixelConfig) -> Result<Vec<PixelDevicePlan>, PixelConfigError> {
    resolve_config_for(config, None)
}

/// Validate a whole config and resolve every device's plan, deriving E1.31
/// CIDs for `installation_id` (see [`derived_cid`]).
///
/// # Errors
///
/// Returns the first validation failure found.
pub fn resolve_config_for(
    config: &PixelConfig,
    installation_id: Option<&str>,
) -> Result<Vec<PixelDevicePlan>, PixelConfigError> {
    if config.devices.len() > MAX_DEVICES {
        return Err(PixelConfigError::TooManyDevices {
            count: config.devices.len(),
        });
    }
    validate_source_name(&config.source_name).map_err(PixelConfigError::Driver)?;
    validate_priority(config.priority).map_err(PixelConfigError::Driver)?;

    let mut seen_ids = HashSet::new();
    let mut plans = Vec::with_capacity(config.devices.len());
    for device in &config.devices {
        if !seen_ids.insert(device.id.to_ascii_lowercase()) {
            return Err(PixelConfigError::DuplicateId {
                id: device.id.clone(),
            });
        }
        plans.push(resolve_device(
            &config.effective_device(device, installation_id),
        )?);
    }
    check_cross_device_conflicts(&plans)?;
    Ok(plans)
}

/// Validate one device entry and resolve its plan.
///
/// E1.31 fields missing here fall back to the standard defaults; call
/// [`PixelConfig::effective_device`] first to apply driver-level defaults.
///
/// # Errors
///
/// Returns a [`PixelConfigError::Device`] describing the first problem.
pub fn resolve_device(device: &PixelDeviceConfig) -> Result<PixelDevicePlan, PixelConfigError> {
    let id = device.id.as_str();
    let fail = |message: String| PixelConfigError::device(id, message);

    if !is_valid_device_id(id) {
        return Err(fail(
            "id must be 1-64 ASCII letters, digits, '-' or '_'".to_owned(),
        ));
    }
    if device
        .name
        .as_deref()
        .is_some_and(|name| name.chars().count() > MAX_NAME_LEN)
    {
        return Err(fail(format!(
            "name is longer than {MAX_NAME_LEN} characters"
        )));
    }
    let host = device.host.to_canonical();
    if !is_unicast_target(host) {
        return Err(fail(format!(
            "host {} is not a unicast address; multicast and broadcast output are not supported",
            device.host
        )));
    }
    if device.protocol == PixelProtocol::ArtNet && host.is_ipv6() {
        return Err(fail(format!(
            "host {} is IPv6, but Art-Net 4 is an IPv4 protocol",
            device.host
        )));
    }
    let port = device.effective_port();
    validate_port(port).map_err(|error| fail(error.to_string()))?;
    check_protocol_fields(device).map_err(fail)?;

    let color_channels = device.color_order.color_channels();
    let channels_per_pixel =
        usize::from(device.channels_per_pixel.unwrap_or(color_channels as u16));
    if channels_per_pixel < color_channels || channels_per_pixel > DMX_UNIVERSE_CHANNELS {
        return Err(fail(format!(
            "channels_per_pixel must be {color_channels}-{DMX_UNIVERSE_CHANNELS} for color order {}",
            device.color_order
        )));
    }

    let segments = resolve_segments(device).map_err(fail)?;
    let total_channels = segments
        .iter()
        .map(|(segment, _)| u64::from(segment.pixel_count))
        .sum::<u64>()
        .saturating_mul(channels_per_pixel as u64);
    if total_channels > MAX_DEVICE_CHANNELS {
        return Err(fail(format!(
            "device needs {total_channels} channels; the limit is {MAX_DEVICE_CHANNELS} per device, \
             so split it into several devices"
        )));
    }
    let max_fps = device.max_fps.unwrap_or(0);
    if max_fps > MAX_FPS_LIMIT {
        return Err(fail(format!("max_fps must be 0-{MAX_FPS_LIMIT}")));
    }

    let output = match device.protocol {
        PixelProtocol::E131 | PixelProtocol::ArtNet => {
            let dmx = resolve_dmx(device, &segments, channels_per_pixel).map_err(fail)?;
            if device.protocol == PixelProtocol::E131 {
                OutputPlan::E131 {
                    dmx,
                    source: resolve_e131_source(device).map_err(fail)?,
                }
            } else {
                OutputPlan::ArtNet { dmx }
            }
        }
        PixelProtocol::Ddp => {
            OutputPlan::Ddp(resolve_ddp(device, &segments, channels_per_pixel).map_err(fail)?)
        }
    };

    Ok(PixelDevicePlan {
        id: device.id.clone(),
        name: device.display_name().to_owned(),
        target: SocketAddr::new(host, port),
        protocol: device.protocol,
        color_order: device.color_order,
        channels_per_pixel,
        segments: segments.into_iter().map(|(plan, _)| plan).collect(),
        max_fps,
        output,
    })
}

/// Segment plus the raw config entry it came from (`None` for the
/// single-output `pixel_count` shorthand).
type ResolvedSegment<'a> = (SegmentPlan, Option<&'a crate::config::PixelSegmentConfig>);

fn resolve_segments(device: &PixelDeviceConfig) -> Result<Vec<ResolvedSegment<'_>>, String> {
    match (device.pixel_count, device.segments.is_empty()) {
        (Some(_), false) => Err("set either pixel_count or segments, not both".to_owned()),
        (None, true) => Err("set pixel_count or at least one segment".to_owned()),
        (Some(0), true) => Err("pixel_count must be at least 1".to_owned()),
        (Some(pixel_count), true) => Ok(vec![(
            SegmentPlan {
                name: "Main".to_owned(),
                pixel_count,
            },
            None,
        )]),
        (None, false) if device.segments.len() > MAX_SEGMENTS => Err(format!(
            "{} segments configured; the limit is {MAX_SEGMENTS} per device",
            device.segments.len()
        )),
        (None, false) => device
            .segments
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                if segment.pixel_count == 0 {
                    return Err(format!("segment {} needs at least 1 pixel", index + 1));
                }
                if segment
                    .name
                    .as_deref()
                    .is_some_and(|name| name.chars().count() > MAX_NAME_LEN)
                {
                    return Err(format!(
                        "segment {} name is longer than {MAX_NAME_LEN} characters",
                        index + 1
                    ));
                }
                let name = segment
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| format!("Output {}", index + 1), ToOwned::to_owned);
                Ok((
                    SegmentPlan {
                        name,
                        pixel_count: segment.pixel_count,
                    },
                    Some(segment),
                ))
            })
            .collect(),
    }
}

fn check_protocol_fields(device: &PixelDeviceConfig) -> Result<(), String> {
    let protocol = device.protocol;
    let only = |field: &str, allowed: &str| {
        Err(format!(
            "{field} only applies to {allowed}, not {}",
            protocol.as_str()
        ))
    };
    if protocol != PixelProtocol::E131 {
        if device.priority.is_some() {
            return only("priority", "e131");
        }
        if device.source_name.is_some() {
            return only("source_name", "e131");
        }
        if device.sync_universe.is_some() {
            return only("sync_universe", "e131");
        }
        if device.cid.is_some() {
            return only("cid", "e131");
        }
    }
    if protocol.is_dmx() {
        if device.ddp_offset.is_some()
            || device
                .segments
                .iter()
                .any(|segment| segment.ddp_offset.is_some())
        {
            return only("ddp_offset", "ddp");
        }
    } else if device.start_universe.is_some()
        || device.start_channel.is_some()
        || device.channels_per_universe.is_some()
        || device
            .segments
            .iter()
            .any(|segment| segment.start_universe.is_some() || segment.start_channel.is_some())
    {
        return only("universe and channel addressing", "e131 or artnet");
    }
    Ok(())
}

fn universe_range(protocol: PixelProtocol) -> (u16, u16) {
    match protocol {
        PixelProtocol::E131 => (E131_MIN_UNIVERSE, E131_MAX_UNIVERSE),
        // Art-Net 4 deprecates Port-Address 0 but nodes configured on
        // 0:0:0 still exist, so it stays addressable.
        PixelProtocol::ArtNet => (0, ARTNET_MAX_PORT_ADDRESS),
        PixelProtocol::Ddp => (0, 0),
    }
}

fn resolve_dmx(
    device: &PixelDeviceConfig,
    segments: &[ResolvedSegment<'_>],
    channels_per_pixel: usize,
) -> Result<DmxPlan, String> {
    let (min_universe, max_universe) = universe_range(device.protocol);
    let per_universe = device.channels_per_universe.map_or_else(
        || pixel_aligned_universe_channels(channels_per_pixel),
        usize::from,
    );
    if per_universe == 0 || per_universe > DMX_UNIVERSE_CHANNELS {
        return Err(format!(
            "channels_per_universe must be 1-{DMX_UNIVERSE_CHANNELS}"
        ));
    }
    if per_universe < channels_per_pixel {
        return Err(format!(
            "channels_per_universe ({per_universe}) is smaller than one pixel ({channels_per_pixel} channels)"
        ));
    }
    let check_universe = |universe: u16, what: &str| {
        if universe < min_universe || universe > max_universe {
            Err(format!(
                "{what} {universe} is outside the {} range {min_universe}-{max_universe}",
                device.protocol.label()
            ))
        } else {
            Ok(())
        }
    };
    let check_channel = |channel: u16, what: &str| {
        if channel == 0 || usize::from(channel) > per_universe {
            Err(format!("{what} {channel} must be 1-{per_universe}"))
        } else {
            Ok(())
        }
    };

    let device_universe = device.start_universe.unwrap_or(1);
    check_universe(device_universe, "start_universe")?;
    let device_channel = device.start_channel.unwrap_or(1);
    check_channel(device_channel, "start_channel")?;

    // Absolute slot = universe * per_universe + zero-based channel. Inputs
    // are bounded (universe <= 65535, per_universe <= 512, the device's
    // channels <= MAX_DEVICE_CHANNELS), so slots stay far below usize::MAX.
    let absolute = |universe: u16, channel: u16| {
        usize::from(universe) * per_universe + usize::from(channel) - 1
    };
    let mut cursor = absolute(device_universe, device_channel);
    let mut stream_offset = 0_usize;
    let mut slots: BTreeMap<u16, Vec<Range<usize>>> = BTreeMap::new();
    let mut raw_runs = Vec::new();

    for (index, (segment, config)) in segments.iter().enumerate() {
        let label = format!("segment {}", index + 1);
        if let Some(config) = config {
            match (config.start_universe, config.start_channel) {
                (Some(universe), channel) => {
                    check_universe(universe, &format!("{label} start_universe"))?;
                    let channel = channel.unwrap_or(1);
                    check_channel(channel, &format!("{label} start_channel"))?;
                    cursor = absolute(universe, channel);
                }
                (None, Some(_)) => {
                    return Err(format!("{label} start_channel requires start_universe"));
                }
                (None, None) => {}
            }
        }
        let length = segment.pixel_count as usize * channels_per_pixel;
        // Reject before building runs: the last addressed universe decides.
        let last_universe = cursor
            .checked_add(length - 1)
            .map(|last_slot| last_slot / per_universe);
        if last_universe.is_none_or(|universe| universe > usize::from(max_universe)) {
            return Err(format!(
                "{label} runs past {} universe {max_universe}",
                device.protocol.label()
            ));
        }
        for run in split_into_universes(cursor, length, per_universe) {
            let universe = u16::try_from(run.universe_offset).map_err(|_| {
                format!(
                    "{label} runs past {} universe {max_universe}",
                    device.protocol.label()
                )
            })?;
            let slot_range = run.first_slot..run.first_slot + run.source.len();
            let used = slots.entry(universe).or_default();
            if used.iter().any(|existing| overlaps(existing, &slot_range)) {
                return Err(format!(
                    "{label} overlaps another segment in universe {universe}"
                ));
            }
            used.push(slot_range);
            raw_runs.push((
                universe,
                run.first_slot,
                stream_offset + run.source.start..stream_offset + run.source.end,
            ));
        }
        cursor += length;
        stream_offset += length;
    }

    let universes: Vec<UniversePlan> = slots
        .iter()
        .map(|(&universe, ranges)| UniversePlan {
            universe,
            channel_count: ranges.iter().map(|range| range.end).max().unwrap_or(0),
        })
        .collect();
    let index_of: HashMap<u16, usize> = universes
        .iter()
        .enumerate()
        .map(|(index, plan)| (plan.universe, index))
        .collect();
    let runs = raw_runs
        .into_iter()
        .map(|(universe, first_slot, source)| ChannelRun {
            universe_index: index_of[&universe],
            first_slot,
            source,
        })
        .collect();
    Ok(DmxPlan { universes, runs })
}

fn resolve_e131_source(device: &PixelDeviceConfig) -> Result<E131Source, String> {
    let priority = device.priority.unwrap_or(E131_DEFAULT_PRIORITY);
    validate_priority(priority)?;
    let source_name = device
        .source_name
        .clone()
        .unwrap_or_else(|| DEFAULT_SOURCE_NAME.to_owned());
    validate_source_name(&source_name)?;
    if let Some(sync) = device.sync_universe
        && !(E131_MIN_UNIVERSE..=E131_MAX_UNIVERSE).contains(&sync)
    {
        return Err(format!(
            "sync_universe {sync} is outside the E1.31 range {E131_MIN_UNIVERSE}-{E131_MAX_UNIVERSE}"
        ));
    }
    Ok(E131Source {
        priority,
        source_name,
        cid: device
            .cid
            .unwrap_or_else(|| derived_cid(fallback_installation_id(), &device.id)),
        sync_universe: device.sync_universe,
    })
}

fn resolve_ddp(
    device: &PixelDeviceConfig,
    segments: &[ResolvedSegment<'_>],
    channels_per_pixel: usize,
) -> Result<DdpPlan, String> {
    let mut cursor = u64::from(device.ddp_offset.unwrap_or(0));
    let mut stream_offset = 0_usize;
    let mut spans: Vec<DdpSpanPlan> = Vec::with_capacity(segments.len());
    let mut used: Vec<Range<u64>> = Vec::with_capacity(segments.len());

    for (index, (segment, config)) in segments.iter().enumerate() {
        if let Some(offset) = config.and_then(|config| config.ddp_offset) {
            cursor = u64::from(offset);
        }
        let length = segment.pixel_count as usize * channels_per_pixel;
        let end = cursor.saturating_add(length as u64);
        if end > u64::from(u32::MAX) + 1 {
            return Err(format!(
                "segment {} runs past the 32-bit DDP offset space",
                index + 1
            ));
        }
        let range = cursor..end;
        if used.iter().any(|existing| overlaps(existing, &range)) {
            return Err(format!(
                "segment {} overlaps another segment's DDP bytes",
                index + 1
            ));
        }
        used.push(range);
        spans.push(DdpSpanPlan {
            offset: u32::try_from(cursor).map_err(|_| "DDP offset overflow".to_owned())?,
            source: stream_offset..stream_offset + length,
        });
        cursor = end;
        stream_offset += length;
    }

    let data_type = if channels_per_pixel != device.color_order.color_channels() {
        DDP_DTYPE_UNDEFINED
    } else if device.color_order.has_white() {
        DDP_DTYPE_RGBW8
    } else {
        DDP_DTYPE_RGB8
    };
    Ok(DdpPlan {
        spans,
        data_type,
        chunk_len: pixel_aligned_chunk_len(channels_per_pixel),
    })
}

fn validate_priority(priority: u8) -> Result<(), String> {
    if priority > E131_MAX_PRIORITY {
        Err(format!(
            "priority {priority} is outside the E1.31 range 0-{E131_MAX_PRIORITY}"
        ))
    } else {
        Ok(())
    }
}

fn validate_source_name(name: &str) -> Result<(), String> {
    if name.len() > E131_SOURCE_NAME_MAX_BYTES {
        Err(format!(
            "source_name is {} bytes; E1.31 allows at most {E131_SOURCE_NAME_MAX_BYTES}",
            name.len()
        ))
    } else {
        Ok(())
    }
}

fn overlaps<T: Ord>(left: &Range<T>, right: &Range<T>) -> bool {
    left.start < right.end && right.start < left.end
}

fn check_cross_device_conflicts(plans: &[PixelDevicePlan]) -> Result<(), PixelConfigError> {
    let mut dmx_owners: HashMap<(SocketAddr, PixelProtocol, u16), &str> = HashMap::new();
    let mut ddp_ranges: HashMap<SocketAddr, Vec<(Range<u64>, &str)>> = HashMap::new();

    for plan in plans {
        match &plan.output {
            OutputPlan::E131 { dmx, .. } | OutputPlan::ArtNet { dmx } => {
                for universe in &dmx.universes {
                    let key = (plan.target, plan.protocol, universe.universe);
                    if let Some(first) = dmx_owners.insert(key, &plan.id) {
                        return Err(PixelConfigError::UniverseConflict {
                            first: first.to_owned(),
                            second: plan.id.clone(),
                            protocol: plan.protocol.label(),
                            universe: universe.universe,
                            endpoint: plan.target.to_string(),
                        });
                    }
                }
            }
            OutputPlan::Ddp(ddp) => {
                let existing = ddp_ranges.entry(plan.target).or_default();
                let ranges: Vec<Range<u64>> = ddp
                    .spans
                    .iter()
                    .map(|span| {
                        u64::from(span.offset)..u64::from(span.offset) + span.source.len() as u64
                    })
                    .collect();
                for range in &ranges {
                    if let Some((other, first)) =
                        existing.iter().find(|(other, _)| overlaps(other, range))
                    {
                        return Err(PixelConfigError::DdpOverlap {
                            first: (*first).to_owned(),
                            second: plan.id.clone(),
                            start: range.start.max(other.start),
                            end: range.end.min(other.end) - 1,
                            endpoint: plan.target.to_string(),
                        });
                    }
                }
                existing.extend(ranges.into_iter().map(|range| (range, plan.id.as_str())));
            }
        }
    }
    Ok(())
}
