//! NZXT Phase 1 lighting descriptors (Spec 39 §6.5).
//!
//! Every entry is unverified: nobody has run these protocols on NZXT
//! hardware yet.

use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding};
use crate::transport::{
    HidAccessMode, HidTransportIntent, TransportIntent, resolve_current_transport,
};

use super::NZXT_VID;
use super::gen2::{Gen2Profile, NzxtGen2Protocol};
use super::kraken_elite_v2::NzxtKrakenEliteV2Protocol;
use super::kraken_modern::KrakenX3Logo;
use super::motherboard::MotherboardHeaders;
use super::packet::{ELITE_V2_REPORT_LEN, GEN2_REPORT_LEN};

/// HID interface carrying the Gen2 controller, motherboard, and Kraken X3
/// protocol.
pub const NZXT_GEN2_INTERFACE: u8 = 0;

/// HID interface carrying the Kraken Z3 and Elite v2 protocol.
pub const NZXT_KRAKEN_SCREEN_INTERFACE: u8 = 1;

/// Hue 2.
pub const PID_HUE_2: u16 = 0x2001;
/// Hue 2 Ambient.
pub const PID_HUE_2_AMBIENT: u16 = 0x2002;
/// N7 Z390 motherboard.
pub const PID_N7_Z390: u16 = 0x2005;
/// Smart Device V2.
pub const PID_SMART_DEVICE_V2: u16 = 0x2006;
/// Kraken X3 Series.
pub const PID_KRAKEN_X3: u16 = 0x2007;
/// RGB and Fan Controller (`0x2009`).
pub const PID_RGB_FAN_CONTROLLER_2009: u16 = 0x2009;
/// N7 Z490 motherboard.
pub const PID_N7_Z490: u16 = 0x200A;
/// N7 B550 motherboard.
pub const PID_N7_B550: u16 = 0x200B;
/// N7 Z590 motherboard.
pub const PID_N7_Z590: u16 = 0x200C;
/// Smart Device V2 Case Controller (`0x200D`).
pub const PID_SMART_DEVICE_V2_CASE_200D: u16 = 0x200D;
/// RGB and Fan Controller (`0x200E`).
pub const PID_RGB_FAN_CONTROLLER_200E: u16 = 0x200E;
/// Smart Device V2 Case Controller (`0x200F`).
pub const PID_SMART_DEVICE_V2_CASE_200F: u16 = 0x200F;
/// RGB and Fan Controller (`0x2010`).
pub const PID_RGB_FAN_CONTROLLER_2010: u16 = 0x2010;
/// RGB and Fan Controller (`0x2011`).
pub const PID_RGB_FAN_CONTROLLER_2011: u16 = 0x2011;
/// RGB Controller (`0x2012`).
pub const PID_RGB_CONTROLLER_2012: u16 = 0x2012;
/// Kraken X3 Series RGB.
pub const PID_KRAKEN_X3_RGB: u16 = 0x2014;
/// N7 Z690 motherboard.
pub const PID_N7_Z690: u16 = 0x2016;
/// N5 Z690 motherboard.
pub const PID_N5_Z690: u16 = 0x2017;
/// RGB and Fan Controller (`0x2019`).
pub const PID_RGB_FAN_CONTROLLER_2019: u16 = 0x2019;
/// N7 B650E motherboard.
pub const PID_N7_B650E: u16 = 0x201B;
/// N7 Z790 motherboard.
pub const PID_N7_Z790: u16 = 0x201D;
/// RGB and Fan Controller (`0x201F`).
pub const PID_RGB_FAN_CONTROLLER_201F: u16 = 0x201F;
/// RGB and Fan Controller (`0x2020`).
pub const PID_RGB_FAN_CONTROLLER_2020: u16 = 0x2020;
/// RGB Controller (`0x2021`).
pub const PID_RGB_CONTROLLER_2021: u16 = 0x2021;
/// Control Hub.
pub const PID_CONTROL_HUB: u16 = 0x2022;
/// Kraken Z3.
pub const PID_KRAKEN_Z3: u16 = 0x3008;
/// Kraken Elite v2 (2024 Elite RGB).
pub const PID_KRAKEN_ELITE_V2: u16 = 0x3012;

/// Header counts for N7 Z390 (Spec 39 §4.4).
pub const N7_Z390_HEADERS: MotherboardHeaders = MotherboardHeaders::new(3, 0, 0);
/// Header counts for the Z490, B550, Z590, and both Z690 boards.
pub const N7_Z490_FAMILY_HEADERS: MotherboardHeaders = MotherboardHeaders::new(2, 1, 1);
/// Header counts for N7 B650E and N7 Z790.
pub const N7_B650E_FAMILY_HEADERS: MotherboardHeaders = MotherboardHeaders::new(4, 2, 0);

