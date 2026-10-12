//! A payload identical to the destination's last durable bytes lands
//! without touching the filesystem.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::time::Duration;

use hypercolor_persistence::{AtomicFileWriter, AtomicWriteOutcome, PersistenceFlushOutcome};

/// The inode behind `path`. Every real replacement renames a fresh file
/// over the destination, so an unchanged inode proves no replacement ran.
fn inode(path: &Path) -> u64 {
    fs::metadata(path).expect("destination metadata").ino()
}

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
fn an_identical_payload_lands_without_replacing_the_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"stable").expect("first write lands");
    let before = inode(&path);

    for _ in 0..3 {
        assert_eq!(
            writer.write(b"stable").expect("identical write lands"),
            AtomicWriteOutcome::Written
        );
    }

    assert_eq!(inode(&path), before, "no replacement for identical bytes");
    assert_eq!(fs::read(&path).expect("read destination"), b"stable");
}

#[test]
fn a_changed_payload_still_replaces_the_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"first").expect("first write lands");
    let before = inode(&path);

    assert_eq!(
        writer.write(b"second").expect("changed write lands"),
        AtomicWriteOutcome::Written
    );

    assert_ne!(inode(&path), before, "changed bytes replace the file");
    assert_eq!(fs::read(&path).expect("read destination"), b"second");
}

#[test]
fn writers_sharing_a_live_destination_share_its_durable_digest() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let first = AtomicFileWriter::new(&path).expect("first writer");
    first.write(b"shared").expect("first write lands");
    let before = inode(&path);

    // A short-lived writer of the same path resolves to the destination the
    // first writer keeps alive, digest included.
    let second = AtomicFileWriter::new(&path).expect("second writer");
    assert_eq!(
        second.write(b"shared").expect("identical write lands"),
        AtomicWriteOutcome::Written
    );

    assert_eq!(inode(&path), before);
    drop(first);
}

#[test]
fn a_failed_replacement_leaves_the_old_bytes_as_the_durable_digest() {
    let root = tempfile::tempdir().expect("tempdir");
    let directory = root.path().join("store");
    fs::create_dir_all(&directory).expect("store directory");
    let path = directory.join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"first").expect("first write lands");
    let before = inode(&path);

    if !lock_directory(&directory) {
        return;
    }
    let failed = writer.write(b"second");
    // The file still holds the first bytes durably, so asking for them
    // again is already satisfied, and it retires the failed snapshot.
    let reverted = writer.write(b"first");
    set_mode(&directory, 0o700);

    assert!(failed.is_err(), "a read-only directory rejects the write");
    assert_eq!(
        reverted.expect("the durable bytes are already in place"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(writer.pending_failure(), None, "nothing is left pending");
    assert_eq!(
        writer
            .flush(Duration::from_secs(5))
            .expect("nothing waits on a retry"),
        PersistenceFlushOutcome::Clean
    );
    assert_eq!(inode(&path), before);
    assert_eq!(fs::read(&path).expect("read destination"), b"first");
}

#[test]
fn a_replacement_made_without_the_writer_is_not_mistaken_for_its_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"ours").expect("first write lands");

    // Another code path stages and renames its own file over the same path.
    let staged = root.path().join("staged.json");
    fs::write(&staged, b"theirs").expect("stage outside replacement");
    fs::rename(&staged, &path).expect("replace outside the writer");

    assert_eq!(
        writer.write(b"ours").expect("rewrite lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read destination"), b"ours");
}

#[test]
fn an_in_place_edit_is_not_mistaken_for_the_durable_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"ours").expect("first write lands");

    // A hand edit rewrites the same inode.
    fs::write(&path, b"edited by hand").expect("edit in place");

    assert_eq!(
        writer.write(b"ours").expect("rewrite lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read destination"), b"ours");
}

#[test]
fn a_destination_reports_whether_it_still_holds_its_durable_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    assert!(!writer.holds_durable_bytes(), "nothing written yet");

    writer.write(b"ours").expect("first write lands");
    assert!(writer.holds_durable_bytes());

    fs::write(&path, b"edited by hand").expect("edit in place");
    assert!(!writer.holds_durable_bytes());
}

#[test]
fn forgotten_durable_bytes_are_written_again() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"ours").expect("first write lands");
    let before = inode(&path);

    writer.forget_durable_bytes();
    assert!(!writer.holds_durable_bytes());
    assert_eq!(
        writer.write(b"ours").expect("rewrite lands"),
        AtomicWriteOutcome::Written
    );

    assert_ne!(inode(&path), before, "a forgotten destination is replaced");
    assert!(writer.holds_durable_bytes());
}
