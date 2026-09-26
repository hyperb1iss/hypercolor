use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;

use super::durable_replace_file_at_with;

#[test]
fn directory_sync_failure_does_not_restore_displaced_authority() {
    let fixture = tempfile::tempdir().expect("fixture");
    fs::write(fixture.path().join("locator"), b"legacy-v1").expect("legacy");
    fs::write(fixture.path().join("staged"), b"managed-v2").expect("prepared locator");
    let directory = File::open(fixture.path()).expect("directory");
    let result = durable_replace_file_at_with(
        &directory,
        OsStr::new("staged"),
        OsStr::new("locator"),
        |_| Err(io::Error::other("injected directory fsync failure")),
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(fixture.path().join("locator")).expect("visible authority"),
        b"managed-v2"
    );
    assert!(!fixture.path().join("staged").exists());
    directory.sync_all().expect("retry durability barrier");
    assert_eq!(
        fs::read(fixture.path().join("locator")).expect("durable authority"),
        b"managed-v2"
    );
}

#[test]
fn failed_rename_preserves_prior_authority_without_attempting_sync() {
    let fixture = tempfile::tempdir().expect("fixture");
    fs::write(fixture.path().join("locator"), b"legacy-v1").expect("legacy");
    let directory = File::open(fixture.path()).expect("directory");
    let result = durable_replace_file_at_with(
        &directory,
        OsStr::new("missing"),
        OsStr::new("locator"),
        |_| panic!("rename failure must precede fsync"),
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(fixture.path().join("locator")).expect("prior authority"),
        b"legacy-v1"
    );
}
