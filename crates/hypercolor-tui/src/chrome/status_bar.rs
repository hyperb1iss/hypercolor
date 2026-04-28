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

/// Interactive region hit by a status-bar mouse click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarHit {
    Screen(ScreenId),
    Sponsor,
    Help,
}

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
        let wide = area.width > 100;
        let right_spans =
            build_nav_hints(active_screen, available_screens, state.show_donate, wide);

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

    /// Resolve a terminal cell in the status bar to the action-sized region
    /// rendered at that position.
    #[must_use]
    pub fn hit_test(
        area: Rect,
        col: u16,
        row: u16,
        active_screen: ScreenId,
        available_screens: &[ScreenId],
        show_donate: bool,
    ) -> Option<StatusBarHit> {
        if area.height == 0
            || area.width == 0
            || row != area.y
            || col < area.x
            || col >= area.x + area.width
        {
            return None;
        }

        let wide = area.width > 100;
        let right_len = nav_hints_width(active_screen, available_screens, show_donate, wide);
        let Ok(right_len_u16) = u16::try_from(right_len) else {
            return None;
        };
        let start = area
            .x
            .saturating_add(area.width.saturating_sub(right_len_u16));
        if col < start || usize::from(col - start) >= right_len {
            return None;
        }

        hit_test_nav_hints(
            usize::from(col - start),
            available_screens,
            show_donate,
            wide,
        )
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
        if let Some(scene_name) = daemon.scene_name.as_ref() {
            spans.push(Span::styled(" \u{2500} ", Style::default().fg(muted)));
            spans.push(Span::styled(
                scene_name.clone(),
                Style::default().fg(theme::accent_secondary()),
            ));
            if daemon.scene_snapshot_locked {
                spans.push(Span::styled(" ", Style::default().fg(muted)));
                spans.push(Span::styled(
                    "[snap]",
                    Style::default()
                        .fg(theme::warning())
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }

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
fn build_nav_hints(
    active: ScreenId,
    screens: &[ScreenId],
    show_donate: bool,
    wide: bool,
) -> Vec<Span<'static>> {
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
                spans.push(Span::styled(
                    chars.collect::<String>(),
                    Style::default().fg(muted),
                ));
            }
        }
    }

    // Sponsor link — heart in coral, text in accent (text hidden on narrow terminals)
    if show_donate {
        spans.push(Span::styled(" \u{2502} ", sep));
        spans.push(Span::styled(
            "\u{2665}",
            Style::default()
                .fg(ratatui::style::Color::Rgb(255, 106, 193))
                .add_modifier(Modifier::BOLD),
        ));
        if wide {
            spans.push(Span::styled(
                " Sponsor",
                Style::default().fg(theme::accent_primary()),
            ));
        }
    }

    // Help hint
    spans.push(Span::styled(" \u{2502} ", sep));
    spans.push(Span::styled("?", Style::default().fg(theme::warning())));
    spans.push(Span::styled("help", Style::default().fg(muted)));
    spans.push(Span::raw(" "));
    spans
}

fn nav_hints_width(active: ScreenId, screens: &[ScreenId], show_donate: bool, wide: bool) -> usize {
    build_nav_hints(active, screens, show_donate, wide)
        .iter()
        .map(Span::width)
        .sum()
}

fn hit_test_nav_hints(
    col: usize,
    screens: &[ScreenId],
    show_donate: bool,
    wide: bool,
) -> Option<StatusBarHit> {
    let mut cursor = 0usize;

    for (idx, &screen) in screens.iter().enumerate() {
        if idx > 0 {
            cursor += 3;
        }
        let width = screen.label().len();
        if col >= cursor && col < cursor + width {
            return Some(StatusBarHit::Screen(screen));
        }
        cursor += width;
    }

    if show_donate {
        cursor += 3;
        let sponsor_width = if wide {
            Span::raw("\u{2665} Sponsor").width()
        } else {
            Span::raw("\u{2665}").width()
        };
        if col >= cursor && col < cursor + sponsor_width {
            return Some(StatusBarHit::Sponsor);
        }
        cursor += sponsor_width;
    }

    cursor += 3;
    (col >= cursor && col < cursor + "?help".len()).then_some(StatusBarHit::Help)
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
