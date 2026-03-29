//! Lian Li UNI Hub protocol implementations.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

/// Shared report ID for ENE 6K77 feature/output/input reports.
pub const ENE_REPORT_ID: u8 = 0xE0;
/// Shared report ID for TL output/input reports.
pub const TL_REPORT_ID: u8 = 0x01;
/// Recommended inter-command delay for ENE hubs.
pub const ENE_COMMAND_DELAY: Duration = Duration::from_millis(20);

const ENE_FRAME_INTERVAL: Duration = Duration::from_millis(20);
const ENE_SYNC_DELAY: Duration = Duration::from_millis(200);
const ENE_STATIC_EFFECT: u8 = 0x01;
const ENE_STATIC_SPEED: u8 = 0x02;
const ENE_DIRECTION_FORWARD: u8 = 0x00;
const ENE_BRIGHTNESS_FULL: u8 = 0x00;

const TL_PACKET_LEN: usize = 64;
const TL_PAYLOAD_LEN: usize = 58;
const TL_SET_LIGHT_LEN: usize = 20;
const TL_LEDS_PER_FAN: usize = 26;
const TL_LEDS_PER_FAN_U8: u8 = 26;
const TL_LEDS_PER_FAN_U32: u32 = 26;
const TL_MAX_PORTS: usize = 4;
const TL_MAX_FANS_PER_PORT: usize = 10;
const TL_MAX_TOTAL_FANS: usize = 16;
const TL_RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);
const TL_FRAME_INTERVAL: Duration = Duration::from_millis(100);
const TL_EFFECT_STATIC: u8 = 0x01;
const TL_BRIGHTNESS_FULL: u8 = 0x04;
const TL_SPEED_MEDIUM: u8 = 0x02;
const TL_DIRECTION_CLOCKWISE: u8 = 0x00;

const WHITE_LIMIT_THRESHOLD: u16 = 460;
const AL_WHITE_LIMIT: u8 = 153;

