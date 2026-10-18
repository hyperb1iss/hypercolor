use std::time::Duration;

use hypercolor_hal::drivers::asus::smbus::{
    AURA_GPU_MAGIC, AuraSmBusProtocol, ENE_ADDRESS_REGISTER, ENE_APPLY_VAL, ENE_BLOCK_WRITE_LIMIT,
    ENE_BLOCK_WRITE_REGISTER, ENE_DRAM_COLOR_APPLY_REGISTER, ENE_DRAM_I2C_ADDRESS_REGISTER,
    ENE_DRAM_SLOT_INDEX_REGISTER, ENE_FULL_WRITE_INTERVAL, ENE_OPERATION_DELAY, ENE_READ_REGISTER,
    ENE_SAVE_VAL, decode_ene_transaction, encode_ene_transaction, ene_byte_swap,
    ene_direct_color_writes, ene_direct_led_write, ene_dram_remap_sequence, ene_permute_color,
    ene_read_register_range, ene_write_register, ene_write_register_block,
    lookup_ene_firmware_variant, simple_gpu_magic, supports_mode_14,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus, TransferType};
use hypercolor_hal::transport::smbus::SmBusOperation;

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
    assert_eq!(
        dram.frame_apply_register,
        Some(ENE_DRAM_COLOR_APPLY_REGISTER)
    );
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
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0010,
        }
    );
    assert_eq!(
        operations[1],
        SmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        }
    );
    assert_eq!(
        operations[2],
        SmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        }
    );
    assert_eq!(
        operations[3],
        SmBusOperation::WriteWordData {
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
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x2080,
        }
    );
    assert_eq!(
        operations[1],
        SmBusOperation::Delay {
            duration: Duration::from_millis(1),
        }
    );
    assert_eq!(
        operations[2],
        SmBusOperation::WriteByteData {
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
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0081,
        }
    );
    assert_eq!(
        operations[2],
        SmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![1, 2, 3],
        }
    );
    assert_eq!(
        operations[3],
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x0381,
        }
    );
    assert_eq!(
        operations[5],
        SmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![4, 5, 6],
        }
    );
    assert_eq!(
        operations[8],
        SmBusOperation::WriteBlockData {
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
        SmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: vec![0x10, 0x30, 0x20],
        }
    );
    assert_eq!(
        operations[5],
        SmBusOperation::WriteBlockData {
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
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(ENE_DRAM_SLOT_INDEX_REGISTER),
        }
    );
    assert_eq!(
        operations[2],
        SmBusOperation::WriteByteData {
            register: 0x01,
            value: 3,
        }
    );
    assert_eq!(
        operations[3],
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(ENE_DRAM_I2C_ADDRESS_REGISTER),
        }
    );
    assert_eq!(
        operations[5],
        SmBusOperation::WriteByteData {
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
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: 0x2080,
        },
        SmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        },
        SmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        },
        SmBusOperation::WriteBlockData {
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
    let mut commands_into = Vec::new();
    protocol.encode_frame_into(
        &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]],
        &mut commands_into,
    );
    assert_eq!(commands_into.len(), commands.len());
    assert_eq!(commands_into[0].data, commands[0].data);
    assert_eq!(commands.len(), 1);
    let operations =
        decode_ene_transaction(&commands[0].data).expect("frame transaction should decode");
    let variant = lookup_ene_firmware_variant("AUMA0-E6K5-0104").expect("variant should resolve");
    assert_eq!(
        operations,
        ene_direct_color_writes(variant, &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]])
    );
}

#[test]
fn smbus_protocol_encode_frame_into_reuses_existing_command_buffers() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUMA0-E6K5-0104");
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");
    let mut commands = vec![ProtocolCommand {
        data: Vec::with_capacity(256),
        expects_response: true,
        response_delay: Duration::from_millis(7),
        post_delay: Duration::from_millis(9),
        transfer_type: TransferType::Bulk,
        ..Default::default()
    }];
    let original_ptr = commands[0].data.as_ptr();

    protocol.encode_frame_into(&[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]], &mut commands);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data.as_ptr(), original_ptr);
    assert!(!commands[0].expects_response);
    assert_eq!(commands[0].transfer_type, TransferType::Primary);
}

