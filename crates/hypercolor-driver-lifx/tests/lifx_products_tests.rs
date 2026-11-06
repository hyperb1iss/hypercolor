//! Product facts and capability resolution (Spec 88 §3.1 and §10).

use hypercolor_driver_lifx::products::{
    ExtendedMultizone, MatrixQuirk, ProductKind, lookup_product, products,
};
use hypercolor_driver_lifx::protocol::FirmwareVersion;

#[test]
fn table_is_sorted_and_unique_for_binary_search() {
    let table = products();
    assert!(table.windows(2).all(|pair| pair[0].pid < pair[1].pid));
    assert_eq!(table.len(), 173);
    for product in table {
        assert_eq!(lookup_product(product.pid), Some(product));
    }
    assert_eq!(lookup_product(4), None);
    assert_eq!(lookup_product(9_999), None);
}

#[test]
fn extended_multizone_follows_firmware_upgrades() {
    let z = lookup_product(32).expect("LIFX Z");
    assert_eq!(
        z.extended_multizone,
        ExtendedMultizone::SinceFirmware(2, 77)
    );
    assert!(!z.supports_extended_multizone(FirmwareVersion::new(2, 76)));
    assert!(z.supports_extended_multizone(FirmwareVersion::new(2, 77)));
    assert!(z.supports_extended_multizone(FirmwareVersion::new(3, 0)));

    let beam = lookup_product(38).expect("LIFX Beam");
    assert!(beam.supports_extended_multizone(FirmwareVersion::new(2, 80)));

    let first_gen = lookup_product(31).expect("first generation LIFX Z");
    assert!(!first_gen.supports_extended_multizone(FirmwareVersion::new(9, 0)));
    let outdoor = lookup_product(300).expect("Everyday Permanent Outdoor");
    assert!(!outdoor.supports_extended_multizone(FirmwareVersion::new(9, 0)));

    let neon = lookup_product(141).expect("LIFX Neon");
    assert!(neon.supports_extended_multizone(FirmwareVersion::new(0, 0)));

    let bulb = lookup_product(27).expect("LIFX A19");
    assert!(!bulb.supports_extended_multizone(FirmwareVersion::new(9, 0)));
}

#[test]
fn classes_match_the_registry_features() {
    assert_eq!(lookup_product(27).map(|p| p.kind), Some(ProductKind::Color));
    assert_eq!(lookup_product(10).map(|p| p.kind), Some(ProductKind::White));
    assert_eq!(
        lookup_product(70).map(|p| p.kind),
        Some(ProductKind::NonLight)
    );
    assert_eq!(
        lookup_product(226).map(|p| p.kind),
        Some(ProductKind::NonLight)
    );

    let tile = lookup_product(55).expect("LIFX Tile");
    assert_eq!(tile.kind, ProductKind::Matrix);
    assert!(tile.chain);
    assert_eq!(products().iter().filter(|product| product.chain).count(), 1);

    let candle = lookup_product(57).expect("LIFX Candle C");
    assert_eq!(candle.quirk, MatrixQuirk::CandleTopZone);

    assert_eq!(lookup_product(176).and_then(|p| p.uplight), Some((7, 7)));
    assert_eq!(lookup_product(201).and_then(|p| p.uplight), Some((15, 7)));
    assert_eq!(lookup_product(219).and_then(|p| p.uplight), None);
}

#[test]
fn white_products_are_not_color_capable() {
    assert!(!lookup_product(10).expect("White 800").color_capable());
    assert!(lookup_product(55).expect("Tile").color_capable());
    assert_eq!(
        lookup_product(51).and_then(|p| p.kelvin_range),
        Some((2_700, 2_700))
    );
}

#[test]
fn counts_match_the_registry_snapshot() {
    let count = |kind| products().iter().filter(|p| p.kind == kind).count();
    assert_eq!(count(ProductKind::Matrix), 28);
    assert_eq!(count(ProductKind::Multizone), 26);
    assert_eq!(count(ProductKind::NonLight), 7);
}
