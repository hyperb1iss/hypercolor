//! Golden-byte coverage for the Razer Chroma Addressable RGB Controller frame
//! report (Spec 17 section 6.3).

use std::time::Duration;

use hypercolor_hal::attachment_profile::effective_attachment_slots;
use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{
    ARGB_CHANNELS, ARGB_LEDS_PER_CHANNEL, ARGB_REPORT_LEN, RAZER_VENDOR_ID, RazerArgbProtocol,
};
use hypercolor_hal::protocol::{Protocol, TransferType};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_hal::transport::hidapi::encode_hidapi_packet_for_testing;
use hypercolor_types::attachment::ComponentCategory;
use hypercolor_types::device::{
    ConnectionType, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo, DeviceOrigin,
    DeviceTopologyHint,
};

const PID_CHROMA_ARGB_CONTROLLER: u16 = 0x0F1F;
const LEDS: usize = 480;

fn protocol() -> RazerArgbProtocol {
    RazerArgbProtocol::new()
}

/// LED `index` of header `channel` is `[0x10 * (channel + 1), index, 0xFF - index]`,
/// so header, position, and byte order are all visible in a report.
fn tagged_frame() -> Vec<[u8; 3]> {
    (0..LEDS)
        .map(|led| {
            let channel = u8::try_from(led / 80).expect("channel fits in u8");
            let index = u8::try_from(led % 80).expect("index fits in u8");
            [0x10 * (channel + 1), index, 0xFF - index]
        })
        .collect()
}

/// The 320-byte report a header should produce from [`tagged_frame`]:
/// selector, channel twice, pad, last LED index, 80 RGB triples, then zero
/// padding up to the 315-byte color field's end.
fn expected_report(selector: u8, channel: u8) -> Vec<u8> {
    let mut report = vec![selector, channel, channel, 0x00, 0x4F];
    for index in 0..80_u8 {
        report.extend_from_slice(&[0x10 * (channel + 1), index, 0xFF - index]);
    }
    report.resize(320, 0x00);
    report
}

#[test]
fn constants_match_the_wire_facts() {
    assert_eq!(ARGB_REPORT_LEN, 320);
    assert_eq!(ARGB_CHANNELS, 6);
    assert_eq!(ARGB_LEDS_PER_CHANNEL, 80);
}

#[test]
fn a_frame_is_one_report_per_header() {
    let commands = protocol().encode_frame(&tagged_frame());

    assert_eq!(commands.len(), 6);
    for command in &commands {
        assert_eq!(command.data.len(), 320);
        assert!(!command.expects_response);
        assert_eq!(command.response_delay, Duration::ZERO);
        assert_eq!(command.post_delay, Duration::from_millis(1));
        assert_eq!(command.transfer_type, TransferType::Primary);
    }
}

#[test]
fn first_header_report_matches_golden_bytes() {
    let commands = protocol().encode_frame(&tagged_frame());

    assert_eq!(
        &commands[0].data[..8],
        &[0x04, 0x00, 0x00, 0x00, 0x4F, 0x10, 0x00, 0xFF]
    );
    assert_eq!(
        &commands[0].data[242..248],
        &[0x10, 0x4F, 0xB0, 0x00, 0x00, 0x00]
    );
    assert_eq!(commands[0].data, expected_report(0x04, 0x00));
}

#[test]
fn headers_zero_through_four_use_selector_0x04_and_header_five_uses_0x84() {
    let commands = protocol().encode_frame(&tagged_frame());

    for (channel, selector) in [
        (0_u8, 0x04),
        (1, 0x04),
        (2, 0x04),
        (3, 0x04),
        (4, 0x04),
        (5, 0x84),
    ] {
        let report = &commands[usize::from(channel)].data;
        assert_eq!(
            report,
            &expected_report(selector, channel),
            "header {channel}"
        );
    }
    assert_eq!(
        &commands[5].data[..6],
        &[0x84, 0x05, 0x05, 0x00, 0x4F, 0x60]
    );
}

#[test]
fn color_bytes_land_in_red_green_blue_order() {
    let mut colors = vec![[0_u8; 3]; LEDS];
    colors[81] = [0xAA, 0xBB, 0xCC];
    let commands = protocol().encode_frame(&colors);

    // LED 81 is header 1, position 1: the second triple after the 5-byte header.
    assert_eq!(&commands[1].data[8..11], &[0xAA, 0xBB, 0xCC]);
    assert!(commands[0].data[5..].iter().all(|&byte| byte == 0));
}

#[test]
fn hidapi_framing_prepends_report_id_zero() {
    let commands = protocol().encode_frame(&tagged_frame());
    let packet =
        encode_hidapi_packet_for_testing(&commands[0].data, 0x00, HidRawReportMode::FeatureReport);

    assert_eq!(packet.len(), 321);
    assert_eq!(&packet[..6], &[0x00, 0x04, 0x00, 0x00, 0x00, 0x4F]);
}

