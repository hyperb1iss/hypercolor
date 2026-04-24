//! ASUS Aura ENE `SMBus` protocol helpers.

use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use tracing::warn;

use crate::transport::TransportError;
use crate::transport::smbus::{SmBusOperation, decode_operations, encode_operations};

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

/// ENE indirect address register.
pub const ENE_ADDRESS_REGISTER: u8 = 0x00;

/// ENE indirect write-data register.
pub const ENE_WRITE_REGISTER: u8 = 0x01;

/// ENE indirect block-write register.
pub const ENE_BLOCK_WRITE_REGISTER: u8 = 0x03;

/// ENE indirect read-data register.
pub const ENE_READ_REGISTER: u8 = 0x81;

/// ENE apply register value for transient updates.
pub const ENE_APPLY_VAL: u8 = 0x01;

/// ENE apply register value for persistent writes.
pub const ENE_SAVE_VAL: u8 = 0xAA;

/// ENE direct-mode register.
pub const ENE_DIRECT_MODE_REGISTER: u16 = 0x8020;

/// ENE mode register.
pub const ENE_MODE_REGISTER: u16 = 0x8021;

/// ENE speed register.
pub const ENE_SPEED_REGISTER: u16 = 0x8022;

/// ENE direction register.
pub const ENE_DIRECTION_REGISTER: u16 = 0x8023;

/// ENE apply/save register.
pub const ENE_APPLY_REGISTER: u16 = 0x80A0;

/// ENE DRAM frame-apply register used by Aura DIMMs.
pub const ENE_DRAM_COLOR_APPLY_REGISTER: u16 = 0x802F;

/// ENE DRAM remap slot-index register.
pub const ENE_DRAM_SLOT_INDEX_REGISTER: u16 = 0x80F8;

/// ENE DRAM remap target-address register.
pub const ENE_DRAM_I2C_ADDRESS_REGISTER: u16 = 0x80F9;

/// ENE maximum `SMBus` block-write payload.
pub const ENE_BLOCK_WRITE_LIMIT: usize = 3;

/// ENE simple GPU magic word.
pub const AURA_GPU_MAGIC: u16 = 0x1589;

/// Delay between `SMBus` operations recommended by the spec.
pub const ENE_OPERATION_DELAY: Duration = Duration::from_millis(1);

const ENE_FIRMWARE_NAME_LEN: usize = 16;
const ENE_CONFIG_TABLE_LEN: usize = 64;

/// ENE firmware register layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EneFirmwareVariant {
    /// Direct-mode color register base.
    pub direct_reg: u16,
    /// Effect-mode color register base.
    pub effect_reg: u16,
    /// Config-table offset of the channel-id list.
    pub channel_cfg_offset: usize,
    /// Config-table offset of the LED-count byte.
    pub led_count_offset: usize,
    /// Optional register that must be poked after one direct frame upload.
    pub frame_apply_register: Option<u16>,
    /// Whether this firmware can expose mode 14.
    pub may_support_mode_14: bool,
}

/// Low-level `SMBus` operations emitted by ENE helper builders.
/// ASUS ENE `SMBus` operation alias.
pub type EneSmBusOperation = SmBusOperation;

#[derive(Debug, Clone, Default)]
struct AuraSmBusState {
    firmware_name: Option<String>,
    variant: Option<EneFirmwareVariant>,
    led_count: u32,
    supports_mode_14: bool,
}

/// Byte-swap a 16-bit ENE register address for the indirect address port.
#[must_use]
pub const fn ene_byte_swap(register: u16) -> u16 {
    register.rotate_left(8)
}

/// Rearrange RGB into ENE's RBG wire order.
#[must_use]
pub const fn ene_permute_color(r: u8, g: u8, b: u8) -> [u8; 3] {
    [r, b, g]
}