const _: () = assert!(
    std::mem::size_of::<EnePacket11>() == 11,
    "EnePacket11 must match the 11-byte SL report size"
);
const _: () = assert!(
    std::mem::size_of::<EnePacket65>() == 65,
    "EnePacket65 must match the 65-byte ENE feature report size"
);
const _: () = assert!(
    std::mem::size_of::<EneOutputPacket146>() == 146,
    "EneOutputPacket146 must match the 146-byte AL output report size"
);
const _: () = assert!(
    std::mem::size_of::<EneOutputPacket353>() == 353,
    "EneOutputPacket353 must match the 353-byte V2/Infinity output report size"
);
const _: () = assert!(
    std::mem::size_of::<TlPacket>() == TL_PACKET_LEN,
    "TlPacket must match the 64-byte TL HID packet size"
);

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct EnePacket11 {
    report_id: u8,
    command: u8,
    subcommand: u8,
    arg0: u8,
    padding: [u8; 7],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct EnePacket65 {
    report_id: u8,
    command: u8,
    subcommand: u8,
    arg0: u8,
    arg1: u8,
    padding: [u8; 60],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct EneOutputPacket146 {
    report_id: u8,
    port: u8,
    payload: [u8; 144],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct EneOutputPacket353 {
    report_id: u8,
    port: u8,
    payload: [u8; 351],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TlPacket {
    report_id: u8,
    command: u8,
    reserved: u8,
    packet_hi: u8,
    packet_lo: u8,
    data_len: u8,
    payload: [u8; TL_PAYLOAD_LEN],
}

/// Supported Lian Li hub families currently wired into the HAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LianLiHubVariant {
    /// UNI FAN SL hub (`0xA100`) — single ring, 11-byte feature packets.
    Sl,
    /// UNI FAN AL hub (`0xA101`) — dual ring, 65-byte feature packets.
    Al,
    /// UNI FAN SL V2 hub (`0xA103`, `0xA105`) — V2 single ring.
    SlV2,
    /// UNI FAN AL V2 hub (`0xA104`) — V2 dual ring.
    AlV2,
    /// UNI FAN SL Infinity hub (`0xA102`) — 4 physical groups, 8 logical channels.
    SlInfinity,
    /// UNI FAN SL Redragon hub (`0xA106`) — SL-compatible single ring.
    SlRedragon,
    /// TL Fan hub (`0x7372`) — framed output/input HID packets.
    TlFan,
}

impl LianLiHubVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sl => "Lian Li UNI Hub SL",
            Self::Al => "Lian Li UNI Hub AL",
            Self::SlV2 => "Lian Li UNI Hub SL V2",
            Self::AlV2 => "Lian Li UNI Hub AL V2",
            Self::SlInfinity => "Lian Li UNI Hub SL Infinity",
            Self::SlRedragon => "Lian Li UNI Hub SL Redragon",
            Self::TlFan => "Lian Li TL Fan Hub",
        }
    }

    #[must_use]
    pub const fn activate_subcommand(self) -> u8 {
        match self {
            Self::Sl | Self::SlRedragon => 0x32,
            Self::Al => 0x40,
            Self::SlV2 | Self::AlV2 | Self::SlInfinity => 0x60,
            Self::TlFan => 0x00,
        }
    }

    #[must_use]
    pub const fn group_count(self) -> u8 {
        4
    }

    #[must_use]
    pub const fn logical_channel_count(self) -> u8 {
        match self {
            Self::SlInfinity => 8,
            Self::TlFan => 0,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn uses_double_port(self) -> bool {
        matches!(self, Self::Al | Self::AlV2 | Self::SlInfinity)
    }

    #[must_use]
    pub const fn is_v2(self) -> bool {
        matches!(self, Self::SlV2 | Self::AlV2)
    }

    #[must_use]
    pub const fn max_fans_per_group(self) -> u8 {
        match self {
            Self::SlV2 | Self::AlV2 => 6,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn leds_per_fan(self) -> u8 {
        match self {
            Self::Al | Self::AlV2 | Self::SlInfinity => 20,
            Self::TlFan => TL_LEDS_PER_FAN_U8,
            _ => 16,
        }
    }

    #[must_use]
    pub const fn color_format(self) -> DeviceColorFormat {
        match self {
            Self::TlFan => DeviceColorFormat::Rgb,
            _ => DeviceColorFormat::Rbg,
        }
    }

    #[must_use]
    pub const fn default_fan_counts(self) -> [u8; 8] {
        let max = self.max_fans_per_group();
        match self {
            Self::SlInfinity => [max, max, max, max, max, max, max, max],
            Self::TlFan => [0; 8],
            _ => [max, max, max, max, 0, 0, 0, 0],
        }
    }

    #[must_use]
    const fn feature_packet_len(self) -> usize {
        match self {
            Self::Sl | Self::SlRedragon => 11,
            _ => 65,
        }
    }
}

/// Pure encoder for modern ENE 6K77 UNI Hub variants.
#[derive(Debug, Clone)]
pub struct Ene6k77Protocol {
    variant: LianLiHubVariant,
    fan_counts: [u8; 8],
}

impl Ene6k77Protocol {
    /// Create a new ENE protocol instance for one modern hub variant.
    #[must_use]
    pub fn new(variant: LianLiHubVariant) -> Self {
        assert!(
            !matches!(variant, LianLiHubVariant::TlFan),
            "use TlFanProtocol for TL hubs"
        );

        Self {
            variant,
            fan_counts: variant.default_fan_counts(),
        }
    }

    /// Override the per-logical-channel fan counts used for zone reporting.
    #[must_use]
    pub const fn with_fan_counts(mut self, fan_counts: [u8; 8]) -> Self {
        self.fan_counts = fan_counts;
        self
    }

    /// Raw ENE RPM query request bytes.
    #[must_use]
    pub const fn rpm_read_request(&self) -> [u8; 3] {
        [ENE_REPORT_ID, 0x50, 0x00]
    }

    /// Raw ENE firmware query request bytes.
    #[must_use]
    pub const fn firmware_read_request(&self) -> [u8; 3] {
        [ENE_REPORT_ID, 0x50, 0x01]
    }

    /// Encode a fixed fan speed write for one physical group.
    #[must_use]
    pub fn encode_fixed_speed(&self, group: u8, percent: u8) -> Option<ProtocolCommand> {
        if group >= self.variant.group_count() {
            return None;
        }

        let mut data = vec![
            ENE_REPORT_ID,
            0x20 | group,
            0x00,
            duty_byte(self.variant, percent),
        ];
        data.resize(self.variant.feature_packet_len(), 0x00);

        Some(ProtocolCommand {
            data,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: ENE_COMMAND_DELAY,
            transfer_type: TransferType::HidReport,
        })
    }

    /// Encode motherboard PWM sync enable/disable for one group.
    #[must_use]
    pub fn encode_pwm_sync(&self, group: u8, enabled: bool) -> Option<ProtocolCommand> {
        let subcommand = match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => 0x31,
            LianLiHubVariant::Al => 0x42,
            LianLiHubVariant::SlV2 | LianLiHubVariant::AlV2 | LianLiHubVariant::SlInfinity => 0x62,
            LianLiHubVariant::TlFan => return None,
        };

        self.encode_sync_packet(subcommand, group, enabled)
    }

    /// Encode motherboard ARGB sync enable/disable for one group.
    #[must_use]
    pub fn encode_argb_sync(&self, group: u8, enabled: bool) -> Option<ProtocolCommand> {
        let subcommand = match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => 0x30,
            LianLiHubVariant::Al => 0x41,
            LianLiHubVariant::SlV2 | LianLiHubVariant::AlV2 | LianLiHubVariant::SlInfinity => 0x61,
            LianLiHubVariant::TlFan => return None,
        };

        self.encode_sync_packet(subcommand, group, enabled)
    }

    /// Parse an ENE RPM response into four big-endian group RPM values.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the response shape does not match the
    /// selected hub variant.
    pub fn parse_rpm_response(&self, data: &[u8]) -> Result<[u16; 4], ProtocolError> {
        let payload = strip_optional_report_id(data, ENE_REPORT_ID);
        let offset = usize::from(self.variant.is_v2());
        let expected_len = if self.variant.is_v2() { 9 } else { 8 };

        if payload.len() < expected_len {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "expected at least {expected_len} bytes for {:?} RPM response, got {}",
                    self.variant,
                    payload.len()
                ),
            });
        }

        Ok(std::array::from_fn(|index| {
            let start = offset + index * 2;
            u16::from_be_bytes([payload[start], payload[start + 1]])
        }))
    }

    /// Parse the 5-byte ENE firmware response into a display string.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the response is too short.
    pub fn parse_firmware_response(&self, data: &[u8]) -> Result<String, ProtocolError> {
        let payload = strip_optional_report_id(data, ENE_REPORT_ID);
        if payload.len() < 5 {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "expected 5 bytes for ENE firmware response, got {}",
                    payload.len()
                ),
            });
        }

        Ok(firmware_version_from_fine_tune(payload[4]))
    }

    fn encode_sync_packet(
        &self,
        subcommand: u8,
        group: u8,
        enabled: bool,
    ) -> Option<ProtocolCommand> {
        if group >= self.variant.group_count() {
            return None;
        }

        let data_byte = (1_u8 << (group + 4)) | (u8::from(enabled) << group);
        let data = match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => {
                let mut packet = EnePacket11::new_zeroed();
                packet.report_id = ENE_REPORT_ID;
                packet.command = 0x10;
                packet.subcommand = subcommand;
                packet.arg0 = data_byte;
                packet.as_bytes().to_vec()
            }
            _ => {
                let mut packet = EnePacket65::new_zeroed();
                packet.report_id = ENE_REPORT_ID;
                packet.command = 0x10;
                packet.subcommand = subcommand;
                packet.arg0 = data_byte;
                packet.as_bytes().to_vec()
            }
        };

        Some(ProtocolCommand {
            data,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: ENE_SYNC_DELAY,
            transfer_type: TransferType::HidReport,
        })
    }

    fn zone_name(&self, logical_channel: usize) -> String {
        match self.variant {
            LianLiHubVariant::SlInfinity => {
                let group = logical_channel / 2 + 1;
                let ring = if logical_channel.is_multiple_of(2) {
                    "Inner"
                } else {
                    "Outer"
                };
                format!("Group {group} {ring}")
            }
            _ => format!("Group {}", logical_channel + 1),
        }
    }

    fn zone_topology(&self, logical_channel: usize) -> DeviceTopologyHint {
        match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlV2 | LianLiHubVariant::SlRedragon => {
                DeviceTopologyHint::Strip
            }
            LianLiHubVariant::Al | LianLiHubVariant::AlV2 | LianLiHubVariant::TlFan => {
                DeviceTopologyHint::Custom
            }
            LianLiHubVariant::SlInfinity => {
                if logical_channel.is_multiple_of(2) {
                    DeviceTopologyHint::Ring {
                        count: u32::from(self.zone_leds_per_fan(logical_channel)),
                    }
                } else {
                    DeviceTopologyHint::Strip
                }
            }
        }
    }

    fn zone_leds_per_fan(&self, logical_channel: usize) -> u8 {
        match self.variant {
            LianLiHubVariant::SlInfinity => {
                if logical_channel.is_multiple_of(2) {
                    8
                } else {
                    12
                }
            }
            _ => self.variant.leds_per_fan(),
        }
    }

    fn logical_zone_led_count(&self, logical_channel: usize) -> u32 {
        let leds_per_fan = u32::from(self.zone_leds_per_fan(logical_channel));
        let fan_count = u32::from(self.fan_counts[logical_channel]);
        leds_per_fan * fan_count
    }

    fn push_feature_packet11(
        encoder: &mut CommandBuffer<'_>,
        command: u8,
        subcommand: u8,
        arg0: u8,
    ) {
        let mut packet = EnePacket11::new_zeroed();
        packet.report_id = ENE_REPORT_ID;
        packet.command = command;
        packet.subcommand = subcommand;
        packet.arg0 = arg0;
        encoder.push_struct(
            &packet,
            false,
            Duration::ZERO,
            ENE_COMMAND_DELAY,
            TransferType::HidReport,
        );
    }

    fn push_feature_packet65(
        encoder: &mut CommandBuffer<'_>,
        command: u8,
        subcommand: u8,
        arg0: u8,
        arg1: u8,
    ) {
        let mut packet = EnePacket65::new_zeroed();
        packet.report_id = ENE_REPORT_ID;
        packet.command = command;
        packet.subcommand = subcommand;
        packet.arg0 = arg0;
        packet.arg1 = arg1;
        encoder.push_struct(
            &packet,
            false,
            Duration::ZERO,
            ENE_COMMAND_DELAY,
            TransferType::HidReport,
        );
    }

    fn push_activate(&self, encoder: &mut CommandBuffer<'_>, group: u8, fan_count: u8) {
        match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => Self::push_feature_packet11(
                encoder,
                0x10,
                self.variant.activate_subcommand(),
                (group << 4) | (fan_count & 0x0F),
            ),
            LianLiHubVariant::Al | LianLiHubVariant::AlV2 | LianLiHubVariant::SlInfinity => {
                Self::push_feature_packet65(
                    encoder,
                    0x10,
                    self.variant.activate_subcommand(),
                    group + 1,
                    fan_count,
                );
            }
            LianLiHubVariant::SlV2 => Self::push_feature_packet65(
                encoder,
                0x10,
                self.variant.activate_subcommand(),
                (group << 4) | (fan_count & 0x0F),
                0x00,
            ),
            LianLiHubVariant::TlFan => {}
        }
    }

    fn push_commit(&self, encoder: &mut CommandBuffer<'_>, port: u8) {
        match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => {
                let mut packet = EnePacket11::new_zeroed();
                packet.report_id = ENE_REPORT_ID;
                packet.command = 0x10 | port;
                packet.subcommand = ENE_STATIC_EFFECT;
                packet.arg0 = ENE_STATIC_SPEED;
                packet.padding[0] = ENE_DIRECTION_FORWARD;
                packet.padding[1] = ENE_BRIGHTNESS_FULL;
                encoder.push_struct(
                    &packet,
                    false,
                    Duration::ZERO,
                    ENE_COMMAND_DELAY,
                    TransferType::HidReport,
                );
            }
            _ => {
                let mut packet = EnePacket65::new_zeroed();
                packet.report_id = ENE_REPORT_ID;
                packet.command = 0x10 | port;
                packet.subcommand = ENE_STATIC_EFFECT;
                packet.arg0 = ENE_STATIC_SPEED;
                packet.arg1 = ENE_DIRECTION_FORWARD;
                packet.padding[0] = ENE_BRIGHTNESS_FULL;
                encoder.push_struct(
                    &packet,
                    false,
                    Duration::ZERO,
                    ENE_COMMAND_DELAY,
                    TransferType::HidReport,
                );
            }
        }
    }

    fn push_frame_commit(&self, encoder: &mut CommandBuffer<'_>) {
        match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => {
                Self::push_feature_packet11(encoder, 0x60, 0x00, 0x01);
            }
            _ => Self::push_feature_packet65(encoder, 0x60, 0x00, 0x01, 0x00),
        }
    }

    fn push_sl_color_data(
        &self,
        encoder: &mut CommandBuffer<'_>,
        port: u8,
        colors: &[[u8; 3]],
        expected_leds: usize,
    ) {
        encoder.push_fill(
            false,
            Duration::ZERO,
            ENE_COMMAND_DELAY,
            TransferType::Primary,
            |buffer| {
                buffer.reserve(2 + expected_leds.saturating_mul(3));
                buffer.push(ENE_REPORT_ID);
                buffer.push(0x30 | port);
                buffer.resize(2 + expected_leds.saturating_mul(3), 0x00);
                write_single_ring_payload(&mut buffer[2..], colors, expected_leds, self.variant);
            },
        );
    }

    fn push_al_color_data(
        &self,
        encoder: &mut CommandBuffer<'_>,
        port: u8,
        colors: &[[u8; 3]],
        fan_count: usize,
        ring: DualRing,
    ) {
        let mut packet = EneOutputPacket146::new_zeroed();
        packet.report_id = ENE_REPORT_ID;
        packet.port = 0x30 | port;
        write_dual_ring_payload(&mut packet.payload, colors, fan_count, ring, self.variant);
        encoder.push_struct(
            &packet,
            false,
            Duration::ZERO,
            ENE_COMMAND_DELAY,
            TransferType::Primary,
        );
    }

    fn push_large_color_data(
        encoder: &mut CommandBuffer<'_>,
        port: u8,
        fill: impl FnOnce(&mut [u8]),
    ) {
        let mut packet = EneOutputPacket353::new_zeroed();
        packet.report_id = ENE_REPORT_ID;
        packet.port = 0x30 | port;
        fill(&mut packet.payload);
        encoder.push_struct(
            &packet,
            false,
            Duration::ZERO,
            ENE_COMMAND_DELAY,
            TransferType::Primary,
        );
    }

    fn encode_single_ring(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let leds_per_fan = usize::from(self.variant.leds_per_fan());
        let group_capacity = leds_per_fan * usize::from(self.variant.max_fans_per_group());
        let mut encoder = CommandBuffer::new(commands);
        let mut wrote_any = false;

        for group in 0..usize::from(self.variant.group_count()) {
            let start = group * group_capacity;
            if start >= colors.len() {
                break;
            }

            let end = colors.len().min(start + group_capacity);
            let group_colors = &colors[start..end];
            let fan_count = group_colors.len().div_ceil(leds_per_fan);
            if fan_count == 0 {
                continue;
            }

            wrote_any = true;
            self.push_activate(
                &mut encoder,
                u8::try_from(group).expect("group index should fit in u8"),
                u8::try_from(fan_count).expect("fan count should fit in u8"),
            );

            match self.variant {
                LianLiHubVariant::Sl | LianLiHubVariant::SlRedragon => self.push_sl_color_data(
                    &mut encoder,
                    u8::try_from(group).expect("group index should fit in u8"),
                    group_colors,
                    fan_count * leds_per_fan,
                ),
                LianLiHubVariant::SlV2 => Self::push_large_color_data(
                    &mut encoder,
                    u8::try_from(group).expect("group index should fit in u8"),
                    |payload| {
                        write_single_ring_payload(
                            payload,
                            group_colors,
                            fan_count * leds_per_fan,
                            self.variant,
                        );
                    },
                ),
                _ => {}
            }

            self.push_commit(
                &mut encoder,
                u8::try_from(group).expect("group index should fit in u8"),
            );
        }

        if wrote_any {
            self.push_frame_commit(&mut encoder);
        }
        encoder.finish();
    }

    fn encode_dual_port_groups(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let group_capacity = usize::from(self.variant.max_fans_per_group()) * 20;
        let mut encoder = CommandBuffer::new(commands);
        let mut wrote_any = false;

        for group in 0..usize::from(self.variant.group_count()) {
            let start = group * group_capacity;
            if start >= colors.len() {
                break;
            }

            let end = colors.len().min(start + group_capacity);
            let group_colors = &colors[start..end];
            let fan_count = group_colors.len().div_ceil(20);
            if fan_count == 0 {
                continue;
            }

            wrote_any = true;
            let group_u8 = u8::try_from(group).expect("group index should fit in u8");
            let fan_count_u8 = u8::try_from(fan_count).expect("fan count should fit in u8");

            for ring in [DualRing::Inner, DualRing::Outer] {
                self.push_activate(&mut encoder, group_u8, fan_count_u8);
                let port = group_u8 * 2 + ring.port_offset();
                match self.variant {
                    LianLiHubVariant::Al => {
                        self.push_al_color_data(&mut encoder, port, group_colors, fan_count, ring);
                    }
                    LianLiHubVariant::AlV2 => {
                        Self::push_large_color_data(&mut encoder, port, |payload| {
                            write_dual_ring_payload(
                                payload,
                                group_colors,
                                fan_count,
                                ring,
                                self.variant,
                            );
                        });
                    }
                    _ => {}
                }
                self.push_commit(&mut encoder, port);
            }
        }

        if wrote_any {
            self.push_frame_commit(&mut encoder);
        }
        encoder.finish();
    }

    fn encode_sl_infinity(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let max_capacities = [32_usize, 48, 32, 48, 32, 48, 32, 48];
        let mut segments = [&[][..]; 8];
        let mut offset = 0_usize;

        for (index, max_capacity) in max_capacities.iter().copied().enumerate() {
            if offset >= colors.len() {
                break;
            }

            let configured_fans = usize::from(self.fan_counts[index]);
            let configured_capacity =
                configured_fans.saturating_mul(usize::from(self.zone_leds_per_fan(index)));
            let capacity = if configured_capacity > 0 {
                configured_capacity
            } else {
                max_capacity
            };
            let end = colors.len().min(offset + capacity);
            segments[index] = &colors[offset..end];
            offset = end;
        }

        let mut encoder = CommandBuffer::new(commands);
        let mut wrote_any = false;

        for group in 0..usize::from(self.variant.group_count()) {
            let inner = segments[group * 2];
            let outer = segments[group * 2 + 1];
            let fan_count = inner.len().div_ceil(8).max(outer.len().div_ceil(12));
            if fan_count == 0 {
                continue;
            }

            wrote_any = true;
            let group_u8 = u8::try_from(group).expect("group index should fit in u8");
            let fan_count_u8 = u8::try_from(fan_count).expect("fan count should fit in u8");

            self.push_activate(&mut encoder, group_u8, fan_count_u8);
            Self::push_large_color_data(&mut encoder, group_u8 * 2, |payload| {
                write_single_ring_payload(payload, inner, fan_count * 8, self.variant);
            });
            self.push_commit(&mut encoder, group_u8 * 2);

            self.push_activate(&mut encoder, group_u8, fan_count_u8);
            Self::push_large_color_data(&mut encoder, group_u8 * 2 + 1, |payload| {
                write_single_ring_payload(payload, outer, fan_count * 12, self.variant);
            });
            self.push_commit(&mut encoder, group_u8 * 2 + 1);
        }

        if wrote_any {
            self.push_frame_commit(&mut encoder);
        }
        encoder.finish();
    }
}