#[test]
fn short_frames_pad_with_black_and_long_frames_truncate() {
    let short = protocol().encode_frame(&[[0x01, 0x02, 0x03]]);
    assert_eq!(short.len(), 6);
    assert_eq!(&short[0].data[5..8], &[0x01, 0x02, 0x03]);
    assert!(short[0].data[8..].iter().all(|&byte| byte == 0));
    for report in &short[1..] {
        assert!(report.data[5..].iter().all(|&byte| byte == 0));
        assert_eq!(report.data[4], 0x4F, "every header still sends 80 LEDs");
    }

    let mut long = tagged_frame();
    long.extend(std::iter::repeat_n([0xFF, 0xFF, 0xFF], 40));
    let commands = protocol().encode_frame(&long);
    assert_eq!(commands.len(), 6);
    assert_eq!(commands[5].data, expected_report(0x84, 0x05));
}

#[test]
fn encode_frame_into_reuses_buffers_and_matches_encode_frame() {
    let frame = tagged_frame();
    let mut commands = Vec::new();
    protocol().encode_frame_into(&vec![[0x7F, 0x7F, 0x7F]; LEDS], &mut commands);
    protocol().encode_frame_into(&frame, &mut commands);

    let fresh = protocol().encode_frame(&frame);
    assert_eq!(commands.len(), fresh.len());
    for (reused, fresh) in commands.iter().zip(&fresh) {
        assert_eq!(reused.data, fresh.data);
    }
}

#[test]
fn exposes_six_full_length_header_strips() {
    let protocol = protocol();
    let zones = protocol.zones();

    assert_eq!(protocol.total_leds(), 480);
    assert_eq!(zones.len(), 6);
    for (index, zone) in zones.iter().enumerate() {
        assert_eq!(zone.name, format!("Channel {}", index + 1));
        assert_eq!(zone.led_count, 80);
        assert_eq!(zone.topology, DeviceTopologyHint::Strip);
        assert_eq!(zone.color_format, DeviceColorFormat::Rgb);
        assert!(zone.layout_hint.is_none());
    }
}

#[test]
fn direct_color_needs_no_mode_switch_and_no_hardware_brightness() {
    let protocol = protocol();
    let capabilities = protocol.capabilities();

    assert_eq!(protocol.name(), "Razer Chroma ARGB");
    assert!(protocol.init_sequence().is_empty());
    assert!(protocol.shutdown_sequence().is_empty());
    assert!(protocol.connection_diagnostics().is_empty());
    assert!(protocol.keepalive().is_none());
    assert!(protocol.encode_brightness(0x80).is_none());
    assert!(capabilities.supports_direct);
    assert!(!capabilities.supports_brightness);
    assert_eq!(capabilities.led_count, 480);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(7));
    assert_eq!(capabilities.max_fps, 142);
}

#[test]
fn database_binds_the_controller_to_feature_reports_on_interface_one() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_CHROMA_ARGB_CONTROLLER)
        .expect("Chroma Addressable RGB Controller descriptor should exist");

    assert_eq!(descriptor.name, "Razer Chroma Addressable RGB Controller");
    assert_eq!(descriptor.protocol.id, "razer/chroma-argb-controller");
    assert_eq!(descriptor.driver_id(), "razer");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert!(descriptor.firmware_predicate.is_none());
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: Some(1),
            report_id: 0x00,
            report_mode: HidRawReportMode::FeatureReport,
            max_report_len: 321,
            usage_page: Some(0x000C),
            usage: Some(0x0001),
        }
    );

    let built = (descriptor.protocol.build)();
    assert_eq!(built.total_leds(), 480);
    assert_eq!(
        built.encode_frame(&tagged_frame())[0].data,
        expected_report(0x04, 0x00)
    );
}

#[test]
fn header_slots_accept_fan_and_ring_attachments() {
    let protocol = protocol();
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Razer Chroma Addressable RGB Controller".to_owned(),
        vendor: "Razer".to_owned(),
        family: DeviceFamily::new_static("razer", "Razer"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("razer", "usb", ConnectionType::Usb)
            .with_protocol_id("razer/chroma-argb-controller"),
        segments: protocol.zones(),
        firmware_version: None,
        capabilities: protocol.capabilities(),
    };

    let slots = effective_attachment_slots(&info, &[]);
    assert_eq!(slots.len(), 6, "one attachment slot per header");
    for (index, slot) in slots.iter().enumerate() {
        assert_eq!(
            slot.led_start,
            u32::try_from(index * 80).expect("offset fits")
        );
        assert_eq!(slot.led_count, 80);
        for category in [
            ComponentCategory::Strip,
            ComponentCategory::Fan,
            ComponentCategory::Aio,
            ComponentCategory::Ring,
        ] {
            assert!(
                slot.suggested_categories.contains(&category),
                "header slot {} should offer {category:?}",
                slot.id
            );
        }
    }
}
