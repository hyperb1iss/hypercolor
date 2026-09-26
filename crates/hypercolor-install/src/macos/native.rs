use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosDirectLaunchdExecutableExpectation, MacosDirectLaunchdInspector,
    MacosDirectLaunchdMutationOutcome, MacosDirectLaunchdMutator,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState, MacosOwnerRecord,
    MacosOwnerStore, NativeMacosDirectLaunchdInspector, NativeMacosDirectLaunchdMutator,
    canonical_macos_daemon_guard_path, corroborate_direct_launchd_owner,
    validate_retained_macos_executable, wait_for_exact_direct_launchd_publication,
    wait_for_macos_guard_release,
};
use hypercolor_platform_fs::{DirectoryAuthority, ExactEntry, PublicDirectoryAuthority};

use super::super::{InstallLock, InstallPlatformError, InstallStore, UnitId, UnitRecord};
use super::executor::MacosInstallExecutor;
use super::launcher_store::MacosLauncherStore;
use super::model::{
    MAX_LEGACY_FILE_BYTES, MacosCandidateLayout, MacosDirectoryState, MacosEntryPublication,
    MacosExactEntry, MacosFilePublication, MacosInstallConfig, MacosLaunchdObservation,
    MacosLauncherSnapshot, MacosLegacyExecutable, MacosLegacySnapshot, MacosMutationOutcome,
    MacosPublicSnapshot, MacosRuntimeExecutable, MacosRuntimeTransition, error,
};
use super::public_tree::MacosPublicTree;

const MUTATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct MacosNativeExecutor<
    M = NativeMacosDirectLaunchdMutator,
    I = NativeMacosDirectLaunchdInspector,
> {
    pub(super) config: MacosInstallConfig,
    pub(super) home_path: PathBuf,
    pub(super) install_prefix: PathBuf,
    pub(super) install_dir: PathBuf,
    pub(super) active: PublicDirectoryAuthority,
    pub(super) units: DirectoryAuthority,
    pub(super) units_root_hint: PathBuf,
    pub(super) public_tree: MacosPublicTree,
    pub(super) launcher_store: MacosLauncherStore,
    pub(super) owner_store: MacosOwnerStore,
    pub(super) launchd_mutator: M,
    pub(super) launchd_inspector: I,
}

impl MacosNativeExecutor {
    pub fn new(
        store: &InstallStore,
        lock: &mut InstallLock,
        home: &Path,
        install_prefix: &Path,
        install_dir: &Path,
        owner_data_dir: &Path,
        config: MacosInstallConfig,
    ) -> Result<Self, InstallPlatformError> {
        let owner_store = MacosOwnerStore::new(owner_data_dir);
        Self::new_with_launchd(
            store,
            lock,
            home,
            install_prefix,
            install_dir,
            config,
            owner_store.clone(),
            NativeMacosDirectLaunchdMutator::new(owner_store),
            NativeMacosDirectLaunchdInspector::new(),
        )
    }
}

impl<M: MacosDirectLaunchdMutator, I: MacosDirectLaunchdInspector> MacosNativeExecutor<M, I> {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_launchd(
        store: &InstallStore,
        lock: &mut InstallLock,
        home: &Path,
        install_prefix: &Path,
        install_dir: &Path,
        config: MacosInstallConfig,
        owner_store: MacosOwnerStore,
        launchd_mutator: M,
        launchd_inspector: I,
    ) -> Result<Self, InstallPlatformError> {
        require_below_home(home, install_prefix, "install prefix")?;
        require_below_home(home, install_dir, "install directory")?;
        require_below_home(home, Path::new(&config.direct_plist_path), "launchd plist")?;
        require_below_home(home, &config.log_directory, "log directory")?;
        let owner_data_dir = owner_store
            .owner_record_path()
            .parent()
            .ok_or_else(|| error("macOS owner store has no data directory"))?
            .to_path_buf();
        require_below_home(home, &owner_data_dir, "owner store")?;
        let store_root = lock.open_store_public_directory().map_err(store_error)?;
        let active = lock.open_store_public_directory().map_err(store_error)?;
        let launcher_root = store_root.into_directory_authority().map_err(io_error)?;
        let launcher_store = MacosLauncherStore::new(store.root().to_path_buf(), &launcher_root)?;
        let units = store.units_authority(lock).map_err(store_error)?;
        let home_authority = lock.open_public_directory(home).map_err(store_error)?;
        let public_tree = MacosPublicTree::new(exact_path(home)?, home_authority)?;
        Ok(Self {
            config,
            home_path: home.to_path_buf(),
            install_prefix: install_prefix.to_path_buf(),
            install_dir: install_dir.to_path_buf(),
            active,
            units,
            units_root_hint: store.root().join("units"),
            public_tree,
            launcher_store,
            owner_store,
            launchd_mutator,
            launchd_inspector,
        })
    }

