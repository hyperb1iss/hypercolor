//! The launcher: read the journal before the pointer, run the app it
//! names, and get out of the way.
//!
//! Sign-in runs it as well, and after a power cut that is usually before
//! the update service's delayed start has run recovery. A transaction whose
//! candidate already proved itself has only its commit record left, so the
//! launcher starts the candidate there: a prior started then would keep
//! running under that commit.

use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use hypercolor_install::windows::{
    ReleaseSelectionError, WINDOWS_APP_PROGRAM, WindowsInstallLayout, select_app_release,
};
use hypercolor_install::{InstallStore, MAX_MANAGED_INSTALL_JOURNAL_BYTES};

use crate::dialog::show_failure;
use crate::failure_copy::{FAILURE_TITLE, failure_message};

/// Why the launcher started nothing. Which record gave out matters to
/// nobody reading the dialog, so only the selection's answer survives.
enum LaunchFailure {
    Selection(ReleaseSelectionError),
    Damaged,
}

impl<E: Error> From<E> for LaunchFailure {
    fn from(_: E) -> Self {
        Self::Damaged
    }
}

/// Run the app of the release that may run now, with this process's
/// arguments, and exit without waiting for it.
#[must_use]
pub fn run() -> ExitCode {
    match launch(std::env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let selection = match &failure {
                LaunchFailure::Selection(error) => Some(error),
                LaunchFailure::Damaged => None,
            };
            show_failure(FAILURE_TITLE, failure_message(selection));
            ExitCode::FAILURE
        }
    }
}

fn launch(arguments: Vec<OsString>) -> Result<(), LaunchFailure> {
    let program = app_program()?;
    Command::new(program).args(arguments).spawn()?;
    Ok(())
}

fn app_program() -> Result<PathBuf, LaunchFailure> {
    let executable = std::env::current_exe()?;
    let layout = std::env::var_os("ProgramData")
        .and_then(|data| WindowsInstallLayout::of_launcher(&executable, data.as_ref()))
        .ok_or(LaunchFailure::Damaged)?;
    let store = InstallStore::with_roots(
        layout.release_root(),
        layout.state_root(),
        MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    )?;
    let mut records = store.release_records()?;
    let unit = select_app_release(&mut records).map_err(LaunchFailure::Selection)?;
    Ok(records.unit_program(&unit, WINDOWS_APP_PROGRAM)?)
}
