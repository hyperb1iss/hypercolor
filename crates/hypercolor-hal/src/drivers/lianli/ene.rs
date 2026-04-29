//! ENE 6K77 UNI Hub protocol encoder.
//!
//! Covers the modern ENE-based Lian Li hubs: SL, AL, SL V2, AL V2, SL Infinity,
//! and SL Redragon. All six share the same wire envelope (report ID `0xE0`,
//! feature packets for control, large output reports for color data) and
//! differ only in packet sizes, subcommands, and ring topology.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceTopologyHint};
use zerocopy::{FromZeros, IntoBytes};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

use super::common::{
    ENE_COMMAND_DELAY, ENE_REPORT_ID, EneOutputPacket146, EneOutputPacket353, EnePacket11,
    EnePacket65, LianLiHubVariant, apply_al_white_limit, apply_sum_white_limit,
    firmware_version_from_fine_tune, strip_optional_report_id,
};

const ENE_FRAME_INTERVAL: Duration = Duration::from_millis(20);
const ENE_SYNC_DELAY: Duration = Duration::from_millis(200);
const ENE_STATIC_EFFECT: u8 = 0x01;
const ENE_STATIC_SPEED: u8 = 0x02;
const ENE_DIRECTION_FORWARD: u8 = 0x00;
const ENE_BRIGHTNESS_FULL: u8 = 0x00;

/// Pure encoder for modern ENE 6K77 UNI Hub variants.
#[derive(Debug, Clone)]
pub struct Ene6k77Protocol {
    variant: LianLiHubVariant,
    fan_counts: [u8; 8],
}

impl Ene6k77Protocol {
    /// Create a new ENE protocol instance for one modern hub variant.
    ///
    /// # Panics
    ///
    /// Panics if constructed with [`LianLiHubVariant::TlFan`]. TL hubs use
    /// [`super::tl::TlFanProtocol`] instead — they have a fundamentally
    /// different wire format.
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
                layout_hint: None,
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
