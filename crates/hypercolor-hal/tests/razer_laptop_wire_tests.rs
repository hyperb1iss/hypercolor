//! Wire equivalence for Razer Blade and Book laptops.
//!
//! The laptops used to send their 90-byte feature reports as raw USB
//! control transfers on a claimed interface 2. They now send the same
//! reports through hidapi's hidraw backend, which leaves `usbhid` bound.
//! These tests prove the host controller sees identical requests either
//! way: for every command each laptop protocol emits, the same
//! `SET_REPORT` setup and data stage, and for every reply, the same
//! `GET_REPORT` setup and length with the same bytes handed back to the
//! protocol.
//!
//! The old side is read from the control transport's own request builder.
//! The new side runs hidapi's own buffer framing, then applies what the
//! Linux HID stack does with a hidraw feature buffer (modelled below).

use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{
    PID_BLADE_15_LATE_2021_ADVANCED, RAZER_REPORT_LEN, RAZER_VENDOR_ID,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand};
use hypercolor_hal::registry::{DeviceDescriptor, HidRawReportMode, TransportType};
use hypercolor_hal::transport::control::{FEATURE_REPORT_READ_LEN, FeatureReportSetup};
use hypercolor_hal::transport::hidapi::{
    decode_feature_report_packet_for_testing, encode_feature_report_request_buffer_for_testing,
    encode_hidapi_packet_for_testing,
};

/// The transport every laptop descriptor carried before the move.
const LEGACY_INTERFACE: u8 = 2;
const LEGACY_REPORT_ID: u8 = 0x00;

/// One `SET_REPORT` request as it leaves the host controller.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SetReportOnWire {
    setup: FeatureReportSetup,
    data: Vec<u8>,
}

/// One `GET_REPORT` request as it leaves the host controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GetReportOnWire {
    setup: FeatureReportSetup,
    length: usize,
}

/// The HIDAPI tuple a laptop descriptor now carries.
#[derive(Debug, Clone, Copy)]
struct HidApiFeature {
    interface: u8,
    report_id: u8,
    report_mode: HidRawReportMode,
    max_report_len: usize,
}

fn laptop_descriptors() -> Vec<&'static DeviceDescriptor> {
    ProtocolDatabase::all()
        .iter()
        .filter(|descriptor| {
            descriptor.vendor_id == RAZER_VENDOR_ID
                && (descriptor.name.starts_with("Razer Blade")
                    || descriptor.name.starts_with("Razer Book"))
        })
        .collect()
}

fn hidapi_feature(descriptor: &DeviceDescriptor) -> HidApiFeature {
    let TransportType::UsbHidApi {
        interface,
        report_id,
        report_mode,
        max_report_len,
        ..
    } = descriptor.transport
    else {
        panic!(
            "{} must use the HIDAPI feature-report transport, got {:?}",
            descriptor.name, descriptor.transport
        );
    };
    HidApiFeature {
        interface: interface
            .unwrap_or_else(|| panic!("{} must pin its HID interface", descriptor.name)),
        report_id,
        report_mode,
        max_report_len,
    }
}

/// What the control transport put on the wire for one outgoing report: the
/// payload is the data stage, untouched.
fn legacy_set_report(payload: &[u8]) -> SetReportOnWire {
    SetReportOnWire {
        setup: FeatureReportSetup::set_report(LEGACY_REPORT_ID, LEGACY_INTERFACE),
        data: payload.to_vec(),
    }
}

fn legacy_get_report() -> GetReportOnWire {
    GetReportOnWire {
        setup: FeatureReportSetup::get_report(LEGACY_REPORT_ID, LEGACY_INTERFACE),
        length: FEATURE_REPORT_READ_LEN,
    }
}

/// What the Linux HID stack sends for a hidraw `HIDIOCSFEATURE` buffer.
///
/// The first buffer byte is the report ID and becomes the low byte of
/// `wValue`. A device that declares no report IDs uses ID 0, and for ID 0
/// usbhid drops that byte from the data stage, so the device receives the
/// bytes after it.
fn hidraw_set_report(buffer: &[u8], interface: u8) -> SetReportOnWire {
    let (&report_id, rest) = buffer
        .split_first()
        .expect("a hidraw feature buffer starts with its report ID");
    let data = if report_id == 0 { rest } else { buffer };
    SetReportOnWire {
        setup: FeatureReportSetup::set_report(report_id, interface),
        data: data.to_vec(),
    }
}

/// What the Linux HID stack sends for a hidraw `HIDIOCGFEATURE` buffer.
///
/// Only the first buffer byte (the report ID) is read from userspace. For
/// ID 0 the data stage is one byte shorter than the buffer, because the
/// reply is placed after the ID byte.
fn hidraw_get_report(buffer: &[u8], interface: u8) -> GetReportOnWire {
    let report_id = *buffer
        .first()
        .expect("a hidraw feature buffer starts with its report ID");
    let length = if report_id == 0 {
        buffer.len() - 1
    } else {
        buffer.len()
    };
    GetReportOnWire {
        setup: FeatureReportSetup::get_report(report_id, interface),
        length,
    }
}

/// The buffer a hidraw `HIDIOCGFEATURE` hands back for a device reply.
fn hidraw_get_report_reply(report_id: u8, device_reply: &[u8]) -> Vec<u8> {
    if report_id == 0 {
        let mut buffer = Vec::with_capacity(device_reply.len() + 1);
        buffer.push(report_id);
        buffer.extend_from_slice(device_reply);
        buffer
    } else {
        device_reply.to_vec()
    }
}

