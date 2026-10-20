//! When the chrome's ambient hue moves.
//!
//! The shell publishes the live canvas's dominant hue as `--ambient-hue`
//! on the document element, where every `--ambient-*` token resolves it.
//! A write there restyles the whole document, so it happens on a slow,
//! fixed cadence and only for a change the eye could see. CSS interpolates
//! between published values (`--ambient-hue-drift` in
//! `tokens/semantic.css`), so the tint still drifts instead of stepping.

/// Least time between two published hues.
///
/// The tint is decorative chrome at single-digit alpha with no cadence of
/// its own to honor, unlike the preview, and the halo spends two seconds
/// drifting to each new value. One publish a second keeps that drift
/// continuous while capping document-wide restyles at one a second.
pub const AMBIENT_PUBLISH_INTERVAL_MS: f64 = 1_000.0;

/// Smallest hue change, in degrees, worth a publish.
///
/// The dominant hue of a steady effect wanders by a few degrees from one
/// sample to the next. Below this the tint cannot visibly change, and
/// publishing would restart the drift for nothing.
pub const AMBIENT_HUE_DEADBAND_DEG: f64 = 4.0;

/// Paces `--ambient-hue` writes and keeps the published value continuous.
///
/// Published values are unwrapped: a move from 350° to 10° publishes 370,
/// not 10, so CSS interpolating the number turns 20° the short way rather
/// than 340° the long way. OKLCH reads any angle modulo 360.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AmbientHuePublisher {
    published: Option<f64>,
    published_at_ms: f64,
}

impl AmbientHuePublisher {
    /// Offer a freshly measured hue in degrees. Returns the value to write
    /// as `--ambient-hue`, or `None` when the tint should stay put.
    pub fn offer(&mut self, hue_deg: f64, now_ms: f64) -> Option<f64> {
        if !hue_deg.is_finite() || !now_ms.is_finite() {
            return None;
        }
        let Some(published) = self.published else {
            let first = hue_deg.rem_euclid(360.0);
            self.published = Some(first);
            self.published_at_ms = now_ms;
            return Some(first);
        };
        if now_ms - self.published_at_ms < AMBIENT_PUBLISH_INTERVAL_MS {
            return None;
        }
        let delta = shortest_hue_delta(published, hue_deg);
        if delta.abs() < AMBIENT_HUE_DEADBAND_DEG {
            return None;
        }
        let next = published + delta;
        self.published = Some(next);
        self.published_at_ms = now_ms;
        Some(next)
    }

    /// The last value handed out, unwrapped.
    #[must_use]
    pub const fn published(&self) -> Option<f64> {
        self.published
    }
}

/// Signed turn from `from` to `to` along the shorter arc, in `(-180, 180]`.
#[must_use]
pub fn shortest_hue_delta(from: f64, to: f64) -> f64 {
    let delta = (to - from).rem_euclid(360.0);
    if delta > 180.0 { delta - 360.0 } else { delta }
}
