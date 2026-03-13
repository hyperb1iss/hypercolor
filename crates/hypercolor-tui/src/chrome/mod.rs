//! Chrome layout manager — the persistent UI shell that wraps every screen.
//!
//! The chrome is the "always visible" frame: title bar (with inline nav),
//! audio spectrum strip, and status bar. The main content area is carved out
//! of whatever space remains and handed to the active view.

mod audio_strip;
mod status_bar;
mod title_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::action::Action;
use crate::screen::ScreenId;
use crate::state::AppState;

pub use audio_strip::AudioStrip;
pub use status_bar::StatusBar;
pub use title_bar::TitleBar;

/// Owns all persistent chrome components and manages the frame layout.
pub struct Chrome {
    pub title_bar: TitleBar,
    pub audio_strip: AudioStrip,
    pub status_bar: StatusBar,
}

impl Chrome {
    /// Create a new chrome shell with default component state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            title_bar: TitleBar,
            audio_strip: AudioStrip,
            status_bar: StatusBar,
        }
    }

    /// Forward an action to all chrome sub-components that care about it.
    pub fn update(&mut self, _action: &Action) {
        // Chrome components are stateless renderers today — they pull
        // everything they need from `AppState` at render time. This hook
        // exists so future animation state (e.g. beat pulse timers) can
        // be driven by the action loop without refactoring callers.
    }

    /// Render all chrome regions and return the `Rect` for the main content area.
    ///
    /// Layout (top to bottom):
    /// ```text
    /// ┌─────────────────────────────────────┐
    /// │ Title Bar + Nav Tabs       (1 row)   │
    /// ├─────────────────────────────────────┤
    /// │ Main Content Area       (remaining)  │
    /// ├─────────────────────────────────────┤
    /// │ Audio Strip                (2 rows)  │
    /// ├─────────────────────────────────────┤
    /// │ Status Bar                 (1 row)   │
    /// └─────────────────────────────────────┘
    /// ```
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &AppState,
        available_screens: &[ScreenId],
    ) -> Rect {
        // Vertical split: title(1) | content(flex) | audio(2) | status(1)
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title bar + nav
                Constraint::Min(4),    // main content (full width)
                Constraint::Length(2), // audio strip
                Constraint::Length(1), // status bar
            ])
            .split(area);

        let title_area = vertical[0];
        let content_area = vertical[1];
        let audio_area = vertical[2];
        let status_area = vertical[3];

        // Render each chrome region.
        self.title_bar
            .render(frame, title_area, state, state.active_screen, available_screens);
        self.audio_strip.render(frame, audio_area, state);
        self.status_bar.render(frame, status_area, state);

        content_area
    }
}

impl Default for Chrome {
    fn default() -> Self {
        Self::new()
    }
}
