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

/// Lock-free shared canvas dominant-color state. Single-writer
/// (App on each `CanvasFrameReceived`), single-reader (the bleed effect).
///
/// Stores a single packed RGB value plus a "valid" flag (top byte).
/// Layout: `0x00RRGGBB` when valid, `0` when no data.
#[derive(Debug, Clone, Default)]
pub struct CanvasColorChannel {
    packed: Arc<AtomicU32>,
}

impl CanvasColorChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write the most recent dominant color sampled from the canvas.
    pub fn write(&self, r: u8, g: u8, b: u8) {
        let packed = 0x0100_0000_u32 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        self.packed.store(packed, Ordering::Relaxed);
    }

    /// Read the latest sampled color, if any data has been written yet.
    pub fn read(&self) -> Option<(u8, u8, u8)> {
        let packed = self.packed.load(Ordering::Relaxed);
        if packed >> 24 == 0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let r = ((packed >> 16) & 0xFF) as u8;
        #[allow(clippy::cast_possible_truncation)]
        let g = ((packed >> 8) & 0xFF) as u8;
        #[allow(clippy::cast_possible_truncation)]
        let b = (packed & 0xFF) as u8;
        Some((r, g, b))
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
                    cell.set_fg(Color::Rgb(bumped_r as u8, bumped_g as u8, bumped_b as u8));
                }
            });
        },
    ))
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}

// ── Canvas ambient bleed ─────────────────────────────────────────────────

/// State carried by the canvas bleed closure each frame.
#[derive(Debug, Clone)]
struct BleedState {
    channel: CanvasColorChannel,
    sensitivity: MotionSensitivity,
}

/// Build the canvas ambient bleed effect — Ambilight for the chrome.
///
/// Reads the dominant canvas color each frame and applies it as a subtle
/// background tint to all cells. Sensitivity controls the maximum tint
/// strength so the chrome stays readable.
pub fn canvas_ambient_bleed(channel: CanvasColorChannel, sensitivity: MotionSensitivity) -> Effect {
    let state = BleedState {
        channel,
        sensitivity,
    };

    fx::never_complete(fx::effect_fn(
        state,
        EffectTimer::from_ms(16, Interpolation::Linear),
        |state, _ctx, cell_iter| {
            let Some((r, g, b)) = state.channel.read() else {
                return;
            };

            // Subtle is the ceiling — we never want to overpower the chrome.
            // Sensitivity Subtle = 0.04, Full = 0.08 of the canvas color.
            let strength = 0.08 * state.sensitivity.amplitude();
            if strength < 0.005 {
                return;
            }

            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let tint_r = (f32::from(r) * strength) as u8;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let tint_g = (f32::from(g) * strength) as u8;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let tint_b = (f32::from(b) * strength) as u8;

            cell_iter.into_iter().for_each(|(_pos, cell)| {
                // Blend the tint into the existing background, additive.
                if let Color::Rgb(br, bg, bb) = cell.bg {
                    cell.set_bg(Color::Rgb(
                        br.saturating_add(tint_r),
                        bg.saturating_add(tint_g),
                        bb.saturating_add(tint_b),
                    ));
                } else if cell.bg == Color::Reset {
                    // Default background — apply tint over near-black
                    cell.set_bg(Color::Rgb(tint_r, tint_g, tint_b));
                }
            });
        },
    ))
}

// ── Canvas color sampling ────────────────────────────────────────────────

/// Compute the average border color of an RGB canvas frame.
///
/// Samples the four edges (top, bottom, left, right) of the canvas and
/// averages them — Ambilight-style edge detection. Returns `None` for empty
/// or malformed frames. Pixels are 3 bytes each (R, G, B).
pub fn sample_canvas_border(width: u16, height: u16, pixels: &[u8]) -> Option<(u8, u8, u8)> {
    if width == 0 || height == 0 {
        return None;
    }
    let w = usize::from(width);
    let h = usize::from(height);
    let stride = w * 3;
    if pixels.len() < stride * h {
        return None;
    }

    let mut sum_r: u64 = 0;
    let mut sum_g: u64 = 0;
    let mut sum_b: u64 = 0;
    let mut count: u64 = 0;

    // Top + bottom rows
    for y in [0_usize, h - 1] {
        let row = &pixels[y * stride..y * stride + stride];
        for px in row.chunks_exact(3) {
            sum_r += u64::from(px[0]);
            sum_g += u64::from(px[1]);
            sum_b += u64::from(px[2]);
            count += 1;
        }
    }

    // Left + right columns (skip corners already counted)
    for y in 1..h.saturating_sub(1) {
        let row_start = y * stride;
        // Left
        sum_r += u64::from(pixels[row_start]);
        sum_g += u64::from(pixels[row_start + 1]);
        sum_b += u64::from(pixels[row_start + 2]);
        // Right
        sum_r += u64::from(pixels[row_start + stride - 3]);
        sum_g += u64::from(pixels[row_start + stride - 2]);
        sum_b += u64::from(pixels[row_start + stride - 1]);
        count += 2;
    }

    if count == 0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some((
        (sum_r / count) as u8,
        (sum_g / count) as u8,
        (sum_b / count) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_returns_none_for_empty() {
        assert_eq!(sample_canvas_border(0, 0, &[]), None);
        assert_eq!(sample_canvas_border(2, 2, &[]), None);
    }

    #[test]
    fn sample_solid_red_2x2() {
        let red = vec![255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0];
        assert_eq!(sample_canvas_border(2, 2, &red), Some((255, 0, 0)));
    }

    #[test]
    fn sample_averages_edges_only() {
        // 3x3 canvas with red border, green center
        // top row: R R R, middle row: R G R, bottom row: R R R
        #[rustfmt::skip]
        let pixels = vec![
            255, 0, 0,  255, 0, 0,  255, 0, 0,
            255, 0, 0,    0,255,0,  255, 0, 0,
            255, 0, 0,  255, 0, 0,  255, 0, 0,
        ];
        // 8 border pixels all red — center green is ignored
        assert_eq!(sample_canvas_border(3, 3, &pixels), Some((255, 0, 0)));
    }

    #[test]
    fn canvas_color_channel_round_trip() {
        let chan = CanvasColorChannel::new();
        assert_eq!(chan.read(), None);
        chan.write(225, 53, 255);
        assert_eq!(chan.read(), Some((225, 53, 255)));
    }

    #[test]
    fn spectrum_channel_round_trip() {
        let chan = SpectrumChannel::new();
        chan.write(0.42, 0.7);
        assert!((chan.bass() - 0.42).abs() < 1e-6);
        assert!((chan.level() - 0.7).abs() < 1e-6);
    }
}
