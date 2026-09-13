#![cfg(unix)]

use std::fs;
use std::path::Path;

use hypercolor_platform_fs::ExclusiveDirectory;

#[test]
fn retained_relationship_distinguishes_equal_nested_and_sibling_directories() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    fs::create_dir_all(fixture.path().join("data/releases")).expect("release");
    fs::create_dir(fixture.path().join("state")).expect("state");
    let gate = ExclusiveDirectory::try_acquire(fixture.path(), Path::new("lock"))
        .expect("lock")
        .expect("exclusive");
    let data = gate
        .open_public_directory(&fixture.path().join("data"))
        .expect("data");
    let release = gate
        .open_public_directory(&fixture.path().join("data/releases"))
        .expect("release");
    let same = gate
        .open_public_directory(&fixture.path().join("data/releases"))
        .expect("same");
    let state = gate
        .open_public_directory(&fixture.path().join("state"))
        .expect("state");
    assert!(release.is_within(&data).expect("nested"));
    assert!(release.is_within(&same).expect("equal"));
    assert!(!data.is_within(&release).expect("reverse"));
    assert!(!state.is_within(&release).expect("sibling"));
    assert!(!release.is_within(&state).expect("sibling"));
}

#[test]
fn moved_ancestor_is_an_error_even_when_leaf_inode_survives() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let data_path = fixture.path().join("data");
    fs::create_dir_all(data_path.join("releases")).expect("release");
    let gate = ExclusiveDirectory::try_acquire(fixture.path(), Path::new("lock"))
        .expect("lock")
        .expect("exclusive");
    let data = gate.open_public_directory(&data_path).expect("data");
    let release = gate
        .open_public_directory(&data_path.join("releases"))
        .expect("release");
    fs::rename(&data_path, fixture.path().join("old")).expect("displace parent");
    fs::create_dir(&data_path).expect("replacement parent");
    fs::rename(
        fixture.path().join("old/releases"),
        data_path.join("releases"),
    )
    .expect("preserve leaf inode");
    assert!(release.is_within(&data).is_err());
    assert!(data.is_within(&release).is_err());
}

#[test]
fn separate_operation_gates_cannot_be_combined_into_one_authority() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    fs::create_dir(fixture.path().join("data")).expect("data");
    let left = ExclusiveDirectory::try_acquire(fixture.path(), Path::new("left.lock"))
        .expect("left")
        .expect("exclusive");
    let right = ExclusiveDirectory::try_acquire(fixture.path(), Path::new("right.lock"))
        .expect("right")
        .expect("exclusive");
    let left = left
        .open_public_directory(&fixture.path().join("data"))
        .expect("left data");
    let right = right
        .open_public_directory(&fixture.path().join("data"))
        .expect("right data");
    assert_eq!(
        left.is_within(&right).expect_err("foreign gate").kind(),
        std::io::ErrorKind::InvalidInput
    );
}
