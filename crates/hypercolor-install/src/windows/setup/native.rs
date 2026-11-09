//! The machine an installer runs on.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess};
use hypercolor_windows_install_native::{desktop_user_sid, own_new_objects_as_administrators};
use hypercolor_windows_pawnio::prepare_machine_update_directory;

use super::{PendingTransaction, WindowsSetupHost, run_windows_setup};
use crate::windows::machine_layout::{
    BootstrapSlot, WINDOWS_BOOTSTRAP_PROGRAM, WindowsInstallLayout,
};
use crate::windows::model::{WindowsAppStart, WindowsInstallConfig, error};
use crate::windows::payload::{retain_installed_release_unit, stage_release_payload};
use crate::windows::session::WindowsSessionHost;
use crate::windows::store::{InstallLock, InstallStore, InstallStoreError, replace_durably};
use crate::windows::update_service::{
    PROBE_FLAG, REGISTER_SERVICE_FLAG, ScmUpdateService, run_slot,
};
use crate::windows::{
    WindowsInstallExecutor as _, WindowsInstallPlatform, WindowsInstallation, WindowsNativeExecutor,
};
use crate::{
    InstallCoordinator, InstallDisposition, InstallJournalV1, InstallOutcome, InstallPlatformError,
    InstallRequest, InstallTargetPolicy, InstallTransactionId, MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    UnitId, UnitRecord,
};

/// `CREATE_NO_WINDOW`: an installer has no console to lend.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Never asked: an installer's transactions prove nothing over the API.
const UNUSED_DAEMON_API: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// What one run of the installer lays down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsInstallRequest {
    /// The release root, `%ProgramFiles%\Hypercolor`.
    pub install_dir: PathBuf,
    /// The extracted unit, which may stand anywhere: every byte is proven
    /// against `unit` as it is copied into the store.
    pub source: PathBuf,
    /// The digest compiled into the installer.
    pub unit: UnitId,
    /// This build ships the updater.
    pub managed_update: bool,
}

/// Take this machine to the release in `request`, and return a note for
/// anything that stayed behind.
///
/// # Errors
///
/// Returns the first step that failed. Running the installer again resumes
/// there.
pub fn install_windows_release(
    request: &WindowsInstallRequest,
) -> Result<Vec<String>, InstallPlatformError> {
    let mut host = NativeSetup::prepare(request)?;
    run_windows_setup(&mut host, &request.unit, request.managed_update)
}

fn io_failure(context: &str, source: impl std::fmt::Display) -> InstallPlatformError {
    error(format!("{context}: {source}"))
}

/// `path` as the file system spells it, so a short name or a verbatim
/// prefix cannot read as a different directory to a handle's final path.
/// Only for the unit's source, whose bytes are proven whatever it is. The
/// release root stays as given: the installer goes on to write through that
/// exact path, so a link in it has to be refused, not resolved.
fn canonical(path: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    Ok(resolved
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .map_or_else(|| resolved.clone(), PathBuf::from))
}

/// Create the release root if it is new and make Administrators its owner.
/// An elevated account can create it under `Program Files` but cannot hold
/// what a directory handle for writing asks of that parent, so both happen
/// by path, after the store proved no link or replaceable ancestor is on it.
fn take_release_root(root: &Path) -> Result<(), InstallPlatformError> {
    match fs::create_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(io_failure("create the install directory", source)),
    }
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| error("SystemRoot is not set"))?;
    let status = Command::new(system.join("System32").join("takeown.exe"))
        .arg("/A")
        .arg("/F")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|source| io_failure("run takeown", source))?;
    if status.success() {
        Ok(())
    } else {
        Err(error(format!(
            "takeown could not give Administrators {}: {status}",
            root.display()
        )))
    }
}

/// An installer reaches nobody's session. It ends what still runs by force,
/// and only finishes transactions that start nothing.
struct InstallerSession;

impl WindowsSessionHost for InstallerSession {
    fn ask_app_to_quit(&mut self) -> bool {
        false
    }

    fn start_app(
        &mut self,
        _app: &str,
        _arguments: &[&str],
    ) -> Result<WindowsAppStart, InstallPlatformError> {
        Ok(WindowsAppStart::NoSession)
    }
}

struct NativeSetup {
    store: InstallStore,
    lock: InstallLock,
    layout: WindowsInstallLayout,
    source: PathBuf,
    unit: UnitId,
    candidate: Option<UnitRecord>,
}

