use std::fs;

use hypercolor_daemon::persistence::write_atomic;

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
