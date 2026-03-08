//! ASUS Aura ENE SMBus protocol helpers.

use std::time::Duration;

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

/// ENE DRAM remap slot-index register.
pub const ENE_DRAM_SLOT_INDEX_REGISTER: u16 = 0x80F8;

/// ENE DRAM remap target-address register.
pub const ENE_DRAM_I2C_ADDRESS_REGISTER: u16 = 0x80F9;

/// ENE maximum SMBus block-write payload.
pub const ENE_BLOCK_WRITE_LIMIT: usize = 3;

/// ENE simple GPU magic word.
pub const AURA_GPU_MAGIC: u16 = 0x1589;

/// Delay between SMBus operations recommended by the spec.
pub const ENE_OPERATION_DELAY: Duration = Duration::from_millis(1);

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
    /// Whether this firmware can expose mode 14.
    pub may_support_mode_14: bool,
}

/// Low-level SMBus operations emitted by ENE helper builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EneSmBusOperation {
    /// `i2c_smbus_write_word_data(register, value)`.
    WriteWordData {
        /// SMBus command/register byte.
        register: u8,
        /// 16-bit value.
        value: u16,
    },
    /// `i2c_smbus_write_byte_data(register, value)`.
    WriteByteData {
        /// SMBus command/register byte.
        register: u8,
        /// Byte payload.
        value: u8,
    },
    /// `i2c_smbus_read_byte_data(register)`.
    ReadByteData {
        /// SMBus command/register byte.
        register: u8,
    },
    /// `i2c_smbus_write_block_data(register, data)`.
    WriteBlockData {
        /// SMBus command/register byte.
        register: u8,
        /// Block payload. Must not exceed [`ENE_BLOCK_WRITE_LIMIT`].
        data: Vec<u8>,
    },
    /// Required recovery delay between bus operations.
    Delay {
        /// Delay duration.
        duration: Duration,
    },
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

/// Resolve one known ENE firmware string to its register layout.
#[must_use]
pub fn lookup_ene_firmware_variant(name: &str) -> Option<EneFirmwareVariant> {
    match name {
        "LED-0116" | "AUMA0-E8K4-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            may_support_mode_14: false,
        }),
        "AUMA0-E6K5-0104" | "AUMA0-E6K5-0105" | "AUMA0-E6K5-0106" | "AUMA0-E6K5-0107"
        | "AUMA0-E6K5-1107" | "AUMA0-E6K5-1110" | "AUMA0-E6K5-1111"
        | "AUMA0-E6K5-1113" => Some(EneFirmwareVariant {
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
            may_support_mode_14: false,
        }),
        "AUMA0-E6K5-0008" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x03,
            may_support_mode_14: false,
        }),
        "DIMM_LED-0102" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            may_support_mode_14: false,
        }),
        "AUDA0-E6K5-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
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
#[must_use]
pub fn ene_read_register_range(register: u16, len: usize) -> Vec<EneSmBusOperation> {
    let mut operations = Vec::with_capacity(len.saturating_mul(3));

    for offset in 0..len {
        let current = register
            .checked_add(u16::try_from(offset).expect("register offset must fit in u16"))
            .expect("register range should not overflow");
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
#[must_use]
pub fn ene_write_register_block(register: u16, data: &[u8]) -> Vec<EneSmBusOperation> {
    if data.is_empty() {
        return Vec::new();
    }

    let chunk_count = data.len().div_ceil(ENE_BLOCK_WRITE_LIMIT);
    let mut operations = Vec::with_capacity(chunk_count.saturating_mul(3));

    for (chunk_index, chunk) in data.chunks(ENE_BLOCK_WRITE_LIMIT).enumerate() {
        let chunk_offset = u16::try_from(chunk_index * ENE_BLOCK_WRITE_LIMIT)
            .expect("ENE block offset must fit in u16");
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
    ((hi as u16) << 8) | (lo as u16)
}
