use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

use crate::{
    InstallJournalV1, InstallLock, InstallStore, InstallTargetPolicy, InstallTransactionId,
    PlatformState, PlatformTransactionRecord, PlatformTransitionStates, UnitId,
};

use super::{LinuxInstallAuthority, LinuxInstallLocation, LinuxInstallLocator, LinuxLocatorError};

#[path = "locator_test_platform.rs"]
mod platform;
use platform::PriorProof;

struct Fixture {
    home: tempfile::TempDir,
    old: InstallStore,
    old_lock: InstallLock,
    locator: LinuxInstallLocator,
    location: LinuxInstallLocation,
    state: InstallStore,
    state_lock: InstallLock,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
        let old = InstallStore::new(home.path().join(".local/lib/hypercolor"), 65536);
        let old_lock = old
            .acquire_anchored_lock(home.path())
            .expect("old lock first");
        let locator = LinuxInstallLocator::retain(home.path(), &old_lock).expect("locator");
        let location = LinuxInstallLocation::new(
            home.path(),
            &home.path().join("data"),
            &home.path().join("state"),
            &home.path().join("config"),
            fs::metadata(home.path()).expect("owner").uid(),
        )
        .expect("location");
        let state = InstallStore::with_roots(location.release_root(), location.state_root(), 65536)
            .expect("state");
        let state_lock = state
            .acquire_anchored_lock(home.path())
            .expect("state lock second");
        fs::create_dir_all(location.config_root()).expect("config root");
        Self {
            home,
            old,
            old_lock,
            locator,
            location,
            state,
            state_lock,
        }
    }

    fn prepare(&self) {
        self.locator
            .prepare_adoption(
                &self.location,
                &journal(),
                &self.state,
                &self.state_lock,
                &mut PriorProof::valid(),
            )
            .expect("bound preparation receipt");
        self.state
            .write_journal(&journal(), &self.state_lock)
            .expect("prepared journal");
        fs::write(
            self.location.state_root().join("installation.json"),
            serde_json::to_vec(&self.location).expect("identity"),
        )
        .expect("write identity");
    }
}

fn journal() -> InstallJournalV1 {
    let candidate = UnitId::new("a".repeat(64)).expect("candidate");
    let prior = PlatformState {
        layout_unit: None,
        launcher_unit: None,
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };
    let target = PlatformState {
        layout_unit: Some(candidate.clone()),
        ..prior.clone()
    };
    InstallJournalV1::new(
        InstallTransactionId::new("test-adoption").expect("transaction"),
        None,
        candidate,
        prior.clone(),
        InstallTargetPolicy::Preserve,
        PlatformTransitionStates {
            prior_unloaded: prior.clone(),
            candidate_manager: target.clone(),
            candidate_autostart: target,
            prior_manager: prior.clone(),
            prior_autostart: prior,
        },
        1,
        PlatformTransactionRecord::linux(1, b"fixture platform preparation".to_vec())
            .expect("record"),
    )
    .expect("journal")
}

#[test]
fn absent_and_valid_v1_journals_select_legacy() {
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.locator.read().expect("absent"),
        LinuxInstallAuthority::Legacy(None)
    ));
    fixture
        .old
        .write_journal(&journal(), &fixture.old_lock)
        .expect("legacy journal");
    assert!(matches!(
        fixture.locator.read().expect("v1"),
        LinuxInstallAuthority::Legacy(Some(_))
    ));
}

#[test]
fn incomplete_preparation_and_pending_legacy_transaction_refuse_publication() {
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.locator.publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid()
        ),
        Err(LinuxLocatorError::Unprepared)
    ));
    assert!(!fixture.old.journal_path().exists());
    fixture.prepare();
    fixture
        .old
        .write_journal(&journal(), &fixture.old_lock)
        .expect("legacy pending");
    assert!(matches!(
        fixture.locator.publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid()
        ),
        Err(LinuxLocatorError::Unprepared)
    ));
    assert!(matches!(
        fixture.locator.read().expect("legacy intact"),
        LinuxInstallAuthority::Legacy(Some(_))
    ));
}

