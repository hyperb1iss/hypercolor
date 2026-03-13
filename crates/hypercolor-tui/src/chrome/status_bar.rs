//! Status bar — the bottom chrome row.
//!
//! Renders a single line: current effect name, device count, LED count
//! (left-aligned) and the active profile name (right-aligned).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::screen::ScreenId;
use crate::state::AppState;
use crate::theme;

/// Stateless status bar renderer.
pub struct StatusBar;

impl StatusBar {
    /// Render the status bar into the given single-row area.
    #[allow(clippy::as_conversions)]
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

        let left_spans = build_left(state);
        let right_spans = build_nav_hints(active_screen, available_screens);

        let left_len: usize = left_spans.iter().map(Span::width).sum();
        let right_len: usize = right_spans.iter().map(Span::width).sum();

        let pad = (area.width as usize).saturating_sub(left_len + right_len);

        let mut spans = left_spans;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(right_spans);

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(theme::bg_panel()));

        frame.render_widget(paragraph, area);
    }
}

/// Build the left-aligned status spans.
fn build_left(state: &AppState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let muted = theme::text_muted();

    spans.push(Span::raw(" "));

    // Current effect name — gradient brand style.
    let effect_name = state
        .daemon
        .as_ref()
        .and_then(|d| d.effect_name.clone())
        .unwrap_or_else(|| "No effect".to_string());

    gradient_text(&mut spans, &effect_name);

    // Separator + device count.
    if let Some(ref daemon) = state.daemon {
        spans.push(Span::styled(" \u{2500} ", Style::default().fg(muted)));
        spans.push(Span::styled(
            format!("{} devices", daemon.device_count),
            Style::default().fg(theme::text_primary()),
        ));

        // LED count.
        spans.push(Span::styled(" \u{00B7} ", Style::default().fg(muted)));
        spans.push(Span::styled(
            format!("{} LEDs", daemon.total_leds),
            Style::default().fg(theme::text_primary()),
        ));
    }

    spans
}

/// Build right-aligned nav hints: `dash | effx | ctrl | ?help`
/// Active screen's first char is highlighted; items separated by `|`.
fn build_nav_hints(active: ScreenId, screens: &[ScreenId]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let muted = theme::text_muted();
    let sep = Style::default().fg(muted);

    for &screen in screens {
        if !spans.is_empty() {
            spans.push(Span::styled(" \u{2502} ", sep));
        }
        let label = screen.label().to_ascii_lowercase();
        let is_active = screen == active;

        if is_active {
            // Highlight first char, rest in accent
            let mut chars = label.chars();
            if let Some(first) = chars.next() {
                spans.push(Span::styled(
                    first.to_string(),
                    Style::default()
                        .fg(theme::accent_secondary())
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    chars.collect::<String>(),
                    Style::default().fg(theme::accent_primary()),
                ));
            }
        } else {
            // Highlight first char as key hint, rest muted
            let mut chars = label.chars();
            if let Some(first) = chars.next() {
                spans.push(Span::styled(
                    first.to_string(),
                    Style::default().fg(theme::warning()),
                ));
                spans.push(Span::styled(chars.collect::<String>(), Style::default().fg(muted)));
            }
        }
    }

    // Help hint
    spans.push(Span::styled(" \u{2502} ", sep));
    spans.push(Span::styled("?", Style::default().fg(theme::warning())));
    spans.push(Span::styled("help", Style::default().fg(muted)));
    spans.push(Span::raw(" "));
    spans
}

/// Render text with a per-character gradient (Neon Cyan → Electric Purple).
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn gradient_text(spans: &mut Vec<Span<'static>>, text: &str) {
    let len = text.chars().count();
    for (i, ch) in text.chars().enumerate() {
        let t = if len <= 1 {
            0.0
        } else {
            i as f32 / (len - 1) as f32
        };
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(theme::gradient_color(t, &theme::EFFECT_GRADIENT))
                .add_modifier(Modifier::BOLD),
        ));
    }
}
