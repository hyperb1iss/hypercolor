//! Keeping "start with Windows" on the stable launcher.
//!
//! The autostart plugin records the path of the program running it. On
//! Windows that program sits in a release unit an update will retire, and
//! the name that outlives every release is the launcher, `Hypercolor.exe`.
//! Each start of the app therefore moves the current user's `Run` value
//! there. That covers the value the plugin just wrote, and a value from
//! before the installation moved into units, which an installer can only
//! fix for the account that ran it.

use std::path::{Path, PathBuf};

const UNITS_DIRECTORY: &str = "units";
const LAUNCHER: &str = "Hypercolor.exe";

/// The launcher of the installation `executable` runs from, when that is a
/// release unit: `<root>\units\<digest>\<program>`.
#[cfg_attr(not(windows), allow(dead_code))]
fn launcher_of(executable: &Path) -> Option<PathBuf> {
    let units = executable.parent()?.parent()?;
    units
        .file_name()?
        .eq_ignore_ascii_case(UNITS_DIRECTORY)
        .then(|| units.parent().map(|root| root.join(LAUNCHER)))
        .flatten()
}

/// The value that replaces `value`, or `None` when it already runs
/// `launcher`.
#[cfg_attr(not(windows), allow(dead_code))]
fn launcher_run_value(value: &str, launcher: &Path) -> Option<String> {
    let launcher = format!("\"{}\"", launcher.display());
    (!value.starts_with(&launcher)).then(|| format!("{launcher} --minimized"))
}

/// Point the current user's autostart at the launcher, if it is on at all.
#[cfg(windows)]
pub fn keep_on_the_launcher() {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Hypercolor";

    let Some(launcher) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(launcher_of)
    else {
        return;
    };
    let Ok(run) = windows_registry::CURRENT_USER
        .options()
        .read()
        .write()
        .open(RUN_KEY)
    else {
        return;
    };
    let Ok(value) = run.get_string(VALUE) else {
        return;
    };
    if let Some(moved) = launcher_run_value(&value, &launcher)
        && let Err(error) = run.set_string(VALUE, moved)
    {
        tracing::warn!(%error, "could not move autostart to the launcher");
    }
}

/// Only Windows keeps its programs in units.
#[cfg(not(windows))]
pub const fn keep_on_the_launcher() {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{launcher_of, launcher_run_value};

    #[test]
    fn a_program_in_a_unit_finds_the_launcher_above_it() {
        assert_eq!(
            launcher_of(Path::new("/install/units/aaaa/hypercolor-app.exe")),
            Some(Path::new("/install/Hypercolor.exe").to_path_buf())
        );
    }

    #[test]
    fn a_program_outside_a_unit_has_no_launcher() {
        for path in [
            "/install/hypercolor-app.exe",
            "/target/debug/hypercolor-app.exe",
            "hypercolor-app.exe",
        ] {
            assert_eq!(launcher_of(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn every_older_value_moves_to_the_launcher() {
        let launcher = Path::new(r"C:\Program Files\Hypercolor\Hypercolor.exe");
        let moved = format!("\"{}\" --minimized", launcher.display());
        for value in [
            r"C:\Program Files\Hypercolor\hypercolor-app.exe --minimized",
            r#""C:\Program Files\Hypercolor\hypercolor-app.exe" --minimized"#,
            r"C:\Program Files\Hypercolor\units\aaaa\hypercolor-app.exe --minimized",
        ] {
            assert_eq!(
                launcher_run_value(value, launcher).as_deref(),
                Some(moved.as_str()),
                "{value}"
            );
        }
    }

    #[test]
    fn a_value_on_the_launcher_stays() {
        let launcher = Path::new(r"C:\Program Files\Hypercolor\Hypercolor.exe");
        let value = format!("\"{}\" --minimized", launcher.display());
        assert_eq!(launcher_run_value(&value, launcher), None);
    }
}
