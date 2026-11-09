//! NZXT Phase 1 descriptor and database registration tests (Spec 39 §6.5).

use std::collections::BTreeSet;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::nzxt::{
    N7_B650E_FAMILY_HEADERS, N7_Z390_HEADERS, N7_Z490_FAMILY_HEADERS, NZXT_VID, descriptors,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_types::device::{DriverTransportKind, SegmentInfo};

/// PID, protocol ID, interface, report length (Spec 39 §6.3, §6.5).
const EXPECTED: &[(u16, &str, u8, usize)] = &[
    (0x2001, "nzxt/hue-2", 0, 64),
    (0x2002, "nzxt/hue-2-ambient", 0, 64),
    (0x2005, "nzxt/n7-z390", 0, 64),
    (0x2006, "nzxt/smart-device-v2", 0, 64),
    (0x2007, "nzxt/kraken-x3", 0, 64),
    (0x2009, "nzxt/rgb-fan-controller-2009", 0, 64),
    (0x200A, "nzxt/n7-z490", 0, 64),
    (0x200B, "nzxt/n7-b550", 0, 64),
    (0x200C, "nzxt/n7-z590", 0, 64),
    (0x200D, "nzxt/smart-device-v2-case-200d", 0, 64),
    (0x200E, "nzxt/rgb-fan-controller-200e", 0, 64),
    (0x200F, "nzxt/smart-device-v2-case-200f", 0, 64),
    (0x2010, "nzxt/rgb-fan-controller-2010", 0, 64),
    (0x2011, "nzxt/rgb-fan-controller-2011", 0, 64),
    (0x2012, "nzxt/rgb-controller-2012", 0, 64),
    (0x2014, "nzxt/kraken-x3-rgb", 0, 64),
    (0x2016, "nzxt/n7-z690", 0, 64),
    (0x2017, "nzxt/n5-z690", 0, 64),
    (0x2019, "nzxt/rgb-fan-controller-2019", 0, 64),
    (0x201B, "nzxt/n7-b650e", 0, 64),
    (0x201D, "nzxt/n7-z790", 0, 64),
    (0x201F, "nzxt/rgb-fan-controller-201f", 0, 64),
    (0x2020, "nzxt/rgb-fan-controller-2020", 0, 64),
    (0x2021, "nzxt/rgb-controller-2021", 0, 64),
    (0x2022, "nzxt/control-hub", 0, 64),
    (0x3008, "nzxt/kraken-z3", 1, 64),
    (0x3012, "nzxt/kraken-elite-v2", 1, 512),
];

/// PIDs Spec 39 names whose lighting is held back or out of Phase 1 scope.
const NOT_REGISTERED: &[u16] = &[
    0x1714, // Smart Device V1: direct-mode byte unspecified (§9.4)
    0x170E, 0x1715, // legacy Kraken X2/M2: Phase 2
    0x2004, // Vertagear kit: Phase 2
    0x300C, 0x300E, // Kraken / Kraken Elite: lighting unspecified (§9.2)
    0x2100, 0x2103, 0x2104, 0x2105, 0x2106, 0x2107, 0x2108, 0x2130, 0x2131,
    0x2136, // peripherals: Phase 3
];

fn expected_transport(interface: u8, max_report_len: usize) -> TransportType {
    #[cfg(any(windows, target_os = "macos"))]
    {
        TransportType::UsbHidApi {
            interface: Some(interface),
            report_id: 0x00,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len,
            usage_page: None,
            usage: None,
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = max_report_len;
        TransportType::UsbHidRaw {
            interface,
            report_id: 0x00,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            usage_page: None,
            usage: None,
        }
    }
}

fn zone_names(zones: &[SegmentInfo]) -> Vec<&str> {
    zones.iter().map(|zone| zone.name.as_str()).collect()
}

#[test]
fn every_phase_one_pid_has_one_descriptor() {
    let nzxt = descriptors();
    assert_eq!(nzxt.len(), EXPECTED.len());

    for (pid, protocol_id, interface, report_len) in EXPECTED {
        let matches: Vec<_> = nzxt
            .iter()
            .filter(|descriptor| descriptor.product_id == *pid)
            .collect();
        assert_eq!(matches.len(), 1, "PID 0x{pid:04X}");
        let descriptor = matches[0];

        assert_eq!(descriptor.vendor_id, NZXT_VID);
        assert_eq!(descriptor.protocol.id, *protocol_id);
        assert_eq!(descriptor.driver_id(), "nzxt");
        assert_eq!(descriptor.family.id(), "nzxt");
        assert!(descriptor.name.starts_with("NZXT "));
        assert!(descriptor.firmware_predicate.is_none());
        assert_eq!(
            descriptor.transport,
            expected_transport(*interface, *report_len),
            "PID 0x{pid:04X} transport"
        );
    }

    let ids: BTreeSet<_> = nzxt
        .iter()
        .map(|descriptor| descriptor.protocol.id)
        .collect();
    assert_eq!(ids.len(), nzxt.len(), "protocol IDs are unique");
}

#[test]
fn database_resolves_nzxt_descriptors() {
    for (pid, protocol_id, _, _) in EXPECTED {
        let descriptor =
            ProtocolDatabase::lookup(NZXT_VID, *pid).expect("Phase 1 PID is registered");
        assert_eq!(descriptor.protocol.id, *protocol_id);
        assert!(
            ProtocolDatabase::known_vid_pids().contains(&(NZXT_VID, *pid)),
            "scanner filter includes 0x{pid:04X}"
        );
    }
    assert_eq!(ProtocolDatabase::by_vendor(NZXT_VID).len(), EXPECTED.len());
}

#[test]
fn held_back_pids_stay_unregistered() {
    for pid in NOT_REGISTERED {
        assert!(
            ProtocolDatabase::lookup(NZXT_VID, *pid).is_none(),
            "PID 0x{pid:04X} must not be claimed"
        );
    }
    // Hue Plus serial: channel count unspecified (§9.4).
    assert!(ProtocolDatabase::lookup(0x04D8, 0x00DF).is_none());
}

#[test]
fn nzxt_module_is_a_usb_protocol_catalog() {
    let module = ProtocolDatabase::module_descriptors()
        .iter()
        .find(|module| module.id == "nzxt")
        .expect("nzxt module is registered");
    assert_eq!(module.display_name, "NZXT");
    assert_eq!(module.transports.len(), 1);
    assert_eq!(module.transports[0].kind, DriverTransportKind::Usb);
    assert!(module.capabilities.protocol_catalog);

    let protocols = ProtocolDatabase::protocol_descriptors_for_driver("nzxt");
    assert_eq!(protocols.len(), EXPECTED.len());
    assert!(
        protocols
            .iter()
            .all(|protocol| protocol.route_backend_id == "usb")
    );
}

#[test]
fn motherboard_header_table_matches_spec() {
    assert_eq!(
        (
            N7_Z390_HEADERS.nzxt,
            N7_Z390_HEADERS.argb,
            N7_Z390_HEADERS.analog
        ),
        (3, 0, 0)
    );
    assert_eq!(
        (
            N7_Z490_FAMILY_HEADERS.nzxt,
            N7_Z490_FAMILY_HEADERS.argb,
            N7_Z490_FAMILY_HEADERS.analog
        ),
        (2, 1, 1)
    );
    assert_eq!(
        (
            N7_B650E_FAMILY_HEADERS.nzxt,
            N7_B650E_FAMILY_HEADERS.argb,
            N7_B650E_FAMILY_HEADERS.analog
        ),
        (4, 2, 0)
    );
}

#[test]
fn descriptor_protocols_expose_their_static_zones() {
    let build = |pid: u16| {
        let descriptor = ProtocolDatabase::lookup(NZXT_VID, pid).expect("registered");
        (descriptor.protocol.build)()
    };

    // Boards with a 12V header expose it before topology; the rest are empty.
    for pid in [0x200A, 0x200B, 0x200C, 0x2016, 0x2017] {
        let protocol = build(pid);
        assert_eq!(
            zone_names(&protocol.zones()),
            ["12V RGB Header"],
            "0x{pid:04X}"
        );
        assert_eq!(protocol.init_sequence().len(), 1);
    }
    for pid in [0x2005, 0x201B, 0x201D, 0x2001, 0x2009, 0x2022] {
        let protocol = build(pid);
        assert!(protocol.zones().is_empty(), "0x{pid:04X}");
        assert_eq!(protocol.init_sequence().len(), 1);
    }

    // Kraken X3 RGB carries the logo zone; the original X3 does not.
    assert_eq!(zone_names(&build(0x2014).zones()), ["Logo"]);
    assert!(build(0x2007).zones().is_empty());

    let z3 = build(0x3008);
    assert_eq!(z3.total_leds(), 40);
    assert!(z3.init_sequence().is_empty());

    let elite = build(0x3012);
    assert_eq!(elite.total_leds(), 24);
    assert!(elite.init_sequence().is_empty());
    assert_eq!(elite.encode_frame(&[[0, 0, 0]; 24])[0].data.len(), 512);
}

/// Build a PID's registered protocol and hand it a topology reply with
/// `slots` on every one of eight channels.
fn resolve_with_every_channel(pid: u16, slots: &[u8]) -> Box<dyn Protocol> {
    let descriptor = ProtocolDatabase::lookup(NZXT_VID, pid).expect("registered");
    let protocol = (descriptor.protocol.build)();
    let mut report = vec![0_u8; 64];
    report[..2].copy_from_slice(&[0x21, 0x03]);
    report[14] = 8;
    for channel in 0..8 {
        let start = 15 + channel * 6;
        report[start..start + slots.len()].copy_from_slice(slots);
    }
    let response = protocol
        .parse_response(&report)
        .expect("topology reply parses");
    assert_eq!(response.status, ResponseStatus::Ok);
    protocol
}

/// Apply packets per frame, which is the number of streamed channels.
fn streamed_channels(protocol: &dyn Protocol) -> usize {
    let colors = vec![[0_u8; 3]; protocol.total_leds() as usize];
    protocol
        .encode_frame(&colors)
        .iter()
        .filter(|command| command.data[..2] == [0x22, 0xA0])
        .count()
}

#[test]
fn each_pid_resolves_its_own_profile() {
    // (PID, streamed channels with a Hue 2 strip on all eight, 12V zones)
    let cases: &[(u16, usize, usize)] = &[
        (0x2001, 8, 0),
        (0x2002, 8, 0),
        (0x2006, 8, 0),
        (0x2009, 8, 0),
        (0x200D, 8, 0),
        (0x200E, 8, 0),
        (0x200F, 8, 0),
        (0x2010, 8, 0),
        (0x2011, 8, 0),
        (0x2012, 8, 0),
        (0x2019, 8, 0),
        (0x201F, 8, 0),
        (0x2020, 8, 0),
        (0x2021, 8, 0),
        (0x2022, 8, 0),
        (0x2005, 3, 0),
        (0x200A, 3, 1),
        (0x200B, 3, 1),
        (0x200C, 3, 1),
        (0x2016, 3, 1),
        (0x2017, 3, 1),
        (0x201B, 6, 0),
        (0x201D, 6, 0),
    ];

    for (pid, channels, analog) in cases {
        let protocol = resolve_with_every_channel(*pid, &[0x04]);
        assert_eq!(
            streamed_channels(protocol.as_ref()),
            *channels,
            "0x{pid:04X}"
        );
        let fixed = protocol
            .zones()
            .iter()
            .filter(|zone| zone.name == "12V RGB Header")
            .count();
        assert_eq!(fixed, *analog, "0x{pid:04X} 12V zones");
        assert_eq!(
            protocol.total_leds() as usize,
            channels * 10 + analog,
            "0x{pid:04X} LED total"
        );
    }
}

#[test]
fn kraken_x3_pids_cap_external_channels_and_route_the_logo() {
    // Three F120 RGB (54 LEDs) on every channel: the Kraken cap keeps two.
    let x3 = resolve_with_every_channel(0x2007, &[0x13, 0x13, 0x13]);
    assert_eq!(x3.total_leds(), 8 * 36);
    assert!(x3.zones().iter().all(|zone| zone.name != "Logo"));

    let x3_rgb = resolve_with_every_channel(0x2014, &[0x13, 0x13, 0x13]);
    assert_eq!(x3_rgb.total_leds(), 8 * 36 + 1);
    assert_eq!(
        x3_rgb.zones().last().map(|zone| zone.name.as_str()),
        Some("Logo")
    );

    // A logo accessory on every channel leaves nothing to stream.
    let logo_only = resolve_with_every_channel(0x2007, &[0x11]);
    assert_eq!(logo_only.total_leds(), 0);

    // A plain controller streams the same reply uncapped.
    let controller = resolve_with_every_channel(0x2009, &[0x13, 0x13, 0x13]);
    assert_eq!(controller.total_leds(), 8 * 54);
}
