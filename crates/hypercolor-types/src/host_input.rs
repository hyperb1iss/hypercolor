//! Platform-neutral host input acquisition vocabulary.

use std::sync::Arc;

use crate::event::{PointerScrollPhase, PointerScrollUnit};

/// Input classes exposed by one native device lifetime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct HostInputCapabilities {
    pub keyboard: bool,
    pub pointer: bool,
}

/// Immutable identity for one native device lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInputDevice {
    pub source_id: Arc<str>,
    pub label: Arc<str>,
    pub capabilities: HostInputCapabilities,
    pub session_generation: u64,
    pub device_generation: u64,
}

/// A normalized key name plus its backend-native identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostKeyIdentity {
    pub key: Arc<str>,
    pub physical_code: Arc<str>,
}

/// Native evidence about whether a pressed report is automatic repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostRepeatEvidence {
    Unknown,
    NotRepeat,
    Repeat,
}

/// A native key report before shared held-state folding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeySignal {
    /// A direct hardware edge.
    Edge {
        pressed: bool,
        repeat: HostRepeatEvidence,
    },
    /// Aggregate modifier evidence, as reported by APIs such as CGEventTap.
    ///
    /// The fold compares `active` with held state for this key and its optional
    /// counterpart before deciding which physical edge occurred.
    AggregateState {
        active: bool,
        active_counterpart: Option<Arc<str>>,
    },
}

/// Extensible canonical name for a pointer button.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostPointerButton(Arc<str>);

impl HostPointerButton {
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn left() -> Self {
        Self::new("left")
    }

    #[must_use]
    pub fn right() -> Self {
        Self::new("right")
    }

    #[must_use]
    pub fn middle() -> Self {
        Self::new("middle")
    }

    #[must_use]
    pub fn side() -> Self {
        Self::new("side")
    }

    #[must_use]
    pub fn extra() -> Self {
        Self::new("extra")
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T> From<T> for HostPointerButton
where
    T: Into<Arc<str>>,
{
    fn from(name: T) -> Self {
        Self::new(name)
    }
}

/// Cursor position sampled with one native event drain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostPointerSnapshot {
    pub x: f64,
    pub y: f64,
    pub norm_x: f32,
    pub norm_y: f32,
    pub coordinate_space_generation: u64,
}

/// Pointer motion before shared cursor and delta folding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostPointerMotion {
    /// Relative native units with an explicit per-axis normalized scale.
    Relative {
        delta_x: f64,
        delta_y: f64,
        units_per_x: f64,
        units_per_y: f64,
    },
    /// Absolute normalized position in one identified coordinate space.
    Absolute {
        norm_x: f32,
        norm_y: f32,
        coordinate_space_generation: u64,
    },
}

/// Why the shared fold must discard assumptions about native held state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostInputGapReason {
    SynchronizationLost,
    QueueOverflow,
    ReadFailed,
    PermissionLost,
    SessionInterrupted,
    WorkerStopped,
}

/// One ordered neutral edge, motion report, or lifecycle marker.
#[derive(Debug, Clone, PartialEq)]
pub enum HostInputEvent {
    Key {
        device: Option<Arc<HostInputDevice>>,
        identity: HostKeyIdentity,
        signal: HostKeySignal,
    },
    Button {
        device: Option<Arc<HostInputDevice>>,
        button: HostPointerButton,
        pressed: bool,
        physical_code: Arc<str>,
    },
    Motion {
        device: Option<Arc<HostInputDevice>>,
        motion: HostPointerMotion,
    },
    Scroll {
        device: Option<Arc<HostInputDevice>>,
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
        unit: PointerScrollUnit,
        phase: PointerScrollPhase,
        momentum_phase: PointerScrollPhase,
        physical_code: Arc<str>,
    },
    DeviceArrived {
        device: Arc<HostInputDevice>,
    },
    StateGap {
        device: Option<Arc<HostInputDevice>>,
        reason: HostInputGapReason,
    },
    DeviceRemoved {
        device: Arc<HostInputDevice>,
    },
}

impl HostInputEvent {
    /// Native device attached to the event, when the backend can identify it.
    #[must_use]
    pub fn device(&self) -> Option<&Arc<HostInputDevice>> {
        match self {
            Self::Key { device, .. }
            | Self::Button { device, .. }
            | Self::Motion { device, .. }
            | Self::Scroll { device, .. }
            | Self::StateGap { device, .. } => device.as_ref(),
            Self::DeviceArrived { device } | Self::DeviceRemoved { device } => Some(device),
        }
    }
}

/// One coherent native publication.
#[derive(Debug)]
pub struct HostInputBatch<'a> {
    pub events: &'a [HostInputEvent],
    pub pointer: Option<HostPointerSnapshot>,
    pub at_ms: u64,
    /// Advances only when the native device catalog changes.
    pub device_catalog_generation: u64,
}