/// Serialize one ENE transaction into the shared `SMBus` transport framing.
///
/// # Errors
///
/// Returns [`ProtocolError`] if an operation cannot be represented by the
/// shared transport format.
pub fn encode_ene_transaction(operations: &[EneSmBusOperation]) -> Result<Vec<u8>, ProtocolError> {
    encode_operations(operations).map_err(|error| protocol_encoding_error(&error))
}

/// Decode one serialized ENE transaction.
///
/// # Errors
///
/// Returns [`ProtocolError`] when the frame is malformed.
pub fn decode_ene_transaction(data: &[u8]) -> Result<Vec<EneSmBusOperation>, ProtocolError> {
    decode_operations(data).map_err(|error| protocol_malformed_response(&error))
}

fn protocol_encoding_error(error: &TransportError) -> ProtocolError {
    ProtocolError::EncodingError {
        detail: error.to_string(),
    }
}

fn protocol_malformed_response(error: &TransportError) -> ProtocolError {
    ProtocolError::MalformedResponse {
        detail: error.to_string(),
    }
}

fn decode_ene_firmware_name(data: &[u8]) -> Result<String, ProtocolError> {
    let filtered = data
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b' '))
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return Err(ProtocolError::MalformedResponse {
            detail: "ENE firmware name did not contain any ASCII identifier bytes".to_owned(),
        });
    }

    let firmware =
        String::from_utf8(filtered).map_err(|error| ProtocolError::MalformedResponse {
            detail: format!("ENE firmware name could not be normalized into UTF-8: {error}"),
        })?;
    let firmware = firmware.trim().to_owned();

    if firmware.is_empty() {
        return Err(ProtocolError::MalformedResponse {
            detail: "ENE firmware name normalized to an empty string".to_owned(),
        });
    }

    Ok(firmware)
}

/// Resolve one known ENE firmware string to its register layout.
#[must_use]
pub fn lookup_ene_firmware_variant(name: &str) -> Option<EneFirmwareVariant> {
    match name {
        "LED-0116" | "AUMA0-E8K4-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: None,
            may_support_mode_14: false,
        }),
        "AUMA0-E6K5-0104" | "AUMA0-E6K5-0105" | "AUMA0-E6K5-0106" | "AUMA0-E6K5-0107"
        | "AUMA0-E6K5-1107" | "AUMA0-E6K5-1110" | "AUMA0-E6K5-1111" | "AUMA0-E6K5-1113" => {
            Some(EneFirmwareVariant {
                direct_reg: 0x8100,
                effect_reg: 0x8160,
                channel_cfg_offset: 0x1B,
                led_count_offset: if name == "AUMA0-E6K5-0107"
                    || name == "AUMA0-E6K5-1107"
                    || name == "AUMA0-E6K5-1110"
                    || name == "AUMA0-E6K5-1111"
                    || name == "AUMA0-E6K5-1113"
                {
                    0x03
                } else {
                    0x02
                },
                frame_apply_register: None,
                may_support_mode_14: false,
            })
        }
        "AUMA0-E6K5-0008" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x03,
            frame_apply_register: None,
            may_support_mode_14: false,
        }),
        "DIMM_LED-0102" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: Some(ENE_DRAM_COLOR_APPLY_REGISTER),
            may_support_mode_14: false,
        }),
        "AUDA0-E6K5-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: Some(ENE_DRAM_COLOR_APPLY_REGISTER),
            may_support_mode_14: true,
        }),
        _ => None,
    }
}

/// Whether the config-table channel list enables ENE hardware mode 14.
#[must_use]
pub fn supports_mode_14(config_table: &[u8], variant: EneFirmwareVariant) -> bool {
    variant.may_support_mode_14
        && config_table
            .get(variant.channel_cfg_offset..)
            .is_some_and(|channels| channels.contains(&0x0E))
}

