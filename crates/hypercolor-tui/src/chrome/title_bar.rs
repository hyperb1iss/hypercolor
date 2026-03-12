//! Title bar — the topmost chrome row.
//!
//! Renders: `HYPERCOLOR` (bold accent) on the left, with right-aligned
//! daemon status indicators: FPS, audio status, and device count.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::state::{AppState, ConnectionStatus};
use crate::theme;

/// Stateless title bar renderer.
pub struct TitleBar;

impl TitleBar {
    /// Render the title bar into the given area.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    pub fn render(&self, frame: &mut Frame, area: Rect, state: &AppState) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let title_span = Span::styled(
            "HYPERCOLOR",
            Style::default()
                .fg(theme::accent_primary())
                .add_modifier(Modifier::BOLD),
        );

        let right_spans = build_status_spans(state);

        // Calculate how much padding we need between left and right.
        let left_len: u16 = 10; // "HYPERCOLOR"
        let right_len: u16 = right_spans.iter().map(|s| s.width() as u16).sum();
        let pad = area.width.saturating_sub(left_len + right_len);

        let mut spans = vec![title_span];
        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.extend(right_spans);

        let line = Line::from(spans);
        let paragraph = ratatui::widgets::Paragraph::new(line)
            .alignment(Alignment::Left)
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
            // FPS indicator
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
                spans.push(Span::styled(" | ", Style::default().fg(muted)));

                // Audio status
                let audio_label = if state.spectrum.is_some() {
                    Span::styled("Audio", Style::default().fg(theme::success()))
                } else {
                    Span::styled("No Audio", Style::default().fg(muted))
                };
                spans.push(audio_label);
                spans.push(Span::styled(" | ", Style::default().fg(muted)));

                // Device count
                spans.push(Span::styled(
                    format!("{} dev", daemon.device_count),
                    Style::default().fg(primary),
                ));
            }
        }
        ConnectionStatus::Connecting | ConnectionStatus::Reconnecting => {
            spans.push(Span::styled(
                "connecting...",
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

    // Trailing space for visual padding.
    spans.push(Span::raw(" "));
    spans
}