const fn nzxt_hid_intent(interface: u8, max_report_len: usize) -> TransportIntent {
    TransportIntent::Hid(HidTransportIntent {
        access: HidAccessMode::HostManaged,
        interface,
        report_id: 0x00,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len,
        usage_page: None,
        usage: None,
    })
}

/// Transport intent for Gen2 controllers, motherboards, and Kraken X3.
pub const NZXT_GEN2_TRANSPORT_INTENT: TransportIntent =
    nzxt_hid_intent(NZXT_GEN2_INTERFACE, GEN2_REPORT_LEN);

/// Transport intent for the Kraken Z3.
pub const NZXT_KRAKEN_Z3_TRANSPORT_INTENT: TransportIntent =
    nzxt_hid_intent(NZXT_KRAKEN_SCREEN_INTERFACE, GEN2_REPORT_LEN);

/// Transport intent for the Kraken Elite v2.
pub const NZXT_KRAKEN_ELITE_V2_TRANSPORT_INTENT: TransportIntent =
    nzxt_hid_intent(NZXT_KRAKEN_SCREEN_INTERFACE, ELITE_V2_REPORT_LEN);

macro_rules! gen2_builder {
    ($fn_name:ident, $name:expr, $profile:expr) => {
        #[doc = concat!("Build the ", $name, " protocol.")]
        #[must_use]
        pub fn $fn_name() -> Box<dyn Protocol> {
            Box::new(NzxtGen2Protocol::new($name, $profile))
        }
    };
}

gen2_builder!(
    build_hue_2_protocol,
    "NZXT Hue 2",
    Gen2Profile::controller()
);
gen2_builder!(
    build_hue_2_ambient_protocol,
    "NZXT Hue 2 Ambient",
    Gen2Profile::controller()
);
gen2_builder!(
    build_smart_device_v2_protocol,
    "NZXT Smart Device V2",
    Gen2Profile::controller()
);
gen2_builder!(
    build_smart_device_v2_case_protocol,
    "NZXT Smart Device V2 Case Controller",
    Gen2Profile::controller()
);
gen2_builder!(
    build_rgb_fan_controller_protocol,
    "NZXT RGB & Fan Controller",
    Gen2Profile::controller()
);
gen2_builder!(
    build_rgb_controller_protocol,
    "NZXT RGB Controller",
    Gen2Profile::controller()
);
gen2_builder!(
    build_control_hub_protocol,
    "NZXT Control Hub",
    Gen2Profile::controller()
);
gen2_builder!(
    build_n7_z390_protocol,
    "NZXT N7 Z390",
    Gen2Profile::motherboard(N7_Z390_HEADERS)
);
gen2_builder!(
    build_n7_z490_protocol,
    "NZXT N7 Z490",
    Gen2Profile::motherboard(N7_Z490_FAMILY_HEADERS)
);
gen2_builder!(
    build_n7_b550_protocol,
    "NZXT N7 B550",
    Gen2Profile::motherboard(N7_Z490_FAMILY_HEADERS)
);
gen2_builder!(
    build_n7_z590_protocol,
    "NZXT N7 Z590",
    Gen2Profile::motherboard(N7_Z490_FAMILY_HEADERS)
);
gen2_builder!(
    build_n7_z690_protocol,
    "NZXT N7 Z690",
    Gen2Profile::motherboard(N7_Z490_FAMILY_HEADERS)
);
gen2_builder!(
    build_n5_z690_protocol,
    "NZXT N5 Z690",
    Gen2Profile::motherboard(N7_Z490_FAMILY_HEADERS)
);
gen2_builder!(
    build_n7_b650e_protocol,
    "NZXT N7 B650E",
    Gen2Profile::motherboard(N7_B650E_FAMILY_HEADERS)
);
gen2_builder!(
    build_n7_z790_protocol,
    "NZXT N7 Z790",
    Gen2Profile::motherboard(N7_B650E_FAMILY_HEADERS)
);
gen2_builder!(
    build_kraken_x3_protocol,
    "NZXT Kraken X3",
    Gen2Profile::kraken_x3(KrakenX3Logo::Unresolved)
);
gen2_builder!(
    build_kraken_x3_rgb_protocol,
    "NZXT Kraken X3 RGB",
    Gen2Profile::kraken_x3(KrakenX3Logo::FixedColorWrite)
);
gen2_builder!(
    build_kraken_z3_protocol,
    "NZXT Kraken Z3",
    Gen2Profile::kraken_z3()
);

/// Build the Kraken Elite v2 protocol.
#[must_use]
pub fn build_kraken_elite_v2_protocol() -> Box<dyn Protocol> {
    Box::new(NzxtKrakenEliteV2Protocol::new("NZXT Kraken Elite V2"))
}

