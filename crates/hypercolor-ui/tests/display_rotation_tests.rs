use hypercolor_types::scene::DisplayRotation;
use hypercolor_ui::display_rotation::{
    DISPLAY_ROTATIONS, display_rotation_label, display_rotation_select_options,
    display_rotation_value, mounted_footprint, mounted_preview, parse_display_rotation,
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

#[test]
fn an_upright_panel_previews_its_frame_as_sent() {
    let preview = mounted_preview(960, 160, DisplayRotation::Deg0);
    assert_eq!(preview.host_style, "aspect-ratio: 960 / 160;");
    assert_eq!(preview.frame_style, "");
}

#[test]
fn an_upside_down_panel_turns_its_frame_back_in_place() {
    let preview = mounted_preview(400, 400, DisplayRotation::Deg180);
    assert_eq!(preview.host_style, "aspect-ratio: 400 / 400;");
    assert_eq!(preview.frame_style, "transform: rotate(180deg);");
}

#[test]
fn a_panel_on_its_side_swaps_its_footprint_and_turns_with_the_mount() {
    // The daemon samples a viewport turned clockwise by the mount, which
    // leaves the frame turned the other way; turning it clockwise by the
    // same angle is what mounting the panel does.
    let clockwise = mounted_preview(960, 160, DisplayRotation::Deg90);
    assert_eq!(
        clockwise.host_style,
        "aspect-ratio: 160 / 960; display: grid; place-items: center;"
    );
    assert_eq!(
        clockwise.frame_style,
        "width: 600.0000%; height: 16.6667%; transform: rotate(90deg);"
    );

    let counter = mounted_preview(960, 160, DisplayRotation::Deg270);
    assert_eq!(counter.host_style, clockwise.host_style);
    assert_eq!(
        counter.frame_style,
        "width: 600.0000%; height: 16.6667%; transform: rotate(270deg);"
    );
}

#[test]
fn every_mount_turns_the_frame_by_its_own_clockwise_angle() {
    for (mount, _, _) in DISPLAY_ROTATIONS {
        let degrees = mount.radians().to_degrees().round();
        let style = mounted_preview(480, 480, mount).frame_style;
        if degrees == 0.0 {
            assert_eq!(style, "");
        } else {
            assert!(
                style.ends_with(&format!("transform: rotate({degrees}deg);")),
                "{mount:?}: {style}"
            );
        }
    }
}

#[test]
fn a_panel_with_no_size_yet_still_has_a_footprint() {
    let preview = mounted_preview(0, 0, DisplayRotation::Deg90);
    assert_eq!(
        preview.host_style,
        "aspect-ratio: 1 / 1; display: grid; place-items: center;"
    );
}

#[test]
fn only_a_quarter_turn_swaps_the_footprint() {
    assert_eq!(
        mounted_footprint(960, 160, DisplayRotation::Deg0),
        (960, 160)
    );
    assert_eq!(
        mounted_footprint(960, 160, DisplayRotation::Deg180),
        (960, 160)
    );
    assert_eq!(
        mounted_footprint(960, 160, DisplayRotation::Deg90),
        (160, 960)
    );
    assert_eq!(
        mounted_footprint(960, 160, DisplayRotation::Deg270),
        (160, 960)
    );
    assert_eq!(mounted_footprint(0, 0, DisplayRotation::Deg0), (1, 1));
}
