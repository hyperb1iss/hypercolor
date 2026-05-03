//! QMK HID RGB device registry entries.
//!
//! QMK keyboards are identified by their standard USB VID/PID. The RGB
//! protocol is accessed through a vendor-defined HID usage page (`0xFF60`,
//! usage `0x61`). Each keyboard entry specifies its known LED count,
//! protocol revision, and optional matrix dimensions.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding, TransportType};

use super::protocol::{QmkKeyboardConfig, QmkProtocol};
use super::types::{PACKET_SIZE, ProtocolRevision, USAGE_ID, USAGE_PAGE};

// ── Well-known QMK keyboard VIDs ─────────────────────────────────────────

/// Keychron vendor ID.
pub const VID_KEYCHRON: u16 = 0x3434;

/// ZSA Technology Labs (Moonlander, Voyager).
pub const VID_ZSA: u16 = 0x3297;

/// Drop vendor ID (OLKB).
pub const VID_DROP: u16 = 0xFEED;

/// IDOBAO vendor ID.
pub const VID_IDOBAO: u16 = 0x6964;

/// `KBDFans` vendor ID.
pub const VID_KBDFANS: u16 = 0x4B42;

/// `Cannonkeys` vendor ID.
pub const VID_CANNONKEYS: u16 = 0xCA04;

/// Glorious vendor ID.
pub const VID_GLORIOUS: u16 = 0x320F;

/// Sonix (used by many budget QMK boards).
pub const VID_SONIX: u16 = 0x0C45;

/// `WinChipHead` (CH552/CH558 based QMK boards).
pub const VID_WCH: u16 = 0x1A86;

// ── Builder functions ────────────────────────────────────────────────────

/// Keychron Q1 — 87 keys, revision D.
pub fn build_keychron_q1_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(87, ProtocolRevision::RevD).with_matrix(6, 16),
    ))
}

/// Keychron Q2 — 68 keys, revision D.
pub fn build_keychron_q2_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(68, ProtocolRevision::RevD).with_matrix(5, 15),
    ))
}

/// Keychron Q3 — full-size, 104 keys, revision D.
pub fn build_keychron_q3_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(104, ProtocolRevision::RevD).with_matrix(6, 20),
    ))
}

/// Keychron Q5 — 96%, 99 keys, revision D.
pub fn build_keychron_q5_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(99, ProtocolRevision::RevD).with_matrix(6, 19),
    ))
}

/// Keychron Q6 — full-size, 108 keys, revision D.
pub fn build_keychron_q6_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(108, ProtocolRevision::RevD).with_matrix(6, 21),
    ))
}

/// Keychron V1 — 75%, 84 keys, revision D.
pub fn build_keychron_v1_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(84, ProtocolRevision::RevD).with_matrix(6, 16),
    ))
}

/// ZSA Moonlander — 72 keys, revision D, with underglow.
pub fn build_moonlander_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(72, ProtocolRevision::RevD)
            .with_matrix(6, 14)
            .with_underglow(6),
    ))
}

/// ZSA Voyager — 52 keys, revision D.
pub fn build_voyager_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(52, ProtocolRevision::RevD).with_matrix(4, 14),
    ))
}

/// Drop OLKB Planck — 47 LEDs, ortholinear, revision D.
pub fn build_planck_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(47, ProtocolRevision::RevD).with_matrix(4, 12),
    ))
}

/// GMMK Pro (Glorious) — 100 LEDs, revision D, with underglow.
pub fn build_gmmk_pro_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(100, ProtocolRevision::RevD)
            .with_matrix(6, 16)
            .with_underglow(16),
    ))
}

/// Generic QMK 60% — 62 LEDs, revision D.
pub fn build_generic_60_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(62, ProtocolRevision::RevD).with_matrix(5, 14),
    ))
}

/// Generic QMK TKL — 87 LEDs, revision D.
pub fn build_generic_tkl_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(87, ProtocolRevision::RevD).with_matrix(6, 17),
    ))
}

/// Generic QMK full-size — 104 LEDs, revision D.
pub fn build_generic_fullsize_protocol() -> Box<dyn Protocol> {
    Box::new(QmkProtocol::new(
        QmkKeyboardConfig::new(104, ProtocolRevision::RevD).with_matrix(6, 21),
    ))
}

// ── Descriptor macro ─────────────────────────────────────────────────────

macro_rules! qmk_descriptor {
    (
        vid: $vid:expr,
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: $vid,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("qmk", "QMK"),
            transport: TransportType::UsbHidApi {
                interface: None,
                report_id: 0x00,
                report_mode: HidRawReportMode::OutputReportWithReportId,
                max_report_len: PACKET_SIZE,
                usage_page: Some(USAGE_PAGE),
                usage: Some(USAGE_ID),
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

// ── Device table ─────────────────────────────────────────────────────────

static QMK_DESCRIPTORS: &[DeviceDescriptor] = &[
    // ── Keychron ──
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0110,
        name: "Keychron Q1",
        protocol_id: "qmk/keychron-q1",
        builder: build_keychron_q1_protocol
    ),
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0120,
        name: "Keychron Q2",
        protocol_id: "qmk/keychron-q2",
        builder: build_keychron_q2_protocol
    ),
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0130,
        name: "Keychron Q3",
        protocol_id: "qmk/keychron-q3",
        builder: build_keychron_q3_protocol
    ),
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0150,
        name: "Keychron Q5",
        protocol_id: "qmk/keychron-q5",
        builder: build_keychron_q5_protocol
    ),
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0160,
        name: "Keychron Q6",
        protocol_id: "qmk/keychron-q6",
        builder: build_keychron_q6_protocol
    ),
    qmk_descriptor!(
        vid: VID_KEYCHRON,
        pid: 0x0310,
        name: "Keychron V1",
        protocol_id: "qmk/keychron-v1",
        builder: build_keychron_v1_protocol
    ),
    // ── ZSA ──
    qmk_descriptor!(
        vid: VID_ZSA,
        pid: 0x1969,
        name: "ZSA Moonlander",
        protocol_id: "qmk/zsa-moonlander",
        builder: build_moonlander_protocol
    ),
    qmk_descriptor!(
        vid: VID_ZSA,
        pid: 0x0791,
        name: "ZSA Voyager",
        protocol_id: "qmk/zsa-voyager",
        builder: build_voyager_protocol
    ),
    // ── Drop / OLKB ──
    qmk_descriptor!(
        vid: VID_DROP,
        pid: 0x6060,
        name: "OLKB Planck",
        protocol_id: "qmk/olkb-planck",
        builder: build_planck_protocol
    ),
    // ── Glorious ──
    qmk_descriptor!(
        vid: VID_GLORIOUS,
        pid: 0x5044,
        name: "Glorious GMMK Pro",
        protocol_id: "qmk/gmmk-pro",
        builder: build_gmmk_pro_protocol
    ),
];

/// Static QMK descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    QMK_DESCRIPTORS
}
