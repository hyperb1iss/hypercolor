#![cfg(unix)]

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{
    EntryReplacement, ExactDirectoryEntry, ExactEntry, ExclusiveDirectory, MAX_EXACT_ENTRY_BYTES,
    MAX_PUBLIC_DIRECTORY_CHILD_COUNT, MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES,
};

struct Fixture {
    _temporary: tempfile::TempDir,
    lock_root: PathBuf,
    public: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("platform-fs-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let canonical = fs::canonicalize(temporary.path()).expect("canonical temporary directory");
        let lock_root = canonical.join("lock-root");
        let public = canonical.join("public");
        fs::create_dir(&lock_root).expect("create lock root");
        fs::create_dir(&public).expect("create public root");
        Self {
            _temporary: temporary,
            lock_root,
            public,
        }
    }

    fn lock(&self) -> ExclusiveDirectory {
        ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
            .expect("acquire global lock")
            .expect("uncontended global lock")
    }
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

/// Create a socket filesystem entry at `path`.
///
/// A deep fixture path can exceed the `sockaddr_un` limit, so the socket is
/// bound in the short system temp directory and its inode renamed into the
/// fixture when the direct bind cannot address the path.
fn create_socket_entry(path: &std::path::Path) {
    if UnixListener::bind(path).is_ok() {
        return;
    }
    let staging = tempfile::tempdir().expect("short socket staging directory");
    let staged = staging.path().join("s");
    let _listener = UnixListener::bind(&staged).expect("bind staged socket entry");
    fs::rename(&staged, path).expect("move socket entry into the fixture");
}

#[test]
fn public_child_names_are_bounded_sorted_and_handle_relative() {
    let fixture = Fixture::new();
    write_mode(&fixture.public.join("z-last"), b"last", 0o644);
    write_mode(&fixture.public.join("a-first"), b"first", 0o644);
    fs::create_dir(fixture.public.join("d-directory")).expect("create child directory");
    symlink("a-first", fixture.public.join("m-link")).expect("create child symlink");
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");

    let names = authority.child_names().expect("enumerate child names");
    assert_eq!(
        names,
        ["a-first", "d-directory", "m-link", "z-last"]
            .map(OsString::from)
            .to_vec()
    );
    let mut opened = authority
        .open_regular_file(Path::new(&names[0]))
        .expect("open enumerated regular file");
    let mut bytes = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut bytes)
        .expect("read enumerated regular file");
    assert_eq!(bytes, b"first");
    authority
        .open_child_directory(Path::new(&names[1]))
        .expect("open enumerated child directory");
    assert!(matches!(
        authority
            .observe_entry(Path::new(&names[2]))
            .expect("observe enumerated symlink"),
        ExactEntry::Symlink { .. }
    ));
    authority
        .open_child_directory(Path::new(&names[2]))
        .expect_err("enumerated symlink must not open as a directory");
}

