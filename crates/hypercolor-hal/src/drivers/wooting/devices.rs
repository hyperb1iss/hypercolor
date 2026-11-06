//! Wooting device registry (Spec 90 §2, §8).
//!
//! Every descriptor reaches the keyboard through the host HID stack on the
//! vendor configuration collection, selected by usage page exactly as the
//! vendor SDK selects it. Nothing here opens the USB device or claims an
//! interface, so the kernel's keyboard, consumer, mouse, analog, and gamepad
//! functions stay bound while Hypercolor drives the lighting.

use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{
    DeviceDescriptor, HidRawReportMode, ProtocolBinding, ProtocolFactory, TransportType,
};

use super::protocol::WootingProtocol;
use super::types::{
    CONFIG_USAGE_PAGE, GamepadMode, MULTI_REPORT_FEATURE_REPORT_ID, MULTI_REPORT_USAGE_PAGE,
    UNNUMBERED_REPORT_ID, V1_REPORT_LEN, V2_REPORT_LEN, V3_REPORT_LEN, WOOTING_LEGACY_VID,
    WOOTING_VID, WootingInterface, WootingModel,
};

/// Device family shared by every Wooting descriptor.
pub const WOOTING_FAMILY: DeviceFamily = DeviceFamily::new_static("wooting", "Wooting");

/// Wooting One on v1 firmware (Atmel VID).
pub const PID_ONE_LEGACY: u16 = 0xFF01;
/// Wooting Two on v1 firmware (Atmel VID).
pub const PID_TWO_LEGACY: u16 = 0xFF02;
/// Wooting One base product ID.
pub const PID_ONE: u16 = 0x1100;
/// Wooting Two base product ID.
pub const PID_TWO: u16 = 0x1200;
/// Wooting Two Lekker Edition base product ID.
pub const PID_TWO_LEKKER: u16 = 0x1210;
/// Wooting Two HE base product ID.
pub const PID_TWO_HE: u16 = 0x1220;
/// Wooting Two HE (ARM) base product ID.
pub const PID_TWO_HE_ARM: u16 = 0x1230;
/// Wooting 60HE base product ID.
pub const PID_SIXTY_HE: u16 = 0x1300;
/// Wooting 60HE (ARM) base product ID.
pub const PID_SIXTY_HE_ARM: u16 = 0x1310;
/// Wooting 60HE+ base product ID.
pub const PID_SIXTY_HE_PLUS: u16 = 0x1320;
/// Wooting 60HE v2 base product ID.
pub const PID_SIXTY_HE_V2: u16 = 0x1340;
/// Wooting 80HE base product ID.
pub const PID_EIGHTY_HE: u16 = 0x1400;
/// Wooting UwU RGB base product ID.
pub const PID_UWU_RGB: u16 = 0x1510;

/// HID transport for one Wooting wire generation.
///
/// The configuration collection is chosen by usage page alone, never by
/// interface number, since gamepad emulation adds an interface to the
/// device. Packets carry their own report ID, and commands travel as
/// feature reports on the same handle.
#[must_use]
pub const fn transport_for(interface: WootingInterface) -> TransportType {
    let (report_id, max_report_len, usage_page) = match interface {
        WootingInterface::V1 => (UNNUMBERED_REPORT_ID, V1_REPORT_LEN, CONFIG_USAGE_PAGE),
        WootingInterface::V2 { .. } => (UNNUMBERED_REPORT_ID, V2_REPORT_LEN, CONFIG_USAGE_PAGE),
        WootingInterface::V3 => (
            MULTI_REPORT_FEATURE_REPORT_ID,
            V3_REPORT_LEN,
            MULTI_REPORT_USAGE_PAGE,
        ),
    };

    TransportType::UsbHidApi {
        interface: None,
        report_id,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len,
        usage_page: Some(usage_page),
        usage: None,
    }
}

