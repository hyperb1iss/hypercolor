//! The canonical key inventory both host backends are built from.
//!
//! Names in this codebase are **physical-position** names, `KeyboardEvent.code`
//! semantics: `a` is the key where `A` sits on QWERTY, whatever the active
//! layout prints on it. That is what makes `wasdVector()` and every positional
//! effect mean the same thing on a French AZERTY keyboard as on a US one.
//!
//! Two platforms have to agree on those names, and the way they drift is for
//! someone to add a key to one table and forget the other. So there is exactly
//! one physical table, [`CANONICAL_KEYS`], and both mappers are derived from it — the
//! Linux one keyed by evdev code, the Windows one by set-1 scan code plus
//! prefix. The parity test asserts totality in both directions over this
//! inventory rather than sampling tuples, so a one-sided addition fails the
//! build instead of silently diverging.
//!
//! Consumer-control keys do not have stable set-1 positions. They live in the
//! separate [`MEDIA_KEYS`] inventory, keyed by evdev code and Windows virtual
//! key, so both platforms still expose exactly the same logical names.
//!
//! The two key spaces line up almost entirely by construction: evdev's
//! keycodes 1..=83 were derived from the AT set-1 scan codes, so those rows
//! carry the same number twice. The extended block is where they part company,
//! and those rows carry an `E0` prefix on the Windows side.

use hypercolor_windows_input::RawKeyPrefix;
use hypercolor_windows_input::decode::{KEYBOARD_OVERRUN_MAKE_CODE, unknown_key_name};

/// One physical key, in both platforms' identifier spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRow {
    /// Linux evdev keycode.
    pub evdev_code: u16,
    /// Windows set-1 scan code.
    pub make_code: u16,
    /// Windows scan-code prefix.
    pub prefix: RawKeyPrefix,
    /// The canonical name both platforms report.
    pub name: &'static str,
}

const fn row(evdev_code: u16, make_code: u16, prefix: RawKeyPrefix, name: &'static str) -> KeyRow {
    KeyRow {
        evdev_code,
        make_code,
        prefix,
        name,
    }
}

use RawKeyPrefix::{E0, None as NoPrefix};

