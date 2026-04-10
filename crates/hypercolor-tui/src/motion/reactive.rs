//! Reactive layers — continuous effects driven by live daemon data.
//!
//! Unlike the discrete catalog effects, reactive layers persist indefinitely
//! and read live state from shared atomics that the App updates as new
//! WebSocket data arrives. They use `fx::never_complete` so they never
//! self-terminate.
//!
//! ## Spectrum border pulse (Spec 38 §7.1)
//!
//! Border cell brightness modulates with audio bass energy. Sub-bass and bass
//! values come from the daemon's existing `SpectrumSnapshot` (already extracted
//! from FFT bins, so no client-side processing needed). When the music hits
//! hard, borders bloom; when it's quiet, they sit at the theme default.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ratatui::layout::Margin;
use ratatui::style::Color;
use tachyonfx::{CellFilter, Effect, EffectTimer, Interpolation, fx};

use super::sensitivity::MotionSensitivity;

// ── Shared state plumbing ────────────────────────────────────────────────

/// Lock-free shared spectrum state. Cloned into the reactive layer's closure
/// and updated by the App on every `SpectrumUpdated` action.
///
/// `f32` is encoded as `u32` bits for atomic storage. Single-writer
/// (App's action handler) and single-reader (the effect closure).
#[derive(Debug, Clone, Default)]
pub struct SpectrumChannel {
    bass: Arc<AtomicU32>,
    level: Arc<AtomicU32>,
}

impl SpectrumChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write the latest spectrum snapshot. Called from the action handler.
    pub fn write(&self, bass: f32, level: f32) {
        self.bass.store(bass.to_bits(), Ordering::Relaxed);
        self.level.store(level.to_bits(), Ordering::Relaxed);
    }

    /// Read the most recent bass energy (0.0..=1.0 typical).
    pub fn bass(&self) -> f32 {
        f32::from_bits(self.bass.load(Ordering::Relaxed))
    }

    /// Read the most recent overall level (0.0..=1.0 typical).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

// ── Spectrum border pulse effect ─────────────────────────────────────────

/// State carried by the spectrum pulse closure each frame.
#[derive(Debug, Clone)]
struct PulseState {
    channel: SpectrumChannel,
    sensitivity: MotionSensitivity,
}

/// Build the spectrum border pulse effect.
///
/// Reads bass energy from `channel` each frame and brightens border cells
/// proportionally. Caps at +30% lightness so it never blows out. Sensitivity
/// scales the maximum boost.
pub fn spectrum_border_pulse(channel: SpectrumChannel, sensitivity: MotionSensitivity) -> Effect {
    let state = PulseState {
        channel,
        sensitivity,
    };

    fx::never_complete(fx::effect_fn(
        state,
        EffectTimer::from_ms(16, Interpolation::Linear),
        |state, _ctx, cell_iter| {
            let bass = state.channel.bass().clamp(0.0, 1.0);
            let max_boost = 0.30 * state.sensitivity.amplitude();
            let boost = bass * max_boost;

            if boost < 0.01 {
                return; // Quiet — leave borders alone
            }

            cell_iter.into_iter().for_each(|(_pos, cell)| {
                if let Color::Rgb(r, g, b) = cell.fg {
                    let factor = 1.0 + boost;
                    let bumped_r = (f32::from(r) * factor).min(255.0);
                    let bumped_g = (f32::from(g) * factor).min(255.0);
                    let bumped_b = (f32::from(b) * factor).min(255.0);
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        clippy::as_conversions
                    )]
                    cell.set_fg(Color::Rgb(
                        bumped_r as u8,
                        bumped_g as u8,
                        bumped_b as u8,
                    ));
                }
            });
        },
    ))
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}