macro_rules! wooting_builder {
    ($(#[$doc:meta])* $name:ident, $model:ident, $interface:expr) => {
        $(#[$doc])*
        #[must_use]
        pub fn $name() -> Box<dyn Protocol> {
            Box::new(WootingProtocol::new(WootingModel::$model, $interface))
        }
    };
}

wooting_builder!(
    /// Wooting One on v1 firmware.
    build_one_v1, One, WootingInterface::V1
);
wooting_builder!(
    /// Wooting Two on v1 firmware.
    build_two_v1, Two, WootingInterface::V1
);
wooting_builder!(
    /// Wooting One on v2 firmware.
    build_one, One, WootingInterface::V2 { small_packets: false }
);
wooting_builder!(
    /// Wooting Two on v2 firmware.
    build_two, Two, WootingInterface::V2 { small_packets: false }
);
wooting_builder!(
    /// Wooting Two Lekker Edition on v2 firmware.
    build_two_lekker, TwoLekker, WootingInterface::V2 { small_packets: false }
);
wooting_builder!(
    /// Wooting Two HE on v2 firmware.
    build_two_he, TwoHe, WootingInterface::V2 { small_packets: false }
);
wooting_builder!(
    /// Wooting 60HE on v2 firmware.
    build_sixty_he, SixtyHe, WootingInterface::V2 { small_packets: false }
);
wooting_builder!(
    /// Wooting Two HE (ARM) on the multi-report collection.
    build_two_he_arm, TwoHeArm, WootingInterface::V3
);
wooting_builder!(
    /// Wooting 60HE (ARM) on the multi-report collection.
    build_sixty_he_arm, SixtyHeArm, WootingInterface::V3
);
wooting_builder!(
    /// Wooting 60HE+ on the multi-report collection.
    build_sixty_he_plus, SixtyHePlus, WootingInterface::V3
);
wooting_builder!(
    /// Wooting 60HE v2 on the multi-report collection.
    build_sixty_he_v2, SixtyHeV2, WootingInterface::V3
);
wooting_builder!(
    /// Wooting 80HE on the multi-report collection.
    build_eighty_he, EightyHe, WootingInterface::V3
);
wooting_builder!(
    /// Wooting UwU RGB on the multi-report collection.
    build_uwu_rgb, UwuRgb, WootingInterface::V3
);

/// One registered Wooting model on one wire generation.
#[derive(Debug, Clone, Copy)]
pub struct WootingModelEntry {
    /// Vendor ID the firmware enumerates with.
    pub vendor_id: u16,
    /// Product ID with gamepad emulation set to Xbox (or the only PID, for
    /// v1 firmware).
    pub base_pid: u16,
    /// Whether the firmware adds the gamepad mode to the low PID nibble.
    pub gamepad_variants: bool,
    /// Human-readable device name.
    pub name: &'static str,
    /// Stable protocol ID shared by the model's gamepad variants.
    pub protocol_id: &'static str,
    /// Model geometry.
    pub model: WootingModel,
    /// Wire generation the descriptor binds.
    pub interface: WootingInterface,
    /// Protocol constructor.
    pub build: ProtocolFactory,
}

const fn entry(
    vendor_id: u16,
    base_pid: u16,
    gamepad_variants: bool,
    name: &'static str,
    protocol_id: &'static str,
    model: WootingModel,
    interface: WootingInterface,
    build: ProtocolFactory,
) -> WootingModelEntry {
    WootingModelEntry {
        vendor_id,
        base_pid,
        gamepad_variants,
        name,
        protocol_id,
        model,
        interface,
        build,
    }
}

const V2_LARGE: WootingInterface = WootingInterface::V2 {
    small_packets: false,
};

/// Every Wooting model Hypercolor registers, with the wire generation each
/// one's current firmware exposes (Spec 90 §3.2).
pub const MODELS: &[WootingModelEntry] = &[
    entry(
        WOOTING_LEGACY_VID,
        PID_ONE_LEGACY,
        false,
        "Wooting One (v1 firmware)",
        "wooting/one-v1",
        WootingModel::One,
        WootingInterface::V1,
        build_one_v1,
    ),
    entry(
        WOOTING_LEGACY_VID,
        PID_TWO_LEGACY,
        false,
        "Wooting Two (v1 firmware)",
        "wooting/two-v1",
        WootingModel::Two,
        WootingInterface::V1,
        build_two_v1,
    ),
    entry(
        WOOTING_VID,
        PID_ONE,
        true,
        "Wooting One",
        "wooting/one",
        WootingModel::One,
        V2_LARGE,
        build_one,
    ),
    entry(
        WOOTING_VID,
        PID_TWO,
        true,
        "Wooting Two",
        "wooting/two",
        WootingModel::Two,
        V2_LARGE,
        build_two,
    ),
    entry(
        WOOTING_VID,
        PID_TWO_LEKKER,
        true,
        "Wooting Two Lekker Edition",
        "wooting/two-lekker",
        WootingModel::TwoLekker,
        V2_LARGE,
        build_two_lekker,
    ),
    entry(
        WOOTING_VID,
        PID_TWO_HE,
        true,
        "Wooting Two HE",
        "wooting/two-he",
        WootingModel::TwoHe,
        V2_LARGE,
        build_two_he,
    ),
    entry(
        WOOTING_VID,
        PID_TWO_HE_ARM,
        true,
        "Wooting Two HE (ARM)",
        "wooting/two-he-arm",
        WootingModel::TwoHeArm,
        WootingInterface::V3,
        build_two_he_arm,
    ),
    entry(
        WOOTING_VID,
        PID_SIXTY_HE,
        true,
        "Wooting 60HE",
        "wooting/60he",
        WootingModel::SixtyHe,
        V2_LARGE,
        build_sixty_he,
    ),
    entry(
        WOOTING_VID,
        PID_SIXTY_HE_ARM,
        true,
        "Wooting 60HE (ARM)",
        "wooting/60he-arm",
        WootingModel::SixtyHeArm,
        WootingInterface::V3,
        build_sixty_he_arm,
    ),
    entry(
        WOOTING_VID,
        PID_SIXTY_HE_PLUS,
        true,
        "Wooting 60HE+",
        "wooting/60he-plus",
        WootingModel::SixtyHePlus,
        WootingInterface::V3,
        build_sixty_he_plus,
    ),
    entry(
        WOOTING_VID,
        PID_SIXTY_HE_V2,
        true,
        "Wooting 60HE v2",
        "wooting/60he-v2",
        WootingModel::SixtyHeV2,
        WootingInterface::V3,
        build_sixty_he_v2,
    ),
    entry(
        WOOTING_VID,
        PID_EIGHTY_HE,
        true,
        "Wooting 80HE",
        "wooting/80he",
        WootingModel::EightyHe,
        WootingInterface::V3,
        build_eighty_he,
    ),
    entry(
        WOOTING_VID,
        PID_UWU_RGB,
        true,
        "Wooting UwU RGB",
        "wooting/uwu-rgb",
        WootingModel::UwuRgb,
        WootingInterface::V3,
        build_uwu_rgb,
    ),
];

impl WootingModelEntry {
    /// Product IDs this model enumerates with.
    pub fn product_ids(&self) -> impl Iterator<Item = u16> + '_ {
        let variants: &[GamepadMode] = if self.gamepad_variants {
            &GamepadMode::ALL
        } else {
            &[GamepadMode::Xinput]
        };
        variants
            .iter()
            .map(move |mode| self.base_pid | mode.pid_offset())
    }
}

static WOOTING_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    MODELS
        .iter()
        .flat_map(|model| {
            model.product_ids().map(move |product_id| DeviceDescriptor {
                vendor_id: model.vendor_id,
                product_id,
                name: model.name,
                family: WOOTING_FAMILY,
                transport: transport_for(model.interface),
                protocol: ProtocolBinding {
                    id: model.protocol_id,
                    build: model.build,
                },
                firmware_predicate: None,
                serial_quirk: None,
            })
        })
        .collect()
});

/// All registered Wooting descriptors.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    WOOTING_DESCRIPTORS.as_slice()
}