impl Protocol for Ene6k77Protocol {
    fn name(&self) -> &'static str {
        self.variant.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        match self.variant {
            LianLiHubVariant::Sl | LianLiHubVariant::SlV2 | LianLiHubVariant::SlRedragon => {
                self.encode_single_ring(colors, commands);
            }
            LianLiHubVariant::Al | LianLiHubVariant::AlV2 => {
                self.encode_dual_port_groups(colors, commands);
            }
            LianLiHubVariant::SlInfinity => self.encode_sl_infinity(colors, commands),
            LianLiHubVariant::TlFan => commands.clear(),
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let payload = strip_optional_report_id(data, ENE_REPORT_ID);
        if payload.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty ENE response".to_owned(),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: payload.to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let zone_count = usize::from(self.variant.logical_channel_count());
        let mut zones = Vec::with_capacity(zone_count);

        for logical_channel in 0..zone_count {
            zones.push(ProtocolZone {
                name: self.zone_name(logical_channel),
                led_count: self.logical_zone_led_count(logical_channel),
                topology: self.zone_topology(logical_channel),
                color_format: self.variant.color_format(),
            });
        }

        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            max_fps: 30,
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        self.zones().iter().map(|zone| zone.led_count).sum()
    }

    fn frame_interval(&self) -> Duration {
        ENE_FRAME_INTERVAL
    }
}

