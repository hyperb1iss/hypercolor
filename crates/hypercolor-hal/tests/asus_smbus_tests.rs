use std::time::Duration;

use hypercolor_hal::drivers::asus::smbus::{
    AURA_GPU_MAGIC, AuraSmBusProtocol, ENE_ADDRESS_REGISTER, ENE_APPLY_VAL, ENE_BLOCK_WRITE_LIMIT,
    ENE_BLOCK_WRITE_REGISTER, ENE_DRAM_I2C_ADDRESS_REGISTER, ENE_DRAM_SLOT_INDEX_REGISTER,
    ENE_OPERATION_DELAY, ENE_READ_REGISTER, ENE_SAVE_VAL, EneSmBusOperation,
    decode_ene_transaction, encode_ene_transaction, ene_byte_swap, ene_direct_color_writes,
    ene_dram_remap_sequence, ene_permute_color, ene_read_register_range, ene_write_register,
    ene_write_register_block, lookup_ene_firmware_variant, simple_gpu_magic, supports_mode_14,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};

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

    let dram = lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("dram firmware should exist");
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
    let variant = lookup_ene_firmware_variant("AUMA0-E6K5-0104").expect("variant should exist");
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

    let dram = lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("dram variant should exist");
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

#[test]
fn transaction_codec_round_trips_operations() {
    let operations = vec![
        EneSmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x2080,
        },
        EneSmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        },
        EneSmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        },
        EneSmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![1, 2, 3],
        },
    ];

    let encoded = encode_ene_transaction(&operations).expect("transaction should encode");
    let decoded = decode_ene_transaction(&encoded).expect("transaction should decode");
    assert_eq!(decoded, operations);
}

#[test]
fn smbus_protocol_init_queries_firmware_and_config_then_enables_direct_mode() {
    let protocol = AuraSmBusProtocol::new();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 3);
    assert!(commands[0].expects_response);
    assert!(commands[1].expects_response);
    assert!(!commands[2].expects_response);

    let firmware_ops =
        decode_ene_transaction(&commands[0].data).expect("firmware transaction should decode");
    assert_eq!(firmware_ops, ene_read_register_range(0x1000, 16));

    let config_ops =
        decode_ene_transaction(&commands[1].data).expect("config transaction should decode");
    assert_eq!(config_ops, ene_read_register_range(0x1C00, 64));

    let direct_mode_ops =
        decode_ene_transaction(&commands[2].data).expect("direct-mode transaction should decode");
    assert_eq!(direct_mode_ops, ene_write_register(0x8020, 0x01));
}

#[test]
fn smbus_protocol_parses_firmware_and_config_responses() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUDA0-E6K5-0101");

    let parsed = protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");
    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert_eq!(protocol.firmware_name().as_deref(), Some("AUDA0-E6K5-0101"));

    let mut config = [0_u8; 64];
    config[0x02] = 10;
    config[0x13] = 0x0E;

    let parsed = protocol
        .parse_response(&config)
        .expect("config response should parse");
    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert_eq!(protocol.total_leds(), 10);
    assert!(protocol.supports_mode_14());
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn smbus_protocol_tolerates_non_utf8_noise_in_firmware_name() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..16].copy_from_slice(b"AUDA0\xff-E6K5-0101");

    let parsed = protocol
        .parse_response(&firmware)
        .expect("firmware response with one noisy byte should parse");

    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert_eq!(protocol.firmware_name().as_deref(), Some("AUDA0-E6K5-0101"));
    assert!(protocol.firmware_variant().is_some());
}

#[test]
fn smbus_protocol_frame_encoding_uses_serialized_direct_writes() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUMA0-E6K5-0104");
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");

    let commands = protocol.encode_frame(&[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]]);
    assert_eq!(commands.len(), 1);
    let operations =
        decode_ene_transaction(&commands[0].data).expect("frame transaction should decode");
    let variant = lookup_ene_firmware_variant("AUMA0-E6K5-0104").expect("variant should resolve");
    assert_eq!(
        operations,
        ene_direct_color_writes(variant, &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]])
    );
}