    fn retained_unit(&self, id: &UnitId) -> Result<UnitRecord, InstallPlatformError> {
        let directory = self
            .units
            .open_child_directory(Path::new(id.as_str()))
            .map_err(io_error)?;
        let read_only = directory.read_only().map_err(io_error)?;
        if id.as_str().starts_with("legacy-") {
            super::native_legacy::validate_legacy_snapshot_binding(&read_only, id)?;
        }
        UnitRecord::new(
            id.clone(),
            self.units_root_hint.join(id.as_str()),
            read_only,
        )
        .map_err(io_error)
    }

    fn executable_expectation(
        executable: &MacosRuntimeExecutable,
    ) -> Result<MacosDirectLaunchdExecutableExpectation, InstallPlatformError> {
        MacosDirectLaunchdExecutableExpectation::new(
            &executable.path,
            &executable.designated_requirement,
            &executable.designated_requirement_sha256,
            &executable.cdhash,
            &executable.sha256,
            executable.mode,
            executable.size,
            executable.device,
            executable.inode,
        )
        .map_err(owner_store_error)
    }

    fn map_outcome<T>(outcome: MacosDirectLaunchdMutationOutcome<T>) -> MacosMutationOutcome {
        match outcome {
            MacosDirectLaunchdMutationOutcome::Complete(_) => MacosMutationOutcome::Complete,
            MacosDirectLaunchdMutationOutcome::SubmittedUnknown => {
                MacosMutationOutcome::SubmittedUnknown
            }
        }
    }
}

