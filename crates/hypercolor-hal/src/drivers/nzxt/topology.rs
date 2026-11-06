//! Gen2 accessory catalog (Spec 39 §3.6) and topology response parsing.

use hypercolor_types::device::DeviceTopologyHint;
use tracing::warn;

use crate::protocol::ProtocolError;

use super::packet::{GEN2_REPORT_LEN, GEN2_TOPOLOGY_RESPONSE};

/// Byte offset of the channel count in a `0x21 0x03` response.
pub const TOPOLOGY_CHANNEL_COUNT_OFFSET: usize = 14;

/// Byte offset of channel 0's first accessory slot.
pub const TOPOLOGY_SLOTS_OFFSET: usize = 0x0F;

/// Accessory slots per channel record.
pub const TOPOLOGY_SLOTS_PER_CHANNEL: usize = 6;

/// Highest channel count the protocol can address: the channel mask is one
/// byte.
pub const GEN2_MAX_CHANNELS: u8 = 8;

/// Slot value that marks an empty slot and ends a channel's chain.
pub const EMPTY_SLOT: u8 = 0x00;

/// Accessory ID of the Kraken X3 pump ring.
pub const ACCESSORY_KRAKEN_X3_RING: u8 = 0x10;

/// Accessory ID of the Kraken X3 logo.
pub const ACCESSORY_KRAKEN_X3_LOGO: u8 = 0x11;

const _: () = assert!(
    TOPOLOGY_SLOTS_OFFSET + TOPOLOGY_SLOTS_PER_CHANNEL * GEN2_MAX_CHANNELS as usize
        <= GEN2_REPORT_LEN,
    "eight channel records fit a 64-byte topology response"
);

/// Physical shape of an accessory, used for its spatial layout hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessoryShape {
    /// Linear LED strip.
    Strip,
    /// LEDs around a fan frame or pump cap.
    Ring,
    /// Single LED.
    Point,
}

/// One entry of the Gen2 accessory catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accessory {
    /// ID reported in the topology response.
    pub id: u8,
    /// Display name.
    pub name: &'static str,
    /// LED count.
    pub leds: u8,
    /// Physical shape.
    pub shape: AccessoryShape,
}

impl Accessory {
    /// Spatial topology hint for one instance of this accessory.
    #[must_use]
    pub fn topology_hint(&self) -> DeviceTopologyHint {
        match self.shape {
            AccessoryShape::Strip => DeviceTopologyHint::Strip,
            AccessoryShape::Ring => DeviceTopologyHint::Ring {
                count: u32::from(self.leds),
            },
            AccessoryShape::Point => DeviceTopologyHint::Point,
        }
    }
}

const fn accessory(id: u8, name: &'static str, leds: u8, shape: AccessoryShape) -> Accessory {
    Accessory {
        id,
        name,
        leds,
        shape,
    }
}

/// Every accessory ID Spec 39 §3.6 documents.
///
/// The F360 and F420 entries are triple-fan units whose LEDs span three
/// separate fans, so they are laid out as strips rather than one ring.
pub static ACCESSORY_CATALOG: &[Accessory] = &[
    accessory(0x01, "Hue 1 Strip", 10, AccessoryShape::Strip),
    accessory(0x02, "Aer 1 Fan", 8, AccessoryShape::Ring),
    accessory(0x04, "Hue 2 Strip 10 LED", 10, AccessoryShape::Strip),
    accessory(0x05, "Hue 2 Strip 8 LED", 8, AccessoryShape::Strip),
    accessory(0x06, "Hue 2 Strip 6 LED", 6, AccessoryShape::Strip),
    accessory(0x08, "Hue 2 Cable Comb", 14, AccessoryShape::Strip),
    accessory(0x09, "Hue 2 Underglow 300 mm", 15, AccessoryShape::Strip),
    accessory(0x0A, "Hue 2 Underglow 200 mm", 10, AccessoryShape::Strip),
    accessory(0x0B, "Aer 2 120", 8, AccessoryShape::Ring),
    accessory(0x0C, "Aer 2 140", 8, AccessoryShape::Ring),
    accessory(
        ACCESSORY_KRAKEN_X3_RING,
        "Kraken X3 Pump Ring",
        8,
        AccessoryShape::Ring,
    ),
    accessory(
        ACCESSORY_KRAKEN_X3_LOGO,
        "Kraken X3 Logo",
        1,
        AccessoryShape::Point,
    ),
    accessory(0x13, "F120 RGB", 18, AccessoryShape::Ring),
    accessory(0x14, "F140 RGB", 18, AccessoryShape::Ring),
    accessory(0x15, "F120 RGB Duo", 20, AccessoryShape::Ring),
    accessory(0x16, "F140 RGB Duo", 20, AccessoryShape::Ring),
    accessory(0x17, "F120 RGB Core", 8, AccessoryShape::Ring),
    accessory(0x18, "F140 RGB Core", 8, AccessoryShape::Ring),
    accessory(0x19, "F120 RGB Core Case", 8, AccessoryShape::Ring),
    accessory(0x1D, "F360 RGB Core Case", 24, AccessoryShape::Strip),
    accessory(0x1E, "Kraken Elite Ring", 24, AccessoryShape::Ring),
    accessory(0x1F, "F420 RGB", 24, AccessoryShape::Strip),
];