/// Every command a laptop protocol can emit during a session.
fn session_commands(protocol: &dyn Protocol) -> Vec<(&'static str, ProtocolCommand)> {
    let total_leds = usize::try_from(protocol.total_leds()).expect("LED count fits usize");
    let gradient = (0..total_leds)
        .map(|index| {
            let byte = u8::try_from(index % 256).expect("index modulo 256 fits u8");
            [byte, byte.wrapping_mul(3), 0xFF - byte]
        })
        .collect::<Vec<_>>();

    let mut commands = Vec::new();
    let mut push = |stage: &'static str, batch: Vec<ProtocolCommand>| {
        commands.extend(batch.into_iter().map(|command| (stage, command)));
    };
    push("init", protocol.init_sequence());
    push("diagnostics", protocol.connection_diagnostics());
    push("frame", protocol.encode_frame(&gradient));
    push(
        "black frame",
        protocol.encode_frame(&vec![[0, 0, 0]; total_leds]),
    );
    if let Some(brightness) = protocol.encode_brightness(0x80) {
        push("brightness", brightness);
    }
    push("keepalive", protocol.keepalive_commands());
    push("shutdown", protocol.shutdown_sequence());
    commands
}

#[test]
fn every_laptop_command_reaches_the_wire_unchanged() {
    let laptops = laptop_descriptors();
    assert!(
        laptops.len() >= 36,
        "expected every Blade and Book descriptor, found {}",
        laptops.len()
    );

    for descriptor in laptops {
        let feature = hidapi_feature(descriptor);
        let protocol = (descriptor.protocol.build)();
        let commands = session_commands(protocol.as_ref());
        assert!(
            commands.iter().any(|(stage, _)| *stage == "frame"),
            "{} emitted no frame commands",
            descriptor.name
        );

        for (stage, command) in commands {
            assert_eq!(
                command.data.len(),
                RAZER_REPORT_LEN,
                "{} {stage} command is not one Razer report",
                descriptor.name
            );

            let buffer = encode_hidapi_packet_for_testing(
                &command.data,
                feature.report_id,
                feature.report_mode,
            );
            assert_eq!(
                buffer.len(),
                feature.max_report_len,
                "{} {stage}: the HID buffer must be exactly one report long",
                descriptor.name
            );
            assert_eq!(
                hidraw_set_report(&buffer, feature.interface),
                legacy_set_report(&command.data),
                "{} {stage} command differs on the wire",
                descriptor.name
            );
        }
    }
}

#[test]
fn every_laptop_reply_is_read_the_same_way() {
    // A plausible Ok reply to a brightness query: status 0x02, the
    // transaction ID echoed, then arguments and checksum.
    let mut device_reply = vec![0_u8; RAZER_REPORT_LEN];
    device_reply[0] = 0x02;
    device_reply[1] = 0x1F;
    device_reply[5] = 0x03;
    device_reply[6] = 0x03;
    device_reply[7] = 0x83;
    device_reply[8..11].copy_from_slice(&[0x01, 0x05, 0x80]);
    device_reply[88] = device_reply[2..88].iter().fold(0, |crc, byte| crc ^ byte);

    for descriptor in laptop_descriptors() {
        let feature = hidapi_feature(descriptor);

        for transaction_id in [None, Some(0x1F), Some(0xFF)] {
            let request = encode_feature_report_request_buffer_for_testing(
                feature.report_id,
                feature.max_report_len,
                transaction_id,
            );
            assert_eq!(
                hidraw_get_report(&request, feature.interface),
                legacy_get_report(),
                "{} GET_REPORT differs on the wire",
                descriptor.name
            );
        }

        let returned = hidraw_get_report_reply(feature.report_id, &device_reply);
        let decoded = decode_feature_report_packet_for_testing(
            &returned,
            feature.report_id,
            feature.max_report_len,
            false,
        );
        assert_eq!(
            decoded, device_reply,
            "{} must hand the protocol the same reply bytes the control transport did",
            descriptor.name
        );
    }
}

/// A literal pin of one command, so the equivalence above cannot pass by
/// both sides drifting together.
#[test]
fn blade_15_brightness_command_matches_its_golden_bytes() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)
        .expect("Blade 15 (Late 2021 Advanced) is registered");
    let feature = hidapi_feature(descriptor);
    let protocol = (descriptor.protocol.build)();
    let command = protocol
        .encode_brightness(0x80)
        .expect("Blade 15 supports brightness")
        .into_iter()
        .next()
        .expect("one brightness command");

    // Razer report: status 0x00, transaction ID 0x1F, no remaining packets,
    // protocol type 0, three argument bytes, class 0x03 command 0x03 (set
    // brightness), arguments VARSTORE, backlight LED, level 0x80. The XOR
    // checksum of bytes 2 through 87 lands at offset 88.
    let mut report = [0_u8; RAZER_REPORT_LEN];
    report[..11].copy_from_slice(&[
        0x00, 0x1F, 0x00, 0x00, 0x00, 0x03, 0x03, 0x03, 0x01, 0x05, 0x80,
    ]);
    report[88] = 0x87;
    assert_eq!(command.data, report);

    let mut hid_buffer = [0_u8; RAZER_REPORT_LEN + 1];
    hid_buffer[1..].copy_from_slice(&report);
    assert_eq!(
        encode_hidapi_packet_for_testing(&command.data, feature.report_id, feature.report_mode),
        hid_buffer,
        "the HID buffer is report ID 0 followed by the report"
    );

    let expected_setup = FeatureReportSetup {
        request: 0x09,
        value: 0x0300,
        index: 0x0002,
    };
    assert_eq!(
        hidraw_set_report(&hid_buffer, feature.interface),
        SetReportOnWire {
            setup: expected_setup,
            data: report.to_vec(),
        }
    );
    assert_eq!(legacy_set_report(&report).setup, expected_setup);
}
