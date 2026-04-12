#![allow(dead_code)]

#[path = "../src/render_presets.rs"]
mod render_presets;

#[test]
fn canvas_preset_key_matches_added_high_res_presets() {
    assert_eq!(render_presets::canvas_preset_key(1280, 1024), "1280x1024");
    assert_eq!(render_presets::canvas_preset_key(3440, 1440), "3440x1440");
    assert_eq!(render_presets::canvas_preset_key(3840, 2160), "3840x2160");
}

#[test]
fn canvas_preset_key_falls_back_to_custom_for_unknown_size() {
    assert_eq!(render_presets::canvas_preset_key(1234, 777), "custom");
    assert_eq!(render_presets::canvas_preset_key(5120, 2880), "custom");
    assert_eq!(render_presets::canvas_preset_key(7680, 4320), "custom");
}

#[test]
fn custom_canvas_limits_cap_at_4k() {
    assert_eq!(render_presets::MAX_CUSTOM_CANVAS_WIDTH, 3840.0);
    assert_eq!(render_presets::MAX_CUSTOM_CANVAS_HEIGHT, 2160.0);
}