#[test]
fn prepared_publication_permanently_fences_the_old_journal_decoder() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    assert!(matches!(
        fixture.locator.read().expect("managed"),
        LinuxInstallAuthority::Managed(_)
    ));
    assert!(fixture.old.load_journal(&fixture.old_lock).is_err());
    fixture
        .locator
        .confirm_durable(&fixture.location)
        .expect("durability");
    assert!(matches!(
        fixture.locator.publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid()
        ),
        Err(LinuxLocatorError::AlreadyManaged)
    ));
    assert!(
        fixture
            .state
            .load_journal(&fixture.state_lock)
            .expect("prepared retained")
            .is_some()
    );
}

#[test]
fn ambiguous_publication_error_keeps_managed_authority_and_requires_barrier_retry() {
    let fixture = Fixture::new();
    fixture.prepare();
    let result = fixture.locator.publish_prepared_with(
        &fixture.location,
        &fixture.state,
        &fixture.state_lock,
        &mut PriorProof::valid(),
        |directory, source, destination| {
            directory.durable_replace_file(source, destination)?;
            Err(std::io::Error::other(
                "injected unknown postvisibility result",
            ))
        },
    );
    assert!(result.is_err());
    assert!(matches!(
        fixture.locator.read().expect("visible managed authority"),
        LinuxInstallAuthority::Managed(_)
    ));
    assert!(fixture.old.load_journal(&fixture.old_lock).is_err());
    fixture
        .locator
        .confirm_durable(&fixture.location)
        .expect("barrier retry");
    let other = LinuxInstallLocation::new(
        fixture.home.path(),
        &fixture.home.path().join("data"),
        &fixture.home.path().join("state"),
        &fixture.home.path().join("config"),
        fixture.location.uid(),
    )
    .expect("different identity");
    assert!(fixture.locator.confirm_durable(&other).is_err());
}

#[test]
fn malformed_or_unknown_location_never_falls_back_to_legacy() {
    let fixture = Fixture::new();
    for bytes in [
        b"not json".as_slice(),
        br#"{"schema_version":2}"#,
        br#"{"schema_version":3}"#,
        br#"{"schema_version":1}"#,
    ] {
        fs::write(fixture.old.journal_path(), bytes).expect("malformed");
        assert!(fixture.locator.read().is_err());
    }
    assert!(matches!(
        LinuxInstallLocator::retain(fixture.home.path(), &fixture.state_lock),
        Err(LinuxLocatorError::WrongLock)
    ));
}

#[test]
fn missing_identity_advanced_journal_and_wrong_state_lock_cannot_publish() {
    for scenario in 0..3 {
        let fixture = Fixture::new();
        fixture.prepare();
        match scenario {
            0 => fs::remove_file(fixture.location.state_root().join("installation.json"))
                .expect("remove identity"),
            1 => {
                let mut advanced = journal();
                advanced.revision = 2;
                fixture
                    .state
                    .write_journal(&advanced, &fixture.state_lock)
                    .expect("advanced journal");
            }
            _ => {}
        }
        let lock = if scenario == 2 {
            &fixture.old_lock
        } else {
            &fixture.state_lock
        };
        assert!(
            fixture
                .locator
                .publish_prepared(
                    &fixture.location,
                    &fixture.state,
                    lock,
                    &mut PriorProof::valid()
                )
                .is_err()
        );
        assert!(!fixture.old.journal_path().exists());
    }
}

#[test]
fn orphan_journal_cannot_acquire_a_receipt_after_the_fact() {
    let fixture = Fixture::new();
    fixture
        .state
        .write_journal(&journal(), &fixture.state_lock)
        .expect("orphan journal");
    assert!(matches!(
        fixture.locator.prepare_adoption(
            &fixture.location,
            &journal(),
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid()
        ),
        Err(LinuxLocatorError::Unprepared)
    ));
    assert!(
        !fixture
            .location
            .state_root()
            .join("adoption-preparation.json")
            .exists()
    );
    assert!(!fixture.old.journal_path().exists());
}