/// Build the indirect-read sequence for one ENE register range.
///
/// Returns an empty vec if `len` is zero or if the requested range would
/// overflow the 16-bit ENE address space. The overflow case is logged at
/// warn level so a misconfigured caller is observable without panicking.
#[must_use]
pub fn ene_read_register_range(register: u16, len: usize) -> Vec<EneSmBusOperation> {
    if len == 0 {
        return Vec::new();
    }

    let Ok(len_u16) = u16::try_from(len) else {
        warn!(
            register = %format_args!("{register:#06X}"),
            len,
            "ENE register read length exceeds u16::MAX; dropping request"
        );
        return Vec::new();
    };

    let Some(last_offset) = len_u16.checked_sub(1) else {
        return Vec::new();
    };

    if register.checked_add(last_offset).is_none() {
        warn!(
            register = %format_args!("{register:#06X}"),
            len,
            "ENE register read range overflows u16::MAX; dropping request"
        );
        return Vec::new();
    }

    let mut operations = Vec::with_capacity(len.saturating_mul(3));

    for offset in 0..len_u16 {
        let current = register + offset;
        operations.push(EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(current),
        });
        operations.push(EneSmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        });
        operations.push(EneSmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        });
    }

    operations
}

/// Build the indirect-write sequence for one ENE register byte.
#[must_use]
pub fn ene_write_register(register: u16, value: u8) -> Vec<EneSmBusOperation> {
    vec![
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(register),
        },
        EneSmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        },
        EneSmBusOperation::WriteByteData {
            register: ENE_WRITE_REGISTER,
            value,
        },
    ]
}

/// Build the indirect block-write sequence for an ENE register range.
///
/// Returns an empty vec if `data` is empty or if the requested range would
/// overflow the 16-bit ENE address space. The overflow case is logged at
/// warn level so a misconfigured caller is observable without panicking.
#[must_use]
pub fn ene_write_register_block(register: u16, data: &[u8]) -> Vec<EneSmBusOperation> {
    if data.is_empty() {
        return Vec::new();
    }

    let chunk_count = data.len().div_ceil(ENE_BLOCK_WRITE_LIMIT);
    let max_offset_usize = (chunk_count - 1).saturating_mul(ENE_BLOCK_WRITE_LIMIT);

    let Ok(max_offset) = u16::try_from(max_offset_usize) else {
        warn!(
            register = %format_args!("{register:#06X}"),
            data_len = data.len(),
            "ENE block write length exceeds u16::MAX; dropping request"
        );
        return Vec::new();
    };

    if register.checked_add(max_offset).is_none() {
        warn!(
            register = %format_args!("{register:#06X}"),
            data_len = data.len(),
            "ENE block write range overflows u16::MAX; dropping request"
        );
        return Vec::new();
    }

    let mut operations = Vec::with_capacity(chunk_count.saturating_mul(3));

    for (chunk_index, chunk) in data.chunks(ENE_BLOCK_WRITE_LIMIT).enumerate() {
        let chunk_offset = u16::try_from(chunk_index * ENE_BLOCK_WRITE_LIMIT).unwrap_or(max_offset);
        operations.push(EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(register + chunk_offset),
        });
        operations.push(EneSmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        });
        operations.push(EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: chunk.to_vec(),
        });
    }

    operations
}

/// Build a direct-color write sequence using the firmware's register layout.
#[must_use]
pub fn ene_direct_color_writes(
    variant: EneFirmwareVariant,
    colors: &[[u8; 3]],
) -> Vec<EneSmBusOperation> {
    let payload = colors
        .iter()
        .flat_map(|color| ene_permute_color(color[0], color[1], color[2]))
        .collect::<Vec<_>>();

    ene_write_register_block(variant.direct_reg, &payload)
}

/// Build the DRAM remap write sequence for one slot/address pair.
#[must_use]
pub fn ene_dram_remap_sequence(slot_index: u8, target_address: u8) -> Vec<EneSmBusOperation> {
    let mut operations = ene_write_register(ENE_DRAM_SLOT_INDEX_REGISTER, slot_index);
    operations.extend(ene_write_register(
        ENE_DRAM_I2C_ADDRESS_REGISTER,
        target_address << 1,
    ));
    operations
}

