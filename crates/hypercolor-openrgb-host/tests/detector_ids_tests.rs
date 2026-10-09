use std::collections::BTreeSet;

use hypercolor_openrgb_host::{
    DetectorUsbClaim, HostError, UsbDeviceId, detector_families, detector_usb_claim,
    detector_usb_ids, parse_detector_usb_ids,
};

/// Seed detectors that drive SMBus hardware: OpenRGB generates no udev rule
/// for them, so the id map cannot know them.
const SMBUS_SEEDS: [&str; 5] = [
    "ASUS Aura SMBus Motherboard",
    "Corsair Dominator Platinum",
    "Corsair Vengeance Pro",
    "Corsair Vengeance RGB DRAM",
    "ENE SMBus DRAM",
];

fn ids(list: &[(u16, u16)]) -> BTreeSet<UsbDeviceId> {
    list.iter()
        .map(|&(vendor, product)| UsbDeviceId::new(vendor, product))
        .collect()
}

#[test]
fn embedded_map_parses_and_covers_the_openrgb_usb_catalog() {
    assert!(
        detector_usb_ids().len() > 1000,
        "OpenRGB 1.0 prints over a thousand USB detectors, got {}",
        detector_usb_ids().len()
    );
}

#[test]
fn embedded_map_knows_the_issue_362_razer_devices() {
    let kraken = detector_usb_claim("Razer Kraken Ultimate").expect("Kraken Ultimate is mapped");
    assert_eq!(kraken.devices, ids(&[(0x1532, 0x0527)]));
    assert!(kraken.vendors.is_empty());

    let base_station =
        detector_usb_claim("Razer Base Station V2 Chroma").expect("Base Station V2 is mapped");
    assert_eq!(base_station.devices, ids(&[(0x1532, 0x0f20)]));
}

#[test]
fn every_usb_seed_detector_is_mapped_and_smbus_seeds_are_not() {
    for family in detector_families() {
        for name in &family.detectors {
            let mapped = detector_usb_claim(name).is_some();
            if SMBUS_SEEDS.contains(&name.as_str()) {
                assert!(
                    !mapped,
                    "{name} is an SMBus detector and must stay unmapped"
                );
            } else {
                assert!(mapped, "{name} is a USB seed the id map should know");
            }
        }
    }
}

#[test]
fn lookup_is_exact_so_unknown_spellings_stay_unmapped() {
    assert!(detector_usb_claim("razer kraken ultimate").is_none());
    assert!(detector_usb_claim("Razer Kraken Ultimate ").is_none());
    assert!(detector_usb_claim("Razer Future Gizmo").is_none());
}

#[test]
fn vendor_wildcards_claim_every_product_from_the_vendor() {
    let keychron = detector_usb_claim("Keychron RGB QMK/ZMK Keyboard")
        .expect("the vendor-wide Keychron detector is mapped");
    assert!(keychron.devices.is_empty());
    assert_eq!(keychron.vendors, BTreeSet::from([0x3434]));
    assert!(keychron.overlaps(&ids(&[(0x3434, 0x0123)])));
    assert!(!keychron.overlaps(&ids(&[(0x1532, 0x3434)])));
}

#[test]
fn overlap_needs_a_shared_device_or_vendor() {
    let claim = DetectorUsbClaim {
        devices: ids(&[(0x1532, 0x0527)]),
        vendors: BTreeSet::new(),
    };
    assert!(claim.overlaps(&ids(&[(0x1532, 0x0f20), (0x1532, 0x0527)])));
    assert!(!claim.overlaps(&ids(&[(0x1532, 0x0f20)])));
    assert!(!claim.overlaps(&BTreeSet::new()));
}

#[test]
fn parse_accepts_devices_and_wildcards() {
    let map = parse_detector_usb_ids(
        r#"
        [source]
        openrgb_version = "test"

        [detectors]
        "Razer Thing" = ["1532:0F20", "1532:0527"]
        "Logitech Everything" = ["046d:*"]
        "#,
    )
    .expect("valid map");
    assert_eq!(
        map["Razer Thing"].devices,
        ids(&[(0x1532, 0x0527), (0x1532, 0x0f20)])
    );
    assert_eq!(map["Logitech Everything"].vendors, BTreeSet::from([0x046d]));
}

#[test]
fn parse_rejects_malformed_maps() {
    for text in [
        "[detectors\n",
        "[detectors]\n\"X\" = []\n",
        "[detectors]\n\"X\" = [\"1532\"]\n",
        "[detectors]\n\"X\" = [\"1532:52\"]\n",
        "[detectors]\n\"X\" = [\"+532:0527\"]\n",
        "[detectors]\n\"X\" = [\"zzzz:0527\"]\n",
        "[detectors]\n\"X\" = [\"*:0527\"]\n",
    ] {
        let error = parse_detector_usb_ids(text).expect_err(text);
        assert!(
            matches!(error, HostError::DetectorUsbIds(_)),
            "{text}: {error}"
        );
    }
}

#[test]
fn usb_device_ids_round_trip_as_lowercase_hex() {
    let id = UsbDeviceId::new(0x1532, 0x0f20);
    assert_eq!(id.to_string(), "1532:0f20");
    assert_eq!("1532:0F20".parse::<UsbDeviceId>().expect("parse"), id);

    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, "\"1532:0f20\"");
    let back: UsbDeviceId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, id);
    assert!(serde_json::from_str::<UsbDeviceId>("\"nope\"").is_err());
}
