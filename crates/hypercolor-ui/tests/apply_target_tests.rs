use hypercolor_ui::apply_target::{ALL_ZONES_VALUE, ApplyTarget};

#[test]
fn select_values_round_trip_through_apply_targets() {
    assert_eq!(
        ApplyTarget::from_select_value(String::new()),
        ApplyTarget::Primary
    );
    assert_eq!(
        ApplyTarget::from_select_value("default".to_owned()),
        ApplyTarget::Primary,
    );
    assert_eq!(
        ApplyTarget::from_select_value(ALL_ZONES_VALUE.to_owned()),
        ApplyTarget::AllZones,
    );
    assert_eq!(
        ApplyTarget::from_select_value("zone-123".to_owned()),
        ApplyTarget::Zone("zone-123".to_owned()),
    );

    assert_eq!(ApplyTarget::Primary.select_value(), "");
    assert_eq!(
        ApplyTarget::AllZones.select_value(),
        ALL_ZONES_VALUE,
    );
    assert_eq!(
        ApplyTarget::Zone("zone-123".to_owned()).select_value(),
        "zone-123",
    );
}

#[test]
fn only_zone_targets_expose_zone_ids() {
    assert_eq!(ApplyTarget::Primary.zone_id(), None);
    assert_eq!(ApplyTarget::AllZones.zone_id(), None);
    assert_eq!(
        ApplyTarget::Zone("zone-123".to_owned()).zone_id(),
        Some("zone-123"),
    );
}
