//! Self-contained Razer device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::RazerProtocol;
use super::seiren_v3::SeirenV3Protocol;
use super::types::{
    LED_ID_BACKLIGHT, LED_ID_ZERO, RazerLightingCommandSet, RazerMatrixType, RazerProtocolVersion,
    VARSTORE,
};

/// Razer vendor ID.
pub const RAZER_VENDOR_ID: u16 = 0x1532;

const RAZER_CONSUMER_USAGE_PAGE: u16 = 0x000C;
const RAZER_CONSUMER_USAGE: u16 = 0x0001;
const RAZER_VENDOR_USAGE_PAGE: u16 = 0xFF53;
const RAZER_VENDOR_USAGE: u16 = 0x0004;

/// Razer Huntsman V2 (full-size).
pub const PID_HUNTSMAN_V2: u16 = 0x026C;

/// Razer Basilisk V3.
pub const PID_BASILISK_V3: u16 = 0x0099;

/// Razer Seiren Emote.
pub const PID_SEIREN_EMOTE: u16 = 0x0F1B;

/// Razer Seiren V3 Chroma.
pub const PID_SEIREN_V3_CHROMA: u16 = 0x056F;

/// Razer Blade 14 (2021).
pub const PID_BLADE_14_2021: u16 = 0x0270;

/// Razer Blade 15 (Late 2021 Advanced).
pub const PID_BLADE_15_LATE_2021_ADVANCED: u16 = 0x0276;

/// Razer Blade 15 (2022).
pub const PID_BLADE_15_2022: u16 = 0x028A;

/// Razer Blade 14 (2023).
pub const PID_BLADE_14_2023: u16 = 0x029D;

/// Build a Huntsman V2 protocol instance.
pub fn build_huntsman_v2_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (6, 22),
            LED_ID_BACKLIGHT,
        )
        .with_init_custom_effect()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Basilisk V3 protocol instance.
pub fn build_basilisk_v3_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (1, 11),
            LED_ID_ZERO,
        )
        .without_device_mode_commands()
        .with_init_custom_effect()
        .with_write_only_custom_effect_activation(std::time::Duration::from_millis(10))
        .with_write_only_frame_uploads(),
    )
}

/// Build a Seiren Emote protocol instance.
pub fn build_seiren_emote_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (4, 16),
            LED_ID_ZERO,
        )
        .with_reported_matrix_size((8, 8))
        .with_init_custom_effect()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Seiren V3 Chroma protocol instance.
pub fn build_seiren_v3_protocol() -> Box<dyn Protocol> {
    Box::new(SeirenV3Protocol)
}

/// Build a Blade 15 (Late 2021 Advanced) protocol instance.
pub fn build_blade_15_late_2021_advanced_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

/// Build a Blade 14 (2021) protocol instance.
pub fn build_blade_14_2021_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads()
        .with_device_mode_keepalive(std::time::Duration::from_millis(2_500)),
    )
}

/// Build a Blade 15 (2022) protocol instance.
pub fn build_blade_15_2022_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

/// Build a Blade 14 (2023) protocol instance.
pub fn build_blade_14_2023_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

macro_rules! razer_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        interface: $interface:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: RAZER_VENDOR_ID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Razer,
            transport: TransportType::UsbHidRaw {
                interface: $interface,
                report_id: 0x00,
                usage_page: None,
                usage: None,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static RAZER_DESCRIPTORS: &[DeviceDescriptor] = &[
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_HUNTSMAN_V2,
        name: "Razer Huntsman V2",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbHidRaw {
            interface: 3,
            report_id: 0x00,
            usage_page: Some(RAZER_CONSUMER_USAGE_PAGE),
            usage: Some(RAZER_CONSUMER_USAGE),
        },
        protocol: ProtocolBinding {
            id: "razer/huntsman-v2",
            build: build_huntsman_v2_protocol,
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_BASILISK_V3,
        name: "Razer Basilisk V3",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbHidRaw {
            interface: 3,
            report_id: 0x00,
            usage_page: Some(RAZER_CONSUMER_USAGE_PAGE),
            usage: Some(RAZER_CONSUMER_USAGE),
        },
        protocol: ProtocolBinding {
            id: "razer/basilisk-v3",
            build: build_basilisk_v3_protocol,
        },
        firmware_predicate: None,
    },
    razer_descriptor!(
        pid: PID_SEIREN_EMOTE,
        name: "Razer Seiren Emote",
        protocol_id: "razer/seiren-emote",
        interface: 3,
        builder: build_seiren_emote_protocol
    ),
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_SEIREN_V3_CHROMA,
        name: "Razer Seiren V3 Chroma",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbHidRaw {
            interface: 3,
            report_id: 0x07,
            usage_page: Some(RAZER_VENDOR_USAGE_PAGE),
            usage: Some(RAZER_VENDOR_USAGE),
        },
        protocol: ProtocolBinding {
            id: "razer/seiren-v3-chroma",
            build: build_seiren_v3_protocol,
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_BLADE_14_2021,
        name: "Razer Blade 14 (2021)",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbControl {
            interface: 2,
            report_id: 0x00,
        },
        protocol: ProtocolBinding {
            id: "razer/blade-14-2021",
            build: build_blade_14_2021_protocol,
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_BLADE_15_LATE_2021_ADVANCED,
        name: "Razer Blade 15 (Late 2021 Advanced)",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbControl {
            interface: 2,
            report_id: 0x00,
        },
        protocol: ProtocolBinding {
            id: "razer/blade-15-late-2021-advanced",
            build: build_blade_15_late_2021_advanced_protocol,
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_BLADE_15_2022,
        name: "Razer Blade 15 (2022)",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbControl {
            interface: 2,
            report_id: 0x00,
        },
        protocol: ProtocolBinding {
            id: "razer/blade-15-2022",
            build: build_blade_15_2022_protocol,
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id: PID_BLADE_14_2023,
        name: "Razer Blade 14 (2023)",
        family: DeviceFamily::Razer,
        transport: TransportType::UsbControl {
            interface: 2,
            report_id: 0x00,
        },
        protocol: ProtocolBinding {
            id: "razer/blade-14-2023",
            build: build_blade_14_2023_protocol,
        },
        firmware_predicate: None,
    },
];

/// Static Razer descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    RAZER_DESCRIPTORS
}
