//! Host hardware identification used by first-run flows and UI gating.

pub use hypercolor_types::motherboard::MotherboardInfo;

#[cfg(target_os = "windows")]
mod windows;

/// Best-effort motherboard identification.
///
/// Returns `None` on platforms that don't expose vendor identity or when the
/// underlying query fails. Callers should treat `None` as "unknown" — never
/// gate user-visible behavior on it without an explicit fallback.
#[must_use]
pub fn motherboard_info() -> Option<MotherboardInfo> {
    #[cfg(target_os = "windows")]
    {
        windows::motherboard_info()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}
