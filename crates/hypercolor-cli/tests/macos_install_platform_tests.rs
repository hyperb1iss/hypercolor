#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hypercolor_cli::install::macos::{
    MacosCandidateLayout, MacosDirectoryState, MacosEntryPublication, MacosExactEntry,
    MacosFilePublication, MacosInstallConfig, MacosInstallExecutor, MacosInstallPlatform,
    MacosLaunchdObservation, MacosLauncherSnapshot, MacosLegacyExecutable, MacosLegacyFile,
    MacosLegacySnapshot, MacosMutationOutcome, MacosNativeExecutor, MacosPublicSnapshot,
    MacosRuntimeExecutable, MacosRuntimeTransition, bind_macos_retained_legacy_unit,
    retain_macos_unit,
};
use hypercolor_cli::install::{
    InstallAction, InstallCoordinator, InstallDisposition, InstallPlatform, InstallRequest,
    InstallStore, InstallTargetPolicy, InstallTransactionId, InstallationState, PlatformState,
    PlatformTransactionRecord, UnitId, UnitRecord, stage_release_payload,
};
use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosDirectLaunchdBootstrapSource, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdInspector, MacosDirectLaunchdMutationOutcome, MacosDirectLaunchdMutator,
    MacosDirectLaunchdOwnerProof, MacosDirectLaunchdPublicationExpectation,
    MacosDirectLaunchdState, MacosOwnerExecutionError, MacosOwnerIdentity, MacosOwnerRecord,
    MacosOwnerStore,
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
const CDHASH: &str = "0123456789abcdef0123456789abcdef01234567";

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

    fn validate_unit_executable(
        &mut self,
        _unit: &UnitRecord,
        _executable: &MacosRuntimeExecutable,
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

    fn bind_public_inventory(
        &mut self,
        _directories: &[String],
        _entries: &[String],
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        Ok(())
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
            let active_matches = self.active().as_ref() == Some(&authority.unit)
                || (self.active().is_none() && authority.unit.as_str().starts_with("legacy-"));
            if owner.owner_epoch != authority.owner_epoch
                || owner.active_identity.audit_token_identity != authority.audit_token_identity
                || owner.active_identity.executable_path != authority.executable_path
                || owner.active_identity.designated_requirement_hash
                    != authority.designated_requirement_hash
                || owner.active_identity.pid != authority.pid
                || !active_matches
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

    fn wait_for_guard_release(
        &mut self,
        _timeout: Duration,
    ) -> Result<bool, hypercolor_cli::install::InstallPlatformError> {
        Ok(true)
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
        2,
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
        2,
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
            2,
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

    for forged_cdhash in ["A".repeat(40), "0".repeat(40)] {
        let mut value = original.clone();
        value["candidate"]["cdhash"] = json!(forged_cdhash);
        let forged = PlatformTransactionRecord::macos(
            2,
            serde_json::to_vec(&value).expect("encode forged CDHash binding"),
        )
        .expect("bounded forged record");
        assert!(
            platform
                .validate_transaction_plan(
                    &prior_platform,
                    &target,
                    &prepared.transitions,
                    prepared.layout_operation_count,
                    &forged,
                )
                .is_err()
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
    let legacy_executable = MacosLegacyExecutable {
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
        cdhash: CDHASH.to_owned(),
        version: "0.2.9".to_owned(),
    };
    executor.launchd.pid = Some(3100);
    executor.owner = Some(MacosOwnerRecord {
        schema_version: 1,
        active_owner: MacosDaemonOwner::DirectLaunchd,
        active_identity: MacosOwnerIdentity::new(
            "legacy-audit",
            &legacy_executable.path,
            &legacy_executable.designated_requirement_sha256,
            3100,
        )
        .expect("legacy owner identity"),
        owner_epoch: 7,
        conflict: None,
        selected_external_owner: None,
    });
    executor.next_epoch = 7;
    executor.legacy_executable = Some(legacy_executable);
    let mut probe = MacosInstallPlatform::new(executor, config.clone(), [candidate.clone()])
        .expect("probe platform");
    let prior = probe.inspect().expect("legacy inspection");
    let legacy_id = prior.layout_unit.expect("synthetic legacy ID");
    let mut executor = probe.into_executor();
    let executable = executor
        .legacy_executable
        .clone()
        .expect("legacy executable");
    let snapshot = MacosLegacySnapshot {
        unit: legacy_id,
        version: executable.version.clone(),
        launcher: Some(MacosFilePublication {
            mode: 0o600,
            contents: executor.launcher_bytes.clone(),
        }),
        entries: executor.public.entries.clone().into_iter().collect(),
        regular_files: executor
            .public
            .regular_bytes
            .iter()
            .map(|(path, contents)| MacosLegacyFile {
                path: path.clone(),
                mode: match &executor.public.entries[path] {
                    MacosExactEntry::RegularFile { mode, .. } => *mode,
                    MacosExactEntry::Absent | MacosExactEntry::Symlink { .. } => {
                        panic!("regular bytes require a regular entry")
                    }
                },
                contents: contents.clone(),
            })
            .collect(),
        executable,
    };
    let legacy = legacy_unit(&snapshot, b"legacy-daemon");
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
    assert!(executor.launchd.pid.is_some());
    assert!(executor.owner.expect("restored legacy owner").owner_epoch > 7);
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

#[derive(Debug, Default)]
struct NoLaunchdMutator;

impl MacosDirectLaunchdMutator for NoLaunchdMutator {
    fn autostart_enabled(&mut self) -> Result<bool, MacosOwnerExecutionError> {
        Ok(false)
    }

    fn set_autostart(
        &mut self,
        _enabled: bool,
        _timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        panic!("filesystem authority tests must not mutate launchd")
    }

    fn bootout_exact(
        &mut self,
        _expected: &MacosDirectLaunchdOwnerProof,
        _timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        panic!("filesystem authority tests must not mutate launchd")
    }

    fn bootstrap_and_kickstart_exact(
        &mut self,
        _source: &mut MacosDirectLaunchdBootstrapSource,
        _expected: &MacosDirectLaunchdPublicationExpectation,
        _timeout: Duration,
    ) -> Result<
        MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof>,
        MacosOwnerExecutionError,
    > {
        panic!("filesystem authority tests must not mutate launchd")
    }
}

#[derive(Debug, Default)]
struct NoLaunchdInspector;

impl MacosDirectLaunchdInspector for NoLaunchdInspector {
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        Ok(MacosDirectLaunchdState::NotLoaded)
    }

    fn live_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(false)
    }

    fn publication_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
        _executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(false)
    }
}

#[derive(Debug, Clone)]
struct CaptureStartMutator {
    observed_executable: Arc<Mutex<Option<(u64, u64, String)>>>,
}

impl MacosDirectLaunchdMutator for CaptureStartMutator {
    fn autostart_enabled(&mut self) -> Result<bool, MacosOwnerExecutionError> {
        Ok(false)
    }

    fn set_autostart(
        &mut self,
        _enabled: bool,
        _timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        panic!("runtime identity test must not change autostart")
    }

    fn bootout_exact(
        &mut self,
        _expected: &MacosDirectLaunchdOwnerProof,
        _timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        panic!("runtime identity test must not stop launchd")
    }

    fn bootstrap_and_kickstart_exact(
        &mut self,
        _source: &mut MacosDirectLaunchdBootstrapSource,
        expected: &MacosDirectLaunchdPublicationExpectation,
        _timeout: Duration,
    ) -> Result<
        MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof>,
        MacosOwnerExecutionError,
    > {
        *self
            .observed_executable
            .lock()
            .expect("capture start expectation") = Some((
            expected.executable().device(),
            expected.executable().inode(),
            expected.executable().cdhash().to_owned(),
        ));
        Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown)
    }
}

#[test]
fn native_executor_uses_retained_home_for_scaffolds_entries_and_private_launcher() {
    let fixture = ReleaseFixture::new();
    let home = tempfile::tempdir().expect("HOME");
    let home_path = fs::canonicalize(home.path()).expect("canonical HOME");
    let store = InstallStore::new(home_path.join(".local/lib/hypercolor"), 128 * 1024);
    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("anchored install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage complete candidate");
    drop(candidate);
    let candidate =
        retain_macos_unit(&store, &lock, &fixture.unit).expect("cold rebind complete candidate");
    let config = MacosInstallConfig {
        direct_plist_path: home_path
            .join("Library/LaunchAgents/tech.hyperbliss.hypercolor.plist")
            .to_string_lossy()
            .into_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.root().join("active"),
        log_directory: home_path.join("Library/Logs/hypercolor"),
    };
    let mut executor = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &home_path.join(".local"),
        &home_path.join(".local/bin"),
        config,
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        NoLaunchdMutator,
        NoLaunchdInspector,
    )
    .expect("native executor");
    executor
        .validate_unit_authority(&candidate)
        .expect("candidate belongs to retained units root");
    let projection = executor
        .candidate_layout(&candidate)
        .expect("bounded candidate projection");
    let before = executor
        .public_snapshot(std::slice::from_ref(&projection))
        .expect("fresh public snapshot");
    for directory in &projection.directories {
        if before.directories[directory] == MacosDirectoryState::Absent {
            executor
                .replace_directory(directory, MacosDirectoryState::Absent, true)
                .expect("create one retained scaffold");
        }
    }
    let (path, target) = &projection.entries[0];
    executor
        .replace_layout(
            path,
            &MacosExactEntry::Absent,
            Some(&MacosEntryPublication::Symlink(target.clone())),
        )
        .expect("publish one exact public link");
    let after = executor
        .public_snapshot(std::slice::from_ref(&projection))
        .expect("published public snapshot");
    assert_eq!(
        after.entries[path],
        MacosExactEntry::Symlink {
            target: target.clone()
        }
    );
    let launcher = MacosFilePublication {
        mode: 0o644,
        contents: b"<?xml version=\"1.0\"?><plist/>\n".to_vec(),
    };
    let snapshot = executor
        .persist_launcher_snapshot(&launcher)
        .expect("private launcher snapshot");
    executor
        .validate_launcher_snapshot(&launcher, &snapshot)
        .expect("private launcher identity");
    fs::write(
        store.root().join(&snapshot.relative_path),
        b"foreign private bytes",
    )
    .expect("tamper private snapshot");
    assert!(
        executor
            .validate_launcher_snapshot(&launcher, &snapshot)
            .is_err()
    );
}

