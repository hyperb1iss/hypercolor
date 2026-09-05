//! L-Wireless wire framing: USB packets to the controller and the 240-byte
//! RF envelopes it relays (spec 80 sections 6.1 to 6.3, 6.7, 6.8).
//!
//! The controller takes 64-byte bulk packets. A control packet is a short
//! command padded to the packet; an RF send is one 240-byte envelope shipped
//! as four 60-byte slices, each behind a four-byte USB header naming the
//! slice index, the RF channel, and the target radio slot.

use super::tinyuz;

/// Every packet to the controller is this long.
pub const USB_PACKET_LEN: usize = 64;
/// USB command: relay an RF envelope slice (also the GetDev poll on the RX).
pub const USB_CMD_SEND_RF: u8 = 0x10;
/// USB command: report the controller's MAC and firmware.
pub const USB_CMD_GET_MAC: u8 = 0x11;
/// USB header bytes ahead of each envelope slice.
pub const USB_RF_HEADER_LEN: usize = 4;

/// One RF envelope, relayed in four slices.
pub const RF_ENVELOPE_LEN: usize = 240;
/// Envelope bytes per USB packet.
pub const RF_SLICE_LEN: usize = RF_ENVELOPE_LEN / RF_SLICES_PER_ENVELOPE;
/// USB packets per envelope.
pub const RF_SLICES_PER_ENVELOPE: usize = 4;
/// Envelope opcode: a sub-command follows.
pub const RF_SELECT: u8 = 0x12;
/// Radio slot that addresses every receiver.
pub const RF_BROADCAST_SLOT: u8 = 0xFF;
/// RF channel the controller ships on.
pub const DEFAULT_CHANNEL: u8 = 8;

/// Compressed RGB bytes carried per data envelope.
pub const RGB_CHUNK_LEN: usize = 220;
/// Where the RGB chunk starts inside a data envelope.
pub const RGB_DATA_OFFSET: usize = 20;
/// Bytes of the clock-sync blob.
pub const CLOCK_PAYLOAD_LEN: usize = 220;

/// Precomposed TX control: reset the radio.
pub const TX_RESET: [u8; 4] = [USB_CMD_GET_MAC, 0x08, 0x00, 0x00];
/// Precomposed TX control: enter the streaming ("video") mode.
pub const TX_VIDEO_START: [u8; 4] = [USB_CMD_GET_MAC, 0x01, 0x00, 0x00];

/// Sub-command byte at envelope offset 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RfSubCommand {
    /// Fan PWM set; also the bind carrier.
    Pwm = 0x10,
    /// Group select.
    SelectedGroup = 0x12,
    /// 1 Hz master clock and sensor broadcast.
    ClockSync = 0x14,
    /// Per-LED RGB, tinyuz-compressed.
    SetRgb = 0x20,
}

/// A six-byte radio MAC.
pub type Mac = [u8; 6];

/// Pad a short control command to a controller packet.
#[must_use]
pub fn control_packet(prefix: &[u8]) -> Vec<u8> {
    let mut packet = vec![0_u8; USB_PACKET_LEN];
    let len = prefix.len().min(USB_PACKET_LEN);
    packet[..len].copy_from_slice(&prefix[..len]);
    packet
}

/// TX query: the controller's MAC and firmware, on `channel`.
#[must_use]
pub fn get_mac_query(channel: u8) -> Vec<u8> {
    control_packet(&[USB_CMD_GET_MAC, channel])
}

/// RX poll: the device table, `pages` pages of ten records.
#[must_use]
pub fn get_dev_poll(pages: u8) -> Vec<u8> {
    control_packet(&[USB_CMD_SEND_RF, pages])
}

/// TX preparation packet sent once per known device after video start.
#[must_use]
pub fn stream_prep_packet(device_index: u8, channel: u8) -> Vec<u8> {
    control_packet(&[USB_CMD_SEND_RF, device_index, channel, RF_BROADCAST_SLOT])
}

/// One 240-byte RF envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfEnvelope(pub [u8; RF_ENVELOPE_LEN]);

