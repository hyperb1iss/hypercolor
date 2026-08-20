#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Cursor, Read as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::Path;

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, ExclusiveDirectory, MAX_PUBLIC_DIRECTORY_CHILD_COUNT,
    MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES, ReadOnlyDirectoryAuthority,
};

fn acquire(directory: &Path) -> DirectoryAuthority {
    let exclusive = ExclusiveDirectory::try_acquire(directory, Path::new("install.lock"))
        .expect("acquire exclusive directory")
        .expect("directory authority is uncontended");
    exclusive
        .root_directory()
        .expect("open root directory authority")
}

fn write_file(directory: &DirectoryAuthority, name: &str, mode: u32, contents: &[u8]) {
    let expected_size = u64::try_from(contents.len()).expect("test payload size fits u64");
    directory
        .create_regular_file(
            Path::new(name),
            mode,
            expected_size,
            &mut Cursor::new(contents),
        )
        .expect("create regular file");
}

#[test]
fn authority_stays_bound_after_parent_path_replacement() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let current = parent.path().join("current");
    let original = parent.path().join("original");
    fs::create_dir(&current).expect("create governed root");
    let root = acquire(&current);

    fs::rename(&current, &original).expect("rename governed root");
    fs::create_dir(&current).expect("create pathname replacement");
    let unit = root
        .create_child_directory(Path::new("unit"))
        .expect("create child through retained authority");
    write_file(&unit, "hypercolor", 0o755, b"verified");

    assert_eq!(
        fs::read(original.join("unit/hypercolor")).expect("read governed incarnation"),
        b"verified"
    );
    assert!(!current.join("unit").exists());
}

#[test]
fn read_only_conversion_retains_exact_child_after_root_replacement() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let current = parent.path().join("current");
    let original = parent.path().join("original");
    fs::create_dir(&current).expect("create governed root");
    let root = acquire(&current);
    let unit = root
        .create_child_directory(Path::new("unit"))
        .expect("create unit");
    write_file(&unit, "hypercolor", 0o555, b"verified");
    let retained = unit.read_only().expect("retain read-only unit authority");
    drop(unit);
    drop(root);

    fs::rename(&current, &original).expect("rename governed root");
    fs::create_dir(&current).expect("create pathname replacement");
    fs::write(current.join("hypercolor"), b"attacker").expect("write replacement file");

    let mut opened = retained
        .open_regular_file(Path::new("hypercolor"))
        .expect("open retained file");
    let mut contents = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut contents)
        .expect("read retained file");
    assert_eq!(contents, b"verified");
}

#[test]
fn root_authority_keeps_exclusive_lock_alive() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());

    assert!(
        ExclusiveDirectory::try_acquire(directory.path(), Path::new("install.lock"))
            .expect("probe retained lock")
            .is_none()
    );

    drop(root);
    assert!(
        ExclusiveDirectory::try_acquire(directory.path(), Path::new("install.lock"))
            .expect("reacquire released lock")
            .is_some()
    );
}

