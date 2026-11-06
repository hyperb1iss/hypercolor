use hypercolor_hal::drivers::corsair::peripheral::devices::{
    K95_PLATINUM_LIGHT_BAR_SLOTS, PID_K95_RGB_PLATINUM,
};
use hypercolor_hal::drivers::corsair::peripheral::legacy::{
    CorsairLegacyPeripheralProtocol, LegacyPeripheralConfig, LegacyPeripheralKind,
};
use hypercolor_hal::drivers::corsair::peripheral::nxp::CorsairNxpProtocol;
use hypercolor_hal::drivers::corsair::peripheral::types::{
    CorsairPeripheralClass, CorsairPeripheralTopology, NXP_PACKET_SIZE, NxpCommand,
    NxpDeviceConfig, NxpField, NxpLightingKind, NxpLightingMode,
};
use hypercolor_hal::protocol::Protocol;
use hypercolor_hal::transport::vendor::{VendorControlOperation, decode_operations};
use hypercolor_types::device::DeviceTopologyHint;

fn keyboard_config(
    name: &'static str,
    lighting_kind: NxpLightingKind,
    led_count: usize,
) -> NxpDeviceConfig {
    NxpDeviceConfig::new(
        name,
        CorsairPeripheralClass::Keyboard,
        lighting_kind,
        led_count,
        CorsairPeripheralTopology::KeyboardMatrix { rows: 6, cols: 24 },
    )
    .with_max_fps(30)
}

fn pointer_config(
    name: &'static str,
    class: CorsairPeripheralClass,
    lighting_kind: NxpLightingKind,
    led_count: usize,
) -> NxpDeviceConfig {
    NxpDeviceConfig::new(
        name,
        class,
        lighting_kind,
        led_count,
        CorsairPeripheralTopology::Strip,
    )
}

#[test]
fn init_enters_software_lighting_mode_without_touching_input_routing() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data.len(), NXP_PACKET_SIZE);
    assert_eq!(commands[0].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[0].data[1], NxpField::Lighting as u8);
    assert_eq!(commands[0].data[2], NxpLightingMode::Software as u8);
    assert!(commands[0].data[3..].iter().all(|byte| *byte == 0));
    assert!(
        commands
            .iter()
            .all(|command| command.data[1] != NxpField::KeyInput as u8
                && command.data[1] != NxpField::Special as u8),
        "lighting init must leave key input and special-function routing alone"
    );
}

#[test]
fn shutdown_restores_hardware_mode_when_safe() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));
    let commands = protocol.shutdown_sequence();

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[0].data[1], NxpField::Lighting as u8);
    assert_eq!(commands[0].data[2], NxpLightingMode::Hardware as u8);
}

#[test]
fn unclean_exit_devices_skip_the_hardware_mode_handover() {
    let protocol = CorsairNxpProtocol::new(
        keyboard_config("K95 RGB", NxpLightingKind::Packed512Keyboard, 144).with_unclean_exit(),
    );

    assert_eq!(protocol.init_sequence().len(), 1);
    assert!(protocol.shutdown_sequence().is_empty());
}

/// Older NXP firmware answers through feature reports, not input reports,
/// so any command that waited for an input report would stall it.
#[test]
fn every_nxp_command_is_write_only_and_one_packet_long() {
    let configs = [
        keyboard_config("K70 Lux", NxpLightingKind::FullRangeKeyboard, 144),
        keyboard_config("K70 RGB", NxpLightingKind::Packed512Keyboard, 144),
        keyboard_config("K55", NxpLightingKind::ZonedKeyboard, 3),
        keyboard_config("K70 Non-RGB", NxpLightingKind::MonochromeKeyboard, 144),
        pointer_config(
            "M65",
            CorsairPeripheralClass::Mouse,
            NxpLightingKind::Mouse,
            6,
        ),
        pointer_config(
            "Polaris",
            CorsairPeripheralClass::Mousepad,
            NxpLightingKind::Mousepad,
            15,
        ),
    ];

    for config in configs {
        let protocol = CorsairNxpProtocol::new(config);
        let colors = vec![[0x40_u8, 0x80, 0xC0]; config.led_count];
        let commands = protocol
            .init_sequence()
            .into_iter()
            .chain(protocol.encode_frame(&colors))
            .chain(protocol.shutdown_sequence())
            .collect::<Vec<_>>();

        assert!(!commands.is_empty(), "{}", config.name);
        for command in commands {
            assert!(!command.expects_response, "{}", config.name);
            assert_eq!(command.data.len(), NXP_PACKET_SIZE, "{}", config.name);
        }
    }
}

