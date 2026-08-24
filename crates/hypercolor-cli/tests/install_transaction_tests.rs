#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use hypercolor_cli::install::{
    InstallAction, InstallCoordinator, InstallCoordinatorError, InstallDisposition,
    InstallJournalV1, InstallModelError, InstallOutcome, InstallPlatform, InstallPlatformError,
    InstallRequest, InstallStore, InstallStoreError, InstallTargetPolicy, InstallTransactionId,
    MAX_PLATFORM_OWNER_RECEIPT_BYTES, MAX_PLATFORM_TRANSACTION_RECORD_BYTES, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, PlatformTransitionStates,
    PreparedPlatformTransaction, UnitId, UnitRecord, stage_release_payload,
};
use hypercolor_platform_fs::DirectoryEntryKind;
use serde_json::json;
use sha2::{Digest as _, Sha256};

const PRIOR_ID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CANDIDATE_ID: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const THIRD_ID: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const RELEASE_BINARIES: [&str; 5] = [
    "hypercolor-daemon",
    "hypercolor",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectionKind {
    Fail,
    FailAfter,
    DriftAfter,
    PanicAfter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectRecord {
    action: InstallAction,
    active_target: Option<PathBuf>,
}

struct FakePlatform {
    state: PlatformState,
    journal_path: PathBuf,
    active_path: PathBuf,
    effects: Vec<EffectRecord>,
    injections: Vec<(InstallAction, InjectionKind)>,
    deny_active_switch: Option<InstallAction>,
    restore_directory_permissions: bool,
    store_root: PathBuf,
    exact_state_valid: bool,
    prior_restored: bool,
    layout_operation_progress: u16,
    layout_state_drifted: bool,
    candidate_launcher_installed: bool,
}

impl FakePlatform {
    fn new(state: PlatformState, store: &InstallStore) -> Self {
        Self {
            state,
            journal_path: store.journal_path(),
            active_path: store.active_path(),
            effects: Vec::new(),
            injections: Vec::new(),
            deny_active_switch: None,
            restore_directory_permissions: false,
            store_root: store.root().to_path_buf(),
            exact_state_valid: true,
            prior_restored: false,
            layout_operation_progress: 0,
            layout_state_drifted: false,
            candidate_launcher_installed: false,
        }
    }

    fn inject(&mut self, action: InstallAction, kind: InjectionKind) {
        self.injections.push((action, kind));
    }

    fn begin_effect(
        &mut self,
    ) -> Result<(InstallAction, Option<InjectionKind>), InstallPlatformError> {
        let journal: InstallJournalV1 = serde_json::from_slice(
            &fs::read(&self.journal_path).expect("effect must observe a durable journal"),
        )
        .expect("effect journal must decode");
        let action = journal
            .next_action
            .expect("effect must be write-ahead named");
        self.effects.push(EffectRecord {
            action,
            active_target: fs::read_link(&self.active_path).ok(),
        });
        let injection = self
            .injections
            .iter()
            .position(|(injected, _)| *injected == action)
            .map(|index| self.injections.remove(index).1);
        if injection == Some(InjectionKind::Fail) {
            return Err(InstallPlatformError::new(format!(
                "injected {action:?} failure"
            )));
        }
        Ok((action, injection))
    }

    fn finish_effect(
        action: InstallAction,
        injection: Option<InjectionKind>,
    ) -> Result<(), InstallPlatformError> {
        match injection {
            Some(InjectionKind::PanicAfter) => panic!("injected crash after {action:?}"),
            Some(InjectionKind::FailAfter | InjectionKind::DriftAfter) => Err(
                InstallPlatformError::new(format!("injected {action:?} failure after mutation")),
            ),
            Some(InjectionKind::Fail) => unreachable!("pre-mutation failure returned from begin"),
            None => Ok(()),
        }
    }

    fn transaction_record() -> PlatformTransactionRecord {
        PlatformTransactionRecord::linux(1, b"exact fake launcher and owner proof".to_vec())
            .expect("valid fake platform record")
    }

    fn assert_record(record: &PlatformTransactionRecord) {
        assert_eq!(record, &Self::transaction_record());
    }

    fn owner_receipt() -> PlatformOwnerReceipt {
        PlatformOwnerReceipt::linux(1, b"candidate systemd invocation".to_vec())
            .expect("valid fake owner receipt")
    }

    fn journal(&self) -> InstallJournalV1 {
        serde_json::from_slice(&fs::read(&self.journal_path).expect("journal bytes"))
            .expect("journal JSON")
    }

    fn active_unit(&self) -> Option<UnitId> {
        fs::read_link(&self.active_path)
            .ok()
            .and_then(|target| target.file_name().map(ToOwned::to_owned))
            .and_then(|name| UnitId::new(name.to_string_lossy()).ok())
    }

    fn transitions(prior: &PlatformState, target: &PlatformState) -> PlatformTransitionStates {
        let prior_unloaded = PlatformState {
            loaded: false,
            running_unit: None,
            ..prior.clone()
        };
        let candidate_manager = PlatformState {
            loaded: target.loaded,
            running_unit: None,
            autostart_enabled: prior.autostart_enabled,
            ..target.clone()
        };
        let candidate_autostart = PlatformState {
            autostart_enabled: target.autostart_enabled,
            ..candidate_manager.clone()
        };
        let prior_manager = PlatformState {
            loaded: prior.loaded,
            running_unit: None,
            autostart_enabled: prior.autostart_enabled,
            ..prior.clone()
        };
        let prior_autostart = PlatformState {
            autostart_enabled: prior.autostart_enabled,
            ..prior_manager.clone()
        };
        PlatformTransitionStates {
            prior_unloaded,
            candidate_manager,
            candidate_autostart,
            prior_manager,
            prior_autostart,
        }
    }

    fn candidate_active(prior: &PlatformState, target: &PlatformState) -> PlatformState {
        PlatformState {
            layout_unit: target.layout_unit.clone(),
            launcher_unit: target.launcher_unit.clone(),
            loaded: false,
            running_unit: None,
            autostart_enabled: prior.autostart_enabled,
        }
    }

    fn restore_install_permissions(&mut self) {
        fs::set_permissions(&self.store_root, fs::Permissions::from_mode(0o755))
            .expect("restore install directory permissions");
        self.restore_directory_permissions = false;
    }
}

impl InstallPlatform for FakePlatform {
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError> {
        if self.restore_directory_permissions {
            fs::set_permissions(&self.store_root, fs::Permissions::from_mode(0o755))
                .expect("restore install directory permissions");
            self.restore_directory_permissions = false;
        }
        if let Some(action) = self.deny_active_switch
            && fs::read(&self.journal_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<InstallJournalV1>(&bytes).ok())
                .and_then(|journal| journal.next_action)
                == Some(action)
        {
            fs::set_permissions(&self.store_root, fs::Permissions::from_mode(0o500))
                .expect("deny active switch");
            self.deny_active_switch = None;
            self.restore_directory_permissions = true;
        }
        let active = self.active_unit();
        if self.layout_operation_progress == 3 && !self.layout_state_drifted {
            self.state.layout_unit.clone_from(&active);
        }
        if self.candidate_launcher_installed {
            self.state.launcher_unit = active;
        }
        Ok(self.state.clone())
    }

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &hypercolor_cli::install::InstallationState,
        target: &PlatformState,
    ) -> Result<PreparedPlatformTransaction, InstallPlatformError> {
        assert!(!candidate.id().as_str().is_empty());
        assert_eq!(
            candidate
                .directory()
                .metadata()
                .expect("candidate directory metadata")
                .kind(),
            DirectoryEntryKind::Directory
        );
        assert_eq!(prior.platform, self.state);
        Ok(PreparedPlatformTransaction {
            record: Self::transaction_record(),
            transitions: Self::transitions(&prior.platform, target),
            layout_operation_count: 3,
        })
    }

    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<bool, InstallPlatformError> {
        Self::assert_record(record);
        let journal = self.journal();
        assert_eq!(
            candidate_owner_receipt,
            journal.candidate_owner_receipt.as_ref()
        );
        let incarnation_matches = match checkpoint {
            PlatformCheckpoint::PriorOriginal => !self.prior_restored,
            PlatformCheckpoint::PriorRestored => self.prior_restored,
            _ => true,
        };
        let candidate_launcher = journal.target_platform.launcher_unit.is_some()
            && matches!(
                checkpoint,
                PlatformCheckpoint::CandidateLauncher
                    | PlatformCheckpoint::CandidateActive
                    | PlatformCheckpoint::CandidateManager
                    | PlatformCheckpoint::CandidateAutostart
                    | PlatformCheckpoint::CandidateRuntime
                    | PlatformCheckpoint::PriorActiveRestored
            );
        let layout_checkpoint = matches!(
            checkpoint,
            PlatformCheckpoint::CandidateLayout | PlatformCheckpoint::PriorLayoutRestored
        );
        let state_matches = if layout_checkpoint {
            self.state.launcher_unit == expected.launcher_unit
                && self.state.loaded == expected.loaded
                && self.state.running_unit == expected.running_unit
                && self.state.autostart_enabled == expected.autostart_enabled
        } else {
            &self.state == expected
        };
        Ok(self.exact_state_valid
            && incarnation_matches
            && self.layout_operation_progress == layout_operation_index
            && self.candidate_launcher_installed == candidate_launcher
            && (!layout_checkpoint || !self.layout_state_drifted)
            && state_matches)
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
        Self::assert_record(record);
        if &self.state != expected {
            return Err(InstallPlatformError::new(
                "candidate owner receipt does not match runtime",
            ));
        }
        Ok(Self::owner_receipt())
    }

    fn validate_transaction_plan(
        &mut self,
        prior: &PlatformState,
        target: &PlatformState,
        transitions: &PlatformTransitionStates,
        layout_operation_count: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        Self::assert_record(record);
        if layout_operation_count != 3 || transitions != &Self::transitions(prior, target) {
            return Err(InstallPlatformError::new("unexpected transition plan"));
        }
        Ok(())
    }

    fn preflight_authority(
        &mut self,
        candidate: &UnitId,
        prior: &hypercolor_cli::install::InstallationState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        assert_eq!(action, InstallAction::PreflightCandidate);
        assert_eq!(candidate.as_str().len(), 64);
        assert_eq!(prior.platform, self.state);
        Self::assert_record(record);
        Self::finish_effect(action, injection)
    }

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::ProvePriorGuardReleased | InstallAction::ProveCandidateGuardReleased
        ));
        assert_eq!(&self.state, unloaded);
        Self::finish_effect(action, injection)
    }

    fn install_launcher(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::InstallCandidateLauncher | InstallAction::RestorePriorLauncher
        ));
        assert_eq!(
            checkpoint,
            if action == InstallAction::InstallCandidateLauncher {
                PlatformCheckpoint::CandidateLauncher
            } else {
                PlatformCheckpoint::PriorLauncherRestored
            }
        );
        self.candidate_launcher_installed =
            action == InstallAction::InstallCandidateLauncher && unit.is_some();
        self.state.launcher_unit = unit.is_some().then(|| self.active_unit()).flatten();
        Self::finish_effect(action, injection)
    }

    fn install_layout_operation(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        operation_index: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::InstallCandidateLayout | InstallAction::RestorePriorLayout
        ));
        assert_eq!(
            checkpoint,
            if action == InstallAction::InstallCandidateLayout {
                PlatformCheckpoint::CandidateLayout
            } else {
                PlatformCheckpoint::PriorLayoutRestored
            }
        );
        if action == InstallAction::InstallCandidateLayout {
            assert_eq!(operation_index, self.layout_operation_progress);
            self.layout_operation_progress += 1;
            if self.layout_operation_progress == 3 {
                self.state.layout_unit = unit.is_some().then(|| self.active_unit()).flatten();
            }
        } else {
            assert_eq!(operation_index + 1, self.layout_operation_progress);
            self.layout_operation_progress -= 1;
            self.state.layout_unit = unit.cloned();
        }
        if injection == Some(InjectionKind::DriftAfter) {
            self.state.layout_unit = Some(UnitId::new(THIRD_ID).expect("third-state unit ID"));
            self.layout_state_drifted = true;
        }
        Self::finish_effect(action, injection)
    }

    fn reload_manager(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::ReloadCandidateManager
                | InstallAction::UnloadCandidateManager
                | InstallAction::ReloadPriorManager
        ));
        self.state.clone_from(expected);
        Self::finish_effect(action, injection)
    }

    fn restore_autostart(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::RestoreCandidateAutostart
                | InstallAction::UnloadCandidateAutostart
                | InstallAction::RestorePriorAutostart
        ));
        self.state.autostart_enabled = expected.autostart_enabled;
        Self::finish_effect(action, injection)
    }

    fn restore_runtime(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert_eq!(
            candidate_owner_receipt,
            self.journal().candidate_owner_receipt.as_ref()
        );
        assert!(matches!(
            action,
            InstallAction::UnloadPrior
                | InstallAction::RestoreCandidateRuntime
                | InstallAction::UnloadCandidateRuntime
                | InstallAction::RestorePriorRuntime
        ));
        self.state.loaded = expected.loaded;
        self.state.running_unit.clone_from(&expected.running_unit);
        if action == InstallAction::RestorePriorRuntime {
            self.prior_restored = true;
        }
        Self::finish_effect(action, injection)
    }

    fn wait_for_newer_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError> {
        let (action, injection) = self.begin_effect()?;
        Self::assert_record(record);
        assert_eq!(
            candidate_owner_receipt,
            self.journal().candidate_owner_receipt.as_ref()
        );
        assert!(matches!(
            action,
            InstallAction::ProveCandidate | InstallAction::ProvePrior
        ));
        assert_eq!(
            checkpoint,
            if action == InstallAction::ProveCandidate {
                PlatformCheckpoint::CandidateRuntime
            } else {
                PlatformCheckpoint::PriorRestored
            }
        );
        if &self.state != expected {
            return Err(InstallPlatformError::new("publication does not match"));
        }
        Self::finish_effect(action, injection)
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: InstallStore,
    prior: UnitRecord,
    candidate: UnitRecord,
    owner_sentinel: PathBuf,
    config_sentinel: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary install fixture");
        let prefix = directory.path().join("install");
        let store = InstallStore::new(&prefix, 64 * 1024);
        let prior = prepare_unit(&store, PRIOR_ID);
        let candidate = prepare_unit(&store, CANDIDATE_ID);
        let lock = store.acquire_lock().expect("initial install lock");
        store
            .set_active(Some(prior.id()), &lock)
            .expect("initial active unit");
        drop(lock);

        let owner_sentinel = directory.path().join("owner-state.json");
        let config_sentinel = directory.path().join("hypercolor.toml");
        fs::write(&owner_sentinel, b"owner sentinel").expect("owner sentinel");
        fs::write(&config_sentinel, b"config sentinel").expect("config sentinel");
        Self {
            directory,
            store,
            prior,
            candidate,
            owner_sentinel,
            config_sentinel,
        }
    }

    fn prior_state(&self) -> PlatformState {
        PlatformState {
            layout_unit: Some(self.prior.id().clone()),
            launcher_unit: Some(self.prior.id().clone()),
            loaded: true,
            running_unit: Some(self.prior.id().clone()),
            autostart_enabled: true,
        }
    }

    fn request(&self) -> InstallRequest {
        InstallRequest {
            transaction_id: InstallTransactionId::new("test-transaction")
                .expect("valid transaction ID"),
            candidate: self.candidate.clone(),
            target_policy: InstallTargetPolicy::Preserve,
        }
    }

    fn assert_sentinels(&self) {
        assert_eq!(
            fs::read(&self.owner_sentinel).expect("owner sentinel remains"),
            b"owner sentinel"
        );
        assert_eq!(
            fs::read(&self.config_sentinel).expect("config sentinel remains"),
            b"config sentinel"
        );
    }

    fn active_unit(&self) -> Option<UnitId> {
        let lock = self.store.acquire_lock().expect("inspect active lock");
        self.store.active_unit(&lock).expect("inspect active unit")
    }

    fn journal(&self) -> InstallJournalV1 {
        let lock = self.store.acquire_lock().expect("inspect journal lock");
        self.store
            .load_journal(&lock)
            .expect("load journal")
            .expect("journal exists")
    }
}

