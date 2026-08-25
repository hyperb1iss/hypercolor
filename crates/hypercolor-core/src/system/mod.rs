//! Host hardware identification used by first-run flows and UI gating.

pub use hypercolor_types::motherboard::MotherboardInfo;

/// Best-effort motherboard identification.
///
/// Returns `None` on platforms that don't expose vendor identity or when the
/// underlying query fails. Callers should treat `None` as "unknown" — never
/// gate user-visible behavior on it without an explicit fallback.
#[must_use]
pub fn motherboard_info() -> Option<MotherboardInfo> {
    hypercolor_windows_telemetry::motherboard_info()
}