#[test]
fn receipt_binds_initial_journal_identity_and_unchanged_legacy_observations() {
    for scenario in 0..3 {
        let fixture = Fixture::new();
        fixture.prepare();
        match scenario {
            0 => {
                let mut unrelated = journal();
                unrelated.transaction_id =
                    InstallTransactionId::new("unrelated-orphan").expect("id");
                fixture
                    .state
                    .write_journal(&unrelated, &fixture.state_lock)
                    .expect("unrelated initial journal");
            }
            1 => {
                let mut settled = journal();
                settled.disposition = crate::InstallDisposition::Committed;
                settled.next_action = None;
                settled.layout_operation_index = settled.layout_operation_count;
                fixture
                    .old
                    .write_journal(&settled, &fixture.old_lock)
                    .expect("changed settled journal");
            }
            _ => fixture
                .old
                .set_active(
                    Some(&UnitId::new("b".repeat(64)).expect("other unit")),
                    &fixture.old_lock,
                )
                .expect("changed active pointer"),
        }
        assert!(matches!(
            fixture.locator.publish_prepared(
                &fixture.location,
                &fixture.state,
                &fixture.state_lock,
                &mut PriorProof::valid()
            ),
            Err(LinuxLocatorError::Unprepared)
        ));
        assert!(matches!(
            fixture.locator.read().expect("legacy retained"),
            LinuxInstallAuthority::Legacy(_)
        ));
    }
}

#[test]
fn current_platform_must_still_match_the_prepared_prior_original() {
    let fixture = Fixture::new();
    fixture.prepare();
    assert!(matches!(
        fixture.locator.publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof { matches: false }
        ),
        Err(LinuxLocatorError::Unprepared)
    ));
    assert!(!fixture.old.journal_path().exists());
}