fn prepare_unit(store: &InstallStore, value: &str) -> UnitRecord {
    let release = tempfile::tempdir().expect("transaction release root");
    let candidate = write_transaction_release(release.path(), value);
    let manifest = fs::read(release.path().join("manifest.json")).expect("read release manifest");
    let expected_unit = UnitId::new(transaction_sha256(&manifest)).expect("valid manifest digest");
    let lock = store.acquire_lock().expect("transaction release lock");
    stage_release_payload(store, &lock, release.path(), &candidate, &expected_unit)
        .expect("stage transaction release")
}

fn write_transaction_release(root: &Path, seed: &str) -> File {
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
    let files = [
        ("bin/hypercolor-daemon", b"daemon".as_slice()),
        ("bin/hypercolor", seed.as_bytes()),
        ("bin/hypercolor-app", b"app".as_slice()),
        ("bin/hypercolor-tui", b"tui".as_slice()),
        ("bin/hypercolor-open", b"open".as_slice()),
        ("share/hypercolor/ui/index.html", b"ui".as_slice()),
        (
            "share/hypercolor/effects/bundled/effect.html",
            b"effect".as_slice(),
        ),
        (
            "share/hypercolor/agents/skills/skill.md",
            b"skill".as_slice(),
        ),
        (
            "share/hypercolor/agents/agents/agent.md",
            b"agent".as_slice(),
        ),
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("create release directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755))
            .expect("set release directory mode");
        members.push(json!({"path": directory, "type": "directory", "mode": 0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), bytes).expect("write release file");
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode))
            .expect("set release file mode");
        members.push(json!({
            "path": path,
            "type": "file",
            "mode": mode,
            "size": bytes.len(),
            "sha256": transaction_sha256(bytes),
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = serde_json::to_vec_pretty(&json!({
        "name": "hypercolor",
        "version": "0.3.2",
        "platform": "macos-arm64",
        "rust_target": "aarch64-apple-darwin",
        "binaries": RELEASE_BINARIES,
        "assets": {
            "ui_files": 1,
            "bundled_effect_files": 1,
            "docs_files": 0,
            "skill_files": 1,
            "agent_files": 1,
            "site_files": 0,
        },
        "members": members,
    }))
    .expect("encode release manifest");
    fs::write(root.join("manifest.json"), manifest).expect("write release manifest");
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("set release manifest mode");
    File::open(root.join("bin/hypercolor")).expect("open release candidate")
}

fn transaction_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("write digest");
    }
    output
}

fn install(fixture: &Fixture, platform: &mut FakePlatform) -> InstallOutcome {
    InstallCoordinator::new(&fixture.store, platform)
        .install(fixture.request())
        .expect("install transaction")
}

fn target_state(
    prior: &PlatformState,
    candidate: &UnitId,
    policy: InstallTargetPolicy,
) -> PlatformState {
    let (launcher_unit, loaded, running_unit, autostart_enabled) = match policy {
        InstallTargetPolicy::Preserve => (
            prior.launcher_unit.as_ref().map(|_| candidate.clone()),
            prior.loaded,
            prior.running_unit.as_ref().map(|_| candidate.clone()),
            prior.autostart_enabled,
        ),
        InstallTargetPolicy::EnableOnFirstInstall if prior.launcher_unit.is_none() => {
            (Some(candidate.clone()), true, Some(candidate.clone()), true)
        }
        InstallTargetPolicy::EnableOnFirstInstall => (
            Some(candidate.clone()),
            prior.loaded,
            prior.running_unit.as_ref().map(|_| candidate.clone()),
            prior.autostart_enabled,
        ),
        InstallTargetPolicy::EnabledAndRunning => {
            (Some(candidate.clone()), true, Some(candidate.clone()), true)
        }
        InstallTargetPolicy::Disabled => (Some(candidate.clone()), false, None, false),
    };
    PlatformState {
        layout_unit: Some(candidate.clone()),
        launcher_unit,
        loaded,
        running_unit,
        autostart_enabled,
    }
}

fn new_journal(
    transaction_id: InstallTransactionId,
    prior_active_unit: Option<UnitId>,
    candidate_unit: UnitId,
    prior_platform: PlatformState,
    target_policy: InstallTargetPolicy,
) -> InstallJournalV1 {
    let target = target_state(&prior_platform, &candidate_unit, target_policy);
    InstallJournalV1::new(
        transaction_id,
        prior_active_unit,
        candidate_unit,
        prior_platform.clone(),
        target_policy,
        FakePlatform::transitions(&prior_platform, &target),
        3,
        FakePlatform::transaction_record(),
    )
    .expect("journal")
}

fn seed_rollback(fixture: &Fixture, action: InstallAction) -> FakePlatform {
    seed_rollback_for_policy(fixture, action, InstallTargetPolicy::Preserve)
}

fn seed_rollback_for_policy(
    fixture: &Fixture,
    action: InstallAction,
    target_policy: InstallTargetPolicy,
) -> FakePlatform {
    let prior = fixture.prior_state();
    let target = target_state(&prior, fixture.candidate.id(), target_policy);
    let transitions = FakePlatform::transitions(&prior, &target);
    let candidate_active = FakePlatform::candidate_active(&prior, &target);
    let prior_active_restored = PlatformState {
        layout_unit: Some(fixture.prior.id().clone()),
        launcher_unit: Some(fixture.prior.id().clone()),
        ..candidate_active.clone()
    };
    let prior_launcher_restored = PlatformState {
        launcher_unit: prior.launcher_unit.clone(),
        ..prior_active_restored.clone()
    };
    let prior_layout_restored = PlatformState {
        layout_unit: prior.layout_unit.clone(),
        ..prior_launcher_restored.clone()
    };
    let (active, state) = match action {
        InstallAction::UnloadCandidateRuntime => (Some(fixture.candidate.id()), target),
        InstallAction::UnloadCandidateAutostart => (
            Some(fixture.candidate.id()),
            transitions.candidate_autostart,
        ),
        InstallAction::UnloadCandidateManager => {
            (Some(fixture.candidate.id()), transitions.candidate_manager)
        }
        InstallAction::ProveCandidateGuardReleased | InstallAction::RestorePriorActive => {
            (Some(fixture.candidate.id()), candidate_active)
        }
        InstallAction::RestorePriorLauncher => (Some(fixture.prior.id()), prior_active_restored),
        InstallAction::RestorePriorLayout => (Some(fixture.prior.id()), prior_launcher_restored),
        InstallAction::ReloadPriorManager => (Some(fixture.prior.id()), prior_layout_restored),
        InstallAction::RestorePriorAutostart => {
            (Some(fixture.prior.id()), transitions.prior_manager)
        }
        InstallAction::RestorePriorRuntime => {
            (Some(fixture.prior.id()), transitions.prior_autostart)
        }
        InstallAction::ProvePrior => (Some(fixture.prior.id()), fixture.prior_state()),
        _ => panic!("{action:?} is not a rollback effect"),
    };
    let mut journal = new_journal(
        InstallTransactionId::new("rollback-replay").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        target_policy,
    );
    journal.revision = 20;
    journal.disposition = InstallDisposition::Rollback;
    journal.next_action = Some(action);
    journal.failure = Some("seeded forward failure".to_owned());
    if action == InstallAction::UnloadCandidateRuntime {
        journal.candidate_owner_receipt = Some(FakePlatform::owner_receipt());
    }
    journal.layout_operation_index = if matches!(
        action,
        InstallAction::UnloadCandidateRuntime
            | InstallAction::UnloadCandidateAutostart
            | InstallAction::UnloadCandidateManager
            | InstallAction::ProveCandidateGuardReleased
            | InstallAction::RestorePriorActive
            | InstallAction::RestorePriorLauncher
            | InstallAction::RestorePriorLayout
    ) {
        3
    } else {
        0
    };
    let lock = fixture.store.acquire_lock().expect("seed rollback lock");
    fixture
        .store
        .set_active(active, &lock)
        .expect("seed rollback active unit");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed rollback journal");
    drop(lock);
    let mut platform = FakePlatform::new(state, &fixture.store);
    platform.layout_operation_progress = journal.layout_operation_index;
    platform.candidate_launcher_installed = matches!(
        action,
        InstallAction::UnloadCandidateRuntime
            | InstallAction::UnloadCandidateAutostart
            | InstallAction::UnloadCandidateManager
            | InstallAction::ProveCandidateGuardReleased
            | InstallAction::RestorePriorActive
            | InstallAction::RestorePriorLauncher
    );
    platform.prior_restored = action == InstallAction::ProvePrior;
    platform
}

fn seed_state_neutral_rollback_manager(fixture: &Fixture, action: InstallAction) -> FakePlatform {
    assert!(matches!(
        action,
        InstallAction::UnloadCandidateManager | InstallAction::ReloadPriorManager
    ));
    let prior = PlatformState {
        layout_unit: Some(fixture.prior.id().clone()),
        launcher_unit: Some(fixture.prior.id().clone()),
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };
    let target = target_state(
        &prior,
        fixture.candidate.id(),
        InstallTargetPolicy::Disabled,
    );
    let transitions = FakePlatform::transitions(&prior, &target);
    let candidate_active = FakePlatform::candidate_active(&prior, &target);
    let prior_layout_restored = PlatformState {
        layout_unit: prior.layout_unit.clone(),
        launcher_unit: prior.launcher_unit.clone(),
        ..candidate_active.clone()
    };
    let (active, state, layout_operation_index, candidate_launcher_installed) = match action {
        InstallAction::UnloadCandidateManager => (
            Some(fixture.candidate.id()),
            transitions.candidate_manager.clone(),
            3,
            true,
        ),
        InstallAction::ReloadPriorManager => {
            (Some(fixture.prior.id()), prior_layout_restored, 0, false)
        }
        _ => unreachable!("manager action checked above"),
    };
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("state-neutral-rollback-manager").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        prior,
        InstallTargetPolicy::Disabled,
        transitions,
        3,
        FakePlatform::transaction_record(),
    )
    .expect("state-neutral rollback journal");
    journal.revision = 20;
    journal.disposition = InstallDisposition::Rollback;
    journal.next_action = Some(action);
    journal.failure = Some("seeded state-neutral rollback".to_owned());
    journal.layout_operation_index = layout_operation_index;
    let lock = fixture.store.acquire_lock().expect("seed rollback lock");
    fixture
        .store
        .set_active(active, &lock)
        .expect("seed rollback active unit");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed rollback journal");
    drop(lock);
    let mut platform = FakePlatform::new(state, &fixture.store);
    platform.layout_operation_progress = layout_operation_index;
    platform.candidate_launcher_installed = candidate_launcher_installed;
    platform
}

#[test]
fn successful_install_preserves_write_ahead_order_and_private_journal() {
    let fixture = Fixture::new();
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);

    let outcome = install(&fixture, &mut platform);

    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: fixture.candidate.id().clone()
        }
    );
    assert_eq!(fixture.active_unit(), Some(fixture.candidate.id().clone()));
    assert_eq!(
        platform.effects,
        [
            EffectRecord {
                action: InstallAction::PreflightCandidate,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::UnloadPrior,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::ProvePriorGuardReleased,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::InstallCandidateLayout,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::InstallCandidateLayout,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::InstallCandidateLayout,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::InstallCandidateLauncher,
                active_target: Some(Path::new("units").join(fixture.prior.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::ReloadCandidateManager,
                active_target: Some(Path::new("units").join(fixture.candidate.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::RestoreCandidateRuntime,
                active_target: Some(Path::new("units").join(fixture.candidate.id().as_str())),
            },
            EffectRecord {
                action: InstallAction::ProveCandidate,
                active_target: Some(Path::new("units").join(fixture.candidate.id().as_str())),
            },
        ]
    );
    let journal = fixture.journal();
    assert_eq!(journal.disposition, InstallDisposition::Committed);
    assert_eq!(journal.revision, 14);
    assert_eq!(journal.platform_record, FakePlatform::transaction_record());
    assert_eq!(
        journal.candidate_owner_receipt,
        Some(FakePlatform::owner_receipt())
    );
    assert_eq!(
        fs::metadata(fixture.store.journal_path())
            .expect("journal metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fixture.assert_sentinels();
}

#[test]
fn requested_target_policy_controls_first_install_without_changing_rollback_state() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: None,
        launcher_unit: None,
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };

    for (policy, loaded, running, autostart_enabled) in [
        (InstallTargetPolicy::EnabledAndRunning, true, true, true),
        (InstallTargetPolicy::Disabled, false, false, false),
    ] {
        let journal = new_journal(
            InstallTransactionId::new(format!("target-{policy:?}"))
                .expect("valid target transaction ID"),
            None,
            candidate.clone(),
            prior.clone(),
            policy,
        );

        assert_eq!(journal.prior_platform, prior);
        assert_eq!(journal.target_platform.loaded, loaded);
        assert_eq!(journal.target_platform.running_unit.is_some(), running);
        assert_eq!(journal.target_platform.autostart_enabled, autostart_enabled);
    }
}

#[test]
fn persisted_target_policy_rejects_every_corrupt_target_field() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: Some(UnitId::new(PRIOR_ID).expect("prior layout unit ID")),
        launcher_unit: Some(UnitId::new(PRIOR_ID).expect("prior unit ID")),
        loaded: true,
        running_unit: Some(UnitId::new(PRIOR_ID).expect("prior unit ID")),
        autostart_enabled: true,
    };

    for policy in [
        InstallTargetPolicy::Preserve,
        InstallTargetPolicy::EnableOnFirstInstall,
        InstallTargetPolicy::EnabledAndRunning,
        InstallTargetPolicy::Disabled,
    ] {
        let journal = new_journal(
            InstallTransactionId::new(format!("corrupt-{policy:?}"))
                .expect("valid corruption transaction ID"),
            Some(UnitId::new(PRIOR_ID).expect("prior active unit")),
            candidate.clone(),
            prior.clone(),
            policy,
        );
        let mut corruptions = Vec::new();

        let mut corrupted = journal.clone();
        corrupted.target_platform.launcher_unit = None;
        corruptions.push(corrupted);

        let mut corrupted = journal.clone();
        corrupted.target_platform.loaded = !corrupted.target_platform.loaded;
        corruptions.push(corrupted);

        let mut corrupted = journal.clone();
        corrupted.target_platform.running_unit = if corrupted.target_platform.running_unit.is_some()
        {
            None
        } else {
            Some(candidate.clone())
        };
        corruptions.push(corrupted);

        let mut corrupted = journal;
        corrupted.target_platform.autostart_enabled = !corrupted.target_platform.autostart_enabled;
        corruptions.push(corrupted);

        for corrupted in corruptions {
            let decoded: InstallJournalV1 = serde_json::from_slice(
                &serde_json::to_vec(&corrupted).expect("encode corrupt persisted journal"),
            )
            .expect("decode structurally valid corrupt journal");
            assert_eq!(
                decoded.validate(),
                Err(InstallModelError::InvalidTargetState)
            );
        }
    }
}

#[test]
fn persisted_transition_plan_rejects_corrupt_intermediate_states() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: Some(UnitId::new(PRIOR_ID).expect("prior layout unit ID")),
        launcher_unit: Some(UnitId::new(PRIOR_ID).expect("prior launcher unit ID")),
        loaded: true,
        running_unit: Some(UnitId::new(PRIOR_ID).expect("prior running unit ID")),
        autostart_enabled: true,
    };
    let journal = new_journal(
        InstallTransactionId::new("corrupt-transitions").expect("transaction ID"),
        prior.layout_unit.clone(),
        candidate.clone(),
        prior,
        InstallTargetPolicy::Preserve,
    );
    let mut corruptions = Vec::new();

    let mut corrupted = journal.clone();
    corrupted.transition_states.prior_unloaded.layout_unit = None;
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.transition_states.candidate_manager.running_unit = Some(candidate.clone());
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.transition_states.candidate_autostart.loaded = false;
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.transition_states.prior_manager.running_unit = Some(candidate);
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.transition_states.prior_manager.autostart_enabled = false;
    corruptions.push(corrupted);

    let mut corrupted = journal;
    corrupted
        .transition_states
        .prior_autostart
        .autostart_enabled = false;
    corruptions.push(corrupted);

    for corrupted in corruptions {
        assert_eq!(
            corrupted.validate(),
            Err(InstallModelError::InvalidTransitionStates)
        );
    }
}

