//! Wooting encoder golden bytes (Spec 90 §4 to §7).

use std::time::Duration;

use hypercolor_hal::drivers::wooting::packet::{crc16_xmodem, rgb565};
use hypercolor_hal::drivers::wooting::v1::{pwm_level, red_register};
use hypercolor_hal::drivers::wooting::{WootingInterface, WootingModel, WootingProtocol};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, TransferType};
use hypercolor_types::device::DeviceTopologyHint;

const V2_LARGE: WootingInterface = WootingInterface::V2 {
    small_packets: false,
};
const V2_SMALL: WootingInterface = WootingInterface::V2 {
    small_packets: true,
};

fn protocol(model: WootingModel, interface: WootingInterface) -> WootingProtocol {
    WootingProtocol::new(model, interface)
}

fn black(protocol: &WootingProtocol) -> Vec<[u8; 3]> {
    vec![[0, 0, 0]; usize::try_from(protocol.total_leds()).expect("LED count fits usize")]
}

/// Byte offset of wire-matrix cell `(row, col)` in a v2 or v3 colour report.
fn cell_offset(row: usize, col: usize) -> usize {
    4 + 2 * (row * 21 + col)
}

fn assert_header(command: &ProtocolCommand, header: &[u8]) {
    assert_eq!(&command.data[..header.len()], header);
}

// ── Colour packing (§5.2) ────────────────────────────────────────────────

#[test]
fn rgb565_matches_the_spec_table() {
    assert_eq!(rgb565(0xFF, 0x00, 0x00), 0xF800);
    assert_eq!(rgb565(0x00, 0xFF, 0x00), 0x07E0);
    assert_eq!(rgb565(0x00, 0x00, 0xFF), 0x001F);
    assert_eq!(rgb565(0x12, 0x34, 0x56), 0x11AA);
    assert_eq!(rgb565(0xFF, 0xFF, 0xFF), 0xFFFF);
    assert_eq!(rgb565(0x01, 0x02, 0x03), 0x0000, "low bits truncate");
}

#[test]
fn crc16_xmodem_matches_the_catalogue_check_value() {
    assert_eq!(crc16_xmodem(b"123456789"), 0x31C3);
    assert_eq!(crc16_xmodem(&[]), 0x0000);
}

// ── Feature commands (§5.1) ──────────────────────────────────────────────