#[test]
fn identical_preparation_reuses_the_durable_receipt_without_replacing_it() {
    let fixture = Fixture::new();
    fixture.prepare();
    let path = fixture
        .location
        .state_root()
        .join("adoption-preparation.json");
    let before = fs::metadata(&path).expect("receipt").ino();
    fixture
        .locator
        .prepare_adoption(
            &fixture.location,
            &journal(),
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("exact retry");
    assert_eq!(fs::metadata(path).expect("same receipt").ino(), before);
}

#[test]
fn receipt_only_restart_retains_the_exact_initial_transaction() {
    let fixture = Fixture::new();
    let intended = journal();
    fixture
        .locator
        .prepare_adoption(
            &fixture.location,
            &intended,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("durable receipt before state journal");
    assert!(!fixture.state.journal_path().exists());
    let Fixture {
        home,
        old,
        old_lock,
        locator,
        location,
        state,
        state_lock,
    } = fixture;
    drop(locator);
    drop(state_lock);
    drop(old_lock);
    let old_lock = old.acquire_lock().expect("cold old lock first");
    let locator = LinuxInstallLocator::retain(home.path(), &old_lock).expect("cold locator");
    let state_lock = state.acquire_lock().expect("cold state lock second");
    let recovered = locator
        .prepared_journal(&location, &state, &state_lock)
        .expect("unchanged receipt")
        .expect("original proposal");
    assert_eq!(recovered, intended);
    locator
        .prepare_adoption(
            &location,
            &recovered,
            &state,
            &state_lock,
            &mut PriorProof::valid(),
        )
        .expect("resume exact proposal");
    state
        .write_journal(&recovered, &state_lock)
        .expect("finish state journal");
    assert_eq!(
        locator
            .prepared_journal(&location, &state, &state_lock)
            .expect("read after journal")
            .expect("proposal"),
        intended
    );
}

#[test]
fn receipt_resume_rejects_changed_body_hash_and_legacy_authority() {
    for scenario in 0..4 {
        let fixture = Fixture::new();
        fixture.prepare();
        let path = fixture
            .location
            .state_root()
            .join("adoption-preparation.json");
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("receipt bytes")).expect("receipt JSON");
        match scenario {
            0 => receipt["initial_journal"]["transaction_id"] = "different-transaction".into(),
            1 => {
                receipt["journal_sha256"][0] =
                    (receipt["journal_sha256"][0].as_u64().expect("digest byte") ^ 1).into();
            }
            2 => receipt["schema_version"] = 1.into(),
            _ => fixture
                .old
                .set_active(
                    Some(&UnitId::new("b".repeat(64)).expect("changed unit")),
                    &fixture.old_lock,
                )
                .expect("changed legacy active"),
        }
        fs::write(
            &path,
            serde_json::to_vec(&receipt).expect("changed receipt"),
        )
        .expect("write receipt");
        assert!(
            fixture
                .locator
                .prepared_journal(&fixture.location, &fixture.state, &fixture.state_lock,)
                .is_err()
        );
        assert!(!fixture.old.journal_path().exists());
    }
}

#[test]
fn writable_preparation_files_cannot_authorize_locator_publication() {
    use std::os::unix::fs::PermissionsExt as _;
    for name in [
        "installation.json",
        "adoption-preparation.json",
        "install-journal.json",
    ] {
        let fixture = Fixture::new();
        fixture.prepare();
        fs::set_permissions(
            fixture.location.state_root().join(name),
            fs::Permissions::from_mode(0o666),
        )
        .expect("make preparation writable");
        assert!(matches!(
            fixture.locator.publish_prepared(
                &fixture.location,
                &fixture.state,
                &fixture.state_lock,
                &mut PriorProof::valid()
            ),
            Err(LinuxLocatorError::Unprepared)
        ));
        assert!(!fixture.old.journal_path().exists());
    }
}

#[test]
fn managed_election_uses_only_state_lock_while_old_lock_is_held() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    let elected =
        super::elect_linux_installation(fixture.home.path()).expect("state-only election");
    let super::LinuxInstallElection::Managed {
        store,
        lock,
        authority,
    } = elected
    else {
        panic!("managed authority")
    };
    assert_eq!(store.root(), fixture.location.release_root());
    assert_eq!(authority.location(), &fixture.location);
    authority.confirm_durable().expect("durable authority");
    let identity = fixture.location.state_root().join("installation.json");
    let bytes = fs::read(&identity).expect("identity bytes");
    fs::rename(
        &identity,
        fixture.location.state_root().join("previous-identity.json"),
    )
    .expect("retain previous identity inode");
    fs::write(&identity, bytes).expect("replace identical identity");
    assert!(
        authority.confirm_durable().is_err(),
        "replacement identity is not original authority"
    );
    assert!(LinuxInstallLocator::retain(fixture.home.path(), &lock).is_err());
    assert!(
        fixture.old.acquire_lock().is_err(),
        "old lock remains held independently"
    );
}

#[test]
fn managed_election_never_bootstraps_missing_state_or_falls_back() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    let relocated = fixture.home.path().join("displaced-state");
    fs::rename(fixture.location.state_root(), &relocated).expect("displace state");
    assert!(super::elect_linux_installation(fixture.home.path()).is_err());
    assert!(!fixture.location.state_root().exists());
    assert!(matches!(
        fixture.locator.read().expect("permanent locator"),
        LinuxInstallAuthority::Managed(_)
    ));
}

#[test]
fn managed_election_requires_matching_identity_and_existing_journal() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    let journal = fixture.location.state_root().join("install-journal.json");
    let bytes = fs::read(&journal).expect("journal");
    fs::remove_file(&journal).expect("remove journal");
    assert!(super::elect_linux_installation(fixture.home.path()).is_err());
    fs::write(&journal, bytes).expect("restore journal");
    fs::write(
        fixture.location.state_root().join("installation.json"),
        b"{}",
    )
    .expect("corrupt identity");
    assert!(super::elect_linux_installation(fixture.home.path()).is_err());
}

