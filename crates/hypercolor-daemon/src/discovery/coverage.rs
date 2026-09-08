//! Coverage: which stack sees and drives each physical device.
//!
//! Three sources describe hardware from different angles: the native
//! registry (HAL drivers), bridge routes (OpenRGB controllers the bridge
//! driver discovered), and the unclaimed USB inventory. This module joins
//! them per physical device so the API, the CLI, and the conflict guard
//! all reason about the same rows.
//!
//! Match keys, in order: serial (trimmed, case-insensitive), SMBus bus plus
//! address, USB bus path. A source contributes every key it has; the first
//! one already seen decides which row it joins.

use std::collections::HashMap;

use hypercolor_types::api::devices::{
    CoverageActive, CoverageBridgeDevice, CoverageIdentity, CoverageIdentityKind,
    CoverageNativeDevice, DeviceCoverageRow,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState, DriverTransportKind};

use super::DiscoveryRuntime;
use super::conflict_guard::BridgeOutputLock;

/// The OpenRGB fallback driver's module id.
pub const OPENRGB_DRIVER_ID: &str = "openrgb";
/// Metadata key the bridge driver publishes for its server endpoint.
pub(crate) const BRIDGE_METADATA_ENDPOINT: &str = "endpoint";
/// Metadata key for the bridge controller index on its server.
pub(crate) const BRIDGE_METADATA_CONTROLLER_INDEX: &str = "controller_index";
/// Metadata key for the bridge controller's OpenRGB location string.
pub(crate) const BRIDGE_METADATA_LOCATION: &str = "location";
/// Metadata key for the bridge controller's advertised output state.
pub(crate) const BRIDGE_METADATA_OUTPUT_ENABLED: &str = "output_enabled";
/// Metadata key for the bridge controller's advertised disable reason.
pub(crate) const BRIDGE_METADATA_DISABLED_REASON: &str = "disabled_reason";

/// Whether a tracked device is an OpenRGB bridge route.
///
/// `transport == Bridge` alone is not enough: the ROLI Blocks driver is a
/// bridge too. The driver id decides, with the OpenRGB metadata keys as a
/// fallback for routes recorded before the id was stable.
#[must_use]
pub fn is_openrgb_bridge_device(info: &DeviceInfo, metadata: &HashMap<String, String>) -> bool {
    info.origin.transport == DriverTransportKind::Bridge
        && (info.driver_id().eq_ignore_ascii_case(OPENRGB_DRIVER_ID)
            || (metadata.contains_key(BRIDGE_METADATA_ENDPOINT)
                && metadata.contains_key(BRIDGE_METADATA_CONTROLLER_INDEX)))
}

/// A native registry device as the join sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageNativeSide {
    pub device_id: DeviceId,
    pub driver_id: String,
    pub name: String,
    pub state: DeviceState,
    pub serial: Option<String>,
    /// `(bus_path, smbus_address)` as the SMBus scanner reports them.
    pub smbus: Option<(String, String)>,
    pub usb_path: Option<String>,
}

/// A bridge route as the join sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageBridgeSide {
    pub device_id: DeviceId,
    pub name: String,
    pub state: DeviceState,
    /// What the bridge itself says about writability.
    pub advertised_output_enabled: bool,
    pub advertised_disabled_reason: Option<String>,
    /// The daemon's own output lock, when the conflict guard holds one.
    pub lock: Option<BridgeOutputLock>,
    pub serial: Option<String>,
    /// The raw OpenRGB location string.
    pub location: Option<String>,
}

impl CoverageBridgeSide {
    /// Whether frames may flow through the route once the daemon's lock is
    /// applied on top of what the bridge advertises.
    #[must_use]
    pub fn output_enabled(&self) -> bool {
        self.advertised_output_enabled && self.lock.is_none()
    }

    /// The effective reason output is off, the daemon's lock first.
    #[must_use]
    pub fn disabled_reason(&self) -> Option<String> {
        self.lock
            .as_ref()
            .map(|lock| lock.reason.clone())
            .or_else(|| self.advertised_disabled_reason.clone())
    }
}

