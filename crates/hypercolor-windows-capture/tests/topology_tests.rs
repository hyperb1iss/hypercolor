use hypercolor_windows_capture::{CaptureError, DisplayRotation, MonitorInfo, MonitorSelector};

fn monitor(index: usize, id: &str, origin_x: i32, primary: bool) -> MonitorInfo {
    MonitorInfo {
        index,
        id: id.to_owned(),
        name: format!(r"\\.\DISPLAY{}", index + 1),
        width: 2560,
        height: 1440,
        origin_x,
        origin_y: -120,
        primary,
        rotation: DisplayRotation::Clockwise90,
        topology_generation: 7,
    }
}

#[test]
fn auto_selects_the_primary_output_when_it_is_not_first() {
    let monitors = [
        monitor(0, "secondary", -2560, false),
        monitor(1, "primary", 0, true),
    ];

    let selected = MonitorSelector::Auto
        .resolve(&monitors)
        .expect("primary monitor resolves");

    assert_eq!(selected.id, "primary");
    assert_eq!(selected.origin_x, 0);
}

#[test]
fn stable_id_survives_enumeration_reorder() {
    let before = [
        monitor(0, "left", -2560, false),
        monitor(1, "main", 0, true),
    ];
    let after = [
        monitor(0, "main", 0, true),
        monitor(1, "left", -2560, false),
    ];
    let selector = MonitorSelector::StableId("left".to_owned());

    assert_eq!(selector.resolve(&before).expect("source exists").id, "left");
    assert_eq!(
        selector.resolve(&after).expect("source still exists").id,
        "left"
    );
}

#[test]
fn programmatic_index_selection_remains_available() {
    let monitors = [
        monitor(0, "left", -2560, false),
        monitor(1, "main", 0, true),
    ];

    let selected = MonitorSelector::Index(1)
        .resolve(&monitors)
        .expect("low-level index selector resolves");

    assert_eq!(selected.id, "main");
}

#[test]
fn disappeared_stable_source_is_not_rebound_to_an_index_neighbor() {
    let monitors = [monitor(0, "main", 0, true)];

    assert!(matches!(
        MonitorSelector::StableId("left".to_owned()).resolve(&monitors),
        Err(CaptureError::SourceNotFound { requested }) if requested == "left"
    ));
}

#[test]
fn parser_preserves_stable_ids_without_reinterpreting_numeric_strings() {
    assert_eq!(MonitorSelector::parse("auto"), MonitorSelector::Auto);
    assert_eq!(
        MonitorSelector::parse("monitor:dxgi:adapter:display"),
        MonitorSelector::StableId("dxgi:adapter:display".to_owned())
    );
    assert_eq!(
        MonitorSelector::parse("display:3"),
        MonitorSelector::StableId("display:3".to_owned())
    );
    assert_eq!(
        MonitorSelector::parse("monitor:1"),
        MonitorSelector::StableId("1".to_owned())
    );
    assert_eq!(
        MonitorSelector::parse("3"),
        MonitorSelector::StableId("3".to_owned())
    );
    assert_eq!(
        MonitorSelector::parse(r"display:\\?\display#del4098#instance"),
        MonitorSelector::StableId(r"display:\\?\display#del4098#instance".to_owned())
    );
}

#[test]
fn descriptors_keep_negative_origins_rotation_and_generation() {
    let descriptor = monitor(0, "left", -2560, false);

    assert_eq!(descriptor.origin_x, -2560);
    assert_eq!(descriptor.origin_y, -120);
    assert_eq!(descriptor.rotation, DisplayRotation::Clockwise90);
    assert_eq!(descriptor.topology_generation, 7);
}
