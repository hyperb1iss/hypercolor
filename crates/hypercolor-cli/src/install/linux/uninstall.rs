//! Remove a raw per-user Linux installation through its recorded authority.
//!
//! Uninstall follows the permanent locator, never the caller's environment.
//! It settles any interrupted transaction first, removes only the service,
//! launcher and public layout entries this installer generates, then removes
//! the release root, the update state root and the historical locator root.
//! Application data beside the release root and the configuration root are
//! preserved. Every step is replayable: a run interrupted anywhere continues
//! from whatever state the previous run left.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::super::{
    InstallLock, InstallOutcome, InstallPlatformError, InstallStore, MAX_INSTALL_JOURNAL_BYTES,
    MAX_MANAGED_INSTALL_JOURNAL_BYTES, OwnershipPolicy,
};
use super::command::{LinuxInstallCommandError, PlatformInputs, pending, platform_with, recover};
use super::model::{
    LINUX_LAYOUT_ITEMS, LinuxExactEntry, LinuxLayoutItem, MAX_LAUNCHER_BYTES,
    MAX_SYSTEMD_SHOW_BYTES, parse_systemd_show,
};
use super::proof::{layout_target_for, render_launcher};
use super::{
    LinuxInstallAuthority, LinuxInstallExecutor, LinuxInstallLocation, LinuxInstallLocator,
    LinuxPublicTree,
};

/// Durable boundaries of one uninstall run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxUninstallCheckpoint {
    /// Authority is elected and any interrupted transaction is settled.
    Settled,
    /// The generated service, launcher and public layout entries are gone.
    PlatformRemoved,
    /// The recorded release root is gone.
    ReleasesRemoved,
    /// The recorded update state root is gone.
    StateRemoved,
}

/// Process-dependent inputs to [`run_linux_uninstall`].
pub trait LinuxUninstallHost {
    /// The platform executor bound to one elected store.
    type Executor: LinuxInstallExecutor;

    /// Bind a platform executor to the elected store and lock.
    ///
    /// # Errors
    /// Returns an error when the executor cannot retain its authority.
    fn executor(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
        tree: LinuxPublicTree,
    ) -> Result<Self::Executor, InstallPlatformError>;

    /// Observe one durable boundary.
    ///
    /// # Errors
    /// An error stops the run at this boundary without further writes.
    fn checkpoint(
        &mut self,
        checkpoint: LinuxUninstallCheckpoint,
    ) -> Result<(), InstallPlatformError> {
        let _ = checkpoint;
        Ok(())
    }
}

/// What one uninstall run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxUninstallRun {
    /// The outcome of an interrupted transaction settled before removal.
    pub recovered: Option<InstallOutcome>,
    /// Installer-owned paths removed by this run.
    pub removed: Vec<PathBuf>,
    /// User data and configuration roots left in place.
    pub preserved: Vec<PathBuf>,
}