/// Primitive representation of a Windows set-1 scan-code prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostScanCodePrefix {
    None,
    E0,
    E1,
}

/// One physical key in every host platform's identifier space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostKeyRow {
    pub evdev_code: u16,
    pub windows_make_code: u16,
    pub windows_prefix: HostScanCodePrefix,
    pub macos_virtual_keycode: u16,
    pub name: &'static str,
}

const fn key_row(
    evdev_code: u16,
    windows_make_code: u16,
    windows_prefix: HostScanCodePrefix,
    name: &'static str,
) -> HostKeyRow {
    HostKeyRow {
        evdev_code,
        windows_make_code,
        windows_prefix,
        macos_virtual_keycode: macos_virtual_keycode(evdev_code),
        name,
    }
}

const fn macos_virtual_keycode(evdev_code: u16) -> u16 {
    match evdev_code {
        1 => 0x35,
        2 => 0x12,
        3 => 0x13,
        4 => 0x14,
        5 => 0x15,
        6 => 0x17,
        7 => 0x16,
        8 => 0x1A,
        9 => 0x1C,
        10 => 0x19,
        11 => 0x1D,
        12 => 0x1B,
        13 => 0x18,
        14 => 0x33,
        15 => 0x30,
        16 => 0x0C,
        17 => 0x0D,
        18 => 0x0E,
        19 => 0x0F,
        20 => 0x11,
        21 => 0x10,
        22 => 0x20,
        23 => 0x22,
        24 => 0x1F,
        25 => 0x23,
        26 => 0x21,
        27 => 0x1E,
        28 => 0x24,
        29 => 0x3B,
        30 => 0x00,
        31 => 0x01,
        32 => 0x02,
        33 => 0x03,
        34 => 0x05,
        35 => 0x04,
        36 => 0x26,
        37 => 0x28,
        38 => 0x25,
        39 => 0x29,
        40 => 0x27,
        41 => 0x32,
        42 => 0x38,
        43 => 0x2A,
        44 => 0x06,
        45 => 0x07,
        46 => 0x08,
        47 => 0x09,
        48 => 0x0B,
        49 => 0x2D,
        50 => 0x2E,
        51 => 0x2B,
        52 => 0x2F,
        53 => 0x2C,
        54 => 0x3C,
        55 => 0x43,
        56 => 0x3A,
        57 => 0x31,
        58 => 0x39,
        59 => 0x7A,
        60 => 0x78,
        61 => 0x63,
        62 => 0x76,
        63 => 0x60,
        64 => 0x61,
        65 => 0x62,
        66 => 0x64,
        67 => 0x65,
        68 => 0x6D,
        69 => 0x47,
        70 => 0x6B,
        71 => 0x59,
        72 => 0x5B,
        73 => 0x5C,
        74 => 0x4E,
        75 => 0x56,
        76 => 0x57,
        77 => 0x58,
        78 => 0x45,
        79 => 0x53,
        80 => 0x54,
        81 => 0x55,
        82 => 0x52,
        83 => 0x41,
        87 => 0x67,
        88 => 0x6F,
        96 => 0x4C,
        97 => 0x3E,
        98 => 0x4B,
        99 => 0x69,
        100 => 0x3D,
        102 => 0x73,
        103 => 0x7E,
        104 => 0x74,
        105 => 0x7B,
        106 => 0x7C,
        107 => 0x77,
        108 => 0x7D,
        109 => 0x79,
        110 => 0x72,
        111 => 0x75,
        125 => 0x37,
        126 => 0x36,
        127 => 0x6E,
        _ => panic!("canonical key lacks an explicit macOS mapping"),
    }
}

use HostScanCodePrefix::{E0, None as NoPrefix};

