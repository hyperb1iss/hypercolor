//! When the chrome's ambient hue moves.
//!
//! App-owned hue state feeds only the chrome that paints it. Consumers bind
//! the non-inheriting `--ambient-hue` property locally; the document and
//! unrelated descendants never receive a hue write. CSS interpolates the
//! halo's `--ambient-hue-drift`, so its tint drifts instead of stepping.

use hypercolor_leptos_ext::prelude::now_ms;
use leptos::prelude::*;

use crate::color::CanvasFrameAnalysis;

/// Lives above route shells so remounting a consumer retains the unwrapped hue.
#[derive(Clone, Copy)]
struct AmbientContext(ReadSignal<f64>);

pub(crate) fn provide_ambient_context(analysis: ReadSignal<Option<CanvasFrameAnalysis>>) {
    let (hue, set_hue) = signal(AMBIENT_HUE_FALLBACK_DEG);
    let publisher = StoredValue::new(AmbientHuePublisher::default());
    Effect::new(move |_| {
        let Some(analysis) = analysis.get() else {
            return;
        };
        if let Some(next) = publisher
            .try_update_value(|publisher| publisher.offer(analysis.dominant_hue, now_ms()))
            .flatten()
        {
            set_hue.set(next);
        }
    });
    provide_context(AmbientContext(hue));
}

/// Bind this to `style:--ambient-hue` on a halo or `.ambient-scroll` owner.
///
/// Extension-owned scroll surfaces use the same app hue without inheriting
/// dynamic values through their descendants. Create the binding in the
/// component's owner; outside the app it uses the static purple fallback.
pub fn hue_style() -> impl Fn() -> String + Copy {
    let ambient = use_context::<AmbientContext>();
    move || {
        let hue = ambient.map_or(AMBIENT_HUE_FALLBACK_DEG, |ambient| ambient.0.get());
        format!("{hue:.1}")
    }
}

/// Least time between two published hues.
///
/// The tint is decorative chrome at single-digit alpha with no cadence of
/// its own to honor, unlike the preview, and the halo spends two seconds
/// drifting to each new value. One publish a second keeps that drift
/// continuous without restarting it on every sampled frame.
pub const AMBIENT_PUBLISH_INTERVAL_MS: f64 = 1_000.0;

/// Smallest hue change, in degrees, worth a publish.
///
/// The dominant hue of a steady effect wanders by a few degrees from one
/// sample to the next. Below this the tint cannot visibly change, and
/// publishing would restart the drift for nothing.
pub const AMBIENT_HUE_DEADBAND_DEG: f64 = 4.0;

/// The hue `tokens/semantic.css` declares before anything is published.
pub const AMBIENT_HUE_FALLBACK_DEG: f64 = 320.0;

/// Paces `--ambient-hue` writes and keeps the published value continuous.
///
/// Published values are unwrapped: a move from 350° to 10° publishes 370,
/// not 10, so CSS interpolating the number turns 20° the short way rather
/// than 340° the long way. OKLCH reads any angle modulo 360. The first
/// publish unwraps against the value CSS already holds for the same reason.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmbientHuePublisher {
    published: f64,
    published_at_ms: f64,
}

impl Default for AmbientHuePublisher {
    fn default() -> Self {
        Self::starting_at(AMBIENT_HUE_FALLBACK_DEG)
    }
}

impl AmbientHuePublisher {
    /// A publisher continuing from `hue_deg`, the value `--ambient-hue`
    /// holds now. Its first move is not held back by the interval.
    #[must_use]
    pub fn starting_at(hue_deg: f64) -> Self {
        Self {
            published: if hue_deg.is_finite() {
                hue_deg
            } else {
                AMBIENT_HUE_FALLBACK_DEG
            },
            published_at_ms: f64::NEG_INFINITY,
        }
    }

    /// Offer a freshly measured hue in degrees. Returns the value to write
    /// as `--ambient-hue`, or `None` when the tint should stay put.
    pub fn offer(&mut self, hue_deg: f64, now_ms: f64) -> Option<f64> {
        if !hue_deg.is_finite() || !now_ms.is_finite() {
            return None;
        }
        if now_ms - self.published_at_ms < AMBIENT_PUBLISH_INTERVAL_MS {
            return None;
        }
        let delta = shortest_hue_delta(self.published, hue_deg);
        if delta.abs() < AMBIENT_HUE_DEADBAND_DEG {
            return None;
        }
        self.published += delta;
        self.published_at_ms = now_ms;
        Some(self.published)
    }

    /// The value `--ambient-hue` holds, unwrapped.
    #[must_use]
    pub const fn published(&self) -> f64 {
        self.published
    }
}

/// Signed turn from `from` to `to` along the shorter arc, in `(-180, 180]`.
#[must_use]
pub fn shortest_hue_delta(from: f64, to: f64) -> f64 {
    let delta = (to - from).rem_euclid(360.0);
    if delta > 180.0 { delta - 360.0 } else { delta }
}
