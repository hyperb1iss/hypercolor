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
pub mod reactive;
pub mod sensitivity;

pub use keys::MotionKey;
pub use reactive::{CanvasColorChannel, SpectrumChannel, sample_canvas_border};
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
    /// Lock-free shared spectrum state, written by the App on each
    /// `SpectrumUpdated` action and read by the spectrum pulse effect.
    spectrum: SpectrumChannel,
    /// Lock-free shared canvas color, written on each `CanvasFrameReceived`
    /// and read by the ambient bleed effect.
    canvas_color: CanvasColorChannel,
}

impl MotionSystem {
    /// Create a new motion system at the given sensitivity level.
    #[must_use]
    pub fn new(sensitivity: MotionSensitivity) -> Self {
        let spectrum = SpectrumChannel::new();
        let canvas_color = CanvasColorChannel::new();
        let mut manager = EffectManager::<MotionKey>::default();

        // Install reactive layers immediately. They're never_complete effects
        // that read from shared channels every frame, so it's safe to spawn
        // now and let App write into the channels as data arrives.
        if sensitivity != MotionSensitivity::Off {
            manager.add_unique_effect(
                MotionKey::SpectrumPulse,
                reactive::spectrum_border_pulse(spectrum.clone(), sensitivity),
            );
            manager.add_unique_effect(
                MotionKey::CanvasBleed,
                reactive::canvas_ambient_bleed(canvas_color.clone(), sensitivity),
            );
        }

        Self {
            manager,
            sensitivity,
            last_tick: Instant::now(),
            last_process_us: 0,
            spectrum,
            canvas_color,
        }
    }

    /// Get a clone of the spectrum channel for writing fresh snapshots.
    /// Each clone shares the same atomics — clone is cheap and safe.
    #[must_use]
    pub fn spectrum_channel(&self) -> SpectrumChannel {
        self.spectrum.clone()
    }

    /// Get a clone of the canvas color channel for writing fresh samples.
    #[must_use]
    pub fn canvas_color_channel(&self) -> CanvasColorChannel {
        self.canvas_color.clone()
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
        self.last_process_us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);

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
        self.refresh_reactive_layers();
    }

    /// Cycle Off → Subtle → Full → Off.
    pub fn cycle_sensitivity(&mut self) {
        self.sensitivity = self.sensitivity.next();
        self.refresh_reactive_layers();
    }

    /// Re-install reactive layers at the current sensitivity. Called when
    /// sensitivity changes — we need to rebuild because the captured
    /// sensitivity inside the effect closure won't update on its own.
    fn refresh_reactive_layers(&mut self) {
        if self.sensitivity == MotionSensitivity::Off {
            self.manager.cancel_unique_effect(MotionKey::SpectrumPulse);
            self.manager.cancel_unique_effect(MotionKey::CanvasBleed);
        } else {
            self.manager.add_unique_effect(
                MotionKey::SpectrumPulse,
                reactive::spectrum_border_pulse(self.spectrum.clone(), self.sensitivity),
            );
            self.manager.add_unique_effect(
                MotionKey::CanvasBleed,
                reactive::canvas_ambient_bleed(self.canvas_color.clone(), self.sensitivity),
            );
        }
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
