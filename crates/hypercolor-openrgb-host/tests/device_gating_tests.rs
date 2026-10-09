//! Per-device detector gating: a native driver withholds only the OpenRGB
//! detectors whose USB devices its catalog can claim.
//!
//! The catalogs below are pinned fixtures, not the live HAL database, so
//! these tests keep meaning the same thing as native support grows. The
//! Kraken Ultimate (1532:0527) stands in for "a Razer device the native
//! driver cannot drive", which is true until native support for it lands.

use std::collections::BTreeSet;

use hypercolor_openrgb_host::{
    DETECTORS_MAP, DETECTORS_SECTION, DetectorPartitionPlan, DeviceFacts, DriverFacts, UsbClaim,
    UsbDeviceId, detector_usb_claim, known_detector_driver_ids, managed_config_dir,
    partition_detectors, partition_driver_ids, write_detector_partition,
};
use hypercolor_types::api::drivers::DriverSummary;
use hypercolor_types::device::{
    DRIVER_MODULE_API_SCHEMA_VERSION, DriverCapabilitySet, DriverModuleDescriptor,
    DriverModuleKind, DriverPresentation, DriverProtocolDescriptor, DriverTransportKind,
};
use serde_json::{Map, Value, json};

const BASE_STATION_V2: UsbDeviceId = UsbDeviceId::new(0x1532, 0x0f20);
const KRAKEN_ULTIMATE: UsbDeviceId = UsbDeviceId::new(0x1532, 0x0527);
const O11_RAZER_EDITION: UsbDeviceId = UsbDeviceId::new(0x1532, 0x0f13);
const ICUE_LINK_HUB: UsbDeviceId = UsbDeviceId::new(0x1b1c, 0x0c3f);
const KEYCHRON_QMK: UsbDeviceId = UsbDeviceId::new(0x3434, 0x0321);

fn driver(id: &str, enabled: bool, usb_ids: &[UsbDeviceId]) -> DriverFacts {
    DriverFacts {
        id: id.to_owned(),
        module_kind: if id == "openrgb" {
            DriverModuleKind::Bridge
        } else {
            DriverModuleKind::Hal
        },
        enabled,
        usb: UsbClaim {
            devices: usb_ids.iter().copied().collect(),
            vendors: BTreeSet::new(),
        },
    }
}

fn device(driver_id: &str) -> DeviceFacts {
    DeviceFacts {
        driver_id: driver_id.to_owned(),
        disabled: false,
    }
}

/// Issue #362's rig: a natively driven Base Station V2 and a Kraken
/// Ultimate the native Razer driver has no protocol for.
fn issue_362_plan(razer_ids: &[UsbDeviceId]) -> DetectorPartitionPlan {
    partition_driver_ids(
        &[
            driver("razer", true, razer_ids),
            driver("corsair", true, &[ICUE_LINK_HUB]),
            driver("lianli", true, &[]),
            driver("openrgb", true, &[]),
        ],
        &[device("razer"), device("corsair"), device("openrgb")],
        &known_detector_driver_ids(),
    )
}

fn read_detectors(path: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(path).expect("config written");
    let value: Value = serde_json::from_str(&text).expect("valid JSON");
    value[DETECTORS_SECTION][DETECTORS_MAP].clone()
}

#[test]
fn native_razer_withholds_supported_devices_and_releases_the_kraken_ultimate() {
    assert!(
        detector_usb_claim("Razer Kraken Ultimate").is_some(),
        "the id map must know the Kraken Ultimate for this test to mean anything"
    );
    let rules = issue_362_plan(&[BASE_STATION_V2, O11_RAZER_EDITION]).detector_rules();

    assert!(!rules.detector_enabled("Razer Base Station V2 Chroma", None));
    assert!(!rules.detector_enabled("Razer Base Station V2 Chroma", Some(true)));
    assert!(rules.detector_enabled("Razer Kraken Ultimate", None));
    assert!(
        rules.detector_enabled("Razer Kraken Ultimate", Some(false)),
        "a false left behind by the old prefix rule is handed back"
    );
}

