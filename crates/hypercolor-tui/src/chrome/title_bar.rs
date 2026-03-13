//! Title bar — the topmost chrome row with stylized brand and status.
//!
//! Renders: `H Y P E R C O L O R` gradient brand on the left,
//! active screen name centered, and daemon status indicators right-aligned.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::screen::ScreenId;
use crate::state::{AppState, ConnectionStatus};
use crate::theme;

/// Animated title bar renderer with sine-wave shimmer on the brand text.
pub struct TitleBar {
    /// Animation phase in radians, advanced on each Tick.
    phase: f32,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

impl TitleBar {
    /// Create a new title bar with initial phase.
    #[must_use]
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }

    /// Advance the shimmer animation by one tick (~66ms at 15fps render).
    pub fn tick(&mut self) {
        self.phase += 0.12;
        if self.phase > std::f32::consts::TAU * 100.0 {
            self.phase -= std::f32::consts::TAU * 100.0;
        }
    }

    /// Render the title bar into the given area.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &AppState,
        active_screen: ScreenId,
        _available_screens: &[ScreenId],
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let mut spans = Vec::new();

        // Animated gradient brand
        spans.push(Span::raw(" "));
        build_gradient_brand(&mut spans, self.phase);

        // Separator + active screen name
        spans.push(Span::styled(
            " \u{2502} ",
            Style::default().fg(theme::text_muted()),
        ));
        spans.push(Span::styled(
            active_screen.full_name(),
            Style::default()
                .fg(theme::accent_secondary())
                .add_modifier(Modifier::BOLD),
        ));

        // Right-aligned status
        let right_spans = build_status_spans(state);
        let left_len: u16 = spans.iter().map(|s| s.width() as u16).sum();
        let right_len: u16 = right_spans.iter().map(|s| s.width() as u16).sum();
        let pad = area.width.saturating_sub(left_len + right_len);

        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.extend(right_spans);

        let line = Line::from(spans);
        let paragraph = ratatui::widgets::Paragraph::new(line)
            .style(Style::default().bg(theme::bg_panel()));

        frame.render_widget(paragraph, area);
    }
}

/// Build the right-aligned status spans: fps, audio, device count.
fn build_status_spans(state: &AppState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let muted = theme::text_muted();
    let primary = theme::text_primary();

    match state.connection_status {
        ConnectionStatus::Connected => {
            if let Some(ref daemon) = state.daemon {
                let fps_color = if daemon.fps_actual >= daemon.fps_target * 0.9 {
                    theme::success()
                } else if daemon.fps_actual >= daemon.fps_target * 0.5 {
                    theme::warning()
                } else {
                    theme::error()
                };

                spans.push(Span::styled(
                    format!("{:.0}fps", daemon.fps_actual),
                    Style::default().fg(fps_color),
                ));
                spans.push(Span::styled(" \u{2502} ", Style::default().fg(muted)));

                let audio_label = if state.spectrum.is_some() {
                    Span::styled("Audio", Style::default().fg(theme::success()))
                } else {
                    Span::styled("No Audio", Style::default().fg(muted))
                };
                spans.push(audio_label);
                spans.push(Span::styled(" \u{2502} ", Style::default().fg(muted)));

                spans.push(Span::styled(
                    format!("{} dev", daemon.device_count),
                    Style::default().fg(primary),
                ));
            }
        }
        ConnectionStatus::Connecting | ConnectionStatus::Reconnecting => {
            spans.push(Span::styled(
                "connecting\u{2026}",
                Style::default().fg(theme::warning()),
            ));
        }
        ConnectionStatus::Disconnected => {
            spans.push(Span::styled(
                "disconnected",
                Style::default().fg(theme::error()),
            ));
        }
    }

    spans.push(Span::raw(" "));
    spans
}

/// Render `H Y P E R C O L O R` with layered animated gradient effects.
///
/// Combines multiple animation layers for a dynamic, organic shimmer:
/// 1. Primary traveling wave — fast ripple across the gradient
/// 2. Secondary slow wave — different frequency adds depth
/// 3. Global color drift — the whole gradient breathes over time
/// 4. Traveling spark — a bright highlight rolls across periodically
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn build_gradient_brand(spans: &mut Vec<Span<'static>>, phase: f32) {
    const BRAND: &str = "HYPERCOLOR";
    let len = BRAND.len();
    let len_f = (len - 1).max(1) as f32;

    // Spark: a bright pulse traveling left-to-right (~5s period)
    let spark_pos = (phase * 1.8) % (len_f + 6.0) - 3.0;

    for (i, ch) in BRAND.chars().enumerate() {
        let i_f = i as f32;
        let base_t = i_f / len_f;

        // Layer 1: Primary traveling wave — fast, tight spacing
        let wave1 = (phase + i_f * 0.4).sin() * 0.25;
        // Layer 2: Secondary slow wave — different frequency, wider spacing
        let wave2 = (phase * 0.6 + i_f * 0.7).sin() * 0.15;
        // Layer 3: Global color drift — the whole gradient shifts slowly
        let drift = (phase * 0.03).sin() * 0.2;

        let t = (base_t + wave1 + wave2 + drift).clamp(0.0, 1.0);
        let base_color = theme::gradient_color(t, &theme::BRAND_GRADIENT);

        // Layer 4: Traveling spark — gaussian highlight bloom
        let spark_d = i_f - spark_pos;
        let spark = (-spark_d * spark_d * 0.5).exp();
        let color = if spark > 0.05 {
            brighten(base_color, spark * 0.7)
        } else {
            base_color
        };

        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD),
        ));
        if i < len - 1 {
            spans.push(Span::raw(" "));
        }
    }
}

/// Blend an RGB color toward white by `amount` (0.0 = unchanged, 1.0 = pure white).
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn brighten(color: ratatui::style::Color, amount: f32) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        Color::Rgb(r, g, b) => {
            let a = amount.clamp(0.0, 1.0);
            let lerp = |from: u8| (f32::from(from) + (255.0 - f32::from(from)) * a) as u8;
            Color::Rgb(lerp(r), lerp(g), lerp(b))
        }
        other => other,
    }
}
