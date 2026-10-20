use hypercolor_ui::ambient::{
    AMBIENT_HUE_DEADBAND_DEG, AMBIENT_HUE_FALLBACK_DEG, AMBIENT_PUBLISH_INTERVAL_MS,
    AmbientHuePublisher, shortest_hue_delta,
};

const LATER: f64 = AMBIENT_PUBLISH_INTERVAL_MS;

#[test]
fn the_first_hue_publishes_at_once_unwrapped_from_the_css_fallback() {
    let mut publisher = AmbientHuePublisher::default();
    assert_eq!(publisher.published(), AMBIENT_HUE_FALLBACK_DEG);
    // 320 to 10 is 50 degrees forward, not 310 back through green.
    assert_eq!(publisher.offer(10.0, 0.0), Some(370.0));
}

#[test]
fn a_first_hue_beside_the_fallback_leaves_the_css_alone() {
    let mut publisher = AmbientHuePublisher::default();
    let near = AMBIENT_HUE_FALLBACK_DEG + AMBIENT_HUE_DEADBAND_DEG * 0.5;
    assert_eq!(publisher.offer(near, 0.0), None);
    assert_eq!(publisher.published(), AMBIENT_HUE_FALLBACK_DEG);
}

#[test]
fn a_publisher_resuming_from_an_unwrapped_value_keeps_turning_the_short_way() {
    // A remounted shell picks up where `--ambient-hue` was left, laps and all.
    let mut publisher = AmbientHuePublisher::starting_at(1_090.0);
    assert_eq!(publisher.offer(20.0, 0.0), Some(1_100.0));
}

#[test]
fn a_non_finite_starting_value_falls_back() {
    let publisher = AmbientHuePublisher::starting_at(f64::NAN);
    assert_eq!(publisher.published(), AMBIENT_HUE_FALLBACK_DEG);
}

#[test]
fn a_change_inside_the_interval_waits_for_the_next_slot() {
    let mut publisher = AmbientHuePublisher::starting_at(100.0);
    assert_eq!(publisher.offer(120.0, 0.0), Some(120.0));
    assert_eq!(publisher.offer(160.0, LATER - 1.0), None);
    assert_eq!(publisher.offer(160.0, LATER), Some(160.0));
}

#[test]
fn a_change_below_the_deadband_never_publishes() {
    let mut publisher = AmbientHuePublisher::starting_at(100.0);
    let jitter = AMBIENT_HUE_DEADBAND_DEG * 0.9;
    assert_eq!(publisher.offer(100.0 + jitter, 0.0), None);
    assert_eq!(publisher.offer(100.0 - jitter, LATER), None);
    assert_eq!(publisher.published(), 100.0);
}

#[test]
fn a_rejected_offer_does_not_restart_the_interval() {
    let mut publisher = AmbientHuePublisher::starting_at(100.0);
    assert_eq!(publisher.offer(140.0, 0.0), Some(140.0));
    assert_eq!(publisher.offer(141.0, LATER), None);
    assert_eq!(publisher.offer(190.0, LATER + 1.0), Some(190.0));
}

#[test]
fn crossing_zero_publishes_an_unwrapped_value_the_short_way() {
    let mut publisher = AmbientHuePublisher::starting_at(350.0);
    assert_eq!(publisher.offer(10.0, 0.0), Some(370.0));
    assert_eq!(publisher.offer(340.0, LATER), Some(340.0));
}

#[test]
fn a_full_lap_keeps_climbing_instead_of_snapping_back() {
    let mut publisher = AmbientHuePublisher::starting_at(0.0);
    let mut now = 0.0;
    let mut last = publisher.published();
    for step in 1..=12 {
        let hue = f64::from(step * 30 % 360);
        let next = publisher
            .offer(hue, now)
            .expect("every 30 degree step publishes");
        assert!(
            (next - last - 30.0).abs() < 1e-9,
            "step {step}: {last} -> {next}"
        );
        last = next;
        now += LATER;
    }
    assert!((last - 360.0).abs() < 1e-9);
}

#[test]
fn non_finite_input_is_ignored() {
    let mut publisher = AmbientHuePublisher::default();
    assert_eq!(publisher.offer(f64::NAN, 0.0), None);
    assert_eq!(publisher.offer(120.0, f64::INFINITY), None);
    assert_eq!(publisher.published(), AMBIENT_HUE_FALLBACK_DEG);
}

#[test]
fn shortest_delta_takes_the_shorter_arc() {
    assert!((shortest_hue_delta(350.0, 10.0) - 20.0).abs() < 1e-9);
    assert!((shortest_hue_delta(10.0, 350.0) + 20.0).abs() < 1e-9);
    assert!((shortest_hue_delta(370.0, 20.0) - 10.0).abs() < 1e-9);
    assert!((shortest_hue_delta(0.0, 180.0) - 180.0).abs() < 1e-9);
}
