//! Title bar — the topmost chrome row with stylized brand and status.
//!
//! Renders: `H Y P E R C O L O R` brand on the left (animated by the
//! tachyonfx motion layer via `MotionKey::TitleShimmer`), active screen
//! name centered, and daemon status indicators right-aligned.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::screen::ScreenId;
use crate::state::{AppState, ConnectionStatus};
use crate::theme;

/// Width in cells of the brand text "H Y P E R C O L O R" (10 chars + 9 spaces).
pub const BRAND_WIDTH: u16 = 19;

/// Title bar renderer. Stateless — the shimmer animation is owned by
/// `MotionSystem` as `MotionKey::TitleShimmer`.
#[derive(Default)]
pub struct TitleBar;

impl TitleBar {
    /// Create a new title bar.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Compute the brand area within the given title bar area.
    ///
    /// The brand starts at column +1 (after a leading space) and spans
    /// `BRAND_WIDTH` columns. Returns an empty rect if the area is too small.
    #[must_use]
    pub fn brand_area(area: Rect) -> Rect {
        if area.width < BRAND_WIDTH + 2 || area.height == 0 {
            return Rect::new(area.x, area.y, 0, 0);
        }
        Rect::new(area.x + 1, area.y, BRAND_WIDTH, 1)
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

        // Brand: spaced characters at the base accent color. The
        // tachyonfx title_shimmer effect (running as MotionKey::TitleShimmer)
        // overrides each cell's foreground every render frame.
        spans.push(Span::raw(" "));
        for (i, ch) in "HYPERCOLOR".chars().enumerate() {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(theme::accent_primary())
                    .add_modifier(Modifier::BOLD),
            ));
            if i < 9 {
                spans.push(Span::raw(" "));
            }
        }

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
        let paragraph =
            ratatui::widgets::Paragraph::new(line).style(Style::default().bg(theme::bg_panel()));

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
