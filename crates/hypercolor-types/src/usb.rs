//! USB vendor identity resolved from the vendor ID.
//!
//! A device's manufacturer string is not always available: nusb's Windows
//! backend never reads it, and plenty of devices ship without one. The
//! curated table here names the company behind a vendor ID so the daemon's
//! hardware inventory and every client label the same device the same way
//! on every OS.

use std::borrow::Cow;

/// USB vendor IDs that one company registered and ships under its own
/// brand, sorted by VID.
///
/// Membership is deliberately narrow. A VID belongs here only when its
/// USB-IF registration and the products seen on it agree on one vendor.
/// Silicon vendors whose VID rides on other brands' products stay out
/// (ENE `0x0CF2`, ITE `0x048D`, Winbond `0x0416`, Luminary Micro `0x1CBE`),
/// and so do VIDs a brand ships on without owning them: TURZX screens and
/// Lian Li wireless LCDs both enumerate on `0x1CBE`, and several RGB
/// controller makers sit on VIDs registered to unrelated companies. Naming
/// a vendor from such a VID would put the wrong company on a support
/// request, which is worse than showing the raw VID.
///
/// The vendor TOMLs under `data/drivers/vendors/` list the VIDs each
/// brand's devices use, shared ones included, so they are a source for
/// candidates here but not a substitute for this table.
const OWNED_VENDOR_IDS: &[(u16, &str)] = &[
    (0x03F0, "HP"),
    (0x0414, "Gigabyte"),
    (0x041E, "Creative"),
    (0x045E, "Microsoft"),
    (0x046D, "Logitech"),
    (0x054C, "Sony"),
    (0x05AC, "Apple"),
    (0x0951, "Kingston"),
    (0x0B05, "ASUS"),
    (0x0DB0, "MSI"),
    (0x0FD9, "Elgato"),
    (0x1038, "SteelSeries"),
    (0x1462, "MSI"),
    (0x1532, "Razer"),
    (0x1770, "MSI"),
    (0x187C, "Alienware"),
    (0x1B1C, "Corsair"),
    (0x1E71, "NZXT"),
    (0x1E7D, "Roccat"),
    (0x2516, "Cooler Master"),
    (0x264A, "Thermaltake"),
    (0x26CE, "ASRock"),
    (0x2982, "Ableton"),
    (0x2F0E, "Fnatic"),
    (0x31E3, "Wooting"),
    (0x3297, "ZSA"),
    (0x3434, "Keychron"),
    (0x35EF, "Dygma"),
    (0x8086, "Intel"),
    (0x8087, "Intel"),
];

/// Every `(vendor ID, vendor name)` pair the curated table knows, sorted
/// by vendor ID.
#[must_use]
pub fn owned_usb_vendor_ids() -> &'static [(u16, &'static str)] {
    OWNED_VENDOR_IDS
}

/// The company that owns `vendor_id`, when the curated table knows it.
#[must_use]
pub fn usb_vendor_name(vendor_id: u16) -> Option<&'static str> {
    OWNED_VENDOR_IDS
        .binary_search_by_key(&vendor_id, |(vid, _)| *vid)
        .ok()
        .map(|index| OWNED_VENDOR_IDS[index].1)
}

/// A manufacturer string worth showing: trimmed, and `None` when blank.
#[must_use]
pub fn reported_manufacturer(manufacturer: Option<&str>) -> Option<&str> {
    manufacturer.map(str::trim).filter(|name| !name.is_empty())
}

/// Who made a USB device: the manufacturer string it reports when that
/// says anything, otherwise the owner of its vendor ID.
#[must_use]
pub fn usb_vendor(manufacturer: Option<&str>, vendor_id: u16) -> Option<&str> {
    reported_manufacturer(manufacturer).or_else(|| usb_vendor_name(vendor_id))
}

/// Display text for a USB device's vendor, never empty.
///
/// Falls back to `VID 1532` style text when neither the device nor the
/// curated table names a vendor, so a label always carries something a
/// reader can look up.
#[must_use]
pub fn usb_vendor_label(manufacturer: Option<&str>, vendor_id: u16) -> Cow<'_, str> {
    usb_vendor(manufacturer, vendor_id)
        .map_or_else(|| Cow::Owned(format!("VID {vendor_id:04X}")), Cow::Borrowed)
}
