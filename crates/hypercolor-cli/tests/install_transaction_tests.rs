#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use hypercolor_cli::install::{
    InstallAction, InstallCoordinator, InstallCoordinatorError, InstallDisposition,
    InstallJournalV1, InstallModelError, InstallOutcome, InstallPlatform, InstallPlatformError,
    InstallRequest, InstallStore, InstallStoreError, InstallTransactionId,
    MAX_PLATFORM_TRANSACTION_RECORD_BYTES, PlatformCheckpoint, PlatformState,
    PlatformTransactionRecord, UnitId, UnitRecord,
};

const PRIOR_ID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CANDIDATE_ID: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const THIRD_ID: &str = "3333333333333333333333333333333333333333333333333333333333333333";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectionKind {
    Fail,
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
        }
    }

    fn inject(&mut self, action: InstallAction, kind: InjectionKind) {
        self.injections.push((action, kind));
    }

    fn begin_effect(&mut self) -> Result<(InstallAction, bool), InstallPlatformError> {
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
        Ok((action, injection == Some(InjectionKind::PanicAfter)))
    }

    fn finish_effect(action: InstallAction, panic_after: bool) {
        assert!(!panic_after, "injected crash after {action:?}");
    }

    fn transaction_record() -> PlatformTransactionRecord {
        PlatformTransactionRecord::linux(1, b"exact fake launcher and owner proof".to_vec())
            .expect("valid fake platform record")
    }

    fn assert_record(record: &PlatformTransactionRecord) {
        assert_eq!(record, &Self::transaction_record());
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
        Ok(self.state.clone())
    }

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &hypercolor_cli::install::InstallationState,
    ) -> Result<PlatformTransactionRecord, InstallPlatformError> {
        assert_eq!(candidate.id.as_str(), CANDIDATE_ID);
        assert!(candidate.root.is_dir());
        assert_eq!(prior.platform, self.state);
        Ok(Self::transaction_record())
    }

    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError> {
        Self::assert_record(record);
        let incarnation_matches = match checkpoint {
            PlatformCheckpoint::PriorOriginal => !self.prior_restored,
            PlatformCheckpoint::PriorRestored => self.prior_restored,
            _ => true,
        };
        Ok(self.exact_state_valid && incarnation_matches && &self.state == expected)
    }

    fn preflight_authority(
        &mut self,
        candidate: &UnitId,
        prior: &hypercolor_cli::install::InstallationState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        assert_eq!(action, InstallAction::PreflightCandidate);
        assert_eq!(candidate.as_str(), CANDIDATE_ID);
        assert_eq!(prior.platform, self.state);
        Self::assert_record(record);
        Self::finish_effect(action, panic_after);
        Ok(())
    }

    fn unload(&mut self, record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::UnloadPrior | InstallAction::UnloadCandidate
        ));
        self.state.loaded = false;
        self.state.running_unit = None;
        Self::finish_effect(action, panic_after);
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::ProvePriorGuardReleased | InstallAction::ProveCandidateGuardReleased
        ));
        assert_eq!(&self.state, unloaded);
        Self::finish_effect(action, panic_after);
        Ok(())
    }

    fn install_launcher(
        &mut self,
        unit: Option<&UnitId>,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::InstallCandidateLauncher | InstallAction::RestorePriorLauncher
        ));
        self.state.launcher_unit = unit.cloned();
        Self::finish_effect(action, panic_after);
        Ok(())
    }

    fn restore_loaded_state(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::ReloadCandidate | InstallAction::ReloadPrior
        ));
        self.state = expected.clone();
        if action == InstallAction::ReloadPrior {
            self.prior_restored = true;
        }
        Self::finish_effect(action, panic_after);
        Ok(())
    }

    fn wait_for_newer_owner(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError> {
        let (action, panic_after) = self.begin_effect()?;
        Self::assert_record(record);
        assert!(matches!(
            action,
            InstallAction::ProveCandidate | InstallAction::ProvePrior
        ));
        if &self.state != expected {
            return Err(InstallPlatformError::new("publication does not match"));
        }
        Self::finish_effect(action, panic_after);
        Ok(())
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
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
            .set_active(Some(&prior.id), &lock)
            .expect("initial active unit");
        drop(lock);

        let owner_sentinel = directory.path().join("owner-state.json");
        let config_sentinel = directory.path().join("hypercolor.toml");
        fs::write(&owner_sentinel, b"owner sentinel").expect("owner sentinel");
        fs::write(&config_sentinel, b"config sentinel").expect("config sentinel");
        Self {
            _directory: directory,
            store,
            prior,
            candidate,
            owner_sentinel,
            config_sentinel,
        }
    }

    fn prior_state(&self) -> PlatformState {
        PlatformState {
            launcher_unit: Some(self.prior.id.clone()),
            loaded: true,
            running_unit: Some(self.prior.id.clone()),
            autostart_enabled: true,
        }
    }

    fn request(&self) -> InstallRequest {
        InstallRequest {
            transaction_id: InstallTransactionId::new("test-transaction")
                .expect("valid transaction ID"),
            candidate: self.candidate.clone(),
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
    let id = UnitId::new(value).expect("valid unit ID");
    let root = store.root().join("units").join(id.as_str());
    fs::create_dir_all(&root).expect("prepared unit directory");
    UnitRecord::new(id, root)
}

fn install(fixture: &Fixture, platform: &mut FakePlatform) -> InstallOutcome {
    InstallCoordinator::new(&fixture.store, platform)
        .install(fixture.request())
        .expect("install transaction")
}

fn seed_rollback(fixture: &Fixture, action: InstallAction) -> FakePlatform {
    let candidate_quiescent = PlatformState {
        launcher_unit: Some(fixture.candidate.id.clone()),
        loaded: false,
        running_unit: None,
        autostart_enabled: true,
    };
    let prior_quiescent = fixture.prior_state().quiescent();
    let (active, state) = match action {
        InstallAction::UnloadCandidate => (
            Some(&fixture.candidate.id),
            PlatformState {
                launcher_unit: Some(fixture.candidate.id.clone()),
                loaded: true,
                running_unit: Some(fixture.candidate.id.clone()),
                autostart_enabled: true,
            },
        ),
        InstallAction::ProveCandidateGuardReleased | InstallAction::RestorePriorActive => {
            (Some(&fixture.candidate.id), candidate_quiescent.clone())
        }
        InstallAction::RestorePriorLauncher => (Some(&fixture.prior.id), candidate_quiescent),
        InstallAction::ReloadPrior => (Some(&fixture.prior.id), prior_quiescent),
        InstallAction::ProvePrior => (Some(&fixture.prior.id), fixture.prior_state()),
        _ => panic!("{action:?} is not a rollback effect"),
    };
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("rollback-replay").expect("transaction ID"),
        Some(fixture.prior.id.clone()),
        fixture.candidate.id.clone(),
        fixture.prior_state(),
        FakePlatform::transaction_record(),
    )
    .expect("journal");
    journal.revision = 20;
    journal.disposition = InstallDisposition::Rollback;
    journal.next_action = Some(action);
    journal.failure = Some("seeded forward failure".to_owned());
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
    platform.prior_restored = action == InstallAction::ProvePrior;
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
            active_unit: fixture.candidate.id.clone()
        }
    );
    assert_eq!(fixture.active_unit(), Some(fixture.candidate.id.clone()));
    assert_eq!(
        platform.effects,
        [
            EffectRecord {
                action: InstallAction::PreflightCandidate,
                active_target: Some(Path::new("units").join(PRIOR_ID)),
            },
            EffectRecord {
                action: InstallAction::UnloadPrior,
                active_target: Some(Path::new("units").join(PRIOR_ID)),
            },
            EffectRecord {
                action: InstallAction::ProvePriorGuardReleased,
                active_target: Some(Path::new("units").join(PRIOR_ID)),
            },
            EffectRecord {
                action: InstallAction::InstallCandidateLauncher,
                active_target: Some(Path::new("units").join(PRIOR_ID)),
            },
            EffectRecord {
                action: InstallAction::ReloadCandidate,
                active_target: Some(Path::new("units").join(CANDIDATE_ID)),
            },
            EffectRecord {
                action: InstallAction::ProveCandidate,
                active_target: Some(Path::new("units").join(CANDIDATE_ID)),
            },
        ]
    );
    let journal = fixture.journal();
    assert_eq!(journal.disposition, InstallDisposition::Committed);
    assert_eq!(journal.revision, 9);
    assert_eq!(journal.platform_record, FakePlatform::transaction_record());
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
fn every_platform_forward_failure_rolls_back_exact_prior_state() {
    for action in [
        InstallAction::PreflightCandidate,
        InstallAction::UnloadPrior,
        InstallAction::ProvePriorGuardReleased,
        InstallAction::InstallCandidateLauncher,
        InstallAction::ReloadCandidate,
        InstallAction::ProveCandidate,
    ] {
        let fixture = Fixture::new();
        let prior = fixture.prior_state();
        let mut platform = FakePlatform::new(prior.clone(), &fixture.store);
        platform.inject(action, InjectionKind::Fail);

        let outcome = install(&fixture, &mut platform);

        assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
        assert_eq!(platform.state, prior, "failed at {action:?}");
        assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
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
                InstallAction::ReloadPrior | InstallAction::ProvePrior
            )));
        }
        fixture.assert_sentinels();
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
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
    fixture.assert_sentinels();
}

