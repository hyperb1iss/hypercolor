use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use hypercolor_core::input::InputSource;
use hypercolor_core::input::screen::{
    CaptureCadence, CaptureCadenceError, CaptureConfig, MAX_REPRESENTABLE_CAPTURE_FPS,
    ScreenCaptureInput,
};

#[test]
fn cadence_rejects_only_unrepresentable_scheduler_requests() {
    assert_eq!(CaptureCadence::new(0), Err(CaptureCadenceError::Zero));
    assert!(CaptureCadence::new(MAX_REPRESENTABLE_CAPTURE_FPS).is_ok());
    assert_eq!(
        CaptureCadence::new(MAX_REPRESENTABLE_CAPTURE_FPS + 1),
        Err(CaptureCadenceError::ClockResolutionExceeded {
            requested_fps: MAX_REPRESENTABLE_CAPTURE_FPS + 1,
            maximum_fps: MAX_REPRESENTABLE_CAPTURE_FPS,
        })
    );
}

#[test]
fn freshness_window_never_rounds_to_zero() {
    let cadence = CaptureCadence::new(MAX_REPRESENTABLE_CAPTURE_FPS)
        .expect("maximum representable cadence must be admitted");
    let captured_at = Instant::now();

    assert_eq!(
        cadence.interval_window(NonZeroU32::new(2).expect("two is non-zero")),
        Duration::from_nanos(2)
    );
    assert!(
        cadence
            .freshness_deadline(captured_at)
            .expect("live capture deadline fits Instant")
            > captured_at
    );
}

#[test]
fn rational_pacer_preserves_exact_average_without_zero_intervals() {
    let cadence = CaptureCadence::new(60).expect("60 FPS must be representable");
    let mut pacer = cadence.pacer();
    let total = (0..60)
        .map(|_| pacer.next_interval())
        .fold(Duration::ZERO, |total, interval| {
            assert!(!interval.is_zero());
            total + interval
        });

    assert_eq!(total, Duration::from_secs(1));
}

#[test]
fn rational_pacer_supports_the_clock_resolution_boundary() {
    let cadence = CaptureCadence::new(MAX_REPRESENTABLE_CAPTURE_FPS)
        .expect("maximum representable cadence must be admitted");
    let mut pacer = cadence.pacer();

    for _ in 0..8 {
        assert_eq!(pacer.next_interval(), Duration::from_nanos(1));
    }
}

#[test]
fn deadline_advancement_drops_lateness_without_drift() {
    let cadence = CaptureCadence::new(30).expect("30 FPS must be representable");
    let mut pacer = cadence.pacer();
    let initial = Instant::now();
    let late = initial + Duration::from_secs(1);

    assert!(
        pacer
            .advance_deadline(initial, late)
            .expect("live scheduler deadline fits Instant")
            > late
    );
}

#[test]
fn generic_live_reconfiguration_rejects_invalid_cadence_transactionally() {
    let baseline = CaptureConfig::default();
    let mut input = ScreenCaptureInput::new(baseline.clone());
    let mut rejected = baseline.clone();
    rejected.target_fps = MAX_REPRESENTABLE_CAPTURE_FPS + 1;

    let error = input
        .apply_settings(rejected)
        .expect_err("unrepresentable cadence must fail before mutating settings");

    assert!(matches!(
        error,
        CaptureCadenceError::ClockResolutionExceeded { .. }
    ));
    assert_eq!(input.config(), &baseline);
}

#[test]
fn generic_capture_rejects_invalid_initial_cadence_before_starting() {
    let config = CaptureConfig {
        target_fps: MAX_REPRESENTABLE_CAPTURE_FPS + 1,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config);

    let error = input
        .start()
        .expect_err("unrepresentable cadence must fail before session mutation");

    assert!(error.to_string().contains("scheduler clock limit"));
    assert!(!input.is_running());
}
