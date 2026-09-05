//! Device discovery: the RX half's GetDev table and the TX half's MAC reply
//! (spec 80 section 6.5, corrected against hardware).
//!
//! Two facts the spec did not carry, both observed on a V1 controller with
//! no fans bound: the table comes from the RX device, not the TX, and a poll
//! for `pages` pages answers with `448 * pages` bytes regardless of how many
//! records are filled. The TX answers every command with its own status
//! packet (echo byte, its MAC, a counter, its firmware), so a parser that
//! trusted the count byte of a TX reply would read the MAC's first byte as
//! a device count.

use super::frame::{Mac, USB_CMD_GET_MAC, USB_CMD_SEND_RF};

/// Records a device table can hold.
pub const MAX_DEVICES: usize = 12;
/// Bytes per device record.
pub const RECORD_LEN: usize = 42;
/// Bytes ahead of the first record.
pub const TABLE_HEADER_LEN: usize = 4;
/// Marker every valid record ends with.
pub const RECORD_VALIDATION: u8 = 0x1C;
/// Read capacity that covers a two-page reply (896 bytes observed).
pub const GET_DEV_REPLY_CAPACITY: usize = 1024;
/// Record device type of a master (another controller heard on the channel).
pub const DEVICE_TYPE_MASTER: u8 = 0xFF;
/// Record device type of a fan cluster.
pub const DEVICE_TYPE_FAN_CLUSTER: u8 = 0x00;
/// Fan slots a cluster record describes.
pub const SLOTS_PER_CLUSTER: usize = 4;

/// What the TX reports about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterInfo {
    /// The controller's radio MAC, carried in every envelope it sends.
    pub mac: Mac,
    /// Firmware word, `0x0010` for `SLV3TX_V1.6`.
    pub firmware: Option<u16>,
}

/// Parse a TX reply to the MAC query. `None` when the reply is not one.
#[must_use]
pub fn parse_master_reply(reply: &[u8]) -> Option<MasterInfo> {
    if reply.len() < 7 || reply[0] != USB_CMD_GET_MAC {
        return None;
    }
    let mut mac = [0_u8; 6];
    mac.copy_from_slice(&reply[1..7]);
    if mac == [0; 6] {
        return None;
    }
    let firmware = reply
        .get(11..13)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
    Some(MasterInfo { mac, firmware })
}

/// Fan model behind a record's fan-type byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WirelessFanModel {
    /// UNI FAN TL Wireless, 2024 generation (TL V2 in Lian Li's numbering).
    TlV2 {
        /// The SKU with the wired LCD.
        lcd: bool,
    },
    /// UNI FAN TL V3 Wireless. Same RF protocol by inspection, unvalidated
    /// on hardware; classified so it is named, not gated on features.
    TlV3 {
        /// The SKU with the wired LCD.
        lcd: bool,
    },
    /// UNI FAN SL V3 Wireless.
    SlV3 {
        /// The SKU with the wired LCD.
        lcd: bool,
    },
    /// UNI FAN SL-INF Wireless.
    SlInf,
    /// UNI FAN SL-INF V3 Wireless.
    SlInfV3 {
        /// The SKU with the wired LCD.
        lcd: bool,
    },
    /// UNI FAN SL V4 Wireless.
    SlV4,
    /// A fan type byte this driver has no table entry for.
    Unknown(u8),
}

impl WirelessFanModel {
    /// Classify a record's fan-type byte.
    #[must_use]
    pub const fn from_type_byte(byte: u8) -> Self {
        match byte {
            20..=22 => Self::SlV3 { lcd: false },
            23..=26 => Self::SlV3 { lcd: true },
            27 | 32..=35 => Self::TlV2 { lcd: true },
            28..=31 => Self::TlV2 { lcd: false },
            36..=39 => Self::SlInf,
            43..=50 => Self::SlInfV3 {
                lcd: matches!(byte, 43 | 44 | 47 | 48),
            },
            51..=58 => Self::TlV3 {
                lcd: matches!(byte, 51 | 52 | 55 | 56),
            },
            59..=62 => Self::SlV4,
            other => Self::Unknown(other),
        }
    }

