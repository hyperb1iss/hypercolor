#![cfg(all(feature = "ws-core", not(target_arch = "wasm32")))]

use hypercolor_leptos_ext::ws::{ExponentialBackoff, Jitter};
use std::time::Duration;

#[test]
fn default_backoff_matches_hypercolor_schedule_midpoint() {
    let policy = ExponentialBackoff::HYPERCOLOR_DEFAULT;
    let midpoint = |attempt| policy.delay_for_attempt_with_sample(attempt, 0.5);

    assert_eq!(midpoint(0), Some(Duration::from_millis(500)));
    assert_eq!(midpoint(1), Some(Duration::from_secs(1)));
    assert_eq!(midpoint(2), Some(Duration::from_secs(2)));
    assert_eq!(midpoint(3), Some(Duration::from_secs(4)));
    assert_eq!(midpoint(4), Some(Duration::from_secs(8)));
    assert_eq!(midpoint(5), Some(Duration::from_secs(15)));
    assert_eq!(midpoint(8), Some(Duration::from_secs(15)));
}

#[test]
fn equal_jitter_scales_delay_with_sample() {
    let policy = ExponentialBackoff {
        base: Duration::from_secs(4),
        max: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: Jitter::Equal(0.25),
    };

    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 0.0),
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 0.5),
        Some(Duration::from_secs(4))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 1.0),
        Some(Duration::from_secs(5))
    );
}

#[test]
fn no_jitter_ignores_the_sample_and_caps_at_max() {
    let policy = ExponentialBackoff {
        base: Duration::from_millis(250),
        max: Duration::from_secs(3),
        multiplier: 2.0,
        jitter: Jitter::None,
    };

    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 0.0),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(2, 1.0),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(9, 0.5),
        Some(Duration::from_secs(3))
    );
}

#[test]
fn a_degenerate_schedule_produces_no_delay() {
    let zero_base = ExponentialBackoff {
        base: Duration::ZERO,
        ..ExponentialBackoff::HYPERCOLOR_DEFAULT
    };
    assert_eq!(zero_base.delay_for_attempt_with_sample(0, 0.5), None);

    let zero_max = ExponentialBackoff {
        max: Duration::ZERO,
        ..ExponentialBackoff::HYPERCOLOR_DEFAULT
    };
    assert_eq!(zero_max.delay_for_attempt_with_sample(0, 0.5), None);

    let no_growth = ExponentialBackoff {
        multiplier: 0.0,
        ..ExponentialBackoff::HYPERCOLOR_DEFAULT
    };
    assert_eq!(no_growth.delay_for_attempt_with_sample(0, 0.5), None);
}
