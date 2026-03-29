//! Legacy libusb control-transfer protocols for early UNI hubs.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};
use crate::transport::vendor::{VendorControlOperation, encode_operations as encode_vendor_ops};

use super::protocol::apply_al_white_limit;

const LEGACY_GROUP_COUNT: usize = 4;
const LEGACY_MAX_FANS_PER_GROUP: usize = 4;
const LEGACY_MAX_FANS_PER_GROUP_U8: u8 = 4;
const ORIGINAL_LEDS_PER_FAN: usize = 16;
const AL10_LEDS_PER_FAN: usize = 20;

const LEGACY_CONTROL_DELAY: Duration = Duration::from_millis(5);
const LEGACY_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const ORIGINAL_FRAME_INTERVAL: Duration = Duration::from_millis(125);
const AL10_FRAME_INTERVAL: Duration = Duration::from_millis(250);

const LEGACY_VENDOR_WRITE_REQUEST: u8 = 0x80;
const LEGACY_VENDOR_READ_REQUEST: u8 = 0x81;
const LEGACY_VENDOR_WVALUE: u16 = 0x0000;
const LEGACY_FIRMWARE_REGISTER: u16 = 0xB500;

const LEGACY_STATIC_EFFECT: u8 = 0x01;
const LEGACY_DIRECTION_FORWARD: u8 = 0x00;
const LEGACY_BRIGHTNESS_FULL: u8 = 0x00;
const ORIGINAL_SPEED_MEDIUM: u8 = 0x02;
const AL10_SPEED_MEDIUM: u8 = 0x00;

const ORIGINAL_GLOBAL_ACTION_REGISTER: u16 = 0xE021;
const ORIGINAL_GLOBAL_COMMIT_REGISTER: u16 = 0xE02F;
const AL10_GLOBAL_ACTION_REGISTER: u16 = 0xE020;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyHubModel {
    Original,
    Al10,
}

#[derive(Debug, Clone, Copy)]
struct LegacyGroupRegisters {
    fan_count_offset: u8,
    color_register: u16,
    mode_register: u16,
    speed_register: u16,
    direction_register: u16,
    brightness_register: u16,
    commit_register: u16,
}

const ORIGINAL_GROUP_REGISTERS: [LegacyGroupRegisters; LEGACY_GROUP_COUNT] = [
    LegacyGroupRegisters {
        fan_count_offset: 0x00,
        color_register: 0xE300,
        mode_register: 0xE021,
        speed_register: 0xE022,
        direction_register: 0xE023,
        brightness_register: 0xE029,
        commit_register: 0xE02F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x10,
        color_register: 0xE3C0,
        mode_register: 0xE031,
        speed_register: 0xE032,
        direction_register: 0xE033,
        brightness_register: 0xE039,
        commit_register: 0xE03F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x20,
        color_register: 0xE480,
        mode_register: 0xE041,
        speed_register: 0xE042,
        direction_register: 0xE043,
        brightness_register: 0xE049,
        commit_register: 0xE04F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x30,
        color_register: 0xE540,
        mode_register: 0xE051,
        speed_register: 0xE052,
        direction_register: 0xE053,
        brightness_register: 0xE059,
        commit_register: 0xE05F,
    },
];

const AL10_GROUP_REGISTERS: [LegacyGroupRegisters; LEGACY_GROUP_COUNT] = [
    LegacyGroupRegisters {
        fan_count_offset: 0x00,
        color_register: 0xE500,
        mode_register: 0xE021,
        speed_register: 0xE022,
        direction_register: 0xE023,
        brightness_register: 0xE029,
        commit_register: 0xE02F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x14,
        color_register: 0xE5F0,
        mode_register: 0xE031,
        speed_register: 0xE032,
        direction_register: 0xE033,
        brightness_register: 0xE039,
        commit_register: 0xE03F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x28,
        color_register: 0xE6E0,
        mode_register: 0xE041,
        speed_register: 0xE042,
        direction_register: 0xE043,
        brightness_register: 0xE049,
        commit_register: 0xE04F,
    },
    LegacyGroupRegisters {
        fan_count_offset: 0x3C,
        color_register: 0xE7D0,
        mode_register: 0xE051,
        speed_register: 0xE052,
        direction_register: 0xE053,
        brightness_register: 0xE059,
        commit_register: 0xE05F,
    },
];

