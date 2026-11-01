//! When a drag on the phone bottom sheet's grab area dismisses it.

use hypercolor_ui::components::bottom_sheet::{
    DISMISS_DISTANCE_PX, FLICK_MIN_PX, FLICK_VELOCITY, drag_dismisses, smoothed_velocity,
    velocity_at_release,
};

#[test]
fn a_long_drag_dismisses_a_tall_sheet() {
    assert!(drag_dismisses(DISMISS_DISTANCE_PX, 700.0, 0.0));
    assert!(!drag_dismisses(DISMISS_DISTANCE_PX - 1.0, 700.0, 0.0));
}

#[test]
fn a_short_sheet_dismisses_at_a_quarter_of_its_height() {
    assert!(drag_dismisses(60.0, 240.0, 0.0));
    assert!(!drag_dismisses(59.0, 240.0, 0.0));
}

#[test]
fn a_downward_flick_dismisses_once_it_has_moved() {
    assert!(drag_dismisses(FLICK_MIN_PX, 700.0, FLICK_VELOCITY));
    assert!(!drag_dismisses(FLICK_MIN_PX - 1.0, 700.0, 2.0));
    assert!(!drag_dismisses(40.0, 700.0, FLICK_VELOCITY - 0.01));
}

#[test]
fn an_upward_or_still_release_keeps_the_sheet() {
    assert!(!drag_dismisses(0.0, 700.0, 3.0));
    assert!(!drag_dismisses(-50.0, 700.0, 3.0));
}

#[test]
fn velocity_smoothing_resists_one_noisy_sample() {
    let steady = smoothed_velocity(0.0, 10.0, 16.0);
    assert!((steady - 0.375).abs() < 1e-9);
    // A zero-length interval keeps the previous estimate.
    assert!((smoothed_velocity(0.8, 50.0, 0.0) - 0.8).abs() < 1e-9);
    // One slow sample after a fast run stays a flick-sized estimate.
    let blended = smoothed_velocity(1.2, 2.0, 16.0);
    assert!(blended > FLICK_VELOCITY);
}

#[test]
fn a_release_right_after_a_move_keeps_its_flick() {
    assert!((velocity_at_release(1.2, 0.0) - 1.2).abs() < 1e-9);
    assert!(drag_dismisses(30.0, 700.0, velocity_at_release(1.2, 8.0)));
}

#[test]
fn a_release_one_frame_after_the_last_move_is_still_a_flick() {
    // A 50px pull at 0.6px/ms released 16ms after its last move: just
    // over the flick speed, and a frame is no pause.
    let released = velocity_at_release(0.6, 16.0);
    assert!((released - 0.6).abs() < 1e-9, "{released}");
    assert!(drag_dismisses(50.0, 700.0, released));
}

#[test]
fn a_pause_before_release_is_judged_by_position_alone() {
    // Pull 30px fast, hold still, then let go: not a flick.
    let held = velocity_at_release(1.2, 350.0);
    assert!(held < FLICK_VELOCITY / 100.0, "{held}");
    assert!(!drag_dismisses(30.0, 700.0, held));
    // Position still decides on its own.
    assert!(drag_dismisses(DISMISS_DISTANCE_PX, 700.0, held));
}