#[test]
fn reused_command_buffers_match_fresh_encodes() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));
    let first = vec![[1_u8, 2, 3]; 144];
    let second = vec![[9_u8, 8, 7]; 144];
    let mut reused = Vec::new();

    protocol.encode_frame_into(&first, &mut reused);
    protocol.encode_frame_into(&second, &mut reused);
    let fresh = protocol.encode_frame(&second);

    assert_eq!(reused.len(), fresh.len());
    for (reused, fresh) in reused.iter().zip(&fresh) {
        assert_eq!(reused.data, fresh.data);
        assert_eq!(reused.expects_response, fresh.expects_response);
    }
}

#[test]
fn short_frames_pad_with_black_and_long_frames_truncate() {
    let protocol = CorsairNxpProtocol::new(pointer_config(
        "M65",
        CorsairPeripheralClass::Mouse,
        NxpLightingKind::Mouse,
        3,
    ));

    let short = protocol.encode_frame(&[[10_u8, 20, 30]]);
    assert_eq!(short[0].data[2], 3);
    assert_eq!(
        &short[0].data[4..16],
        &[1, 10, 20, 30, 2, 0, 0, 0, 3, 0, 0, 0]
    );

    let long = protocol.encode_frame(&[[1_u8, 1, 1]; 8]);
    assert_eq!(long[0].data[2], 3);
    assert!(long[0].data[16..].iter().all(|byte| *byte == 0));
}

#[test]
fn full_range_chunks_stop_at_the_declared_plane_boundaries() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));
    let colors = vec![[0xFF_u8, 0, 0]; 144];
    let commands = protocol.encode_frame(&colors);

    // The third red chunk declares 48 bytes but a 144-slot plane only has
    // 24 left after the first two chunks; the rest stays zero.
    assert!(commands[2].data[4..28].iter().all(|byte| *byte == 0xFF));
    assert!(commands[2].data[28..].iter().all(|byte| *byte == 0));
}

#[test]
fn full_range_keyboard_emits_12_packets_in_component_order() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));
    let colors = (0_u8..144)
        .map(|value| [value, value.wrapping_add(1), value.wrapping_add(2)])
        .collect::<Vec<_>>();
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 12);
    assert_eq!(commands[0].data[0], NxpCommand::WriteBulk as u8);
    assert_eq!(commands[0].data[1], 0x01);
    assert_eq!(commands[0].data[2], 0x3C);
    assert_eq!(&commands[0].data[4..8], &[0, 1, 2, 3]);
    assert_eq!(commands[2].data[1], 0x03);
    assert_eq!(commands[2].data[2], 0x30);
    assert_eq!(commands[2].data[4], 120);

    assert_eq!(commands[3].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[3].data[1], NxpField::KeyboardColor as u8);
    assert_eq!(commands[3].data[2], 0x01);
    assert_eq!(commands[3].data[3], 0x03);
    assert_eq!(commands[3].data[4], 0x01);

    assert_eq!(&commands[4].data[4..8], &[1, 2, 3, 4]);
    assert_eq!(&commands[8].data[4..8], &[2, 3, 4, 5]);
    assert_eq!(commands[11].data[2], 0x03);
    assert_eq!(commands[11].data[4], 0x02);
}