#[test]
fn legacy_election_holds_the_original_install_lock() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let elected = super::elect_linux_installation(home.path()).expect("legacy authority");
    let super::LinuxInstallElection::Legacy { store, locator, .. } = elected else {
        panic!("legacy authority")
    };
    assert!(matches!(
        locator.read().expect("legacy journal"),
        LinuxInstallAuthority::Legacy(None)
    ));
    assert!(store.acquire_lock().is_err());
}

#[test]
fn adoption_refuses_pending_legacy_before_creating_recorded_roots() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let elected = super::elect_linux_installation(home.path()).expect("legacy election");
    let super::LinuxInstallElection::Legacy { store, lock, .. } = &elected else {
        panic!("legacy authority")
    };
    store
        .write_journal(&journal(), lock)
        .expect("pending legacy transaction");
    let proposed = LinuxInstallLocation::new(
        home.path(),
        &home.path().join("new-data"),
        &home.path().join("new-state"),
        &home.path().join("new-config"),
        fs::metadata(home.path()).expect("owner").uid(),
    )
    .expect("location");
    assert!(matches!(
        crate::LinuxAdoption::begin(home.path(), elected, proposed),
        Err(crate::LinuxAdoptionError::LegacyRecoveryRequired)
    ));
    for name in ["new-data", "new-state", "new-config"] {
        assert!(!home.path().join(name).exists());
    }
}

#[test]
fn adoption_reuses_recorded_identity_and_refuses_orphan_state_journal() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let propose = || {
        LinuxInstallLocation::new(
            home.path(),
            &home.path().join("data"),
            &home.path().join("state"),
            &home.path().join("config"),
            fs::metadata(home.path()).expect("owner").uid(),
        )
        .expect("location")
    };
    let adoption = crate::LinuxAdoption::begin(
        home.path(),
        super::elect_linux_installation(home.path()).expect("election"),
        propose(),
    )
    .expect("begin preparation");
    let original_id = adoption.location().installation_id();
    adoption
        .store()
        .write_journal(&journal(), adoption.lock())
        .expect("unbound orphan");
    drop(adoption);
    let adoption = crate::LinuxAdoption::begin(
        home.path(),
        super::elect_linux_installation(home.path()).expect("cold election"),
        propose(),
    )
    .expect("retain original identity");
    assert_eq!(adoption.location().installation_id(), original_id);
    assert!(adoption.prepared_journal().is_err());
    assert!(matches!(
        super::LinuxInstallLocator::retain(home.path(), adoption.lock()),
        Err(LinuxLocatorError::WrongLock)
    ));
}

#[test]
fn managed_election_rechecks_the_locator_after_the_unlocked_hint() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    let result = super::election::elect_with(
        fixture.home.path(),
        &crate::OwnershipPolicy::system(),
        || {
            fs::write(
                fixture.old.root().join("install-journal.json"),
                br#"{"schema_version":99}"#,
            )
            .expect("replace locator after hint");
        },
    );
    assert!(result.is_err());
    assert!(
        fixture.state.acquire_lock().is_ok(),
        "failed election releases state lock"
    );
}

#[test]
fn legacy_hint_rechecks_a_concurrently_published_managed_locator() {
    let fixture = Fixture::new();
    fixture.prepare();
    let Fixture {
        home,
        old: _,
        old_lock,
        locator,
        location,
        state,
        state_lock,
    } = fixture;
    let expected = location.clone();
    let elected =
        super::election::elect_with(home.path(), &crate::OwnershipPolicy::system(), move || {
            locator
                .publish_prepared(&location, &state, &state_lock, &mut PriorProof::valid())
                .expect("publish after legacy hint");
            drop(locator);
            drop(state_lock);
            drop(old_lock);
        })
        .expect("reread selects managed authority");
    let super::LinuxInstallElection::Managed { authority, .. } = elected else {
        panic!("managed authority")
    };
    assert_eq!(authority.location(), &expected);
}

#[test]
fn managed_election_refuses_a_writable_permanent_locator() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    fs::set_permissions(
        fixture.old.root().join("install-journal.json"),
        fs::Permissions::from_mode(0o666),
    )
    .expect("make locator writable");
    assert!(super::elect_linux_installation(fixture.home.path()).is_err());
}