impl RfEnvelope {
    /// An envelope addressed from `master` to `target` carrying `sub`.
    #[must_use]
    pub fn new(sub: RfSubCommand, target: Mac, master: Mac) -> Self {
        let mut bytes = [0_u8; RF_ENVELOPE_LEN];
        bytes[0] = RF_SELECT;
        bytes[1] = sub as u8;
        bytes[2..8].copy_from_slice(&target);
        bytes[8..14].copy_from_slice(&master);
        Self(bytes)
    }

    /// The four USB packets that relay this envelope on `channel` to the
    /// radio slot `rx_type`.
    #[must_use]
    pub fn usb_packets(
        &self,
        channel: u8,
        rx_type: u8,
    ) -> [[u8; USB_PACKET_LEN]; RF_SLICES_PER_ENVELOPE] {
        let mut packets = [[0_u8; USB_PACKET_LEN]; RF_SLICES_PER_ENVELOPE];
        for (index, packet) in packets.iter_mut().enumerate() {
            packet[0] = USB_CMD_SEND_RF;
            packet[1] = u8::try_from(index).expect("four slices");
            packet[2] = channel;
            packet[3] = rx_type;
            let start = index * RF_SLICE_LEN;
            packet[USB_RF_HEADER_LEN..].copy_from_slice(&self.0[start..start + RF_SLICE_LEN]);
        }
        packets
    }
}

/// Fan PWM for one cluster: hold-steady upkeep and the bind carrier.
///
/// `slot_index` is the 1-based position of the cluster among the bound
/// devices; `pwm` is one duty byte per fan slot, zero for empty slots.
#[must_use]
pub fn pwm_envelope(
    target: Mac,
    master: Mac,
    rx_type: u8,
    channel: u8,
    slot_index: u8,
    pwm: [u8; 4],
) -> RfEnvelope {
    let mut envelope = RfEnvelope::new(RfSubCommand::Pwm, target, master);
    envelope.0[14] = rx_type;
    envelope.0[15] = channel;
    envelope.0[16] = slot_index;
    envelope.0[17..21].copy_from_slice(&pwm);
    envelope
}

/// Wall-clock fields the clock-sync blob carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WallClock {
    /// Four-digit year.
    pub year: u16,
    /// Month, 1 to 12.
    pub month: u8,
    /// Day of month, 1 to 31.
    pub day: u8,
    /// Hour, 0 to 23.
    pub hour: u8,
    /// Minute, 0 to 59.
    pub minute: u8,
    /// Second, 0 to 59.
    pub second: u8,
}

impl WallClock {
    /// The host's local time, which is what the fan LCD clock themes show.
    #[must_use]
    pub fn now_local() -> Self {
        use chrono::{Datelike, Local, Timelike};

        let now = Local::now();
        Self {
            year: u16::try_from(now.year()).unwrap_or(u16::MAX),
            month: u8::try_from(now.month()).unwrap_or(1),
            day: u8::try_from(now.day()).unwrap_or(1),
            hour: u8::try_from(now.hour()).unwrap_or(0),
            minute: u8::try_from(now.minute()).unwrap_or(0),
            second: u8::try_from(now.second()).unwrap_or(0),
        }
    }
}

/// The 220-byte clock-sync blob: date and time at offsets 32 to 38, sensor
/// fields zeroed (Hypercolor is not a fan-curve product), per-receiver fan
/// blocks left at their defaults.
#[must_use]
pub fn clock_payload(clock: WallClock) -> [u8; CLOCK_PAYLOAD_LEN] {
    let mut payload = [0_u8; CLOCK_PAYLOAD_LEN];
    payload[32..34].copy_from_slice(&clock.year.to_be_bytes());
    payload[34] = clock.month;
    payload[35] = clock.day;
    payload[36] = clock.hour;
    payload[37] = clock.minute;
    payload[38] = clock.second;
    payload
}

