#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hypercolor_cli::install::macos::{
    MacosCandidateLayout, MacosDirectoryState, MacosEntryPublication, MacosExactEntry,
    MacosFilePublication, MacosInstallConfig, MacosInstallExecutor, MacosInstallPlatform,
    MacosLaunchdObservation, MacosLauncherSnapshot, MacosLegacyExecutable, MacosLegacySnapshot,
    MacosMutationOutcome, MacosPublicSnapshot, MacosRuntimeTransition,
    bind_macos_retained_legacy_unit, retain_macos_unit,
};
use hypercolor_cli::install::{
    InstallAction, InstallCoordinator, InstallDisposition, InstallPlatform, InstallRequest,
    InstallStore, InstallTargetPolicy, InstallTransactionId, InstallationState, PlatformState,
    PlatformTransactionRecord, UnitId, UnitRecord, stage_release_payload,
};
use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosDirectLaunchdPublicationExpectation, MacosOwnerIdentity,
    MacosOwnerRecord,
};
use hypercolor_platform_fs::ExclusiveDirectory;
use serde_json::json;
use sha2::{Digest as _, Sha256};

const REQUIREMENT: &str = concat!(
    "identifier \"tech.hyperbliss.hypercolor.daemon\" and anchor apple generic and ",
    "certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and ",
    "certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and ",
    "certificate leaf[subject.OU] = \"AB12CD34EF\""
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    Before,
    After,
}

#[derive(Debug, Clone)]
enum SnapshotMutation {
    ExtraEntry {
        call: usize,
        path: String,
        entry: MacosExactEntry,
    },
    DirectoryState {
        call: usize,
        path: String,
        state: MacosDirectoryState,
    },
}

#[derive(Debug, Clone)]
struct FakeExecutor {
    config: MacosInstallConfig,
    active_path: PathBuf,
    projection: MacosCandidateLayout,
    public: MacosPublicSnapshot,
    launcher: MacosExactEntry,
    launcher_bytes: Vec<u8>,
    launchd: MacosLaunchdObservation,
    owner: Option<MacosOwnerRecord>,
    effects: Vec<String>,
    next_pid: u32,
    next_epoch: u64,
    fault: Option<(String, FaultPoint)>,
    secondary_fault: Option<(String, FaultPoint)>,
    submitted_unknown: Option<String>,
    launcher_race: Option<(MacosExactEntry, Vec<u8>)>,
    layout_race: Option<(String, MacosExactEntry, Option<Vec<u8>>)>,
    snapshot_files: BTreeMap<String, Vec<u8>>,
    legacy_unit: Option<UnitRecord>,
    legacy_executable: Option<MacosLegacyExecutable>,
    legacy_snapshot: Option<MacosLegacySnapshot>,
    private_launchers: BTreeMap<String, (MacosFilePublication, MacosLauncherSnapshot)>,
    stop_owner_race: bool,
    public_snapshot_calls: usize,
    snapshot_mutation: Option<SnapshotMutation>,
}

impl FakeExecutor {
    fn new(config: MacosInstallConfig, projection: MacosCandidateLayout) -> Self {
        let directories = projection
            .directories
            .iter()
            .cloned()
            .map(|path| (path, MacosDirectoryState::Present))
            .collect();
        let entries = projection
            .entries
            .iter()
            .map(|(path, _)| (path.clone(), MacosExactEntry::Absent))
            .collect();
        Self {
            active_path: config.active_root.clone(),
            config,
            projection,
            public: MacosPublicSnapshot {
                directories,
                entries,
                regular_bytes: BTreeMap::new(),
            },
            launcher: MacosExactEntry::Absent,
            launcher_bytes: Vec::new(),
            launchd: MacosLaunchdObservation {
                pid: None,
                autostart_enabled: false,
            },
            owner: None,
            effects: Vec::new(),
            next_pid: 4000,
            next_epoch: 0,
            fault: None,
            secondary_fault: None,
            submitted_unknown: None,
            launcher_race: None,
            layout_race: None,
            snapshot_files: BTreeMap::new(),
            legacy_unit: None,
            legacy_executable: None,
            legacy_snapshot: None,
            private_launchers: BTreeMap::new(),
            stop_owner_race: false,
            public_snapshot_calls: 0,
            snapshot_mutation: None,
        }
    }

    fn active(&self) -> Option<UnitId> {
        fs::read_link(&self.active_path)
            .ok()
            .and_then(|target| target.file_name().map(ToOwned::to_owned))
            .and_then(|name| UnitId::new(name.to_string_lossy()).ok())
    }

    fn begin_effect(
        &mut self,
        effect: String,
    ) -> Result<bool, hypercolor_cli::install::InstallPlatformError> {
        self.effects.push(effect.clone());
        let primary = self
            .fault
            .as_ref()
            .filter(|(name, _)| name == &effect)
            .map(|(_, point)| *point);
        let secondary = self
            .fault
            .is_none()
            .then(|| {
                self.secondary_fault
                    .as_ref()
                    .filter(|(name, _)| name == &effect)
                    .map(|(_, point)| *point)
            })
            .flatten();
        let point = primary.or(secondary);
        if primary.is_some() {
            self.fault = None;
        } else if secondary.is_some() {
            self.secondary_fault = None;
        }
        if point == Some(FaultPoint::Before) {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted before-effect failure",
            ));
        }
        Ok(point == Some(FaultPoint::After))
    }

    fn finish_effect(
        fail_after: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if fail_after {
            Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted after-effect failure",
            ))
        } else {
            Ok(())
        }
    }
}