#[test]
fn native_executor_discovers_complete_historical_inventory_without_sentinels() {
    let fixture = ReleaseFixture::new();
    let home = tempfile::tempdir().expect("HOME");
    let home_path = fs::canonicalize(home.path()).expect("canonical HOME");
    let install_prefix = home_path.join(".local");
    let install_dir = install_prefix.join("bin");
    let store = InstallStore::new(install_prefix.join("lib/hypercolor"), 128 * 1024);
    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("anchored install lock");
    let candidate = stage_release_payload(
        &store,
        &lock,
        fixture.path(),
        &fixture.candidate,
        &fixture.unit,
    )
    .expect("stage complete candidate");
    let historical = [
        (install_dir.join("hyper"), b"old-cli".as_slice(), 0o755),
        (
            install_dir.join("hypercolor-tray"),
            b"old-tray".as_slice(),
            0o700,
        ),
        (
            install_prefix.join("share/bash-completion/completions/hyper"),
            b"old-bash".as_slice(),
            0o644,
        ),
        (
            install_prefix.join("share/zsh/site-functions/_hyper"),
            b"old-zsh".as_slice(),
            0o600,
        ),
        (
            home_path.join(".config/fish/completions/hyper.fish"),
            b"old-fish".as_slice(),
            0o644,
        ),
        (
            install_prefix.join("share/hypercolor/ui/assets/nested/app.js"),
            b"old-ui".as_slice(),
            0o640,
        ),
        (
            install_prefix.join("share/hypercolor/effects/bundled/legacy.html"),
            b"old-effect".as_slice(),
            0o600,
        ),
        (
            install_prefix.join("share/icons/hicolor/128x128/apps/hypercolor.png"),
            b"old-icon".as_slice(),
            0o644,
        ),
        (
            install_prefix.join("share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg"),
            b"old-symbolic".as_slice(),
            0o644,
        ),
    ];
    for (path, bytes, mode) in &historical {
        write_mode(path, bytes, *mode);
    }
    let unrelated = install_prefix.join("share/icons/hicolor/128x128/apps/unrelated.png");
    write_mode(&unrelated, b"sentinel", 0o644);
    let config = native_config(&store, &home_path);
    let mut executor = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &install_prefix,
        &install_dir,
        config,
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        NoLaunchdMutator,
        NoLaunchdInspector,
    )
    .expect("native executor");
    let projection = executor
        .candidate_layout(&candidate)
        .expect("candidate projection");
    let snapshot = executor
        .public_snapshot(&[projection])
        .expect("complete historical snapshot");
    for (path, bytes, mode) in historical {
        let path = path.to_string_lossy().into_owned();
        assert!(matches!(
            snapshot.entries.get(&path),
            Some(MacosExactEntry::RegularFile { mode: actual, .. }) if *actual == mode
        ));
        assert_eq!(snapshot.regular_bytes[&path], bytes);
    }
    assert!(
        !snapshot
            .entries
            .contains_key(&unrelated.to_string_lossy().into_owned())
    );
    assert_eq!(
        fs::read(unrelated).expect("unrelated sentinel"),
        b"sentinel"
    );
}