#[test]
fn rollback_manager_reload_preserves_prior_autostart() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: None,
        launcher_unit: None,
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };
    let journal = new_journal(
        InstallTransactionId::new("manager-autostart-boundary").expect("transaction ID"),
        None,
        candidate,
        prior,
        InstallTargetPolicy::EnableOnFirstInstall,
    );

    assert!(journal.target_platform.autostart_enabled);
    assert!(!journal.transition_states.prior_manager.autostart_enabled);
    assert!(!journal.transition_states.prior_autostart.autostart_enabled);
}

#[test]
fn persisted_layout_cursor_must_match_the_named_journal_action() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: Some(UnitId::new(PRIOR_ID).expect("prior layout unit ID")),
        launcher_unit: Some(UnitId::new(PRIOR_ID).expect("prior launcher unit ID")),
        loaded: true,
        running_unit: Some(UnitId::new(PRIOR_ID).expect("prior running unit ID")),
        autostart_enabled: true,
    };
    let journal = new_journal(
        InstallTransactionId::new("corrupt-layout-cursor").expect("transaction ID"),
        prior.layout_unit.clone(),
        candidate,
        prior,
        InstallTargetPolicy::Preserve,
    );
    let mut corruptions = Vec::new();

    let mut corrupted = journal.clone();
    corrupted.layout_operation_index = 1;
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.next_action = Some(InstallAction::InstallCandidateLayout);
    corrupted.layout_operation_index = corrupted.layout_operation_count;
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.next_action = Some(InstallAction::InstallCandidateLauncher);
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.disposition = InstallDisposition::Rollback;
    corrupted.next_action = Some(InstallAction::RestorePriorLayout);
    corrupted.failure = Some("forward failure".to_owned());
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.disposition = InstallDisposition::Rollback;
    corrupted.next_action = Some(InstallAction::UnloadCandidateRuntime);
    corrupted.failure = Some("forward failure".to_owned());
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.disposition = InstallDisposition::Rollback;
    corrupted.next_action = Some(InstallAction::ReloadPriorManager);
    corrupted.layout_operation_index = 1;
    corrupted.failure = Some("forward failure".to_owned());
    corruptions.push(corrupted);

    let mut corrupted = journal.clone();
    corrupted.disposition = InstallDisposition::Committed;
    corrupted.next_action = None;
    corruptions.push(corrupted);

    let mut corrupted = journal;
    corrupted.disposition = InstallDisposition::RolledBack;
    corrupted.next_action = None;
    corrupted.layout_operation_index = corrupted.layout_operation_count;
    corrupted.failure = Some("forward failure".to_owned());
    corruptions.push(corrupted);

    for corrupted in corruptions {
        assert_eq!(
            corrupted.validate(),
            Err(InstallModelError::InvalidLayoutOperationCursor)
        );
    }
}

