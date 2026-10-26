#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{
    WindowsDirectory, WindowsDirectoryAccess, WindowsEntryKind, WindowsGrant, WindowsSid,
};

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary directory");
    // Runners hand out 8.3 short temp paths; the authority wants the real one.
    let canonical = fs::canonicalize(directory.path()).expect("canonical temporary directory");
    (directory, canonical)
}

fn open_write(path: &Path) -> WindowsDirectory {
    WindowsDirectory::open(path, WindowsDirectoryAccess::Write).expect("open directory for write")
}

#[test]
fn children_are_created_listed_inspected_and_removed_through_the_handle() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);

    let child = directory
        .create_child_directory(OsStr::new("units"))
        .expect("create child directory");
    assert_eq!(child.path(), root.join("units"));
    let mut file = directory
        .create_file(OsStr::new("journal.json"))
        .expect("create file");
    file.write_all(b"{}").expect("write file");
    file.sync_all().expect("sync file");
    drop(file);

    let mut names = directory.entries().expect("entries");
    names.sort();
    assert_eq!(names, ["journal.json", "units"]);
    let file_metadata = directory
        .entry_metadata(OsStr::new("journal.json"))
        .expect("inspect file")
        .expect("file exists");
    assert_eq!(file_metadata.kind(), WindowsEntryKind::File);
    assert_eq!(file_metadata.size(), 2);
    assert_eq!(
        directory
            .entry_metadata(OsStr::new("units"))
            .expect("inspect directory")
            .expect("directory exists")
            .kind(),
        WindowsEntryKind::Directory
    );
    assert!(
        directory
            .entry_metadata(OsStr::new("missing"))
            .expect("inspect missing")
            .is_none()
    );

    let mut contents = String::new();
    directory
        .open_file(OsStr::new("journal.json"))
        .expect("open file")
        .read_to_string(&mut contents)
        .expect("read file");
    assert_eq!(contents, "{}");
    assert!(directory.open_file(OsStr::new("units")).is_err());

    child
        .create_file(OsStr::new("member"))
        .expect("create nested file");
    child
        .create_child_directory(OsStr::new("nested"))
        .expect("create nested directory");
    drop(child);
    directory
        .remove_tree(OsStr::new("units"))
        .expect("remove tree");
    directory
        .remove_file(OsStr::new("journal.json"))
        .expect("remove file");
    assert!(directory.entries().expect("entries").is_empty());
}

#[test]
fn create_refuses_a_taken_name() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);
    directory.create_file(OsStr::new("taken")).expect("create");
    let error = directory
        .create_file(OsStr::new("taken"))
        .expect_err("second create fails");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    let error = directory
        .create_child_directory(OsStr::new("taken"))
        .expect_err("a directory over a file fails");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
}

#[test]
fn rename_replaces_only_when_asked() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);
    for (name, bytes) in [("staged", b"new".as_slice()), ("current", b"old")] {
        let mut file = directory.create_file(OsStr::new(name)).expect("create");
        file.write_all(bytes).expect("write");
        file.sync_all().expect("sync");
    }

    let error = directory
        .rename(OsStr::new("staged"), OsStr::new("current"), false)
        .expect_err("a taken destination refuses a plain rename");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");

    directory
        .rename(OsStr::new("staged"), OsStr::new("current"), true)
        .expect("replace");
    assert_eq!(fs::read(root.join("current")).expect("read"), b"new");
    assert!(!root.join("staged").exists());

    directory
        .create_child_directory(OsStr::new("staging"))
        .expect("create staging");
    directory
        .rename(OsStr::new("staging"), OsStr::new("unit"), false)
        .expect("publish a directory");
    assert!(root.join("unit").is_dir());
}

#[test]
fn an_exclusive_lock_is_refused_while_held() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);
    let held = directory
        .try_lock_file(OsStr::new("install.lock"))
        .expect("lock")
        .expect("first lock succeeds");
    assert!(
        directory
            .try_lock_file(OsStr::new("install.lock"))
            .expect("contended lock")
            .is_none()
    );
    assert!(
        directory.remove_file(OsStr::new("install.lock")).is_err(),
        "a held lock file cannot be unlinked from under its holder"
    );
    assert!(
        directory
            .rename(OsStr::new("install.lock"), OsStr::new("moved.lock"), false)
            .is_err(),
        "a held lock file cannot be renamed from under its holder"
    );
    drop(held);
    assert!(
        directory
            .try_lock_file(OsStr::new("install.lock"))
            .expect("relock")
            .is_some()
    );
}

