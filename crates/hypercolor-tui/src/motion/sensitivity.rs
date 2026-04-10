//! Motion sensitivity setting for accessibility and reduced motion.
//!
//! Three levels:
//! - `Off`     — no motion at all; the TUI behaves as it did before tachyonfx
//! - `Subtle`  — short durations, small amplitudes, no full-screen effects
//! - `Full`    — the complete Spec 38 catalog at full intensity
//!
//! Defaults to `Full`. Respects `REDUCE_MOTION` env var by capping at
//! `Subtle`. The user can cycle with the `M` keybinding.

use serde::{Deserialize, Serialize};

/// Motion sensitivity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSensitivity {
    /// No motion. Effects are completely suppressed.
    Off,
    /// Reduced motion. Shorter durations, no full-screen effects.
    Subtle,
    /// Full motion. The complete Spec 38 catalog at full intensity.
    #[default]
    Full,
}

impl MotionSensitivity {
    /// Resolve from environment, honoring `REDUCE_MOTION` as a max cap.
    ///
    /// `REDUCE_MOTION` set → caps at `Subtle` regardless of `requested`.
    /// `HYPERCOLOR_MOTION=off|subtle|full` → explicit override.
    /// Otherwise → `requested` (typically the persisted preference).
    #[must_use]
    pub fn resolve(requested: MotionSensitivity) -> Self {
        let from_env = std::env::var("HYPERCOLOR_MOTION")
            .ok()
            .and_then(|v| match v.to_ascii_lowercase().as_str() {
                "off" => Some(Self::Off),
                "subtle" => Some(Self::Subtle),
                "full" => Some(Self::Full),
                _ => None,
            })
            .unwrap_or(requested);

        if std::env::var_os("REDUCE_MOTION").is_some() && from_env == Self::Full {
            Self::Subtle
        } else {
            from_env
        }
    }

    /// Cycle Off → Subtle → Full → Off.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Subtle,
            Self::Subtle => Self::Full,
            Self::Full => Self::Off,
        }
    }

    /// Display label for the status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Subtle => "subtle",
            Self::Full => "full",
        }
    }

    /// Multiplier applied to effect amplitudes (0.0..=1.0).
    #[must_use]
    pub fn amplitude(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Subtle => 0.5,
            Self::Full => 1.0,
        }
    }

    /// Multiplier applied to effect durations (longer = more visible).
    #[must_use]
    pub fn duration_scale(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Subtle => 0.6,
            Self::Full => 1.0,
        }
    }
}
