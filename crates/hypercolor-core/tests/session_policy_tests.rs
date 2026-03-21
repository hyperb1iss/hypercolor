use std::time::Duration;

use hypercolor_core::session::SleepPolicy;
use hypercolor_types::session::{
    OffOutputBehavior, SessionConfig, SessionEvent, SleepAction, SleepBehavior,
};

#[test]
fn off_sleep_actions_carry_static_hold_policy() {
    let config = SessionConfig {
        off_output_behavior: OffOutputBehavior::Static,
        off_output_color: "#102030".to_owned(),
        on_suspend: SleepBehavior::Off,
        ..SessionConfig::default()
    };

    let policy = SleepPolicy::new(config);
    assert_eq!(
        policy.sleep_action(&SessionEvent::Suspending),
        Some(SleepAction::Off {
            fade_ms: 300,
            output_behavior: OffOutputBehavior::Static,
            static_color: [0x10, 0x20, 0x30],
        })
    );
}

#[test]
fn idle_off_actions_can_release_devices() {
    let config = SessionConfig {
        off_output_behavior: OffOutputBehavior::Release,
        idle_off_timeout_secs: 5,
        ..SessionConfig::default()
    };

    let policy = SleepPolicy::new(config);
    assert_eq!(
        policy.sleep_action(&SessionEvent::IdleEntered {
            idle_duration: Duration::from_secs(5),
        }),
        Some(SleepAction::Off {
            fade_ms: 5_000,
            output_behavior: OffOutputBehavior::Release,
            static_color: [0, 0, 0],
        })
    );
}
