use std::fs;
use std::path::PathBuf;

use hypercolor_platform_fs::WindowsSid;

use super::{InstallStore, InstallStoreError, UnitCollection};
use crate::model::{
    InstallJournalV1, InstallTargetPolicy, InstallTransactionId, PlatformState,
    PlatformTransactionRecord, PlatformTransitionStates, UnitId,
};
use crate::windows::WindowsOwnershipPolicy;

const JOURNAL_LIMIT: usize = 64 * 1024;

struct Fixture {
    _guard: tempfile::TempDir,
    release: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let guard = tempfile::tempdir().expect("temporary install roots");
        // Runners hand out 8.3 short temp paths; the store wants real names.
        let base = fs::canonicalize(guard.path()).expect("canonical roots");
        let base = PathBuf::from(
            base.to_str()
                .expect("UTF-8 temporary path")
                .trim_start_matches(r"\\?\"),
        );
        let release = base.join("Program Files").join("Hypercolor");
        let state = base.join("ProgramData").join("Hypercolor").join("update");
        fs::create_dir_all(&release).expect("release root");
        fs::create_dir_all(&state).expect("state root");
        Self {
            _guard: guard,
            release,
            state,
        }
    }

    /// The test runs as its own user beneath its own profile, so it trusts
    /// that user on top of the machine principals.
    fn store(&self) -> InstallStore {
        InstallStore::with_roots(&self.release, &self.state, JOURNAL_LIMIT)
            .expect("store")
            .with_ownership_policy(WindowsOwnershipPolicy::trusting([
                WindowsSid::current_user().expect("current user"),
            ]))
    }
}

fn unit(byte: char) -> UnitId {
    UnitId::new(byte.to_string().repeat(64)).expect("unit")
}

fn journal(candidate: &UnitId) -> InstallJournalV1 {
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
        InstallTransactionId::new("windows-store-test").expect("transaction"),
        None,
        candidate.clone(),
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
fn the_lock_is_exclusive_and_bound_to_its_store() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    assert!(matches!(
        store.acquire_lock(),
        Err(InstallStoreError::LockContended)
    ));

    let other = Fixture::new();
    let other_store = other.store();
    let other_lock = other_store.acquire_lock().expect("other lock");
    assert!(matches!(
        store.active_unit(&other_lock),
        Err(InstallStoreError::WrongLock)
    ));
    drop(lock);
    store.acquire_lock().expect("relock after release");
}

#[test]
fn the_active_pointer_switches_in_one_step() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    assert_eq!(store.active_unit(&lock).expect("absent"), None);

    let first = unit('a');
    store.set_active(Some(&first), &lock).expect("activate");
    assert_eq!(store.active_unit(&lock).expect("active"), Some(first));
    let second = unit('b');
    store.set_active(Some(&second), &lock).expect("switch");
    assert_eq!(store.active_unit(&lock).expect("switched"), Some(second));
    assert_eq!(
        fs::read_dir(&fixture.state)
            .expect("state entries")
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".active.json."))
            .count(),
        0,
        "no temporary pointer is left behind"
    );

    store.set_active(None, &lock).expect("deactivate");
    assert_eq!(store.active_unit(&lock).expect("cleared"), None);
    store
        .set_active(None, &lock)
        .expect("clearing twice is a no-op");
}

#[test]
fn a_damaged_pointer_is_refused_rather_than_guessed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    for contents in [
        "units\\aaaa".to_owned(),
        format!(r#"{{"schema_version":2,"unit":"{}"}}"#, "a".repeat(64)),
        format!(
            r#"{{"schema_version":1,"unit":"{}","extra":true}}"#,
            "a".repeat(64)
        ),
        r#"{"schema_version":1,"unit":"not-a-digest"}"#.to_owned(),
        " ".repeat(8 * 1024),
    ] {
        fs::write(store.active_path(), contents).expect("seed pointer");
        assert!(
            matches!(
                store.active_unit(&lock),
                Err(InstallStoreError::InvalidActivePointer(_))
            ),
            "a damaged pointer must not name a unit"
        );
    }
}

#[test]
fn the_journal_round_trips_and_stays_bounded() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    assert!(store.load_journal(&lock).expect("absent").is_none());

    let written = journal(&unit('c'));
    store.write_journal(&written, &lock).expect("write");
    assert_eq!(store.load_journal(&lock).expect("load"), Some(written));

    fs::write(store.journal_path(), vec![b' '; JOURNAL_LIMIT + 1]).expect("oversize");
    assert!(matches!(
        store.load_journal(&lock),
        Err(InstallStoreError::JournalTooLarge { .. })
    ));
    let tight = InstallStore::with_roots(&fixture.release, &fixture.state, 16)
        .expect("store")
        .with_ownership_policy(store.ownership_policy().clone());
    drop(lock);
    let lock = tight.acquire_lock().expect("lock");
    assert!(matches!(
        tight.write_journal(&journal(&unit('c')), &lock),
        Err(InstallStoreError::JournalTooLarge { .. })
    ));
}