#[test]
fn read_only_authority_stays_bound_without_creating_a_lock_entry() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let source = parent.path().join("source");
    let displaced = parent.path().join("displaced");
    fs::create_dir(&source).expect("create source root");
    fs::create_dir(source.join("bin")).expect("create source bin");
    fs::write(source.join("bin/hypercolor"), b"verified").expect("write source binary");
    fs::set_permissions(
        source.join("bin/hypercolor"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("set source binary mode");

    let root = ReadOnlyDirectoryAuthority::open(&source).expect("open read-only authority");
    assert!(!source.join("install.lock").exists());
    fs::rename(&source, &displaced).expect("displace source pathname");
    fs::create_dir(&source).expect("replace source pathname");
    fs::write(source.join("attacker"), b"replacement").expect("write replacement entry");

    let bin = root
        .open_child_directory(Path::new("bin"))
        .expect("open retained bin");
    assert_eq!(
        bin.entries().expect("enumerate retained bin"),
        ["hypercolor"]
    );
    let mut binary = bin
        .open_regular_file(Path::new("hypercolor"))
        .expect("open retained binary");
    let mut contents = Vec::new();
    binary
        .file_mut()
        .read_to_end(&mut contents)
        .expect("read retained binary");
    assert_eq!(contents, b"verified");
    assert_eq!(root.entries().expect("enumerate retained root"), ["bin"]);
    assert!(
        !root
            .entries()
            .expect("enumerate again")
            .contains(&OsString::from("attacker"))
    );
}

#[test]
fn enumeration_and_metadata_are_handle_relative_and_exact() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let child = root
        .create_child_directory(Path::new("payload"))
        .expect("create payload directory");
    write_file(&child, "z-last", 0o640, b"last");
    write_file(&child, "a-first", 0o755, b"first");

    assert_eq!(
        child.entries().expect("enumerate payload"),
        vec![OsString::from("a-first"), OsString::from("z-last")]
    );
    let metadata = child
        .entry_metadata(Path::new("a-first"))
        .expect("inspect entry")
        .expect("entry exists");
    assert_eq!(metadata.kind(), DirectoryEntryKind::RegularFile);
    assert_eq!(metadata.mode(), 0o755);
    assert_eq!(metadata.size(), 5);
    assert_eq!(metadata.link_count(), 1);
    assert_ne!(metadata.device(), 0);
    assert_ne!(metadata.inode(), 0);
    assert!(
        child
            .entry_metadata(Path::new("missing"))
            .expect("inspect missing entry")
            .is_none()
    );

    let mut opened = child
        .open_regular_file(Path::new("a-first"))
        .expect("open exact regular file");
    assert_eq!(opened.metadata(), metadata);
    let mut contents = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut contents)
        .expect("read opened file");
    assert_eq!(contents, b"first");
    child.set_mode(0o550).expect("set exact directory mode");
    child.sync().expect("sync directory");
    assert_eq!(
        root.entry_metadata(Path::new("payload"))
            .expect("inspect payload")
            .expect("payload exists")
            .mode(),
        0o550
    );
}

#[test]
fn bounded_child_names_are_sorted_retained_and_openable() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let current = parent.path().join("current");
    let detached = parent.path().join("detached");
    fs::create_dir(&current).expect("create governed root");
    let root = acquire(&current);
    let payload = root
        .create_child_directory(Path::new("payload"))
        .expect("create payload directory");
    write_file(&payload, "z-last", 0o600, b"last");
    write_file(&payload, "a-first", 0o600, b"first");
    payload
        .create_child_directory(Path::new("m-directory"))
        .expect("create nested directory");

    fs::rename(&current, &detached).expect("detach governed root");
    fs::create_dir(&current).expect("replace governed pathname");
    fs::write(current.join("attacker"), b"attacker").expect("write replacement entry");

    let names = payload.child_names().expect("enumerate retained payload");
    assert_eq!(
        names,
        ["a-first", "m-directory", "z-last"]
            .map(OsString::from)
            .to_vec()
    );
    let mut opened = payload
        .open_regular_file(Path::new(&names[0]))
        .expect("open enumerated regular file");
    let mut contents = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut contents)
        .expect("read enumerated regular file");
    assert_eq!(contents, b"first");
    payload
        .open_child_directory(Path::new(&names[1]))
        .expect("open enumerated child directory");
    assert!(!names.contains(&OsString::from("attacker")));
}

