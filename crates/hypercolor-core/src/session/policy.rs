//! Mapping from raw session events to sleep and wake actions.

use crate::types::session::{
    OffOutputBehavior, SessionConfig, SessionEvent, SleepAction, SleepBehavior, WakeAction,
};

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
                self.config.off_output_behavior,
                &self.config.off_output_color,
                self.config.on_screen_lock,
                self.config.screen_lock_brightness,
                &self.config.screen_lock_scene,
                self.config.screen_lock_fade_ms,
            )),
            SessionEvent::Suspending => Some(sleep_action_from_behavior(
                self.config.off_output_behavior,
                &self.config.off_output_color,
                self.config.on_suspend,
                0.0,
                "",
                self.config.suspend_fade_ms,
            )),
            SessionEvent::LidClosed => Some(sleep_action_from_behavior(
                self.config.off_output_behavior,
                &self.config.off_output_color,
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
                        output_behavior: self.config.off_output_behavior,
                        static_color: parse_static_color(&self.config.off_output_color),
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
    off_output_behavior: OffOutputBehavior,
    off_output_color: &str,
    behavior: SleepBehavior,
    brightness: f32,
    scene_name: &str,
    fade_ms: u64,
) -> SleepAction {
    match behavior {
        SleepBehavior::Ignore => SleepAction::Ignore,
        SleepBehavior::Off => SleepAction::Off {
            fade_ms,
            output_behavior: off_output_behavior,
            static_color: parse_static_color(off_output_color),
        },
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

fn parse_static_color(raw: &str) -> [u8; 3] {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    match hex.len() {
        3 => {
            let bytes = hex.as_bytes();
            [
                parse_nibble(bytes[0]).map_or(0, |value| (value << 4) | value),
                parse_nibble(bytes[1]).map_or(0, |value| (value << 4) | value),
                parse_nibble(bytes[2]).map_or(0, |value| (value << 4) | value),
            ]
        }
        6 => [
            parse_byte(&hex[0..2]).unwrap_or(0),
            parse_byte(&hex[2..4]).unwrap_or(0),
            parse_byte(&hex[4..6]).unwrap_or(0),
        ],
        _ => [0, 0, 0],
    }
}

fn parse_byte(raw: &str) -> Option<u8> {
    u8::from_str_radix(raw, 16).ok()
}

fn parse_nibble(raw: u8) -> Option<u8> {
    match raw {
        b'0'..=b'9' => Some(raw - b'0'),
        b'a'..=b'f' => Some(raw - b'a' + 10),
        b'A'..=b'F' => Some(raw - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::SleepPolicy;
    use crate::types::session::{
        OffOutputBehavior, SessionConfig, SessionEvent, SleepAction, SleepBehavior, WakeAction,
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
            Some(SleepAction::Off {
                fade_ms: 5_000,
                output_behavior: OffOutputBehavior::Static,
                static_color: [0, 0, 0],
            })
        );
    }
}