#[test]
fn a_read_only_authority_refuses_every_mutation() {
    let (_guard, root) = scratch();
    let writable = open_write(&root);
    writable.create_file(OsStr::new("present")).expect("create");
    let directory = writable.read_only().expect("reopen read-only");
    assert!(directory.create_file(OsStr::new("new")).is_err());
    assert!(directory.create_child_directory(OsStr::new("new")).is_err());
    assert!(directory.remove_file(OsStr::new("present")).is_err());
    assert!(
        directory
            .rename(OsStr::new("present"), OsStr::new("moved"), false)
            .is_err()
    );
    assert!(directory.try_lock_file(OsStr::new("lock")).is_err());
    assert!(directory.sync().is_err());
    assert!(
        directory
            .metadata()
            .expect("metadata")
            .same_entry(writable.metadata().expect("metadata"))
    );
}

#[test]
fn names_that_win32_reads_differently_are_refused() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);
    for name in [
        "",
        ".",
        "..",
        "a\\b",
        "a/b",
        "a:stream",
        "trailing.",
        "trailing ",
        "CON",
        "con.txt",
        "LPT1",
        "star*",
        "quote\"",
    ] {
        assert!(
            directory.create_file(OsStr::new(name)).is_err(),
            "{name:?} must be refused"
        );
    }
    assert!(WindowsDirectory::open(Path::new("relative"), WindowsDirectoryAccess::Read).is_err());
    assert!(
        WindowsDirectory::open(&root.join("..").join("x"), WindowsDirectoryAccess::Read).is_err()
    );
}

#[test]
fn reparse_points_are_refused_at_the_path_and_inside() {
    let (_guard, root) = scratch();
    let target = root.join("target");
    fs::create_dir(&target).expect("target");
    let link = root.join("link");
    if let Err(error) = std::os::windows::fs::symlink_dir(&target, &link) {
        // Creating symlinks needs Developer Mode or elevation.
        eprintln!("skipping reparse refusal: {error}");
        return;
    }
    assert!(WindowsDirectory::open(&link, WindowsDirectoryAccess::Read).is_err());
    assert!(
        WindowsDirectory::open(&link.join("below"), WindowsDirectoryAccess::Read).is_err(),
        "a reparse point above the path is refused too"
    );
    let directory = open_write(&root);
    assert_eq!(
        directory
            .entry_metadata(OsStr::new("link"))
            .expect("inspect link")
            .expect("link exists")
            .kind(),
        WindowsEntryKind::ReparsePoint
    );
    assert!(
        directory
            .open_child_directory(OsStr::new("link"), WindowsDirectoryAccess::Read)
            .is_err()
    );

    fs::write(target.join("kept"), b"kept").expect("target file");
    let tree = directory
        .create_child_directory(OsStr::new("tree"))
        .expect("tree");
    std::os::windows::fs::symlink_dir(&target, root.join("tree").join("escape"))
        .expect("nested link");
    drop(tree);
    directory
        .remove_tree(OsStr::new("tree"))
        .expect("remove a tree holding a link");
    assert_eq!(
        fs::read(target.join("kept")).expect("link target survives"),
        b"kept"
    );
}

#[test]
fn the_owner_and_grants_of_a_directory_are_readable() {
    let (_guard, root) = scratch();
    let directory = open_write(&root);
    let security = directory.security().expect("security");
    assert!(!security.null_dacl);
    assert!(
        security.grants.iter().any(WindowsGrant::writes),
        "a scratch directory its creator can write grants someone write"
    );
    let user = WindowsSid::current_user().expect("current user");
    assert!(user.to_string_form().starts_with("S-1-5-"));
    directory.create_file(OsStr::new("file")).expect("create");
    assert_eq!(
        directory
            .entry_security(OsStr::new("file"))
            .expect("file security")
            .owner,
        security.owner,
        "a new file takes the creator's owner"
    );
}

#[test]
fn grant_classes_follow_the_access_mask() {
    let sid = WindowsSid::parse("S-1-5-32-545").expect("users");
    let grant = |mask| WindowsGrant {
        sid: sid.clone(),
        mask,
    };
    // Read and execute.
    assert!(!grant(0x0012_00A9).writes());
    assert!(grant(0x0012_00A9).reads_and_executes());
    // Create files and folders only, as ProgramData grants users.
    assert!(grant(0x0000_0006).writes());
    assert!(!grant(0x0000_0006).replaces());
    // Delete, delete child, write DAC, write owner, generic all.
    for mask in [
        0x0001_0000,
        0x0000_0040,
        0x0004_0000,
        0x0008_0000,
        0x1000_0000,
    ] {
        assert!(grant(mask).replaces(), "{mask:#x}");
        assert!(grant(mask).writes(), "{mask:#x}");
    }
}

#[test]
fn sid_strings_round_trip() {
    for text in [
        "S-1-5-18",
        "S-1-5-32-544",
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
    ] {
        assert_eq!(
            WindowsSid::parse(text).expect("parse").to_string_form(),
            text
        );
    }
    for text in ["", "S-1", "S-2-5-18", "S-1-x-18", "S-1-5"] {
        assert!(WindowsSid::parse(text).is_err(), "{text:?}");
    }
}