#[test]
fn bounded_child_names_enforce_count_and_aggregate_byte_limits() {
    let count_directory = tempfile::tempdir().expect("temporary count directory");
    let count_root = acquire(count_directory.path());
    let count_payload = count_root
        .create_child_directory(Path::new("payload"))
        .expect("create count payload");
    for index in 0..=MAX_PUBLIC_DIRECTORY_CHILD_COUNT {
        fs::write(
            count_directory
                .path()
                .join(format!("payload/entry-{index:04}")),
            b"entry",
        )
        .expect("write count entry");
    }
    count_payload
        .child_names()
        .expect_err("child count above the bound must fail");
    assert!(count_directory.path().join("payload/entry-0000").is_file());

    let bytes_directory = tempfile::tempdir().expect("temporary byte directory");
    let bytes_root = acquire(bytes_directory.path());
    let bytes_payload = bytes_root
        .create_child_directory(Path::new("payload"))
        .expect("create byte payload");
    let name_len = 255;
    let entry_count = MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES / name_len + 1;
    assert!(entry_count < MAX_PUBLIC_DIRECTORY_CHILD_COUNT);
    for index in 0..entry_count {
        let mut name = format!("{index:04}").into_bytes();
        name.resize(name_len, b'x');
        fs::write(
            bytes_directory
                .path()
                .join("payload")
                .join(OsString::from_vec(name)),
            b"entry",
        )
        .expect("write byte-bound entry");
    }
    bytes_payload
        .child_names()
        .expect_err("aggregate child name bytes above the bound must fail");
    assert_eq!(
        fs::read_dir(bytes_directory.path().join("payload"))
            .expect("read unchanged byte-bound payload")
            .count(),
        entry_count
    );
}

#[test]
#[cfg(target_os = "linux")]
fn bounded_child_names_preserve_non_utf8_names() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let payload = root
        .create_child_directory(Path::new("payload"))
        .expect("create payload directory");
    let raw_name = OsString::from_vec(vec![b'n', 0xff]);
    fs::write(directory.path().join("payload").join(&raw_name), b"raw")
        .expect("write raw-name entry");

    assert_eq!(
        payload.child_names().expect("enumerate raw child name"),
        [raw_name]
    );
}

#[test]
fn opened_file_metadata_remains_bound_when_the_entry_name_changes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    write_file(&root, "selected", 0o600, b"selected-inode");
    write_file(&root, "replacement", 0o600, b"replacement-inode");
    let mut opened = root
        .open_regular_file(Path::new("selected"))
        .expect("open selected inode");
    let opened_metadata = opened.metadata();

    fs::rename(
        directory.path().join("selected"),
        directory.path().join("displaced"),
    )
    .expect("displace selected name");
    fs::rename(
        directory.path().join("replacement"),
        directory.path().join("selected"),
    )
    .expect("replace selected name");

    let replacement_metadata = root
        .entry_metadata(Path::new("selected"))
        .expect("inspect replacement name")
        .expect("replacement exists");
    assert_ne!(opened_metadata.inode(), replacement_metadata.inode());
    let mut contents = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut contents)
        .expect("read retained inode");
    assert_eq!(contents, b"selected-inode");
    assert_eq!(
        opened.into_file().metadata().expect("fstat retained").len(),
        14
    );
}

#[test]
fn special_permission_bits_are_preserved_and_rejected() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    fs::write(directory.path().join("setuid-file"), b"unsafe").expect("write unsafe file");
    fs::set_permissions(
        directory.path().join("setuid-file"),
        fs::Permissions::from_mode(0o4755),
    )
    .expect("set setuid bit");
    fs::create_dir(directory.path().join("setgid-dir")).expect("create unsafe directory");
    fs::set_permissions(
        directory.path().join("setgid-dir"),
        fs::Permissions::from_mode(0o2755),
    )
    .expect("set setgid bit");

    assert_eq!(
        root.entry_metadata(Path::new("setuid-file"))
            .expect("inspect unsafe file")
            .expect("unsafe file exists")
            .mode(),
        0o4755
    );
    assert_eq!(
        root.entry_metadata(Path::new("setgid-dir"))
            .expect("inspect unsafe directory")
            .expect("unsafe directory exists")
            .mode(),
        0o2755
    );
    root.open_regular_file(Path::new("setuid-file"))
        .expect_err("setuid file must be rejected");
    root.open_child_directory(Path::new("setgid-dir"))
        .expect_err("setgid directory must be rejected");
}