#[test]
fn native_executor_publishes_reuses_and_cold_rebinds_synthetic_legacy_unit() {
    let home = tempfile::tempdir().expect("HOME");
    let home_path = fs::canonicalize(home.path()).expect("canonical HOME");
    let install_prefix = home_path.join(".local");
    let install_dir = install_prefix.join("bin");
    let daemon_path = install_dir.join("hypercolor-daemon");
    let public_path = install_dir.join("hyper");
    write_mode(&daemon_path, b"legacy-daemon", 0o555);
    write_mode(&public_path, b"legacy-cli", 0o700);
    let daemon_metadata = fs::metadata(&daemon_path).expect("legacy daemon metadata");
    let executable = MacosLegacyExecutable {
        path: daemon_path.to_string_lossy().into_owned(),
        sha256: sha256(b"legacy-daemon"),
        size: b"legacy-daemon".len() as u64,
        mode: 0o555,
        device: daemon_metadata.dev(),
        inode: daemon_metadata.ino(),
        designated_requirement: REQUIREMENT.to_owned(),
        designated_requirement_sha256: sha256(REQUIREMENT.as_bytes()),
        cdhash: CDHASH.to_owned(),
        version: "0.2.9".to_owned(),
    };
    let public_path = public_path.to_string_lossy().into_owned();
    let entries = BTreeMap::from([(
        public_path.clone(),
        MacosExactEntry::RegularFile {
            mode: 0o700,
            sha256: sha256(b"legacy-cli"),
            snapshot_unit: None,
            snapshot_path: None,
        },
    )]);
    let regular_bytes = BTreeMap::from([(public_path.clone(), b"legacy-cli".to_vec())]);
    let launcher = MacosFilePublication {
        mode: 0o600,
        contents: b"<?xml version=\"1.0\"?><plist>legacy</plist>\n".to_vec(),
    };
    let unit = legacy_snapshot_unit_id(Some(&launcher), &entries, &regular_bytes, &executable);
    let snapshot = MacosLegacySnapshot {
        unit: unit.clone(),
        version: executable.version.clone(),
        launcher: Some(launcher.clone()),
        entries: entries.clone().into_iter().collect(),
        regular_files: vec![MacosLegacyFile {
            path: public_path,
            mode: 0o700,
            contents: b"legacy-cli".to_vec(),
        }],
        executable: executable.clone(),
    };
    let store = InstallStore::new(install_prefix.join("lib/hypercolor"), 128 * 1024);
    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("anchored install lock");
    let config = native_config(&store, &home_path);
    let mut executor = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &install_prefix,
        &install_dir,
        config.clone(),
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        NoLaunchdMutator,
        NoLaunchdInspector,
    )
    .expect("native executor");
    let retained = executor
        .snapshot_legacy_unit(&snapshot)
        .expect("publish synthetic legacy unit");
    let reused = executor
        .snapshot_legacy_unit(&snapshot)
        .expect("reuse exact synthetic legacy unit");
    assert_eq!(retained, reused);
    drop(retained);
    drop(reused);
    drop(executor);
    drop(lock);

    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("cold anchored lock");
    let retained = retain_macos_unit(&store, &lock, &unit).expect("cold synthetic rebind");
    let mut executor = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &install_prefix,
        &install_dir,
        config,
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        NoLaunchdMutator,
        NoLaunchdInspector,
    )
    .expect("cold native executor");
    executor
        .validate_unit_authority(&retained)
        .expect("cold synthetic authority");
    executor
        .validate_legacy_snapshot(
            &retained,
            &executable,
            &MacosExactEntry::RegularFile {
                mode: launcher.mode,
                sha256: sha256(&launcher.contents),
                snapshot_unit: None,
                snapshot_path: None,
            },
            &launcher.contents,
            &entries,
        )
        .expect("cold synthetic content binding");
    drop(retained);
    drop(executor);
    drop(lock);

    let unit_root = store.root().join("units").join(unit.as_str());
    let index_path = unit_root.join("legacy-snapshot.json");
    let manifest_path = unit_root.join("manifest.json");
    let original_index = fs::read(&index_path).expect("canonical legacy index");
    let original_manifest = fs::read(&manifest_path).expect("canonical legacy manifest");
    let mut forged_index: serde_json::Value =
        serde_json::from_slice(&original_index).expect("parse legacy index");
    forged_index["executable"]["designated_requirement"] =
        serde_json::Value::String("identifier forged".to_owned());
    let forged_index = serde_json::to_vec(&forged_index).expect("encode forged legacy index");
    let mut forged_manifest: serde_json::Value =
        serde_json::from_slice(&original_manifest).expect("parse legacy manifest");
    forged_manifest["index_sha256"] = serde_json::Value::String(sha256(&forged_index));
    fs::write(&index_path, &forged_index).expect("write forged legacy index");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&forged_manifest).expect("encode forged legacy manifest"),
    )
    .expect("write forged legacy manifest");
    let lock = store
        .acquire_anchored_lock(&home_path)
        .expect("forged anchored lock");
    assert!(retain_macos_unit(&store, &lock, &unit).is_err());
    drop(lock);
    fs::write(&index_path, original_index).expect("restore canonical legacy index");
    fs::write(&manifest_path, original_manifest).expect("restore canonical legacy manifest");

    let original_index = fs::read(&index_path).expect("restored legacy index");
    let original_manifest = fs::read(&manifest_path).expect("restored legacy manifest");
    let mut forged_index: serde_json::Value =
        serde_json::from_slice(&original_index).expect("parse restored legacy index");
    forged_index["executable"]["cdhash"] = serde_json::Value::String("0".repeat(40));
    let forged_index = serde_json::to_vec(&forged_index).expect("encode forged CDHash index");
    let mut forged_manifest: serde_json::Value =
        serde_json::from_slice(&original_manifest).expect("parse restored legacy manifest");
    forged_manifest["index_sha256"] = serde_json::Value::String(sha256(&forged_index));
    fs::write(&index_path, &forged_index).expect("write forged CDHash index");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&forged_manifest).expect("encode forged CDHash manifest"),
    )
    .expect("write forged CDHash manifest");
    let lock = store
        .acquire_anchored_lock(&home_path)
        .expect("CDHash-forged anchored lock");
    assert!(retain_macos_unit(&store, &lock, &unit).is_err());
    drop(lock);
    fs::write(&index_path, original_index).expect("restore CDHash index");
    fs::write(&manifest_path, original_manifest).expect("restore CDHash manifest");

    fs::set_permissions(&unit_root, fs::Permissions::from_mode(0o777))
        .expect("weaken synthetic root mode");
    let lock = store
        .acquire_anchored_lock(&home_path)
        .expect("root-mode anchored lock");
    assert!(retain_macos_unit(&store, &lock, &unit).is_err());
    drop(lock);
    fs::set_permissions(&unit_root, fs::Permissions::from_mode(0o700))
        .expect("restore synthetic root mode");
    fs::set_permissions(unit_root.join("bin"), fs::Permissions::from_mode(0o777))
        .expect("weaken synthetic descendant mode");
    let lock = store
        .acquire_anchored_lock(&home_path)
        .expect("descendant-mode anchored lock");
    assert!(retain_macos_unit(&store, &lock, &unit).is_err());
    drop(lock);
    fs::set_permissions(unit_root.join("bin"), fs::Permissions::from_mode(0o700))
        .expect("restore synthetic descendant mode");

    fs::write(unit_root.join("public/00000.bin"), b"tampered").expect("tamper synthetic snapshot");
    let lock = store
        .acquire_anchored_lock(&home_path)
        .expect("tampered anchored lock");
    assert!(retain_macos_unit(&store, &lock, &unit).is_err());
}