#[test]
fn managed_election_refuses_writable_state_journal_and_allows_normal_replacement() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    drop(fixture.state_lock);
    let path = fixture.location.state_root().join("install-journal.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("writable journal");
    assert!(super::elect_linux_installation(fixture.home.path()).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private journal");
    let elected = super::elect_linux_installation(fixture.home.path()).expect("valid journal");
    let super::LinuxInstallElection::Managed {
        store,
        lock,
        authority,
    } = elected
    else {
        panic!("managed authority")
    };
    let journal = store
        .load_journal(&lock)
        .expect("read journal")
        .expect("journal exists");
    store
        .write_journal(&journal, &lock)
        .expect("normal atomic journal replacement");
    authority
        .confirm_durable()
        .expect("journal is mutable during recovery");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("late writable journal");
    assert!(authority.confirm_durable().is_err());
}

#[test]
fn unpublished_preparation_is_discarded_only_while_initial_and_unpublished() {
    let fixture = Fixture::new();
    fixture.prepare();
    let state = fixture.location.state_root();
    assert!(state.join("install-journal.json").exists());
    fixture
        .locator
        .discard_preparation(&fixture.location, &fixture.state, &fixture.state_lock)
        .expect("discard an initial unpublished preparation");
    assert!(!state.join("install-journal.json").exists());
    assert!(
        !state
            .join(super::super::locator_receipt::RECEIPT_NAME)
            .exists()
    );
    assert!(
        state.join("installation.json").exists(),
        "the identity stays"
    );

    fixture.prepare();
    let mut advanced = journal();
    advanced.revision += 1;
    fixture
        .state
        .write_journal(&advanced, &fixture.state_lock)
        .expect("an advanced journal");
    assert!(matches!(
        fixture
            .locator
            .discard_preparation(&fixture.location, &fixture.state, &fixture.state_lock),
        Err(LinuxLocatorError::Unprepared)
    ));
    assert!(state.join("install-journal.json").exists());

    let fixture = Fixture::new();
    fixture.prepare();
    fixture
        .locator
        .publish_prepared(
            &fixture.location,
            &fixture.state,
            &fixture.state_lock,
            &mut PriorProof::valid(),
        )
        .expect("publish");
    assert!(matches!(
        fixture
            .locator
            .discard_preparation(&fixture.location, &fixture.state, &fixture.state_lock),
        Err(LinuxLocatorError::AlreadyManaged)
    ));
    assert!(state_journal_exists(&fixture));
}

fn state_journal_exists(fixture: &Fixture) -> bool {
    fixture
        .location
        .state_root()
        .join("install-journal.json")
        .exists()
}

#[test]
fn adoption_intent_is_recorded_once_and_wins_over_later_proposals() {
    let fixture = Fixture::new();
    assert!(fixture.locator.adoption_intent().expect("absent").is_none());
    let recorded = fixture
        .locator
        .record_adoption_intent(fixture.location.clone())
        .expect("record");
    assert_eq!(recorded, fixture.location);
    let other = LinuxInstallLocation::new(
        fixture.home.path(),
        &fixture.home.path().join("other-data"),
        &fixture.home.path().join("other-state"),
        &fixture.home.path().join("other-config"),
        fixture.location.uid(),
    )
    .expect("other");
    assert_eq!(
        fixture
            .locator
            .record_adoption_intent(other)
            .expect("existing intent wins"),
        fixture.location
    );
    assert_eq!(
        fixture.locator.adoption_intent().expect("read"),
        Some(fixture.location.clone())
    );
    let intent = fixture
        .home
        .path()
        .join(".local/lib/hypercolor/managed-adoption.json");
    assert_eq!(
        fs::metadata(&intent).expect("intent").permissions().mode() & 0o777,
        0o600
    );
    fs::write(&intent, b"{\"schema_version\":2}").expect("corrupt intent");
    assert!(fixture.locator.adoption_intent().is_err());
}
