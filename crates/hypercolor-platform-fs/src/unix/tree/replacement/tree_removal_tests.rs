//! Tree removal under the rename rule of macOS 15, which CI's runners
//! (Linux and macOS 26) do not enforce.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::{RenameFlags, renameat_with};
use rustix::io::Errno;

use super::super::traversal::entry_metadata_at;
use crate::unix::tree::{DirectoryEntryKind, ExclusiveDirectory, PublicDirectoryAuthority};

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
        let canonical = fs::canonicalize(temporary.path()).expect("canonical temporary root");
        let lock_root = canonical.join("lock");
        let public = canonical.join("public");
        fs::create_dir(&lock_root).expect("create lock root");
        fs::create_dir(&public).expect("create public root");
        Self {
            _temporary: temporary,
            lock_root,
            public,
        }
    }

    fn authority(&self) -> (ExclusiveDirectory, PublicDirectoryAuthority) {
        let lock = ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
            .expect("acquire lock")
            .expect("uncontended lock");
        let authority = lock
            .open_public_directory(&self.public)
            .expect("open public directory");
        (lock, authority)
    }

    /// An installed unit: every directory read-only, the root included.
    fn read_only_unit(&self) -> PathBuf {
        let unit = self.public.join("unit");
        let macos = unit.join("Hypercolor.app/Contents/MacOS");
        fs::create_dir_all(&macos).expect("create unit");
        fs::write(unit.join("manifest.json"), b"{}").expect("manifest");
        fs::write(macos.join("hypercolor-daemon"), b"daemon").expect("daemon");
        fs::set_permissions(
            unit.join("manifest.json"),
            fs::Permissions::from_mode(0o444),
        )
        .expect("manifest mode");
        for directory in [
            macos.as_path(),
            macos.parent().expect("Contents"),
            &unit.join("Hypercolor.app"),
            &unit,
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o555))
                .expect("read-only directory");
        }
        unit
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("inspect")
        .permissions()
        .mode()
        & 0o7777
}

/// The no-replace rename as macOS 15 performs it: a directory its owner
/// cannot write refuses to move, even within one parent.
fn macos_15_rename(directory: &File, from: &OsStr, to: &OsStr) -> Result<(), Errno> {
    let source = entry_metadata_at(directory, from)
        .expect("inspect the rename source")
        .expect("the rename source exists");
    if source.kind() == DirectoryEntryKind::Directory && source.mode() & 0o200 == 0 {
        return Err(Errno::ACCESS);
    }
    renameat_with(directory, from, directory, to, RenameFlags::NOREPLACE)
}

#[test]
fn a_read_only_root_is_renamed_where_macos_15_refuses_read_only_directories() {
    let fixture = Fixture::new();
    let unit = fixture.read_only_unit();
    let (_lock, authority) = fixture.authority();

    assert!(
        authority
            .durable_remove_child_tree_with(Path::new("unit"), macos_15_rename)
            .expect("remove a read-only unit")
    );
    assert!(!unit.exists());
    assert_eq!(fs::read_dir(&fixture.public).expect("list").count(), 0);
}

#[test]
fn a_root_whose_rename_fails_keeps_its_name_and_mode() {
    let fixture = Fixture::new();
    let unit = fixture.read_only_unit();
    let (_lock, authority) = fixture.authority();

    let error = authority
        .durable_remove_child_tree_with(Path::new("unit"), |_, _, _| Err(Errno::IO))
        .expect_err("the rename fails");
    assert_eq!(error.raw_os_error(), Some(Errno::IO.raw_os_error()));
    assert_eq!(mode(&unit), 0o555);
    assert_eq!(mode(&unit.join("Hypercolor.app")), 0o555);
    assert!(
        unit.join("Hypercolor.app/Contents/MacOS/hypercolor-daemon")
            .is_file()
    );
    assert_eq!(fs::read_dir(&fixture.public).expect("list").count(), 1);

    assert!(
        authority
            .durable_remove_child_tree_with(Path::new("unit"), macos_15_rename)
            .expect("a later removal finishes")
    );
    assert!(!unit.exists());
}
