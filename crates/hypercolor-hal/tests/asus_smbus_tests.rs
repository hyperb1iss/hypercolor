use std::time::Duration;

use hypercolor_hal::drivers::asus::smbus::{
    AURA_GPU_MAGIC, ENE_ADDRESS_REGISTER, ENE_APPLY_VAL, ENE_BLOCK_WRITE_LIMIT,
    ENE_BLOCK_WRITE_REGISTER, ENE_DRAM_I2C_ADDRESS_REGISTER, ENE_DRAM_SLOT_INDEX_REGISTER,
    ENE_OPERATION_DELAY, ENE_READ_REGISTER, ENE_SAVE_VAL, EneSmBusOperation,
    ene_byte_swap, ene_direct_color_writes, ene_dram_remap_sequence, ene_permute_color,
    ene_read_register_range, ene_write_register, ene_write_register_block,
    lookup_ene_firmware_variant, simple_gpu_magic, supports_mode_14,
};

#[test]
fn byte_swap_matches_ene_indirect_addressing() {
    assert_eq!(ene_byte_swap(0x1000), 0x0010);
    assert_eq!(ene_byte_swap(0x80A0), 0xA080);
    assert_eq!(ene_byte_swap(0x1C00), 0x001C);
}

#[test]
fn lookup_firmware_variant_covers_known_usb_and_dram_firmwares() {
    let v1 = lookup_ene_firmware_variant("LED-0116").expect("v1 firmware should exist");
    assert_eq!(v1.direct_reg, 0x8000);
    assert_eq!(v1.effect_reg, 0x8010);
    assert_eq!(v1.channel_cfg_offset, 0x13);
    assert_eq!(v1.led_count_offset, 0x02);
    assert!(!v1.may_support_mode_14);

    let hybrid =
        lookup_ene_firmware_variant("AUMA0-E6K5-0008").expect("hybrid firmware should exist");
    assert_eq!(hybrid.direct_reg, 0x8100);
    assert_eq!(hybrid.effect_reg, 0x8160);
    assert_eq!(hybrid.channel_cfg_offset, 0x13);
    assert_eq!(hybrid.led_count_offset, 0x03);

    let dram =
        lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("dram firmware should exist");
    assert_eq!(dram.direct_reg, 0x8100);
    assert!(dram.may_support_mode_14);
}

#[test]
fn ene_color_order_is_rbg_for_all_variants() {
    assert_eq!(ene_permute_color(0x12, 0x34, 0x56), [0x12, 0x56, 0x34]);
}

#[test]
fn read_register_range_emits_indirect_address_then_read_pairs() {
    let operations = ene_read_register_range(0x1000, 2);
    assert_eq!(operations.len(), 6);

    assert_eq!(
        operations[0],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0010,
        }
    );
    assert_eq!(
        operations[1],
        EneSmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        }
    );
    assert_eq!(
        operations[2],
        EneSmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        }
    );
    assert_eq!(
        operations[3],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0110,
        }
    );
}

#[test]
fn write_register_emits_indirect_address_then_write() {
    let operations = ene_write_register(0x8020, 0x01);
    assert_eq!(operations.len(), 3);
    assert_eq!(
        operations[0],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x2080,
        }
    );
    assert_eq!(
        operations[1],
        EneSmBusOperation::Delay {
            duration: Duration::from_millis(1),
        }
    );
    assert_eq!(
        operations[2],
        EneSmBusOperation::WriteByteData {
            register: 0x01,
            value: 0x01,
        }
    );
}

#[test]
fn block_write_chunks_payloads_at_three_bytes() {
    let operations = ene_write_register_block(0x8100, &[1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(operations.len(), 9);

    assert_eq!(
        operations[0],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0081,
        }
    );
    assert_eq!(
        operations[2],
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![1, 2, 3],
        }
    );
    assert_eq!(
        operations[3],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0381,
        }
    );
    assert_eq!(
        operations[5],
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![4, 5, 6],
        }
    );
    assert_eq!(
        operations[8],
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![7],
        }
    );
}

#[test]
fn direct_color_writes_use_rbg_payloads() {
    let variant =
        lookup_ene_firmware_variant("AUMA0-E6K5-0104").expect("variant should exist");
    let operations = ene_direct_color_writes(variant, &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]]);

    assert_eq!(operations.len(), 6);
    assert_eq!(
        operations[2],
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![0x10, 0x30, 0x20],
        }
    );
    assert_eq!(
        operations[5],
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![0xAA, 0xCC, 0xBB],
        }
    );
}

#[test]
fn mode_14_requires_dram_3_channel_and_capable_firmware() {
    let mut config = [0_u8; 64];
    config[0x13] = 0x0E;

    let dram =
        lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("dram variant should exist");
    let v2 = lookup_ene_firmware_variant("AUMA0-E6K5-0104").expect("v2 variant should exist");

    assert!(supports_mode_14(&config, dram));
    assert!(!supports_mode_14(&config, v2));

    config[0x13] = 0x82;
    assert!(!supports_mode_14(&config, dram));
}

#[test]
fn dram_remap_sequence_programs_slot_then_shifted_address() {
    let operations = ene_dram_remap_sequence(3, 0x39);
    assert_eq!(operations.len(), 6);
    assert_eq!(
        operations[0],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(ENE_DRAM_SLOT_INDEX_REGISTER),
        }
    );
    assert_eq!(
        operations[2],
        EneSmBusOperation::WriteByteData {
            register: 0x01,
            value: 3,
        }
    );
    assert_eq!(
        operations[3],
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(ENE_DRAM_I2C_ADDRESS_REGISTER),
        }
    );
    assert_eq!(
        operations[5],
        EneSmBusOperation::WriteByteData {
            register: 0x01,
            value: 0x72,
        }
    );
}

#[test]
fn apply_and_save_values_match_spec() {
    assert_eq!(ENE_APPLY_VAL, 0x01);
    assert_eq!(ENE_SAVE_VAL, 0xAA);
}

#[test]
fn simple_gpu_magic_matches_probe_values() {
    assert_eq!(simple_gpu_magic(0x15, 0x89), AURA_GPU_MAGIC);
}

#[test]
fn block_write_limit_matches_smbus_cap() {
    assert_eq!(ENE_BLOCK_WRITE_LIMIT, 3);
}