#[test]
fn crash_after_platform_effect_is_reconciled_without_repeating_mutation() {
    for action in [
        InstallAction::UnloadPrior,
        InstallAction::InstallCandidateLauncher,
        InstallAction::ReloadCandidate,
    ] {
        let fixture = Fixture::new();
        let mut platform = FakePlatform::new(fixture.prior_state(), &fixture.store);
        platform.inject(action, InjectionKind::PanicAfter);

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            drop(InstallCoordinator::new(&fixture.store, &mut platform).install(fixture.request()));
        }));
        assert!(crashed.is_err(), "{action:?} must inject a crash");
        assert_eq!(fixture.journal().next_action, Some(action));

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
            1,
            "{action:?} must not repeat"
        );
    }
}

#[test]
fn crash_after_active_switch_advances_from_observed_after_state() {
    let fixture = Fixture::new();
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("switch-crash").expect("transaction ID"),
        Some(fixture.prior.id.clone()),
        fixture.candidate.id.clone(),
        fixture.prior_state(),
        FakePlatform::transaction_record(),
    )
    .expect("journal");
    journal.revision = 3;
    journal.next_action = Some(InstallAction::SwitchToCandidate);
    let mut platform = FakePlatform::new(
        PlatformState {
            launcher_unit: Some(fixture.candidate.id.clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: true,
        },
        &fixture.store,
    );
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    fixture
        .store
        .set_active(Some(&fixture.candidate.id), &lock)
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
            InstallAction::ReloadCandidate,
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
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
}

