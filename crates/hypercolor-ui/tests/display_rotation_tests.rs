use hypercolor_types::scene::DisplayRotation;
use hypercolor_ui::display_rotation::{
    DISPLAY_ROTATIONS, display_rotation_label, display_rotation_select_options,
    display_rotation_value, parse_display_rotation,
};

#[test]
fn every_rotation_round_trips_through_its_wire_token() {
    for (rotation, value, _) in DISPLAY_ROTATIONS {
        assert_eq!(display_rotation_value(rotation), value);
        assert_eq!(parse_display_rotation(value), rotation);
        // The token must match the daemon's serde spelling exactly.
        let wire = serde_json::to_value(rotation).expect("rotation serializes");
        assert_eq!(wire, serde_json::Value::String(value.to_owned()));
    }
}

#[test]
fn unknown_tokens_read_upright() {
    assert_eq!(parse_display_rotation("sideways"), DisplayRotation::Deg0);
    assert_eq!(parse_display_rotation(""), DisplayRotation::Deg0);
    assert_eq!(parse_display_rotation(" deg180 "), DisplayRotation::Deg180);
}

#[test]
fn select_options_follow_the_table_in_order() {
    let options = display_rotation_select_options();
    assert_eq!(options.len(), DISPLAY_ROTATIONS.len());
    for ((_, value, label), (option_value, option_label)) in DISPLAY_ROTATIONS.iter().zip(&options)
    {
        assert_eq!(option_value, value);
        assert_eq!(option_label, label);
    }
    assert_eq!(
        display_rotation_label(DisplayRotation::Deg180),
        "Upside down"
    );
}
