//! Public-API coverage for the render canvas extent resolution.

use hypercolor_ui::render_canvas::{
    DEFAULT_RENDER_CANVAS, aspect_ratio_css, resolve_render_canvas_size,
};

#[test]
fn config_wins_over_frame() {
    assert_eq!(
        resolve_render_canvas_size(Some((1280, 720)), Some((640, 480))),
        (1280, 720)
    );
}

#[test]
fn frame_fills_in_before_config_loads() {
    assert_eq!(
        resolve_render_canvas_size(None, Some((800, 600))),
        (800, 600)
    );
}

#[test]
fn zero_dimensions_fall_through() {
    assert_eq!(
        resolve_render_canvas_size(Some((0, 480)), Some((0, 0))),
        DEFAULT_RENDER_CANVAS
    );
}

#[test]
fn aspect_ratio_never_divides_by_zero() {
    assert_eq!(aspect_ratio_css((0, 0)), "1 / 1");
    assert_eq!(aspect_ratio_css((640, 480)), "640 / 480");
}
