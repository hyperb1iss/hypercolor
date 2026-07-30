use std::fs;

use hypercolor_daemon::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, PersistenceError, write_atomic,
};
use hypercolor_daemon::runtime_state::{RuntimeSessionSnapshot, load, reserve_save, save_reserved};

#[test]
fn atomic_write_replaces_an_existing_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");

    write_atomic(&path, br#"{"generation":1}"#).expect("initial write");
    write_atomic(&path, br#"{"generation":2}"#).expect("replacement write");

    assert_eq!(
        fs::read(&path).expect("read replaced state"),
        br#"{"generation":2}"#
    );
}

#[test]
fn atomic_write_creates_missing_parent_directories() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("nested").join("state.json");

    write_atomic(&path, b"ready").expect("nested write");

    assert_eq!(fs::read(&path).expect("read nested state"), b"ready");
}

#[test]
fn newer_reservation_rejects_a_stale_prepared_payload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    let older = writer.reserve();
    let newer = writer.reserve();

    assert_eq!(
        newer.write(br#"{"generation":2}"#).expect("newer write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(br#"{"generation":1}"#).expect("older write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(
        fs::read(&path).expect("read newest state"),
        br#"{"generation":2}"#
    );
}

#[test]
fn equivalent_parent_aliases_share_generation_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).expect("nested directory");
    let path = directory.path().join("state.json");
    let alias = nested.join("..").join("state.json");
    let first = AtomicFileWriter::new(&path).expect("first writer");
    let older = first.reserve();
    let second = AtomicFileWriter::new(&alias).expect("alias writer");
    let newer = second.reserve();

    assert_eq!(
        newer.write(b"new").expect("newer alias write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"old").expect("older alias write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(fs::read(&path).expect("read aliased state"), b"new");
}

#[cfg(windows)]
#[test]
fn windows_case_aliases_share_generation_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let uppercase = directory.path().join("STATE.JSON");
    let lowercase = directory.path().join("state.json");
    let first = AtomicFileWriter::new(&uppercase).expect("uppercase writer");
    let older = first.reserve();
    let second = AtomicFileWriter::new(&lowercase).expect("lowercase writer");
    let newer = second.reserve();

    assert_eq!(
        newer.write(b"new").expect("newer case-alias write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"old").expect("older case-alias write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(fs::read(&lowercase).expect("read case alias"), b"new");
}

#[test]
fn replacement_failures_retain_the_typed_stage() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let error = write_atomic(directory.path(), b"not a directory")
        .expect_err("a file must not replace a directory");

    assert!(matches!(error, PersistenceError::Replace { .. }));
}

#[test]
fn runtime_reservations_prevent_stale_snapshot_resurrection() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("runtime-state.json");
    let older = reserve_save(&path).expect("older reservation");
    let newer = reserve_save(&path).expect("newer reservation");
    let older_snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some("older".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };
    let newer_snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some("newer".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };

    save_reserved(newer, &newer_snapshot).expect("newer snapshot");
    save_reserved(older, &older_snapshot).expect("stale snapshot");

    let loaded = load(&path)
        .expect("load runtime snapshot")
        .expect("runtime snapshot exists");
    assert_eq!(loaded.active_scene_id.as_deref(), Some("newer"));
}