#[test]
fn collection_keeps_referenced_units_and_removes_the_rest() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    assert!(
        store
            .units_directory(&lock, false)
            .expect("absent")
            .is_none()
    );
    store.units_directory(&lock, true).expect("create units");

    let active = unit('a');
    let retained = unit('b');
    let stale = unit('c');
    for id in [&active, &retained, &stale] {
        let path = store.unit_path(id);
        fs::create_dir_all(path.join("nested")).expect("unit");
        fs::write(path.join("nested").join("member"), b"bytes").expect("member");
    }
    let leftover = fixture.release.join("units").join(".hypercolor-stage-1-2");
    fs::create_dir(&leftover).expect("leftover");
    let foreign = fixture.release.join("units").join("not-ours");
    fs::create_dir(&foreign).expect("foreign entry");
    fs::write(fixture.state.join(".install-journal.json.9.9"), b"torn").expect("torn journal");
    store.set_active(Some(&active), &lock).expect("activate");

    assert!(matches!(
        store.remove_unit(&lock, &active),
        Err(InstallStoreError::UnitReferenced(_))
    ));
    store
        .open_unit_directory(&lock, &stale)
        .expect("an installed unit opens");

    let collection = store
        .collect_units(&lock, std::slice::from_ref(&retained))
        .expect("collect");
    assert_eq!(
        collection,
        UnitCollection {
            removed_units: vec![stale.clone()],
            removed_leftovers: vec![
                leftover.clone(),
                fixture.state.join(".install-journal.json.9.9")
            ],
            refused: Vec::new(),
        }
    );
    assert!(store.unit_path(&active).is_dir());
    assert!(store.unit_path(&retained).is_dir());
    assert!(!store.unit_path(&stale).exists());
    assert!(foreign.is_dir(), "names the installer never makes stay");
    assert!(store.open_unit_directory(&lock, &stale).is_err());
    assert!(!store.remove_unit(&lock, &stale).expect("already gone"));
    assert!(store.remove_unit(&lock, &retained).expect("remove"));
}

#[test]
fn roots_must_be_plain_distinct_and_trusted() {
    let fixture = Fixture::new();
    for (release, state) in [
        (fixture.release.clone(), fixture.release.join("update")),
        (fixture.state.join("units"), fixture.state.clone()),
        (PathBuf::from(r"relative\path"), fixture.state.clone()),
        (fixture.release.join(".."), fixture.state.clone()),
    ] {
        assert!(
            InstallStore::with_roots(&release, &state, JOURNAL_LIMIT).is_err(),
            "{} and {} must be refused",
            release.display(),
            state.display()
        );
    }

    let untrusting =
        InstallStore::with_roots(&fixture.release, &fixture.state, JOURNAL_LIMIT).expect("store");
    assert!(
        matches!(
            untrusting.acquire_lock(),
            Err(InstallStoreError::Untrusted(..))
        ),
        "a profile the machine policy does not trust is refused"
    );

    let missing = InstallStore::with_roots(
        fixture.release.with_file_name("Missing"),
        &fixture.state,
        JOURNAL_LIMIT,
    )
    .expect("store")
    .with_ownership_policy(fixture.store().ownership_policy().clone());
    assert!(matches!(
        missing.acquire_lock(),
        Err(InstallStoreError::OpenRoot { .. })
    ));
}

#[test]
fn a_reparse_point_on_the_path_is_refused() {
    let fixture = Fixture::new();
    let target = fixture.release.with_file_name("Elsewhere");
    fs::create_dir(&target).expect("target");
    let link = fixture.release.with_file_name("Linked");
    if let Err(error) = std::os::windows::fs::symlink_dir(&target, &link) {
        eprintln!("skipping reparse refusal: {error}");
        return;
    }
    let store = InstallStore::with_roots(&link, &fixture.state, JOURNAL_LIMIT)
        .expect("store")
        .with_ownership_policy(fixture.store().ownership_policy().clone());
    assert!(matches!(
        store.acquire_lock(),
        Err(InstallStoreError::OpenRoot { .. })
    ));
}

/// Change an entry's DACL the way an administrator would, with `icacls`.
fn icacls(path: &std::path::Path, arguments: &[&str]) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(arguments)
        .status()
        .expect("run icacls");
    assert!(status.success(), "icacls {} {arguments:?}", path.display());
}

#[test]
fn control_files_and_roots_that_users_can_change_are_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let lock = store.acquire_lock().expect("lock");
    store
        .write_journal(&journal(&unit('d')), &lock)
        .expect("journal");
    store.set_active(Some(&unit('d')), &lock).expect("pointer");
    icacls(&store.journal_path(), &["/grant", "*S-1-5-32-545:(W)"]);
    assert!(matches!(
        store.load_journal(&lock),
        Err(InstallStoreError::Untrusted(..))
    ));
    icacls(&store.active_path(), &["/grant", "*S-1-5-32-545:(M)"]);
    assert!(matches!(
        store.active_unit(&lock),
        Err(InstallStoreError::Untrusted(..))
    ));
    drop(lock);

    // A write right the state root only passes to new files still reaches
    // the journal and pointer the store creates there.
    let inheriting = Fixture::new();
    icacls(&inheriting.state, &["/grant", "*S-1-5-32-545:(OI)(IO)(W)"]);
    assert!(matches!(
        inheriting.store().acquire_lock(),
        Err(InstallStoreError::Untrusted(..))
    ));
}