impl<M: MacosDirectLaunchdMutator, I: MacosDirectLaunchdInspector> MacosInstallExecutor
    for MacosNativeExecutor<M, I>
{
    fn validate_topology(
        &mut self,
        config: &MacosInstallConfig,
    ) -> Result<(), InstallPlatformError> {
        if config != &self.config
            || config.immutable_units_root != self.units_root_hint
            || config.active_root
                != self
                    .units_root_hint
                    .parent()
                    .ok_or_else(|| error("native macOS units root has no parent"))?
                    .join("active")
        {
            return Err(error(
                "macOS platform config does not match native authorities",
            ));
        }
        self.active.metadata().map_err(io_error)?;
        Ok(())
    }

    fn validate_unit_authority(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError> {
        if self.retained_unit(unit.id())? != *unit {
            return Err(error(
                "macOS unit does not belong to native units authority",
            ));
        }
        Ok(())
    }

    fn validate_unit_executable(
        &mut self,
        unit: &UnitRecord,
        executable: &MacosRuntimeExecutable,
    ) -> Result<(), InstallPlatformError> {
        if executable.synthetic_legacy || executable.unit != *unit.id() {
            return Err(error(
                "macOS candidate executable authority is inconsistent",
            ));
        }
        let mut opened =
            super::proof::open_retained_file(unit, super::model::DAEMON_RELATIVE_PATH)?;
        let expectation = Self::executable_expectation(executable)?;
        if !validate_retained_macos_executable(opened.file_mut(), &expectation, MUTATION_TIMEOUT)
            .map_err(owner_error)?
        {
            return Err(error(
                "macOS retained candidate failed exact signed-code validation",
            ));
        }
        Ok(())
    }

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        let exact = self
            .active
            .observe_entry(Path::new("active"))
            .map_err(io_error)?;
        let ExactEntry::Symlink { target, .. } = exact else {
            return if matches!(exact, ExactEntry::Absent) {
                Ok(None)
            } else {
                Err(error(
                    "macOS active unit entry is not an exact symbolic link",
                ))
            };
        };
        parse_active_target(&target).map(Some)
    }

    fn launchd_observation(&mut self) -> Result<MacosLaunchdObservation, InstallPlatformError> {
        let before = self
            .launchd_inspector
            .inspect_direct_launchd()
            .map_err(owner_error)?;
        let autostart_enabled = self
            .launchd_mutator
            .autostart_enabled()
            .map_err(owner_error)?;
        let after = self
            .launchd_inspector
            .inspect_direct_launchd()
            .map_err(owner_error)?;
        if before != after {
            return Err(error("launchd runtime changed around exact observation"));
        }
        let pid = match before {
            MacosDirectLaunchdState::NotLoaded => None,
            MacosDirectLaunchdState::Loaded { pid } => Some(pid),
        };
        Ok(MacosLaunchdObservation {
            pid,
            autostart_enabled,
        })
    }

    fn owner_record(&mut self) -> Result<Option<MacosOwnerRecord>, InstallPlatformError> {
        self.owner_store
            .load_owner_record()
            .map_err(owner_store_error)
    }

    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(MacosExactEntry, Vec<u8>), InstallPlatformError> {
        let (entry, bytes) = self
            .public_tree
            .entry(&self.config.direct_plist_path, max_bytes as u64)?;
        Ok((entry, bytes.unwrap_or_default()))
    }

    fn public_snapshot(
        &mut self,
        layouts: &[MacosCandidateLayout],
    ) -> Result<MacosPublicSnapshot, InstallPlatformError> {
        for layout in layouts {
            self.public_tree.bind_paths(
                layout.directories.clone(),
                layout.entries.iter().map(|entry| entry.0.clone()),
            )?;
        }
        super::native_inventory::bind_live_inventory(
            &mut self.public_tree,
            &self.home_path,
            &self.install_prefix,
            &self.install_dir,
        )?;
        self.public_tree.snapshot(MAX_LEGACY_FILE_BYTES)
    }

    fn bind_public_inventory(
        &mut self,
        directories: &[String],
        entries: &[String],
    ) -> Result<(), InstallPlatformError> {
        self.public_tree
            .bind_paths(directories.iter().cloned(), entries.iter().cloned())
    }

    fn candidate_layout(
        &mut self,
        unit: &UnitRecord,
    ) -> Result<MacosCandidateLayout, InstallPlatformError> {
        let layout = super::native_layout::candidate_layout(
            unit,
            &self.home_path,
            &self.install_prefix,
            &self.install_dir,
            &self.config.active_root,
            Path::new(&self.config.direct_plist_path),
            &self.config.log_directory,
        )?;
        self.public_tree.bind_paths(
            layout.directories.clone(),
            layout.entries.iter().map(|entry| entry.0.clone()),
        )?;
        Ok(layout)
    }

    fn inspect_legacy_executable(
        &mut self,
        owner: Option<&MacosOwnerRecord>,
    ) -> Result<Option<MacosLegacyExecutable>, InstallPlatformError> {
        if self.active_unit()?.is_some() {
            return Ok(None);
        }
        let expected_path = self.install_dir.join("hypercolor-daemon");
        if let Some(owner) = owner
            && (owner.active_owner != MacosDaemonOwner::DirectLaunchd
                || owner.active_identity.executable_path != expected_path)
        {
            return Err(error(
                "macOS legacy owner is outside the raw install topology",
            ));
        }
        let path = expected_path
            .to_str()
            .ok_or_else(|| error("macOS legacy executable path is not exact UTF-8"))?
            .to_owned();
        self.public_tree
            .bind_paths(std::iter::empty(), std::iter::once(path.clone()))?;
        let (entry, _) = self.public_tree.entry(&path, MAX_LEGACY_FILE_BYTES)?;
        if !matches!(entry, MacosExactEntry::RegularFile { .. }) {
            return if owner.is_none() {
                Ok(None)
            } else {
                Err(error(
                    "running macOS legacy executable is not a regular file",
                ))
            };
        }
        let (mut opened, bytes) = self
            .public_tree
            .retained_regular_file(&path, MAX_LEGACY_FILE_BYTES)?;
        let before = opened.metadata();
        if before.link_count() != 1 || before.size() == 0 || before.mode() & 0o222 != 0 {
            return Err(error("macOS legacy executable metadata is unsafe"));
        }
        let requirement = super::native_identity::codesign_requirement(Path::new(&path))?;
        let requirement_sha = super::model::hex_digest(requirement.as_bytes());
        let cdhash = super::thin_macho_cdhash(opened.file_mut(), before.size())?;
        if owner.is_some_and(|owner| {
            requirement_sha != owner.active_identity.designated_requirement_hash
        }) {
            return Err(error(
                "macOS legacy executable signature changed from owner record",
            ));
        }
        let expectation = MacosDirectLaunchdExecutableExpectation::new(
            &path,
            &requirement,
            &requirement_sha,
            &cdhash,
            super::model::hex_digest(&bytes),
            before.mode(),
            before.size(),
            before.device(),
            before.inode(),
        )
        .map_err(owner_store_error)?;
        if !validate_retained_macos_executable(opened.file_mut(), &expectation, MUTATION_TIMEOUT)
            .map_err(owner_error)?
        {
            return Err(error(
                "macOS legacy executable failed exact signed-code validation",
            ));
        }
        Ok(Some(MacosLegacyExecutable {
            path,
            sha256: super::model::hex_digest(&bytes),
            size: before.size(),
            mode: before.mode(),
            device: before.device(),
            inode: before.inode(),
            designated_requirement: requirement,
            designated_requirement_sha256: requirement_sha,
            cdhash,
            version: "legacy".to_owned(),
        }))
    }

    fn replace_launcher(
        &mut self,
        expected: &MacosExactEntry,
        replacement: Option<&MacosFilePublication>,
    ) -> Result<(), InstallPlatformError> {
        self.public_tree
            .replace_file(&self.config.direct_plist_path, expected, replacement)
    }

    fn replace_layout(
        &mut self,
        path: &str,
        expected: &MacosExactEntry,
        replacement: Option<&MacosEntryPublication>,
    ) -> Result<(), InstallPlatformError> {
        self.public_tree.replace_entry(path, expected, replacement)
    }

    fn replace_directory(
        &mut self,
        path: &str,
        expected: MacosDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError> {
        self.public_tree.ensure_directory(path, expected, create)
    }

    fn set_autostart(
        &mut self,
        enabled: bool,
    ) -> Result<MacosMutationOutcome, InstallPlatformError> {
        self.launchd_mutator
            .set_autostart(enabled, MUTATION_TIMEOUT)
            .map(Self::map_outcome)
            .map_err(owner_error)
    }

    fn persist_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
    ) -> Result<MacosLauncherSnapshot, InstallPlatformError> {
        self.launcher_store.persist(launcher)
    }

    fn validate_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<(), InstallPlatformError> {
        self.launcher_store.validate(launcher, snapshot)
    }

    fn transition_runtime(
        &mut self,
        transition: &MacosRuntimeTransition,
    ) -> Result<MacosMutationOutcome, InstallPlatformError> {
        match transition {
            MacosRuntimeTransition::Stop { authority } => {
                let active = self.active_unit()?;
                let exact_active = active.as_ref() == Some(&authority.unit)
                    || (active.is_none() && authority.unit.as_str().starts_with("legacy-"));
                if !exact_active {
                    return Err(error("macOS stop authority is not the active unit"));
                }
                let owner = self
                    .owner_store
                    .load_owner_record()
                    .map_err(owner_store_error)?
                    .ok_or_else(|| error("macOS stop authority has no owner record"))?;
                if owner.owner_epoch != authority.owner_epoch
                    || owner.active_identity.audit_token_identity != authority.audit_token_identity
                    || owner.active_identity.executable_path != authority.executable_path
                    || owner.active_identity.designated_requirement_hash
                        != authority.designated_requirement_hash
                    || owner.active_identity.pid != authority.pid
                {
                    return Err(error("macOS stop authority is not the current owner"));
                }
                let proof = corroborate_direct_launchd_owner(&owner, &mut self.launchd_inspector)
                    .map_err(owner_error)?;
                self.launchd_mutator
                    .bootout_exact(&proof, MUTATION_TIMEOUT)
                    .map(Self::map_outcome)
                    .map_err(owner_error)
            }
            MacosRuntimeTransition::Start {
                executable,
                launcher_snapshot,
                after_epoch,
            } => {
                let executable = if executable.synthetic_legacy {
                    let observed = self
                        .inspect_legacy_executable(None)?
                        .ok_or_else(|| error("restored legacy executable is not observable"))?;
                    if observed.path != executable.path
                        || observed.sha256 != executable.sha256
                        || observed.size != executable.size
                        || observed.mode != executable.mode
                        || observed.designated_requirement != executable.designated_requirement
                        || observed.designated_requirement_sha256
                            != executable.designated_requirement_sha256
                        || observed.cdhash != executable.cdhash
                    {
                        return Err(error(
                            "restored legacy executable changed from its snapshot",
                        ));
                    }
                    MacosRuntimeExecutable {
                        device: observed.device,
                        inode: observed.inode,
                        ..executable.clone()
                    }
                } else {
                    let unit = self.retained_unit(&executable.unit)?;
                    self.validate_unit_executable(&unit, executable)?;
                    executable.clone()
                };
                let mut source = self.launcher_store.bootstrap_source(launcher_snapshot)?;
                let expected = MacosDirectLaunchdPublicationExpectation::new(
                    *after_epoch,
                    Self::executable_expectation(&executable)?,
                )
                .map_err(owner_store_error)?;
                self.launchd_mutator
                    .bootstrap_and_kickstart_exact(&mut source, &expected, MUTATION_TIMEOUT)
                    .map(Self::map_outcome)
                    .map_err(owner_error)
            }
        }
    }

    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &MacosLegacySnapshot,
    ) -> Result<UnitRecord, InstallPlatformError> {
        self.public_tree.bind_paths(
            std::iter::empty(),
            std::iter::once(snapshot.executable.path.clone()),
        )?;
        let (metadata, daemon_bytes) = self
            .public_tree
            .regular_file(&snapshot.executable.path, MAX_LEGACY_FILE_BYTES)?;
        if metadata.mode() != snapshot.executable.mode
            || metadata.size() != snapshot.executable.size
            || metadata.device() != snapshot.executable.device
            || metadata.inode() != snapshot.executable.inode
        {
            return Err(error("macOS legacy executable changed before snapshot"));
        }
        super::native_legacy::snapshot_legacy_unit(
            &self.units,
            &self.units_root_hint,
            snapshot,
            &daemon_bytes,
        )
    }

    fn validate_legacy_snapshot(
        &mut self,
        unit: &UnitRecord,
        executable: &MacosLegacyExecutable,
        launcher: &MacosExactEntry,
        launcher_bytes: &[u8],
        entries: &BTreeMap<String, MacosExactEntry>,
    ) -> Result<(), InstallPlatformError> {
        super::native_legacy::validate_legacy_snapshot(
            unit,
            executable,
            launcher,
            launcher_bytes,
            entries,
        )
    }

    fn read_snapshot_file(
        &mut self,
        unit: &UnitRecord,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, InstallPlatformError> {
        super::native_legacy::read_snapshot_file(unit, path, max_bytes)
    }

    fn corroborate_owner(&mut self, record: &MacosOwnerRecord) -> Result<(), InstallPlatformError> {
        corroborate_direct_launchd_owner(record, &mut self.launchd_inspector)
            .map(|_| ())
            .map_err(owner_error)
    }

    fn wait_for_exact_publication(
        &mut self,
        expectation: &MacosDirectLaunchdPublicationExpectation,
        timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, InstallPlatformError> {
        wait_for_exact_direct_launchd_publication(
            &self.owner_store,
            expectation,
            timeout,
            &mut self.launchd_inspector,
        )
        .map_err(owner_error)
    }

    fn wait_for_legacy_publication(
        &mut self,
        executable: &MacosLegacyExecutable,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, InstallPlatformError> {
        let expected = MacosDirectLaunchdPublicationExpectation::new(
            after_epoch,
            MacosDirectLaunchdExecutableExpectation::new(
                &executable.path,
                &executable.designated_requirement,
                &executable.designated_requirement_sha256,
                &executable.cdhash,
                &executable.sha256,
                executable.mode,
                executable.size,
                executable.device,
                executable.inode,
            )
            .map_err(owner_store_error)?,
        )
        .map_err(owner_store_error)?;
        self.wait_for_exact_publication(&expected, timeout)
    }

    fn wait_for_guard_release(&mut self, timeout: Duration) -> Result<bool, InstallPlatformError> {
        let guard = canonical_macos_daemon_guard_path().map_err(owner_error)?;
        wait_for_macos_guard_release(timeout, &guard.to_string_lossy()).map_err(owner_error)
    }
}

fn require_below_home(
    home: &Path,
    path: &Path,
    label: &'static str,
) -> Result<(), InstallPlatformError> {
    super::model::validate_public_path(&exact_path(path)?)?;
    if path == home || path.strip_prefix(home).is_err() {
        return Err(error(format!("macOS {label} is outside retained HOME")));
    }
    Ok(())
}

fn exact_path(path: &Path) -> Result<String, InstallPlatformError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("macOS native topology path is not exact UTF-8"))
}

fn store_error(source: super::super::InstallStoreError) -> InstallPlatformError {
    error(source.to_string())
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

fn owner_error(source: hypercolor_macos_owner::MacosOwnerExecutionError) -> InstallPlatformError {
    error(source.to_string())
}

fn owner_store_error(source: hypercolor_macos_owner::MacosOwnerStoreError) -> InstallPlatformError {
    error(source.to_string())
}

fn parse_active_target(target: &Path) -> Result<UnitId, InstallPlatformError> {
    let mut components = target.components();
    let units = components.next().and_then(|part| part.as_os_str().to_str());
    let unit = components.next().and_then(|part| part.as_os_str().to_str());
    if units != Some("units") || components.next().is_some() {
        return Err(error("macOS active unit link has an invalid exact target"));
    }
    UnitId::new(unit.unwrap_or_default()).map_err(|source| error(source.to_string()))
}
