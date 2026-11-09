use std::fs;

use hypercolor_platform_fs::sync_directory;

fn temporary_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("platform-fs-sync-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory")
}

#[test]
fn syncing_a_directory_after_creating_an_entry_succeeds() {
    let directory = temporary_directory();
    fs::write(directory.path().join("entry.json"), b"{}").expect("write entry");

    sync_directory(directory.path()).expect("directory sync succeeds");
}

#[test]
fn syncing_a_missing_directory_reports_not_found() {
    let directory = temporary_directory();
    let missing = directory.path().join("missing");

    let error = sync_directory(&missing).expect_err("missing directory cannot sync");

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}