/// Largest LED count of any catalog accessory.
pub const MAX_ACCESSORY_LEDS: usize = 24;

/// Look up a catalog accessory by its topology ID.
#[must_use]
pub fn lookup_accessory(id: u8) -> Option<&'static Accessory> {
    ACCESSORY_CATALOG.iter().find(|entry| entry.id == id)
}

/// Accessories reported on one Gen2 channel, in chain order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gen2Channel {
    /// Channel index (the stream mask is `1 << index`).
    pub index: u8,
    /// Known accessories in slot order, ending at the first empty slot or
    /// unknown ID.
    pub accessories: Vec<&'static Accessory>,
}

impl Gen2Channel {
    /// Total LEDs across the channel's accessories.
    #[must_use]
    pub fn led_count(&self) -> usize {
        self.accessories
            .iter()
            .map(|accessory| usize::from(accessory.leds))
            .sum()
    }

    /// Whether any accessory on this channel has `id`.
    #[must_use]
    pub fn carries(&self, id: u8) -> bool {
        self.accessories.iter().any(|accessory| accessory.id == id)
    }
}

/// Parsed `0x21 0x03` topology response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gen2Topology {
    /// One record per reported channel, in channel order. Channels with no
    /// known accessories are kept with an empty accessory list.
    pub channels: Vec<Gen2Channel>,
}

/// Whether `data` is a Gen2 topology response.
#[must_use]
pub fn is_topology_response(data: &[u8]) -> bool {
    data.starts_with(&GEN2_TOPOLOGY_RESPONSE)
}

/// Parse a Gen2 topology response.
///
/// # Errors
///
/// Returns [`ProtocolError::MalformedResponse`] when `data` is not a
/// `0x21 0x03` response or ends before the channel count byte.
pub fn parse_topology_response(data: &[u8]) -> Result<Gen2Topology, ProtocolError> {
    if !is_topology_response(data) {
        return Err(ProtocolError::MalformedResponse {
            detail: "NZXT topology response must start with 0x21 0x03".to_owned(),
        });
    }

    let reported = *data.get(TOPOLOGY_CHANNEL_COUNT_OFFSET).ok_or_else(|| {
        ProtocolError::MalformedResponse {
            detail: format!(
                "NZXT topology response is {} bytes; the channel count sits at byte {TOPOLOGY_CHANNEL_COUNT_OFFSET}",
                data.len()
            ),
        }
    })?;

    if reported > GEN2_MAX_CHANNELS {
        warn!(
            reported,
            max = GEN2_MAX_CHANNELS,
            "NZXT topology reports more channels than the one-byte mask can address; clamping"
        );
    }
    let channel_count = reported.min(GEN2_MAX_CHANNELS);

    let mut channels = Vec::with_capacity(usize::from(channel_count));
    for index in 0..channel_count {
        let start = TOPOLOGY_SLOTS_OFFSET + usize::from(index) * TOPOLOGY_SLOTS_PER_CHANNEL;
        let Some(slots) = data.get(start..start + TOPOLOGY_SLOTS_PER_CHANNEL) else {
            warn!(
                channel = index,
                response_len = data.len(),
                "NZXT topology response ends inside a channel record; dropping the remaining channels"
            );
            break;
        };
        channels.push(Gen2Channel {
            index,
            accessories: parse_channel_slots(index, slots),
        });
    }

    Ok(Gen2Topology { channels })
}

fn parse_channel_slots(channel: u8, slots: &[u8]) -> Vec<&'static Accessory> {
    let mut accessories = Vec::with_capacity(slots.len());
    for (slot, &id) in slots.iter().enumerate() {
        if id == EMPTY_SLOT {
            break;
        }
        let Some(entry) = lookup_accessory(id) else {
            warn!(
                channel,
                slot,
                accessory_id = format_args!("0x{id:02X}"),
                "unknown NZXT accessory; ignoring it and everything after it on this channel"
            );
            break;
        };
        accessories.push(entry);
    }
    accessories
}