#[test]
fn create_new_files_reject_existing_entries_and_unsafe_modes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    write_file(&root, "payload", 0o600, b"original");

    root.create_regular_file(
        Path::new("payload"),
        0o600,
        11,
        &mut Cursor::new(b"replacement"),
    )
    .expect_err("existing file must not be replaced");
    root.create_regular_file(Path::new("unsafe"), 0o1755, 6, &mut Cursor::new(b"unsafe"))
        .expect_err("special mode bits must be rejected");
    root.set_mode(0o1700)
        .expect_err("directory special mode bits must be rejected");

    assert_eq!(
        fs::read(directory.path().join("payload")).expect("read preserved file"),
        b"original"
    );
    assert!(!directory.path().join("unsafe").exists());
}

#[test]
fn file_creation_copies_exact_expected_size_and_requires_eof() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());

    root.create_regular_file(Path::new("short"), 0o600, 4, &mut Cursor::new(b"abc"))
        .expect_err("short source must be rejected");
    root.create_regular_file(Path::new("long"), 0o600, 3, &mut Cursor::new(b"abcd"))
        .expect_err("long source must be rejected");
    root.create_regular_file(Path::new("zero-long"), 0o600, 0, &mut Cursor::new(b"x"))
        .expect_err("nonempty zero-sized source must be rejected");
    let metadata = root
        .create_regular_file(Path::new("exact"), 0o640, 4, &mut Cursor::new(b"data"))
        .expect("exact source must be copied");

    assert_eq!(metadata.size(), 4);
    assert_eq!(metadata.mode(), 0o640);
    assert!(!directory.path().join("short").exists());
    assert!(!directory.path().join("long").exists());
    assert!(!directory.path().join("zero-long").exists());
    assert_eq!(
        fs::read(directory.path().join("exact")).expect("read exact file"),
        b"data"
    );
}

#[test]
fn symlinks_are_rejected_at_root_child_cleanup_and_publication_boundaries() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let governed = parent.path().join("governed");
    let outside = parent.path().join("outside");
    fs::create_dir(&governed).expect("create governed root");
    fs::create_dir(&outside).expect("create outside directory");
    fs::write(outside.join("sentinel"), b"outside").expect("write outside sentinel");
    std::os::unix::fs::symlink(&governed, parent.path().join("root-link"))
        .expect("create root symlink");

    ExclusiveDirectory::try_acquire(&parent.path().join("root-link"), Path::new("install.lock"))
        .expect_err("root symlink must be rejected");

    let root = acquire(&governed);
    std::os::unix::fs::symlink(&outside, governed.join("child-link"))
        .expect("create child directory symlink");
    root.open_child_directory(Path::new("child-link"))
        .expect_err("child directory symlink must be rejected");
    root.open_regular_file(Path::new("child-link"))
        .expect_err("file symlink must be rejected");
    root.create_private_staging_directory(Path::new(".hypercolor-stage-link"))
        .expect("create private staging directory");
    std::os::unix::fs::symlink(
        &outside,
        governed.join(".hypercolor-stage-link/deeper-link"),
    )
    .expect("create staging symlink");
    let unsafe_staging = root
        .open_child_directory(Path::new(".hypercolor-stage-link"))
        .expect("open staging for nested inspection");
    unsafe_staging
        .open_child_directory(Path::new("deeper-link"))
        .expect_err("nested staging symlink must be rejected");
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-publish"))
        .expect("create legitimate staging capability");
    std::os::unix::fs::symlink(&outside, governed.join("published-link"))
        .expect("create publication destination symlink");
    staging
        .publish(Path::new("published-link"))
        .expect_err("publication destination symlink must be preserved");

    let cleanup = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-cleanup"))
        .expect("create cleanup staging capability");
    std::os::unix::fs::symlink(
        &outside,
        governed.join(".hypercolor-stage-cleanup/deeper-link"),
    )
    .expect("create cleanup symlink");
    cleanup
        .remove()
        .expect_err("recursive cleanup must reject nested symlinks");

    assert_eq!(
        fs::read(outside.join("sentinel")).expect("read outside sentinel"),
        b"outside"
    );
    assert!(governed.join("child-link").is_symlink());
    assert!(governed.join("published-link").is_symlink());
    assert!(governed.join(".hypercolor-stage-publish").is_dir());
    assert!(
        governed
            .join(".hypercolor-stage-cleanup/deeper-link")
            .is_symlink()
    );
}