/// An unclaimed USB inventory entry as the join sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageUnclaimedSide {
    pub label: String,
    pub serial: Option<String>,
    pub bus_path: Option<String>,
}

/// One input to the coverage join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageSource {
    Native(CoverageNativeSide),
    Bridge(CoverageBridgeSide),
    Unclaimed(CoverageUnclaimedSide),
}

/// One physical device after the join, with the sides still typed so the
/// conflict guard can plan from it before it becomes an API row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedCoverageRow {
    pub identity: CoverageIdentity,
    pub native: Option<CoverageNativeSide>,
    pub bridge: Option<CoverageBridgeSide>,
    pub unclaimed: Option<CoverageUnclaimedSide>,
}

impl JoinedCoverageRow {
    /// Who drives the hardware right now.
    #[must_use]
    pub fn active(&self) -> CoverageActive {
        let native_renderable = self
            .native
            .as_ref()
            .is_some_and(|native| native.state.is_renderable());
        let bridge_output_enabled = self
            .bridge
            .as_ref()
            .is_some_and(CoverageBridgeSide::output_enabled);
        let bridge_renderable = self
            .bridge
            .as_ref()
            .is_some_and(|bridge| bridge.state.is_renderable());

        if native_renderable && bridge_output_enabled {
            CoverageActive::Conflict
        } else if native_renderable {
            CoverageActive::Native
        } else if bridge_renderable {
            CoverageActive::Bridge
        } else {
            CoverageActive::None
        }
    }

