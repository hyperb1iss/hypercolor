//! Live theme picker modal for the Hypercolor TUI.
//!
//! Wraps `opaline::widgets::ThemeSelectorState` with hypercolor-specific
//! token derivation and persistent preference storage. Pressing `T` opens
//! a searchable, scrollable list of all builtin themes; arrow keys preview
//! live, Enter commits and saves, Esc cancels and rolls back.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::KeyEvent;
use hypercolor_core::client_preferences::TuiPreferenceStore;
pub use hypercolor_types::client_preferences::TuiPreferences as TuiConfig;
use opaline::Theme;
use opaline::widgets::{ThemeSelector, ThemeSelectorAction, ThemeSelectorState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;

// ── Persistence ─────────────────────────────────────────────────────────

/// Locate the TUI config file path.
///
/// `HYPERCOLOR_TUI_CONFIG` overrides the location outright; otherwise the file
/// lands in the shared Hypercolor config directory.
///
/// # Panics
///
/// Panics when no platform config directory exists and no override is set.
/// Guessing a relative location here would scatter config into whatever
/// directory the TUI happened to launch from.
pub fn config_path() -> PathBuf {
    hypercolor_core::client_preferences::tui_config_path()
}

/// Load the TUI config from disk, returning defaults if missing.
pub fn load_config() -> TuiConfig {
    TuiPreferenceStore::new(config_path())
        .load()
        .unwrap_or_default()
}

/// Save the full terminal preference document through the shared writer.
pub fn save_config(config: &TuiConfig) -> Result<()> {
    TuiPreferenceStore::new(config_path()).modify(|current| *current = config.clone())
}

/// Change only the theme while retaining the latest other preferences.
pub fn save_theme(name: &str) -> Result<()> {
    TuiPreferenceStore::new(config_path()).modify(|current| current.theme = Some(name.into()))
}

// ── Token derivation ────────────────────────────────────────────────────

/// Hypercolor-specific token derivation, called on every theme load so the
/// app's custom tokens (gradients, spectrum bands) stay in sync with the
/// active theme's palette.
///
/// Currently a no-op — the tokens we use (`accent.primary`,
/// `accent.secondary`, `text.primary`, `text.muted`, `bg.base`, `bg.panel`,
/// `bg.highlight`, `success`, `error`, `warning`, `accent.tertiary`,
/// `code.number`) are present in every opaline builtin theme, so no
/// derivation is needed. Hook stays in place for future custom tokens.
pub fn derive_tokens(_theme: &mut Theme) {
    // Reserved for future per-app token derivation.
}

// ── Picker state ────────────────────────────────────────────────────────

/// Modal theme picker. `None` when closed.
pub struct ThemePicker {
    state: ThemeSelectorState,
}

impl ThemePicker {
    /// Open the picker with the currently active theme pre-selected.
    pub fn open() -> Self {
        Self {
            state: ThemeSelectorState::with_current_selected().with_derive(derive_tokens),
        }
    }

    /// Handle a key event. Returns the action taken so the app can react.
    pub fn handle_key(&mut self, key: KeyEvent) -> ThemeSelectorAction {
        self.state.handle_key(key)
    }

    /// Render the picker as a centered modal over the existing frame.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Center it: 60 cols wide, 20 rows tall (or whatever fits).
        let width = 60u16.min(area.width.saturating_sub(4));
        let height = 20u16.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let modal_area = Rect::new(x, y, width, height);

        frame.render_widget(Clear, modal_area);
        frame.render_stateful_widget(ThemeSelector::new(), modal_area, &mut self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::config_path;

    #[test]
    fn terminal_and_backup_use_the_shared_path_resolver() {
        assert_eq!(
            config_path(),
            hypercolor_core::client_preferences::tui_config_path()
        );
    }

    #[test]
    fn resolved_path_is_absolute_and_tilde_free() {
        // The env override is caller-owned and cannot be cleared from a test:
        // edition 2024 makes `std::env::set_var` unsafe and this crate forbids
        // unsafe code.
        if std::env::var_os("HYPERCOLOR_TUI_CONFIG").is_some() {
            return;
        }
        let path = config_path();
        assert!(
            path.is_absolute(),
            "resolved {} is not absolute",
            path.display()
        );
        assert!(
            !path.components().any(|part| part.as_os_str() == "~"),
            "resolved {} contains a literal tilde component",
            path.display()
        );
    }
}