#[test]
fn smbus_protocol_encode_frame_into_drops_stale_commands_without_firmware() {
    let protocol = AuraSmBusProtocol::new();
    let mut commands = vec![ProtocolCommand {
        data: vec![0xAA],
        expects_response: true,
        response_delay: Duration::from_millis(7),
        post_delay: Duration::from_millis(9),
        transfer_type: TransferType::Bulk,
        ..Default::default()
    }];

    protocol.encode_frame_into(&[[0x10, 0x20, 0x30]], &mut commands);

    assert!(commands.is_empty());
}

#[test]
fn smbus_protocol_encode_frame_into_drops_stale_commands_for_empty_frames() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUMA0-E6K5-0104");
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");
    let mut commands = vec![ProtocolCommand {
        data: vec![0xAA],
        expects_response: true,
        response_delay: Duration::from_millis(7),
        post_delay: Duration::from_millis(9),
        transfer_type: TransferType::Bulk,
        ..Default::default()
    }];

    protocol.encode_frame_into(&[], &mut commands);

    assert!(commands.is_empty());
}

#[test]
fn smbus_protocol_frame_encoding_reasserts_direct_mode_and_appends_dram_apply_latch() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUDA0-E6K5-0101");
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");

    let commands = protocol.encode_frame(&[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]]);
    let mut commands_into = Vec::new();
    protocol.encode_frame_into(
        &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]],
        &mut commands_into,
    );
    assert_eq!(commands_into.len(), commands.len());
    assert_eq!(commands_into[0].data, commands[0].data);
    assert_eq!(commands.len(), 1);
    let operations =
        decode_ene_transaction(&commands[0].data).expect("frame transaction should decode");
    let variant = lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("variant should resolve");

    let mut expected = ene_write_register(0x8020, 0x01);
    expected.extend(ene_direct_color_writes(
        variant,
        &[[0x10, 0x20, 0x30], [0xAA, 0xBB, 0xCC]],
    ));
    expected.extend(ene_write_register(
        ENE_DRAM_COLOR_APPLY_REGISTER,
        ENE_APPLY_VAL,
    ));

    assert_eq!(operations, expected);
}

fn count_delays(operations: &[SmBusOperation]) -> usize {
    operations
        .iter()
        .filter(|operation| matches!(operation, SmBusOperation::Delay { .. }))
        .count()
}

/// Each ENE delay costs a full bus-arbiter hold and, on Windows, a broker round
/// trip, so the delay count is the transport's real cost model rather than an
/// encoding detail. An eight-LED DIMM sits close to the 16ms frame interval
/// already; a regression in chunking or latch order would blow the budget
/// silently.
#[test]
fn dram_frame_and_init_delay_counts_stay_within_the_transport_cost_model() {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..15].copy_from_slice(b"AUDA0-E6K5-0101");
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");

    let colors = [[0x11, 0x22, 0x33]; 8];
    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1, "a frame must stay one transaction");
    let frame_operations =
        decode_ene_transaction(&commands[0].data).expect("frame transaction should decode");

    // 1 direct-mode re-assert + 24 payload bytes / 3-byte chunks + 1 apply latch.
    assert_eq!(count_delays(&frame_operations), 10);

    let init_delays = protocol
        .init_sequence()
        .iter()
        .map(|command| {
            count_delays(&decode_ene_transaction(&command.data).expect("init should decode"))
        })
        .sum::<usize>();

    // 16-byte firmware read + 64-byte config read + 1 direct-mode write.
    assert_eq!(init_delays, 81);
}

