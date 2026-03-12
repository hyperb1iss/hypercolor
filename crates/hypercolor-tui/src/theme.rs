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