#[test]
fn native_executor_inspects_inactive_raw_executable_without_launchd() {
    let home = tempfile::tempdir().expect("HOME");
    let home_path = fs::canonicalize(home.path()).expect("canonical HOME");
    let install_prefix = home_path.join(".local");
    let install_dir = install_prefix.join("bin");
    let daemon_path = install_dir.join("hypercolor-daemon");
    fs::create_dir_all(&install_dir).expect("raw install directory");
    copy_thin_signed_fixture(&daemon_path);
    fs::set_permissions(&daemon_path, fs::Permissions::from_mode(0o555))
        .expect("safe raw executable mode");
    let metadata = fs::metadata(&daemon_path).expect("raw executable metadata");
    let store = InstallStore::new(install_prefix.join("lib/hypercolor"), 128 * 1024);
    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("anchored install lock");
    let observed_start = Arc::new(Mutex::new(None));
    let mut executor = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &install_prefix,
        &install_dir,
        native_config(&store, &home_path),
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        CaptureStartMutator {
            observed_executable: Arc::clone(&observed_start),
        },
        NoLaunchdInspector,
    )
    .expect("native executor");
    let observed = executor
        .inspect_legacy_executable(None)
        .expect("inactive raw inspection")
        .expect("raw executable");
    assert_eq!(observed.path, daemon_path.to_string_lossy());
    assert_eq!(observed.size, metadata.len());
    assert_eq!(observed.mode, 0o555);
    assert_eq!(observed.device, metadata.dev());
    assert_eq!(observed.inode, metadata.ino());
    assert_eq!(
        observed.sha256,
        sha256(&fs::read(&daemon_path).expect("raw bytes"))
    );
    assert!(!observed.designated_requirement.is_empty());
    let launcher = MacosFilePublication {
        mode: 0o600,
        contents: b"<?xml version=\"1.0\"?><plist>legacy</plist>\n".to_vec(),
    };
    let launcher_snapshot = executor
        .persist_launcher_snapshot(&launcher)
        .expect("legacy launcher snapshot");
    let entries = BTreeMap::new();
    let regular_bytes = BTreeMap::new();
    let unit = legacy_snapshot_unit_id(Some(&launcher), &entries, &regular_bytes, &observed);
    let snapshot = MacosLegacySnapshot {
        unit: unit.clone(),
        version: observed.version.clone(),
        launcher: Some(launcher.clone()),
        entries: Vec::new(),
        regular_files: Vec::new(),
        executable: observed.clone(),
    };
    let retained = executor
        .snapshot_legacy_unit(&snapshot)
        .expect("publish signed synthetic snapshot");
    let displaced = install_dir.join("displaced-daemon");
    fs::rename(&daemon_path, displaced).expect("retain original executable inode");
    copy_thin_signed_fixture(&daemon_path);
    fs::set_permissions(&daemon_path, fs::Permissions::from_mode(0o555))
        .expect("restored raw executable mode");
    let restored = fs::metadata(&daemon_path).expect("restored executable metadata");
    assert_ne!(restored.ino(), observed.inode);
    let refreshed = executor
        .inspect_legacy_executable(None)
        .expect("refreshed signed raw inspection")
        .expect("restored raw executable");
    assert_eq!(refreshed.device, restored.dev());
    assert_eq!(refreshed.inode, restored.ino());
    let retry_snapshot = MacosLegacySnapshot {
        executable: refreshed,
        ..snapshot
    };
    let reused = executor
        .snapshot_legacy_unit(&retry_snapshot)
        .expect("reuse synthetic snapshot after rollback inode refresh");
    assert_eq!(retained, reused);
    assert_eq!(retry_snapshot.unit, unit);
    let outcome = executor
        .transition_runtime(&MacosRuntimeTransition::Start {
            executable: MacosRuntimeExecutable {
                unit: UnitId::new(format!("legacy-{}", "a".repeat(64))).expect("legacy unit"),
                path: observed.path,
                sha256: observed.sha256,
                size: observed.size,
                mode: observed.mode,
                device: observed.device,
                inode: observed.inode,
                designated_requirement: observed.designated_requirement,
                designated_requirement_sha256: observed.designated_requirement_sha256,
                cdhash: observed.cdhash,
                synthetic_legacy: true,
            },
            launcher_snapshot,
            after_epoch: 9,
        })
        .expect("bounded synthetic runtime submission");
    assert_eq!(outcome, MacosMutationOutcome::SubmittedUnknown);
    assert_eq!(
        *observed_start.lock().expect("captured start expectation"),
        Some((
            restored.dev(),
            restored.ino(),
            retry_snapshot.executable.cdhash
        ))
    );
}