#[test]
fn public_child_names_reject_parent_replaced_by_symlink() {
    let fixture = Fixture::new();
    write_mode(&fixture.public.join("entry"), b"entry", 0o644);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let detached = fixture.public.with_extension("detached");
    fs::rename(&fixture.public, &detached).expect("detach public directory");
    symlink(&detached, &fixture.public).expect("replace public parent with symlink");

    authority
        .child_names()
        .expect_err("symlink ancestry replacement must fail closed");

    assert!(fixture.public.is_symlink());
    assert_eq!(
        fs::read(detached.join("entry")).expect("read detached entry"),
        b"entry"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn public_child_names_preserve_non_utf8_platform_names() {
    let fixture = Fixture::new();
    let non_utf8 = OsString::from_vec(vec![b'n', 0xff]);
    write_mode(&fixture.public.join("ascii"), b"ascii", 0o644);
    write_mode(&fixture.public.join(&non_utf8), b"raw", 0o644);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");

    assert_eq!(
        authority.child_names().expect("enumerate raw child name"),
        vec![OsString::from("ascii"), non_utf8]
    );
}

#[test]
fn public_child_names_enforce_count_and_aggregate_byte_bounds() {
    let count_fixture = Fixture::new();
    for index in 0..=MAX_PUBLIC_DIRECTORY_CHILD_COUNT {
        write_mode(
            &count_fixture.public.join(format!("entry-{index:04}")),
            b"entry",
            0o644,
        );
    }
    let count_lock = count_fixture.lock();
    let count_authority = count_lock
        .open_public_directory(&count_fixture.public)
        .expect("open count authority");
    count_authority
        .child_names()
        .expect_err("child count above the bound must fail");
    assert!(count_fixture.public.join("entry-0000").is_file());

    let bytes_fixture = Fixture::new();
    let name_len = 255;
    let entry_count = MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES / name_len + 1;
    assert!(entry_count < MAX_PUBLIC_DIRECTORY_CHILD_COUNT);
    for index in 0..entry_count {
        let mut name = format!("{index:04}").into_bytes();
        name.resize(name_len, b'x');
        write_mode(
            &bytes_fixture.public.join(OsString::from_vec(name)),
            b"entry",
            0o644,
        );
    }
    let bytes_lock = bytes_fixture.lock();
    let bytes_authority = bytes_lock
        .open_public_directory(&bytes_fixture.public)
        .expect("open aggregate-byte authority");
    bytes_authority
        .child_names()
        .expect_err("aggregate child name bytes above the bound must fail");
    assert_eq!(
        fs::read_dir(&bytes_fixture.public)
            .expect("read unchanged byte-bound directory")
            .count(),
        entry_count
    );
}

#[test]
fn public_child_names_retain_global_lock_lifetime() {
    let fixture = Fixture::new();
    write_mode(&fixture.public.join("entry"), b"entry", 0o644);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    drop(lock);

    assert_eq!(
        authority
            .child_names()
            .expect("enumerate while lock retained"),
        [OsString::from("entry")]
    );
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("probe retained lock")
            .is_none()
    );
    drop(authority);
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("reacquire released lock")
            .is_some()
    );
}

#[test]
fn public_authority_rejects_symlink_at_every_ancestor_depth() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let real = fixture.public.join("real");
    fs::create_dir(&real).expect("create real ancestor");
    fs::create_dir(real.join("leaf")).expect("create real leaf");
    symlink(&real, fixture.public.join("linked-parent")).expect("create parent symlink");
    symlink(real.join("leaf"), fixture.public.join("linked-leaf")).expect("create leaf symlink");

    lock.open_public_directory(&fixture.public.join("linked-parent/leaf"))
        .expect_err("intermediate symlink must be rejected");
    lock.open_public_directory(&fixture.public.join("linked-leaf"))
        .expect_err("final symlink must be rejected");
    lock.open_public_directory(Path::new("relative/path"))
        .expect_err("relative paths must be rejected");
}

#[test]
fn parent_replacement_fails_before_mutating_a_detached_directory() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let detached = fixture.public.with_extension("detached");
    fs::rename(&fixture.public, &detached).expect("detach public directory");
    fs::create_dir(&fixture.public).expect("replace public directory");

    authority
        .durable_replace_entry(
            Path::new("hypercolor"),
            &ExactEntry::Absent,
            EntryReplacement::Symlink {
                target: Path::new("../lib/hypercolor/active/bin/hypercolor"),
            },
        )
        .expect_err("replaced ancestry must fail closed");

    assert!(!fixture.public.join("hypercolor").exists());
    assert!(!detached.join("hypercolor").exists());
}

#[test]
fn exact_regular_file_converts_to_stable_relative_symlink() {
    let fixture = Fixture::new();
    let binary = fixture.public.join("hypercolor");
    write_mode(&binary, b"legacy binary", 0o755);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let expected = authority
        .observe_entry(Path::new("hypercolor"))
        .expect("observe legacy binary");
    let target = Path::new("../lib/hypercolor/active/bin/hypercolor");

    let published = authority
        .durable_replace_entry(
            Path::new("hypercolor"),
            &expected,
            EntryReplacement::Symlink { target },
        )
        .expect("convert legacy binary to stable symlink");

    assert_eq!(fs::read_link(&binary).expect("read stable symlink"), target);
    assert!(matches!(
        published,
        ExactEntry::Symlink { target: actual, .. } if actual == target
    ));
}