#[test]
fn persisted_transition_plan_allows_platform_specific_loaded_quiescence() {
    let candidate = UnitId::new(CANDIDATE_ID).expect("candidate unit ID");
    let prior = PlatformState {
        layout_unit: Some(UnitId::new(PRIOR_ID).expect("prior layout unit ID")),
        launcher_unit: Some(UnitId::new(PRIOR_ID).expect("prior launcher unit ID")),
        loaded: true,
        running_unit: Some(UnitId::new(PRIOR_ID).expect("prior running unit ID")),
        autostart_enabled: true,
    };
    let mut journal = new_journal(
        InstallTransactionId::new("loaded-quiescence").expect("transaction ID"),
        prior.layout_unit.clone(),
        candidate,
        prior,
        InstallTargetPolicy::Preserve,
    );
    journal.transition_states.prior_unloaded.loaded = true;

    assert_eq!(journal.validate(), Ok(()));
}

#[test]
fn coordinator_applies_explicit_first_install_service_policy() {
    for (policy, reloads) in [
        (InstallTargetPolicy::Preserve, 0),
        (InstallTargetPolicy::EnableOnFirstInstall, 1),
        (InstallTargetPolicy::EnabledAndRunning, 1),
        (InstallTargetPolicy::Disabled, 0),
    ] {
        let directory = tempfile::tempdir().expect("first install fixture");
        let store = InstallStore::new(directory.path().join("install"), 64 * 1024);
        let candidate = prepare_unit(&store, CANDIDATE_ID);
        let expected = match policy {
            InstallTargetPolicy::Preserve => PlatformState {
                layout_unit: Some(candidate.id().clone()),
                launcher_unit: None,
                loaded: false,
                running_unit: None,
                autostart_enabled: false,
            },
            InstallTargetPolicy::EnableOnFirstInstall | InstallTargetPolicy::EnabledAndRunning => {
                PlatformState {
                    layout_unit: Some(candidate.id().clone()),
                    launcher_unit: Some(candidate.id().clone()),
                    loaded: true,
                    running_unit: Some(candidate.id().clone()),
                    autostart_enabled: true,
                }
            }
            InstallTargetPolicy::Disabled => PlatformState {
                layout_unit: Some(candidate.id().clone()),
                launcher_unit: Some(candidate.id().clone()),
                loaded: false,
                running_unit: None,
                autostart_enabled: false,
            },
        };
        let prior = PlatformState {
            layout_unit: None,
            launcher_unit: None,
            loaded: false,
            running_unit: None,
            autostart_enabled: false,
        };
        let mut platform = FakePlatform::new(prior, &store);
        let outcome = InstallCoordinator::new(&store, &mut platform)
            .install(InstallRequest {
                transaction_id: InstallTransactionId::new(format!("first-{policy:?}"))
                    .expect("first install transaction ID"),
                candidate: candidate.clone(),
                target_policy: policy,
            })
            .expect("first install transaction");

        assert_eq!(
            outcome,
            InstallOutcome::Committed {
                active_unit: candidate.id().clone()
            }
        );
        assert_eq!(platform.state, expected);
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == InstallAction::RestoreCandidateRuntime)
                .count(),
            reloads
        );
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == InstallAction::ProveCandidate)
                .count(),
            1
        );
    }
}