#[test]
fn native_executor_rejects_same_path_store_replacement_before_observation() {
    let home = tempfile::tempdir().expect("HOME");
    let home_path = fs::canonicalize(home.path()).expect("canonical HOME");
    let store = InstallStore::new(home_path.join(".local/lib/hypercolor"), 128 * 1024);
    let mut lock = store
        .acquire_anchored_lock(&home_path)
        .expect("anchored install lock");
    let displaced = home_path.join("displaced-store");
    fs::rename(store.root(), &displaced).expect("displace retained store");
    fs::create_dir_all(store.root()).expect("replace canonical store path");
    let config = MacosInstallConfig {
        direct_plist_path: home_path
            .join("Library/LaunchAgents/tech.hyperbliss.hypercolor.plist")
            .to_string_lossy()
            .into_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.root().join("active"),
        log_directory: home_path.join("Library/Logs/hypercolor"),
    };
    let result = MacosNativeExecutor::new_with_launchd(
        &store,
        &mut lock,
        &home_path,
        &home_path.join(".local"),
        &home_path.join(".local/bin"),
        config,
        MacosOwnerStore::new(home_path.join("Library/Application Support/Hypercolor")),
        NoLaunchdMutator,
        NoLaunchdInspector,
    );
    assert!(result.is_err());
    assert!(!home_path.join("Library").exists());
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

fn native_config(store: &InstallStore, home: &Path) -> MacosInstallConfig {
    MacosInstallConfig {
        direct_plist_path: home
            .join("Library/LaunchAgents/tech.hyperbliss.hypercolor.plist")
            .to_string_lossy()
            .into_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.root().join("active"),
        log_directory: home.join("Library/Logs/hypercolor"),
    }
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().expect("fixture file parent")).expect("fixture parent");
    fs::write(path, bytes).expect("fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn copy_thin_signed_fixture(path: &Path) {
    fs::create_dir_all(path.parent().expect("signed fixture parent"))
        .expect("signed fixture parent directory");
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64e",
        "x86_64" => "x86_64",
        other => panic!("unsupported signed fixture architecture {other}"),
    };
    let status = std::process::Command::new("/usr/bin/lipo")
        .args(["/bin/ls", "-thin", architecture, "-output"])
        .arg(path)
        .status()
        .expect("run lipo for signed fixture");
    assert!(status.success(), "lipo must produce a signed thin fixture");
}