impl MacosInstallExecutor for FakeExecutor {
    fn validate_topology(
        &mut self,
        config: &MacosInstallConfig,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if config != &self.config {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "split macOS topology",
            ));
        }
        Ok(())
    }

    fn validate_unit_authority(
        &mut self,
        _unit: &UnitRecord,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        Ok(())
    }

    fn active_unit(
        &mut self,
    ) -> Result<Option<UnitId>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.active())
    }

    fn launchd_observation(
        &mut self,
    ) -> Result<MacosLaunchdObservation, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.launchd.clone())
    }

    fn owner_record(
        &mut self,
    ) -> Result<Option<MacosOwnerRecord>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.owner.clone())
    }

    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(MacosExactEntry, Vec<u8>), hypercolor_cli::install::InstallPlatformError> {
        assert!(self.launcher_bytes.len() <= max_bytes);
        Ok((self.launcher.clone(), self.launcher_bytes.clone()))
    }

    fn public_snapshot(
        &mut self,
        _layouts: &[MacosCandidateLayout],
    ) -> Result<MacosPublicSnapshot, hypercolor_cli::install::InstallPlatformError> {
        self.public_snapshot_calls += 1;
        let apply = match self.snapshot_mutation.as_ref() {
            Some(SnapshotMutation::ExtraEntry { call, .. })
            | Some(SnapshotMutation::DirectoryState { call, .. }) => {
                *call == self.public_snapshot_calls
            }
            None => false,
        };
        if apply {
            match self
                .snapshot_mutation
                .take()
                .expect("checked snapshot mutation")
            {
                SnapshotMutation::ExtraEntry { path, entry, .. } => {
                    self.public.entries.insert(path, entry);
                }
                SnapshotMutation::DirectoryState { path, state, .. } => {
                    self.public.directories.insert(path, state);
                }
            }
        }
        Ok(self.public.clone())
    }

    fn candidate_layout(
        &mut self,
        _unit: &UnitRecord,
    ) -> Result<MacosCandidateLayout, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.projection.clone())
    }

    fn inspect_legacy_executable(
        &mut self,
        _owner: Option<&MacosOwnerRecord>,
    ) -> Result<Option<MacosLegacyExecutable>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.legacy_executable.clone())
    }

    fn replace_launcher(
        &mut self,
        expected: &MacosExactEntry,
        replacement: Option<&MacosFilePublication>,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if let Some((entry, bytes)) = self.launcher_race.take() {
            self.launcher = entry;
            self.launcher_bytes = bytes;
        }
        if !exact_entry_content_matches(&self.launcher, expected) {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted launcher race",
            ));
        }
        let fail_after = self.begin_effect("launcher".to_owned())?;
        if let Some(replacement) = replacement {
            self.launcher_bytes.clone_from(&replacement.contents);
            self.launcher = MacosExactEntry::RegularFile {
                mode: replacement.mode,
                sha256: sha256(&replacement.contents),
                snapshot_unit: None,
                snapshot_path: None,
            };
        } else {
            self.launcher = MacosExactEntry::Absent;
            self.launcher_bytes.clear();
        }
        Self::finish_effect(fail_after)
    }

    fn replace_layout(
        &mut self,
        path: &str,
        expected: &MacosExactEntry,
        replacement: Option<&MacosEntryPublication>,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if self
            .layout_race
            .as_ref()
            .is_some_and(|(race_path, _, _)| race_path == path)
        {
            let (_, entry, bytes) = self.layout_race.take().expect("checked layout race");
            self.public.entries.insert(path.to_owned(), entry);
            if let Some(bytes) = bytes {
                self.public.regular_bytes.insert(path.to_owned(), bytes);
            } else {
                self.public.regular_bytes.remove(path);
            }
        }
        if !exact_entry_content_matches(&self.public.entries[path], expected) {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted layout race",
            ));
        }
        let fail_after = self.begin_effect(format!("layout:{path}"))?;
        let next = match replacement {
            Some(MacosEntryPublication::RegularFile(file)) => {
                self.public
                    .regular_bytes
                    .insert(path.to_owned(), file.contents.clone());
                MacosExactEntry::RegularFile {
                    mode: file.mode,
                    sha256: sha256(&file.contents),
                    snapshot_unit: None,
                    snapshot_path: None,
                }
            }
            Some(MacosEntryPublication::Symlink(target)) => MacosExactEntry::Symlink {
                target: target.clone(),
            },
            None => {
                self.public.regular_bytes.remove(path);
                MacosExactEntry::Absent
            }
        };
        self.public.entries.insert(path.to_owned(), next);
        Self::finish_effect(fail_after)
    }

    fn replace_directory(
        &mut self,
        path: &str,
        expected: MacosDirectoryState,
        create: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        assert_eq!(self.public.directories[path], expected);
        assert!(create, "rollback retains version-neutral scaffolding");
        let fail_after = self.begin_effect(format!("directory:{path}"))?;
        self.public
            .directories
            .insert(path.to_owned(), MacosDirectoryState::Present);
        Self::finish_effect(fail_after)
    }

    fn set_autostart(
        &mut self,
        enabled: bool,
    ) -> Result<MacosMutationOutcome, hypercolor_cli::install::InstallPlatformError> {
        let effect = format!("autostart:{enabled}");
        if self.submitted_unknown.as_ref() == Some(&effect) {
            self.effects.push(effect);
            self.submitted_unknown = None;
            return Ok(MacosMutationOutcome::SubmittedUnknown);
        }
        let fail_after = self.begin_effect(effect)?;
        self.launchd.autostart_enabled = enabled;
        Self::finish_effect(fail_after)?;
        Ok(MacosMutationOutcome::Complete)
    }

    fn persist_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
    ) -> Result<MacosLauncherSnapshot, hypercolor_cli::install::InstallPlatformError> {
        let snapshot_id = launcher_snapshot_id(launcher.mode, &launcher.contents);
        let snapshot = MacosLauncherSnapshot {
            relative_path: format!("launchd/{snapshot_id}.plist"),
            content_sha256: sha256(&launcher.contents),
            mode: launcher.mode,
            size: launcher.contents.len() as u64,
            device: 91,
            inode: u64::try_from(self.private_launchers.len()).expect("bounded fake") + 400,
            snapshot_id,
        };
        self.private_launchers.insert(
            snapshot.relative_path.clone(),
            (launcher.clone(), snapshot.clone()),
        );
        Ok(snapshot)
    }

    fn validate_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if self
            .private_launchers
            .get(&snapshot.relative_path)
            .is_none_or(|(stored_launcher, stored_snapshot)| {
                stored_launcher != launcher || stored_snapshot != snapshot
            })
        {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "private launcher snapshot changed",
            ));
        }
        Ok(())
    }

    fn transition_runtime(
        &mut self,
        transition: &MacosRuntimeTransition,
    ) -> Result<MacosMutationOutcome, hypercolor_cli::install::InstallPlatformError> {
        let running = matches!(transition, MacosRuntimeTransition::Start { .. });
        let effect = format!("runtime:{running}");
        if self.submitted_unknown.as_ref() == Some(&effect) {
            self.effects.push(effect);
            self.submitted_unknown = None;
            return Ok(MacosMutationOutcome::SubmittedUnknown);
        }
        let fail_after = self.begin_effect(effect)?;
        if let MacosRuntimeTransition::Start {
            executable,
            launcher_snapshot,
            after_epoch,
        } = transition
        {
            if self.next_epoch < *after_epoch
                || self
                    .private_launchers
                    .get(&launcher_snapshot.relative_path)
                    .is_none_or(|(_, snapshot)| snapshot != launcher_snapshot)
            {
                return Err(hypercolor_cli::install::InstallPlatformError::new(
                    "invalid scripted private launcher start authority",
                ));
            }
            self.next_pid += 1;
            self.next_epoch = self.next_epoch.max(*after_epoch) + 1;
            self.launchd.pid = Some(self.next_pid);
            let identity = MacosOwnerIdentity::new(
                format!("audit-{}", self.next_epoch),
                &executable.path,
                &executable.designated_requirement_sha256,
                self.next_pid,
            )
            .expect("valid fake owner identity");
            let mut owner = MacosOwnerRecord::new(MacosDaemonOwner::DirectLaunchd, identity, None);
            owner.owner_epoch = self.next_epoch;
            self.owner = Some(owner);
        } else if let MacosRuntimeTransition::Stop { authority } = transition {
            if self.stop_owner_race {
                self.stop_owner_race = false;
                if let Some(owner) = &mut self.owner {
                    "foreign-audit".clone_into(&mut owner.active_identity.audit_token_identity);
                }
            }
            let owner = self.owner.as_ref().ok_or_else(|| {
                hypercolor_cli::install::InstallPlatformError::new(
                    "scripted stop has no current owner",
                )
            })?;
            if owner.owner_epoch != authority.owner_epoch
                || owner.active_identity.audit_token_identity != authority.audit_token_identity
                || owner.active_identity.executable_path != authority.executable_path
                || owner.active_identity.designated_requirement_hash
                    != authority.designated_requirement_hash
                || owner.active_identity.pid != authority.pid
                || self.active().as_ref() != Some(&authority.unit)
            {
                return Err(hypercolor_cli::install::InstallPlatformError::new(
                    "scripted stop authority is not current",
                ));
            }
            self.launchd.pid = None;
        }
        Self::finish_effect(fail_after)?;
        Ok(MacosMutationOutcome::Complete)
    }

    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &MacosLegacySnapshot,
    ) -> Result<UnitRecord, hypercolor_cli::install::InstallPlatformError> {
        self.effects.push("snapshot".to_owned());
        for file in &snapshot.regular_files {
            self.snapshot_files
                .insert(file.path.clone(), file.contents.clone());
        }
        self.legacy_snapshot = Some(snapshot.clone());
        self.legacy_unit.clone().ok_or_else(|| {
            hypercolor_cli::install::InstallPlatformError::new(
                "scripted legacy snapshot authority is not configured",
            )
        })
    }

    fn validate_legacy_snapshot(
        &mut self,
        unit: &UnitRecord,
        executable: &MacosLegacyExecutable,
        launcher: &MacosExactEntry,
        launcher_bytes: &[u8],
        entries: &BTreeMap<String, MacosExactEntry>,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if self
            .legacy_unit
            .as_ref()
            .is_none_or(|expected| expected != unit)
        {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "foreign scripted legacy snapshot",
            ));
        }
        let snapshot = self.legacy_snapshot.as_ref().ok_or_else(|| {
            hypercolor_cli::install::InstallPlatformError::new(
                "missing scripted complete legacy snapshot",
            )
        })?;
        let snapshot_launcher =
            snapshot
                .launcher
                .as_ref()
                .map_or(MacosExactEntry::Absent, |file| {
                    MacosExactEntry::RegularFile {
                        mode: file.mode,
                        sha256: sha256(&file.contents),
                        snapshot_unit: None,
                        snapshot_path: None,
                    }
                });
        if &snapshot.executable != executable
            || snapshot
                .launcher
                .as_ref()
                .map(|file| file.contents.as_slice())
                != (!launcher_bytes.is_empty()).then_some(launcher_bytes)
            || !exact_entry_content_matches(&snapshot_launcher, launcher)
            || snapshot.entries.len() != entries.len()
            || snapshot.entries.iter().any(|(path, expected)| {
                entries
                    .get(path)
                    .is_none_or(|actual| !exact_entry_content_matches(actual, expected))
            })
        {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted legacy snapshot is incomplete",
            ));
        }
        Ok(())
    }

    fn read_snapshot_file(
        &mut self,
        _unit: &UnitRecord,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, hypercolor_cli::install::InstallPlatformError> {
        let bytes = self.snapshot_files.get(path).cloned().ok_or_else(|| {
            hypercolor_cli::install::InstallPlatformError::new("missing scripted snapshot file")
        })?;
        assert!(u64::try_from(bytes.len()).expect("size") <= max_bytes);
        Ok(bytes)
    }

    fn corroborate_owner(
        &mut self,
        record: &MacosOwnerRecord,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if self.owner.as_ref() != Some(record) {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted owner changed",
            ));
        }
        Ok(())
    }

    fn wait_for_exact_publication(
        &mut self,
        expectation: &MacosDirectLaunchdPublicationExpectation,
        _timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self
            .owner
            .clone()
            .filter(|record| record.owner_epoch > expectation.after_epoch()))
    }

    fn wait_for_legacy_publication(
        &mut self,
        _executable: &MacosLegacyExecutable,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, hypercolor_cli::install::InstallPlatformError> {
        assert!(!timeout.is_zero());
        Ok(self
            .owner
            .clone()
            .filter(|record| record.owner_epoch > after_epoch))
    }
}