#[derive(Debug, Clone, Default)]
struct TlFanState {
    port_fan_counts: [u8; TL_MAX_PORTS],
    firmware: Option<String>,
}

/// Framed HID protocol for Lian Li TL fan hubs.
pub struct TlFanProtocol {
    packet_counter: AtomicU16,
    state: RwLock<TlFanState>,
}

impl TlFanProtocol {
    /// Create a new TL Fan protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            packet_counter: AtomicU16::new(0),
            state: RwLock::new(TlFanState::default()),
        }
    }

    /// Override the discovered per-port fan counts, primarily for tests.
    #[must_use]
    pub fn with_port_fan_counts(self, port_fan_counts: [u8; TL_MAX_PORTS]) -> Self {
        *self
            .state
            .write()
            .expect("TL fan state lock should not be poisoned") = TlFanState {
            port_fan_counts,
            firmware: None,
        };
        self
    }

    /// Latest parsed TL firmware string.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.state
            .read()
            .expect("TL fan state lock should not be poisoned")
            .firmware
            .clone()
    }

    /// Current discovered per-port fan counts.
    #[must_use]
    pub fn port_fan_counts(&self) -> [u8; TL_MAX_PORTS] {
        self.state
            .read()
            .expect("TL fan state lock should not be poisoned")
            .port_fan_counts
    }

    fn next_packet_number(&self) -> u16 {
        self.packet_counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn command(&self, command: u8, payload: &[u8], expects_response: bool) -> ProtocolCommand {
        let mut packet = TlPacket::new_zeroed();
        let packet_number = self.next_packet_number();
        let [packet_hi, packet_lo] = packet_number.to_be_bytes();
        packet.report_id = TL_REPORT_ID;
        packet.command = command;
        packet.packet_hi = packet_hi;
        packet.packet_lo = packet_lo;
        packet.data_len = u8::try_from(payload.len()).expect("TL payload length should fit in u8");
        packet.payload[..payload.len()].copy_from_slice(payload);

        ProtocolCommand {
            data: packet.as_bytes().to_vec(),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn handshake_command(&self) -> ProtocolCommand {
        self.command(0xA1, &[], true)
    }

    fn product_info_command(&self) -> ProtocolCommand {
        self.command(0xA6, &[], true)
    }

    /// Encode one TL per-fan PWM duty write.
    #[must_use]
    pub fn encode_fan_speed(&self, port: u8, fan_index: u8, duty: u8) -> Option<ProtocolCommand> {
        if usize::from(port) >= TL_MAX_PORTS || usize::from(fan_index) >= TL_MAX_FANS_PER_PORT {
            return None;
        }

        Some(self.command(0xAA, &[(port << 4) | (fan_index & 0x0F), duty], true))
    }

    fn effective_port_fan_counts(&self, color_len: usize) -> [u8; TL_MAX_PORTS] {
        let discovered = self.port_fan_counts();
        if discovered.iter().any(|count| *count > 0) {
            return discovered;
        }

        infer_tl_fan_counts(color_len)
    }
}

impl Default for TlFanProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for TlFanProtocol {
    fn name(&self) -> &'static str {
        LianLiHubVariant::TlFan.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![self.handshake_command(), self.product_info_command()]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let counts = self.effective_port_fan_counts(colors.len());
        let mut color_offset = 0_usize;
        let mut encoder = CommandBuffer::new(commands);

        for (port, fan_count) in counts.into_iter().enumerate() {
            for fan_index in 0..usize::from(fan_count) {
                if color_offset >= colors.len() {
                    break;
                }

                let end = colors.len().min(color_offset + TL_LEDS_PER_FAN);
                let fan_colors = &colors[color_offset..end];
                color_offset = end;

                let average = average_rgb(fan_colors);
                let mut payload = [0_u8; TL_SET_LIGHT_LEN];
                payload[0] = u8::try_from(port).expect("port index should fit in u8") << 4;
                payload[1] = (u8::try_from(port).expect("port index should fit in u8") << 4)
                    | u8::try_from(fan_index).expect("fan index should fit in u8");
                payload[2] = TL_EFFECT_STATIC;
                payload[3] = TL_BRIGHTNESS_FULL;
                payload[4] = TL_SPEED_MEDIUM;
                payload[5] = average[0];
                payload[6] = average[1];
                payload[7] = average[2];
                payload[17] = TL_DIRECTION_CLOCKWISE;
                payload[18] = 0x00;
                payload[19] = 0x01;

                let mut packet = TlPacket::new_zeroed();
                let packet_number = self.next_packet_number();
                let [packet_hi, packet_lo] = packet_number.to_be_bytes();
                packet.report_id = TL_REPORT_ID;
                packet.command = 0xA3;
                packet.packet_hi = packet_hi;
                packet.packet_lo = packet_lo;
                packet.data_len =
                    u8::try_from(TL_SET_LIGHT_LEN).expect("TL light payload length should fit");
                packet.payload[..TL_SET_LIGHT_LEN].copy_from_slice(&payload);

                encoder.push_struct(
                    &packet,
                    true,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
            }
        }

        encoder.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let (command_index, data_len_index, payload_index) =
            if data.first().copied() == Some(TL_REPORT_ID) {
                (1_usize, 5_usize, 6_usize)
            } else {
                (0_usize, 4_usize, 5_usize)
            };

        if data.len() <= data_len_index {
            return Err(ProtocolError::MalformedResponse {
                detail: format!("TL response too short: {}", data.len()),
            });
        }

        let data_len = usize::from(data[data_len_index]);
        if data.len() < payload_index + data_len {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "TL response declared {data_len} payload bytes but only {} were present",
                    data.len().saturating_sub(payload_index)
                ),
            });
        }

        let command = data[command_index];
        let payload = &data[payload_index..payload_index + data_len];

        match command {
            0xA1 => {
                let mut counts = [0_u8; TL_MAX_PORTS];
                for chunk in payload.chunks_exact(3) {
                    let descriptor = chunk[0];
                    if descriptor & 0x80 == 0 {
                        continue;
                    }

                    let port = usize::from((descriptor >> 4) & 0x03);
                    let fan_index = descriptor & 0x0F;
                    counts[port] = counts[port].max(fan_index.saturating_add(1));
                }

                self.state
                    .write()
                    .expect("TL fan state lock should not be poisoned")
                    .port_fan_counts = counts;
            }
            0xA6 => {
                let firmware = payload
                    .iter()
                    .take_while(|byte| **byte != 0x00)
                    .copied()
                    .collect::<Vec<_>>();
                let firmware = String::from_utf8_lossy(&firmware).trim().to_owned();
                if !firmware.is_empty() {
                    self.state
                        .write()
                        .expect("TL fan state lock should not be poisoned")
                        .firmware = Some(firmware);
                }
            }
            _ => {}
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: payload.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        TL_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let counts = self.port_fan_counts();
        let total_fans: usize = counts.iter().map(|count| usize::from(*count)).sum();
        let mut zones = Vec::with_capacity(total_fans);
        let led_count = TL_LEDS_PER_FAN_U32;

        for (port, fan_count) in counts.into_iter().enumerate() {
            for fan_index in 0..fan_count {
                zones.push(ProtocolZone {
                    name: format!("Port {} Fan {}", port + 1, fan_index + 1),
                    led_count,
                    topology: DeviceTopologyHint::Ring { count: led_count },
                    color_format: DeviceColorFormat::Rgb,
                });
            }
        }

        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            max_fps: 10,
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        self.zones().iter().map(|zone| zone.led_count).sum()
    }

    fn frame_interval(&self) -> Duration {
        TL_FRAME_INTERVAL
    }
}

