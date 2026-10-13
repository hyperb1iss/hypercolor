//! A rewrite that lands between a write's rename and its durability
//! barrier is never certified as the write's bytes.
//!
//! The replacement observer runs inside that window, which gives this test
//! a deterministic seam. The observer is process-wide, so this suite has a
//! binary of its own.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hypercolor_persistence::{AtomicFileWriter, AtomicWriteOutcome, set_replacement_observer};

/// The destination the observer rewrites once, in place, when it lands.
static REWRITE_ON_LANDING: Mutex<Option<PathBuf>> = Mutex::new(None);

fn rewrite_in_place_once(path: &Path) {
    let mut target = REWRITE_ON_LANDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if target.as_deref() == Some(path) {
        *target = None;
        fs::write(path, b"BBBB").expect("rewrite the landed inode in place");
    }
}

#[test]
fn a_rewrite_inside_the_landing_window_is_not_certified() {
    assert!(set_replacement_observer(rewrite_in_place_once));
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"seed").expect("seed write lands");

    // Write A; after its rename and before its barrier, something rewrites
    // that same inode with B.
    *REWRITE_ON_LANDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(fs::canonicalize(&path).expect("canonical destination"));
    writer.write(b"AAAA").expect("write lands");
    assert_eq!(fs::read(&path).expect("read destination"), b"BBBB");
    assert!(!writer.holds_durable_bytes());

    // Saving A again must really write it.
    let before = fs::metadata(&path).expect("metadata").ino();
    assert_eq!(
        writer.write(b"AAAA").expect("save lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read destination"), b"AAAA");
    assert_ne!(
        fs::metadata(&path).expect("metadata").ino(),
        before,
        "the save replaced the file rather than trusting it"
    );
    assert!(writer.holds_durable_bytes());
}