#[test]
fn metadata_and_regular_open_reject_hardlinks_and_special_files() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    fs::write(directory.path().join("original"), b"linked").expect("write original file");
    fs::hard_link(
        directory.path().join("original"),
        directory.path().join("hardlink"),
    )
    .expect("create hardlink");
    let _listener = UnixListener::bind(directory.path().join("socket")).expect("create socket");

    let hardlink = root
        .entry_metadata(Path::new("hardlink"))
        .expect("inspect hardlink")
        .expect("hardlink exists");
    assert_eq!(hardlink.kind(), DirectoryEntryKind::RegularFile);
    assert_eq!(hardlink.link_count(), 2);
    root.open_regular_file(Path::new("hardlink"))
        .expect_err("multiply linked file must be rejected");

    let socket = root
        .entry_metadata(Path::new("socket"))
        .expect("inspect socket")
        .expect("socket exists");
    assert_eq!(socket.kind(), DirectoryEntryKind::Special);
    root.open_regular_file(Path::new("socket"))
        .expect_err("special file must be rejected");
}

#[test]
fn recursive_cleanup_is_contained_and_durable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let outside = tempfile::tempdir().expect("outside directory");
    fs::write(outside.path().join("sentinel"), b"outside").expect("write outside sentinel");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-cleanup"))
        .expect("create staged directory");
    let bin = staging
        .directory()
        .create_child_directory(Path::new("bin"))
        .expect("create bin directory");
    write_file(&bin, "hypercolor", 0o755, b"binary");
    let share = staging
        .directory()
        .create_child_directory(Path::new("share"))
        .expect("create share directory");
    write_file(&share, "manifest-copy", 0o444, b"manifest");
    bin.set_mode(0o555).expect("finalize bin directory");
    share.set_mode(0o555).expect("finalize share directory");
    staging
        .directory()
        .set_mode(0o555)
        .expect("finalize staging root");

    staging
        .remove()
        .expect("remove finalized private staging tree");
    assert!(!directory.path().join(".hypercolor-stage-cleanup").exists());
    assert_eq!(
        fs::read(outside.path().join("sentinel")).expect("read outside sentinel"),
        b"outside"
    );
}

#[test]
fn cleanup_rejects_a_staging_source_name_swap() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-cleanup"))
        .expect("create cleanup staging");
    write_file(staging.directory(), "marker", 0o600, b"expected");
    fs::rename(
        directory.path().join(".hypercolor-stage-cleanup"),
        directory.path().join("displaced-staging"),
    )
    .expect("displace held staging directory");
    fs::create_dir(directory.path().join(".hypercolor-stage-cleanup"))
        .expect("create cleanup-name replacement");
    fs::write(
        directory.path().join(".hypercolor-stage-cleanup/marker"),
        b"replacement",
    )
    .expect("write cleanup-name replacement");

    staging
        .remove()
        .expect_err("cleanup source-name swap must be rejected");

    assert_eq!(
        fs::read(directory.path().join("displaced-staging/marker"))
            .expect("read held staging directory"),
        b"expected"
    );
    assert_eq!(
        fs::read(directory.path().join(".hypercolor-stage-cleanup/marker"))
            .expect("read cleanup-name replacement"),
        b"replacement"
    );
}

