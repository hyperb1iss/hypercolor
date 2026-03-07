use std::collections::HashMap;

use hypercolor_daemon::layout_store;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection, ZoneGroup,
};

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "layout_saved".into(),
        name: "Saved Layout".into(),
        description: Some("Persist me".into()),
        canvas_width: 640,
        canvas_height: 360,
        zones: vec![DeviceZone {
            id: "zone-1".into(),
            name: "Desk Strip".into(),
            device_id: "wled:desk".into(),
            zone_name: None,
            group_id: Some("group-1".into()),
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.4, 0.1),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 30,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
        }],
        groups: vec![ZoneGroup {
            id: "group-1".into(),
            name: "Desk".into(),
            color: Some("#80ffea".into()),
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[test]
fn load_returns_empty_map_when_file_is_missing() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let path = tempdir.path().join("layouts.json");

    let loaded = layout_store::load(&path).expect("missing layout file should not fail");

    assert!(loaded.is_empty());
}

#[test]
fn save_and_load_roundtrip_preserves_layouts() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let path = tempdir.path().join("layouts.json");
    let layout = sample_layout();
    let mut store = HashMap::new();
    store.insert(layout.id.clone(), layout.clone());

    layout_store::save(&path, &store).expect("save should succeed");
    let loaded = layout_store::load(&path).expect("load should succeed");
    let restored = loaded
        .get(&layout.id)
        .expect("saved layout should be present after load");

    assert_eq!(restored.name, layout.name);
    assert_eq!(restored.groups, layout.groups);
    assert_eq!(restored.zones[0].group_id, layout.zones[0].group_id);
    assert_eq!(restored.canvas_width, layout.canvas_width);
    assert_eq!(restored.canvas_height, layout.canvas_height);
}