/// Every physical-position key both host backends name identically.
///
/// Printable keys use the character they produce on a US layout — a
/// deliberate simplification of the W3C `code` vocabulary that this codebase
/// already established (`a` rather than `KeyA`, `1` rather than `Digit1`).
/// Everything else uses the W3C name.
pub const CANONICAL_KEYS: &[KeyRow] = &[
    row(1, 0x01, NoPrefix, "Escape"),
    row(2, 0x02, NoPrefix, "1"),
    row(3, 0x03, NoPrefix, "2"),
    row(4, 0x04, NoPrefix, "3"),
    row(5, 0x05, NoPrefix, "4"),
    row(6, 0x06, NoPrefix, "5"),
    row(7, 0x07, NoPrefix, "6"),
    row(8, 0x08, NoPrefix, "7"),
    row(9, 0x09, NoPrefix, "8"),
    row(10, 0x0A, NoPrefix, "9"),
    row(11, 0x0B, NoPrefix, "0"),
    row(12, 0x0C, NoPrefix, "-"),
    row(13, 0x0D, NoPrefix, "="),
    row(14, 0x0E, NoPrefix, "Backspace"),
    row(15, 0x0F, NoPrefix, "Tab"),
    row(16, 0x10, NoPrefix, "q"),
    row(17, 0x11, NoPrefix, "w"),
    row(18, 0x12, NoPrefix, "e"),
    row(19, 0x13, NoPrefix, "r"),
    row(20, 0x14, NoPrefix, "t"),
    row(21, 0x15, NoPrefix, "y"),
    row(22, 0x16, NoPrefix, "u"),
    row(23, 0x17, NoPrefix, "i"),
    row(24, 0x18, NoPrefix, "o"),
    row(25, 0x19, NoPrefix, "p"),
    row(26, 0x1A, NoPrefix, "["),
    row(27, 0x1B, NoPrefix, "]"),
    row(28, 0x1C, NoPrefix, "Enter"),
    row(29, 0x1D, NoPrefix, "ControlLeft"),
    row(30, 0x1E, NoPrefix, "a"),
    row(31, 0x1F, NoPrefix, "s"),
    row(32, 0x20, NoPrefix, "d"),
    row(33, 0x21, NoPrefix, "f"),
    row(34, 0x22, NoPrefix, "g"),
    row(35, 0x23, NoPrefix, "h"),
    row(36, 0x24, NoPrefix, "j"),
    row(37, 0x25, NoPrefix, "k"),
    row(38, 0x26, NoPrefix, "l"),
    row(39, 0x27, NoPrefix, ";"),
    row(40, 0x28, NoPrefix, "'"),
    row(41, 0x29, NoPrefix, "`"),
    row(42, 0x2A, NoPrefix, "ShiftLeft"),
    row(43, 0x2B, NoPrefix, "\\"),
    row(44, 0x2C, NoPrefix, "z"),
    row(45, 0x2D, NoPrefix, "x"),
    row(46, 0x2E, NoPrefix, "c"),
    row(47, 0x2F, NoPrefix, "v"),
    row(48, 0x30, NoPrefix, "b"),
    row(49, 0x31, NoPrefix, "n"),
    row(50, 0x32, NoPrefix, "m"),
    row(51, 0x33, NoPrefix, ","),
    row(52, 0x34, NoPrefix, "."),
    row(53, 0x35, NoPrefix, "/"),
    row(54, 0x36, NoPrefix, "ShiftRight"),
    row(55, 0x37, NoPrefix, "NumpadMultiply"),
    row(56, 0x38, NoPrefix, "AltLeft"),
    row(57, 0x39, NoPrefix, "Space"),
    row(58, 0x3A, NoPrefix, "CapsLock"),
    row(59, 0x3B, NoPrefix, "F1"),
    row(60, 0x3C, NoPrefix, "F2"),
    row(61, 0x3D, NoPrefix, "F3"),
    row(62, 0x3E, NoPrefix, "F4"),
    row(63, 0x3F, NoPrefix, "F5"),
    row(64, 0x40, NoPrefix, "F6"),
    row(65, 0x41, NoPrefix, "F7"),
    row(66, 0x42, NoPrefix, "F8"),
    row(67, 0x43, NoPrefix, "F9"),
    row(68, 0x44, NoPrefix, "F10"),
    row(69, 0x45, NoPrefix, "NumLock"),
    row(70, 0x46, NoPrefix, "ScrollLock"),
    row(71, 0x47, NoPrefix, "Numpad7"),
    row(72, 0x48, NoPrefix, "Numpad8"),
    row(73, 0x49, NoPrefix, "Numpad9"),
    row(74, 0x4A, NoPrefix, "NumpadSubtract"),
    row(75, 0x4B, NoPrefix, "Numpad4"),
    row(76, 0x4C, NoPrefix, "Numpad5"),
    row(77, 0x4D, NoPrefix, "Numpad6"),
    row(78, 0x4E, NoPrefix, "NumpadAdd"),
    row(79, 0x4F, NoPrefix, "Numpad1"),
    row(80, 0x50, NoPrefix, "Numpad2"),
    row(81, 0x51, NoPrefix, "Numpad3"),
    row(82, 0x52, NoPrefix, "Numpad0"),
    row(83, 0x53, NoPrefix, "NumpadDecimal"),
    row(87, 0x57, NoPrefix, "F11"),
    row(88, 0x58, NoPrefix, "F12"),
    // The extended block: same scan codes as above, distinguished only by the
    // E0 prefix byte. Collapsing the prefix into a boolean would still work
    // here; it is E1 (Pause) that the three-state prefix exists for.
    row(96, 0x1C, E0, "NumpadEnter"),
    row(97, 0x1D, E0, "ControlRight"),
    row(98, 0x35, E0, "NumpadDivide"),
    row(99, 0x37, E0, "PrintScreen"),
    row(100, 0x38, E0, "AltRight"),
    row(102, 0x47, E0, "Home"),
    row(103, 0x48, E0, "ArrowUp"),
    row(104, 0x49, E0, "PageUp"),
    row(105, 0x4B, E0, "ArrowLeft"),
    row(106, 0x4D, E0, "ArrowRight"),
    row(107, 0x4F, E0, "End"),
    row(108, 0x50, E0, "ArrowDown"),
    row(109, 0x51, E0, "PageDown"),
    row(110, 0x52, E0, "Insert"),
    row(111, 0x53, E0, "Delete"),
    row(125, 0x5B, E0, "MetaLeft"),
    row(126, 0x5C, E0, "MetaRight"),
    row(127, 0x5D, E0, "ContextMenu"),
];

/// One media key in Linux evdev and Windows virtual-key spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaKeyRow {
    /// Linux evdev keycode.
    pub evdev_code: u16,
    /// Windows virtual-key code.
    pub windows_vkey: u16,
    /// The canonical logical name both platforms report.
    pub name: &'static str,
}

const fn media_row(evdev_code: u16, windows_vkey: u16, name: &'static str) -> MediaKeyRow {
    MediaKeyRow {
        evdev_code,
        windows_vkey,
        name,
    }
}

/// Media and volume keys supported identically by both host backends.
pub const MEDIA_KEYS: &[MediaKeyRow] = &[
    media_row(113, 0xAD, "AudioVolumeMute"),
    media_row(114, 0xAE, "AudioVolumeDown"),
    media_row(115, 0xAF, "AudioVolumeUp"),
    media_row(163, 0xB0, "MediaTrackNext"),
    media_row(165, 0xB1, "MediaTrackPrevious"),
    media_row(166, 0xB2, "MediaStop"),
    media_row(164, 0xB3, "MediaPlayPause"),
    media_row(226, 0xB5, "MediaSelect"),
];