#[test]
fn packed_512_keyboard_packs_two_3_bit_components_per_byte() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 RGB",
        NxpLightingKind::Packed512Keyboard,
        144,
    ));
    let mut colors = vec![[0_u8, 0, 0]; 144];
    colors[0] = [255, 0, 0];
    colors[1] = [0, 255, 0];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 5);
    assert_eq!(commands[0].data[0], NxpCommand::WriteBulk as u8);
    assert_eq!(commands[0].data[1], 0x01);
    assert_eq!(commands[0].data[2], 0x3C);
    assert_eq!(commands[0].data[4], 0x70);
    assert_eq!(commands[1].data[4 + 12], 0x07);
    assert_eq!(commands[2].data[4 + 24], 0x77);
    assert_eq!(commands[4].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[4].data[1], NxpField::KeyboardPackedColor as u8);
    assert_eq!(commands[4].data[4], 0xD8);
}

#[test]
fn k55_zoned_packet_emits_three_rgb_triples() {
    let protocol =
        CorsairNxpProtocol::new(keyboard_config("K55", NxpLightingKind::ZonedKeyboard, 3));
    let commands = protocol.encode_frame(&[[1_u8, 2, 3], [4, 5, 6], [7, 8, 9]]);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[0].data[1], NxpField::KeyboardZoneColor as u8);
    assert_eq!(&commands[0].data[4..13], &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn mouse_packet_emits_one_based_zone_ids_and_rgb_entries() {
    let protocol = CorsairNxpProtocol::new(pointer_config(
        "M65",
        CorsairPeripheralClass::Mouse,
        NxpLightingKind::Mouse,
        2,
    ));
    let commands = protocol.encode_frame(&[[10_u8, 20, 30], [40, 50, 60]]);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[0].data[1], NxpField::MouseColor as u8);
    assert_eq!(commands[0].data[2], 2);
    assert_eq!(commands[0].data[3], 0x01);
    assert_eq!(&commands[0].data[4..12], &[1, 10, 20, 30, 2, 40, 50, 60]);
}

#[test]
fn mousepad_packet_emits_contiguous_rgb_triples() {
    let protocol = CorsairNxpProtocol::new(pointer_config(
        "Polaris",
        CorsairPeripheralClass::Mousepad,
        NxpLightingKind::Mousepad,
        3,
    ));
    let commands = protocol.encode_frame(&[[1_u8, 2, 3], [4, 5, 6], [7, 8, 9]]);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[0].data[1], NxpField::MouseColor as u8);
    assert_eq!(commands[0].data[2], 3);
    assert_eq!(commands[0].data[3], 0x00);
    assert_eq!(&commands[0].data[4..13], &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn monochrome_keyboard_emits_intensity_red_path_only() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Non-RGB",
        NxpLightingKind::MonochromeKeyboard,
        144,
    ));
    let mut colors = vec![[0_u8, 0, 0]; 144];
    colors[0] = [1, 20, 3];
    colors[1] = [40, 5, 6];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 4);
    assert_eq!(commands[0].data[0], NxpCommand::WriteBulk as u8);
    assert_eq!(&commands[0].data[4..6], &[20, 40]);
    assert_eq!(commands[3].data[0], NxpCommand::Set as u8);
    assert_eq!(commands[3].data[1], NxpField::KeyboardColor as u8);
    assert_eq!(commands[3].data[2], 0x01);
}

#[test]
fn no_light_devices_expose_no_frame_commands_or_zones() {
    let protocol = CorsairNxpProtocol::new(keyboard_config("K66", NxpLightingKind::NoLights, 0));

    assert!(protocol.encode_frame(&[]).is_empty());
    assert!(protocol.zones().is_empty());
    assert!(!protocol.capabilities().supports_direct);
    assert_eq!(protocol.total_leds(), 0);
}

#[test]
fn topology_and_capabilities_follow_descriptor_config() {
    let protocol = CorsairNxpProtocol::new(keyboard_config(
        "K70 Lux",
        NxpLightingKind::FullRangeKeyboard,
        144,
    ));

    assert_eq!(protocol.total_leds(), 144);
    assert_eq!(protocol.capabilities().max_fps, 30);
    assert_eq!(
        protocol.zones()[0].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 24 }
    );
}