#[derive(Clone, Copy)]
enum DualRing {
    Inner,
    Outer,
}

impl DualRing {
    const fn port_offset(self) -> u8 {
        match self {
            Self::Inner => 0,
            Self::Outer => 1,
        }
    }
}

/// Apply the recommended sum-based white limiter used by V2 and Infinity hubs.
#[must_use]
pub fn apply_sum_white_limit([r, g, b]: [u8; 3]) -> [u8; 3] {
    let sum = u16::from(r) + u16::from(g) + u16::from(b);
    if sum <= WHITE_LIMIT_THRESHOLD {
        return [r, g, b];
    }

    let threshold = u32::from(WHITE_LIMIT_THRESHOLD);
    let sum = u32::from(sum);
    [
        u8::try_from(u32::from(r) * threshold / sum).expect("scaled red should fit in u8"),
        u8::try_from(u32::from(g) * threshold / sum).expect("scaled green should fit in u8"),
        u8::try_from(u32::from(b) * threshold / sum).expect("scaled blue should fit in u8"),
    ]
}

/// Apply the AL-family equal-channel white clamp.
#[must_use]
pub fn apply_al_white_limit([r, g, b]: [u8; 3]) -> [u8; 3] {
    if r > AL_WHITE_LIMIT && r == g && g == b {
        [AL_WHITE_LIMIT, AL_WHITE_LIMIT, AL_WHITE_LIMIT]
    } else {
        [r, g, b]
    }
}

