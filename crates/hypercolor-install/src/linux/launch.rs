//! Select one installed release exactly once, then name everything it runs.
//!
//! A managed service never starts a release through the `active` pointer.
//! Its unit starts the installation's launcher (`<release root>/launcher/
//! hypercolor __launch --role <role>`), which reads `active` once, proves
//! the release it names, and returns every path the process will use
//! (executable, UI and effects directories) under that one release's own
//! directory. The caller then replaces itself with that process, so a swap
//! of `active` at any later moment cannot mix one release's executable with
//! another's assets.
//!
//! Launcher contract 1 reads only what every release since the contract
//! keeps: the permanent locator and recorded location, the `active` pointer
//! grammar, a release's content address (the SHA-256 of its `manifest.json`
//! is its directory name) and its fixed component paths, its manifest's
//! `managed_package.launcher_contract`, and the install journal's
//! `disposition` and `prior_active_unit`. A release that changes any of
//! these changes the launcher contract.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::bootstrap::linux_launcher_path;
use super::{LinuxInstallAuthority, LinuxInstallLocation};
use crate::{MANAGED_LAUNCHER_CONTRACT, MAX_MANAGED_INSTALL_JOURNAL_BYTES, UnitId};

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PASSTHROUGH_ARGUMENTS: usize = 64;
const MAX_PASSTHROUGH_BYTES: usize = 64 * 1024;
const DAEMON_PROGRAM: &str = "hypercolor-daemon";
const CLI_PROGRAM: &str = "hypercolor";
const UI_DIRECTORY: [&str; 3] = ["share", "hypercolor", "ui"];
const EFFECTS_DIRECTORY: [&str; 4] = ["share", "hypercolor", "effects", "bundled"];

/// What the launcher starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxLaunchRole {
    /// The active release's daemon, with that release's UI and effects.
    Daemon,
    /// The active release's CLI.
    Cli,
    /// The CLI that settles installs: while a transaction is unsettled, the
    /// release it would roll back to, so an unproven candidate's code never
    /// runs its own recovery; otherwise the active release's. Either way the
    /// release must declare this launcher contract, since only such a CLI
    /// takes the commands a recovery unit runs.
    UpdateExecutor,
}

impl LinuxLaunchRole {
    /// Every role, in the spelling the unit files use.
    pub const ALL: [Self; 3] = [Self::Daemon, Self::Cli, Self::UpdateExecutor];

    /// The role's spelling after `--role`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Cli => "cli",
            Self::UpdateExecutor => "update-executor",
        }
    }

    /// Parse a role spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == value)
    }
}

/// One launch to plan.
#[derive(Debug, Clone)]
pub struct LinuxLaunchRequest<'a> {
    /// The user's home, which holds the permanent locator.
    pub home: &'a Path,
    pub role: LinuxLaunchRole,
    /// Arguments after `--`, passed to the CLI roles unchanged.
    pub arguments: Vec<OsString>,
    /// The resolved path of the running launcher program; it must be the
    /// launcher of the installation the locator names.
    pub launcher: &'a Path,
}

/// Why the launcher chose its release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxLaunchSelection {
    /// The release the `active` pointer names.
    Active,
    /// The release an unsettled transaction would roll back to.
    PendingTransactionPrior,
}

/// Everything one launch runs, all beneath one release's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxLaunchPlan {
    /// The release that runs.
    pub unit: UnitId,
    pub selection: LinuxLaunchSelection,
    /// The executable to replace the launcher with.
    pub program: PathBuf,
    /// Its arguments, without `argv[0]`.
    pub arguments: Vec<OsString>,
    /// Variables to set on top of the launcher's environment.
    pub environment: Vec<(OsString, OsString)>,
}