impl NativeSetup {
    /// Bring both roots to what the store accepts and take its lock.
    fn prepare(request: &WindowsInstallRequest) -> Result<Self, InstallPlatformError> {
        own_new_objects_as_administrators()
            .map_err(|source| io_failure("take over the owner of new files", source))?;
        let state_root = prepare_machine_update_directory()
            .map_err(|source| io_failure("prepare the update state directory", source))?;
        let source = canonical(&request.source)
            .map_err(|source| io_failure("resolve the unit's location", source))?;
        let store = InstallStore::with_roots(
            &request.install_dir,
            &state_root,
            MAX_MANAGED_INSTALL_JOURNAL_BYTES,
        )
        .map_err(|source| io_failure("open the release store", source))?;
        store
            .require_safe_release_path()
            .map_err(|source| io_failure("check the install directory", source))?;
        take_release_root(&request.install_dir)?;
        let lock = store.acquire_lock().map_err(|source| match source {
            InstallStoreError::LockContended => {
                error("an update of this installation is running; try again once it has finished")
            }
            source => io_failure("open the release store", source),
        })?;
        Ok(Self {
            layout: WindowsInstallLayout::at(store.root(), store.state_root()),
            store,
            lock,
            source,
            unit: request.unit.clone(),
            candidate: None,
        })
    }

    fn store_failure(source: InstallStoreError) -> InstallPlatformError {
        io_failure("read the release store", source)
    }

    fn executor(&self) -> WindowsNativeExecutor {
        WindowsNativeExecutor::new(self.store.clone(), UNUSED_DAEMON_API)
    }

    /// A platform that knows the active unit, both units of an open
    /// journal, and `candidate`.
    fn platform(
        &self,
        candidate: Option<UnitRecord>,
    ) -> Result<WindowsInstallPlatform<WindowsNativeExecutor, InstallerSession>, InstallPlatformError>
    {
        let mut units = candidate.into_iter().collect::<Vec<_>>();
        let active = self
            .store
            .active_unit(&self.lock)
            .map_err(Self::store_failure)?;
        let journal = self
            .store
            .load_journal(&self.lock)
            .map_err(Self::store_failure)?
            .filter(|journal| {
                matches!(
                    journal.disposition,
                    InstallDisposition::Forward | InstallDisposition::Rollback
                )
            });
        let open = journal.iter().flat_map(InstallJournalV1::named_units);
        for id in active.into_iter().chain(open) {
            if units.iter().any(|unit| unit.id() == &id) {
                continue;
            }
            units.push(
                retain_installed_release_unit(&self.store, &self.lock, &id)
                    .map_err(|source| io_failure("bind an installed unit", source))?,
            );
        }
        let units_root = self
            .store
            .root()
            .join("units")
            .to_str()
            .ok_or_else(|| error("the install directory is not Unicode"))?
            .to_owned();
        WindowsInstallPlatform::new(
            self.executor(),
            InstallerSession,
            WindowsInstallConfig::new(units_root),
            units,
        )
    }

    fn bootstrap(&self) -> Result<Vec<u8>, InstallPlatformError> {
        let program = self
            .store
            .unit_path(&self.unit)
            .join(WINDOWS_BOOTSTRAP_PROGRAM.replace('/', "\\"));
        fs::read(&program).map_err(|source| io_failure("read the unit's bootstrap", source))
    }

    fn bootstrap_directory(&self) -> Result<WindowsDirectory, InstallPlatformError> {
        self.store
            .bootstrap_directory(&self.lock)
            .map_err(|source| io_failure("open the bootstrap directory", source))
    }
}

impl WindowsSetupHost for NativeSetup {
    fn pending_transaction(&mut self) -> Result<Option<PendingTransaction>, InstallPlatformError> {
        let journal = self
            .store
            .load_journal(&self.lock)
            .map_err(Self::store_failure)?;
        Ok(journal
            .filter(|journal| {
                matches!(
                    journal.disposition,
                    InstallDisposition::Forward | InstallDisposition::Rollback
                )
            })
            .map(|journal| {
                if journal.prior_platform.running_unit.is_none()
                    && journal.target_platform.running_unit.is_none()
                {
                    PendingTransaction::StartsNothing
                } else {
                    PendingTransaction::StartsARelease
                }
            }))
    }

    fn finish_transaction(&mut self) -> Result<(), InstallPlatformError> {
        let mut platform = self.platform(None)?;
        InstallCoordinator::new(&self.store, &mut platform)
            .recover_with_lock(&mut self.lock)
            .map(drop)
            .map_err(|source| io_failure("finish the open transaction", source))
    }