#[test]
fn no_replace_publication_preserves_existing_destination() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-candidate"))
        .expect("create staged directory");
    write_file(staging.directory(), "marker", 0o444, b"candidate");
    let existing = root
        .create_child_directory(Path::new("unit"))
        .expect("create existing unit");
    write_file(&existing, "marker", 0o444, b"existing");

    staging
        .publish(Path::new("unit"))
        .expect_err("existing destination must not be replaced");

    assert_eq!(
        fs::read(directory.path().join("unit/marker")).expect("read existing unit"),
        b"existing"
    );
    assert_eq!(
        fs::read(directory.path().join(".hypercolor-stage-candidate/marker"))
            .expect("read staged unit"),
        b"candidate"
    );
}

#[test]
fn no_replace_publish_or_remove_preserves_destination_and_cleans_staging() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-candidate"))
        .expect("create staged directory");
    write_file(staging.directory(), "marker", 0o444, b"candidate");
    let existing = root
        .create_child_directory(Path::new("unit"))
        .expect("create existing unit");
    write_file(&existing, "marker", 0o444, b"existing");

    staging
        .publish_or_remove(Path::new("unit"))
        .expect_err("existing destination must not be replaced");

    assert_eq!(
        fs::read(directory.path().join("unit/marker")).expect("read existing unit"),
        b"existing"
    );
    assert!(
        !directory
            .path()
            .join(".hypercolor-stage-candidate")
            .exists()
    );
}

#[test]
fn publication_moves_complete_directory_and_consumes_staging_name() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-candidate"))
        .expect("create staged directory");
    write_file(staging.directory(), "marker", 0o444, b"candidate");
    staging
        .directory()
        .set_mode(0o555)
        .expect("finalize staged mode");
    let staged_identity = staging.directory().metadata().expect("inspect staging");

    let published = staging
        .publish(Path::new("unit"))
        .expect("publish staged directory");

    assert!(
        !directory
            .path()
            .join(".hypercolor-stage-candidate")
            .exists()
    );
    assert_eq!(
        published.metadata().expect("inspect published"),
        staged_identity
    );
    assert_eq!(
        fs::read(directory.path().join("unit/marker")).expect("read published unit"),
        b"candidate"
    );
    assert_eq!(
        root.entry_metadata(Path::new("unit"))
            .expect("inspect published unit")
            .expect("published unit exists")
            .mode(),
        0o555
    );
}

#[test]
fn publication_rejects_a_staging_source_name_swap() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-candidate"))
        .expect("create staged directory");
    write_file(staging.directory(), "marker", 0o444, b"expected");
    fs::rename(
        directory.path().join(".hypercolor-stage-candidate"),
        directory.path().join("displaced-staging"),
    )
    .expect("displace held staging directory");
    fs::create_dir(directory.path().join(".hypercolor-stage-candidate"))
        .expect("create source-name replacement");
    fs::write(
        directory.path().join(".hypercolor-stage-candidate/marker"),
        b"replacement",
    )
    .expect("write source-name replacement");

    staging
        .publish(Path::new("unit"))
        .expect_err("source-name swap must be rejected");

    assert!(!directory.path().join("unit").exists());
    assert_eq!(
        fs::read(directory.path().join("displaced-staging/marker"))
            .expect("read held staging directory"),
        b"expected"
    );
    assert_eq!(
        fs::read(directory.path().join(".hypercolor-stage-candidate/marker"))
            .expect("read source-name replacement"),
        b"replacement"
    );
}

