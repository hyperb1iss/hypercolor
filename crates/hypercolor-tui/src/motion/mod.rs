//! Motion layer — tachyonfx-powered visual effects on top of ratatui rendering.
//!
//! See `docs/specs/38-tui-motion-layer.md` for the full design.
//!
//! Architecture:
//! - `MotionSystem` wraps tachyonfx's `EffectManager` keyed by `MotionKey`
//! - Ticked once per render frame as a post-process over the composed buffer
//! - Effects are triggered through Action variants and dispatched centrally
//! - Sensitivity setting (off/subtle/full) controls amplitude and disables
//!   effects entirely when off
//!
//! Phase 1 (this commit): scaffold only — no actual effects yet.

pub mod catalog;
pub mod keys;
pub mod sensitivity;

pub use keys::MotionKey;
pub use sensitivity::MotionSensitivity;

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tachyonfx::{Effect, EffectManager};

/// The motion engine that drives all TUI animations.
///
/// Owned by `App`, ticked once per render frame after all widgets have
/// rendered. Effects are post-applied to the composed buffer, so they
/// modify whatever the widgets just drew.
pub struct MotionSystem {
    manager: EffectManager<MotionKey>,
    sensitivity: MotionSensitivity,
    last_tick: Instant,
    last_process_us: u64,
}

impl MotionSystem {
    /// Create a new motion system at the given sensitivity level.
    #[must_use]
    pub fn new(sensitivity: MotionSensitivity) -> Self {
        Self {
            manager: EffectManager::default(),
            sensitivity,
            last_tick: Instant::now(),
            last_process_us: 0,
        }
    }

    /// Tick the motion system. Call once per render frame, after all widgets
    /// have rendered into the buffer. Returns the elapsed real time.
    pub fn tick(&mut self, buf: &mut Buffer, area: Rect) -> std::time::Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick);
        self.last_tick = now;

        if self.sensitivity == MotionSensitivity::Off {
            return elapsed;
        }

        let start = Instant::now();
        self.manager.process_effects(elapsed.into(), buf, area);
        #[allow(clippy::cast_possible_truncation)]
        {
            self.last_process_us = start.elapsed().as_micros() as u64;
        }

        elapsed
    }

    /// Fire a discrete effect, replacing any active effect with the same key.
    pub fn trigger(&mut self, key: MotionKey, effect: Effect) {
        if self.sensitivity == MotionSensitivity::Off {
            return;
        }
        self.manager.add_unique_effect(key, effect);
    }

    /// Add a non-keyed (stackable) effect.
    pub fn add(&mut self, effect: Effect) {
        if self.sensitivity == MotionSensitivity::Off {
            return;
        }
        self.manager.add_effect(effect);
    }

    /// Cancel an in-progress keyed effect.
    pub fn cancel(&mut self, key: MotionKey) {
        self.manager.cancel_unique_effect(key);
    }

    #[must_use]
    pub fn sensitivity(&self) -> MotionSensitivity {
        self.sensitivity
    }

    pub fn set_sensitivity(&mut self, s: MotionSensitivity) {
        self.sensitivity = s;
    }

    /// Cycle Off → Subtle → Full → Off.
    pub fn cycle_sensitivity(&mut self) {
        self.sensitivity = self.sensitivity.next();
    }

    /// Microseconds spent processing effects on the last tick.
    /// Used by the status bar / debug overlay.
    #[must_use]
    pub fn last_process_us(&self) -> u64 {
        self.last_process_us
    }

    /// True if any effects are currently in flight.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.manager.is_running()
    }
}

impl Default for MotionSystem {
    fn default() -> Self {
        Self::new(MotionSensitivity::Full)
    }
}