/// Uninstall the raw per-user Linux installation recorded under `home`.
///
/// Returns an empty run when no installation authority exists. Refuses before
/// any write when the service, launcher or public layout holds entries this
/// installer did not generate.
///
/// # Errors
/// Returns the first refusal; a later run resumes from the durable state.
pub fn run_linux_uninstall<H: LinuxUninstallHost>(
    home: &Path,
    ownership: &OwnershipPolicy,
    host: &mut H,
) -> Result<LinuxUninstallRun, LinuxInstallCommandError> {
    let lib = home.join(".local/lib");
    let legacy_root = lib.join("hypercolor");
    match std::fs::symlink_metadata(&legacy_root) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LinuxUninstallRun::default());
        }
        Err(error) => return Err(InstallPlatformError::new(error.to_string()).into()),
    }
    let old = InstallStore::new(&legacy_root, MAX_INSTALL_JOURNAL_BYTES)
        .with_ownership_policy(ownership.clone());
    let mut old_lock = old.acquire_anchored_lock(home)?;
    let locator =
        LinuxInstallLocator::retain(home, &old_lock).map_err(LinuxInstallCommandError::Election)?;
    let authority = locator.read().map_err(LinuxInstallCommandError::Election)?;
    // Roots an interrupted adoption prepared before the locator published.
    let intent = match &authority {
        LinuxInstallAuthority::Legacy(_) => locator
            .adoption_intent()
            .map_err(LinuxInstallCommandError::Election)?,
        LinuxInstallAuthority::Managed(_) => None,
    };
    drop(locator);
    // Prove every removal parent before the first write, so an unsafe
    // ancestor refuses the whole uninstall instead of stranding it midway.
    require_trusted_path(&old_lock, &lib)?;
    let recorded = match &authority {
        LinuxInstallAuthority::Managed(location) => Some(location),
        LinuxInstallAuthority::Legacy(_) => intent.as_ref(),
    };
    if let Some(location) = recorded {
        require_trusted_path(&old_lock, location.data_root())?;
        if let Some(container) = location.state_root().parent() {
            require_trusted_path(&old_lock, container)?;
        }
    }
    let mut run = LinuxUninstallRun::default();
    match authority {
        LinuxInstallAuthority::Legacy(journal) => {
            if journal.as_ref().is_some_and(pending) {
                run.recovered = Some(settle(home, host, &old, &mut old_lock, false)?);
            }
            stop(host, LinuxUninstallCheckpoint::Settled)?;
            remove_platform(home, host, &old, &old_lock, &[old.active_path()])?;
            stop(host, LinuxUninstallCheckpoint::PlatformRemoved)?;
            if let Some(prepared) = &intent {
                remove_prepared_roots(&old_lock, prepared, &mut run)?;
            }
        }
        LinuxInstallAuthority::Managed(location) => {
            uninstall_managed(home, host, &old, &mut old_lock, &location, &mut run)?;
            for root in [location.data_root(), location.config_root()] {
                if root.exists() {
                    run.preserved.push(root.to_path_buf());
                }
            }
        }
    }
    // Removing the historical root removes the locator, which ends managed
    // authority; its units and lock go with it.
    if remove_child_tree(&old_lock, &lib, "hypercolor")? {
        run.removed.push(legacy_root);
    }
    Ok(run)
}

fn uninstall_managed<H: LinuxUninstallHost>(
    home: &Path,
    host: &mut H,
    old: &InstallStore,
    old_lock: &mut InstallLock,
    location: &LinuxInstallLocation,
    run: &mut LinuxUninstallRun,
) -> Result<(), LinuxInstallCommandError> {
    let state_container = location
        .state_root()
        .parent()
        .ok_or_else(|| InstallPlatformError::new("recorded state root has no parent"))?;
    let state_name = file_name(location.state_root())?;
    let active_roots = [location.release_root().join("active"), old.active_path()];
    // Removal always opens parents through the historical lock, which is held
    // for the whole run. While both recorded roots exist the managed state
    // lock is held as well, taken second as election requires. Once either
    // root is gone no managed installer can elect, so the historical lock
    // alone guards the remainder.
    let state_lock = if location.state_root().exists() && location.release_root().exists() {
        let store = InstallStore::with_roots(
            location.release_root(),
            location.state_root(),
            MAX_MANAGED_INSTALL_JOURNAL_BYTES,
        )?
        .with_ownership_policy(old.ownership_policy().clone());
        let mut lock = store.acquire_lock()?;
        if store.load_journal(&lock)?.as_ref().is_some_and(pending) {
            run.recovered = Some(settle(home, host, &store, &mut lock, true)?);
        }
        stop(host, LinuxUninstallCheckpoint::Settled)?;
        remove_platform(home, host, &store, &lock, &active_roots)?;
        Some(lock)
    } else {
        stop(host, LinuxUninstallCheckpoint::Settled)?;
        remove_platform(home, host, old, old_lock, &active_roots)?;
        None
    };
    stop(host, LinuxUninstallCheckpoint::PlatformRemoved)?;
    if remove_child_tree(old_lock, location.data_root(), "releases")? {
        run.removed.push(location.release_root().to_path_buf());
    }
    stop(host, LinuxUninstallCheckpoint::ReleasesRemoved)?;
    if remove_child_tree(old_lock, state_container, state_name)? {
        run.removed.push(location.state_root().to_path_buf());
    }
    drop(state_lock);
    stop(host, LinuxUninstallCheckpoint::StateRemoved)?;
    if remove_empty_container(old_lock, state_container)? {
        run.removed.push(state_container.to_path_buf());
    }
    Ok(())
}