/// Build the simple Aura GPU magic word from the two probe registers.
#[must_use]
pub const fn simple_gpu_magic(hi: u8, lo: u8) -> u16 {
    u16::from_be_bytes([hi, lo])
}

/// ASUS Aura ENE `SMBus` protocol with runtime firmware/config parsing.
pub struct AuraSmBusProtocol {
    state: RwLock<AuraSmBusState>,
}

impl AuraSmBusProtocol {
    /// Create a fresh ENE `SMBus` protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(AuraSmBusState::default()),
        }
    }

    fn command(operations: &[EneSmBusOperation], expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: encode_ene_transaction(operations)
                .expect("ENE transaction encoding should succeed for built-in operations"),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    /// Snapshot the currently parsed firmware string.
    #[must_use]
    pub fn firmware_name(&self) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .firmware_name
            .clone()
    }

    /// Snapshot the currently resolved firmware layout.
    #[must_use]
    pub fn firmware_variant(&self) -> Option<EneFirmwareVariant> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .variant
    }

    /// Whether the parsed config table exposes mode 14.
    #[must_use]
    pub fn supports_mode_14(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .supports_mode_14
    }
}

impl Default for AuraSmBusProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for AuraSmBusProtocol {
    fn name(&self) -> &'static str {
        "ASUS Aura ENE SMBus"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            Self::command(
                &ene_read_register_range(0x1000, ENE_FIRMWARE_NAME_LEN),
                true,
            ),
            Self::command(&ene_read_register_range(0x1C00, ENE_CONFIG_TABLE_LEN), true),
            Self::command(
                &ene_write_register(ENE_DIRECT_MODE_REGISTER, ENE_APPLY_VAL),
                false,
            ),
        ]
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
        let Some(variant) = self.firmware_variant() else {
            commands.truncate(0);
            return;
        };

        let mut operations = Vec::new();
        if variant.frame_apply_register.is_some() {
            operations.extend(ene_write_register(ENE_DIRECT_MODE_REGISTER, ENE_APPLY_VAL));
        }
        operations.extend(ene_direct_color_writes(variant, colors));
        if let Some(register) = variant.frame_apply_register {
            operations.extend(ene_write_register(register, ENE_APPLY_VAL));
        }
        let mut command_buffer = CommandBuffer::new(commands);
        if operations.is_empty() {
            command_buffer.finish();
        } else {
            let data = encode_ene_transaction(&operations)
                .expect("ENE transaction encoding should succeed for built-in operations");
            command_buffer.push_slice(
                &data,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            command_buffer.finish();
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        match data.len() {
            ENE_FIRMWARE_NAME_LEN => {
                let firmware = decode_ene_firmware_name(data)?;
                let variant = lookup_ene_firmware_variant(&firmware);

                let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
                state.firmware_name = Some(firmware.clone());
                state.variant = variant;

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: firmware.into_bytes(),
                })
            }
            ENE_CONFIG_TABLE_LEN => {
                let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
                let Some(variant) = state.variant else {
                    return Err(ProtocolError::MalformedResponse {
                        detail: "ENE config table arrived before firmware variant was known"
                            .to_owned(),
                    });
                };

                let led_count = data.get(variant.led_count_offset).copied().ok_or_else(|| {
                    ProtocolError::MalformedResponse {
                        detail: "ENE config table missing LED count byte".to_owned(),
                    }
                })?;
                state.led_count = u32::from(led_count);
                state.supports_mode_14 = supports_mode_14(data, variant);

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: data.to_vec(),
                })
            }
            _ => Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data.to_vec(),
            }),
        }
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);

        if state.led_count == 0 {
            return Vec::new();
        }

        vec![ProtocolZone {
            name: "Lighting".to_owned(),
            led_count: state.led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .led_count
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}