fn protocol_with_firmware(firmware_name: &[u8], led_count: u8) -> AuraSmBusProtocol {
    let protocol = AuraSmBusProtocol::new();
    let mut firmware = [0_u8; 16];
    firmware[..firmware_name.len()].copy_from_slice(firmware_name);
    protocol
        .parse_response(&firmware)
        .expect("firmware response should parse");
    let mut config = [0_u8; 64];
    config[0x02] = led_count;
    protocol
        .parse_response(&config)
        .expect("config response should parse");
    protocol
}

fn dram_protocol() -> AuraSmBusProtocol {
    protocol_with_firmware(b"AUDA0-E6K5-0101", 8)
}

fn frame_operations(protocol: &AuraSmBusProtocol, colors: &[[u8; 3]]) -> Vec<SmBusOperation> {
    let mut commands = Vec::new();
    protocol.encode_frame_into(colors, &mut commands);
    assert!(commands.len() <= 1, "a frame must stay one transaction");
    commands.first().map_or_else(Vec::new, |command| {
        decode_ene_transaction(&command.data).expect("frame transaction should decode")
    })
}

/// Every color block write in a frame, as (ENE register, wire bytes).
fn color_writes(operations: &[SmBusOperation]) -> Vec<(u16, Vec<u8>)> {
    let mut pointer = None;
    let mut writes = Vec::new();
    for operation in operations {
        match operation {
            SmBusOperation::WriteWordData {
                register: ENE_ADDRESS_REGISTER,
                value,
            } => pointer = Some(ene_byte_swap(*value)),
            SmBusOperation::WriteBlockData {
                register: ENE_BLOCK_WRITE_REGISTER,
                data,
            } => writes.push((
                pointer.expect("a block write must follow a pointer write"),
                data.clone(),
            )),
            _ => {}
        }
    }
    writes
}

fn register_writes(operations: &[SmBusOperation], register: u16) -> usize {
    operations
        .iter()
        .filter(|operation| {
            **operation
                == SmBusOperation::WriteWordData {
                    register: ENE_ADDRESS_REGISTER,
                    value: ene_byte_swap(register),
                }
        })
        .count()
}

fn steady_frame() -> Vec<[u8; 3]> {
    vec![[0x10, 0x20, 0x30]; 8]
}

#[test]
fn single_led_write_matches_its_chunk_of_the_full_frame() {
    let variant = lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("variant should resolve");
    let colors = (0_u8..8)
        .map(|index| [index, index.wrapping_mul(3), index.wrapping_mul(7)])
        .collect::<Vec<_>>();
    let full = ene_direct_color_writes(variant, &colors);

    for (index, color) in colors.iter().enumerate() {
        assert_eq!(
            ene_direct_led_write(variant, index, *color),
            full[index * 3..index * 3 + 3],
            "LED {index} must write exactly its chunk of the full frame"
        );
    }
}

#[test]
fn dram_frame_repeated_after_a_confirmed_write_sends_no_colors_or_latch() {
    let protocol = dram_protocol();
    let first = frame_operations(&protocol, &steady_frame());
    assert_eq!(color_writes(&first).len(), 8);
    assert_eq!(register_writes(&first, ENE_DRAM_COLOR_APPLY_REGISTER), 1);
    protocol.frame_write_finished(true);

    let repeated = frame_operations(&protocol, &steady_frame());

    assert!(color_writes(&repeated).is_empty());
    assert_eq!(register_writes(&repeated, ENE_DRAM_COLOR_APPLY_REGISTER), 0);
    assert_eq!(protocol.written_frame_bytes(), Some(0));
}