#[test]
fn existing_prefix_partitions_are_rewritten_per_device() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    std::fs::create_dir_all(&dir.root).expect("mkdir");
    // What the prefix-only partition wrote for the issue #362 rig once
    // OpenRGB had recorded its detector names.
    let fixture = json!({
        "Detectors": {
            "detectors": {
                "Razer Base Station V2 Chroma": false,
                "Razer Kraken Ultimate": false,
                "Razer Huntsman": false,
                "AMD Wraith Prism": false
            }
        }
    });
    std::fs::write(dir.config_path(), fixture.to_string()).expect("write fixture");

    let rules = issue_362_plan(&[BASE_STATION_V2]).detector_rules();
    let report = write_detector_partition(&dir, &rules, None).expect("partition");
    let detectors = read_detectors(&dir.config_path());

    assert_eq!(detectors["Razer Base Station V2 Chroma"], false);
    assert_eq!(detectors["Razer Kraken Ultimate"], true);
    assert_eq!(
        detectors["Razer Huntsman"], true,
        "a mapped Razer detector the catalog cannot claim goes back to OpenRGB"
    );
    assert_eq!(
        detectors["AMD Wraith Prism"], false,
        "a user toggle outside every native family survives"
    );
    assert!(report.enabled.contains(&"Razer Kraken Ultimate".to_owned()));
    assert!(
        report
            .disabled
            .contains(&"Razer Base Station V2 Chroma".to_owned())
    );
}

#[test]
fn a_fresh_config_disables_claimed_detectors_before_openrgb_writes_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());

    let rules = issue_362_plan(&[BASE_STATION_V2]).detector_rules();
    write_detector_partition(&dir, &rules, None).expect("partition");
    let detectors = read_detectors(&dir.config_path());

    assert_eq!(
        detectors["Razer Base Station V2 Chroma"], false,
        "claimed detectors are seeded even though no seed list names them"
    );
    assert_ne!(detectors["Razer Kraken Ultimate"], false);
    assert_eq!(detectors["Corsair iCUE Link System Hub"], false);
    assert_eq!(detectors["Corsair Commander Pro"], true);
}

#[test]
fn pid_less_detectors_keep_the_prefix_rule() {
    let plan = partition_driver_ids(
        &[
            driver("corsair", true, &[ICUE_LINK_HUB]),
            driver("asus", true, &[UsbDeviceId::new(0x0b05, 0x19af)]),
        ],
        &[device("corsair"), device("asus")],
        &known_detector_driver_ids(),
    );
    let rules = plan.detector_rules();
    assert_eq!(rules.id_gated_prefixes.len(), 3, "{rules:?}");

    for smbus in [
        "Corsair Vengeance Pro",
        "Corsair Dominator Platinum",
        "Corsair Vengeance RGB DRAM",
        "ENE SMBus DRAM",
        "ASUS Aura SMBus Motherboard",
    ] {
        assert!(detector_usb_claim(smbus).is_none(), "{smbus} has no USB id");
        assert!(
            !rules.detector_enabled(smbus, Some(true)),
            "{smbus} stays withheld by its family prefix"
        );
    }
    assert!(!rules.detector_enabled("ASUS Aura Motherboard", None));
    assert!(rules.detector_enabled("ASUS Aura Addressable", Some(false)));
    assert!(rules.detector_enabled("Corsair K70 RGB MK.2", Some(false)));
}

#[test]
fn unknown_names_in_an_existing_config_stay_conservative() {
    let mut existing = Map::new();
    existing.insert("Razer Future Gizmo".to_owned(), Value::Bool(true));
    existing.insert("RAZER kraken ultimate".to_owned(), Value::Bool(true));
    existing.insert("Razer Kraken Ultimate".to_owned(), Value::Bool(false));

    let rules = issue_362_plan(&[BASE_STATION_V2]).detector_rules();
    let map = partition_detectors(&existing, &rules, None);

    assert_eq!(
        map["Razer Future Gizmo"], false,
        "a name the id map does not know is withheld by prefix"
    );
    assert_eq!(
        map["RAZER kraken ultimate"], false,
        "the id map matches exact names only"
    );
    assert_eq!(map["Razer Kraken Ultimate"], true);
}

