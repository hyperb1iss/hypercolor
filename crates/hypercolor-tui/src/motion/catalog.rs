//! Effect catalog — concrete tachyonfx compositions per Spec 38 §6.
//!
//! Each constructor returns a fully-configured `Effect` ready to be added
//! to the `MotionSystem`. Sensitivity scaling is the caller's responsibility:
//! the catalog produces full-amplitude effects, and `MotionSystem` filters
//! them based on its current sensitivity setting before adding.

use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use tachyonfx::{CellFilter, Effect, Interpolation, Motion, fx};

use super::sensitivity::MotionSensitivity;

// ── Color tokens (matching theme.rs constants) ──────────────────────────

const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const SUCCESS_GREEN: Color = Color::Rgb(80, 250, 123);
const ERROR_RED: Color = Color::Rgb(255, 99, 99);
const WARNING_YELLOW: Color = Color::Rgb(241, 250, 140);

// ── Helpers ─────────────────────────────────────────────────────────────

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scale_ms(ms: u32, sensitivity: MotionSensitivity) -> u32 {
    let scaled = (ms as f32) * sensitivity.duration_scale();
    scaled.max(1.0) as u32
}

// ── Discrete event effects ──────────────────────────────────────────────

/// Sweep-in animation for a newly connected device row.
///
/// Spec 38 §6.1 — neon cyan leading edge, fades to row's final colors.
pub fn device_arrival(row_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let total_ms = scale_ms(600, sensitivity);
    fx::sweep_in(
        Motion::LeftToRight,
        12,
        0,
        NEON_CYAN,
        (total_ms, Interpolation::SineOut),
    )
    .with_area(row_area)
}

/// Dissolve-out for a departing device.
///
/// Spec 38 §6.2 — dissolves with red tint as cells disappear.
pub fn device_departure(row_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let total_ms = scale_ms(400, sensitivity);
    fx::parallel(&[
        fx::dissolve((total_ms, Interpolation::ExpoOut)),
        fx::fade_to_fg(ERROR_RED, (scale_ms(300, sensitivity), Interpolation::Linear)),
    ])
    .with_area(row_area)
}

/// Crossfade sweep over the canvas preview when the active effect changes.
///
/// Spec 38 §6.3 — left-to-right wipe revealing the new effect's frames as
/// they arrive from the daemon.
pub fn effect_transition(preview_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let total_ms = scale_ms(500, sensitivity);
    fx::sweep_in(
        Motion::LeftToRight,
        8,
        0,
        ELECTRIC_PURPLE,
        (total_ms, Interpolation::CircInOut),
    )
    .with_area(preview_area)
}

/// Brief brightness pulse on a slider when its control value changes.
///
/// Spec 38 §6.4 — confirms the daemon accepted the patch.
pub fn control_patch(slider_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let in_ms = scale_ms(100, sensitivity);
    let out_ms = scale_ms(150, sensitivity);
    fx::sequence(&[
        fx::lighten_fg(0.3, (in_ms, Interpolation::QuadOut)),
        fx::darken_fg(0.3, (out_ms, Interpolation::QuadIn)),
    ])
    .with_area(slider_area)
    .with_filter(CellFilter::Text)
}

/// Quick red flash for errors.
///
/// Spec 38 §6.10 — fast attention-grab without persisting.
pub fn error_flash(area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let in_ms = scale_ms(150, sensitivity);
    let out_ms = scale_ms(250, sensitivity);
    fx::sequence(&[
        fx::fade_to_fg(ERROR_RED, (in_ms, Interpolation::QuadOut)),
        fx::fade_from_fg(ERROR_RED, (out_ms, Interpolation::CubicIn)),
    ])
    .with_area(area)
}

/// Green flash on connection restored.
///
/// Spec 38 §6.9 — single sweep across borders confirming the link is back.
pub fn connection_restored(area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let in_ms = scale_ms(200, sensitivity);
    let out_ms = scale_ms(300, sensitivity);
    fx::sequence(&[
        fx::fade_to_fg(SUCCESS_GREEN, (in_ms, Interpolation::QuadOut)),
        fx::fade_from_fg(SUCCESS_GREEN, (out_ms, Interpolation::QuadIn)),
    ])
    .with_area(area)
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}

/// Red border tint that persists until connection is restored.
///
/// Spec 38 §6.8 — simpler than the full glitch+HSL spec; just a fade to
/// red on borders. Phase 5 polish can swap in the full glitch effect.
pub fn connection_lost(area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let in_ms = scale_ms(400, sensitivity);
    fx::never_complete(
        fx::fade_to_fg(ERROR_RED, (in_ms, Interpolation::SineInOut))
    )
    .with_area(area)
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}

/// Border glow when keyboard focus moves to a new panel.
///
/// Spec 38 §6.7 — subtle accent fade on the focused panel's border cells.
pub fn panel_focus(panel_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let total_ms = scale_ms(300, sensitivity);
    fx::fade_to_fg(NEON_CYAN, (total_ms, Interpolation::CubicOut))
        .with_area(panel_area)
        .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}

/// Dissolve + coalesce when switching screens.
///
/// Spec 38 §6.6 — approximation of an offscreen-buffer crossfade since
/// ratatui doesn't have true component-level offscreen rendering.
pub fn screen_transition(content_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let half = scale_ms(150, sensitivity);
    fx::sequence(&[
        fx::dissolve((half, Interpolation::QuadIn)),
        fx::coalesce((half, Interpolation::QuadOut)),
    ])
    .with_area(content_area)
}

/// Sleep-aware notification toast slide.
///
/// Spec 38 §6.12 — sweep-in entry, dissolve exit. The dismiss is handled
/// elsewhere; this just animates the entry.
pub fn notification_entry(toast_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let total_ms = scale_ms(300, sensitivity);
    fx::sweep_in(
        Motion::RightToLeft,
        8,
        0,
        WARNING_YELLOW,
        (total_ms, Interpolation::CubicOut),
    )
    .with_area(toast_area)
}