    /// LEDs each fan of this model carries.
    #[must_use]
    pub const fn leds_per_fan(self) -> u8 {
        match self {
            Self::TlV2 { .. } | Self::TlV3 { .. } => 26,
            Self::SlV3 { .. } => 40,
            Self::SlInf | Self::SlInfV3 { .. } => 44,
            Self::SlV4 => 52,
            Self::Unknown(_) => 20,
        }
    }

    /// Product name for segment and diagnostics labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TlV2 { lcd: false } => "UNI FAN TL Wireless",
            Self::TlV2 { lcd: true } => "UNI FAN TL Wireless LCD",
            Self::TlV3 { lcd: false } => "UNI FAN TL V3 Wireless",
            Self::TlV3 { lcd: true } => "UNI FAN TL V3 Wireless LCD",
            Self::SlV3 { lcd: false } => "UNI FAN SL V3 Wireless",
            Self::SlV3 { lcd: true } => "UNI FAN SL V3 Wireless LCD",
            Self::SlInf => "UNI FAN SL-INF Wireless",
            Self::SlInfV3 { lcd: false } => "UNI FAN SL-INF V3 Wireless",
            Self::SlInfV3 { lcd: true } => "UNI FAN SL-INF V3 Wireless LCD",
            Self::SlV4 => "UNI FAN SL V4 Wireless",
            Self::Unknown(_) => "Wireless Fan",
        }
    }
}

/// One fan cluster (up to four daisy-chained fans on one receiver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCluster {
    /// The receiver's radio MAC.
    pub mac: Mac,
    /// The controller it is bound to; all zero when unbound.
    pub master_mac: Mac,
    /// RF channel the receiver listens on.
    pub channel: u8,
    /// Radio endpoint slot, 1 to 13.
    pub rx_type: u8,
    /// Record device type: `0` for a fan cluster; AIO and case gear use
    /// other values and are not driven by this protocol.
    pub device_type: u8,
    /// Fans on the chain, 1 to 4.
    pub fan_count: u8,
    /// SL-INF chains that attach on the right wire right to left, so the
    /// per-fan runs reverse on the wire.
    pub right_attach: bool,
    /// Raw fan-type byte per slot.
    pub fan_types: [u8; SLOTS_PER_CLUSTER],
    /// Model classified from the first non-zero fan-type byte.
    pub model: WirelessFanModel,
    /// Fan RPM per slot.
    pub rpm: [u16; SLOTS_PER_CLUSTER],
    /// Current PWM duty per slot, 0 to 255.
    pub pwm: [u8; SLOTS_PER_CLUSTER],
    /// Sequence byte the receiver echoes to acknowledge commands.
    pub cmd_seq: u8,
    /// The effect tag last accepted by the receiver.
    pub effect_index: [u8; 4],
}

impl FanCluster {
    /// Total LEDs across the chain.
    #[must_use]
    pub fn led_count(&self) -> u32 {
        u32::from(self.fan_count) * u32::from(self.model.leds_per_fan())
    }

    /// Whether this record is a fan cluster bound to `master`, the only kind
    /// this protocol drives. Receivers bound elsewhere, unbound receivers,
    /// and AIO or case gear are visible on the channel but not ours.
    #[must_use]
    pub fn is_bound_fan_cluster(&self, master: Mac) -> bool {
        self.device_type == DEVICE_TYPE_FAN_CLUSTER
            && self.master_mac == master
            && self.fan_count > 0
    }
}

/// A parsed GetDev reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceTable {
    /// Fan clusters bound to any master, in table order.
    pub clusters: Vec<FanCluster>,
    /// Duty the controller reads off the motherboard PWM header, when wired.
    pub motherboard_pwm: Option<u8>,
}

