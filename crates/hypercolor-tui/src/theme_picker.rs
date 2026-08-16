//! Live theme picker modal for the Hypercolor TUI.
//!
//! Wraps `opaline::widgets::ThemeSelectorState` with hypercolor-specific
//! token derivation and persistent preference storage. Pressing `T` opens
//! a searchable, scrollable list of all builtin themes; arrow keys preview
//! live, Enter commits and saves, Esc cancels and rolls back.

use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::event::KeyEvent;
use opaline::Theme;
use opaline::widgets::{ThemeSelector, ThemeSelectorAction, ThemeSelectorState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;
use serde::{Deserialize, Serialize};

// ── Persistence ─────────────────────────────────────────────────────────

/// Application directory appended to the platform config directory.
const APP_DIR: &str = "hypercolor";

/// File name for the TUI config within the Hypercolor config directory.
const CONFIG_FILE_NAME: &str = "tui.toml";

/// Persistent TUI preferences stored at `~/.config/hypercolor/tui.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Active opaline theme name (kebab-case).
    #[serde(default)]
    pub theme: Option<String>,

    /// Show the "♥ Sponsor" link in the status bar (default: true).
    #[serde(default = "default_show_donate")]
    pub show_donate: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: None,
            show_donate: true,
        }
    }
}

fn default_show_donate() -> bool {
    true
}

/// Locate the TUI config file path.
///
/// `HYPERCOLOR_TUI_CONFIG` overrides the location outright; otherwise the file
/// lands in the same XDG layout `hypercolor-core`'s `config::paths` resolves
/// (`~/.config/hypercolor/` on Linux, `%APPDATA%\hypercolor\` on Windows),
/// reached through `dirs` directly so the TUI stays off the render engine.
///
/// # Panics
///
/// Panics when no platform config directory exists and no override is set.
/// Guessing a relative location here would scatter config into whatever
/// directory the TUI happened to launch from.
pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPERCOLOR_TUI_CONFIG") {
        return PathBuf::from(path);
    }
    resolve_config_path(dirs::config_dir())
        .expect("no platform config directory; set HOME or HYPERCOLOR_TUI_CONFIG")
}

/// Place the TUI config file inside a resolved platform config directory.
///
/// Split out from [`config_path`] because the environment-driven half cannot be
/// exercised from a test: edition 2024 makes `std::env::set_var` unsafe and
/// this crate forbids unsafe code.
fn resolve_config_path(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join(APP_DIR).join(CONFIG_FILE_NAME))
}

/// Load the TUI config from disk, returning defaults if missing.
pub fn load_config() -> TuiConfig {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save the full TUI config to disk.
pub fn save_config(config: &TuiConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(config).context("failed to serialize TUI config")?;
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).ok();
    }

    Ok(())
}

/// Save the active theme to the TUI config.
pub fn save_theme(name: &str) -> Result<()> {
    let mut config = load_config();
    config.theme = Some(name.to_string());
    save_config(&config)
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
    use std::path::PathBuf;

    use super::{config_path, resolve_config_path};

    #[test]
    fn unresolvable_config_dir_yields_no_path() {
        assert_eq!(resolve_config_path(None), None);
    }

    #[test]
    fn resolved_path_nests_under_the_platform_config_dir() {
        let resolved = resolve_config_path(Some(PathBuf::from("/xdg-config")))
            .expect("a resolved config dir yields a path");
        assert!(resolved.starts_with("/xdg-config"));
        assert!(resolved.ends_with("hypercolor/tui.toml"));
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