fn launcher_snapshot_id(mode: u32, contents: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"hypercolor-macos-launcher-v1\0");
    digest.update(mode.to_be_bytes());
    digest.update(contents);
    format!("{:x}", digest.finalize())
}

#[test]
fn first_install_enables_runs_and_keeps_manager_checkpoints_state_neutral() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection.clone());
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate.clone()]).expect("macOS platform");
    let outcome = InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-first-install").expect("transaction"),
            candidate,
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("install candidate");
    assert!(matches!(
        outcome,
        hypercolor_cli::install::InstallOutcome::Committed { .. }
    ));
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, Some(4001));
    assert!(executor.launchd.autostart_enabled);
    assert!(matches!(
        executor.launcher,
        MacosExactEntry::RegularFile { .. }
    ));
    assert_eq!(
        executor.effects,
        [
            format!("layout:{}", projection.entries[0].0),
            format!("layout:{}", projection.entries[1].0),
            "launcher".to_owned(),
            "autostart:true".to_owned(),
            "runtime:true".to_owned(),
        ]
    );
}

#[test]
fn preserve_on_first_install_leaves_service_absent_inactive_and_disabled() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection.clone());
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate.clone()]).expect("macOS platform");
    InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-preserve").expect("transaction"),
            candidate,
            target_policy: InstallTargetPolicy::Preserve,
        })
        .expect("install candidate");
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, None);
    assert!(!executor.launchd.autostart_enabled);
    assert_eq!(executor.launcher, MacosExactEntry::Absent);
    assert_eq!(
        executor.effects,
        [
            format!("layout:{}", projection.entries[0].0),
            format!("layout:{}", projection.entries[1].0),
        ]
    );
}

#[test]
fn error_after_candidate_start_captures_authority_and_rolls_back_every_exact_effect() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let mut executor = FakeExecutor::new(config.clone(), projection.clone());
    let directory = projection.directories[0].clone();
    executor
        .public
        .directories
        .insert(directory.clone(), MacosDirectoryState::Absent);
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate.clone()]).expect("macOS platform");
    let outcome = InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-runtime-error").expect("transaction"),
            candidate,
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("rollback completes");
    assert!(matches!(
        outcome,
        hypercolor_cli::install::InstallOutcome::RolledBack { .. }
    ));
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, None);
    assert!(!executor.launchd.autostart_enabled);
    assert_eq!(executor.launcher, MacosExactEntry::Absent);
    assert_eq!(
        executor.public.directories[&directory],
        MacosDirectoryState::Present
    );
    assert!(
        executor
            .public
            .entries
            .values()
            .all(|entry| *entry == MacosExactEntry::Absent)
    );
    assert_eq!(
        executor.effects,
        [
            format!("directory:{directory}"),
            format!("layout:{}", projection.entries[0].0),
            format!("layout:{}", projection.entries[1].0),
            "launcher".to_owned(),
            "autostart:true".to_owned(),
            "runtime:true".to_owned(),
            "runtime:false".to_owned(),
            "autostart:false".to_owned(),
            "launcher".to_owned(),
            format!("layout:{}", projection.entries[1].0),
            format!("layout:{}", projection.entries[0].0),
        ]
    );
}

