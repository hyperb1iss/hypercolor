use std::fs;

use hypercolor_platform_fs::replace_file;

#[test]
fn replacement_overwrites_destination_and_consumes_source() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.tmp");
    let destination = directory.path().join("state.json");
    fs::write(&source, b"new").expect("write source");
    fs::write(&destination, b"old").expect("write destination");

    replace_file(&source, &destination).expect("replace destination");

    assert_eq!(fs::read(&destination).expect("read destination"), b"new");
    assert!(!source.exists());
}

#[test]
fn failed_replacement_preserves_existing_destination() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("missing.tmp");
    let destination = directory.path().join("state.json");
    fs::write(&destination, b"old").expect("write destination");

    replace_file(&source, &destination).expect_err("missing source must fail");

    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
}