/// Why a reply was not a device table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// Shorter than the four-byte table header.
    #[error("GetDev reply of {0} bytes is shorter than its header")]
    Short(usize),
    /// The echo byte was not the poll command.
    #[error("GetDev reply echoes 0x{0:02X}, not the poll command")]
    WrongEcho(u8),
    /// The count byte exceeds what a table can hold.
    #[error("GetDev reply claims {0} devices; a table holds at most {MAX_DEVICES}")]
    TooManyDevices(u8),
    /// The TX's status packet, which echoes the poll command in front of the
    /// controller's own MAC; there is no table in it.
    #[error("controller status packet, not a device table")]
    StatusEcho,
    /// The reply ended before every record it declared.
    #[error("GetDev reply declares {declared} records but carries {present}")]
    Truncated {
        /// Records the count byte promised.
        declared: u8,
        /// Whole records actually present.
        present: usize,
    },
}

/// Parse a GetDev reply from the RX.
///
/// # Errors
///
/// Returns [`DiscoveryError`] when the reply is not a device table:
/// [`DiscoveryError::StatusEcho`] names the TX's status packet, recognised
/// by the controller's MAC where the count and PWM bytes would be.
pub fn parse_device_table(
    reply: &[u8],
    master: Option<Mac>,
) -> Result<DeviceTable, DiscoveryError> {
    if reply.len() < TABLE_HEADER_LEN {
        return Err(DiscoveryError::Short(reply.len()));
    }
    if reply[0] != USB_CMD_SEND_RF {
        return Err(DiscoveryError::WrongEcho(reply[0]));
    }
    if let Some(master) = master
        && reply.len() >= 7
        && reply[1..7] == master
    {
        return Err(DiscoveryError::StatusEcho);
    }

    let count = reply[1];
    if usize::from(count) > MAX_DEVICES {
        return Err(DiscoveryError::TooManyDevices(count));
    }
    let motherboard_pwm = if reply[2] & 0x80 == 0 {
        let on = u16::from(reply[2] & 0x7F);
        let off = u16::from(reply[3]);
        let total = on + off;
        (total > 0).then(|| u8::try_from((255 * on) / total).unwrap_or(u8::MAX))
    } else {
        None
    };

    let records = reply[TABLE_HEADER_LEN..].chunks_exact(RECORD_LEN);
    let present = records.len().min(usize::from(count));
    if present < usize::from(count) {
        // A short reply (an early inter-packet gap) must not replace a good
        // table with the clusters that happened to arrive.
        return Err(DiscoveryError::Truncated {
            declared: count,
            present,
        });
    }
    let clusters = records.take(present).filter_map(parse_record).collect();

    Ok(DeviceTable {
        clusters,
        motherboard_pwm,
    })
}

fn parse_record(record: &[u8]) -> Option<FanCluster> {
    if record[41] != RECORD_VALIDATION || record[18] == DEVICE_TYPE_MASTER {
        return None;
    }
    let mut mac = [0_u8; 6];
    mac.copy_from_slice(&record[0..6]);
    if mac == [0; 6] {
        return None;
    }
    let mut master_mac = [0_u8; 6];
    master_mac.copy_from_slice(&record[6..12]);

    let raw_fan_count = record[19];
    let (fan_count, right_attach) = if raw_fan_count >= 10 {
        ((raw_fan_count - 10).min(4), true)
    } else {
        (raw_fan_count.min(4), false)
    };

    let mut fan_types = [0_u8; SLOTS_PER_CLUSTER];
    fan_types.copy_from_slice(&record[24..28]);
    let model = fan_types
        .iter()
        .find(|byte| **byte != 0)
        .map_or(WirelessFanModel::Unknown(0), |byte| {
            WirelessFanModel::from_type_byte(*byte)
        });

    let mut rpm = [0_u16; SLOTS_PER_CLUSTER];
    for (slot, value) in rpm.iter_mut().enumerate() {
        let high = record[28 + slot * 2] & 0x0F;
        *value = u16::from_be_bytes([high, record[29 + slot * 2]]);
    }
    let mut pwm = [0_u8; SLOTS_PER_CLUSTER];
    pwm.copy_from_slice(&record[36..40]);
    let mut effect_index = [0_u8; 4];
    effect_index.copy_from_slice(&record[20..24]);

    Some(FanCluster {
        mac,
        master_mac,
        channel: record[12],
        rx_type: record[13],
        device_type: record[18],
        fan_count,
        right_attach,
        fan_types,
        model,
        rpm,
        pwm,
        cmd_seq: record[40],
        effect_index,
    })
}