#[test]
fn every_forward_platform_effect_fails_closed_before_and_after_mutation() {
    for effect_index in 0..6 {
        for point in [FaultPoint::Before, FaultPoint::After] {
            let fixture = ReleaseFixture::new();
            let install = tempfile::tempdir().expect("install root");
            let store = InstallStore::new(install.path().join("store"), 128 * 1024);
            let lock = store.acquire_lock().expect("install lock");
            let candidate = stage_release_payload(
                &store,
                &lock,
                fixture.path(),
                &fixture.candidate,
                &fixture.unit,
            )
            .expect("stage candidate");
            drop(lock);
            let config = config(&store, install.path());
            let projection = projection(install.path(), &config);
            let directory = projection.directories[0].clone();
            let effects = [
                format!("directory:{directory}"),
                format!("layout:{}", projection.entries[0].0),
                format!("layout:{}", projection.entries[1].0),
                "launcher".to_owned(),
                "autostart:true".to_owned(),
                "runtime:true".to_owned(),
            ];
            let mut executor = FakeExecutor::new(config.clone(), projection);
            executor
                .public
                .directories
                .insert(directory, MacosDirectoryState::Absent);
            executor.fault = Some((effects[effect_index].clone(), point));
            let mut platform = MacosInstallPlatform::new(executor, config, [candidate.clone()])
                .expect("macOS platform");
            let outcome = InstallCoordinator::new(&store, &mut platform)
                .install(InstallRequest {
                    transaction_id: InstallTransactionId::new(format!(
                        "macos-fault-{effect_index}-{}",
                        match point {
                            FaultPoint::Before => "before",
                            FaultPoint::After => "after",
                        }
                    ))
                    .expect("transaction"),
                    candidate,
                    target_policy: InstallTargetPolicy::EnableOnFirstInstall,
                })
                .expect("fault rollback completes");
            assert!(matches!(
                outcome,
                hypercolor_cli::install::InstallOutcome::RolledBack { .. }
            ));
            let executor = platform.into_executor();
            assert_eq!(executor.launchd.pid, None);
            assert!(!executor.launchd.autostart_enabled);
            assert_eq!(executor.launcher, MacosExactEntry::Absent);
            assert!(
                executor
                    .public
                    .entries
                    .values()
                    .all(|entry| *entry == MacosExactEntry::Absent)
            );
        }
    }
}

#[test]
fn every_rollback_platform_effect_recovers_cold_before_and_after_mutation() {
    for effect_index in 0..5 {
        for point in [FaultPoint::Before, FaultPoint::After] {
            let fixture = ReleaseFixture::new();
            let install = tempfile::tempdir().expect("install root");
            let store = InstallStore::new(install.path().join("store"), 128 * 1024);
            let lock = store.acquire_lock().expect("install lock");
            let candidate = stage_release_payload(
                &store,
                &lock,
                fixture.path(),
                &fixture.candidate,
                &fixture.unit,
            )
            .expect("stage candidate");
            drop(lock);
            let config = config(&store, install.path());
            let projection = projection(install.path(), &config);
            let directory = projection.directories[0].clone();
            let rollback_effects = [
                "runtime:false".to_owned(),
                "autostart:false".to_owned(),
                "launcher".to_owned(),
                format!("layout:{}", projection.entries[1].0),
                format!("layout:{}", projection.entries[0].0),
            ];
            let mut executor = FakeExecutor::new(config.clone(), projection.clone());
            executor
                .public
                .directories
                .insert(directory.clone(), MacosDirectoryState::Absent);
            executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
            executor.secondary_fault = Some((rollback_effects[effect_index].clone(), point));
            let mut platform = MacosInstallPlatform::new(executor, config.clone(), [candidate])
                .expect("macOS platform");
            let interrupted_result =
                InstallCoordinator::new(&store, &mut platform).install(InstallRequest {
                    transaction_id: InstallTransactionId::new(format!(
                        "macos-rollback-fault-{effect_index}-{}",
                        match point {
                            FaultPoint::Before => "before",
                            FaultPoint::After => "after",
                        }
                    ))
                    .expect("transaction"),
                    candidate: retain_candidate(&store, &fixture.unit),
                    target_policy: InstallTargetPolicy::EnableOnFirstInstall,
                });
            assert!(
                interrupted_result.is_err(),
                "rollback effect {} at {point:?} unexpectedly completed: {interrupted_result:?}",
                rollback_effects[effect_index]
            );
            let interrupted = platform.into_executor();
            let lock = store.acquire_lock().expect("journal lock");
            let interrupted_journal = store
                .load_journal(&lock)
                .expect("load interrupted journal")
                .expect("rollback journal");
            assert_eq!(
                interrupted_journal.disposition,
                InstallDisposition::Rollback
            );
            let persisted_action = interrupted_journal.next_action;
            let candidate =
                retain_macos_unit(&store, &lock, &fixture.unit).expect("cold retain candidate");
            drop(lock);

            let mut executor = FakeExecutor::new(config.clone(), projection);
            executor.public = interrupted.public;
            executor.launcher = interrupted.launcher;
            executor.launcher_bytes = interrupted.launcher_bytes;
            executor.launchd = interrupted.launchd;
            executor.owner = interrupted.owner;
            executor.next_pid = interrupted.next_pid;
            executor.next_epoch = interrupted.next_epoch;
            executor.snapshot_files = interrupted.snapshot_files;
            executor.legacy_unit = interrupted.legacy_unit;
            executor.legacy_executable = interrupted.legacy_executable;
            executor.legacy_snapshot = interrupted.legacy_snapshot;
            executor.private_launchers = interrupted.private_launchers;
            let mut platform = MacosInstallPlatform::new(executor, config, [candidate])
                .expect("cold recovery platform");
            let lock = store.acquire_lock().expect("recovery journal lock");
            assert_eq!(
                store
                    .load_journal(&lock)
                    .expect("reload rollback journal")
                    .expect("persisted rollback journal")
                    .next_action,
                persisted_action
            );
            drop(lock);
            let outcome = InstallCoordinator::new(&store, &mut platform)
                .recover()
                .expect("cold rollback recovery")
                .expect("pending rollback");
            assert!(matches!(
                outcome,
                hypercolor_cli::install::InstallOutcome::RolledBack { .. }
            ));
            let executor = platform.into_executor();
            assert_eq!(executor.active(), None);
            assert_eq!(executor.launchd.pid, None);
            assert!(!executor.launchd.autostart_enabled);
            assert_eq!(executor.launcher, MacosExactEntry::Absent);
            assert_eq!(
                executor.public.directories[&directory],
                MacosDirectoryState::Present
            );
            assert!(
                executor
                    .public
                    .entries
                    .values()
                    .all(|entry| *entry == MacosExactEntry::Absent)
            );
        }
    }
}