/// A launch that cannot proceed.
#[derive(Debug, thiserror::Error)]
pub enum LinuxLaunchError {
    #[error("no managed Hypercolor installation is recorded for this user")]
    NotManaged,
    #[error("the installation locator could not be read: {0}")]
    Locator(String),
    #[error(
        "this launcher is {actual}, but the installation's launcher is {expected}",
        actual = .actual.display(),
        expected = .expected.display()
    )]
    ForeignLauncher { expected: PathBuf, actual: PathBuf },
    #[error("the installation has no active release")]
    NoActiveRelease,
    #[error("the active pointer is not a release: {0}")]
    ActivePointer(String),
    #[error("release {unit} cannot be launched: {detail}")]
    Release { unit: String, detail: String },
    #[error("the install journal could not be read: {0}")]
    Journal(String),
    #[error("the recorded roots cannot be passed to the daemon: {0}")]
    Roots(String),
    #[error("invalid launch arguments: {0}")]
    Arguments(String),
}

/// Choose the release one launch runs and name everything it runs.
///
/// Reads the locator and the `active` pointer once each, and for
/// [`LinuxLaunchRole::UpdateExecutor`] the install journal once; takes no
/// lock and writes nothing. The chosen release must be a digest release of
/// this installation whose directory is read-only, owned by the recorded
/// user, content-addressed by its own manifest, and complete for the role.
///
/// # Errors
/// Refuses an unmanaged installation, a foreign launcher, a legacy or
/// incomplete release, and arguments the role does not take.
pub fn plan_linux_launch(
    request: &LinuxLaunchRequest<'_>,
) -> Result<LinuxLaunchPlan, LinuxLaunchError> {
    let location = match super::locator::read_hint(request.home) {
        Ok(LinuxInstallAuthority::Managed(location)) => location,
        Ok(LinuxInstallAuthority::Legacy(_)) => return Err(LinuxLaunchError::NotManaged),
        Err(error) => return Err(LinuxLaunchError::Locator(error.to_string())),
    };
    let expected = linux_launcher_path(&location);
    if request.launcher != expected {
        return Err(LinuxLaunchError::ForeignLauncher {
            expected,
            actual: request.launcher.to_path_buf(),
        });
    }
    let passthrough = validate_arguments(request.role, &request.arguments)?;
    let releases = ReadOnlyDirectoryAuthority::open(location.release_root())
        .map_err(|source| LinuxLaunchError::ActivePointer(source.to_string()))?;
    let active = read_active(&releases)?;
    let (unit, selection) = match request.role {
        LinuxLaunchRole::UpdateExecutor => match pending_prior(&location)? {
            Some(prior) if prove_release(&releases, &location, &prior, request.role).is_ok() => {
                (prior, LinuxLaunchSelection::PendingTransactionPrior)
            }
            _ => (
                active.ok_or(LinuxLaunchError::NoActiveRelease)?,
                LinuxLaunchSelection::Active,
            ),
        },
        LinuxLaunchRole::Daemon | LinuxLaunchRole::Cli => (
            active.ok_or(LinuxLaunchError::NoActiveRelease)?,
            LinuxLaunchSelection::Active,
        ),
    };
    if selection == LinuxLaunchSelection::Active {
        prove_release(&releases, &location, &unit, request.role)?;
    }
    let release = location.release_root().join("units").join(unit.as_str());
    let plan = match request.role {
        LinuxLaunchRole::Daemon => LinuxLaunchPlan {
            program: release.join("bin").join(DAEMON_PROGRAM),
            arguments: vec![
                OsString::from("--ui-dir"),
                release
                    .join(UI_DIRECTORY.iter().collect::<PathBuf>())
                    .into(),
                OsString::from("--effects-dir"),
                release
                    .join(EFFECTS_DIRECTORY.iter().collect::<PathBuf>())
                    .into(),
            ],
            environment: daemon_environment(&location)?,
            unit,
            selection,
        },
        LinuxLaunchRole::Cli | LinuxLaunchRole::UpdateExecutor => LinuxLaunchPlan {
            program: release.join("bin").join(CLI_PROGRAM),
            arguments: passthrough,
            environment: Vec::new(),
            unit,
            selection,
        },
    };
    Ok(plan)
}