#[test]
fn enable_on_first_install_preserves_an_existing_disabled_launcher() {
    let directory = tempfile::tempdir().expect("upgrade fixture");
    let store = InstallStore::new(directory.path().join("install"), 64 * 1024);
    let prior = prepare_unit(&store, PRIOR_ID);
    let candidate = prepare_unit(&store, CANDIDATE_ID);
    let prior_state = PlatformState {
        layout_unit: Some(prior.id().clone()),
        launcher_unit: Some(prior.id().clone()),
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };
    let lock = store.acquire_lock().expect("upgrade active lock");
    store
        .set_active(Some(prior.id()), &lock)
        .expect("upgrade prior active unit");
    drop(lock);
    let mut platform = FakePlatform::new(prior_state, &store);

    let outcome = InstallCoordinator::new(&store, &mut platform)
        .install(InstallRequest {
            transaction_id: InstallTransactionId::new("upgrade-disabled")
                .expect("upgrade transaction ID"),
            candidate: candidate.clone(),
            target_policy: InstallTargetPolicy::EnableOnFirstInstall,
        })
        .expect("upgrade transaction");

    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: candidate.id().clone()
        }
    );
    assert_eq!(
        platform.state,
        PlatformState {
            layout_unit: Some(candidate.id().clone()),
            launcher_unit: Some(candidate.id().clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: false,
        }
    );
    assert!(!platform.effects.iter().any(|effect| matches!(
        effect.action,
        InstallAction::RestoreCandidateAutostart | InstallAction::RestoreCandidateRuntime
    )));
}

#[test]
fn state_neutral_manager_reload_is_still_invoked() {
    let fixture = Fixture::new();
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    let request = InstallRequest {
        transaction_id: InstallTransactionId::new("state-neutral-manager").expect("transaction ID"),
        candidate: fixture.candidate.clone(),
        target_policy: InstallTargetPolicy::Disabled,
    };

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install(request)
        .expect("state-neutral manager transaction");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::ReloadCandidateManager)
            .count(),
        1
    );
}

#[test]
fn state_neutral_manager_reload_replays_after_crash() {
    let fixture = Fixture::new();
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    platform.inject(
        InstallAction::ReloadCandidateManager,
        InjectionKind::PanicAfter,
    );
    let request = InstallRequest {
        transaction_id: InstallTransactionId::new("state-neutral-manager-crash")
            .expect("transaction ID"),
        candidate: fixture.candidate.clone(),
        target_policy: InstallTargetPolicy::Disabled,
    };

    let crashed = catch_unwind(AssertUnwindSafe(|| {
        drop(InstallCoordinator::new(&fixture.store, &mut platform).install(request));
    }));
    assert!(crashed.is_err());

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover state-neutral manager command")
        .expect("recovered outcome");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::ReloadCandidateManager)
            .count(),
        2
    );
}

#[test]
fn state_neutral_rollback_manager_commands_replay_after_crash() {
    for action in [
        InstallAction::UnloadCandidateManager,
        InstallAction::ReloadPriorManager,
    ] {
        let fixture = Fixture::new();
        let mut platform = seed_state_neutral_rollback_manager(&fixture, action);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).recover());
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");

        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover state-neutral rollback manager command")
            .expect("rollback outcome");

        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == action)
                .count(),
            2,
            "{action:?} must replay"
        );
    }
}

#[test]
fn idempotent_manager_command_rejects_exact_third_state_without_effects() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let target = target_state(
        &prior,
        fixture.candidate.id(),
        InstallTargetPolicy::Disabled,
    );
    let candidate_active = FakePlatform::candidate_active(&prior, &target);
    let mut journal = new_journal(
        InstallTransactionId::new("manager-third-state").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        prior,
        InstallTargetPolicy::Disabled,
    );
    journal.revision = 8;
    journal.next_action = Some(InstallAction::ReloadCandidateManager);
    journal.layout_operation_index = journal.layout_operation_count;
    let lock = fixture.store.acquire_lock().expect("seed manager lock");
    fixture
        .store
        .set_active(Some(fixture.candidate.id()), &lock)
        .expect("seed candidate active unit");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed manager journal");
    drop(lock);
    let mut platform = FakePlatform::new(candidate_active, &fixture.store);
    platform.layout_operation_progress = journal.layout_operation_count;
    platform.candidate_launcher_installed = true;
    platform.exact_state_valid = false;

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("exact third state must fail closed");

    assert!(matches!(
        error,
        InstallCoordinatorError::StateDrift {
            action: InstallAction::ReloadCandidateManager,
            ..
        }
    ));
    assert!(platform.effects.is_empty());
    assert_eq!(
        fixture.journal().next_action,
        Some(InstallAction::ReloadCandidateManager)
    );
}

#[test]
fn every_platform_forward_failure_rolls_back_exact_prior_state() {
    for action in [
        InstallAction::PreflightCandidate,
        InstallAction::UnloadPrior,
        InstallAction::ProvePriorGuardReleased,
        InstallAction::InstallCandidateLayout,
        InstallAction::InstallCandidateLauncher,
        InstallAction::ReloadCandidateManager,
        InstallAction::RestoreCandidateRuntime,
        InstallAction::ProveCandidate,
    ] {
        let fixture = Fixture::new();
        let prior = fixture.prior_state();
        let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
        platform.inject(action, InjectionKind::Fail);

        let outcome = install(&fixture, &mut platform);

        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(platform.state, prior, "failed at {action:?}");
        assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
        assert_eq!(
            fixture.journal().disposition,
            InstallDisposition::RolledBack
        );
        if matches!(
            action,
            InstallAction::PreflightCandidate | InstallAction::UnloadPrior
        ) {
            assert!(!platform.effects.iter().any(|effect| matches!(
                effect.action,
                InstallAction::RestorePriorRuntime | InstallAction::ProvePrior
            )));
        }
        fixture.assert_sentinels();
    }
}