#[test]
fn strict_record_validation_rejects_reordered_and_unknown_layout_effects() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection);
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate.clone()]).expect("macOS platform");
    let prior_platform = platform.inspect().expect("prior inspection");
    let prior = InstallationState {
        active_unit: None,
        platform: prior_platform.clone(),
    };
    let target = PlatformState {
        layout_unit: Some(candidate.id().clone()),
        launcher_unit: Some(candidate.id().clone()),
        loaded: true,
        running_unit: Some(candidate.id().clone()),
        autostart_enabled: true,
    };
    let prepared = platform
        .prepare_transaction(&candidate, &prior, &target)
        .expect("prepare transaction");
    let original: serde_json::Value =
        serde_json::from_slice(prepared.record.payload()).expect("record JSON");
    let mut value = original.clone();
    value["layout"].as_array_mut().expect("layout").swap(0, 1);
    let forged = PlatformTransactionRecord::macos(
        1,
        serde_json::to_vec(&value).expect("encode forged record"),
    )
    .expect("bounded forged record");
    let error = platform
        .validate_transaction_plan(
            &prior_platform,
            &target,
            &prepared.transitions,
            prepared.layout_operation_count,
            &forged,
        )
        .expect_err("reordered effects fail closed");
    assert!(error.to_string().contains("canonical"));

    value["unknown"] = json!(true);
    let unknown = PlatformTransactionRecord::macos(
        1,
        serde_json::to_vec(&value).expect("encode unknown record"),
    )
    .expect("bounded unknown record");
    assert!(
        platform
            .validate_transaction_plan(
                &prior_platform,
                &target,
                &prepared.transitions,
                prepared.layout_operation_count,
                &unknown,
            )
            .is_err()
    );

    for (field, forged_value) in [
        ("relative_path", json!("launchd/foreign.plist")),
        ("inode", json!(0)),
    ] {
        let mut value = original.clone();
        value["candidate_launcher_snapshot"][field] = forged_value;
        let forged = PlatformTransactionRecord::macos(
            1,
            serde_json::to_vec(&value).expect("encode forged snapshot"),
        )
        .expect("bounded forged snapshot");
        assert!(
            platform
                .validate_transaction_plan(
                    &prior_platform,
                    &target,
                    &prepared.transitions,
                    prepared.layout_operation_count,
                    &forged,
                )
                .is_err(),
            "forged private snapshot {field} must fail closed"
        );
    }
}

#[test]
fn prior_stop_rejects_owner_drift_without_booting_out_the_foreign_owner() {
    let first = ReleaseFixture::with_version("0.3.2", b"daemon-a");
    let second = ReleaseFixture::with_version("0.3.3", b"daemon-b");
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let first_unit =
        stage_release_payload(&store, &lock, first.path(), &first.candidate, &first.unit)
            .expect("stage first candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection.clone());
    let mut platform =
        MacosInstallPlatform::new(executor, config.clone(), [first_unit]).expect("first platform");
    InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-stop-authority-first")
                .expect("transaction"),
            candidate: retain_candidate(&store, &first.unit),
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("first install");
    let mut executor = platform.into_executor();
    let prior_pid = executor.launchd.pid;
    executor.stop_owner_race = true;
    executor.secondary_fault = Some(("runtime:true".to_owned(), FaultPoint::Before));
    executor.effects.clear();
    let lock = store.acquire_lock().expect("upgrade lock");
    let prior = retain_macos_unit(&store, &lock, &first.unit).expect("retain prior");
    let candidate = stage_release_payload(
        &store,
        &lock,
        second.path(),
        &second.candidate,
        &second.unit,
    )
    .expect("stage second candidate");
    drop(lock);
    let mut platform = MacosInstallPlatform::new(executor, config, [prior, candidate.clone()])
        .expect("upgrade platform");
    assert!(
        InstallCoordinator::new(&store, &mut platform)
            .install(InstallRequest {
                transaction_id: InstallTransactionId::new("macos-stop-authority-race")
                    .expect("transaction"),
                candidate,
                target_policy: InstallTargetPolicy::Preserve,
            })
            .is_err()
    );
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, prior_pid);
    assert_eq!(executor.effects, ["runtime:false".to_owned()]);
    assert_eq!(
        executor
            .owner
            .expect("foreign owner remains")
            .active_identity
            .audit_token_identity,
        "foreign-audit"
    );
}

#[test]
fn cold_process_rebind_preserves_running_upgrade_state() {
    let first = ReleaseFixture::with_version("0.3.2", b"daemon-a");
    let second = ReleaseFixture::with_version("0.3.3", b"daemon-b");
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let first_unit =
        stage_release_payload(&store, &lock, first.path(), &first.candidate, &first.unit)
            .expect("stage first candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection.clone());
    let mut platform =
        MacosInstallPlatform::new(executor, config.clone(), [first_unit]).expect("first platform");
    InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-cold-first").expect("transaction"),
            candidate: retain_candidate(&store, &first.unit),
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("first install");
    let prior_executor = platform.into_executor();

    let lock = store.acquire_lock().expect("upgrade lock");
    let prior = retain_macos_unit(&store, &lock, &first.unit).expect("cold retain prior");
    let candidate = stage_release_payload(
        &store,
        &lock,
        second.path(),
        &second.candidate,
        &second.unit,
    )
    .expect("stage second candidate");
    drop(lock);
    let mut executor = FakeExecutor::new(config.clone(), projection);
    executor.public = prior_executor.public;
    executor.launcher = prior_executor.launcher;
    executor.launcher_bytes = prior_executor.launcher_bytes;
    executor.launchd = prior_executor.launchd;
    executor.owner = prior_executor.owner;
    executor.next_pid = prior_executor.next_pid;
    executor.next_epoch = prior_executor.next_epoch;
    executor.private_launchers = prior_executor.private_launchers;
    let mut platform = MacosInstallPlatform::new(executor, config, [prior, candidate.clone()])
        .expect("cold upgrade platform");
    InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-cold-upgrade").expect("transaction"),
            candidate,
            target_policy: InstallTargetPolicy::Preserve,
        })
        .expect("cold upgrade");
    let executor = platform.into_executor();
    assert_eq!(executor.active(), Some(second.unit));
    assert!(executor.launchd.pid.is_some());
    assert!(executor.launchd.autostart_enabled);
    assert_eq!(
        executor.effects,
        ["runtime:false".to_owned(), "runtime:true".to_owned()]
    );
}

#[test]
fn cold_recovery_uses_persisted_candidate_receipt_to_finish_rollback() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let mut executor = FakeExecutor::new(config.clone(), projection.clone());
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
    executor.secondary_fault = Some(("runtime:false".to_owned(), FaultPoint::Before));
    let mut platform =
        MacosInstallPlatform::new(executor, config.clone(), [candidate]).expect("macOS platform");
    assert!(
        InstallCoordinator::new(&store, &mut platform)
            .install(InstallRequest {
                transaction_id: InstallTransactionId::new("macos-cold-recovery")
                    .expect("transaction"),
                candidate: retain_candidate(&store, &fixture.unit),
                target_policy: InstallTargetPolicy::EnableOnFirstInstall,
            })
            .is_err()
    );
    let interrupted = platform.into_executor();
    assert!(interrupted.launchd.pid.is_some());

    let lock = store.acquire_lock().expect("recovery lock");
    let candidate = retain_macos_unit(&store, &lock, &fixture.unit).expect("cold retain candidate");
    drop(lock);
    let mut executor = FakeExecutor::new(config.clone(), projection);
    executor.public = interrupted.public;
    executor.launcher = interrupted.launcher;
    executor.launcher_bytes = interrupted.launcher_bytes;
    executor.launchd = interrupted.launchd;
    executor.owner = interrupted.owner;
    executor.next_pid = interrupted.next_pid;
    executor.next_epoch = interrupted.next_epoch;
    executor.private_launchers = interrupted.private_launchers;
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate]).expect("recovery platform");
    let mut lock = store.acquire_lock().expect("coordinator recovery lock");
    let outcome = InstallCoordinator::new(&store, &mut platform)
        .recover_with_lock(&mut lock)
        .expect("recover")
        .expect("pending journal");
    assert!(matches!(
        outcome,
        hypercolor_cli::install::InstallOutcome::RolledBack { .. }
    ));
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, None);
    assert_eq!(executor.launcher, MacosExactEntry::Absent);
    assert!(
        executor
            .public
            .entries
            .values()
            .all(|entry| *entry == MacosExactEntry::Absent)
    );
}

