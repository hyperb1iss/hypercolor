//! Contract tests for the curated USB vendor table and the labels built on it.

use hypercolor_types::api::devices::UnclaimedDevice;
use hypercolor_types::usb::{
    owned_usb_vendor_ids, reported_manufacturer, usb_vendor, usb_vendor_label, usb_vendor_name,
};

#[test]
fn the_vendor_table_is_strictly_sorted_so_lookups_can_binary_search() {
    let table = owned_usb_vendor_ids();
    assert!(!table.is_empty());
    for pair in table.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "{:04X} must sort before {:04X} with no duplicates",
            pair[0].0,
            pair[1].0
        );
    }
    for (vendor_id, name) in table {
        assert_eq!(usb_vendor_name(*vendor_id), Some(*name));
        assert!(!name.trim().is_empty());
    }
}

#[test]
fn vendor_owned_ids_resolve_to_their_owner() {
    assert_eq!(usb_vendor_name(0x1532), Some("Razer"));
    assert_eq!(usb_vendor_name(0x1B1C), Some("Corsair"));
    assert_eq!(usb_vendor_name(0x046D), Some("Logitech"));
}

/// A shared or borrowed VID names no vendor, because whichever brand the
/// table picked would be wrong for the others shipping on it.
#[test]
fn shared_vendor_ids_name_no_vendor() {
    for shared in [
        0x1CBE, // Luminary Micro: TURZX screens and Lian Li wireless LCDs
        0x0CF2, // ENE: Lian Li hubs and many motherboard controllers
        0x0416, // Winbond: Lian Li TL controllers
        0x048D, // ITE: Gigabyte and laptop keyboard controllers
        0x16D5, // used by both Nollie and PrismRGB
        0x320F, // Evision: Glorious and QMK keyboards
    ] {
        assert_eq!(usb_vendor_name(shared), None, "{shared:04X}");
    }
}

#[test]
fn a_reported_manufacturer_is_trimmed_and_blank_means_absent() {
    assert_eq!(
        reported_manufacturer(Some("  Razer Inc. ")),
        Some("Razer Inc.")
    );
    assert_eq!(reported_manufacturer(Some(" \t")), None);
    assert_eq!(reported_manufacturer(Some("")), None);
    assert_eq!(reported_manufacturer(None), None);
}

#[test]
fn the_device_string_wins_over_the_table() {
    assert_eq!(usb_vendor(Some("Razer Inc."), 0x1532), Some("Razer Inc."));
    assert_eq!(usb_vendor(Some("Acme"), 0x1532), Some("Acme"));
}

#[test]
fn a_missing_or_blank_string_falls_back_to_the_vid_owner() {
    assert_eq!(usb_vendor(None, 0x1532), Some("Razer"));
    assert_eq!(usb_vendor(Some("   "), 0x1532), Some("Razer"));
    assert_eq!(usb_vendor(None, 0x1CBE), None);
}

#[test]
fn the_label_is_never_empty_and_ends_at_the_raw_vid() {
    assert_eq!(usb_vendor_label(Some("Acme"), 0x1CBE), "Acme");
    assert_eq!(usb_vendor_label(None, 0x1532), "Razer");
    assert_eq!(usb_vendor_label(None, 0x1CBE), "VID 1CBE");
    assert_eq!(usb_vendor_label(Some(" "), 0x00AB), "VID 00AB");
}

#[test]
fn an_unclaimed_device_labels_its_vendor_through_the_same_table() {
    let mut device = UnclaimedDevice {
        vendor_id: 0x1532,
        product_id: 0x0527,
        product: Some("Razer Kraken Ultimate".to_owned()),
        ..Default::default()
    };
    assert_eq!(device.vendor_label(), "Razer");

    device.manufacturer = Some("Razer Inc.".to_owned());
    assert_eq!(device.vendor_label(), "Razer Inc.");

    device.vendor_id = 0x1CBE;
    device.manufacturer = None;
    assert_eq!(device.vendor_label(), "VID 1CBE");
}