    /// Project into the REST row shape.
    #[must_use]
    pub fn into_api_row(self) -> DeviceCoverageRow {
        let active = self.active();
        DeviceCoverageRow {
            identity: self.identity,
            native: self.native.map(|native| CoverageNativeDevice {
                device_id: native.device_id.to_string(),
                driver_id: native.driver_id,
                state: native.state.variant_name().to_lowercase(),
            }),
            bridge: self.bridge.map(|bridge| CoverageBridgeDevice {
                device_id: bridge.device_id.to_string(),
                state: bridge.state.variant_name().to_lowercase(),
                output_enabled: bridge.output_enabled(),
                disabled_reason: bridge.disabled_reason(),
            }),
            unclaimed: self.unclaimed.is_some(),
            active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MatchKey {
    Serial(String),
    Smbus(String),
    UsbPath(String),
}

impl MatchKey {
    fn identity(&self, label: String) -> CoverageIdentity {
        let (kind, value) = match self {
            Self::Serial(value) => (CoverageIdentityKind::Serial, value.clone()),
            Self::Smbus(value) => (CoverageIdentityKind::Smbus, value.clone()),
            Self::UsbPath(value) => (CoverageIdentityKind::UsbPath, value.clone()),
        };
        CoverageIdentity { kind, value, label }
    }
}

/// What an OpenRGB location string tells us about the physical bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedOpenRgbLocation {
    /// `(bus, address)` for `I2C:` locations, address rendered `0x..`.
    pub smbus: Option<(String, String)>,
    /// `<bus>-<ports>` for `USB:` locations that use the host bus-path form.
    pub usb_path: Option<String>,
}

/// Parse an OpenRGB controller location such as
/// `I2C: SMBus I801 adapter (/dev/i2c-9), address 0x71` or `USB: 1-1.2`.
///
/// Returns `None` when the string carries nothing the join can key on
/// (`HID: /dev/hidraw3` and friends).
#[must_use]
pub fn parse_openrgb_location(location: &str) -> Option<ParsedOpenRgbLocation> {
    let location = location.trim();
    if let Some(rest) = location.strip_prefix("I2C:") {
        let (bus_part, address_part) = rest.rsplit_once(", address")?;
        let address = normalize_smbus_address(address_part.trim())?;
        let bus_part = bus_part.trim();
        let bus = bus_part
            .rsplit_once('(')
            .and_then(|(_, tail)| tail.split_once(')'))
            .map_or(bus_part, |(inner, _)| inner)
            .trim();
        if bus.is_empty() {
            return None;
        }
        return Some(ParsedOpenRgbLocation {
            smbus: Some((bus.to_owned(), address)),
            usb_path: None,
        });
    }
    if let Some(path) = location.strip_prefix("USB:") {
        let path = path.trim();
        if is_host_usb_path(path) {
            return Some(ParsedOpenRgbLocation {
                smbus: None,
                usb_path: Some(path.to_owned()),
            });
        }
    }
    None
}

fn is_host_usb_path(path: &str) -> bool {
    let Some((bus, ports)) = path.split_once('-') else {
        return false;
    };
    !bus.is_empty()
        && bus.chars().all(|ch| ch.is_ascii_digit())
        && !ports.is_empty()
        && ports.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
}

fn normalize_serial(serial: &str) -> Option<String> {
    let trimmed = serial.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
}

fn normalize_smbus_address(address: &str) -> Option<String> {
    let trimmed = address.trim();
    let value = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .map_or_else(
            || trimmed.parse::<u8>().ok(),
            |hex| u8::from_str_radix(hex, 16).ok(),
        )?;
    Some(format!("0x{value:02x}"))
}

fn smbus_key(bus: &str, address: &str) -> Option<MatchKey> {
    let bus = bus.trim();
    if bus.is_empty() {
        return None;
    }
    let address = normalize_smbus_address(address)?;
    Some(MatchKey::Smbus(format!("{bus}@{address}")))
}

fn usb_path_key(path: &str) -> Option<MatchKey> {
    let trimmed = path.trim();
    (!trimmed.is_empty()).then(|| MatchKey::UsbPath(trimmed.to_owned()))
}

impl CoverageSource {
    fn match_keys(&self) -> Vec<MatchKey> {
        let mut keys = Vec::new();
        match self {
            Self::Native(native) => {
                if let Some(serial) = native.serial.as_deref().and_then(normalize_serial) {
                    keys.push(MatchKey::Serial(serial));
                }
                if let Some(key) = native
                    .smbus
                    .as_ref()
                    .and_then(|(bus, address)| smbus_key(bus, address))
                {
                    keys.push(key);
                }
                if let Some(key) = native.usb_path.as_deref().and_then(usb_path_key) {
                    keys.push(key);
                }
            }
            Self::Bridge(bridge) => {
                if let Some(serial) = bridge.serial.as_deref().and_then(normalize_serial) {
                    keys.push(MatchKey::Serial(serial));
                }
                if let Some(parsed) = bridge.location.as_deref().and_then(parse_openrgb_location) {
                    if let Some(key) = parsed
                        .smbus
                        .as_ref()
                        .and_then(|(bus, address)| smbus_key(bus, address))
                    {
                        keys.push(key);
                    }
                    if let Some(key) = parsed.usb_path.as_deref().and_then(usb_path_key) {
                        keys.push(key);
                    }
                }
            }
            Self::Unclaimed(unclaimed) => {
                if let Some(serial) = unclaimed.serial.as_deref().and_then(normalize_serial) {
                    keys.push(MatchKey::Serial(serial));
                }
                if let Some(key) = unclaimed.bus_path.as_deref().and_then(usb_path_key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    fn label(&self) -> String {
        match self {
            Self::Native(native) => native.name.clone(),
            Self::Bridge(bridge) => bridge.name.clone(),
            Self::Unclaimed(unclaimed) => unclaimed.label.clone(),
        }
    }

    fn fallback_identity(&self) -> CoverageIdentity {
        let value = match self {
            Self::Native(native) => native.device_id.to_string(),
            Self::Bridge(bridge) => bridge.device_id.to_string(),
            Self::Unclaimed(unclaimed) => unclaimed.label.clone(),
        };
        CoverageIdentity {
            kind: CoverageIdentityKind::Device,
            value,
            label: self.label(),
        }
    }
}

/// Join sources into per-device rows.
///
/// Native devices are joined first so a row's label prefers the native
/// name; within a row the first native and the first bridge side win, and
/// a renderable native side displaces a non-renderable one.
#[must_use]
pub fn join_coverage_sources(sources: Vec<CoverageSource>) -> Vec<JoinedCoverageRow> {
    let mut ordered: Vec<CoverageSource> = Vec::with_capacity(sources.len());
    let priority = |source: &CoverageSource| match source {
        CoverageSource::Native(_) => 0_u8,
        CoverageSource::Bridge(_) => 1,
        CoverageSource::Unclaimed(_) => 2,
    };
    ordered.extend(sources);
    ordered.sort_by_key(|source| priority(source));

    let mut rows: Vec<JoinedCoverageRow> = Vec::new();
    let mut key_index: HashMap<MatchKey, usize> = HashMap::new();

    for source in ordered {
        let keys = source.match_keys();
        let existing = keys.iter().find_map(|key| key_index.get(key).copied());
        let row_index = if let Some(index) = existing {
            index
        } else {
            let identity = keys.first().map_or_else(
                || source.fallback_identity(),
                |key| key.identity(source.label()),
            );
            rows.push(JoinedCoverageRow {
                identity,
                native: None,
                bridge: None,
                unclaimed: None,
            });
            rows.len() - 1
        };
        for key in keys {
            key_index.entry(key).or_insert(row_index);
        }

        let row = &mut rows[row_index];
        match source {
            CoverageSource::Native(native) => match &row.native {
                Some(current) if current.state.is_renderable() || !native.state.is_renderable() => {
                }
                _ => row.native = Some(native),
            },
            CoverageSource::Bridge(bridge) => {
                if row.bridge.is_none() {
                    row.bridge = Some(bridge);
                }
            }
            CoverageSource::Unclaimed(unclaimed) => {
                if row.unclaimed.is_none() {
                    row.unclaimed = Some(unclaimed);
                }
            }
        }
    }

    rows.sort_by(|left, right| {
        left.identity
            .label
            .to_lowercase()
            .cmp(&right.identity.label.to_lowercase())
            .then_with(|| left.identity.value.cmp(&right.identity.value))
    });
    rows
}

/// Read every coverage source from the live runtime.
pub async fn collect_coverage_sources(runtime: &DiscoveryRuntime) -> Vec<CoverageSource> {
    let locks = runtime.bridge_output_locks.snapshot();
    let mut sources = Vec::new();

    for tracked in runtime.device_registry.list().await {
        let metadata = runtime
            .device_registry
            .metadata_for_id(&tracked.info.id)
            .await
            .unwrap_or_default();
        let value = |key: &str| {
            metadata
                .get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        };
        if is_openrgb_bridge_device(&tracked.info, &metadata) {
            sources.push(CoverageSource::Bridge(CoverageBridgeSide {
                device_id: tracked.info.id,
                name: tracked.info.name.clone(),
                state: tracked.state.clone(),
                advertised_output_enabled: value(BRIDGE_METADATA_OUTPUT_ENABLED)
                    .is_none_or(|flag| !flag.eq_ignore_ascii_case("false")),
                advertised_disabled_reason: value(BRIDGE_METADATA_DISABLED_REASON),
                lock: locks.get(&tracked.info.id).cloned(),
                serial: value("serial"),
                location: value(BRIDGE_METADATA_LOCATION),
            }));
        } else {
            let smbus = match (value("bus_path"), value("smbus_address")) {
                (Some(bus), Some(address)) => Some((bus, address)),
                _ => None,
            };
            sources.push(CoverageSource::Native(CoverageNativeSide {
                device_id: tracked.info.id,
                driver_id: tracked.info.driver_id().to_owned(),
                name: tracked.info.name.clone(),
                state: tracked.state.clone(),
                serial: value("serial"),
                smbus,
                usb_path: value("usb_path"),
            }));
        }
    }

    for device in runtime.unclaimed_devices.snapshot() {
        let label = device
            .product
            .clone()
            .unwrap_or_else(|| format!("USB {:04X}:{:04X}", device.vendor_id, device.product_id));
        sources.push(CoverageSource::Unclaimed(CoverageUnclaimedSide {
            label,
            serial: device.serial,
            bus_path: device.bus_path,
        }));
    }

    sources
}

/// The joined coverage rows for the live runtime.
pub async fn collect_device_coverage(runtime: &DiscoveryRuntime) -> Vec<JoinedCoverageRow> {
    join_coverage_sources(collect_coverage_sources(runtime).await)
}