#[test]
fn one_changed_led_sends_one_color_pair_then_the_latch() {
    let protocol = dram_protocol();
    frame_operations(&protocol, &steady_frame());
    protocol.frame_write_finished(true);
    let mut changed = steady_frame();
    changed[5] = [0xAA, 0xBB, 0xCC];

    let operations = frame_operations(&protocol, &changed);

    assert_eq!(
        color_writes(&operations),
        vec![(0x8100 + 15, vec![0xAA, 0xCC, 0xBB])]
    );
    let variant = lookup_ene_firmware_variant("AUDA0-E6K5-0101").expect("variant should resolve");
    let latch = ene_write_register(ENE_DRAM_COLOR_APPLY_REGISTER, ENE_APPLY_VAL);
    let mut tail = ene_direct_led_write(variant, 5, [0xAA, 0xBB, 0xCC]);
    tail.extend(latch);
    assert!(
        operations.ends_with(&tail),
        "the changed LED's pair must be followed by exactly one latch"
    );
    assert_eq!(protocol.written_frame_bytes(), Some(3));
}

#[test]
fn failed_write_makes_the_next_frame_write_every_led() {
    let protocol = dram_protocol();
    frame_operations(&protocol, &steady_frame());
    protocol.frame_write_finished(true);
    let mut changed = steady_frame();
    changed[2] = [0xFF, 0x00, 0x00];
    assert_eq!(
        color_writes(&frame_operations(&protocol, &changed)).len(),
        1
    );
    protocol.frame_write_finished(false);

    let retried = frame_operations(&protocol, &changed);

    assert_eq!(color_writes(&retried).len(), 8);
    assert_eq!(register_writes(&retried, ENE_DRAM_COLOR_APPLY_REGISTER), 1);
    assert_eq!(protocol.written_frame_bytes(), Some(24));
}

#[test]
fn unreported_write_makes_the_next_frame_write_every_led() {
    let protocol = dram_protocol();
    frame_operations(&protocol, &steady_frame());

    let next = frame_operations(&protocol, &steady_frame());

    assert_eq!(color_writes(&next).len(), 8);
}

#[test]
fn init_sequence_makes_the_next_frame_write_every_led() {
    let protocol = dram_protocol();
    frame_operations(&protocol, &steady_frame());
    protocol.frame_write_finished(true);

    let _ = protocol.init_sequence();
    let next = frame_operations(&protocol, &steady_frame());

    assert_eq!(color_writes(&next).len(), 8);
}

#[test]
fn led_count_change_makes_the_next_frame_write_every_led() {
    let protocol = dram_protocol();
    frame_operations(&protocol, &steady_frame());
    protocol.frame_write_finished(true);
    let mut longer = steady_frame();
    longer.extend([[0x10, 0x20, 0x30]; 2]);

    let next = frame_operations(&protocol, &longer);

    assert_eq!(color_writes(&next).len(), 10);
}

#[test]
fn unchanged_frame_without_a_latch_register_encodes_nothing() {
    let protocol = protocol_with_firmware(b"AUMA0-E6K5-0104", 8);
    frame_operations(&protocol, &steady_frame());
    protocol.frame_write_finished(true);

    let mut commands = vec![ProtocolCommand {
        data: vec![0xAA],
        ..Default::default()
    }];
    protocol.encode_frame_into(&steady_frame(), &mut commands);

    assert!(commands.is_empty());
}

#[test]
fn confirmed_frames_still_rewrite_every_led_periodically() {
    let protocol = dram_protocol();
    assert_eq!(
        color_writes(&frame_operations(&protocol, &steady_frame())).len(),
        8
    );
    protocol.frame_write_finished(true);

    for frame in 1..ENE_FULL_WRITE_INTERVAL {
        let operations = frame_operations(&protocol, &steady_frame());
        assert!(
            color_writes(&operations).is_empty(),
            "frame {frame} after a full write should send no colors"
        );
        protocol.frame_write_finished(true);
    }
    let refresh = frame_operations(&protocol, &steady_frame());

    assert_eq!(color_writes(&refresh).len(), 8);
    assert_eq!(register_writes(&refresh, ENE_DRAM_COLOR_APPLY_REGISTER), 1);
    protocol.frame_write_finished(true);
    assert!(
        color_writes(&frame_operations(&protocol, &steady_frame())).is_empty(),
        "the refresh restarts the interval"
    );
}