#[test]
fn legacy_keyboard_brightness_uses_vendor_control_commands() {
    let protocol = CorsairLegacyPeripheralProtocol::new(LegacyPeripheralConfig {
        name: "K95 Legacy",
        kind: LegacyPeripheralKind::Keyboard,
    });
    let commands = protocol
        .encode_brightness(255)
        .expect("legacy keyboard brightness should be supported");
    let operations = decode_operations(&commands[0].data).expect("vendor ops should decode");

    assert_eq!(
        operations,
        vec![VendorControlOperation::Write {
            request: 0x31,
            value: 0x0003,
            index: 0,
            data: Vec::new(),
        }]
    );
}

#[test]
fn legacy_m95_backlight_maps_brightness_to_on_off_vendor_command() {
    let protocol = CorsairLegacyPeripheralProtocol::new(LegacyPeripheralConfig {
        name: "M95",
        kind: LegacyPeripheralKind::M95Mouse,
    });
    let commands = protocol
        .encode_brightness(1)
        .expect("M95 backlight should be supported");
    let operations = decode_operations(&commands[0].data).expect("vendor ops should decode");

    assert_eq!(
        operations,
        vec![VendorControlOperation::Write {
            request: 49,
            value: 1,
            index: 0,
            data: Vec::new(),
        }]
    );
}

/// Bar LED `p` (left to right) arrives at frame position `144 + p` and must
/// land in plane slot `K95_PLATINUM_LIGHT_BAR_SLOTS[p]` of every component.
#[test]
fn k95_platinum_light_bar_colors_reach_their_plane_slots() {
    let descriptor = hypercolor_hal::ProtocolDatabase::lookup(0x1B1C, PID_K95_RGB_PLATINUM)
        .expect("K95 Platinum descriptor should exist");
    let protocol = (descriptor.protocol.build)();

    let mut colors = vec![[0_u8, 0, 0]; 163];
    for position in 0..19_u8 {
        colors[144 + usize::from(position)] = [0x40 + position, 0x80 + position, 0xC0 + position];
    }
    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 12);

    // Chunk 3 of each plane carries slots 120..168 at payload offset 0.
    for (component, chunk) in [(0_usize, 2_usize), (1, 6), (2, 10)] {
        let packet = &commands[chunk].data;
        assert_eq!(packet[0], NxpCommand::WriteBulk as u8);
        assert_eq!(packet[1], 0x03);
        let plane = &packet[4..];

        for (position, &slot) in K95_PLATINUM_LIGHT_BAR_SLOTS.iter().enumerate() {
            let expected = colors[144 + position][component];
            assert_eq!(
                plane[usize::from(slot) - 120],
                expected,
                "bar LED {position} component {component} must land in slot {slot:#04X}"
            );
        }
        // Keys stay black and the unused slots 163..168 stay zero.
        assert!(plane[..24].iter().all(|byte| *byte == 0));
        assert!(plane[43..48].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn light_bar_slots_cover_the_plane_tail_without_overlap() {
    let mut slots = K95_PLATINUM_LIGHT_BAR_SLOTS.to_vec();
    slots.sort_unstable();
    assert_eq!(slots, (0x90_u8..=0xA2).collect::<Vec<_>>());
}

#[test]
#[should_panic(expected = "only full-range planes have room for a light bar")]
fn packed_keyboards_cannot_take_a_light_bar() {
    let _ = keyboard_config("K70 RGB", NxpLightingKind::Packed512Keyboard, 144)
        .with_light_bar(&K95_PLATINUM_LIGHT_BAR_SLOTS);
}

#[test]
#[should_panic(expected = "light bar slots must follow the key slots inside the plane")]
fn light_bar_slots_outside_the_plane_are_refused() {
    let _ = keyboard_config("K95 Platinum", NxpLightingKind::FullRangeKeyboard, 144)
        .with_light_bar(&[0x90, 0xA8]);
}