#[test]
fn v1_and_v2_commands_use_the_unnumbered_report() {
    for interface in [WootingInterface::V1, V2_LARGE, V2_SMALL] {
        let protocol = protocol(WootingModel::Two, interface);
        let init = protocol.init_sequence();
        assert_eq!(init.len(), 1);
        assert_eq!(
            init[0].data,
            [0x00, 0xD0, 0xDA, 0x21, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(init[0].transfer_type, TransferType::HidReport);
        assert!(!init[0].expects_response);
        assert_eq!(init[0].post_delay, Duration::from_millis(50));

        let shutdown = protocol.shutdown_sequence();
        assert_eq!(shutdown.len(), 1);
        assert_eq!(
            shutdown[0].data,
            [0x00, 0xD0, 0xDA, 0x20, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(shutdown[0].transfer_type, TransferType::HidReport);
        assert!(!shutdown[0].expects_response);
    }
}

#[test]
fn multi_report_commands_use_feature_report_one() {
    let protocol = protocol(WootingModel::EightyHe, WootingInterface::V3);
    let init = protocol.init_sequence();
    assert_eq!(init.len(), 1);
    assert_eq!(
        init[0].data,
        [0x01, 0xD1, 0xDA, 0x21, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(init[0].transfer_type, TransferType::HidReport);

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(
        shutdown[0].data,
        [0x01, 0xD1, 0xDA, 0x20, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(shutdown[0].transfer_type, TransferType::HidReport);
}

// ── Multi-report frames (§7) ─────────────────────────────────────────────

#[test]
fn multi_report_frame_is_one_padded_report_five() {
    let protocol = protocol(WootingModel::EightyHe, WootingInterface::V3);
    let commands = protocol.encode_frame(&black(&protocol));

    assert_eq!(commands.len(), 1);
    let frame = &commands[0];
    assert_eq!(frame.data.len(), 2047);
    assert_header(frame, &[0x05, 0xD1, 0xDA, 0x0B]);
    assert_eq!(frame.transfer_type, TransferType::Primary);
    assert!(!frame.expects_response);
    assert!(frame.data[4..].iter().all(|&byte| byte == 0));
}

#[test]
fn multi_report_frame_places_cells_on_the_21_column_stride() {
    let protocol = protocol(WootingModel::EightyHe, WootingInterface::V3);
    let mut colors = black(&protocol);
    // 80HE is 6 x 17: index 17 is row 1, column 0; the last LED is (5, 16).
    colors[0] = [0xFF, 0x00, 0x00];
    colors[17] = [0x00, 0xFF, 0x00];
    colors[6 * 17 - 1] = [0x12, 0x34, 0x56];

    let frame = &protocol.encode_frame(&colors)[0];
    assert_eq!(&frame.data[cell_offset(0, 0)..][..2], &[0x00, 0xF8]);
    assert_eq!(&frame.data[cell_offset(1, 0)..][..2], &[0xE0, 0x07]);
    assert_eq!(&frame.data[cell_offset(5, 16)..][..2], &[0xAA, 0x11]);
    // Columns 17 to 20 do not exist on the 80HE and stay black.
    assert_eq!(&frame.data[cell_offset(0, 17)..][..8], &[0; 8]);
}

#[test]
fn sixty_percent_boards_start_on_wire_row_one() {
    for model in [
        WootingModel::SixtyHeArm,
        WootingModel::SixtyHePlus,
        WootingModel::SixtyHeV2,
    ] {
        let protocol = protocol(model, WootingInterface::V3);
        assert_eq!(protocol.total_leds(), 5 * 14);
        let mut colors = black(&protocol);
        colors[0] = [0xFF, 0xFF, 0xFF];
        colors[5 * 14 - 1] = [0x00, 0x00, 0xFF];

        let frame = &protocol.encode_frame(&colors)[0];
        assert_eq!(
            &frame.data[cell_offset(0, 0)..][..42],
            &[0; 42],
            "{model:?} wire row 0 stays dark"
        );
        assert_eq!(&frame.data[cell_offset(1, 0)..][..2], &[0xFF, 0xFF]);
        assert_eq!(&frame.data[cell_offset(5, 13)..][..2], &[0x1F, 0x00]);
    }
}

#[test]
fn uwu_rgb_maps_a_five_by_seven_keypad() {
    let protocol = protocol(WootingModel::UwuRgb, WootingInterface::V3);
    assert_eq!(protocol.total_leds(), 35);
    let mut colors = black(&protocol);
    colors[7] = [0xFF, 0x00, 0x00];
    colors[34] = [0x00, 0xFF, 0x00];

    let frame = &protocol.encode_frame(&colors)[0];
    assert_eq!(&frame.data[cell_offset(1, 0)..][..2], &[0x00, 0xF8]);
    assert_eq!(&frame.data[cell_offset(4, 6)..][..2], &[0xE0, 0x07]);
    assert_eq!(&frame.data[cell_offset(5, 0)..][..2], &[0x00, 0x00]);
}

// ── v2 frames (§6) ───────────────────────────────────────────────────────

#[test]
fn v2_large_frame_is_one_257_byte_report() {
    let protocol = protocol(WootingModel::TwoHe, V2_LARGE);
    let mut colors = black(&protocol);
    colors[0] = [0xFF, 0x00, 0x00];
    colors[6 * 21 - 1] = [0x00, 0x00, 0xFF];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1);
    let frame = &commands[0];
    assert_eq!(frame.data.len(), 257);
    assert_header(frame, &[0x00, 0xD0, 0xDA, 0x0B]);
    assert_eq!(frame.transfer_type, TransferType::Primary);
    assert_eq!(&frame.data[4..6], &[0x00, 0xF8]);
    assert_eq!(&frame.data[254..256], &[0x1F, 0x00]);
    assert_eq!(frame.data[256], 0x00, "trailing pad byte");
}

#[test]
fn v2_small_packets_split_the_report_body_into_four() {
    let large = protocol(WootingModel::TwoHeArm, V2_LARGE);
    let small = protocol(WootingModel::TwoHeArm, V2_SMALL);
    let mut colors = black(&large);
    for (index, color) in colors.iter_mut().enumerate() {
        let byte = u8::try_from(index).expect("126 LEDs fit u8");
        *color = [byte, byte.wrapping_mul(3), byte.wrapping_mul(7)];
    }

    let whole = &large.encode_frame(&colors)[0].data;
    let chunks = small.encode_frame(&colors);
    assert_eq!(chunks.len(), 4);
    for (index, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.data.len(), 65);
        assert_eq!(chunk.data[0], 0x00, "every chunk leads with the report ID");
        let start = 1 + index * 64;
        assert_eq!(&chunk.data[1..], &whole[start..start + 64]);
        assert_eq!(chunk.transfer_type, TransferType::Primary);
    }
    assert_eq!(&chunks[0].data[1..4], &[0xD0, 0xDA, 0x0B]);
}

// ── v1 frames (§4) ───────────────────────────────────────────────────────

#[test]
fn v1_pwm_registers_follow_the_two_bank_layout() {
    let expected = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x30, 0x31, 0x32,
        0x33, 0x34, 0x35, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D,
    ];
    for (slot, register) in expected.into_iter().enumerate() {
        assert_eq!(red_register(slot), register, "slot {slot}");
    }
}

#[test]
fn v1_pwm_levels_decode_the_srgb_curve() {
    for (srgb, pwm) in [
        (0, 0),
        (1, 0),
        (10, 1),
        (64, 13),
        (128, 55),
        (192, 134),
        (254, 253),
        (255, 255),
    ] {
        assert_eq!(pwm_level(srgb), pwm, "sRGB {srgb}");
    }
}

#[test]
fn v1_black_frames_carry_the_spec_checksums() {
    let two = protocol(WootingModel::Two, WootingInterface::V1);
    let commands = two.encode_frame(&black(&two));
    assert_eq!(commands.len(), 5);

    let expected = [
        (0x00, 0x00, [0xA5, 0xC4]),
        (0x00, 0x60, [0x7C, 0xE8]),
        (0x01, 0x00, [0x02, 0xE9]),
        (0x01, 0x60, [0xDB, 0xC5]),
        (0x02, 0x00, [0xEB, 0x9F]),
    ];
    for (command, (chip, register, crc)) in commands.iter().zip(expected) {
        assert_eq!(command.data.len(), 129);
        assert_header(command, &[0x00, 0xD0, 0xDA, 0x0B, chip, register]);
        assert!(command.data[6..127].iter().all(|&byte| byte == 0));
        assert_eq!(&command.data[127..], &crc);
        assert_eq!(command.transfer_type, TransferType::Primary);
    }

    let one = protocol(WootingModel::One, WootingInterface::V1);
    assert_eq!(
        one.encode_frame(&black(&one)).len(),
        4,
        "the tenkeyless board has no numpad chip"
    );
}

#[test]
fn v1_white_escape_lands_on_slot_zero_of_part_zero() {
    let protocol = protocol(WootingModel::One, WootingInterface::V1);
    let mut colors = black(&protocol);
    colors[0] = [0xFF, 0xFF, 0xFF];

    let commands = protocol.encode_frame(&colors);
    let part = &commands[0].data;
    assert_eq!(part[6], 0xFF, "red");
    assert_eq!(part[6 + 0x10], 0xFF, "green");
    assert_eq!(part[6 + 0x20], 0xFF, "blue");
    assert_eq!(&part[127..], &[0x47, 0x68]);
    let lit: Vec<_> = part[6..102]
        .iter()
        .enumerate()
        .filter(|&(_, &byte)| byte != 0)
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(lit, [0x00, 0x10, 0x20], "nothing else lights");
}

#[test]
fn v1_enter_and_left_shift_mirror_onto_the_iso_leds() {
    let protocol = protocol(WootingModel::Two, WootingInterface::V1);
    let cols = 21;
    let mut colors = black(&protocol);
    colors[3 * cols + 13] = [0xFF, 0x00, 0x00]; // Enter, LED 65
    colors[4 * cols] = [0x00, 0x00, 0xFF]; // left Shift, LED 9

    let commands = protocol.encode_frame(&colors);
    let part_two = &commands[2].data[6..102];
    assert_eq!(part_two[0x35], 0xFF, "ANSI Enter (LED 65, slot 17)");
    assert_eq!(part_two[0x32], 0xFF, "ISO Enter (LED 62, slot 14)");

    let part_zero = &commands[0].data[6..102];
    assert_eq!(part_zero[0x0B + 0x20], 0xFF, "ANSI left Shift (LED 9)");
    assert_eq!(part_zero[0x09 + 0x20], 0xFF, "ISO left Shift (LED 7)");
}

#[test]
fn v1_numpad_lands_on_the_third_chip() {
    let protocol = protocol(WootingModel::Two, WootingInterface::V1);
    let mut colors = black(&protocol);
    colors[20] = [0xFF, 0x00, 0x00]; // (0, 20), LED 116, part 4 slot 20

    let commands = protocol.encode_frame(&colors);
    let part_four = &commands[4].data[6..102];
    assert_eq!(part_four[red_register(116 % 24)], 0xFF);
    assert!(
        commands[..4]
            .iter()
            .all(|part| part.data[6..102].iter().all(|&b| b == 0))
    );
}

// ── Frame shape and normalization ────────────────────────────────────────

#[test]
fn every_model_reports_a_matrix_zone() {
    for (model, rows, cols) in [
        (WootingModel::One, 6, 17),
        (WootingModel::Two, 6, 21),
        (WootingModel::TwoLekker, 6, 21),
        (WootingModel::TwoHe, 6, 21),
        (WootingModel::TwoHeArm, 6, 21),
        (WootingModel::SixtyHe, 5, 14),
        (WootingModel::SixtyHeArm, 5, 14),
        (WootingModel::SixtyHePlus, 5, 14),
        (WootingModel::SixtyHeV2, 5, 14),
        (WootingModel::EightyHe, 6, 17),
        (WootingModel::UwuRgb, 5, 7),
    ] {
        let protocol = protocol(model, WootingInterface::V3);
        let zones = protocol.zones();
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].topology, DeviceTopologyHint::Matrix { rows, cols });
        assert_eq!(zones[0].led_count, rows * cols);
        assert_eq!(protocol.total_leds(), rows * cols);
        assert_eq!(protocol.capabilities().led_count, rows * cols);
    }
}

#[test]
fn short_and_long_frames_are_padded_and_truncated() {
    let protocol = protocol(WootingModel::EightyHe, WootingInterface::V3);
    let short = protocol.encode_frame(&[[0xFF, 0x00, 0x00]]);
    assert_eq!(&short[0].data[4..6], &[0x00, 0xF8]);
    assert!(short[0].data[6..].iter().all(|&byte| byte == 0));

    let long = vec![[0xFF, 0xFF, 0xFF]; 500];
    let frame = &protocol.encode_frame(&long)[0];
    assert_eq!(frame.data.len(), 2047);
    assert_eq!(
        &frame.data[cell_offset(0, 17)..][..2],
        &[0, 0],
        "extra colours never spill past the model's columns"
    );
}

#[test]
fn encode_frame_into_reuses_the_command_buffer() {
    let protocol = protocol(WootingModel::Two, WootingInterface::V1);
    let colors = black(&protocol);
    let mut commands = Vec::new();
    protocol.encode_frame_into(&colors, &mut commands);
    assert_eq!(commands.len(), 5);
    let capacities: Vec<_> = commands
        .iter()
        .map(|command| command.data.capacity())
        .collect();
    let pointers: Vec<_> = commands
        .iter()
        .map(|command| command.data.as_ptr())
        .collect();

    protocol.encode_frame_into(&colors, &mut commands);
    assert_eq!(commands.len(), 5);
    for ((command, capacity), pointer) in commands.iter().zip(capacities).zip(pointers) {
        assert_eq!(command.data.capacity(), capacity);
        assert_eq!(
            command.data.as_ptr(),
            pointer,
            "steady-state frames do not reallocate"
        );
    }

    let v3 = WootingProtocol::new(WootingModel::Two, WootingInterface::V3);
    v3.encode_frame_into(&colors, &mut commands);
    assert_eq!(
        commands.len(),
        1,
        "a shorter frame truncates the reused buffer"
    );
}

#[test]
fn frame_intervals_and_names_track_the_generation() {
    let v1 = protocol(WootingModel::Two, WootingInterface::V1);
    let v2 = protocol(WootingModel::Two, V2_LARGE);
    let v3 = protocol(WootingModel::TwoHeArm, WootingInterface::V3);

    assert_eq!(v1.frame_interval(), Duration::from_millis(33));
    assert_eq!(v1.capabilities().max_fps, 30);
    assert_eq!(v2.frame_interval(), Duration::from_millis(16));
    assert_eq!(v3.capabilities().max_fps, 62);
    assert!(v3.capabilities().supports_direct);
    assert!(!v3.capabilities().supports_brightness);

    assert_eq!(v1.name(), "Wooting RGB (v1)");
    assert_eq!(v2.name(), "Wooting RGB (v2)");
    assert_eq!(v3.name(), "Wooting RGB (multi-report)");
}