#[test]
fn facts_without_usb_catalogs_fall_back_to_the_prefix_rule() {
    let plan = issue_362_plan(&[]);
    assert!(plan.id_gated_driver_ids.iter().all(|id| id != "razer"));

    let rules = plan.detector_rules();
    assert!(
        rules
            .disabled_prefixes
            .iter()
            .any(|prefix| prefix == "Razer ")
    );
    assert!(!rules.detector_enabled("Razer Kraken Ultimate", Some(true)));
    assert!(!rules.detector_enabled("Razer Base Station V2 Chroma", None));
}

#[test]
fn claimed_ids_withhold_detectors_outside_every_family_prefix() {
    let plan = partition_driver_ids(
        &[
            driver("razer", true, &[O11_RAZER_EDITION]),
            driver("qmk", true, &[KEYCHRON_QMK]),
        ],
        &[device("razer"), device("qmk")],
        &known_detector_driver_ids(),
    );
    assert!(plan.re_enable_driver_ids.iter().any(|id| id == "lianli"));
    let rules = plan.detector_rules();

    assert!(
        !rules.detector_enabled("Lian Li O11 Dynamic - Razer Edition", Some(true)),
        "Razer silicon under a Lian Li name is still natively claimed"
    );
    assert!(
        !rules.detector_enabled("Keychron RGB QMK/ZMK Keyboard", Some(true)),
        "a vendor-wide detector is withheld when any of its vendor's devices is"
    );
    assert!(rules.detector_enabled("Lian Li Uni Hub", Some(false)));
}

#[test]
fn released_native_devices_are_handed_back() {
    let mut existing = Map::new();
    existing.insert(
        "Keychron RGB QMK/ZMK Keyboard".to_owned(),
        Value::Bool(false),
    );
    existing.insert("Gigabyte RGB Fusion 2 SMBus".to_owned(), Value::Bool(false));

    // QMK is enabled but owns no device now, so nothing is withheld; its
    // catalog still marks the Keychron detector as one Hypercolor manages.
    let plan = partition_driver_ids(
        &[driver("qmk", true, &[KEYCHRON_QMK])],
        &[],
        &known_detector_driver_ids(),
    );
    assert!(plan.claimed_usb.is_empty());
    let map = partition_detectors(&existing, &plan.detector_rules(), None);

    assert_eq!(map["Keychron RGB QMK/ZMK Keyboard"], true);
    assert_eq!(map["Gigabyte RGB Fusion 2 SMBus"], false);
}

#[test]
fn plans_collect_catalogs_from_withheld_and_registered_drivers() {
    let plan = partition_driver_ids(
        &[
            driver("razer", true, &[BASE_STATION_V2]),
            driver("corsair", false, &[ICUE_LINK_HUB]),
            driver("lianli", true, &[UsbDeviceId::new(0x0cf2, 0x7750)]),
            driver("openrgb", true, &[KRAKEN_ULTIMATE]),
        ],
        &[device("razer"), device("corsair"), device("openrgb")],
        &known_detector_driver_ids(),
    );

    assert_eq!(plan.claimed_usb.devices, BTreeSet::from([BASE_STATION_V2]));
    assert_eq!(
        plan.native_usb.devices,
        BTreeSet::from([
            BASE_STATION_V2,
            ICUE_LINK_HUB,
            UsbDeviceId::new(0x0cf2, 0x7750)
        ]),
        "bridge catalogs never count as native"
    );
    assert_eq!(plan.id_gated_driver_ids, vec!["razer".to_owned()]);
    assert_eq!(plan.disabled_driver_ids, vec!["razer".to_owned()]);
}