fn validate_arguments(
    role: LinuxLaunchRole,
    arguments: &[OsString],
) -> Result<Vec<OsString>, LinuxLaunchError> {
    if role == LinuxLaunchRole::Daemon && !arguments.is_empty() {
        return Err(LinuxLaunchError::Arguments(
            "the daemon role takes no arguments; its release decides them".to_owned(),
        ));
    }
    let bytes: usize = arguments.iter().map(|argument| argument.len()).sum();
    if arguments.len() > MAX_PASSTHROUGH_ARGUMENTS || bytes > MAX_PASSTHROUGH_BYTES {
        return Err(LinuxLaunchError::Arguments(format!(
            "at most {MAX_PASSTHROUGH_ARGUMENTS} arguments of {MAX_PASSTHROUGH_BYTES} bytes"
        )));
    }
    Ok(arguments.to_vec())
}

/// Read `active` once: `units/<digest>`, or nothing.
fn read_active(releases: &ReadOnlyDirectoryAuthority) -> Result<Option<UnitId>, LinuxLaunchError> {
    let Some(target) = releases
        .read_symlink(Path::new("active"))
        .map_err(|source| LinuxLaunchError::ActivePointer(source.to_string()))?
    else {
        return Ok(None);
    };
    let mut components = target.components();
    match (components.next(), components.next(), components.next()) {
        (Some(Component::Normal(units)), Some(Component::Normal(unit)), None)
            if units == "units" =>
        {
            unit.to_str()
                .and_then(|unit| UnitId::new(unit).ok())
                .map(Some)
                .ok_or_else(|| {
                    LinuxLaunchError::ActivePointer(format!("{} names no unit", target.display()))
                })
        }
        _ => Err(LinuxLaunchError::ActivePointer(format!(
            "unexpected target {}",
            target.display()
        ))),
    }
}

