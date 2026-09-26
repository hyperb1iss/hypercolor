#![cfg(unix)]

use std::fs;
use std::path::Path;

use hypercolor_cli::install::{InstallStore, InstallStoreError, UnitId};

fn split(home: &Path) -> InstallStore {
    InstallStore::with_roots(home.join("data/releases"), home.join("state/update"), 65536)
        .expect("separate normalized roots")
}

#[test]
fn immutable_units_and_active_are_separate_from_lock_and_journal() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let store = split(home.path());
    let lock = store.acquire_anchored_lock(home.path()).expect("lock");
    let unit = UnitId::new("a".repeat(64)).expect("unit");
    store.set_active(Some(&unit), &lock).expect("active");
    assert!(store.active_path().is_symlink());
    assert_eq!(store.active_unit(&lock).expect("read active"), Some(unit));
    assert!(store.state_root().join("install.lock").is_file());
    assert!(!store.root().join("install.lock").exists());
    fs::write(store.journal_path(), b"unknown journal").expect("journal fixture");
    assert!(matches!(
        store.load_journal(&lock),
        Err(InstallStoreError::DecodeJournal(_))
    ));
    assert!(!store.root().join("install-journal.json").exists());
}

#[test]
fn split_state_lock_excludes_another_release_store() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let first = split(home.path());
    let _lock = first
        .acquire_anchored_lock(home.path())
        .expect("first lock");
    let second = InstallStore::with_roots(
        home.path().join("other/releases"),
        first.state_root(),
        65536,
    )
    .expect("roots");
    assert!(matches!(
        second.acquire_anchored_lock(home.path()),
        Err(InstallStoreError::LockContended)
    ));
}

#[test]
fn a_lock_cannot_be_reused_with_another_state_root() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let first = split(home.path());
    let lock = first.acquire_anchored_lock(home.path()).expect("lock");
    let other = InstallStore::with_roots(first.root(), home.path().join("other-state"), 65536)
        .expect("roots");
    assert!(matches!(
        other.set_active(None, &lock),
        Err(InstallStoreError::WrongLock)
    ));
}

#[test]
fn equal_nested_and_relative_roots_are_rejected() {
    for (release, state) in [
        ("/data", "/data"),
        ("/data", "/data/state"),
        ("/state/release", "/state"),
        ("relative", "/state"),
        ("/release", "/state/../other"),
    ] {
        assert!(
            InstallStore::with_roots(release, state, 65536).is_err(),
            "{release} {state}"
        );
    }
    assert!(InstallStore::with_roots("/data/releases", "/state/update", 65536).is_ok());
}

#[test]
fn either_root_replacement_refuses_mutation_and_journal_reads() {
    for replace_state in [false, true] {
        let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
        let store = split(home.path());
        let lock = store.acquire_anchored_lock(home.path()).expect("lock");
        let path = if replace_state {
            store.state_root()
        } else {
            store.root()
        };
        let displaced = path.with_extension("displaced");
        fs::rename(path, &displaced).expect("displace");
        fs::create_dir(path).expect("replacement");
        fs::write(path.join("sentinel"), b"untouched").expect("sentinel");
        assert!(store.set_active(None, &lock).is_err());
        assert!(store.load_journal(&lock).is_err());
        assert_eq!(
            fs::read(path.join("sentinel")).expect("sentinel"),
            b"untouched"
        );
        assert!(!path.join("active").exists());
    }
}

#[test]
fn symlinked_release_root_cannot_alias_state() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let state = home.path().join("state");
    let release = home.path().join("release");
    fs::create_dir(&state).expect("state");
    std::os::unix::fs::symlink(&state, &release).expect("alias");
    let store = InstallStore::with_roots(release, state, 65536).expect("lexically distinct");
    assert!(store.acquire_lock().is_err());
}

#[test]
fn legacy_store_keeps_its_original_layout() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let store = InstallStore::new(home.path().join("legacy"), 65536);
    let lock = store
        .acquire_anchored_lock(home.path())
        .expect("legacy lock");
    assert_eq!(store.state_root(), store.root());
    assert!(store.lock_path().is_file());
    assert!(store.load_journal(&lock).expect("journal").is_none());
}

#[test]
fn root_permissions_are_revalidated_before_mutation() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let store = split(home.path());
    let lock = store.acquire_anchored_lock(home.path()).expect("lock");
    fs::set_permissions(store.state_root(), fs::Permissions::from_mode(0o777))
        .expect("change mode");
    assert!(matches!(
        store.set_active(None, &lock),
        Err(InstallStoreError::UnsafeBootstrapDirectory(_, _))
    ));
    assert!(lock.open_store_public_directory().is_err());
    assert!(lock.open_public_directory(home.path()).is_err());
}

#[test]
fn replacing_an_ancestor_cannot_be_hidden_by_preserving_the_root_inode() {
    for replace_state in [false, true] {
        let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
        let store = split(home.path());
        let lock = store.acquire_anchored_lock(home.path()).expect("lock");
        let root = if replace_state {
            store.state_root()
        } else {
            store.root()
        };
        let parent = root.parent().expect("parent");
        let displaced = parent.with_extension("displaced");
        fs::rename(parent, &displaced).expect("displace parent");
        fs::create_dir(parent).expect("new parent");
        fs::rename(displaced.join(root.file_name().expect("root name")), root)
            .expect("preserve root inode");
        assert!(store.set_active(None, &lock).is_err());
        assert!(store.load_journal(&lock).is_err());
    }
}

#[test]
fn state_outside_bootstrap_anchor_is_rejected_before_creating_release_root() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("outside");
    let store = InstallStore::with_roots(
        home.path().join("data/releases"),
        outside.path().join("state"),
        65536,
    )
    .expect("separate roots");
    assert!(matches!(
        store.acquire_anchored_lock(home.path()),
        Err(InstallStoreError::RootOutsideAnchor { .. })
    ));
    assert!(!home.path().join("data").exists());
    assert!(!outside.path().join("state").exists());
}
