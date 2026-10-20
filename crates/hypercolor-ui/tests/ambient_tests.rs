use hypercolor_ui::ambient::{
    AMBIENT_HUE_DEADBAND_DEG, AMBIENT_PUBLISH_INTERVAL_MS, AmbientHuePublisher, shortest_hue_delta,
};

const LATER: f64 = AMBIENT_PUBLISH_INTERVAL_MS;

#[test]
fn the_first_hue_publishes_at_once_normalized_into_the_wheel() {
    let mut publisher = AmbientHuePublisher::default();
    assert_eq!(publisher.offer(-30.0, 0.0), Some(330.0));
    assert_eq!(publisher.published(), Some(330.0));
}

#[test]
fn a_change_inside_the_interval_waits_for_the_next_slot() {
    let mut publisher = AmbientHuePublisher::default();
    publisher.offer(100.0, 0.0);
    assert_eq!(publisher.offer(160.0, LATER - 1.0), None);
    assert_eq!(publisher.offer(160.0, LATER), Some(160.0));
}

#[test]
fn a_change_below_the_deadband_never_publishes() {
    let mut publisher = AmbientHuePublisher::default();
    publisher.offer(100.0, 0.0);
    let jitter = AMBIENT_HUE_DEADBAND_DEG * 0.9;
    assert_eq!(publisher.offer(100.0 + jitter, LATER), None);
    assert_eq!(publisher.offer(100.0 - jitter, LATER * 2.0), None);
    assert_eq!(publisher.published(), Some(100.0));
}

#[test]
fn a_rejected_offer_does_not_restart_the_interval() {
    let mut publisher = AmbientHuePublisher::default();
    publisher.offer(100.0, 0.0);
    assert_eq!(publisher.offer(101.0, LATER), None);
    assert_eq!(publisher.offer(150.0, LATER + 1.0), Some(150.0));
}

#[test]
fn crossing_zero_publishes_an_unwrapped_value_the_short_way() {
    let mut publisher = AmbientHuePublisher::default();
    publisher.offer(350.0, 0.0);
    assert_eq!(publisher.offer(10.0, LATER), Some(370.0));
    assert_eq!(publisher.offer(340.0, LATER * 2.0), Some(340.0));
}

#[test]
fn a_full_lap_keeps_climbing_instead_of_snapping_back() {
    let mut publisher = AmbientHuePublisher::default();
    let mut now = 0.0;
    let mut last = publisher.offer(0.0, now).expect("first publish");
    for step in 1..=12 {
        now += LATER;
        let hue = f64::from(step * 30 % 360);
        let next = publisher
            .offer(hue, now)
            .expect("every 30 degree step publishes");
        assert!(
            (next - last - 30.0).abs() < 1e-9,
            "step {step}: {last} -> {next}"
        );
        last = next;
    }
    assert!((last - 360.0).abs() < 1e-9);
}

#[test]
fn non_finite_input_is_ignored() {
    let mut publisher = AmbientHuePublisher::default();
    assert_eq!(publisher.offer(f64::NAN, 0.0), None);
    assert_eq!(publisher.offer(120.0, f64::INFINITY), None);
    assert_eq!(publisher.published(), None);
}

#[test]
fn shortest_delta_takes_the_shorter_arc() {
    assert!((shortest_hue_delta(350.0, 10.0) - 20.0).abs() < 1e-9);
    assert!((shortest_hue_delta(10.0, 350.0) + 20.0).abs() < 1e-9);
    assert!((shortest_hue_delta(370.0, 20.0) - 10.0).abs() < 1e-9);
    assert!((shortest_hue_delta(0.0, 180.0) - 180.0).abs() < 1e-9);
}