#[test]
fn exact_replacement_supports_absolute_symlink_and_regular_bytes() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let absolute = fixture
        .public
        .join("../lib/hypercolor/active/bin/hypercolor-daemon");
    authority
        .durable_replace_entry(
            Path::new("daemon"),
            &ExactEntry::Absent,
            EntryReplacement::Symlink { target: &absolute },
        )
        .expect("publish absolute stable symlink");
    authority
        .durable_replace_entry(
            Path::new("fragment"),
            &ExactEntry::Absent,
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"stable fragment",
            },
        )
        .expect("publish stable regular file");

    assert_eq!(
        fs::read_link(fixture.public.join("daemon")).expect("read absolute link"),
        absolute
    );
    assert_eq!(
        fs::read(fixture.public.join("fragment")).expect("read fragment"),
        b"stable fragment"
    );
}

#[test]
fn exact_observation_rejects_unsupported_entry_states() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    fs::create_dir(fixture.public.join("directory")).expect("create directory entry");
    create_socket_entry(&fixture.public.join("socket"));
    write_mode(&fixture.public.join("original"), b"linked", 0o644);
    write_mode(&fixture.public.join("set-id"), b"unsafe mode", 0o4644);
    fs::hard_link(
        fixture.public.join("original"),
        fixture.public.join("hardlink"),
    )
    .expect("create hardlink");
    let oversized = File::create(fixture.public.join("oversized")).expect("create oversized file");
    oversized
        .set_len(MAX_EXACT_ENTRY_BYTES + 1)
        .expect("make sparse oversized file");

    authority
        .observe_entry(Path::new("directory"))
        .expect_err("directories must be rejected");
    authority
        .observe_entry(Path::new("socket"))
        .expect_err("special files must be rejected");
    authority
        .observe_entry(Path::new("set-id"))
        .expect_err("set-ID permission bits must be rejected");
    authority
        .observe_entry(Path::new("hardlink"))
        .expect_err("hardlinks must be rejected");
    authority
        .observe_entry(Path::new("oversized"))
        .expect_err("oversized files must be rejected before hashing");
    authority
        .observe_entry(Path::new("nested/name"))
        .expect_err("nested names must be rejected");
    authority
        .durable_replace_entry(
            Path::new("invalid-target"),
            &ExactEntry::Absent,
            EntryReplacement::Symlink {
                target: Path::new("."),
            },
        )
        .expect_err("current-directory targets must be rejected");
    authority
        .durable_replace_entry(
            Path::new("invalid-root-target"),
            &ExactEntry::Absent,
            EntryReplacement::Symlink {
                target: Path::new("/"),
            },
        )
        .expect_err("root-only targets must be rejected");
    authority
        .durable_replace_entry(
            Path::new("invalid-mode"),
            &ExactEntry::Absent,
            EntryReplacement::RegularFile {
                mode: 0o4644,
                contents: b"unsafe mode",
            },
        )
        .expect_err("set-ID replacement modes must be rejected");
}

#[test]
fn anchored_regular_open_rejects_unsupported_entry_states() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    fs::create_dir(fixture.public.join("directory")).expect("create directory entry");
    create_socket_entry(&fixture.public.join("socket"));
    write_mode(&fixture.public.join("original"), b"linked", 0o644);
    write_mode(&fixture.public.join("set-id"), b"unsafe mode", 0o4644);
    fs::hard_link(
        fixture.public.join("original"),
        fixture.public.join("hardlink"),
    )
    .expect("create hardlink");
    symlink("original", fixture.public.join("symlink")).expect("create symlink");

    for name in ["directory", "socket", "set-id", "hardlink", "symlink"] {
        authority
            .open_regular_file(Path::new(name))
            .expect_err("unsupported public read entry must be rejected");
    }
    authority
        .open_regular_file(Path::new("nested/name"))
        .expect_err("nested public read names must be rejected");
}

