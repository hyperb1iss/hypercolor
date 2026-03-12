//! Chrome layout manager — the persistent UI shell that wraps every screen.
//!
//! The chrome is the "always visible" frame: title bar, LED preview strip,
//! navigation sidebar, audio spectrum strip, and status bar. The main content
//! area is carved out of whatever space remains and handed to the active view.

mod audio_strip;
mod led_strip;
mod nav_sidebar;
mod status_bar;
mod title_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::action::Action;
use crate::screen::ScreenId;
use crate::state::AppState;

pub use audio_strip::AudioStrip;
pub use led_strip::LedStrip;
pub use nav_sidebar::NavSidebar;
pub use status_bar::StatusBar;
pub use title_bar::TitleBar;

/// Owns all persistent chrome components and manages the frame layout.
pub struct Chrome {
    pub title_bar: TitleBar,
    pub led_strip: LedStrip,
    pub nav_sidebar: NavSidebar,
    pub audio_strip: AudioStrip,
    pub status_bar: StatusBar,
}

impl Chrome {
    /// Create a new chrome shell with default component state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            title_bar: TitleBar,
            led_strip: LedStrip,
            nav_sidebar: NavSidebar,
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
    /// │ Title Bar                  (1 row)   │
    /// ├─────────────────────────────────────┤
    /// │ LED Preview Strip          (2 rows)  │
    /// ├────────┬────────────────────────────┤
    /// │ Nav    │ Main Content Area           │
    /// │ (10c)  │ (remaining)                 │
    /// ├────────┴────────────────────────────┤
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
        let full = area;

        // Vertical split: title(1) | led(2) | middle(flex) | audio(2) | status(1)
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title bar
                Constraint::Length(2), // LED preview
                Constraint::Min(4),    // nav sidebar + main content
                Constraint::Length(2), // audio strip
                Constraint::Length(1), // status bar
            ])
            .split(full);

        let title_area = vertical[0];
        let led_area = vertical[1];
        let middle_area = vertical[2];
        let audio_area = vertical[3];
        let status_area = vertical[4];

        // Horizontal split of the middle: nav sidebar (10 cols) | content
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(10), // nav sidebar
                Constraint::Min(1),     // main content
            ])
            .split(middle_area);

        let nav_area = horizontal[0];
        let content_area = horizontal[1];

        // Render each chrome region.
        self.title_bar.render(frame, title_area, state);
        self.led_strip.render(frame, led_area, state);
        self.nav_sidebar
            .render(frame, nav_area, state.active_screen, available_screens);
        self.audio_strip.render(frame, audio_area, state);
        self.status_bar.render(frame, status_area, state);

        content_area
    }

    /// Return the content area `Rect` for a given terminal size, without
    /// actually rendering anything. Useful for hit-testing or pre-layout.
    #[must_use]
    pub fn content_area(total: Rect) -> Rect {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(total);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(10), Constraint::Min(1)])
            .split(vertical[2]);

        horizontal[1]
    }
}

impl Default for Chrome {
    fn default() -> Self {
        Self::new()
    }
}
