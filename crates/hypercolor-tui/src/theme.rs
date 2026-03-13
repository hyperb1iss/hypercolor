//! Theme accessors for the Hypercolor TUI.
//!
//! Wraps `opaline` global theme state, providing strongly-typed helpers that
//! return `ratatui::style::Color` and `ratatui::style::Style` values for the
//! `SilkCircuit` Neon palette tokens.

use ratatui::style::{Color, Modifier, Style};

/// Initialize the global Opaline theme.
///
/// If `theme_name` is `Some`, loads that theme by name; otherwise defaults to
/// `"silkcircuit-neon"`.
pub fn initialize(theme_name: Option<&str>) {
    let name = theme_name.unwrap_or("silkcircuit-neon");
    if let Err(e) = opaline::load_theme_by_name(name) {
        tracing::warn!("Failed to load theme {name:?}: {e}, using default");
    }
}

// ── Color accessors ─────────────────────────────────────────────────────────

fn color_from_token(token: &str) -> Color {
    let theme = opaline::current();
    let c = theme.color(token);
    Color::Rgb(c.r, c.g, c.b)
}

/// Primary accent — Electric Purple `#e135ff`.
#[must_use]
pub fn accent_primary() -> Color {
    color_from_token("accent.primary")
}

/// Secondary accent — Neon Cyan `#80ffea`.
#[must_use]
pub fn accent_secondary() -> Color {
    color_from_token("accent.secondary")
}

/// Primary text — near-white `#f8f8f2`.
#[must_use]
pub fn text_primary() -> Color {
    color_from_token("text.primary")
}

/// Muted text — medium gray `#82879f`.
#[must_use]
pub fn text_muted() -> Color {
    color_from_token("text.muted")
}

/// Base background — near-black `#121218`.
#[must_use]
pub fn bg_base() -> Color {
    color_from_token("bg.base")
}

/// Panel background — slightly lighter `#181820`.
#[must_use]
pub fn bg_panel() -> Color {
    color_from_token("bg.panel")
}

/// Highlight background — for selection rows `#37324b`.
#[must_use]
pub fn bg_highlight() -> Color {
    color_from_token("bg.highlight")
}

/// Success green `#50fa7b`.
#[must_use]
pub fn success() -> Color {
    color_from_token("success")
}

/// Error red `#ff6363`.
#[must_use]
pub fn error() -> Color {
    color_from_token("error")
}

/// Warning yellow `#f1fa8c`.
#[must_use]
pub fn warning() -> Color {
    color_from_token("warning")
}

// ── Spectrum band colors ────────────────────────────────────────────────────

/// Bass spectrum color — Coral `#ff6ac1`.
#[must_use]
pub fn spectrum_bass() -> Color {
    color_from_token("accent.tertiary")
}

/// Mid spectrum color — Electric Yellow `#f1fa8c`.
#[must_use]
pub fn spectrum_mid() -> Color {
    color_from_token("warning")
}

/// Treble spectrum color — Neon Cyan `#80ffea`.
#[must_use]
pub fn spectrum_treble() -> Color {
    color_from_token("accent.secondary")
}

// ── Style accessors ─────────────────────────────────────────────────────────

/// Style for focused borders — Neon Cyan fg.
#[must_use]
pub fn border_focused() -> Style {
    Style::default().fg(accent_secondary())
}

/// Style for unfocused borders — muted fg.
#[must_use]
pub fn border_unfocused() -> Style {
    Style::default().fg(text_muted())
}

/// Title style — accent primary, bold.
#[must_use]
pub fn title_style() -> Style {
    Style::default()
        .fg(accent_primary())
        .add_modifier(Modifier::BOLD)
}

// ── Gradient utilities ─────────────────────────────────────────────────────

/// Linearly interpolate through a sequence of RGB color stops at position `t` (0.0..1.0).
#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn gradient_color(t: f32, stops: &[(u8, u8, u8)]) -> Color {
    let t = t.clamp(0.0, 1.0);
    let segments = (stops.len() - 1).max(1);
    let scaled = t * segments as f32;
    let idx = (scaled as usize).min(segments - 1);
    let frac = scaled - idx as f32;
    let (r1, g1, b1) = stops[idx];
    let (r2, g2, b2) = stops[(idx + 1).min(stops.len() - 1)];
    let lerp =
        |a: u8, b: u8, f: f32| -> u8 { (f32::from(a) + (f32::from(b) - f32::from(a)) * f) as u8 };
    Color::Rgb(lerp(r1, r2, frac), lerp(g1, g2, frac), lerp(b1, b2, frac))
}

/// Brand gradient stops: Electric Purple → Coral → Neon Cyan.
pub const BRAND_GRADIENT: [(u8, u8, u8); 3] = [(225, 53, 255), (255, 106, 193), (128, 255, 234)];

/// Spectrum gradient stops: Coral (bass) → Electric Yellow (mid) → Neon Cyan (treble).
pub const SPECTRUM_GRADIENT: [(u8, u8, u8); 3] =
    [(255, 106, 193), (241, 250, 140), (128, 255, 234)];

/// Effect name gradient stops: Neon Cyan → Electric Purple.
pub const EFFECT_GRADIENT: [(u8, u8, u8); 2] = [(128, 255, 234), (225, 53, 255)];