#[test]
fn anchored_regular_open_retains_inode_after_pathname_replacement() {
    let fixture = Fixture::new();
    let entry = fixture.public.join("fragment");
    let displaced = fixture.public.join("displaced-fragment");
    write_mode(&entry, b"prior bytes", 0o644);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let mut opened = authority
        .open_regular_file(Path::new("fragment"))
        .expect("open anchored public regular file");
    let opened_metadata = opened.metadata();

    fs::rename(&entry, &displaced).expect("displace opened entry");
    write_mode(&entry, b"attacker", 0o644);
    let mut bytes = Vec::new();
    opened
        .file_mut()
        .read_to_end(&mut bytes)
        .expect("snapshot retained file");

    assert_eq!(bytes, b"prior bytes");
    assert_eq!(
        opened_metadata.inode(),
        fs::symlink_metadata(&displaced)
            .expect("inspect displaced entry")
            .ino()
    );
    assert_ne!(
        opened_metadata.inode(),
        fs::symlink_metadata(&entry)
            .expect("inspect pathname replacement")
            .ino()
    );
}

#[test]
fn exact_replacement_rejects_all_semantic_and_identity_drift() {
    for drift in ["kind", "inode", "mode", "size", "digest"] {
        let fixture = Fixture::new();
        let destination = fixture.public.join("entry");
        write_mode(&destination, b"before", 0o644);
        let lock = fixture.lock();
        let authority = lock
            .open_public_directory(&fixture.public)
            .expect("open public authority");
        let expected = authority
            .observe_entry(Path::new("entry"))
            .expect("observe expected entry");
        match drift {
            "kind" => {
                fs::remove_file(&destination).expect("remove regular file");
                symlink("target", &destination).expect("replace with symlink");
            }
            "inode" => {
                let replacement = fixture.public.join("replacement");
                write_mode(&replacement, b"before", 0o644);
                fs::rename(replacement, &destination).expect("replace inode");
            }
            "mode" => fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))
                .expect("drift mode"),
            "size" => fs::write(&destination, b"longer-before").expect("drift size"),
            "digest" => fs::write(&destination, b"differ").expect("drift digest"),
            _ => unreachable!(),
        }

        authority
            .durable_replace_entry(
                Path::new("entry"),
                &expected,
                EntryReplacement::RegularFile {
                    mode: 0o644,
                    contents: b"after",
                },
            )
            .expect_err("all exact-state drift must be rejected");
        assert_ne!(
            fs::read(&destination).unwrap_or_default(),
            b"after",
            "drift case {drift} must not publish replacement"
        );
    }
}

#[test]
fn exact_symlink_target_drift_is_rejected() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    symlink("old-target", &destination).expect("create expected symlink");
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected symlink");
    fs::remove_file(&destination).expect("remove expected symlink");
    symlink("drift-target", &destination).expect("create drifted symlink");

    authority
        .durable_replace_entry(
            Path::new("entry"),
            &expected,
            EntryReplacement::Symlink {
                target: Path::new("new-target"),
            },
        )
        .expect_err("target drift must be rejected");
    assert_eq!(
        fs::read_link(destination).expect("read preserved drifted symlink"),
        Path::new("drift-target")
    );
}

#[test]
fn exact_removal_and_single_child_creation_retain_global_lock() {
    let fixture = Fixture::new();
    write_mode(&fixture.public.join("fragment"), b"fragment", 0o644);
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let expected = authority
        .observe_entry(Path::new("fragment"))
        .expect("observe fragment");
    authority
        .durable_remove_entry(Path::new("fragment"), &expected)
        .expect("remove exact fragment");
    let child = authority
        .durable_create_child_directory(Path::new("completions"), 0o755)
        .expect("create one child directory");
    drop(lock);

    assert!(!fixture.public.join("fragment").exists());
    assert_eq!(
        child
            .observe_entry(Path::new("missing"))
            .expect("observe through child"),
        ExactEntry::Absent
    );
    assert_eq!(
        child
            .open_child_directory(Path::new("nested"))
            .expect_err("create only one child at a time")
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("probe retained global lock")
            .is_none()
    );
    assert_eq!(
        fs::symlink_metadata(fixture.public.join("completions"))
            .expect("inspect child")
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
}

#[test]
fn monotone_child_ensure_replays_nested_scaffolding_and_retains_global_lock() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let first = authority
        .durable_ensure_child_directory(Path::new("share"), 0o755)
        .expect("ensure first scaffold");
    fs::write(fixture.public.join("share/retained"), b"retained")
        .expect("populate retained scaffold");
    let replay = authority
        .durable_ensure_child_directory(Path::new("share"), 0o755)
        .expect("replay existing scaffold");
    let nested = replay
        .durable_ensure_child_directory(Path::new("applications"), 0o755)
        .expect("ensure nested scaffold topologically");
    drop(lock);
    drop(first);
    drop(replay);

    assert_eq!(
        fs::read(fixture.public.join("share/retained")).expect("read retained contents"),
        b"retained"
    );
    assert_eq!(
        fs::symlink_metadata(fixture.public.join("share/applications"))
            .expect("inspect nested scaffold")
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("probe retained global lock")
            .is_none()
    );
    nested
        .validate_ancestry()
        .expect("nested authority retains exact ancestry");
}

