use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use super::super::{DirectoryEntryKind, ExclusiveDirectory, PublicDirectoryAuthority};

struct Fixture {
    _temporary: tempfile::TempDir,
    lock_root: PathBuf,
    ancestor: PathBuf,
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
        let ancestor = canonical.join("ancestor");
        let public = ancestor.join("public");
        fs::create_dir(&lock_root).expect("create lock root");
        fs::create_dir(&ancestor).expect("create public ancestor");
        fs::create_dir(&public).expect("create public directory");
        Self {
            _temporary: temporary,
            lock_root,
            ancestor,
            public,
        }
    }

    fn authority(&self) -> (ExclusiveDirectory, PublicDirectoryAuthority) {
        let lock = ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
            .expect("acquire lock")
            .expect("uncontended lock");
        let authority = lock
            .open_public_directory(&self.public)
            .expect("open public authority");
        (lock, authority)
    }
}

#[test]
fn stable_metadata_comes_from_the_retained_final_handle() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.public, fs::Permissions::from_mode(0o750))
        .expect("set public mode");
    let (_lock, authority) = fixture.authority();
    let expected = fs::metadata(&fixture.public).expect("inspect canonical public directory");

    let actual = authority
        .metadata()
        .expect("inspect retained public handle");

    assert_eq!(actual.kind(), DirectoryEntryKind::Directory);
    assert_eq!(actual.mode(), 0o750);
    assert_eq!(actual.device(), expected.dev());
    assert_eq!(actual.inode(), expected.ino());
}

#[test]
fn metadata_rejects_ancestor_replacement_before_handle_inspection() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let detached = fixture.ancestor.with_extension("detached");
    fs::rename(&fixture.ancestor, &detached).expect("detach public ancestor");
    fs::create_dir(&fixture.ancestor).expect("replace public ancestor");
    fs::create_dir(&fixture.public).expect("replace public directory");

    authority
        .metadata()
        .expect_err("ancestor replacement must fail closed");
}

#[test]
fn metadata_rejects_final_path_replacement_after_retained_fstat() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let retained = authority.metadata().expect("inspect retained identity");
    let detached = fixture.public.with_extension("detached");

    authority
        .metadata_with(|| {
            fs::rename(&fixture.public, &detached)?;
            fs::create_dir(&fixture.public)
        })
        .expect_err("final path replacement must fail after fstat");

    let detached_metadata = fs::metadata(&detached).expect("inspect detached directory");
    let replacement_metadata = fs::metadata(&fixture.public).expect("inspect replacement");
    assert_eq!(retained.device(), detached_metadata.dev());
    assert_eq!(retained.inode(), detached_metadata.ino());
    assert_ne!(retained.inode(), replacement_metadata.ino());
}
