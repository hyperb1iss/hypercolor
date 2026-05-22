//! Motherboard identity shared across daemon, app shell, and UI.

use serde::{Deserialize, Serialize};

/// Motherboard identification surfaced from the host OS (Windows `Win32_BaseBoard`,
/// Linux DMI sysfs, macOS IOPlatformExpertDevice in future). Populated on a
/// best-effort basis; absent on platforms that don't expose vendor identity or
/// when the underlying query fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotherboardInfo {
    /// Vendor / OEM (e.g. "ASUSTeK COMPUTER INC.", "Micro-Star International Co., Ltd").
    pub manufacturer: String,
    /// Product / model (e.g. "ROG STRIX X670E-E GAMING WIFI").
    pub product: String,
    /// Board revision string, if reported.
    pub version: Option<String>,
}

impl MotherboardInfo {
    /// Heuristic: does this motherboard belong to a vendor that ships RGB
    /// controllers Hypercolor knows how to address via SMBus / PawnIO?
    ///
    /// The check is intentionally permissive on substring matches (vendor names
    /// vary in capitalization and entity suffixes across BIOS revisions).
    /// False positives (showing the hardware-support offer when no RGB is
    /// present) are recoverable — the user dismisses it. False negatives
    /// (hiding the offer from a real RGB user) are not — they wouldn't know
    /// to look.
    #[must_use]
    pub fn is_likely_rgb_capable(&self) -> bool {
        let vendor = self.manufacturer.to_ascii_lowercase();
        ["asus", "asustek", "msi", "micro-star", "gigabyte", "asrock"]
            .iter()
            .any(|needle| vendor.contains(needle))
    }
}