/// Convert the ENE firmware `fine_tune` byte into a user-facing version.
#[must_use]
pub fn firmware_version_from_fine_tune(fine_tune: u8) -> String {
    if fine_tune < 8 {
        "1.0".to_owned()
    } else {
        let version = f32::from((fine_tune >> 4) * 10 + (fine_tune & 0x0F) + 2) / 10.0;
        format!("{version:.1}")
    }
}

fn infer_tl_fan_counts(color_len: usize) -> [u8; TL_MAX_PORTS] {
    let needed_fans = color_len.div_ceil(TL_LEDS_PER_FAN).min(TL_MAX_TOTAL_FANS);
    let mut counts = [0_u8; TL_MAX_PORTS];
    let mut remaining = needed_fans;

    for count in &mut counts {
        if remaining == 0 {
            break;
        }

        let assigned = remaining.min(TL_MAX_FANS_PER_PORT);
        *count = u8::try_from(assigned).expect("assigned TL fan count should fit in u8");
        remaining -= assigned;
    }

    counts
}

fn average_rgb(colors: &[[u8; 3]]) -> [u8; 3] {
    if colors.is_empty() {
        return [0, 0, 0];
    }

    let (r, g, b) = colors.iter().fold((0_u32, 0_u32, 0_u32), |acc, color| {
        (
            acc.0 + u32::from(color[0]),
            acc.1 + u32::from(color[1]),
            acc.2 + u32::from(color[2]),
        )
    });
    let len = u32::try_from(colors.len()).expect("color length should fit in u32");

    [
        u8::try_from(r / len).expect("averaged red should fit in u8"),
        u8::try_from(g / len).expect("averaged green should fit in u8"),
        u8::try_from(b / len).expect("averaged blue should fit in u8"),
    ]
}

