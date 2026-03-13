use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{
    COMMAND_CLASS_DEVICE, COMMAND_SET_SCROLL_ACCELERATION, COMMAND_SET_SCROLL_MODE,
    COMMAND_SET_SCROLL_SMART_REEL, RAZER_VENDOR_ID, VARSTORE, build_basilisk_v3_protocol,
    build_mamba_elite_protocol,
};
use hypercolor_hal::protocol::ProtocolCommand;
use hypercolor_types::device::ScrollMode;

fn assert_scroll_packet(command: &ProtocolCommand, command_id: u8, value: u8) {
    assert!(command.expects_response);
    assert_eq!(command.response_delay, Duration::ZERO);
    assert_eq!(command.post_delay, Duration::ZERO);
    assert_eq!(command.data[1], 0x1F);
    assert_eq!(command.data[5], 0x02);
    assert_eq!(command.data[6], COMMAND_CLASS_DEVICE);
    assert_eq!(command.data[7], command_id);
    assert_eq!(command.data[8], VARSTORE);
    assert_eq!(command.data[9], value);
}

#[test]
fn scroll_mode_commands_encode_expected_packets() {
    let protocol = build_basilisk_v3_protocol();

    let tactile = protocol
        .encode_scroll_mode(ScrollMode::Tactile)
        .expect("Basilisk V3 should support scroll mode");
    let free_spin = protocol
        .encode_scroll_mode(ScrollMode::FreeSpin)
        .expect("Basilisk V3 should support scroll mode");

    assert_eq!(tactile.len(), 1);
    assert_eq!(free_spin.len(), 1);
    assert_scroll_packet(
        &tactile[0],
        COMMAND_SET_SCROLL_MODE,
        u8::from(ScrollMode::Tactile),
    );
    assert_scroll_packet(
        &free_spin[0],
        COMMAND_SET_SCROLL_MODE,
        u8::from(ScrollMode::FreeSpin),
    );
}

#[test]
fn smart_reel_and_acceleration_commands_encode_expected_packets() {
    let protocol = build_basilisk_v3_protocol();

    let smart_reel_on = protocol
        .encode_scroll_smart_reel(true)
        .expect("Basilisk V3 should support Smart Reel");
    let smart_reel_off = protocol
        .encode_scroll_smart_reel(false)
        .expect("Basilisk V3 should support Smart Reel");
    let acceleration_on = protocol
        .encode_scroll_acceleration(true)
        .expect("Basilisk V3 should support scroll acceleration");
    let acceleration_off = protocol
        .encode_scroll_acceleration(false)
        .expect("Basilisk V3 should support scroll acceleration");

    assert_scroll_packet(&smart_reel_on[0], COMMAND_SET_SCROLL_SMART_REEL, 1);
    assert_scroll_packet(&smart_reel_off[0], COMMAND_SET_SCROLL_SMART_REEL, 0);
    assert_scroll_packet(&acceleration_on[0], COMMAND_SET_SCROLL_ACCELERATION, 1);
    assert_scroll_packet(&acceleration_off[0], COMMAND_SET_SCROLL_ACCELERATION, 0);
}

#[test]
fn unsupported_devices_return_none_for_scroll_commands() {
    let protocol = build_mamba_elite_protocol();

    assert!(protocol.encode_scroll_mode(ScrollMode::Tactile).is_none());
    assert!(protocol.encode_scroll_smart_reel(true).is_none());
    assert!(protocol.encode_scroll_acceleration(true).is_none());
}

#[test]
fn basilisk_descriptors_report_scroll_capabilities_without_affecting_shared_groups() {
    let basilisk_v3 = build_basilisk_v3_protocol();
    let basilisk_v3_caps = basilisk_v3.capabilities();
    assert!(basilisk_v3_caps.features.scroll_mode);
    assert!(basilisk_v3_caps.features.scroll_smart_reel);
    assert!(basilisk_v3_caps.features.scroll_acceleration);

    let v3_x_descriptor =
        ProtocolDatabase::lookup(RAZER_VENDOR_ID, 0x00B9).expect("Basilisk V3 X descriptor");
    let v3_x_protocol = (v3_x_descriptor.protocol.build)();
    assert!(v3_x_protocol.capabilities().features.scroll_mode);

    let deathadder_mini_descriptor =
        ProtocolDatabase::lookup(RAZER_VENDOR_ID, 0x008C).expect("DeathAdder V2 Mini descriptor");
    let deathadder_mini_protocol = (deathadder_mini_descriptor.protocol.build)();
    assert!(!deathadder_mini_protocol.capabilities().features.scroll_mode);
}
