use std::collections::{HashMap, HashSet};

use hypercolor_daemon::layout_auto_exclusions::{
    LayoutAutoExclusionKey, load, reconcile_layout_device_exclusions,
};
use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, StripDirection,
};

fn make_zone(id: &str, device_id: &str) -> Output {
    Output {
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
fn load_restores_scoped_layout_auto_exclusion_fixture() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let path = temp_dir.path().join("layout-auto-exclusions.json");
    let scene_id = SceneId::new();
    let zone_id = ZoneId::new();
    let fixture = serde_json::json!([
        {
            "layout_id": "default",
            "excluded_device_ids": ["usb:defy", "wled:desk"]
        },
        {
            "scope": "zone",
            "scene_id": scene_id,
            "zone_id": zone_id,
            "excluded_device_ids": ["usb:keyboard"]
        },
        {
            "layout_id": "empty",
            "excluded_device_ids": []
        }
    ]);
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&fixture).expect("fixture should serialize"),
    )
    .expect("fixture should write");
    let loaded = load(&path).expect("load exclusions");

    let mut expected = HashMap::new();
    expected.insert(
        LayoutAutoExclusionKey::layout("default"),
        HashSet::from(["usb:defy".to_owned(), "wled:desk".to_owned()]),
    );
    expected.insert(
        LayoutAutoExclusionKey::zone(scene_id, zone_id),
        HashSet::from(["usb:keyboard".to_owned()]),
    );
    assert_eq!(loaded, expected);
}

#[test]
fn load_migrates_legacy_layout_auto_exclusions() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let path = temp_dir.path().join("layout-auto-exclusions.json");
    std::fs::write(
        &path,
        r#"[
          {
            "layout_id": "default",
            "excluded_device_ids": ["usb:defy"]
          }
        ]"#,
    )
    .expect("write legacy exclusions");

    let loaded = load(&path).expect("load exclusions");

    assert_eq!(
        loaded.get(&LayoutAutoExclusionKey::layout("default")),
        Some(&HashSet::from(["usb:defy".to_owned()]))
    );
}