fn strip_optional_report_id(data: &[u8], report_id: u8) -> &[u8] {
    if data.first().copied() == Some(report_id) {
        &data[1..]
    } else {
        data
    }
}

fn duty_byte(variant: LianLiHubVariant, percent: u8) -> u8 {
    let percent = percent.min(100);
    match variant {
        LianLiHubVariant::Sl | LianLiHubVariant::Al | LianLiHubVariant::SlRedragon => {
            if percent == 0 {
                0x28
            } else {
                u8::try_from((800_u32 + 11_u32 * u32::from(percent)) / 19_u32)
                    .expect("duty byte should fit in u8")
            }
        }
        LianLiHubVariant::SlInfinity => {
            if percent == 0 {
                0x0A
            } else {
                u8::try_from((500_u32 + 35_u32 * u32::from(percent)) / 40_u32)
                    .expect("duty byte should fit in u8")
            }
        }
        LianLiHubVariant::SlV2 | LianLiHubVariant::AlV2 => {
            if percent == 0 {
                0x07
            } else {
                u8::try_from((200_u32 + 19_u32 * u32::from(percent)) / 21_u32)
                    .expect("duty byte should fit in u8")
            }
        }
        LianLiHubVariant::TlFan => percent,
    }
}

fn apply_variant_white_limit(variant: LianLiHubVariant, color: [u8; 3]) -> [u8; 3] {
    match variant {
        LianLiHubVariant::Al => apply_al_white_limit(color),
        LianLiHubVariant::SlV2 | LianLiHubVariant::AlV2 | LianLiHubVariant::SlInfinity => {
            apply_sum_white_limit(color)
        }
        _ => color,
    }
}

