//! First-run detection and persistence.
//!
//! Drives the welcome overlay the UI shows on a fresh install so users
//! get a guided path to (1) hardware support, (2) autostart, and (3)
//! device discovery instead of landing on an empty dashboard.
//!
//! Persistence is a single empty marker file under `%LOCALAPPDATA%\
//! hypercolor\first-run-complete` (or the equivalent on macOS/Linux).
//! File presence is the entire state machine — no JSON, no schema, no
//! migration. Deleting the file resets the wizard, which is occasionally
//! useful for testing.

use std::path::PathBuf;

use anyhow::{Context, Result};

const MARKER_FILE_NAME: &str = "first-run-complete";

/// Returns true when no first-run marker exists, meaning the wizard
/// should be surfaced. Defaults to `true` if the marker directory can't
/// be located so we never accidentally suppress the wizard for users
/// who genuinely haven't seen it.
#[must_use]
pub fn is_pending() -> bool {
    !marker_path().is_some_and(|path| path.is_file())
}

/// Persist that the wizard has been completed. Idempotent.
///
/// # Errors
///
/// Returns an error when the LOCALAPPDATA directory cannot be located,
/// the marker directory cannot be created, or the marker file cannot
/// be written.
pub fn mark_complete() -> Result<()> {
    let path = marker_path().context("LOCALAPPDATA / app data directory is unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, b"").with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Delete the marker so the wizard reappears on the next launch.
/// No-op when the marker is already absent. Used by the Developer
/// settings "Show welcome again" affordance and by QA scripts.
///
/// # Errors
///
/// Returns an error when the LOCALAPPDATA directory cannot be located,
/// or when removing an existing marker file fails for a reason other
/// than "not found".
pub fn reset() -> Result<()> {
    let path = marker_path().context("LOCALAPPDATA / app data directory is unavailable")?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("remove {}", path.display()))
        }
    }
}

fn marker_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("hypercolor").join(MARKER_FILE_NAME))
}

/// Tauri command: returns true when the welcome overlay should show.
#[tauri::command]
#[must_use]
pub fn is_first_run_pending() -> bool {
    is_pending()
}

/// Tauri command: persists that the wizard has been seen so it doesn't
/// reappear on subsequent launches.
#[tauri::command]
pub fn mark_first_run_complete() -> Result<(), String> {
    mark_complete().map_err(|err| err.to_string())
}

/// Tauri command: clear the marker so the next launch shows the
/// welcome wizard again. Useful for QA and for users who want to
/// re-run the guided setup.
#[tauri::command]
pub fn reset_first_run() -> Result<(), String> {
    reset().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_path_lives_under_hypercolor_appdata_dir() {
        let path = marker_path().expect("marker path");
        let display = path.display().to_string();
        assert!(
            display.contains("hypercolor"),
            "marker path should live under hypercolor app data dir: {display}"
        );
        assert!(
            display.ends_with(MARKER_FILE_NAME),
            "marker path should end with marker file name: {display}"
        );
    }

    #[test]
    fn reset_is_noop_when_marker_missing() {
        // Probe the real marker path; if it happens to exist (because
        // the dev machine has run the app), skip this test rather than
        // delete it. This test is about the absent-file branch.
        let path = marker_path().expect("marker path");
        if path.is_file() {
            return;
        }
        // Idempotent: removing a missing file should succeed.
        reset().expect("reset succeeds on missing marker");
        reset().expect("reset succeeds again on missing marker");
    }
}