#[test]
fn submitted_unknown_runtime_stays_forward_without_starting_rollback() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let mut executor = FakeExecutor::new(config.clone(), projection);
    executor.submitted_unknown = Some("runtime:true".to_owned());
    let mut platform =
        MacosInstallPlatform::new(executor, config, [candidate.clone()]).expect("macOS platform");
    let result = InstallCoordinator::new(&store, &mut platform).install(InstallRequest {
        transaction_id: InstallTransactionId::new("macos-submitted-unknown").expect("transaction"),
        candidate,
        target_policy: InstallTargetPolicy::EnableOnFirstInstall,
    });
    assert!(result.is_err());
    let executor = platform.into_executor();
    assert_eq!(executor.launchd.pid, None);
    assert_eq!(
        executor.effects.last().map(String::as_str),
        Some("runtime:true")
    );
    assert!(
        !executor
            .effects
            .iter()
            .any(|effect| effect == "runtime:false")
    );
    let lock = store.acquire_lock().expect("journal lock");
    let journal = store
        .load_journal(&lock)
        .expect("load journal")
        .expect("pending journal");
    assert_eq!(journal.disposition, InstallDisposition::Forward);
    assert_eq!(
        journal.next_action,
        Some(InstallAction::RestoreCandidateRuntime)
    );
}

#[test]
fn first_conversion_snapshots_complete_raw_inventory_and_restores_exact_modes() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let mut executor = FakeExecutor::new(config.clone(), projection.clone());
    let current = projection.entries[0].0.clone();
    let historical = install
        .path()
        .join("home/.local/bin/hypercolor-old")
        .to_string_lossy()
        .into_owned();
    executor.public.entries.insert(
        current.clone(),
        MacosExactEntry::RegularFile {
            mode: 0o755,
            sha256: sha256(b"old-cli"),
            snapshot_unit: None,
            snapshot_path: None,
        },
    );
    executor
        .public
        .regular_bytes
        .insert(current.clone(), b"old-cli".to_vec());
    executor.public.entries.insert(
        historical.clone(),
        MacosExactEntry::RegularFile {
            mode: 0o700,
            sha256: sha256(b"historical"),
            snapshot_unit: None,
            snapshot_path: None,
        },
    );
    executor
        .public
        .regular_bytes
        .insert(historical.clone(), b"historical".to_vec());
    executor.launcher_bytes = b"<?xml version=\"1.0\"?><plist>legacy</plist>\n".to_vec();
    executor.launcher = MacosExactEntry::RegularFile {
        mode: 0o600,
        sha256: sha256(&executor.launcher_bytes),
        snapshot_unit: None,
        snapshot_path: None,
    };
    executor.legacy_executable = Some(MacosLegacyExecutable {
        path: install
            .path()
            .join("home/.local/bin/hypercolor-daemon")
            .to_string_lossy()
            .into_owned(),
        sha256: sha256(b"legacy-daemon"),
        size: 13,
        mode: 0o755,
        device: 41,
        inode: 42,
        designated_requirement: REQUIREMENT.to_owned(),
        designated_requirement_sha256: sha256(REQUIREMENT.as_bytes()),
        version: "0.2.9".to_owned(),
    });
    let mut probe = MacosInstallPlatform::new(executor, config.clone(), [candidate.clone()])
        .expect("probe platform");
    let prior = probe.inspect().expect("legacy inspection");
    let legacy_id = prior.layout_unit.expect("synthetic legacy ID");
    let mut executor = probe.into_executor();
    let legacy = legacy_unit(&legacy_id);
    executor.legacy_unit = Some(legacy.clone());
    executor.fault = Some(("launcher".to_owned(), FaultPoint::Before));
    let mut platform = MacosInstallPlatform::new(executor, config, [candidate.clone(), legacy])
        .expect("conversion platform");
    let outcome = InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-first-conversion")
                .expect("transaction"),
            candidate,
            target_policy: InstallTargetPolicy::Preserve,
        })
        .expect("conversion rollback");
    assert!(matches!(
        outcome,
        hypercolor_cli::install::InstallOutcome::RolledBack { .. }
    ));
    let executor = platform.into_executor();
    let snapshot = executor.legacy_snapshot.expect("complete legacy snapshot");
    assert_eq!(snapshot.launcher.expect("launcher snapshot").mode, 0o600);
    assert_eq!(snapshot.regular_files.len(), 2);
    assert!(
        snapshot.regular_files.iter().any(|file| {
            file.path == current && file.mode == 0o755 && file.contents == b"old-cli"
        })
    );
    assert!(snapshot.regular_files.iter().any(|file| {
        file.path == historical && file.mode == 0o700 && file.contents == b"historical"
    }));
    assert_eq!(executor.public.regular_bytes[&current], b"old-cli");
    assert_eq!(executor.public.regular_bytes[&historical], b"historical");
    assert_eq!(
        executor.launcher_bytes,
        b"<?xml version=\"1.0\"?><plist>legacy</plist>\n"
    );
    assert!(matches!(
        executor.launcher,
        MacosExactEntry::RegularFile { mode: 0o600, .. }
    ));
}

#[test]
fn launcher_and_layout_namespace_races_preserve_the_foreign_sentinel() {
    for race in ["launcher", "layout"] {
        let fixture = ReleaseFixture::new();
        let install = tempfile::tempdir().expect("install root");
        let store = InstallStore::new(install.path().join("store"), 128 * 1024);
        let lock = store.acquire_lock().expect("install lock");
        let candidate = stage_release_payload(
            &store,
            &lock,
            fixture.path(),
            &fixture.candidate,
            &fixture.unit,
        )
        .expect("stage candidate");
        drop(lock);
        let config = config(&store, install.path());
        let projection = projection(install.path(), &config);
        let mut executor = FakeExecutor::new(config.clone(), projection.clone());
        if race == "launcher" {
            executor.launcher_race = Some((
                MacosExactEntry::RegularFile {
                    mode: 0o600,
                    sha256: sha256(b"foreign-plist"),
                    snapshot_unit: None,
                    snapshot_path: None,
                },
                b"foreign-plist".to_vec(),
            ));
        } else {
            executor.layout_race = Some((
                projection.entries[0].0.clone(),
                MacosExactEntry::Symlink {
                    target: "/foreign/sentinel".to_owned(),
                },
                None,
            ));
        }
        let mut platform = MacosInstallPlatform::new(executor, config, [candidate.clone()])
            .expect("macOS platform");
        assert!(
            InstallCoordinator::new(&store, &mut platform)
                .install(InstallRequest {
                    transaction_id: InstallTransactionId::new(format!("macos-{race}-race"))
                        .expect("transaction"),
                    candidate,
                    target_policy: InstallTargetPolicy::EnableOnFirstInstall,
                })
                .is_err()
        );
        let executor = platform.into_executor();
        if race == "launcher" {
            assert_eq!(executor.launcher_bytes, b"foreign-plist");
            assert!(matches!(
                executor.launcher,
                MacosExactEntry::RegularFile { mode: 0o600, .. }
            ));
        } else {
            assert_eq!(
                executor.public.entries[&projection.entries[0].0],
                MacosExactEntry::Symlink {
                    target: "/foreign/sentinel".to_owned()
                }
            );
        }
        let lock = store.acquire_lock().expect("journal lock");
        let journal = store
            .load_journal(&lock)
            .expect("load journal")
            .expect("pending journal");
        assert_eq!(journal.disposition, InstallDisposition::Forward);
        assert_eq!(
            journal.next_action,
            Some(if race == "launcher" {
                InstallAction::InstallCandidateLauncher
            } else {
                InstallAction::InstallCandidateLayout
            })
        );
    }
}

