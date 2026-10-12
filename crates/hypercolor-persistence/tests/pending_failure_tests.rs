//! A destination reports the failure holding its newest snapshot back.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use hypercolor_persistence::{AtomicFileWriter, PersistenceFlushOutcome};

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("test directory permissions should change");
}

/// Make `path` read-only, or return false when the process can still
/// create files there (root, or a filesystem that ignores mode bits).
fn lock_directory(path: &Path) -> bool {
    set_mode(path, 0o500);
    let probe = path.join(".write-probe");
    if fs::File::create(&probe).is_ok() {
        drop(fs::remove_file(&probe));
        set_mode(path, 0o700);
        return false;
    }
    true
}

#[test]
fn pending_failure_names_the_error_until_the_retry_lands() {
    let root = tempfile::tempdir().expect("tempdir");
    let directory = root.path().join("store");
    fs::create_dir_all(&directory).expect("store directory");
    let path = directory.join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"first").expect("first write lands");
    assert_eq!(writer.pending_failure(), None, "a landed write is clean");

    if !lock_directory(&directory) {
        return;
    }
    let result = writer.write(b"second");
    let failure = writer.pending_failure();
    set_mode(&directory, 0o700);

    let error = result.expect_err("a read-only directory rejects the write");
    let failure = failure.expect("the failed snapshot stays pending");
    assert_eq!(failure, error.to_string());
    assert!(
        failure.contains("state.json"),
        "the failure names its destination: {failure}"
    );

    assert_eq!(
        writer
            .flush(Duration::from_secs(5))
            .expect("retry converges"),
        PersistenceFlushOutcome::Written
    );
    assert_eq!(writer.pending_failure(), None, "the landed retry clears it");
    assert_eq!(fs::read(&path).expect("read converged file"), b"second");
}

#[test]
fn a_newer_admission_keeps_the_failure_until_something_lands() {
    let root = tempfile::tempdir().expect("tempdir");
    let directory = root.path().join("store");
    fs::create_dir_all(&directory).expect("store directory");
    let path = directory.join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"first").expect("first write lands");

    if !lock_directory(&directory) {
        return;
    }
    let failed = writer.write(b"second");
    let newer = writer.reserve().admit(b"third".to_vec());
    let failure_while_newer_waits = writer.pending_failure();
    set_mode(&directory, 0o700);

    failed.expect_err("a read-only directory rejects the write");
    assert!(
        failure_while_newer_waits.is_some(),
        "admitting a newer snapshot does not make the lagging file current"
    );
    assert!(
        newer.commit().is_ok(),
        "the newer snapshot lands once the directory is writable"
    );
    assert_eq!(writer.pending_failure(), None, "the landed write clears it");
    assert_eq!(fs::read(&path).expect("read converged file"), b"third");
}
