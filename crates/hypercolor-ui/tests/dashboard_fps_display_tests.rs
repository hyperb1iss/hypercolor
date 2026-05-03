#[path = "../src/pages/dashboard/fps_display.rs"]
mod fps_display;

use fps_display::{stabilize_fps_for_display, stabilize_fps_for_display_f32};

#[test]
fn stabilize_fps_locks_near_target_noise() {
    assert_eq!(stabilize_fps_for_display(58.2, 60), 60.0);
    assert_eq!(stabilize_fps_for_display(61.7, 60), 60.0);
    assert_eq!(stabilize_fps_for_display(29.0, 30), 30.0);
}

#[test]
fn stabilize_fps_keeps_real_drops_visible() {
    assert_eq!(stabilize_fps_for_display(55.5, 60), 55.5);
    assert_eq!(stabilize_fps_for_display(27.5, 30), 27.5);
}

#[test]
fn stabilize_fps_ignores_invalid_or_unknown_targets() {
    assert_eq!(stabilize_fps_for_display(f64::NAN, 60), 0.0);
    assert_eq!(stabilize_fps_for_display(-1.0, 60), 0.0);
    assert_eq!(stabilize_fps_for_display(42.4, 0), 42.4);
}

#[test]
fn stabilize_fps_handles_f32_preview_values() {
    assert_eq!(stabilize_fps_for_display_f32(58.2, 60), 60.0);
    assert_eq!(stabilize_fps_for_display_f32(55.5, 60), 55.5);
}