/// The broadcast clock-sync envelope.
///
/// The first one a session sends carries the `0x14` "unset" sentinel across
/// the fixed-data region with only the per-receiver blocks filled; every one
/// after it carries the whole blob.
#[must_use]
pub fn clock_sync_envelope(
    master: Mac,
    payload: &[u8; CLOCK_PAYLOAD_LEN],
    first_of_session: bool,
) -> RfEnvelope {
    let mut envelope = RfEnvelope::new(RfSubCommand::ClockSync, [0; 6], master);
    if first_of_session {
        envelope.0[14..64].fill(RfSubCommand::ClockSync as u8);
        envelope.0[64..234].copy_from_slice(&payload[50..CLOCK_PAYLOAD_LEN]);
    } else {
        envelope.0[14..234].copy_from_slice(payload);
    }
    envelope
}

/// Per-LED RGB for one cluster, ready for the radio.
#[derive(Debug, Clone)]
pub struct RgbTransfer {
    /// The header envelope, sent first (and repeated when asked).
    pub header: RfEnvelope,
    /// The data envelopes carrying the compressed payload in order.
    pub data: Vec<RfEnvelope>,
}

/// Build the envelopes for `raw_rgb` (fans in slot order, three bytes per
/// LED, frames concatenated for an animation).
///
/// `effect_index` is an opaque tag the receiver echoes in its device record.
/// `total_frames` is 1 for a live still; more than one uploads a
/// firmware-looped animation stepping every `interval_ms`.
#[must_use]
pub fn rgb_transfer(
    target: Mac,
    master: Mac,
    effect_index: [u8; 4],
    leds_per_fan: u8,
    total_frames: u16,
    interval_ms: u16,
    raw_rgb: &[u8],
) -> RgbTransfer {
    let compressed = tinyuz::compress(raw_rgb, tinyuz::Params::default());
    let data_packets = compressed.len().div_ceil(RGB_CHUNK_LEN);
    // The header counts itself.
    let total_packets = u8::try_from(data_packets + 1).unwrap_or(u8::MAX);

    let mut header = RfEnvelope::new(RfSubCommand::SetRgb, target, master);
    header.0[14..18].copy_from_slice(&effect_index);
    header.0[18] = 0;
    header.0[19] = total_packets;
    let compressed_len = u32::try_from(compressed.len()).unwrap_or(u32::MAX);
    header.0[20..24].copy_from_slice(&compressed_len.to_be_bytes());
    header.0[24] = 0;
    header.0[25..27].copy_from_slice(&total_frames.to_be_bytes());
    header.0[27] = leds_per_fan;
    header.0[32..34].copy_from_slice(&interval_ms.to_be_bytes());

    let data = compressed
        .chunks(RGB_CHUNK_LEN)
        .enumerate()
        .map(|(index, chunk)| {
            let mut envelope = RfEnvelope::new(RfSubCommand::SetRgb, target, master);
            envelope.0[14..18].copy_from_slice(&effect_index);
            envelope.0[18] = u8::try_from(index + 1).unwrap_or(u8::MAX);
            envelope.0[19] = total_packets;
            envelope.0[RGB_DATA_OFFSET..RGB_DATA_OFFSET + chunk.len()].copy_from_slice(chunk);
            envelope
        })
        .collect();

    RgbTransfer { header, data }
}

/// Reverse per-fan runs so fan 0 in layout order lands on the highest wire
/// slot. SL-INF chains that attach on the right wire right to left.
#[must_use]
pub fn reverse_fan_order(raw_rgb: &[u8], leds_per_fan: usize, fan_count: usize) -> Vec<u8> {
    let bytes_per_fan = leds_per_fan * 3;
    if bytes_per_fan == 0 || fan_count <= 1 {
        return raw_rgb.to_vec();
    }
    let mut out = Vec::with_capacity(raw_rgb.len());
    for fan in (0..fan_count).rev() {
        let start = fan * bytes_per_fan;
        let end = (start + bytes_per_fan).min(raw_rgb.len());
        if start < end {
            out.extend_from_slice(&raw_rgb[start..end]);
        }
    }
    let consumed = (fan_count * bytes_per_fan).min(raw_rgb.len());
    out.extend_from_slice(&raw_rgb[consumed..]);
    out
}