/// Prove one release is this installation's, immutable, content-addressed
/// and complete for `role`.
fn prove_release(
    releases: &ReadOnlyDirectoryAuthority,
    location: &LinuxInstallLocation,
    unit: &UnitId,
    role: LinuxLaunchRole,
) -> Result<(), LinuxLaunchError> {
    let refuse = |detail: String| LinuxLaunchError::Release {
        unit: unit.as_str().to_owned(),
        detail,
    };
    let io = |what: &str| {
        let what = what.to_owned();
        move |source: std::io::Error| refuse(format!("{what}: {source}"))
    };
    if unit.as_str().starts_with("legacy-") {
        return Err(refuse(
            "a legacy snapshot is not a launchable release".to_owned(),
        ));
    }
    let release = releases
        .open_child_directory(Path::new("units"))
        .and_then(|units| units.open_child_directory(Path::new(unit.as_str())))
        .map_err(io("open the release"))?;
    let metadata = release.metadata().map_err(io("inspect the release"))?;
    if metadata.owner_uid() != location.uid() || metadata.mode() & 0o222 != 0 {
        return Err(refuse(format!(
            "the release directory must be read-only and owned by uid {}",
            location.uid()
        )));
    }
    let mut manifest = release
        .open_regular_file(Path::new("manifest.json"))
        .map_err(io("open manifest.json"))?;
    let mut bytes = Vec::new();
    manifest
        .file_mut()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io("read manifest.json"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES
        || hex::encode(Sha256::digest(&bytes)) != unit.as_str()
    {
        return Err(refuse(
            "its manifest is not the one its directory is named for".to_owned(),
        ));
    }
    if role == LinuxLaunchRole::UpdateExecutor && !declares_launcher_contract(&bytes) {
        return Err(refuse(format!(
            "it declares no launcher contract {MANAGED_LAUNCHER_CONTRACT}, so its CLI has no \
             update-executor commands"
        )));
    }
    let program = match role {
        LinuxLaunchRole::Daemon => DAEMON_PROGRAM,
        LinuxLaunchRole::Cli | LinuxLaunchRole::UpdateExecutor => CLI_PROGRAM,
    };
    let executable = release
        .open_child_directory(Path::new("bin"))
        .and_then(|bin| bin.open_regular_file(Path::new(program)))
        .map_err(io(&format!("open bin/{program}")))?;
    let executable = executable.metadata();
    if executable.kind() != DirectoryEntryKind::RegularFile
        || executable.mode() & 0o222 != 0
        || executable.mode() & 0o100 == 0
    {
        return Err(refuse(format!(
            "bin/{program} is not a read-only executable"
        )));
    }
    if role == LinuxLaunchRole::Daemon {
        for directory in [&UI_DIRECTORY[..], &EFFECTS_DIRECTORY[..]] {
            let mut current = release
                .open_child_directory(Path::new(directory[0]))
                .map_err(io(&directory.join("/")))?;
            for name in &directory[1..] {
                current = current
                    .open_child_directory(Path::new(name))
                    .map_err(io(&directory.join("/")))?;
            }
        }
    }
    Ok(())
}

/// Whether a release's manifest declares this launcher contract.
///
/// Reads only `managed_package.launcher_contract` and ignores every other
/// field, so a later manifest's additions never stop the launcher.
fn declares_launcher_contract(manifest: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Manifest {
        managed_package: Option<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        launcher_contract: u64,
    }
    serde_json::from_slice::<Manifest>(manifest)
        .ok()
        .and_then(|manifest| manifest.managed_package)
        .is_some_and(|package| package.launcher_contract == u64::from(MANAGED_LAUNCHER_CONTRACT))
}

#[derive(Deserialize)]
struct JournalSelection {
    disposition: String,
    prior_active_unit: Option<String>,
}

/// The prior an unsettled transaction would roll back to.
fn pending_prior(location: &LinuxInstallLocation) -> Result<Option<UnitId>, LinuxLaunchError> {
    let state = ReadOnlyDirectoryAuthority::open(location.state_root())
        .map_err(|source| LinuxLaunchError::Journal(source.to_string()))?;
    let mut journal = match state.open_regular_file(Path::new("install-journal.json")) {
        Ok(journal) => journal,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(LinuxLaunchError::Journal(source.to_string())),
    };
    let limit = MAX_MANAGED_INSTALL_JOURNAL_BYTES as u64;
    let mut bytes = Vec::new();
    journal
        .file_mut()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| LinuxLaunchError::Journal(source.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(LinuxLaunchError::Journal(
            "the journal exceeds its byte bound".to_owned(),
        ));
    }
    let selection: JournalSelection = serde_json::from_slice(&bytes)
        .map_err(|source| LinuxLaunchError::Journal(source.to_string()))?;
    match selection.disposition.as_str() {
        "committed" | "rolled_back" => Ok(None),
        "forward" | "rollback" => selection
            .prior_active_unit
            .map(|unit| {
                UnitId::new(unit).map_err(|source| LinuxLaunchError::Journal(source.to_string()))
            })
            .transpose(),
        other => Err(LinuxLaunchError::Journal(format!(
            "unknown transaction disposition {other:?}"
        ))),
    }
}

/// The recorded roots the daemon must use, spelled as the XDG bases it
/// resolves them from, with its caches kept inside its recorded state.
fn daemon_environment(
    location: &LinuxInstallLocation,
) -> Result<Vec<(OsString, OsString)>, LinuxLaunchError> {
    let base = |root: &Path, suffix: &[&str]| -> Result<PathBuf, LinuxLaunchError> {
        let mut base = root;
        for expected in suffix.iter().rev() {
            if base.file_name().and_then(|name| name.to_str()) != Some(expected) {
                return Err(LinuxLaunchError::Roots(format!(
                    "{} does not end in {}",
                    root.display(),
                    suffix.join("/")
                )));
            }
            base = base.parent().ok_or_else(|| {
                LinuxLaunchError::Roots(format!("{} has no base", root.display()))
            })?;
        }
        Ok(base.to_path_buf())
    };
    let config = base(location.config_root(), &["hypercolor"])?;
    let data = base(location.data_root(), &["hypercolor"])?;
    let state = base(location.state_root(), &["hypercolor", "update"])?;
    Ok(vec![
        ("XDG_CONFIG_HOME".into(), config.into()),
        ("XDG_DATA_HOME".into(), data.into()),
        ("XDG_STATE_HOME".into(), state.into()),
        (
            "XDG_CACHE_HOME".into(),
            location.daemon_state_root().join("cache").into(),
        ),
    ])
}
