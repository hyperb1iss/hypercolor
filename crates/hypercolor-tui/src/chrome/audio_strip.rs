//! Audio strip — mini spectrum visualizer + stats panel.
//!
//! Renders a 2-row region: the top row is a spectrum bar chart using block
//! characters, and the bottom row shows level, beat, and BPM indicators.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::state::{AppState, SpectrumSnapshot};
use crate::theme;

/// Block characters for 8-level bar heights: empty, 1/8, ..., full.
const BAR_CHARS: [char; 9] = [
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
    '\u{2588}',
];

/// Number of beat history dots to show.
const BEAT_DOTS: usize = 4;

/// Stateless audio strip renderer.
pub struct AudioStrip;

impl AudioStrip {
    /// Render the audio strip into the given 2-row area.
    pub fn render(&self, frame: &mut Frame, area: Rect, state: &AppState) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        match state.spectrum.as_ref() {
            Some(snap) => render_active(frame, area, snap.as_ref()),
            None => render_inactive(frame, area),
        }
    }
}

/// Render the active audio strip with spectrum data.
#[allow(clippy::as_conversions)]
fn render_active(frame: &mut Frame, area: Rect, snap: &SpectrumSnapshot) {
    // Split into two rows: spectrum bars (top) and stats (bottom).
    // If only 1 row, just show the bars.
    if area.height == 1 {
        let bars_line = build_spectrum_line(snap, area.width as usize);
        let paragraph = Paragraph::new(bars_line).style(Style::default().bg(theme::bg_base()));
        frame.render_widget(paragraph, area);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    // Row 1: spectrum bars
    let bars_line = build_spectrum_line(snap, rows[0].width as usize);
    let bars_para = Paragraph::new(bars_line).style(Style::default().bg(theme::bg_base()));
    frame.render_widget(bars_para, rows[0]);

    // Row 2: stats line
    let stats_line = build_stats_line(snap);
    let stats_para = Paragraph::new(stats_line).style(Style::default().bg(theme::bg_base()));
    frame.render_widget(stats_para, rows[1]);
}

/// Render a placeholder when no audio data is available.
fn render_inactive(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("No audio", Style::default().fg(theme::text_muted())),
    ]);

    let paragraph = Paragraph::new(line).style(Style::default().bg(theme::bg_base()));
    frame.render_widget(paragraph, area);
}

/// Build the spectrum bar line, downsampling bins to fit the available width.
///
/// Bins are colored in three bands: bass (coral), mid (yellow), treble (cyan).
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn build_spectrum_line(snap: &SpectrumSnapshot, width: usize) -> Line<'static> {
    if snap.bins.is_empty() || width == 0 {
        return Line::raw("");
    }

    let bin_count = snap.bins.len();
    let spans: Vec<Span<'static>> = (0..width)
        .map(|col| {
            // Map this column to a bin range and take the max.
            let start = col * bin_count / width;
            let end = ((col + 1) * bin_count / width)
                .max(start + 1)
                .min(bin_count);
            let value = snap.bins[start..end]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);

            // Quantize to bar character (0..=8).
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let level = (value.clamp(0.0, 1.0) * 8.0) as usize;
            let ch = BAR_CHARS[level.min(8)];

            // Color by frequency band position.
            let t = col as f32 / width.max(1) as f32;
            let color = band_color(t);

            Span::styled(ch.to_string(), Style::default().fg(color))
        })
        .collect();

    Line::from(spans)
}

/// Build the stats line: level meter, beat dots, BPM.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn build_stats_line(snap: &SpectrumSnapshot) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let muted = theme::text_muted();

    // Level meter: filled/empty blocks.
    spans.push(Span::styled(" Level: ", Style::default().fg(muted)));

    let level_blocks = 8;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let filled = (snap.level.clamp(0.0, 1.0) * level_blocks as f32) as usize;

    let level_color = if snap.level > 0.8 {
        theme::error()
    } else if snap.level > 0.5 {
        theme::warning()
    } else {
        theme::success()
    };

    let bar: String = "\u{2587}".repeat(filled);
    let empty: String = "\u{2591}".repeat(level_blocks - filled);
    spans.push(Span::styled(bar, Style::default().fg(level_color)));
    spans.push(Span::styled(empty, Style::default().fg(muted)));

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let pct = (snap.level * 100.0) as u8;
    spans.push(Span::styled(
        format!(" {pct:>3}%"),
        Style::default().fg(theme::text_primary()),
    ));

    // Beat dots.
    spans.push(Span::styled(" | Beat: ", Style::default().fg(muted)));
    for i in 0..BEAT_DOTS {
        let is_lit = snap.beat && snap.beat_confidence > (i as f32 / BEAT_DOTS as f32);
        let dot = if is_lit { "\u{25CF}" } else { "\u{25CB}" };
        let dot_color = if is_lit {
            theme::accent_primary()
        } else {
            muted
        };
        spans.push(Span::styled(
            format!("{dot} "),
            Style::default().fg(dot_color),
        ));
    }

    // BPM.
    if let Some(bpm) = snap.bpm {
        spans.push(Span::styled("| ", Style::default().fg(muted)));
        spans.push(Span::styled(
            format!("{bpm:.0} BPM"),
            Style::default().fg(theme::accent_secondary()),
        ));
    }

    Line::from(spans)
}

/// Return a smoothly interpolated color for the given frequency position.
///
/// Gradient: Coral (bass) → Electric Yellow (mid) → Neon Cyan (treble).
fn band_color(t: f32) -> ratatui::style::Color {
    theme::gradient_color(t, &theme::SPECTRUM_GRADIENT)
}
