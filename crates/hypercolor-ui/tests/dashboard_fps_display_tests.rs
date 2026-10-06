use hypercolor_ui::pages::dashboard::fps_display::{
    ema_step, stabilize_fps_for_display, stabilize_fps_for_display_f32,
};

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

#[test]
fn ema_step_seeds_on_the_first_sample() {
    assert_eq!(ema_step(None, 42.0, 0.3), 42.0);
}

#[test]
fn ema_step_moves_a_fraction_toward_the_sample() {
    let next = ema_step(Some(20.0), 60.0, 0.3);
    assert!((next - 32.0).abs() < 1e-9, "expected 32.0, got {next}");
}

#[test]
fn ema_step_converges_when_samples_settle() {
    // A gauge that warmed up at 20 fps and then locked to its 60 fps target
    // keeps receiving 60 every metrics sample; stepping per sample must
    // carry the average all the way there.
    let mut average = Some(20.0);
    for _ in 0..20 {
        average = Some(ema_step(average, 60.0, 0.3));
    }
    let settled = average.expect("average should be seeded");
    assert!(
        (settled - 60.0).abs() < 0.05,
        "expected ~60.0, got {settled}"
    );
}