fn encode_ene_color(buffer: &mut [u8], color: [u8; 3], variant: LianLiHubVariant) {
    let [r, g, b] = apply_variant_white_limit(variant, color);
    buffer[0] = r;
    buffer[1] = b;
    buffer[2] = g;
}

fn write_single_ring_payload(
    output: &mut [u8],
    colors: &[[u8; 3]],
    expected_leds: usize,
    variant: LianLiHubVariant,
) {
    for led_index in 0..expected_leds {
        let color = colors.get(led_index).copied().unwrap_or([0, 0, 0]);
        let start = led_index * 3;
        encode_ene_color(&mut output[start..start + 3], color, variant);
    }
}

fn write_dual_ring_payload(
    output: &mut [u8],
    colors: &[[u8; 3]],
    fan_count: usize,
    ring: DualRing,
    variant: LianLiHubVariant,
) {
    let (led_offset, leds_per_ring) = match ring {
        DualRing::Inner => (0_usize, 8_usize),
        DualRing::Outer => (8_usize, 12_usize),
    };

    for fan_index in 0..fan_count {
        for led_in_ring in 0..leds_per_ring {
            let color_index = fan_index * 20 + led_offset + led_in_ring;
            let color = colors.get(color_index).copied().unwrap_or([0, 0, 0]);
            let start = (fan_index * leds_per_ring + led_in_ring) * 3;
            encode_ene_color(&mut output[start..start + 3], color, variant);
        }
    }
}