    fn stage_candidate(&mut self) -> Result<(), InstallPlatformError> {
        let source = WindowsDirectory::open(&self.source, WindowsDirectoryAccess::Read)
            .map_err(|source| io_failure("open the unit", source))?;
        let unit = stage_release_payload(&self.store, &self.lock, &source, &self.unit)
            .map_err(|source| io_failure("publish the unit", source))?;
        self.candidate = Some(unit);
        Ok(())
    }

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        self.store
            .active_unit(&self.lock)
            .map_err(Self::store_failure)
    }

    fn install_candidate(&mut self) -> Result<(), InstallPlatformError> {
        let candidate = self
            .candidate
            .clone()
            .ok_or_else(|| error("no unit was published"))?;
        let transaction_id =
            InstallTransactionId::new(format!("release-{}", &self.unit.as_str()[..56]))
                .map_err(|source| io_failure("name the transaction", source))?;
        let mut platform = self.platform(Some(candidate.clone()))?;
        let outcome = InstallCoordinator::new(&self.store, &mut platform)
            .install_with_lock(
                InstallRequest {
                    transaction_id,
                    candidate,
                    target_policy: InstallTargetPolicy::Disabled,
                },
                &mut self.lock,
            )
            .map_err(|source| io_failure("install the unit", source))?;
        match outcome {
            InstallOutcome::Committed { active_unit } if active_unit == self.unit => Ok(()),
            InstallOutcome::Committed { .. } => Err(error("another transaction committed")),
            InstallOutcome::RolledBack { failure, .. } => {
                Err(error(format!("the install rolled back: {failure}")))
            }
        }
    }

    fn installation_recorded(&mut self) -> Result<bool, InstallPlatformError> {
        self.store
            .installation(&self.lock)
            .map(|record| record.is_some())
            .map_err(Self::store_failure)
    }

    fn record_installation(&mut self) -> Result<(), InstallPlatformError> {
        let user =
            desktop_user_sid().map_err(|source| io_failure("name the desktop's user", source))?;
        let installation = WindowsInstallation::new(&user).map_err(error)?;
        self.store
            .record_installation(&installation, &self.lock)
            .map_err(|source| io_failure("write the installation's record", source))
    }

    fn update_service_slot(&mut self) -> Result<Option<BootstrapSlot>, InstallPlatformError> {
        self.executor().observe_bootstrap()
    }

    fn prepare_coordinator_directory(&mut self) -> Result<(), InstallPlatformError> {
        let installation = self
            .store
            .installation(&self.lock)
            .map_err(Self::store_failure)?
            .ok_or_else(|| error("the installation is not recorded"))?;
        self.store
            .prepare_coordinator_directory(&self.lock, installation.user_sid())
            .map_err(|source| io_failure("give the user its records' directory", source))
    }

    fn write_bootstrap_slots(&mut self) -> Result<(), InstallPlatformError> {
        let bytes = self.bootstrap()?;
        let directory = self.bootstrap_directory()?;
        for slot in [BootstrapSlot::A, BootstrapSlot::B] {
            replace_durably(&directory, slot.file_name(), &bytes)
                .map_err(|source| io_failure("write a bootstrap slot", source))?;
        }
        Ok(())
    }

    fn probe_bootstrap(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError> {
        run_slot(&self.layout, slot, PROBE_FLAG)
            .map_err(|source| io_failure("prove a bootstrap loads", source))
    }

    fn register_update_service(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError> {
        run_slot(&self.layout, slot, REGISTER_SERVICE_FLAG)
            .map_err(|source| io_failure("register the update service", source))
    }

    fn remove_update_service(&mut self) -> Result<(), InstallPlatformError> {
        ScmUpdateService::remove()
            .map_err(|source| io_failure("remove the update service", source))?;
        self.store
            .remove_bootstrap_directory(&self.lock)
            .map_err(|source| io_failure("remove the bootstrap slots", source))
    }

    fn collect_units(&mut self) -> Result<Vec<String>, InstallPlatformError> {
        let collection = self
            .store
            .collect_units(&self.lock, &[])
            .map_err(Self::store_failure)?;
        Ok(collection
            .refused
            .into_iter()
            .map(|(path, reason)| format!("{} stayed behind: {reason}", path.display()))
            .collect())
    }
}