/// Legacy libusb UNI hub protocol for the original hub and AL10 fallback.
#[derive(Debug, Clone)]
pub struct LegacyUniHubProtocol {
    model: LegacyHubModel,
    fan_counts: [u8; LEGACY_GROUP_COUNT],
}

impl LegacyUniHubProtocol {
    /// Create a protocol for the original `0x7750` hub.
    #[must_use]
    pub fn original() -> Self {
        Self {
            model: LegacyHubModel::Original,
            fan_counts: [LEGACY_MAX_FANS_PER_GROUP_U8; LEGACY_GROUP_COUNT],
        }
    }

    /// Create a protocol for the AL10 `0xA101 v1.0` fallback path.
    #[must_use]
    pub fn al10() -> Self {
        Self {
            model: LegacyHubModel::Al10,
            fan_counts: [LEGACY_MAX_FANS_PER_GROUP_U8; LEGACY_GROUP_COUNT],
        }
    }

    /// Override the configured fan counts used for zone sizing and frame splits.
    #[must_use]
    pub const fn with_fan_counts(mut self, fan_counts: [u8; LEGACY_GROUP_COUNT]) -> Self {
        self.fan_counts = fan_counts;
        self
    }

    #[must_use]
    fn protocol_name(&self) -> &'static str {
        match self.model {
            LegacyHubModel::Original => "Lian Li UNI Hub",
            LegacyHubModel::Al10 => "Lian Li UNI Hub AL10",
        }
    }

    #[must_use]
    fn leds_per_fan(&self) -> usize {
        match self.model {
            LegacyHubModel::Original => ORIGINAL_LEDS_PER_FAN,
            LegacyHubModel::Al10 => AL10_LEDS_PER_FAN,
        }
    }

    #[must_use]
    fn frame_interval_hint(&self) -> Duration {
        match self.model {
            LegacyHubModel::Original => ORIGINAL_FRAME_INTERVAL,
            LegacyHubModel::Al10 => AL10_FRAME_INTERVAL,
        }
    }

    #[must_use]
    fn default_speed(&self) -> u8 {
        match self.model {
            LegacyHubModel::Original => ORIGINAL_SPEED_MEDIUM,
            LegacyHubModel::Al10 => AL10_SPEED_MEDIUM,
        }
    }

    #[must_use]
    fn group_registers(&self) -> &'static [LegacyGroupRegisters; LEGACY_GROUP_COUNT] {
        match self.model {
            LegacyHubModel::Original => &ORIGINAL_GROUP_REGISTERS,
            LegacyHubModel::Al10 => &AL10_GROUP_REGISTERS,
        }
    }

    #[must_use]
    fn color_format() -> DeviceColorFormat {
        DeviceColorFormat::Rbg
    }

    #[must_use]
    fn zone_topology(&self) -> DeviceTopologyHint {
        match self.model {
            LegacyHubModel::Original => DeviceTopologyHint::Strip,
            LegacyHubModel::Al10 => DeviceTopologyHint::Custom,
        }
    }

    #[must_use]
    fn firmware_query_command() -> ProtocolCommand {
        Self::vendor_command(
            &[
                Self::read_register(LEGACY_FIRMWARE_REGISTER, 5),
                Self::delay_op(),
            ],
            true,
        )
    }

    fn vendor_command(
        operations: &[VendorControlOperation],
        expects_response: bool,
    ) -> ProtocolCommand {
        let data = encode_vendor_ops(operations)
            .expect("legacy vendor operation sequence should fit transport framing");

        ProtocolCommand {
            data,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: crate::protocol::TransferType::Primary,
        }
    }

    #[must_use]
    fn write_register(index: u16, data: &[u8]) -> VendorControlOperation {
        VendorControlOperation::Write {
            request: LEGACY_VENDOR_WRITE_REQUEST,
            value: LEGACY_VENDOR_WVALUE,
            index,
            data: data.to_vec(),
        }
    }

    #[must_use]
    fn read_register(index: u16, length: u16) -> VendorControlOperation {
        VendorControlOperation::Read {
            request: LEGACY_VENDOR_READ_REQUEST,
            value: LEGACY_VENDOR_WVALUE,
            index,
            length,
        }
    }

    #[must_use]
    fn delay_op() -> VendorControlOperation {
        VendorControlOperation::Delay {
            duration: LEGACY_CONTROL_DELAY,
        }
    }

    fn push_write_op(operations: &mut Vec<VendorControlOperation>, register: u16, payload: &[u8]) {
        operations.push(Self::write_register(register, payload));
        operations.push(Self::delay_op());
    }

    #[must_use]
    fn encoded_fan_count(fan_count: u8) -> u8 {
        if fan_count == 0 {
            0xFF
        } else {
            fan_count.saturating_sub(1)
        }
    }

    #[must_use]
    fn configured_fan_count(&self, group: usize) -> u8 {
        self.fan_counts[group].min(LEGACY_MAX_FANS_PER_GROUP_U8)
    }

    fn append_init_operations(&self, operations: &mut Vec<VendorControlOperation>) {
        match self.model {
            LegacyHubModel::Original => {
                Self::push_write_op(operations, ORIGINAL_GLOBAL_ACTION_REGISTER, &[0x34]);
                Self::push_write_op(operations, ORIGINAL_GLOBAL_COMMIT_REGISTER, &[0x01]);

                for group in self.group_registers().iter().enumerate() {
                    let (index, registers) = group;
                    let configured_fan_count = self.configured_fan_count(index);
                    let effective_fan_count = if configured_fan_count == 0 {
                        Self::encoded_fan_count(1)
                    } else {
                        Self::encoded_fan_count(configured_fan_count)
                    };
                    Self::push_write_op(
                        operations,
                        ORIGINAL_GLOBAL_ACTION_REGISTER,
                        &[0x32, registers.fan_count_offset | effective_fan_count],
                    );
                    Self::push_write_op(operations, ORIGINAL_GLOBAL_COMMIT_REGISTER, &[0x01]);
                }

                Self::push_write_op(operations, ORIGINAL_GLOBAL_ACTION_REGISTER, &[0x30, 0x00]);
                Self::push_write_op(operations, ORIGINAL_GLOBAL_COMMIT_REGISTER, &[0x01]);
            }
            LegacyHubModel::Al10 => {
                let mut init = [0_u8; 16];
                init[0x0F] = 0x01;
                Self::push_write_op(operations, AL10_GLOBAL_ACTION_REGISTER, &init);

                for index in 0..LEGACY_GROUP_COUNT {
                    let mut fan_count_packet = [0_u8; 16];
                    fan_count_packet[0x01] = 0x40;
                    fan_count_packet[0x02] =
                        u8::try_from(index + 1).expect("legacy group index should fit in u8");
                    fan_count_packet[0x03] = self.configured_fan_count(index).max(1);
                    fan_count_packet[0x0F] = 0x01;
                    Self::push_write_op(operations, AL10_GLOBAL_ACTION_REGISTER, &fan_count_packet);
                }
            }
        }
    }

    fn append_original_frame_operations(
        &self,
        operations: &mut Vec<VendorControlOperation>,
        registers: LegacyGroupRegisters,
        colors: &[[u8; 3]],
    ) {
        let mut payload = [0_u8; LEGACY_MAX_FANS_PER_GROUP * ORIGINAL_LEDS_PER_FAN * 3];
        for (index, chunk) in payload.chunks_exact_mut(3).enumerate() {
            let color = colors.get(index).copied().unwrap_or([0, 0, 0]);
            encode_legacy_color(chunk, color, self.model);
        }

        Self::push_write_op(operations, registers.color_register, &payload);
        Self::push_write_op(operations, registers.mode_register, &[LEGACY_STATIC_EFFECT]);
        Self::push_write_op(
            operations,
            registers.speed_register,
            &[self.default_speed()],
        );
        Self::push_write_op(
            operations,
            registers.direction_register,
            &[LEGACY_DIRECTION_FORWARD],
        );
        Self::push_write_op(
            operations,
            registers.brightness_register,
            &[LEGACY_BRIGHTNESS_FULL],
        );
        Self::push_write_op(operations, registers.commit_register, &[0x01]);
    }

    fn append_al10_frame_operations(
        &self,
        operations: &mut Vec<VendorControlOperation>,
        group_index: usize,
        registers: LegacyGroupRegisters,
        colors: &[[u8; 3]],
    ) {
        let fan_count = usize::from(self.configured_fan_count(group_index));

        for fan in 0..LEGACY_MAX_FANS_PER_GROUP {
            let mut inner = [0_u8; 8 * 3];
            for led in 0..8 {
                let color = if fan < fan_count {
                    colors
                        .get(fan * AL10_LEDS_PER_FAN + led)
                        .copied()
                        .unwrap_or([0, 0, 0])
                } else {
                    [0, 0, 0]
                };
                encode_legacy_color(&mut inner[led * 3..led * 3 + 3], color, self.model);
            }
            let fan_offset = u16::try_from(fan).expect("fan index should fit in u16") * 60;
            Self::push_write_op(operations, registers.color_register + fan_offset, &inner);
        }

        let mut config = [0_u8; 16];
        config[0x01] = LEGACY_STATIC_EFFECT;
        config[0x02] = self.default_speed();
        config[0x03] = LEGACY_DIRECTION_FORWARD;
        config[0x09] = LEGACY_BRIGHTNESS_FULL;
        config[0x0F] = 0x01;

        let group_offset =
            u16::try_from(group_index).expect("legacy group index should fit in u16") * 32;
        Self::push_write_op(
            operations,
            AL10_GLOBAL_ACTION_REGISTER + group_offset,
            &config,
        );

        for fan in 0..LEGACY_MAX_FANS_PER_GROUP {
            let mut outer = [0_u8; 12 * 3];
            for led in 0..12 {
                let color = if fan < fan_count {
                    colors
                        .get(fan * AL10_LEDS_PER_FAN + 8 + led)
                        .copied()
                        .unwrap_or([0, 0, 0])
                } else {
                    [0, 0, 0]
                };
                encode_legacy_color(&mut outer[led * 3..led * 3 + 3], color, self.model);
            }
            let fan_offset = u16::try_from(fan).expect("fan index should fit in u16") * 60;
            Self::push_write_op(
                operations,
                registers.color_register + 24 + fan_offset,
                &outer,
            );
        }

        Self::push_write_op(
            operations,
            AL10_GLOBAL_ACTION_REGISTER + 16 + group_offset,
            &config,
        );
    }
}