#[test]
fn every_platform_rollback_failure_replays_from_exact_before_state() {
    for action in [
        InstallAction::UnloadCandidate,
        InstallAction::ProveCandidateGuardReleased,
        InstallAction::RestorePriorLauncher,
        InstallAction::ReloadPrior,
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
        assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
    }
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
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
}

#[test]
fn crash_after_rollback_mutation_is_reconciled_without_repeating_it() {
    for action in [
        InstallAction::UnloadCandidate,
        InstallAction::RestorePriorLauncher,
        InstallAction::ReloadPrior,
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
            1,
            "{action:?} must not repeat"
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
        .set_active(Some(&fixture.prior.id), &lock)
        .expect("restore active effect before crash");
    drop(lock);

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect("recover active rollback switch")
        .expect("rollback outcome");

    assert!(matches!(outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(fixture.active_unit(), Some(fixture.prior.id.clone()));
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
}

#[test]
fn oversized_embedded_platform_record_is_rejected_after_bounded_read() {
    let fixture = Fixture::new();
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("oversized-record").expect("transaction ID"),
        Some(fixture.prior.id.clone()),
        fixture.candidate.id.clone(),
        fixture.prior_state(),
        FakePlatform::transaction_record(),
    )
    .expect("journal");
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
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("exact-drift").expect("transaction ID"),
        Some(fixture.prior.id.clone()),
        fixture.candidate.id.clone(),
        fixture.prior_state(),
        FakePlatform::transaction_record(),
    )
    .expect("journal");
    journal.revision = 6;
    journal.next_action = Some(InstallAction::ReloadCandidate);
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .set_active(Some(&fixture.candidate.id), &lock)
        .expect("seed candidate active unit");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    drop(lock);
    let mut platform = FakePlatform::new(
        PlatformState {
            launcher_unit: Some(fixture.candidate.id.clone()),
            loaded: false,
            running_unit: None,
            autostart_enabled: true,
        },
        &fixture.store,
    );
    platform.exact_state_valid = false;

    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .recover()
        .expect_err("exact metadata mismatch must fail closed");

    assert!(matches!(
        error,
        InstallCoordinatorError::StateDrift {
            action: InstallAction::ReloadCandidate,
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
    let mut journal = InstallJournalV1::new(
        InstallTransactionId::new("drift").expect("transaction ID"),
        Some(fixture.prior.id.clone()),
        fixture.candidate.id.clone(),
        fixture.prior_state(),
        FakePlatform::transaction_record(),
    )
    .expect("journal");
    journal.revision = 4;
    journal.next_action = Some(InstallAction::ReloadCandidate);
    let lock = fixture.store.acquire_lock().expect("seed lock");
    fixture
        .store
        .write_journal(&journal, &lock)
        .expect("seed journal");
    fixture
        .store
        .set_active(Some(&third.id), &lock)
        .expect("seed third active state");
    drop(lock);
    let mut platform = FakePlatform::new(
        PlatformState {
            launcher_unit: Some(fixture.candidate.id.clone()),
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
            action: InstallAction::ReloadCandidate,
            ..
        }
    ));
    assert!(platform.effects.is_empty());
    assert_eq!(fixture.journal(), journal);
    fixture.assert_sentinels();
}