fn legacy_snapshot_unit_id(
    launcher: Option<&MacosFilePublication>,
    entries: &BTreeMap<String, MacosExactEntry>,
    regular_bytes: &BTreeMap<String, Vec<u8>>,
    executable: &MacosLegacyExecutable,
) -> UnitId {
    let mut digest = Sha256::new();
    let launcher_entry = launcher.map_or(MacosExactEntry::Absent, |launcher| {
        MacosExactEntry::RegularFile {
            mode: launcher.mode,
            sha256: sha256(&launcher.contents),
            snapshot_unit: None,
            snapshot_path: None,
        }
    });
    encode_legacy_entry(
        &mut digest,
        "launcher",
        &launcher_entry,
        launcher.map(|launcher| launcher.contents.as_slice()),
    );
    for (path, entry) in entries {
        encode_legacy_entry(
            &mut digest,
            path,
            entry,
            regular_bytes.get(path).map(Vec::as_slice),
        );
    }
    digest.update(b"executable\0");
    digest.update(executable.path.as_bytes());
    digest.update(b"\0");
    digest.update(executable.sha256.as_bytes());
    digest.update(b"\0");
    digest.update(executable.designated_requirement_sha256.as_bytes());
    digest.update(b"\0");
    digest.update(executable.cdhash.as_bytes());
    UnitId::new(format!("legacy-{}", sha256(&digest.finalize()))).expect("legacy unit ID")
}