/// Canonical physical-position inventory shared by all host backends.
pub const HOST_KEYS: &[HostKeyRow] = &[
    key_row(1, 0x01, NoPrefix, "Escape"),
    key_row(2, 0x02, NoPrefix, "1"),
    key_row(3, 0x03, NoPrefix, "2"),
    key_row(4, 0x04, NoPrefix, "3"),
    key_row(5, 0x05, NoPrefix, "4"),
    key_row(6, 0x06, NoPrefix, "5"),
    key_row(7, 0x07, NoPrefix, "6"),
    key_row(8, 0x08, NoPrefix, "7"),
    key_row(9, 0x09, NoPrefix, "8"),
    key_row(10, 0x0A, NoPrefix, "9"),
    key_row(11, 0x0B, NoPrefix, "0"),
    key_row(12, 0x0C, NoPrefix, "-"),
    key_row(13, 0x0D, NoPrefix, "="),
    key_row(14, 0x0E, NoPrefix, "Backspace"),
    key_row(15, 0x0F, NoPrefix, "Tab"),
    key_row(16, 0x10, NoPrefix, "q"),
    key_row(17, 0x11, NoPrefix, "w"),
    key_row(18, 0x12, NoPrefix, "e"),
    key_row(19, 0x13, NoPrefix, "r"),
    key_row(20, 0x14, NoPrefix, "t"),
    key_row(21, 0x15, NoPrefix, "y"),
    key_row(22, 0x16, NoPrefix, "u"),
    key_row(23, 0x17, NoPrefix, "i"),
    key_row(24, 0x18, NoPrefix, "o"),
    key_row(25, 0x19, NoPrefix, "p"),
    key_row(26, 0x1A, NoPrefix, "["),
    key_row(27, 0x1B, NoPrefix, "]"),
    key_row(28, 0x1C, NoPrefix, "Enter"),
    key_row(29, 0x1D, NoPrefix, "ControlLeft"),
    key_row(30, 0x1E, NoPrefix, "a"),
    key_row(31, 0x1F, NoPrefix, "s"),
    key_row(32, 0x20, NoPrefix, "d"),
    key_row(33, 0x21, NoPrefix, "f"),
    key_row(34, 0x22, NoPrefix, "g"),
    key_row(35, 0x23, NoPrefix, "h"),
    key_row(36, 0x24, NoPrefix, "j"),
    key_row(37, 0x25, NoPrefix, "k"),
    key_row(38, 0x26, NoPrefix, "l"),
    key_row(39, 0x27, NoPrefix, ";"),
    key_row(40, 0x28, NoPrefix, "'"),
    key_row(41, 0x29, NoPrefix, "`"),
    key_row(42, 0x2A, NoPrefix, "ShiftLeft"),
    key_row(43, 0x2B, NoPrefix, "\\"),
    key_row(44, 0x2C, NoPrefix, "z"),
    key_row(45, 0x2D, NoPrefix, "x"),
    key_row(46, 0x2E, NoPrefix, "c"),
    key_row(47, 0x2F, NoPrefix, "v"),
    key_row(48, 0x30, NoPrefix, "b"),
    key_row(49, 0x31, NoPrefix, "n"),
    key_row(50, 0x32, NoPrefix, "m"),
    key_row(51, 0x33, NoPrefix, ","),
    key_row(52, 0x34, NoPrefix, "."),
    key_row(53, 0x35, NoPrefix, "/"),
    key_row(54, 0x36, NoPrefix, "ShiftRight"),
    key_row(55, 0x37, NoPrefix, "NumpadMultiply"),
    key_row(56, 0x38, NoPrefix, "AltLeft"),
    key_row(57, 0x39, NoPrefix, "Space"),
    key_row(58, 0x3A, NoPrefix, "CapsLock"),
    key_row(59, 0x3B, NoPrefix, "F1"),
    key_row(60, 0x3C, NoPrefix, "F2"),
    key_row(61, 0x3D, NoPrefix, "F3"),
    key_row(62, 0x3E, NoPrefix, "F4"),
    key_row(63, 0x3F, NoPrefix, "F5"),
    key_row(64, 0x40, NoPrefix, "F6"),
    key_row(65, 0x41, NoPrefix, "F7"),
    key_row(66, 0x42, NoPrefix, "F8"),
    key_row(67, 0x43, NoPrefix, "F9"),
    key_row(68, 0x44, NoPrefix, "F10"),
    key_row(69, 0x45, NoPrefix, "NumLock"),
    key_row(70, 0x46, NoPrefix, "ScrollLock"),
    key_row(71, 0x47, NoPrefix, "Numpad7"),
    key_row(72, 0x48, NoPrefix, "Numpad8"),
    key_row(73, 0x49, NoPrefix, "Numpad9"),
    key_row(74, 0x4A, NoPrefix, "NumpadSubtract"),
    key_row(75, 0x4B, NoPrefix, "Numpad4"),
    key_row(76, 0x4C, NoPrefix, "Numpad5"),
    key_row(77, 0x4D, NoPrefix, "Numpad6"),
    key_row(78, 0x4E, NoPrefix, "NumpadAdd"),
    key_row(79, 0x4F, NoPrefix, "Numpad1"),
    key_row(80, 0x50, NoPrefix, "Numpad2"),
    key_row(81, 0x51, NoPrefix, "Numpad3"),
    key_row(82, 0x52, NoPrefix, "Numpad0"),
    key_row(83, 0x53, NoPrefix, "NumpadDecimal"),
    key_row(87, 0x57, NoPrefix, "F11"),
    key_row(88, 0x58, NoPrefix, "F12"),
    key_row(96, 0x1C, E0, "NumpadEnter"),
    key_row(97, 0x1D, E0, "ControlRight"),
    key_row(98, 0x35, E0, "NumpadDivide"),
    key_row(99, 0x37, E0, "PrintScreen"),
    key_row(100, 0x38, E0, "AltRight"),
    key_row(102, 0x47, E0, "Home"),
    key_row(103, 0x48, E0, "ArrowUp"),
    key_row(104, 0x49, E0, "PageUp"),
    key_row(105, 0x4B, E0, "ArrowLeft"),
    key_row(106, 0x4D, E0, "ArrowRight"),
    key_row(107, 0x4F, E0, "End"),
    key_row(108, 0x50, E0, "ArrowDown"),
    key_row(109, 0x51, E0, "PageDown"),
    key_row(110, 0x52, E0, "Insert"),
    key_row(111, 0x53, E0, "Delete"),
    key_row(125, 0x5B, E0, "MetaLeft"),
    key_row(126, 0x5C, E0, "MetaRight"),
    key_row(127, 0x5D, E0, "ContextMenu"),
];

