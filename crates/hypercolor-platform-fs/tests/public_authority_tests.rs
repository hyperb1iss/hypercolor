#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{
    EntryReplacement, ExactEntry, ExclusiveDirectory, MAX_EXACT_ENTRY_BYTES,
};

struct Fixture {
    _temporary: tempfile::TempDir,
    lock_root: PathBuf,
    public: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary directory");
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
    UnixListener::bind(fixture.public.join("socket")).expect("create socket entry");
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