fn encode_legacy_entry(
    digest: &mut Sha256,
    path: &str,
    entry: &MacosExactEntry,
    contents: Option<&[u8]>,
) {
    digest.update(path.as_bytes());
    digest.update(b"\0");
    match entry {
        MacosExactEntry::Absent => digest.update(b"absent\0"),
        MacosExactEntry::Symlink { target } => {
            digest.update(b"symlink\0");
            digest.update(target.as_bytes());
            digest.update(b"\0");
        }
        MacosExactEntry::RegularFile {
            mode,
            sha256: expected_sha,
            ..
        } => {
            let contents = contents.expect("regular legacy bytes");
            assert_eq!(&sha256(contents), expected_sha);
            digest.update(b"file\0");
            digest.update(mode.to_le_bytes());
            digest.update(contents);
            digest.update(b"\0");
        }
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
        let (daemon_bytes, daemon_cdhash) = thin_macho_daemon(daemon);
        let files = [
            ("bin/hypercolor-daemon", daemon_bytes.as_slice(), 0o755),
            ("bin/hypercolor", b"candidate".as_slice(), 0o755),
            ("bin/hypercolor-app", b"app".as_slice(), 0o755),
            ("bin/hypercolor-tui", b"tui".as_slice(), 0o755),
            ("bin/hypercolor-open", b"open".as_slice(), 0o755),
            ("share/hypercolor/ui/index.html", b"ui".as_slice(), 0o644),
            (
                "share/applications/hypercolor.desktop",
                b"[Desktop Entry]\nExec=hypercolor\n".as_slice(),
                0o644,
            ),
            (
                "share/bash-completion/completions/hypercolor",
                b"complete hypercolor\n".as_slice(),
                0o644,
            ),
            (
                "share/zsh/site-functions/_hypercolor",
                b"#compdef hypercolor\n".as_slice(),
                0o644,
            ),
            (
                "share/fish/vendor_completions.d/hypercolor.fish",
                b"complete -c hypercolor\n".as_slice(),
                0o644,
            ),
            (
                "share/icons/hicolor/48x48/apps/hypercolor.png",
                b"icon".as_slice(),
                0o644,
            ),
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
                "cdhash": daemon_cdhash,
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
            "share/applications",
            "share/bash-completion",
            "share/bash-completion/completions",
            "share/zsh",
            "share/zsh/site-functions",
            "share/fish",
            "share/fish/vendor_completions.d",
            "share/icons",
            "share/icons/hicolor",
            "share/icons/hicolor/48x48",
            "share/icons/hicolor/48x48/apps",
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

fn thin_macho_daemon(payload: &[u8]) -> (Vec<u8>, String) {
    const MACH_MAGIC_64: u32 = 0xfeed_facf;
    const LC_CODE_SIGNATURE: u32 = 0x1d;
    const EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
    const CODE_DIRECTORY: u32 = 0xfade_0c02;
    let cpu_type = match std::env::consts::ARCH {
        "aarch64" => 0x0100_000c_u32,
        "x86_64" => 0x0100_0007_u32,
        architecture => panic!("unsupported fixture architecture {architecture}"),
    };
    let mut code_directory = vec![0_u8; 40];
    code_directory[0..4].copy_from_slice(&CODE_DIRECTORY.to_be_bytes());
    code_directory[4..8].copy_from_slice(&40_u32.to_be_bytes());
    code_directory[36] = 32;
    code_directory[37] = 2;
    let mut signature = vec![0_u8; 20];
    signature[0..4].copy_from_slice(&EMBEDDED_SIGNATURE.to_be_bytes());
    signature[4..8].copy_from_slice(&60_u32.to_be_bytes());
    signature[8..12].copy_from_slice(&1_u32.to_be_bytes());
    signature[12..16].copy_from_slice(&0_u32.to_be_bytes());
    signature[16..20].copy_from_slice(&20_u32.to_be_bytes());
    signature.extend_from_slice(&code_directory);
    let signature_offset = 48_u32 + u32::try_from(payload.len()).expect("fixture payload bound");
    let mut macho = vec![0_u8; 48];
    macho[0..4].copy_from_slice(&MACH_MAGIC_64.to_le_bytes());
    macho[4..8].copy_from_slice(&cpu_type.to_le_bytes());
    macho[16..20].copy_from_slice(&1_u32.to_le_bytes());
    macho[20..24].copy_from_slice(&16_u32.to_le_bytes());
    macho[32..36].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
    macho[36..40].copy_from_slice(&16_u32.to_le_bytes());
    macho[40..44].copy_from_slice(&signature_offset.to_le_bytes());
    macho[44..48].copy_from_slice(&60_u32.to_le_bytes());
    macho.extend_from_slice(payload);
    macho.extend_from_slice(&signature);
    let digest = Sha256::digest(code_directory);
    (macho, hex_bytes(&digest[..20]))
}

fn retain_candidate(store: &InstallStore, id: &UnitId) -> UnitRecord {
    let lock = store.acquire_lock().expect("retain lock");
    retain_macos_unit(store, &lock, id).expect("retain installed macOS unit")
}

fn legacy_unit(snapshot: &MacosLegacySnapshot, daemon_bytes: &[u8]) -> UnitRecord {
    let parent = tempfile::tempdir().expect("legacy authority parent");
    let root = parent.path().join("legacy");
    fs::create_dir(&root).expect("legacy authority directory");
    let regular_files = snapshot
        .regular_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut stored_entries = serde_json::Map::new();
    for (position, (path, entry)) in snapshot.entries.iter().enumerate() {
        let stored = match entry {
            MacosExactEntry::Absent => json!({"kind":"absent"}),
            MacosExactEntry::Symlink { target } => {
                json!({"kind":"symlink","target":target})
            }
            MacosExactEntry::RegularFile { mode, sha256, .. } => {
                let file = regular_files[path.as_str()];
                let relative = format!("public/{position:05}.bin");
                write_mode(&root.join(&relative), &file.contents, *mode);
                json!({
                    "kind":"regular_file",
                    "file":{
                        "relative_path":relative,
                        "mode":mode,
                        "sha256":sha256,
                        "size":file.contents.len(),
                    }
                })
            }
        };
        stored_entries.insert(path.clone(), stored);
    }
    let launcher = snapshot.launcher.as_ref().map(|launcher| {
        write_mode(
            &root.join("launchd/prior.plist"),
            &launcher.contents,
            launcher.mode,
        );
        json!({
            "relative_path":"launchd/prior.plist",
            "mode":launcher.mode,
            "sha256":sha256(&launcher.contents),
            "size":launcher.contents.len(),
        })
    });
    let executable = &snapshot.executable;
    let index = serde_json::to_vec(&json!({
        "schema_version":2,
        "unit":snapshot.unit,
        "version":snapshot.version,
        "executable":{
            "path":executable.path,
            "sha256":executable.sha256,
            "size":executable.size,
            "mode":executable.mode,
            "device":executable.device,
            "inode":executable.inode,
            "designated_requirement":executable.designated_requirement,
            "designated_requirement_sha256":executable.designated_requirement_sha256,
            "cdhash":executable.cdhash,
            "version":executable.version,
        },
        "launcher":launcher,
        "entries":stored_entries,
    }))
    .expect("legacy index");
    let manifest = serde_json::to_vec(&json!({
        "name":"hypercolor-macos-legacy-snapshot",
        "version":snapshot.version,
        "unit":snapshot.unit,
        "index_sha256":sha256(&index),
    }))
    .expect("legacy manifest");
    write_mode(&root.join("manifest.json"), &manifest, 0o644);
    write_mode(&root.join("legacy-snapshot.json"), &index, 0o644);
    write_mode(
        &root.join("bin/hypercolor-daemon"),
        daemon_bytes,
        snapshot.executable.mode,
    );
    for relative in ["", "bin", "launchd", "public"] {
        let directory = root.join(relative);
        if directory.exists() {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("canonical private legacy directory mode");
        }
    }
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
    bind_macos_retained_legacy_unit(snapshot.unit.clone(), root, authority)
        .expect("bind complete legacy unit")
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

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("write hexadecimal bytes");
            output
        },
    )
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