#[test]
fn staging_cleanup_capability_cannot_target_a_published_unit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let candidate = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-candidate"))
        .expect("create candidate staging");
    write_file(candidate.directory(), "marker", 0o444, b"immutable");
    let published = candidate
        .publish(Path::new("unit"))
        .expect("publish immutable unit");
    let cleanup = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-cleanup"))
        .expect("create separate cleanup staging");
    write_file(cleanup.directory(), "marker", 0o600, b"temporary");

    cleanup.remove().expect("remove only private staging");

    assert_eq!(
        fs::read(directory.path().join("unit/marker")).expect("read immutable unit"),
        b"immutable"
    );
    assert_eq!(
        published.metadata().expect("inspect immutable unit").kind(),
        DirectoryEntryKind::Directory
    );
    assert!(!directory.path().join(".hypercolor-stage-cleanup").exists());
}

#[test]
fn mutation_names_are_single_components_and_lock_entry_is_protected() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let invalid = ["", ".", "..", "child/name", "/absolute"];

    for name in invalid {
        root.create_child_directory(Path::new(name))
            .expect_err("unsafe directory name must be rejected");
        root.create_regular_file(Path::new(name), 0o600, 4, &mut Cursor::new(b"data"))
            .expect_err("unsafe file name must be rejected");
        root.entry_metadata(Path::new(name))
            .expect_err("unsafe metadata name must be rejected");
    }
    root.create_regular_file(
        Path::new("install.lock"),
        0o600,
        11,
        &mut Cursor::new(b"replacement"),
    )
    .expect_err("held lock creation must be rejected");
    root.create_child_directory(Path::new("install.lock"))
        .expect_err("held lock directory creation must be rejected");
    root.create_private_staging_directory(Path::new("install.lock"))
        .expect_err("non-staging namespace must be rejected");
    root.create_private_staging_directory(Path::new(".hypercolor-stage-"))
        .expect_err("empty staging suffix must be rejected");
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-lock"))
        .expect("create staged directory");
    staging
        .publish(Path::new("install.lock"))
        .expect_err("held lock publication destination must be rejected");

    assert!(directory.path().join("install.lock").is_file());
    assert!(directory.path().join(".hypercolor-stage-lock").is_dir());
}

#[test]
fn cleanup_rejects_hardlinks_and_special_members() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = acquire(directory.path());
    let staging = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-unsafe"))
        .expect("create staged directory");
    write_file(staging.directory(), "original", 0o600, b"linked");
    fs::hard_link(
        directory.path().join(".hypercolor-stage-unsafe/original"),
        directory.path().join(".hypercolor-stage-unsafe/hardlink"),
    )
    .expect("create staging hardlink");

    staging.remove().expect_err("cleanup must reject hardlinks");
    assert!(
        directory
            .path()
            .join(".hypercolor-stage-unsafe/hardlink")
            .exists()
    );

    fs::remove_dir_all(directory.path().join(".hypercolor-stage-unsafe"))
        .expect("remove rejected hardlink staging tree");
    let special = root
        .create_private_staging_directory(Path::new(".hypercolor-stage-special"))
        .expect("create special staging directory");
    let _listener = UnixListener::bind(directory.path().join(".hypercolor-stage-special/socket"))
        .expect("create staging socket");
    special
        .remove()
        .expect_err("cleanup must reject special files");
    assert!(
        directory
            .path()
            .join(".hypercolor-stage-special/socket")
            .exists()
    );
}

#[test]
fn opened_child_survives_its_path_becoming_a_symlink() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let outside = tempfile::tempdir().expect("outside directory");
    let root = acquire(directory.path());
    let child = root
        .create_child_directory(Path::new("child"))
        .expect("create child directory");
    let retained = directory.path().join("retained");
    fs::rename(directory.path().join("child"), &retained).expect("rename opened child");
    std::os::unix::fs::symlink(outside.path(), directory.path().join("child"))
        .expect("replace child path with symlink");

    write_file(&child, "marker", 0o600, b"retained");

    assert_eq!(
        fs::read(retained.join("marker")).expect("read retained child"),
        b"retained"
    );
    assert!(!outside.path().join("marker").exists());
    root.open_child_directory(Path::new("child"))
        .expect_err("replacement symlink must not be followed");
}
