use hypercolor_ui::render_presets;

#[test]
fn canvas_preset_key_matches_added_high_res_presets() {
    assert_eq!(render_presets::canvas_preset_key(1280, 1024), "1280x1024");
    assert_eq!(render_presets::canvas_preset_key(3440, 1440), "3440x1440");
    assert_eq!(render_presets::canvas_preset_key(3840, 2160), "3840x2160");
    assert_eq!(render_presets::canvas_preset_key(5120, 2880), "5120x2880");
    assert_eq!(render_presets::canvas_preset_key(7680, 4320), "7680x4320");
}

#[test]
fn canvas_preset_key_falls_back_to_custom_for_unknown_size() {
    assert_eq!(render_presets::canvas_preset_key(1234, 777), "custom");
    assert_eq!(render_presets::canvas_preset_key(13, 17), "custom");
}

#[test]
fn custom_canvas_limits_match_the_runtime_dimension_type() {
    assert_eq!(render_presets::MAX_CUSTOM_CANVAS_WIDTH, f64::from(u32::MAX));
    assert_eq!(
        render_presets::MAX_CUSTOM_CANVAS_HEIGHT,
        f64::from(u32::MAX)
    );
}
