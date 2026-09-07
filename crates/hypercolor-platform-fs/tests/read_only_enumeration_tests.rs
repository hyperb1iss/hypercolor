#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use hypercolor_platform_fs::{
    MAX_PUBLIC_DIRECTORY_CHILD_COUNT, MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES,
    ReadOnlyDirectoryAuthority,
};

#[test]
fn read_only_child_names_are_sorted_and_bound_to_the_retained_directory() {
    let temporary = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let root = temporary.path().join("root");
    let detached = temporary.path().join("detached");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("z-last"), b"last").expect("write last file");
    fs::write(root.join("a-first"), b"first").expect("write first file");
    fs::create_dir(root.join("child")).expect("create child directory");
    let authority = ReadOnlyDirectoryAuthority::open(&root).expect("open read-only authority");

    fs::rename(&root, &detached).expect("detach retained directory");
    fs::create_dir(&root).expect("replace pathname directory");
    fs::write(root.join("attacker"), b"attacker").expect("write replacement entry");

    assert_eq!(
        authority
            .child_names()
            .expect("enumerate retained directory"),
        ["a-first", "child", "z-last"].map(OsString::from)
    );
    let opened = authority
        .open_regular_file(Path::new("a-first"))
        .expect("open retained regular file");
    assert_eq!(opened.metadata().size(), 5);
    let child = authority
        .open_child_directory(Path::new("child"))
        .expect("open retained child directory");
    assert!(
        child
            .child_names()
            .expect("enumerate retained child")
            .is_empty()
    );
}

#[test]
fn read_only_child_names_enforce_count_and_aggregate_byte_bounds() {
    let temporary = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let count_root = temporary.path().join("count");
    fs::create_dir(&count_root).expect("create count root");
    for index in 0..=MAX_PUBLIC_DIRECTORY_CHILD_COUNT {
        fs::write(count_root.join(format!("entry-{index:04}")), b"").expect("write count entry");
    }
    let count_authority =
        ReadOnlyDirectoryAuthority::open(&count_root).expect("open count authority");
    let count_error = count_authority
        .child_names()
        .expect_err("child count overflow must fail");
    assert_eq!(count_error.kind(), io::ErrorKind::InvalidData);

    let bytes_root = temporary.path().join("bytes");
    fs::create_dir(&bytes_root).expect("create bytes root");
    let name_length = 255;
    let names_required = MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES / name_length + 1;
    for index in 0..names_required {
        let prefix = format!("{index:04}-");
        let name = format!("{prefix}{}", "x".repeat(name_length - prefix.len()));
        fs::write(bytes_root.join(name), b"").expect("write aggregate-byte entry");
    }
    let bytes_authority =
        ReadOnlyDirectoryAuthority::open(&bytes_root).expect("open bytes authority");
    let bytes_error = bytes_authority
        .child_names()
        .expect_err("aggregate name-byte overflow must fail");
    assert_eq!(bytes_error.kind(), io::ErrorKind::InvalidData);
}

#[cfg(target_os = "linux")]
#[test]
fn read_only_child_names_preserve_non_utf8_names() {
    let temporary = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let raw_name = OsString::from_vec(vec![b'r', b'a', b'w', 0x80]);
    fs::write(temporary.path().join(&raw_name), b"raw").expect("write raw-name entry");
    let authority =
        ReadOnlyDirectoryAuthority::open(temporary.path()).expect("open read-only authority");

    let names = authority.child_names().expect("enumerate raw names");

    assert_eq!(names.len(), 1);
    assert_eq!(
        names[0].as_os_str().as_bytes(),
        raw_name.as_os_str().as_bytes()
    );
    assert_eq!(
        authority
            .open_regular_file(Path::new(&names[0]))
            .expect("open raw-name entry")
            .metadata()
            .size(),
        3
    );
}