#[test]
fn public_inventory_key_and_directory_drift_fails_before_unload() {
    for case in 0..3 {
        let first = ReleaseFixture::with_version("0.3.2", b"daemon-a");
        let second = ReleaseFixture::with_version("0.3.3", b"daemon-b");
        let install = tempfile::tempdir().expect("install root");
        let store = InstallStore::new(install.path().join("store"), 128 * 1024);
        let lock = store.acquire_lock().expect("install lock");
        let first_unit =
            stage_release_payload(&store, &lock, first.path(), &first.candidate, &first.unit)
                .expect("stage first candidate");
        drop(lock);
        let config = config(&store, install.path());
        let projection = projection(install.path(), &config);
        let executor = FakeExecutor::new(config.clone(), projection.clone());
        let mut platform = MacosInstallPlatform::new(executor, config.clone(), [first_unit])
            .expect("first platform");
        InstallCoordinator::new(&store, &mut platform)
            .install(InstallRequest {
                transaction_id: InstallTransactionId::new(format!("macos-drift-first-{case}"))
                    .expect("transaction"),
                candidate: retain_candidate(&store, &first.unit),
                target_policy: InstallTargetPolicy::EnableOnFirstInstall,
            })
            .expect("first install");
        let mut executor = platform.into_executor();
        executor.effects.clear();
        executor.public_snapshot_calls = 0;
        let foreign = install
            .path()
            .join("home/.local/bin/foreign-owned-entry")
            .to_string_lossy()
            .into_owned();
        executor.snapshot_mutation = Some(match case {
            0 => SnapshotMutation::ExtraEntry {
                call: 3,
                path: foreign.clone(),
                entry: MacosExactEntry::Symlink {
                    target: "/foreign/preflight".to_owned(),
                },
            },
            1 => SnapshotMutation::ExtraEntry {
                call: 4,
                path: foreign.clone(),
                entry: MacosExactEntry::Symlink {
                    target: "/foreign/post-preflight".to_owned(),
                },
            },
            _ => SnapshotMutation::DirectoryState {
                call: 4,
                path: projection.directories[0].clone(),
                state: MacosDirectoryState::Absent,
            },
        });
        let prior_pid = executor.launchd.pid;
        let lock = store.acquire_lock().expect("upgrade lock");
        let prior = retain_macos_unit(&store, &lock, &first.unit).expect("retain prior");
        let candidate = stage_release_payload(
            &store,
            &lock,
            second.path(),
            &second.candidate,
            &second.unit,
        )
        .expect("stage second candidate");
        drop(lock);
        let mut platform = MacosInstallPlatform::new(executor, config, [prior, candidate.clone()])
            .expect("upgrade platform");
        assert!(
            InstallCoordinator::new(&store, &mut platform)
                .install(InstallRequest {
                    transaction_id: InstallTransactionId::new(format!("macos-drift-{case}"))
                        .expect("transaction"),
                    candidate,
                    target_policy: InstallTargetPolicy::Preserve,
                })
                .is_err()
        );
        let executor = platform.into_executor();
        assert_eq!(executor.launchd.pid, prior_pid);
        assert!(executor.effects.is_empty());
        match case {
            0 | 1 => assert!(matches!(
                executor.public.entries.get(&foreign),
                Some(MacosExactEntry::Symlink { .. })
            )),
            _ => assert_eq!(
                executor.public.directories[&projection.directories[0]],
                MacosDirectoryState::Absent
            ),
        }
        let lock = store.acquire_lock().expect("journal lock");
        let journal = store
            .load_journal(&lock)
            .expect("load journal")
            .expect("pending journal");
        assert_eq!(journal.disposition, InstallDisposition::Forward);
        assert_eq!(journal.next_action, Some(InstallAction::PreflightCandidate));
        assert_eq!(journal.layout_operation_index, 0);
    }
}

#[test]
fn same_unit_failure_restores_prior_launcher_mode_layout_and_fresh_owner() {
    let fixture = ReleaseFixture::new();
    let install = tempfile::tempdir().expect("install root");
    let store = InstallStore::new(install.path().join("store"), 128 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage candidate");
    drop(lock);
    let config = config(&store, install.path());
    let projection = projection(install.path(), &config);
    let executor = FakeExecutor::new(config.clone(), projection.clone());
    let mut platform =
        MacosInstallPlatform::new(executor, config.clone(), [candidate]).expect("first platform");
    InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-same-unit-first")
                .expect("transaction"),
            candidate: retain_candidate(&store, &fixture.unit),
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("first install");
    let mut executor = platform.into_executor();
    executor.launcher = match executor.launcher {
        MacosExactEntry::RegularFile { sha256, .. } => MacosExactEntry::RegularFile {
            mode: 0o600,
            sha256,
            snapshot_unit: None,
            snapshot_path: None,
        },
        MacosExactEntry::Absent | MacosExactEntry::Symlink { .. } => {
            panic!("installed launcher must be regular")
        }
    };
    executor
        .public
        .entries
        .insert(projection.entries[0].0.clone(), MacosExactEntry::Absent);
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
    executor.effects.clear();
    let prior_epoch = executor.owner.as_ref().expect("prior owner").owner_epoch;
    let lock = store.acquire_lock().expect("retain same unit lock");
    let retained = retain_macos_unit(&store, &lock, &fixture.unit).expect("retain same unit");
    drop(lock);
    let mut platform = MacosInstallPlatform::new(executor, config, [retained.clone()])
        .expect("same-unit platform");
    let outcome = InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("macos-same-unit-reinstall")
                .expect("transaction"),
            candidate: retained,
            target_policy: InstallTargetPolicy::Preserve,
        })
        .expect("same-unit rollback");
    assert!(matches!(
        outcome,
        hypercolor_cli::install::InstallOutcome::RolledBack { .. }
    ));
    let executor = platform.into_executor();
    assert!(matches!(
        executor.launcher,
        MacosExactEntry::RegularFile { mode: 0o600, .. }
    ));
    assert_eq!(
        executor.public.entries[&projection.entries[0].0],
        MacosExactEntry::Absent
    );
    assert!(executor.launchd.pid.is_some());
    assert!(executor.owner.expect("restored owner").owner_epoch > prior_epoch + 1);
}