/// Remove roots an unpublished adoption prepared; they never held authority.
///
/// The historical lock is held, so no installer can resume that adoption
/// while its roots go. The unpublished journal is never settled.
fn remove_prepared_roots(
    old_lock: &InstallLock,
    location: &LinuxInstallLocation,
    run: &mut LinuxUninstallRun,
) -> Result<(), LinuxInstallCommandError> {
    if remove_child_tree(old_lock, location.data_root(), "releases")? {
        run.removed.push(location.release_root().to_path_buf());
    }
    let container = location
        .state_root()
        .parent()
        .ok_or_else(|| InstallPlatformError::new("recorded state root has no parent"))?;
    if remove_child_tree(old_lock, container, file_name(location.state_root())?)? {
        run.removed.push(location.state_root().to_path_buf());
    }
    if remove_empty_container(old_lock, container)? {
        run.removed.push(container.to_path_buf());
    }
    for root in [location.data_root(), location.config_root()] {
        if root.exists() {
            run.preserved.push(root.to_path_buf());
        }
    }
    Ok(())
}

fn settle<H: LinuxUninstallHost>(
    home: &Path,
    host: &mut H,
    store: &InstallStore,
    lock: &mut InstallLock,
    managed: bool,
) -> Result<InstallOutcome, LinuxInstallCommandError> {
    let journal = store
        .load_journal(lock)?
        .ok_or(LinuxInstallCommandError::MissingJournal)?;
    let mut platform = platform_with(
        home,
        |store, lock, tree| host.executor(store, lock, tree),
        store,
        lock,
        PlatformInputs {
            candidate: None,
            journal: Some(&journal),
            managed,
            original: None,
        },
    )?;
    Ok(recover(store, lock, &mut platform)?.outcome)
}

