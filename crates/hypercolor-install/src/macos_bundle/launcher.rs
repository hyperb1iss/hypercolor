//! The stable launcher's `update-executor` role.
//!
//! launchd resolves an agent's program when it starts the agent, so an
//! activator started through the active pointer could resume a transaction
//! with the candidate it is deciding about. The launcher sits outside every
//! unit, reads the journal the way [`select_release`] does, and becomes that
//! release's CLI.
//!
//! The launcher is the CLI of the bundle that was adopted, copied whole: it
//! already carries the team's signature, the policy already names it, and an
//! update never replaces it. It must replace itself rather than spawn a
//! child. macOS's App Management lets code signed by the app's own team
//! change the app, and it judges the agent's process, so the release that
//! touches the bundle has to be that process.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use super::installation::MacosBundleRoots;
use crate::release_selection::{ReleaseRecords, ReleaseSelectionError, select_release};
use crate::store::INSTALL_JOURNAL_FILE;
use crate::{MAX_MANAGED_INSTALL_JOURNAL_BYTES, ReleasePayloadError, UnitId};

const ROLE_FLAG: &str = "--role";
const UPDATE_EXECUTOR_ROLE: &str = "update-executor";
const ACTIVE_POINTER: &str = "active";
/// `EX_USAGE`: the launcher was asked for something it does not do.
const EXIT_USAGE: i32 = 64;
/// `EX_UNAVAILABLE`: no release may run now.
const EXIT_UNAVAILABLE: i32 = 69;
/// `EX_OSERR`: the release that may run could not be executed.
const EXIT_EXEC_FAILED: i32 = 71;

/// Why the launcher has no release to run.
#[derive(Debug, thiserror::Error)]
pub enum MacosBundleLauncherError {
    #[error("the installation's records cannot be opened: {0}")]
    Records(#[source] io::Error),
    #[error(transparent)]
    Selection(#[from] ReleaseSelectionError),
    /// A missing or altered CLI never recovers in the prior's place.
    #[error(transparent)]
    Unverified(#[from] ReleasePayloadError),
}

/// The CLI the update agents of `roots` must run now: the open transaction's
/// prior, otherwise the active unit's, proven against its unit's manifest.
///
/// # Errors
/// Returns an error when neither rule names a unit, or that unit's CLI
/// cannot be verified. It never settles for another release.
pub fn select_macos_bundle_update_executor(
    roots: &MacosBundleRoots,
) -> Result<PathBuf, MacosBundleLauncherError> {
    let mut records = BundleRecords::open(roots).map_err(MacosBundleLauncherError::Records)?;
    let unit = select_release(&mut records)?;
    Ok(crate::verified_macos_bundle_cli(
        &roots.release_root,
        &unit,
    )?)
}

/// Run the launcher role this process was started for, if it was.
///
/// Returns `None` when the arguments ask for no role, so the program goes on
/// as the CLI. Otherwise this process becomes the selected release's CLI and
/// the call only returns the exit status of a launcher that ran nothing.
#[must_use]
pub fn run_macos_bundle_launcher_role() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.first().map(OsString::as_os_str) != Some(OsStr::new(ROLE_FLAG)) {
        return None;
    }
    let Some(command) = update_executor_command(&arguments) else {
        eprintln!("hypercolor: the launcher only takes --role {UPDATE_EXECUTOR_ROLE} -- <command>");
        return Some(EXIT_USAGE);
    };
    let roots = std::env::current_exe()
        .ok()
        .and_then(|executable| MacosBundleRoots::of_launcher(&executable));
    let Some(roots) = roots else {
        eprintln!("hypercolor: --role runs only as an installation's launcher");
        return Some(EXIT_USAGE);
    };
    let executable = match select_macos_bundle_update_executor(&roots) {
        Ok(executable) => executable,
        Err(failure) => {
            eprintln!("hypercolor: no release may run this update: {failure}");
            return Some(EXIT_UNAVAILABLE);
        }
    };
    let failure = Command::new(&executable).args(command).exec();
    eprintln!(
        "hypercolor: {} did not start: {failure}",
        executable.display()
    );
    Some(EXIT_EXEC_FAILED)
}

/// The command after `--role update-executor --`, when `arguments` are
/// exactly that.
pub(super) fn update_executor_command(arguments: &[OsString]) -> Option<&[OsString]> {
    match arguments {
        [flag, role, separator, command @ ..]
            if flag == ROLE_FLAG
                && role == UPDATE_EXECUTOR_ROLE
                && separator == "--"
                && !command.is_empty() =>
        {
            Some(command)
        }
        _ => None,
    }
}

/// The unit a release root's `active` link names.
///
/// # Errors
/// Returns why the target is not `units/<digest>`.
pub(super) fn unit_of_active_link(target: &Path) -> Result<UnitId, String> {
    let mut components = target.components();
    match (components.next(), components.next(), components.next()) {
        (Some(Component::Normal(units)), Some(Component::Normal(unit)), None)
            if units == "units" =>
        {
            let unit = unit
                .to_str()
                .ok_or_else(|| "the active link is not UTF-8".to_owned())?;
            UnitId::new(unit).map_err(|source| source.to_string())
        }
        _ => Err(format!(
            "the active link names {}, not a unit",
            target.display()
        )),
    }
}

/// The two records selection reads, through retained directory handles.
struct BundleRecords {
    state: ReadOnlyDirectoryAuthority,
    release: ReadOnlyDirectoryAuthority,
}

impl BundleRecords {
    fn open(roots: &MacosBundleRoots) -> io::Result<Self> {
        Ok(Self {
            state: ReadOnlyDirectoryAuthority::open(&roots.state_root)?,
            release: ReadOnlyDirectoryAuthority::open(&roots.release_root)?,
        })
    }
}

impl ReleaseRecords for BundleRecords {
    fn read_journal(&mut self) -> io::Result<Option<Vec<u8>>> {
        let opened = match self
            .state
            .open_regular_file(Path::new(INSTALL_JOURNAL_FILE))
        {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let limit = MAX_MANAGED_INSTALL_JOURNAL_BYTES as u64;
        let mut bytes = Vec::new();
        opened.into_file().take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the install journal exceeds its bound",
            ));
        }
        Ok(Some(bytes))
    }

    fn read_pointer(&mut self) -> io::Result<Option<Vec<u8>>> {
        Ok(self
            .release
            .read_symlink(Path::new(ACTIVE_POINTER))?
            .map(|target| target.into_os_string().into_vec()))
    }

    fn decode_pointer(&self, bytes: &[u8]) -> Result<UnitId, String> {
        let target = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        unit_of_active_link(Path::new(target))
    }
}