fn config(store: &InstallStore, root: &Path) -> MacosInstallConfig {
    MacosInstallConfig {
        direct_plist_path: root
            .join("home/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist")
            .to_string_lossy()
            .into_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.root().join("active"),
        log_directory: root.join("home/Library/Logs/hypercolor"),
    }
}

fn projection(root: &Path, config: &MacosInstallConfig) -> MacosCandidateLayout {
    let bin = root.join("home/.local/bin");
    MacosCandidateLayout {
        directories: vec![bin.to_string_lossy().into_owned()],
        entries: vec![
            (
                bin.join("hypercolor").to_string_lossy().into_owned(),
                config
                    .active_root
                    .join("bin/hypercolor")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                bin.join("hypercolor-daemon").to_string_lossy().into_owned(),
                config
                    .active_root
                    .join("bin/hypercolor-daemon")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ],
    }
}

struct ReleaseFixture {
    root: tempfile::TempDir,
    candidate: File,
    unit: UnitId,
}

impl ReleaseFixture {
    fn new() -> Self {
        Self::with_version("0.3.2", b"daemon")
    }

    fn with_version(version: &str, daemon: &[u8]) -> Self {
        let root = tempfile::tempdir().expect("release root");
        let files = [
            ("bin/hypercolor-daemon", daemon, 0o755),
            ("bin/hypercolor", b"candidate".as_slice(), 0o755),
            ("bin/hypercolor-app", b"app".as_slice(), 0o755),
            ("bin/hypercolor-tui", b"tui".as_slice(), 0o755),
            ("bin/hypercolor-open", b"open".as_slice(), 0o755),
            ("share/hypercolor/ui/index.html", b"ui".as_slice(), 0o644),
            (
                "share/hypercolor/effects/bundled/effect.html",
                b"effect".as_slice(),
                0o644,
            ),
            ("share/hypercolor/docs/readme.md", b"docs".as_slice(), 0o644),
            (
                "share/hypercolor/agents/skills/skill.md",
                b"skill".as_slice(),
                0o644,
            ),
            (
                "share/hypercolor/agents/agents/agent.md",
                b"agent".as_slice(),
                0o644,
            ),
            (
                "share/hypercolor/site/index.html",
                b"site".as_slice(),
                0o644,
            ),
        ];
        let provenance = serde_json::to_vec_pretty(&json!({
            "team_id": "AB12CD34EF",
            "target": native_target(),
            "objects": [{
                "path": "bin/hypercolor-daemon",
                "identifier": "tech.hyperbliss.hypercolor.daemon",
                "designated_requirement": REQUIREMENT,
            }],
            "notarization": {
                "id": "2efe2717-52ef-43a5-96dc-0797e4ca1041",
                "message": "Processing complete",
                "status": "Accepted",
            },
        }))
        .expect("provenance");
        let mut owned_files = files
            .into_iter()
            .map(|(path, bytes, mode)| (path.to_owned(), bytes.to_vec(), mode))
            .collect::<Vec<_>>();
        owned_files.push((
            "share/hypercolor/macos-notarization.json".to_owned(),
            provenance,
            0o644,
        ));
        let directories = [
            "bin",
            "share",
            "share/hypercolor",
            "share/hypercolor/ui",
            "share/hypercolor/effects",
            "share/hypercolor/effects/bundled",
            "share/hypercolor/docs",
            "share/hypercolor/agents",
            "share/hypercolor/agents/skills",
            "share/hypercolor/agents/agents",
            "share/hypercolor/site",
        ];
        let mut members = directories
            .iter()
            .map(|path| json!({"path":path,"type":"directory","mode":0o755}))
            .collect::<Vec<_>>();
        for directory in directories {
            fs::create_dir_all(root.path().join(directory)).expect("release directory");
            fs::set_permissions(
                root.path().join(directory),
                fs::Permissions::from_mode(0o755),
            )
            .expect("directory mode");
        }
        for (path, bytes, mode) in &owned_files {
            fs::write(root.path().join(path), bytes).expect("release file");
            fs::set_permissions(root.path().join(path), fs::Permissions::from_mode(*mode))
                .expect("file mode");
            members.push(json!({
                "path": path,
                "type": "file",
                "mode": mode,
                "size": bytes.len(),
                "sha256": sha256(bytes),
            }));
        }
        members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        let manifest = serde_json::to_vec_pretty(&json!({
            "name":"hypercolor",
            "version":version,
            "platform":native_platform(),
            "rust_target":native_target(),
            "binaries":["hypercolor-daemon","hypercolor","hypercolor-app","hypercolor-tui","hypercolor-open"],
            "assets":{
                "ui_files":1,
                "bundled_effect_files":1,
                "docs_files":1,
                "skill_files":1,
                "agent_files":1,
                "site_files":1,
            },
            "members":members,
        }))
        .expect("manifest");
        fs::write(root.path().join("manifest.json"), &manifest).expect("manifest file");
        fs::set_permissions(
            root.path().join("manifest.json"),
            fs::Permissions::from_mode(0o644),
        )
        .expect("manifest mode");
        let candidate = File::open(root.path().join("bin/hypercolor")).expect("candidate");
        Self {
            root,
            candidate,
            unit: UnitId::new(sha256(&manifest)).expect("unit ID"),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }
}

fn retain_candidate(store: &InstallStore, id: &UnitId) -> UnitRecord {
    let lock = store.acquire_lock().expect("retain lock");
    retain_macos_unit(store, &lock, id).expect("retain installed macOS unit")
}

fn legacy_unit(id: &UnitId) -> UnitRecord {
    let parent = tempfile::tempdir().expect("legacy authority parent");
    fs::create_dir(parent.path().join("legacy")).expect("legacy authority directory");
    let exclusive = ExclusiveDirectory::try_acquire(parent.path(), Path::new(".legacy.lock"))
        .expect("legacy lock")
        .expect("exclusive legacy lock");
    let authority = exclusive
        .root_directory()
        .expect("legacy parent authority")
        .open_child_directory(Path::new("legacy"))
        .expect("legacy directory authority")
        .read_only()
        .expect("read-only legacy authority");
    bind_macos_retained_legacy_unit(id.clone(), parent.path().join("legacy"), authority)
        .expect("bind legacy unit")
}

fn native_platform() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "macos-arm64",
        "x86_64" => "macos-amd64",
        architecture => panic!("unsupported test architecture {architecture}"),
    }
}

fn native_target() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64-apple-darwin",
        "x86_64" => "x86_64-apple-darwin",
        architecture => panic!("unsupported test architecture {architecture}"),
    }
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("write digest");
            output
        })
}

fn exact_entry_content_matches(left: &MacosExactEntry, right: &MacosExactEntry) -> bool {
    match (left, right) {
        (MacosExactEntry::Absent, MacosExactEntry::Absent) => true,
        (
            MacosExactEntry::RegularFile {
                mode: left_mode,
                sha256: left_sha,
                ..
            },
            MacosExactEntry::RegularFile {
                mode: right_mode,
                sha256: right_sha,
                ..
            },
        ) => left_mode == right_mode && left_sha == right_sha,
        (
            MacosExactEntry::Symlink {
                target: left_target,
            },
            MacosExactEntry::Symlink {
                target: right_target,
            },
        ) => left_target == right_target,
        _ => false,
    }
}