macro_rules! nzxt_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        intent: $intent:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: NZXT_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("nzxt", "NZXT"),
            transport: resolve_current_transport($intent)
                .expect("NZXT HID transport should support the current platform"),
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
            serial_quirk: None,
        }
    };
    (
        gen2 pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        nzxt_descriptor!(
            pid: $pid,
            name: $name,
            protocol_id: $protocol_id,
            intent: NZXT_GEN2_TRANSPORT_INTENT,
            builder: $builder
        )
    };
}

static NZXT_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![
        nzxt_descriptor!(
            gen2 pid: PID_HUE_2,
            name: "NZXT Hue 2",
            protocol_id: "nzxt/hue-2",
            builder: build_hue_2_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_HUE_2_AMBIENT,
            name: "NZXT Hue 2 Ambient",
            protocol_id: "nzxt/hue-2-ambient",
            builder: build_hue_2_ambient_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_Z390,
            name: "NZXT N7 Z390",
            protocol_id: "nzxt/n7-z390",
            builder: build_n7_z390_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_SMART_DEVICE_V2,
            name: "NZXT Smart Device V2",
            protocol_id: "nzxt/smart-device-v2",
            builder: build_smart_device_v2_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_KRAKEN_X3,
            name: "NZXT Kraken X3",
            protocol_id: "nzxt/kraken-x3",
            builder: build_kraken_x3_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_2009,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-2009",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_Z490,
            name: "NZXT N7 Z490",
            protocol_id: "nzxt/n7-z490",
            builder: build_n7_z490_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_B550,
            name: "NZXT N7 B550",
            protocol_id: "nzxt/n7-b550",
            builder: build_n7_b550_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_Z590,
            name: "NZXT N7 Z590",
            protocol_id: "nzxt/n7-z590",
            builder: build_n7_z590_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_SMART_DEVICE_V2_CASE_200D,
            name: "NZXT Smart Device V2 Case Controller",
            protocol_id: "nzxt/smart-device-v2-case-200d",
            builder: build_smart_device_v2_case_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_200E,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-200e",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_SMART_DEVICE_V2_CASE_200F,
            name: "NZXT Smart Device V2 Case Controller",
            protocol_id: "nzxt/smart-device-v2-case-200f",
            builder: build_smart_device_v2_case_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_2010,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-2010",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_2011,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-2011",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_CONTROLLER_2012,
            name: "NZXT RGB Controller",
            protocol_id: "nzxt/rgb-controller-2012",
            builder: build_rgb_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_KRAKEN_X3_RGB,
            name: "NZXT Kraken X3 RGB",
            protocol_id: "nzxt/kraken-x3-rgb",
            builder: build_kraken_x3_rgb_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_Z690,
            name: "NZXT N7 Z690",
            protocol_id: "nzxt/n7-z690",
            builder: build_n7_z690_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N5_Z690,
            name: "NZXT N5 Z690",
            protocol_id: "nzxt/n5-z690",
            builder: build_n5_z690_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_2019,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-2019",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_B650E,
            name: "NZXT N7 B650E",
            protocol_id: "nzxt/n7-b650e",
            builder: build_n7_b650e_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_N7_Z790,
            name: "NZXT N7 Z790",
            protocol_id: "nzxt/n7-z790",
            builder: build_n7_z790_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_201F,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-201f",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_FAN_CONTROLLER_2020,
            name: "NZXT RGB & Fan Controller",
            protocol_id: "nzxt/rgb-fan-controller-2020",
            builder: build_rgb_fan_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_RGB_CONTROLLER_2021,
            name: "NZXT RGB Controller",
            protocol_id: "nzxt/rgb-controller-2021",
            builder: build_rgb_controller_protocol
        ),
        nzxt_descriptor!(
            gen2 pid: PID_CONTROL_HUB,
            name: "NZXT Control Hub",
            protocol_id: "nzxt/control-hub",
            builder: build_control_hub_protocol
        ),
        nzxt_descriptor!(
            pid: PID_KRAKEN_Z3,
            name: "NZXT Kraken Z3",
            protocol_id: "nzxt/kraken-z3",
            intent: NZXT_KRAKEN_Z3_TRANSPORT_INTENT,
            builder: build_kraken_z3_protocol
        ),
        nzxt_descriptor!(
            pid: PID_KRAKEN_ELITE_V2,
            name: "NZXT Kraken Elite V2",
            protocol_id: "nzxt/kraken-elite-v2",
            intent: NZXT_KRAKEN_ELITE_V2_TRANSPORT_INTENT,
            builder: build_kraken_elite_v2_protocol
        ),
    ]
});

/// Every NZXT Phase 1 lighting descriptor.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    NZXT_DESCRIPTORS.as_slice()
}
