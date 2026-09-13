use std::fs;
use std::os::unix::fs::MetadataExt as _;

use crate::install::{
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
                settled.disposition = crate::install::InstallDisposition::Committed;
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
