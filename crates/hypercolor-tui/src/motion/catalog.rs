//! Effect catalog — concrete tachyonfx compositions per Spec 38 §6.
//!
//! Each constructor returns a fully-configured `Effect` ready to be added
//! to the `MotionSystem`. Sensitivity scaling is the caller's responsibility:
//! the catalog produces full-amplitude effects, and `MotionSystem` filters
//! them based on its current sensitivity setting before adding.

use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use tachyonfx::{CellFilter, Effect, EffectTimer, Interpolation, Motion, fx};

use super::sensitivity::MotionSensitivity;

// ── Brand gradient stops (matching theme::BRAND_GRADIENT) ───────────────

const BRAND_PURPLE: (u8, u8, u8) = (225, 53, 255);
const BRAND_CORAL: (u8, u8, u8) = (255, 106, 193);
const BRAND_CYAN: (u8, u8, u8) = (128, 255, 234);

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn brand_gradient(t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let stops = [BRAND_PURPLE, BRAND_CORAL, BRAND_CYAN];
    let scaled = t * 2.0;
    let idx = (scaled as usize).min(1);
    let frac = scaled - idx as f32;
    let (r1, g1, b1) = stops[idx];
    let (r2, g2, b2) = stops[idx + 1];
    let lerp = |a: u8, b: u8, f: f32| {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * f) as u8
    };
    (lerp(r1, r2, frac), lerp(g1, g2, frac), lerp(b1, b2, frac))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn brighten(rgb: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let a = amount.clamp(0.0, 1.0);
    let lerp = |from: u8| (f32::from(from) + (255.0 - f32::from(from)) * a) as u8;
    (lerp(rgb.0), lerp(rgb.1), lerp(rgb.2))
}

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
        fx::fade_to_fg(
            ERROR_RED,
            (scale_ms(300, sensitivity), Interpolation::Linear),
        ),
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
    fx::never_complete(fx::fade_to_fg(ERROR_RED, (in_ms, Interpolation::SineInOut)))
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

/// Slow breathing effect for idle borders.
///
/// Spec 38 §6.11 — gentle ping-pong lighten/darken on border cells with
/// a 3-second period. Never completes, runs as long as the user is idle.
pub fn idle_breathing(area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let half_period_ms = scale_ms(3000, sensitivity);
    let amplitude = 0.05 * sensitivity.amplitude();
    fx::never_complete(fx::repeating(fx::ping_pong(fx::lighten_fg(
        amplitude,
        (half_period_ms, Interpolation::SineInOut),
    ))))
    .with_area(area)
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}

// ── Title bar shimmer (Spec 38 §6.x — chrome migration) ─────────────────

/// State for the title bar shimmer effect.
#[derive(Debug, Clone, Default)]
struct ShimmerState {
    phase: f32,
}

/// Animated brand-gradient shimmer for the title bar.
///
/// Replaces the hand-rolled `TitleBar::tick()` phase advancement with a
/// `never_complete` effect_fn that runs continuously and modulates each
/// brand cell's foreground color through four animation layers:
///   1. Base position-based gradient (Purple → Coral → Cyan)
///   2. Primary fast traveling wave (sin)
///   3. Secondary slow wave at different frequency
///   4. Global drift (very slow)
///   + Traveling spark — gaussian highlight rolling across the brand
///
/// The effect operates over the brand area only (CellFilter::Text excludes
/// the inter-character spaces). Time-based phase advancement uses
/// `last_tick` so it stays smooth at any render rate.
pub fn title_shimmer(brand_area: Rect, sensitivity: MotionSensitivity) -> Effect {
    let amplitude = sensitivity.amplitude();
    if amplitude < 0.01 {
        // Sensitivity::Off — return a no-op that consumes a tick and exits
        return fx::consume_tick();
    }

    fx::never_complete(fx::effect_fn(
        ShimmerState::default(),
        EffectTimer::from_ms(16, Interpolation::Linear),
        move |state, ctx, cell_iter| {
            // Advance phase at ~1.82 rad/sec to match the legacy
            // 0.12 rad / 66ms cadence.
            state.phase += ctx.last_tick.as_secs_f32() * 1.82;
            let phase = state.phase;

            // Spark position: bright bloom rolling across the brand width.
            let len_f = 9.0_f32; // 10 characters - 1
            let spark_pos = (phase * 1.8).rem_euclid(len_f + 6.0) - 3.0;

            let area = ctx.area;
            cell_iter.into_iter().for_each(|(pos, cell)| {
                // Skip whitespace cells (the spaces between characters)
                if cell.symbol().chars().all(char::is_whitespace) {
                    return;
                }

                // Character index from x position. Brand text is laid out
                // as "H Y P E R C O L O R" — letters at even offsets,
                // spaces at odd offsets.
                #[allow(clippy::cast_precision_loss)]
                let i_f = f32::from(pos.x.saturating_sub(area.x)) / 2.0;
                let base_t = i_f / len_f;

                // Layer 1: primary wave
                let wave1 = (phase + i_f * 0.4).sin() * 0.25;
                // Layer 2: secondary slow wave
                let wave2 = (phase * 0.6 + i_f * 0.7).sin() * 0.15;
                // Layer 3: global drift
                let drift = (phase * 0.03).sin() * 0.2;

                let t = (base_t + wave1 * amplitude + wave2 * amplitude + drift * amplitude)
                    .clamp(0.0, 1.0);
                let mut rgb = brand_gradient(t);

                // Layer 4: traveling spark
                let spark_d = i_f - spark_pos;
                let spark = (-spark_d * spark_d * 0.5).exp();
                if spark > 0.05 {
                    rgb = brighten(rgb, spark * 0.7 * amplitude);
                }

                cell.set_fg(Color::Rgb(rgb.0, rgb.1, rgb.2));
            });
        },
    ))
    .with_area(brand_area)
}
