use std::fs;
use std::io::Read as _;

use hypercolor_platform_fs::{open_no_follow, write_secret};

#[test]
fn secret_write_creates_new_file_with_complete_contents() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("credential");

    write_secret(&path, b"private material").expect("write secret");

    assert_eq!(fs::read(path).expect("read secret"), b"private material");
}

#[test]
fn secret_write_refuses_to_replace_and_preserves_prior_content() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("credential");
    fs::write(&path, b"original").expect("write original secret");

    write_secret(&path, b"replacement").expect_err("existing secret must be refused");

    assert_eq!(fs::read(path).expect("read original secret"), b"original");
}

#[test]
fn no_follow_open_reads_regular_files() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("credential");
    fs::write(&path, b"private material").expect("write secret fixture");
    let mut contents = Vec::new();

    open_no_follow(&path)
        .expect("open regular file")
        .read_to_end(&mut contents)
        .expect("read regular file");

    assert_eq!(contents, b"private material");
}

#[cfg(unix)]
#[test]
fn secret_write_enforces_private_mode_and_current_owner() {
    use std::os::unix::fs::MetadataExt as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("credential");

    write_secret(&path, b"private material").expect("write secret");

    let metadata = fs::metadata(path).expect("inspect secret");
    let directory_metadata = fs::metadata(directory.path()).expect("inspect parent directory");
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), directory_metadata.uid());
}

#[cfg(unix)]
#[test]
fn no_follow_open_refuses_symlink_without_reading_its_target() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory");
    let target = directory.path().join("target");
    let link = directory.path().join("credential");
    fs::write(&target, b"must stay hidden").expect("write symlink target");
    symlink(&target, &link).expect("create symlink");

    open_no_follow(&link).expect_err("symlink must be refused");
}

#[cfg(unix)]
#[test]
fn secret_write_refuses_symlink_and_preserves_its_target() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory");
    let target = directory.path().join("target");
    let link = directory.path().join("credential");
    fs::write(&target, b"original").expect("write symlink target");
    symlink(&target, &link).expect("create symlink");

    write_secret(&link, b"replacement").expect_err("symlink must be refused");

    assert_eq!(fs::read(target).expect("read symlink target"), b"original");
}

#[cfg(target_os = "windows")]
#[test]
fn no_follow_open_refuses_windows_file_symlink() {
    use std::os::windows::fs::symlink_file;

    let directory = tempfile::tempdir().expect("temporary directory");
    let target = directory.path().join("target");
    let link = directory.path().join("credential");
    fs::write(&target, b"must stay hidden").expect("write symlink target");
    symlink_file(&target, &link).expect("create file symlink");

    open_no_follow(&link).expect_err("symlink must be refused");
}
