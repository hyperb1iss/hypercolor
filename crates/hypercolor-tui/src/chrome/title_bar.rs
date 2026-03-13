//! Title bar — the topmost chrome row with integrated navigation.
//!
//! Renders: `HYPERCOLOR  [D]ash [E]ffx [C]trl De[v]s [P]rof [S]ttg`
//! on the left, with right-aligned daemon status indicators: FPS, audio
//! status, and device count.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::screen::ScreenId;
use crate::state::{AppState, ConnectionStatus};
use crate::theme;

/// Stateless title bar renderer (includes inline nav tabs).
pub struct TitleBar;

impl TitleBar {
    /// Render the title bar into the given area.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &AppState,
        active_screen: ScreenId,
        available_screens: &[ScreenId],
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let mut spans = Vec::new();

        // Gradient brand: Electric Purple → Neon Cyan
        build_gradient_brand(&mut spans);
        spans.push(Span::styled(
            " \u{2502} ",
            Style::default().fg(theme::text_muted()),
        ));

        // Inline nav tabs
        for (i, &screen) in available_screens.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            build_nav_tab(&mut spans, screen, screen == active_screen);
        }

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

/// Append spans for a single nav tab: `[D]ash` style.
fn build_nav_tab(spans: &mut Vec<Span<'static>>, screen: ScreenId, is_active: bool) {
    let key = screen.key_hint();
    let label = screen.label();

    let (key_style, label_style) = if is_active {
        (
            Style::default()
                .fg(theme::accent_secondary())
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::accent_primary())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(theme::warning()),
            Style::default().fg(theme::text_muted()),
        )
    };

    let key_lower = key.to_ascii_lowercase();
    let key_upper = key.to_ascii_uppercase();
    let bracket_style = Style::default().fg(theme::text_muted());

    if let Some(pos) = label.find(key_upper).or_else(|| label.find(key_lower)) {
        let before = &label[..pos];
        let after = &label[pos + key.len_utf8()..];
        spans.push(Span::styled(before.to_string(), label_style));
        spans.push(Span::styled("[", bracket_style));
        spans.push(Span::styled(key.to_string(), key_style));
        spans.push(Span::styled("]", bracket_style));
        spans.push(Span::styled(after.to_string(), label_style));
    } else {
        spans.push(Span::styled("[", bracket_style));
        spans.push(Span::styled(key.to_string(), key_style));
        spans.push(Span::styled("]", bracket_style));
        spans.push(Span::styled(label.to_string(), label_style));
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

/// Render "HYPERCOLOR" with a per-character gradient (Electric Purple → Coral → Neon Cyan).
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn build_gradient_brand(spans: &mut Vec<Span<'static>>) {
    const BRAND: &str = "HYPERCOLOR";
    let len = BRAND.len();
    for (i, ch) in BRAND.chars().enumerate() {
        let t = i as f32 / (len - 1).max(1) as f32;
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(theme::gradient_color(t, &theme::BRAND_GRADIENT))
                .add_modifier(Modifier::BOLD),
        ));
    }
}