/// One consumer-control key in each host platform's identifier space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMediaKeyRow {
    pub evdev_code: u16,
    pub windows_vkey: u16,
    pub macos_nx_key_type: Option<u16>,
    pub name: &'static str,
}

/// Canonical media-key inventory shared by all host backends.
pub const HOST_MEDIA_KEYS: &[HostMediaKeyRow] = &[
    HostMediaKeyRow {
        evdev_code: 113,
        windows_vkey: 0xAD,
        macos_nx_key_type: Some(7),
        name: "AudioVolumeMute",
    },
    HostMediaKeyRow {
        evdev_code: 114,
        windows_vkey: 0xAE,
        macos_nx_key_type: Some(1),
        name: "AudioVolumeDown",
    },
    HostMediaKeyRow {
        evdev_code: 115,
        windows_vkey: 0xAF,
        macos_nx_key_type: Some(0),
        name: "AudioVolumeUp",
    },
    HostMediaKeyRow {
        evdev_code: 163,
        windows_vkey: 0xB0,
        macos_nx_key_type: Some(17),
        name: "MediaTrackNext",
    },
    HostMediaKeyRow {
        evdev_code: 165,
        windows_vkey: 0xB1,
        macos_nx_key_type: Some(18),
        name: "MediaTrackPrevious",
    },
    HostMediaKeyRow {
        evdev_code: 166,
        windows_vkey: 0xB2,
        macos_nx_key_type: None,
        name: "MediaStop",
    },
    HostMediaKeyRow {
        evdev_code: 164,
        windows_vkey: 0xB3,
        macos_nx_key_type: Some(16),
        name: "MediaPlayPause",
    },
    HostMediaKeyRow {
        evdev_code: 226,
        windows_vkey: 0xB5,
        macos_nx_key_type: None,
        name: "MediaSelect",
    },
];

/// Canonical physical or media key name for a Linux evdev code.
#[must_use]
pub fn host_key_name_from_evdev(code: u16) -> Option<&'static str> {
    HOST_KEYS
        .iter()
        .find(|row| row.evdev_code == code)
        .map(|row| row.name)
        .or_else(|| {
            HOST_MEDIA_KEYS
                .iter()
                .find(|row| row.evdev_code == code)
                .map(|row| row.name)
        })
}

/// Canonical physical key name for a Windows set-1 scan code.
#[must_use]
pub fn host_key_name_from_windows(
    make_code: u16,
    prefix: HostScanCodePrefix,
) -> Option<&'static str> {
    HOST_KEYS
        .iter()
        .find(|row| row.windows_make_code == make_code && row.windows_prefix == prefix)
        .map(|row| row.name)
}

/// Canonical consumer-control key name for a Windows virtual-key code.
#[must_use]
pub fn host_media_key_name_from_windows(vkey: u16) -> Option<&'static str> {
    HOST_MEDIA_KEYS
        .iter()
        .find(|row| row.windows_vkey == vkey)
        .map(|row| row.name)
}

/// Canonical physical key name for a macOS virtual keycode.
#[must_use]
pub fn host_key_name_from_macos(virtual_keycode: u16) -> Option<&'static str> {
    HOST_KEYS
        .iter()
        .find(|row| row.macos_virtual_keycode == virtual_keycode)
        .map(|row| row.name)
}

/// Canonical consumer-control key name for a macOS `NX_KEYTYPE` value.
#[must_use]
pub fn host_media_key_name_from_macos(nx_key_type: u16) -> Option<&'static str> {
    HOST_MEDIA_KEYS
        .iter()
        .find(|row| row.macos_nx_key_type == Some(nx_key_type))
        .map(|row| row.name)
}
