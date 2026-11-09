//! Which child names the Win32 layer reads the way handle-relative NT calls
//! do.
//!
//! The rule is plain data, so it compiles on every host: the Windows
//! directory authority refuses these names before it opens anything, and
//! code that writes or checks a Windows release unit elsewhere (a manifest
//! built on a Linux runner, a test fixture) holds its paths to the same
//! rule.

/// Names Win32 maps to a device whatever extension follows them.
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The reason `wide` is not one plain child name, or `None` when it is.
/// `text` is the same name as UTF-8 when it has one; the device-name check
/// needs it.
pub(crate) fn name_refusal(wide: &[u16], text: Option<&str>) -> Option<&'static str> {
    if wide.is_empty() || wide.len() > 255 {
        return Some("it must hold 1 to 255 UTF-16 units");
    }
    if wide.iter().any(|&unit| {
        unit < 0x20
            || matches!(
                unit,
                0x22 | 0x2A | 0x2F | 0x3A | 0x3C | 0x3E | 0x3F | 0x5C | 0x7C
            )
    }) {
        return Some("it holds a separator, stream marker or reserved character");
    }
    if matches!(wide.last(), Some(0x20 | 0x2E)) {
        return Some("Win32 drops a trailing dot or space");
    }
    if let Some(text) = text {
        let stem = text.split('.').next().unwrap_or(text);
        if RESERVED_DEVICE_NAMES
            .iter()
            .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        {
            return Some("it is a reserved device name");
        }
    }
    None
}

/// Whether `name` is one plain Windows child name: no separators, stream
/// markers or reserved characters, no trailing dot or space, and no
/// reserved device name, so the Win32 layer reads it the way the
/// handle-relative NT calls do. The same rule the Windows directory
/// authority applies, on every host.
#[must_use]
pub fn windows_file_name_is_valid(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().collect();
    name_refusal(&wide, Some(name)).is_none()
}