#[test]
fn candidate_autostart_failure_and_crash_replay_under_disabled_policy() {
    for injection in [InjectionKind::Fail, InjectionKind::PanicAfter] {
        let fixture = Fixture::new();
        let prior = fixture.prior_state();
        let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
        platform.inject(InstallAction::RestoreCandidateAutostart, injection);
        let request = InstallRequest {
            transaction_id: InstallTransactionId::new(format!("candidate-auto-{injection:?}"))
                .expect("transaction ID"),
            candidate: fixture.candidate.clone(),
            target_policy: InstallTargetPolicy::Disabled,
        };

        if injection == InjectionKind::Fail {
            let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
                .install(request)
                .expect("failed autostart mutation rolls back");
            assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
            assert_eq!(platform.state, prior);
        } else {
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                drop(InstallCoordinator::new(&fixture.store, &mut platform).install(request));
            }));
            assert!(crashed.is_err(), "autostart mutation must inject a crash");
            let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
                .recover()
                .expect("recover autostart mutation")
                .expect("recovered outcome");
            assert!(matches!(outcome, InstallOutcome::Committed { .. }));
            assert_eq!(
                platform
                    .effects
                    .iter()
                    .filter(|effect| { effect.action == InstallAction::RestoreCandidateAutostart })
                    .count(),
                1
            );
        }
    }
}

#[test]
fn active_switch_failure_rolls_back_without_touching_other_paths() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.deny_active_switch = Some(InstallAction::SwitchToCandidate);

    let outcome = install(&fixture, &mut platform);

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
    fixture.assert_sentinels();
}

#[test]
fn crash_after_platform_effect_is_reconciled_without_repeating_mutation() {
    for action in [
        InstallAction::UnloadPrior,
        InstallAction::InstallCandidateLayout,
        InstallAction::InstallCandidateLauncher,
        InstallAction::ReloadCandidateManager,
        InstallAction::RestoreCandidateRuntime,
    ] {
        let fixture = Fixture::new();
        let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).install(fixture.request()));
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");
        assert_eq!(fixture.journal().next_action, Some(action));
        if action == InstallAction::RestoreCandidateRuntime {
            assert_eq!(fixture.journal().candidate_owner_receipt, None);
        }

        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover transaction")
            .expect("recovered outcome");

        assert!(matches!(outcome, InstallOutcome::Committed { .. }));
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == action)
                .count(),
            if action == InstallAction::InstallCandidateLayout {
                3
            } else if action == InstallAction::ReloadCandidateManager {
                2
            } else {
                1
            },
            "{action:?} replay count"
        );
        if action == InstallAction::RestoreCandidateRuntime {
            assert_eq!(
                fixture.journal().candidate_owner_receipt,
                Some(FakePlatform::owner_receipt())
            );
        }
    }
}

#[test]
fn runtime_error_after_start_persists_owner_receipt_before_candidate_stop() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.inject(
        InstallAction::RestoreCandidateRuntime,
        InjectionKind::FailAfter,
    );

    let outcome = install(&fixture, &mut platform);

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(
        fixture.journal().candidate_owner_receipt,
        Some(FakePlatform::owner_receipt())
    );
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::RestoreCandidateRuntime)
            .count(),
        1
    );
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::UnloadCandidateRuntime)
            .count(),
        1
    );
}

#[test]
fn layout_error_after_mutation_advances_the_cursor_before_rollback() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.inject(
        InstallAction::InstallCandidateLayout,
        InjectionKind::FailAfter,
    );

    let outcome = install(&fixture, &mut platform);

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(platform.layout_operation_progress, 0);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::InstallCandidateLayout)
            .count(),
        1
    );
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::RestorePriorLayout)
            .count(),
        1
    );
    let journal = fixture.journal();
    assert_eq!(journal.disposition, InstallDisposition::RolledBack);
    assert_eq!(journal.layout_operation_index, 0);
}

#[test]
fn first_conversion_reconciles_platform_specific_layout_progress() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let lock = fixture.store.acquire_lock().expect("clear active lock");
    fixture
        .store
        .set_active(None, &lock)
        .expect("clear active unit");
    drop(lock);
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.inject(
        InstallAction::InstallCandidateLayout,
        InjectionKind::FailAfter,
    );

    let outcome = install(&fixture, &mut platform);

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(fixture.active_unit(), None);
    assert_eq!(platform.state, prior);
    assert_eq!(platform.layout_operation_progress, 0);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::RestorePriorLayout)
            .count(),
        1
    );
}

#[test]
fn layout_error_with_third_state_drift_does_not_advance_the_journal() {
    let fixture = Fixture::new();
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    platform.inject(
        InstallAction::InstallCandidateLayout,
        InjectionKind::DriftAfter,
    );

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .install(fixture.request())
        .expect_err("third-state layout drift must fail closed");

    assert!(matches!(
        error,
        InstallCoordinatorError::StateDrift {
            action: InstallAction::InstallCandidateLayout,
            ..
        }
    ));
    let journal = fixture.journal();
    assert_eq!(journal.disposition, InstallDisposition::Forward);
    assert_eq!(
        journal.next_action,
        Some(InstallAction::InstallCandidateLayout)
    );
    assert_eq!(journal.layout_operation_index, 0);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::InstallCandidateLayout)
            .count(),
        1
    );
    assert!(
        !platform
            .effects
            .iter()
            .any(|effect| effect.action == InstallAction::RestorePriorLayout)
    );
}

#[test]
fn manager_error_after_mutation_rolls_back_from_the_observed_state() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.inject(
        InstallAction::ReloadCandidateManager,
        InjectionKind::FailAfter,
    );

    let outcome = install(&fixture, &mut platform);

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::ReloadCandidateManager)
            .count(),
        1
    );
}

#[test]
fn crash_after_active_switch_advances_from_observed_after_state() {
    let fixture = Fixture::new();
    let mut journal = new_journal(
        InstallTransactionId::new("switch-crash").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    journal.revision = 3;
    journal.next_action = Some(InstallAction::SwitchToCandidate);
    journal.layout_operation_index = 3;
    let mut platform = FakePlatform::new(
        PlatformState {
            layout_unit: Some(fixture.candidate.id().clone()),
            launcher_unit: Some(fixture.candidate.id().clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: true,
        },
        &fixture.store,
    );
    platform.layout_operation_progress = 3;
    platform.candidate_launcher_installed = true;
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    fixture
        .store
        .set_active(Some(fixture.candidate.id()), &lock)
        .expect("effect happened before crash");
    drop(lock);

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover switched state")
        .expect("recovered outcome");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    assert_eq!(
        platform
            .effects
            .iter()
            .map(|effect| effect.action)
            .collect::<Vec<_>>(),
        [
            InstallAction::ReloadCandidateManager,
            InstallAction::RestoreCandidateRuntime,
            InstallAction::ProveCandidate
        ]
    );
}

#[test]
fn proof_crash_repeats_only_the_idempotent_proof() {
    let fixture = Fixture::new();
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    platform.inject(InstallAction::ProveCandidate, InjectionKind::PanicAfter);

    let crashed = catch_unwind(AssertUnwindSafe(|| {
        drop(InstallCoordinator::new(&fixture.store, &mut platform).install(fixture.request()));
    }));
    assert!(crashed.is_err());
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover proof")
        .expect("recovered outcome");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::ProveCandidate)
            .count(),
        2
    );
}

#[test]
fn crash_after_forward_checks_repeats_only_the_idempotent_check() {
    for action in [
        InstallAction::PreflightCandidate,
        InstallAction::ProvePriorGuardReleased,
        InstallAction::ProveCandidate,
    ] {
        let fixture = Fixture::new();
        let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).install(fixture.request()));
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");
        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover check")
            .expect("recovered outcome");

        assert!(matches!(outcome, InstallOutcome::Committed { .. }));
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == action)
                .count(),
            2,
            "{action:?} must be safely repeated"
        );
    }
}

#[test]
fn rollback_effect_failure_replays_from_write_ahead_action() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
    platform.inject(InstallAction::ProveCandidate, InjectionKind::Fail);
    platform.inject(InstallAction::RestorePriorLauncher, InjectionKind::Fail);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .install(fixture.request())
        .expect_err("rollback effect must fail once");
    assert!(matches!(
        error,
        InstallCoordinatorError::Platform {
            action: InstallAction::RestorePriorLauncher,
            ..
        }
    ));
    assert_eq!(
        fixture.journal().next_action,
        Some(InstallAction::RestorePriorLauncher)
    );

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover rollback")
        .expect("rollback outcome");

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
}

