//! The edge of the durability contract: a writer active during a call.
//!
//! A write's guarantee covers outside edits that completed before the call.
//! A writer that rewrites the destination while the call is in flight can
//! leave other bytes behind after the call reports success, as it can for
//! any rename-based write. What the contract does promise is that once that
//! writer has finished, nothing trusts the stale result: the next check or
//! save reads the file, finds the difference, and repairs it.
//!
//! The replacement observer runs between a write's rename and its directory
//! barrier, which gives this suite a deterministic seam inside the call. The
//! observer is process-wide, so the suite has a binary of its own.

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
fn a_rewrite_during_the_call_is_repaired_by_the_next_save() {
    assert!(set_replacement_observer(rewrite_in_place_once));
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.write(b"seed").expect("seed write lands");

    // Write A while something rewrites that same inode with B after the
    // rename and before the barrier. That writer is active during the call,
    // so the call is outside the contract and may report success.
    *REWRITE_ON_LANDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(fs::canonicalize(&path).expect("canonical destination"));
    writer.write(b"AAAA").expect("write lands");
    assert_eq!(fs::read(&path).expect("read destination"), b"BBBB");

    // That writer has finished, so from here the contract holds: nothing
    // trusts the stale result, and the next save of A really writes it.
    assert!(!writer.holds_durable_bytes());
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