/// Canonical name for a Linux evdev keycode.
#[must_use]
pub fn evdev_key_name(evdev_code: u16) -> Option<&'static str> {
    CANONICAL_KEYS
        .iter()
        .find(|row| row.evdev_code == evdev_code)
        .map(|row| row.name)
        .or_else(|| {
            MEDIA_KEYS
                .iter()
                .find(|row| row.evdev_code == evdev_code)
                .map(|row| row.name)
        })
}

/// Canonical name for a Windows scan code plus prefix.
#[must_use]
pub fn scancode_name(make_code: u16, prefix: RawKeyPrefix) -> Option<&'static str> {
    CANONICAL_KEYS
        .iter()
        .find(|row| row.make_code == make_code && row.prefix == prefix)
        .map(|row| row.name)
}

/// Where a resolved key name came from.
///
/// The provenance matters and callers must not be able to lose it: a
/// `VKey`-derived name is layout-dependent, so treating one as positional
/// would reintroduce exactly the AZERTY divergence the scan-code path exists
/// to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyNameResult {
    /// A physical position from [`CANONICAL_KEYS`]. Portable across platforms
    /// and layouts.
    Positional(&'static str),
    /// A logical consumer-control key from [`MEDIA_KEYS`]. Portable across
    /// platforms even though it has no stable keyboard position.
    Media(&'static str),
    /// Derived from the virtual-key code, which is layout-dependent. Only
    /// reached for keys with no positional meaning to diverge on.
    Logical(&'static str),
    /// Recognized as a key, but outside the canonical inventory. Named
    /// stably so an effect author can see it rather than debug a silent drop.
    Unknown(String),
    /// Not a key at all — a rollover overrun report.
    Discard,
}

impl KeyNameResult {
    /// The name to report, if this result names a key at all.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        match self {
            Self::Positional(name) | Self::Media(name) | Self::Logical(name) => {
                Some((*name).to_owned())
            }
            Self::Unknown(name) => Some(name.clone()),
            Self::Discard => None,
        }
    }
}

/// Resolve one Windows key report to a canonical name.
///
/// Takes no Windows types, so it compiles and unit-tests on every platform.
pub fn scancode_key_name(make_code: u16, prefix: RawKeyPrefix, vkey: u16) -> KeyNameResult {
    if make_code == KEYBOARD_OVERRUN_MAKE_CODE {
        return KeyNameResult::Discard;
    }

    // Pause is identified by its virtual key rather than by E1 alone: Raw
    // Input does not contract that E1 means Pause, and force-fitting every
    // unrecognized E1 sequence to "Pause" would name a key nobody pressed.
    if prefix == RawKeyPrefix::E1 {
        if vkey == VK_PAUSE {
            return KeyNameResult::Positional("Pause");
        }
        return KeyNameResult::Unknown(unknown_key_name(make_code, prefix));
    }

    if let Some(name) = media_key_name(vkey) {
        return KeyNameResult::Media(name);
    }

    if let Some(name) = scancode_name(make_code, prefix) {
        return KeyNameResult::Positional(name);
    }

    // A scan code of zero carries no position at all, so the virtual key is
    // the only identifier left. This is the one place a non-QWERTY layout
    // could produce a different name than Linux would — acceptable only
    // because these keys have no positional meaning to diverge on.
    if make_code == 0
        && let Some(name) = virtual_key_name(vkey)
    {
        return KeyNameResult::Logical(name);
    }

    KeyNameResult::Unknown(unknown_key_name(make_code, prefix))
}

/// `VK_PAUSE`.
const VK_PAUSE: u16 = 0x13;

/// Names for keys that arrive with no scan code.
///
/// Deliberately short: these are the media and browser keys a keyboard exposes
/// through its own top-level collection with no positional identity. A key
/// with a real position must never land here.
fn virtual_key_name(vkey: u16) -> Option<&'static str> {
    match vkey {
        0xA6 => Some("BrowserBack"),
        0xA7 => Some("BrowserForward"),
        0xA8 => Some("BrowserRefresh"),
        0xA9 => Some("BrowserStop"),
        0xAA => Some("BrowserSearch"),
        0xAB => Some("BrowserFavorites"),
        0xAC => Some("BrowserHome"),
        0xB4 => Some("LaunchMail"),
        0xB6 => Some("LaunchApp1"),
        0xB7 => Some("LaunchApp2"),
        _ => None,
    }
}

fn media_key_name(vkey: u16) -> Option<&'static str> {
    MEDIA_KEYS
        .iter()
        .find(|row| row.windows_vkey == vkey)
        .map(|row| row.name)
}