#[test]
fn every_platform_rollback_failure_replays_from_exact_before_state() {
    for action in [
        InstallAction::UnloadCandidateRuntime,
        InstallAction::UnloadCandidateManager,
        InstallAction::ProveCandidateGuardReleased,
        InstallAction::RestorePriorLauncher,
        InstallAction::RestorePriorLayout,
        InstallAction::ReloadPriorManager,
        InstallAction::RestorePriorRuntime,
        InstallAction::ProvePrior,
    ] {
        let fixture = Fixture::new();
        let prior = fixture.prior_state();
        let mut platform = seed_rollback(&fixture, action);
        platform.inject(action, InjectionKind::Fail);

        let error = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect_err("rollback effect must fail once");
        assert!(matches!(
            error,
            InstallCoordinatorError::Platform {
                action: failed_action,
                ..
            } if failed_action == action
        ));
        assert_eq!(fixture.journal().next_action, Some(action));

        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover rollback effect")
            .expect("rollback outcome");
        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(platform.state, prior);
        assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
    }
}

#[test]
fn rollback_candidate_autostart_failure_and_crash_replay_under_disabled_policy() {
    let action = InstallAction::UnloadCandidateAutostart;
    for injection in [InjectionKind::Fail, InjectionKind::PanicAfter] {
        let fixture = Fixture::new();
        let prior = fixture.prior_state();
        let mut platform =
            seed_rollback_for_policy(&fixture, action, InstallTargetPolicy::Disabled);
        platform.inject(action, injection);

        if injection == InjectionKind::Fail {
            let error = InstallCoordinator::new(&fixture.store, &mut platform)
                .recover()
                .expect_err("rollback autostart action must fail once");
            assert!(matches!(
                error,
                InstallCoordinatorError::Platform {
                    action: failed_action,
                    ..
                } if failed_action == action
            ));
            let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
                .recover()
                .expect("recover rollback autostart failure")
                .expect("rollback outcome");
            assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        } else {
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                drop(InstallCoordinator::new(&fixture.store, &mut platform).recover());
            }));
            assert!(crashed.is_err(), "{action:?} must inject a crash");
            let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
                .recover()
                .expect("recover rollback autostart crash")
                .expect("rollback outcome");
            assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
            assert_eq!(
                platform
                    .effects
                    .iter()
                    .filter(|effect| effect.action == action)
                    .count(),
                1,
                "{action:?} must not repeat"
            );
        }
        assert_eq!(platform.state, prior);
    }
}

#[test]
fn rollback_prior_autostart_skips_a_state_neutral_platform_effect() {
    let action = InstallAction::RestorePriorAutostart;
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = seed_rollback_for_policy(&fixture, action, InstallTargetPolicy::Disabled);
    platform.inject(action, InjectionKind::Fail);

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover state-neutral prior autostart")
        .expect("rollback outcome");

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert!(
        platform
            .effects
            .iter()
            .all(|effect| effect.action != action)
    );
}

#[test]
fn reverse_layout_error_after_mutation_replays_without_repeating_the_operation() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = seed_rollback(&fixture, InstallAction::RestorePriorLayout);
    platform.inject(InstallAction::RestorePriorLayout, InjectionKind::FailAfter);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("reverse layout operation must report its injected error");
    assert!(matches!(
        error,
        InstallCoordinatorError::Platform {
            action: InstallAction::RestorePriorLayout,
            ..
        }
    ));
    assert_eq!(fixture.journal().layout_operation_index, 3);
    assert_eq!(platform.layout_operation_progress, 2);

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover reverse layout mutation")
        .expect("rollback outcome");
    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::RestorePriorLayout)
            .count(),
        3
    );
}

#[test]
fn rollback_runtime_error_after_mutation_advances_without_repeating_it() {
    let fixture = Fixture::new();
    let prior = fixture.prior_state();
    let mut platform = seed_rollback(&fixture, InstallAction::RestorePriorRuntime);
    platform.inject(InstallAction::RestorePriorRuntime, InjectionKind::FailAfter);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("rollback runtime operation must report its injected error");
    assert!(matches!(
        error,
        InstallCoordinatorError::Platform {
            action: InstallAction::RestorePriorRuntime,
            ..
        }
    ));

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover rollback runtime mutation")
        .expect("rollback outcome");
    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(platform.state, prior);
    assert_eq!(
        platform
            .effects
            .iter()
            .filter(|effect| effect.action == InstallAction::RestorePriorRuntime)
            .count(),
        1
    );
}

#[test]
fn rollback_active_switch_failure_replays_after_directory_is_restored() {
    let fixture = Fixture::new();
    let mut platform = seed_rollback(&fixture, InstallAction::RestorePriorActive);
    platform.deny_active_switch = Some(InstallAction::RestorePriorActive);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("prior active switch must fail");
    assert!(matches!(
        error,
        InstallCoordinatorError::Store(InstallStoreError::SwitchActive(_))
    ));
    assert_eq!(
        fixture.journal().next_action,
        Some(InstallAction::RestorePriorActive)
    );

    platform.restore_install_permissions();
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover prior active switch")
        .expect("rollback outcome");
    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
}

#[test]
fn crash_after_rollback_mutation_replays_by_action_policy() {
    for action in [
        InstallAction::UnloadCandidateRuntime,
        InstallAction::UnloadCandidateManager,
        InstallAction::RestorePriorLauncher,
        InstallAction::RestorePriorLayout,
        InstallAction::ReloadPriorManager,
        InstallAction::RestorePriorRuntime,
    ] {
        let fixture = Fixture::new();
        let mut platform = seed_rollback(&fixture, action);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).recover());
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");
        assert_eq!(fixture.journal().next_action, Some(action));

        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover rollback mutation")
            .expect("rollback outcome");
        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == action)
                .count(),
            if action == InstallAction::RestorePriorLayout {
                3
            } else if matches!(
                action,
                InstallAction::UnloadCandidateManager | InstallAction::ReloadPriorManager
            ) {
                2
            } else {
                1
            },
            "{action:?} replay count"
        );
    }
}

#[test]
fn crash_after_rollback_active_switch_advances_from_observed_state() {
    let fixture = Fixture::new();
    let mut platform = seed_rollback(&fixture, InstallAction::RestorePriorActive);
    let lock = fixture.store.acquire_lock().expect("crash simulation lock");
    fixture
        .store
        .set_active(Some(fixture.prior.id()), &lock)
        .expect("restore active effect before crash");
    drop(lock);

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover active rollback switch")
        .expect("rollback outcome");

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id().clone()));
}

#[test]
fn crash_after_rollback_checks_repeats_only_the_idempotent_check() {
    for action in [
        InstallAction::ProveCandidateGuardReleased,
        InstallAction::ProvePrior,
    ] {
        let fixture = Fixture::new();
        let mut platform = seed_rollback(&fixture, action);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).recover());
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");
        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .recover()
            .expect("recover rollback check")
            .expect("rollback outcome");

        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(
            platform
                .effects
                .iter()
                .filter(|effect| effect.action == action)
                .count(),
            2,
            "{action:?} must be safely repeated"
        );
    }
}

#[test]
fn lock_contention_prevents_transaction_start() {
    let fixture = Fixture::new();
    let _held = fixture.store.acquire_lock().expect("held lock");
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .install(fixture.request())
        .expect_err("contended transaction must fail");

    assert!(matches!(
        error,
        InstallCoordinatorError::Store(InstallStoreError::LockContended)
    ));
    assert!(platform.effects.is_empty());
}

/// Public layout authority walks every ancestor and rejects non-permission
/// mode bits, so the fixture cannot live under a sticky system temp root
/// such as /tmp on Linux hosts; anchor it inside the crate directory.
fn public_tree_fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("install-public-tree-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("public tree fixture")
}

#[test]
fn install_lock_lends_its_authority_to_public_layout_mutations() {
    let directory = public_tree_fixture();
    let store = InstallStore::new(directory.path().join("install"), 64 * 1024);
    let public = directory.path().join("public");
    fs::create_dir(&public).expect("public directory");
    let public = fs::canonicalize(public).expect("canonical public directory");
    let lock = store.acquire_lock().expect("install lock");

    let authority = lock
        .open_public_directory(&public)
        .expect("public directory authority");

    authority
        .validate_ancestry()
        .expect("public ancestry remains exact");
    drop(lock);
    assert!(matches!(
        store.acquire_lock(),
        Err(InstallStoreError::LockContended)
    ));
    drop(authority);
    drop(
        store
            .acquire_lock()
            .expect("authority releases install lock"),
    );
}