#[test]
fn monotone_child_ensure_rejects_unsupported_existing_states() {
    let fixture = Fixture::new();
    fs::write(fixture.public.join("file"), b"file").expect("create regular file");
    symlink("file", fixture.public.join("link")).expect("create symlink");
    fs::create_dir(fixture.public.join("wrong-mode")).expect("create wrong-mode directory");
    fs::set_permissions(
        fixture.public.join("wrong-mode"),
        fs::Permissions::from_mode(0o750),
    )
    .expect("set wrong mode");
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");

    for name in ["file", "link", "wrong-mode"] {
        authority
            .durable_ensure_child_directory(Path::new(name), 0o755)
            .expect_err("unsupported state must be rejected");
    }
    authority
        .durable_ensure_child_directory(Path::new("nested/name"), 0o755)
        .expect_err("nested name must be rejected");
    authority
        .durable_ensure_child_directory(Path::new("unsafe-mode"), 0o1755)
        .expect_err("special permission bits must be rejected");
    assert!(!fixture.public.join("unsafe-mode").exists());
}

#[test]
fn exact_empty_child_create_observe_remove_retains_global_lock() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let child = authority
        .durable_create_child_directory(Path::new("completions"), 0o755)
        .expect("create exact empty child");
    let expected = authority
        .observe_empty_child_directory(Path::new("completions"))
        .expect("observe exact empty child");
    assert!(matches!(
        expected,
        ExactDirectoryEntry::Empty { mode: 0o755, .. }
    ));

    authority
        .durable_remove_empty_child_directory(Path::new("completions"), &expected)
        .expect("remove exact empty child");
    drop(lock);

    assert_eq!(
        authority
            .observe_empty_child_directory(Path::new("completions"))
            .expect("observe removed child"),
        ExactDirectoryEntry::Absent
    );
    assert_eq!(
        fs::read_dir(&fixture.public)
            .expect("enumerate public tombstones")
            .filter(|entry| {
                entry
                    .as_ref()
                    .expect("read public tombstone entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".hypercolor-public-directory-recovery-")
            })
            .count(),
        1
    );
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("probe retained global lock")
            .is_none()
    );
    drop(child);
}

#[test]
fn exact_empty_child_observation_rejects_unsafe_or_nonempty_entries() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let authority = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    fs::create_dir(fixture.public.join("nonempty")).expect("create nonempty directory");
    fs::write(fixture.public.join("nonempty/entry"), b"content").expect("populate directory");
    fs::create_dir(fixture.public.join("unsafe-mode")).expect("create unsafe directory");
    fs::set_permissions(
        fixture.public.join("unsafe-mode"),
        fs::Permissions::from_mode(0o1755),
    )
    .expect("set sticky mode");
    fs::write(fixture.public.join("regular"), b"regular").expect("create regular entry");
    symlink("nonempty", fixture.public.join("symlink")).expect("create directory symlink");

    assert_eq!(
        authority
            .observe_empty_child_directory(Path::new("absent"))
            .expect("observe absent child"),
        ExactDirectoryEntry::Absent
    );
    for name in ["nonempty", "unsafe-mode", "regular", "symlink"] {
        authority
            .observe_empty_child_directory(Path::new(name))
            .expect_err("unsupported empty-directory state must be rejected");
    }
    authority
        .observe_empty_child_directory(Path::new("nested/name"))
        .expect_err("nested directory names must be rejected");
    authority
        .durable_remove_empty_child_directory(Path::new("absent"), &ExactDirectoryEntry::Absent)
        .expect_err("absent exact state cannot authorize removal");
}
