//! Mapping from raw session events to sleep and wake actions.

use crate::types::session::{SessionConfig, SessionEvent, SleepAction, SleepBehavior, WakeAction};

const IDLE_DIM_BRIGHTNESS: f32 = 0.3;
const IDLE_DIM_FADE_MS: u64 = 3_000;
const IDLE_OFF_FADE_MS: u64 = 5_000;

/// Preference-driven session policy mapper.
#[derive(Debug, Clone)]
pub struct SleepPolicy {
    config: SessionConfig,
}

impl SleepPolicy {
    /// Create a policy from a config snapshot.
    #[must_use]
    pub fn new(config: SessionConfig) -> Self {
        Self { config }
    }

    /// Resolve a sleep-side action for a session event, if any.
    #[must_use]
    pub fn sleep_action(&self, event: &SessionEvent) -> Option<SleepAction> {
        match event {
            SessionEvent::ScreenLocked => Some(sleep_action_from_behavior(
                self.config.on_screen_lock,
                self.config.screen_lock_brightness,
                &self.config.screen_lock_scene,
                self.config.screen_lock_fade_ms,
            )),
            SessionEvent::Suspending => Some(sleep_action_from_behavior(
                self.config.on_suspend,
                0.0,
                "",
                self.config.suspend_fade_ms,
            )),
            SessionEvent::LidClosed => Some(sleep_action_from_behavior(
                self.config.on_lid_close,
                self.config.lid_close_brightness,
                &self.config.lid_close_scene,
                self.config.lid_close_fade_ms,
            )),
            SessionEvent::IdleEntered { idle_duration } => {
                if !self.config.idle_enabled {
                    return None;
                }

                let idle_secs = idle_duration.as_secs();
                if idle_secs >= self.config.idle_off_timeout_secs {
                    Some(SleepAction::Off {
                        fade_ms: IDLE_OFF_FADE_MS,
                    })
                } else if idle_secs >= self.config.idle_dim_timeout_secs {
                    Some(SleepAction::Dim {
                        brightness: IDLE_DIM_BRIGHTNESS,
                        fade_ms: IDLE_DIM_FADE_MS,
                    })
                } else {
                    None
                }
            }
            SessionEvent::ScreenUnlocked
            | SessionEvent::Resumed
            | SessionEvent::IdleExited
            | SessionEvent::LidOpened => None,
        }
    }

    /// Resolve a wake-side action for a session event, if any.
    #[must_use]
    pub fn wake_action(&self, event: &SessionEvent) -> Option<WakeAction> {
        match event {
            SessionEvent::ScreenUnlocked => Some(WakeAction::Restore {
                fade_ms: self.config.screen_unlock_fade_ms,
            }),
            SessionEvent::Resumed => Some(WakeAction::Restore {
                fade_ms: self.config.resume_fade_ms,
            }),
            SessionEvent::IdleExited => {
                if !self.config.idle_enabled {
                    return None;
                }
                Some(WakeAction::Restore { fade_ms: 300 })
            }
            SessionEvent::LidOpened => Some(WakeAction::Restore {
                fade_ms: self.config.lid_open_fade_ms,
            }),
            SessionEvent::ScreenLocked
            | SessionEvent::Suspending
            | SessionEvent::IdleEntered { .. }
            | SessionEvent::LidClosed => None,
        }
    }
}

fn sleep_action_from_behavior(
    behavior: SleepBehavior,
    brightness: f32,
    scene_name: &str,
    fade_ms: u64,
) -> SleepAction {
    match behavior {
        SleepBehavior::Ignore => SleepAction::Ignore,
        SleepBehavior::Off => SleepAction::Off { fade_ms },
        SleepBehavior::Dim => SleepAction::Dim {
            brightness: brightness.clamp(0.0, 1.0),
            fade_ms,
        },
        SleepBehavior::Scene => {
            if scene_name.trim().is_empty() {
                SleepAction::Ignore
            } else {
                SleepAction::Scene {
                    scene_name: scene_name.to_owned(),
                    fade_ms,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::SleepPolicy;
    use crate::types::session::{
        SessionConfig, SessionEvent, SleepAction, SleepBehavior, WakeAction,
    };

    #[test]
    fn screen_lock_maps_to_ignore_by_default() {
        let policy = SleepPolicy::new(SessionConfig::default());
        assert_eq!(
            policy.sleep_action(&SessionEvent::ScreenLocked),
            Some(SleepAction::Ignore)
        );
    }

    #[test]
    fn unlock_maps_to_restore() {
        let policy = SleepPolicy::new(SessionConfig::default());
        assert_eq!(
            policy.wake_action(&SessionEvent::ScreenUnlocked),
            Some(WakeAction::Restore { fade_ms: 500 })
        );
    }

    #[test]
    fn scene_behavior_requires_non_empty_scene_name() {
        let mut config = SessionConfig {
            on_screen_lock: SleepBehavior::Scene,
            ..SessionConfig::default()
        };
        let policy = SleepPolicy::new(config.clone());
        assert_eq!(
            policy.sleep_action(&SessionEvent::ScreenLocked),
            Some(SleepAction::Ignore)
        );

        config.screen_lock_scene = "desk-night".to_owned();
        let policy = SleepPolicy::new(config);
        assert_eq!(
            policy.sleep_action(&SessionEvent::ScreenLocked),
            Some(SleepAction::Scene {
                scene_name: "desk-night".to_owned(),
                fade_ms: 2_000,
            })
        );
    }

    #[test]
    fn idle_thresholds_progress_from_dim_to_off() {
        let policy = SleepPolicy::new(SessionConfig::default());
        assert_eq!(
            policy.sleep_action(&SessionEvent::IdleEntered {
                idle_duration: Duration::from_secs(120),
            }),
            Some(SleepAction::Dim {
                brightness: 0.3,
                fade_ms: 3_000,
            })
        );
        assert_eq!(
            policy.sleep_action(&SessionEvent::IdleEntered {
                idle_duration: Duration::from_secs(600),
            }),
            Some(SleepAction::Off { fade_ms: 5_000 })
        );
    }
}