#[test]
fn coordinator_uses_the_same_caller_held_lock_as_public_layout() {
    let fixture = Fixture::new();
    let public_tree = public_tree_fixture();
    let public = public_tree.path().join("public");
    fs::create_dir(&public).expect("public directory");
    let public = fs::canonicalize(public).expect("canonical public directory");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let authority = lock
        .open_public_directory(&public)
        .expect("public directory authority");
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    let mut coordinator = InstallCoordinator::new(&fixture.store, &mut platform);

    assert_eq!(
        coordinator
            .recover_with_lock(&mut lock)
            .expect("recover with caller lock"),
        None
    );
    let outcome = coordinator
        .install_with_lock(fixture.request(), &mut lock)
        .expect("install with caller lock");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    authority
        .validate_ancestry()
        .expect("public authority remains exact");
}

#[test]
fn coordinator_rejects_a_caller_lock_from_another_store_before_platform_effects() {
    let fixture = Fixture::new();
    let foreign_store = InstallStore::new(fixture.directory.path().join("foreign"), 64 * 1024);
    let mut foreign_lock = foreign_store.acquire_lock().expect("foreign install lock");
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
    let mut coordinator = InstallCoordinator::new(&fixture.store, &mut platform);

    let recovery_error = coordinator
        .recover_with_lock(&mut foreign_lock)
        .expect_err("foreign recovery lock must fail");
    assert!(matches!(
        recovery_error,
        InstallCoordinatorError::Store(InstallStoreError::WrongLock)
    ));
    let install_error = coordinator
        .install_with_lock(fixture.request(), &mut foreign_lock)
        .expect_err("foreign install lock must fail");

    assert!(matches!(
        install_error,
        InstallCoordinatorError::Store(InstallStoreError::WrongLock)
    ));
    assert!(platform.effects.is_empty());
}

#[test]
fn corrupt_and_oversized_journals_fail_before_platform_effects() {
    for contents in [b"not json".to_vec(), vec![b'x'; 1_025]] {
        let fixture = Fixture::new();
        let store = InstallStore::new(fixture.store.root(), 1_024);
        fs::write(store.journal_path(), contents).expect("write malformed journal");
        let mut platform = FakePlatform::new(fixture.prior_state(), &store);

        let error = InstallCoordinator::new(&store, &mut platform)
            .recover()
            .expect_err("invalid journal must fail");

        assert!(matches!(
            error,
            InstallCoordinatorError::Store(
                InstallStoreError::DecodeJournal(_) | InstallStoreError::JournalTooLarge { .. }
            )
        ));
        assert!(platform.effects.is_empty());
    }
}

#[test]
fn platform_records_are_tagged_bounded_and_round_trip_unchanged() {
    let linux = PlatformTransactionRecord::linux(7, b"linux exact snapshot".to_vec())
        .expect("linux record");
    let macos = PlatformTransactionRecord::macos(9, b"macos exact snapshot".to_vec())
        .expect("macOS record");
    for record in [linux, macos] {
        let encoded = serde_json::to_vec(&record).expect("encode platform record");
        let decoded: PlatformTransactionRecord =
            serde_json::from_slice(&encoded).expect("decode platform record");
        assert_eq!(decoded, record);
        assert_eq!(decoded.payload(), record.payload());
    }

    assert!(matches!(
        PlatformTransactionRecord::linux(1, vec![0; MAX_PLATFORM_TRANSACTION_RECORD_BYTES + 1]),
        Err(InstallModelError::PlatformRecordTooLarge { .. })
    ));
    assert!(matches!(
        PlatformTransactionRecord::macos(0, vec![1]),
        Err(InstallModelError::ZeroPlatformRecordSchema)
    ));

    let receipt = PlatformOwnerReceipt::linux(3, b"systemd invocation".to_vec())
        .expect("Linux owner receipt");
    let encoded = serde_json::to_vec(&receipt).expect("encode owner receipt");
    let decoded: PlatformOwnerReceipt =
        serde_json::from_slice(&encoded).expect("decode owner receipt");
    assert_eq!(decoded, receipt);
    assert!(matches!(
        PlatformOwnerReceipt::macos(1, vec![0; MAX_PLATFORM_OWNER_RECEIPT_BYTES + 1]),
        Err(InstallModelError::OwnerReceiptTooLarge { .. })
    ));
    assert!(matches!(
        PlatformOwnerReceipt::linux(0, vec![1]),
        Err(InstallModelError::ZeroOwnerReceiptSchema)
    ));
    assert!(matches!(
        PlatformOwnerReceipt::macos(1, Vec::new()),
        Err(InstallModelError::EmptyOwnerReceipt)
    ));
}

#[test]
fn journal_rejects_owner_receipts_with_impossible_platform_or_runtime_state() {
    let fixture = Fixture::new();
    let mut wrong_platform = new_journal(
        InstallTransactionId::new("wrong-receipt-platform").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    wrong_platform.candidate_owner_receipt =
        Some(PlatformOwnerReceipt::macos(1, b"macOS owner".to_vec()).expect("macOS receipt"));
    assert_eq!(
        wrong_platform.validate(),
        Err(InstallModelError::InvalidOwnerReceipt)
    );

    let mut inactive = new_journal(
        InstallTransactionId::new("inactive-receipt").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Disabled,
    );
    inactive.candidate_owner_receipt = Some(FakePlatform::owner_receipt());
    assert_eq!(
        inactive.validate(),
        Err(InstallModelError::InvalidOwnerReceipt)
    );

    let mut early = new_journal(
        InstallTransactionId::new("early-receipt").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    early.candidate_owner_receipt = Some(FakePlatform::owner_receipt());
    assert_eq!(
        early.validate(),
        Err(InstallModelError::InvalidOwnerReceipt)
    );

    let mut missing = new_journal(
        InstallTransactionId::new("missing-required-receipt").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    missing.next_action = Some(InstallAction::ProveCandidate);
    missing.layout_operation_index = missing.layout_operation_count;
    assert_eq!(
        missing.validate(),
        Err(InstallModelError::InvalidOwnerReceipt)
    );
}

#[test]
fn oversized_embedded_platform_record_is_rejected_after_bounded_read() {
    let fixture = Fixture::new();
    let mut journal = new_journal(
        InstallTransactionId::new("oversized-record").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    journal.platform_record = PlatformTransactionRecord::Linux {
        schema_version: 1,
        payload: vec![0; MAX_PLATFORM_TRANSACTION_RECORD_BYTES + 1],
    };
    fs::write(
        fixture.store.journal_path(),
        serde_json::to_vec(&journal).expect("encode intentionally invalid journal"),
    )
    .expect("write intentionally invalid journal");
    let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("oversized platform record must fail");

    assert!(matches!(
        error,
        InstallCoordinatorError::Store(InstallStoreError::InvalidJournal(
            InstallModelError::PlatformRecordTooLarge { .. }
        ))
    ));
    assert!(platform.effects.is_empty());
}

#[test]
fn exact_platform_metadata_drift_fails_closed() {
    let fixture = Fixture::new();
    let mut journal = new_journal(
        InstallTransactionId::new("exact-drift").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    journal.revision = 6;
    journal.next_action = Some(InstallAction::RestoreCandidateRuntime);
    journal.layout_operation_index = 3;
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .set_active(Some(fixture.candidate.id()), &lock)
        .expect("seed candidate active unit");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    drop(lock);
    let mut platform = FakePlatform::new(
        PlatformState {
            layout_unit: Some(fixture.candidate.id().clone()),
            launcher_unit: Some(fixture.candidate.id().clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: true,
        },
        &fixture.store,
    );
    platform.layout_operation_progress = 3;
    platform.candidate_launcher_installed = true;
    platform.exact_state_valid = false;

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("exact metadata mismatch must fail closed");

    assert!(matches!(
        error,
        InstallCoordinatorError::StateDrift {
            action: InstallAction::RestoreCandidateRuntime,
            ..
        }
    ));
    assert!(platform.effects.is_empty());
    assert_eq!(fixture.journal(), journal);
}

#[test]
fn third_state_drift_fails_closed_without_advancing_journal() {
    let fixture = Fixture::new();
    let third = prepare_unit(&fixture.store, THIRD_ID);
    let mut journal = new_journal(
        InstallTransactionId::new("drift").expect("transaction ID"),
        Some(fixture.prior.id().clone()),
        fixture.candidate.id().clone(),
        fixture.prior_state(),
        InstallTargetPolicy::Preserve,
    );
    journal.revision = 4;
    journal.next_action = Some(InstallAction::RestoreCandidateRuntime);
    journal.layout_operation_index = 3;
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    fixture
        .store
        .set_active(Some(third.id()), &lock)
        .expect("seed third active state");
    drop(lock);
    let mut platform = FakePlatform::new(
        PlatformState {
            layout_unit: Some(fixture.candidate.id().clone()),
            launcher_unit: Some(fixture.candidate.id().clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: true,
        },
        &fixture.store,
    );

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("third state must fail closed");

    assert!(matches!(
        error,
        InstallCoordinatorError::StateDrift {
            action: InstallAction::RestoreCandidateRuntime,
            ..
        }
    ));
    assert!(platform.effects.is_empty());
    assert_eq!(fixture.journal(), journal);
    fixture.assert_sentinels();
}