/// Remove the generated service, launcher and public layout entries.
///
/// Every observed entry must be absent or exactly what this installer renders
/// for one of `active_roots`. Anything else refuses the whole removal before
/// the first write.
fn remove_platform<H: LinuxUninstallHost>(
    home: &Path,
    host: &mut H,
    store: &InstallStore,
    lock: &InstallLock,
    active_roots: &[PathBuf],
) -> Result<(), LinuxInstallCommandError> {
    let tree = LinuxPublicTree::new(lock, home)?;
    let direct_fragment = home
        .join(".config/systemd/user/hypercolor.service")
        .to_str()
        .ok_or_else(|| InstallPlatformError::new("Linux HOME must be exact UTF-8"))?
        .to_owned();
    let mut executor = host.executor(store, lock, tree)?;
    let systemd = parse_systemd_show(&executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
    let (launcher, launcher_bytes) = executor.launcher_entry(MAX_LAUNCHER_BYTES)?;
    let mut layout = BTreeMap::new();
    for item in LINUX_LAYOUT_ITEMS {
        layout.insert(item, executor.layout_entry(item)?);
    }

    let mut foreign = Vec::new();
    let loaded = systemd.load_state == "loaded";
    if loaded && systemd.fragment_path != direct_fragment {
        foreign.push(format!(
            "hypercolor.service is loaded from {}",
            systemd.fragment_path
        ));
    }
    if !owned_launcher(&launcher, &launcher_bytes, active_roots)? {
        foreign.push(direct_fragment.clone());
    }
    for (item, entry) in &layout {
        if !owned_layout(*item, entry, active_roots) {
            foreign.push(format!("public layout entry {item:?}"));
        }
    }
    if !foreign.is_empty() {
        return Err(LinuxInstallCommandError::ForeignInstallation(foreign));
    }

    if loaded && systemd.active_state == "active" {
        executor.set_runtime(false)?;
    }
    if loaded && systemd.unit_file_state == "enabled" {
        executor.set_autostart(false)?;
    }
    for (item, entry) in &layout {
        if !matches!(entry, LinuxExactEntry::Absent) {
            executor.replace_layout(*item, entry, None)?;
        }
    }
    let launcher_present = !matches!(launcher, LinuxExactEntry::Absent);
    if launcher_present {
        executor.replace_launcher(&launcher, None)?;
    }
    if launcher_present || loaded {
        executor.reload_manager()?;
    }
    Ok(())
}

fn owned_launcher(
    launcher: &LinuxExactEntry,
    bytes: &[u8],
    active_roots: &[PathBuf],
) -> Result<bool, InstallPlatformError> {
    match launcher {
        LinuxExactEntry::Absent => Ok(true),
        LinuxExactEntry::Symlink { .. } => Ok(false),
        LinuxExactEntry::RegularFile { mode, .. } => {
            for root in active_roots {
                let rendered = render_launcher(root)?;
                if *mode == rendered.mode && bytes == rendered.bytes.as_slice() {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn owned_layout(item: LinuxLayoutItem, entry: &LinuxExactEntry, active_roots: &[PathBuf]) -> bool {
    match entry {
        LinuxExactEntry::Absent => true,
        LinuxExactEntry::RegularFile { .. } => false,
        LinuxExactEntry::Symlink { target } => active_roots
            .iter()
            .any(|root| *target == layout_target_for(root, item)),
    }
}

fn remove_child_tree(
    lock: &InstallLock,
    parent: &Path,
    name: &str,
) -> Result<bool, LinuxInstallCommandError> {
    let parent_path = parent;
    let parent = match lock.open_public_directory(parent) {
        Ok(parent) => parent,
        Err(error) if not_found(&error) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    require_trusted_path(lock, parent_path)?;
    parent
        .durable_remove_child_tree(Path::new(name))
        .map_err(|error| InstallPlatformError::new(format!("failed to remove {name}: {error}")))
        .map_err(Into::into)
}

/// Remove the installer-created state container only when nothing else lives
/// there.
fn remove_empty_container(
    lock: &InstallLock,
    container: &Path,
) -> Result<bool, LinuxInstallCommandError> {
    let Some(parent) = container.parent() else {
        return Ok(false);
    };
    let name = file_name(container)?;
    let directory = match lock.open_public_directory(container) {
        Ok(directory) => directory,
        Err(error) if not_found(&error) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let empty = directory
        .into_directory_authority()
        .and_then(|directory| directory.entries())
        .map_err(|error| InstallPlatformError::new(error.to_string()))?
        .is_empty();
    if !empty {
        return Ok(false);
    }
    remove_child_tree(lock, parent, name)
}

/// Refuse removal beneath a directory another principal could rename.
///
/// `path` and every directory above it, except `/`, must be root-owned and
/// writable by nobody else, or owned by the installing user under the
/// ancestor rule.
fn require_trusted_path(lock: &InstallLock, path: &Path) -> Result<(), LinuxInstallCommandError> {
    for directory in path.ancestors() {
        if directory.parent().is_none() {
            continue;
        }
        let authority = match lock.open_public_directory(directory) {
            Ok(authority) => authority,
            // An interrupted run may already have removed the deeper part.
            Err(error) if not_found(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        let metadata = authority
            .metadata()
            .map_err(|error| InstallPlatformError::new(error.to_string()))?;
        lock.ownership_policy()
            .require_trusted_ancestor(&authority, metadata)
            .map_err(|refusal| {
                LinuxInstallCommandError::UnsafeDirectory(directory.to_path_buf(), refusal)
            })?;
    }
    Ok(())
}

fn file_name(path: &Path) -> Result<&str, InstallPlatformError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| InstallPlatformError::new("recorded root has no UTF-8 final component"))
}

fn not_found(error: &super::super::InstallStoreError) -> bool {
    matches!(
        error,
        super::super::InstallStoreError::OpenPublicDirectory(source)
            if source.kind() == io::ErrorKind::NotFound
    )
}

fn stop<H: LinuxUninstallHost>(
    host: &mut H,
    checkpoint: LinuxUninstallCheckpoint,
) -> Result<(), LinuxInstallCommandError> {
    host.checkpoint(checkpoint)
        .map_err(|source| LinuxInstallCommandError::UninstallStopped(checkpoint, source))
}
