//! Navigation sidebar — vertical screen selector with keybinding hints.
//!
//! Renders one line per screen: `[D]ash`, `[E]ffx`, etc.  The active screen
//! is highlighted with the accent color and bold modifier.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::screen::ScreenId;
use crate::theme;

/// Stateless navigation sidebar renderer.
pub struct NavSidebar;

impl NavSidebar {
    /// Render the sidebar into the given area.
    ///
    /// Each screen appears as a single line with its key hint bracketed:
    /// `[D]ash  [E]ffx  [C]trl  De[v]s  [P]rof  [S]ttg  De[b]ug`
    pub fn render(&self, frame: &mut Frame, area: Rect, active: ScreenId, screens: &[ScreenId]) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let lines: Vec<Line<'_>> = screens
            .iter()
            .map(|&screen| build_nav_line(screen, screen == active))
            .collect();

        let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::bg_panel()));

        frame.render_widget(paragraph, area);
    }
}

/// Build a single nav line for a screen entry.
///
/// The key hint character is wrapped in brackets and highlighted.
/// Active screen: accent primary + bold.  Inactive: muted.
fn build_nav_line(screen: ScreenId, is_active: bool) -> Line<'static> {
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

    // Find the key hint position within the label for inline bracketing.
    // For labels like "Dash", key='D' -> "[D]ash"
    // For labels like "Devs", key='V' -> "De[v]s"
    // For labels like "Dbug", key='B' -> "D[b]ug"
    let key_lower = key.to_ascii_lowercase();
    let key_upper = key.to_ascii_uppercase();

    if let Some(pos) = label.find(key_upper).or_else(|| label.find(key_lower)) {
        let before = &label[..pos];
        let after = &label[pos + key.len_utf8()..];

        Line::from(vec![
            Span::raw(" "),
            Span::styled(before.to_string(), label_style),
            Span::styled("[", Style::default().fg(theme::text_muted())),
            Span::styled(key.to_string(), key_style),
            Span::styled("]", Style::default().fg(theme::text_muted())),
            Span::styled(after.to_string(), label_style),
        ])
    } else {
        // Fallback: just bracket the key at the front.
        Line::from(vec![
            Span::raw(" "),
            Span::styled("[", Style::default().fg(theme::text_muted())),
            Span::styled(key.to_string(), key_style),
            Span::styled("]", Style::default().fg(theme::text_muted())),
            Span::styled(label.to_string(), label_style),
        ])
    }
}
