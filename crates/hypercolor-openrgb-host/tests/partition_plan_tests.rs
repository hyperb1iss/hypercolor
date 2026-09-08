use hypercolor_openrgb_host::{
    DetectorPartitionPlan, DeviceFacts, DriverFacts, bridge_enabled, detector_prefixes_for_drivers,
    known_detector_driver_ids, partition_driver_ids,
};
use hypercolor_types::device::DriverModuleKind;
const OPENRGB_BRIDGE_DRIVER_ID: &str = "openrgb";

fn facts(id: &str, module_kind: DriverModuleKind, enabled: bool) -> DriverFacts {
    DriverFacts {
        id: id.to_owned(),
        module_kind,
        enabled,
    }
}

fn typical_registry() -> Vec<DriverFacts> {
    vec![
        facts("razer", DriverModuleKind::Hal, true),
        facts("lianli", DriverModuleKind::Hal, false),
        facts("corsair", DriverModuleKind::Hal, true),
        facts("hue", DriverModuleKind::Network, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ]
}

fn device(driver_id: &str, disabled: bool) -> DeviceFacts {
    DeviceFacts {
        driver_id: driver_id.to_owned(),
        disabled,
    }
}

/// The live rig this was verified against: Corsair, Lian Li, and Razer own
/// enabled devices; Dygma's only device sits in state `known`; Nollie's only
/// device is user-disabled; ASUS is enabled with no device at all; QMK has
/// devices but no family in the detector table.
fn rig_drivers() -> Vec<DriverFacts> {
    vec![
        facts("corsair", DriverModuleKind::Hal, true),
        facts("lianli", DriverModuleKind::Hal, true),
        facts("razer", DriverModuleKind::Hal, true),
        facts("dygma", DriverModuleKind::Hal, true),
        facts("nollie", DriverModuleKind::Hal, true),
        facts("asus", DriverModuleKind::Hal, true),
        facts("qmk", DriverModuleKind::Hal, true),
        facts("hue", DriverModuleKind::Network, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ]
}

fn rig_devices() -> Vec<DeviceFacts> {
    vec![
        device("corsair", false),
        device("corsair", true),
        device("lianli", false),
        device("razer", false),
        device("dygma", false),
        device("nollie", true),
        device("qmk", false),
        device("hue", false),
        device(OPENRGB_BRIDGE_DRIVER_ID, false),
    ]
}

#[test]
fn partition_disables_only_enabled_drivers_that_own_an_enabled_device() {
    let plan = partition_driver_ids(&rig_drivers(), &rig_devices(), &known_detector_driver_ids());

    assert_eq!(
        plan.disabled_driver_ids,
        vec![
            "corsair".to_owned(),
            "dygma".to_owned(),
            "lianli".to_owned(),
            "razer".to_owned(),
        ]
    );
    // Only-disabled devices (nollie) and no devices (asus) are handed back
    // to OpenRGB. Drivers without a detector family (hue, qmk) never appear.
    assert_eq!(
        plan.re_enable_driver_ids,
        vec!["asus".to_owned(), "nollie".to_owned()]
    );
}

#[test]
fn partition_hands_back_a_family_whose_driver_is_disabled() {
    let mut drivers = rig_drivers();
    drivers
        .iter_mut()
        .filter(|driver| driver.id == "razer")
        .for_each(|driver| driver.enabled = false);

    let plan = partition_driver_ids(&drivers, &rig_devices(), &known_detector_driver_ids());

    assert!(!plan.disabled_driver_ids.iter().any(|id| id == "razer"));
    assert!(plan.re_enable_driver_ids.iter().any(|id| id == "razer"));
}

#[test]
fn partition_maps_to_the_detector_prefixes_those_families_own() {
    let plan = partition_driver_ids(&rig_drivers(), &rig_devices(), &known_detector_driver_ids());

    let disabled = detector_prefixes_for_drivers(&plan.disabled_driver_ids);
    assert_eq!(
        disabled,
        vec![
            "Corsair ".to_owned(),
            "Dygma ".to_owned(),
            "Lian Li ".to_owned(),
            "Razer ".to_owned(),
        ]
    );
    let re_enable = detector_prefixes_for_drivers(&plan.re_enable_driver_ids);
    assert!(re_enable.iter().any(|prefix| prefix == "Nollie "));
    assert!(re_enable.iter().any(|prefix| prefix == "ASUS Aura"));
    assert!(!re_enable.iter().any(|prefix| prefix == "Razer "));
}

#[test]
fn partition_with_no_daemon_devices_re_enables_every_known_family() {
    let plan = partition_driver_ids(&rig_drivers(), &[], &known_detector_driver_ids());

    assert!(plan.disabled_driver_ids.is_empty());
    let mut known = known_detector_driver_ids();
    known.sort();
    assert_eq!(plan.re_enable_driver_ids, known);
}

#[test]
fn partition_ignores_case_and_bridge_devices() {
    let drivers = vec![
        facts("Razer", DriverModuleKind::Hal, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ];
    let devices = vec![
        device("razer", false),
        device(OPENRGB_BRIDGE_DRIVER_ID, false),
    ];

    let plan = partition_driver_ids(&drivers, &devices, &["razer", "openrgb"]);

    assert_eq!(
        plan,
        DetectorPartitionPlan {
            disabled_driver_ids: vec!["Razer".to_owned()],
            re_enable_driver_ids: vec!["openrgb".to_owned()],
        }
    );
}

#[test]
fn device_facts_treat_only_the_disabled_status_as_disabled() {
    for (status, expected) in [
        ("disabled", true),
        ("Disabled", true),
        ("known", false),
        ("connected", false),
        ("active", false),
        ("reconnecting", false),
    ] {
        let facts = DeviceFacts {
            driver_id: "razer".to_owned(),
            disabled: status.eq_ignore_ascii_case("disabled"),
        };
        assert_eq!(facts.disabled, expected, "status {status}");
    }
}

#[test]
fn bridge_enabled_requires_the_openrgb_driver_to_be_on() {
    assert!(bridge_enabled(&typical_registry()));

    let mut disabled = typical_registry();
    disabled
        .iter_mut()
        .filter(|driver| driver.id == OPENRGB_BRIDGE_DRIVER_ID)
        .for_each(|driver| driver.enabled = false);
    assert!(!bridge_enabled(&disabled));

    let absent: Vec<DriverFacts> = typical_registry()
        .into_iter()
        .filter(|driver| driver.id != OPENRGB_BRIDGE_DRIVER_ID)
        .collect();
    assert!(!bridge_enabled(&absent));
}
