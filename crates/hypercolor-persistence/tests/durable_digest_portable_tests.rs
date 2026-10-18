//! The identical-bytes skip never trusts a file something else changed.
//!
//! Unlike the Unix-only digest tests, these run on every platform the
//! change stamp supports, Windows included.

use std::fs;

use hypercolor_persistence::{AtomicFileWriter, AtomicWriteOutcome};

#[test]
fn a_file_renamed_over_the_destination_is_written_over() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"ours").expect("first write lands");

    let staged = root.path().join("staged.json");
    fs::write(&staged, b"other").expect("stage outside replacement");
    fs::rename(&staged, &path).expect("replace outside the writer");
    assert!(!writer.holds_durable_bytes());

    assert_eq!(
        writer.write(b"ours").expect("save lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read destination"), b"ours");
    assert!(writer.holds_durable_bytes());
}

#[test]
fn a_same_length_rewrite_with_its_mtime_restored_is_written_over() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"original").expect("first write lands");
    let modified = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("landed mtime");

    fs::write(&path, b"restored").expect("overwrite in place");
    fs::File::options()
        .write(true)
        .open(&path)
        .expect("reopen destination")
        .set_modified(modified)
        .expect("restore mtime");

    assert_eq!(
        writer.write(b"original").expect("save lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read destination"), b"original");
}

#[test]
fn an_untouched_destination_still_skips_identical_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"stable").expect("first write lands");
    assert!(writer.holds_durable_bytes());

    assert_eq!(
        writer.write(b"stable").expect("identical save lands"),
        AtomicWriteOutcome::Written
    );
    assert!(writer.holds_durable_bytes());
}