#[test]
fn driver_facts_take_usb_claims_from_the_published_protocol_catalog() {
    let protocol = |vendor_id: Option<u16>, product_id: Option<u16>| DriverProtocolDescriptor {
        driver_id: "razer".to_owned(),
        protocol_id: "razer/test".to_owned(),
        display_name: "Test".to_owned(),
        vendor_id,
        product_id,
        family_id: "razer".to_owned(),
        model_id: None,
        transport: DriverTransportKind::Usb,
        route_backend_id: "usb".to_owned(),
        presentation: None,
    };
    let summary = DriverSummary {
        descriptor: DriverModuleDescriptor {
            id: "razer".to_owned(),
            display_name: "Razer".to_owned(),
            vendor_name: None,
            module_kind: DriverModuleKind::Hal,
            transports: Vec::new(),
            capabilities: DriverCapabilitySet::empty(),
            api_schema_version: DRIVER_MODULE_API_SCHEMA_VERSION,
            config_version: 1,
            default_enabled: true,
        },
        presentation: DriverPresentation {
            label: "Razer".to_owned(),
            short_label: None,
            accent_rgb: None,
            secondary_rgb: None,
            icon: None,
            default_device_class: None,
        },
        enabled: true,
        config_key: "drivers.razer".to_owned(),
        protocols: vec![
            protocol(Some(0x1532), Some(0x0f20)),
            protocol(Some(0x1532), Some(0x0f20)),
            protocol(Some(0x1532), None),
            protocol(None, None),
        ],
        control_surface_id: None,
        control_surface_path: None,
    };

    let facts = DriverFacts::from(&summary);
    assert_eq!(facts.usb.devices, BTreeSet::from([BASE_STATION_V2]));
    assert_eq!(
        facts.usb.vendors,
        BTreeSet::from([0x1532]),
        "a vendor id without a product id claims the whole vendor"
    );
}

#[test]
fn driver_facts_from_older_peers_deserialize_without_usb_claims() {
    let facts: DriverFacts =
        serde_json::from_value(json!({"id": "razer", "module_kind": "hal", "enabled": true}))
            .expect("facts without usb");
    assert!(facts.usb.is_empty());

    let wire = serde_json::to_value(driver("razer", true, &[BASE_STATION_V2])).expect("serialize");
    assert_eq!(
        wire["usb"],
        json!({"devices": ["1532:0f20"], "vendors": []})
    );
}

#[test]
fn vendor_wide_native_protocols_withhold_every_device_of_that_vendor() {
    let plan = partition_driver_ids(
        &[DriverFacts {
            id: "razer".to_owned(),
            module_kind: DriverModuleKind::Hal,
            enabled: true,
            usb: UsbClaim {
                devices: BTreeSet::new(),
                vendors: BTreeSet::from([0x1532]),
            },
        }],
        &[device("razer")],
        &known_detector_driver_ids(),
    );
    assert_eq!(plan.id_gated_driver_ids, vec!["razer".to_owned()]);
    let rules = plan.detector_rules();

    assert!(!rules.detector_enabled("Razer Kraken Ultimate", Some(true)));
    assert!(!rules.detector_enabled("Razer Base Station V2 Chroma", None));
    assert!(
        !rules.detector_enabled("Lian Li O11 Dynamic - Razer Edition", Some(true)),
        "the vendor claim reaches Razer silicon under another brand's name"
    );
    assert!(rules.detector_enabled("Lian Li Uni Hub", Some(false)));
}

#[test]
fn detector_rules_match_driver_ids_without_regard_to_case() {
    let plan = partition_driver_ids(
        &[
            driver("Razer", true, &[BASE_STATION_V2]),
            driver("CORSAIR", true, &[]),
        ],
        &[device("razer"), device("corsair")],
        &known_detector_driver_ids(),
    );
    let rules = plan.detector_rules();

    assert_eq!(rules.id_gated_prefixes, vec!["Razer ".to_owned()]);
    assert_eq!(rules.disabled_prefixes, vec!["Corsair ".to_owned()]);
    assert!(rules.detector_enabled("Razer Kraken Ultimate", Some(false)));
    assert!(!rules.detector_enabled("Corsair K70 RGB MK.2", Some(true)));
}
