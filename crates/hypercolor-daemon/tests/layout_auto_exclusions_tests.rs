use std::collections::{HashMap, HashSet};

use hypercolor_daemon::layout_auto_exclusions::{
    LayoutAutoExclusionStore, load, reconcile_layout_device_exclusions, save,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, StripDirection,
};

fn make_zone(id: &str, device_id: &str) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,

        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.25, 0.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 16,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

#[test]
fn reconcile_layout_device_exclusions_marks_removed_devices_and_clears_readded_devices() {
    let previous_zones = vec![
        make_zone("zone-a", "usb:defy"),
        make_zone("zone-b", "wled:desk"),
    ];
    let updated_zones = vec![
        make_zone("zone-b", "wled:desk"),
        make_zone("zone-c", "usb:mouse"),
    ];
    let existing_exclusions = HashSet::from(["usb:mouse".to_owned()]);

    let next =
        reconcile_layout_device_exclusions(&previous_zones, &updated_zones, &existing_exclusions);

    assert_eq!(next, HashSet::from(["usb:defy".to_owned()]));
}

#[test]
fn save_and_load_round_trip_layout_auto_exclusions() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let path = temp_dir.path().join("layout-auto-exclusions.json");
    let mut store = LayoutAutoExclusionStore::new();
    store.insert(
        "default".to_owned(),
        HashSet::from(["usb:defy".to_owned(), "wled:desk".to_owned()]),
    );
    store.insert("empty".to_owned(), HashSet::new());

    save(&path, &store).expect("save exclusions");
    let loaded = load(&path).expect("load exclusions");

    let mut expected = HashMap::new();
    expected.insert(
        "default".to_owned(),
        HashSet::from(["usb:defy".to_owned(), "wled:desk".to_owned()]),
    );
    assert_eq!(loaded, expected);
}