impl Protocol for LegacyUniHubProtocol {
    fn name(&self) -> &'static str {
        self.protocol_name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut operations = Vec::new();
        self.append_init_operations(&mut operations);
        vec![Self::vendor_command(&operations, false)]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::with_capacity(LEGACY_GROUP_COUNT);
        let mut offset = 0_usize;

        for (group_index, registers) in self.group_registers().iter().copied().enumerate() {
            let group_leds =
                usize::from(self.configured_fan_count(group_index)) * self.leds_per_fan();
            let end = colors.len().min(offset + group_leds);
            let group_colors = &colors[offset..end];
            offset = end;

            let mut operations = Vec::new();
            match self.model {
                LegacyHubModel::Original => {
                    self.append_original_frame_operations(&mut operations, registers, group_colors);
                }
                LegacyHubModel::Al10 => {
                    self.append_al10_frame_operations(
                        &mut operations,
                        group_index,
                        registers,
                        group_colors,
                    );
                }
            }
            commands.push(Self::vendor_command(&operations, false));
        }

        commands
    }

    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        vec![Self::firmware_query_command()]
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty legacy UNI hub response".to_owned(),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        LEGACY_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        self.fan_counts
            .iter()
            .enumerate()
            .map(|(index, fan_count)| ProtocolZone {
                name: format!("Group {}", index + 1),
                led_count: u32::from(*fan_count)
                    * u32::try_from(self.leds_per_fan())
                        .expect("legacy LEDs per fan should fit in u32"),
                topology: self.zone_topology(),
                color_format: Self::color_format(),
            })
            .collect()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            max_fps: 8,
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        self.zones().iter().map(|zone| zone.led_count).sum()
    }

    fn frame_interval(&self) -> Duration {
        self.frame_interval_hint()
    }
}

fn encode_legacy_color(output: &mut [u8], color: [u8; 3], model: LegacyHubModel) {
    let [r, g, b] = match model {
        LegacyHubModel::Original => color,
        LegacyHubModel::Al10 => apply_al_white_limit(color),
    };
    output[0] = r;
    output[1] = b;
    output[2] = g;
}
