use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::unix::tree::{ExclusiveDirectory, PublicDirectoryAuthority, ReadOnlyDirectoryAuthority};

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
}

#[test]
fn enumeration_rejects_name_mutation_between_confirming_scans() {
    let fixture = Fixture::new();
    fs::write(fixture.public.join("before"), b"before").expect("write initial entry");
    let (_lock, authority) = fixture.authority();

    let error = authority
        .child_names_with(|| fs::write(fixture.public.join("after"), b"after"))
        .expect_err("name mutation must invalidate enumeration");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        fs::read(fixture.public.join("before")).expect("read retained initial entry"),
        b"before"
    );
    assert_eq!(
        fs::read(fixture.public.join("after")).expect("read retained added entry"),
        b"after"
    );
}

#[test]
fn enumeration_rejects_ancestor_replacement_with_symlink() {
    let fixture = Fixture::new();
    fs::write(fixture.public.join("entry"), b"entry").expect("write initial entry");
    let (_lock, authority) = fixture.authority();
    let detached = fixture.public.with_extension("detached");

    authority
        .child_names_with(|| {
            fs::rename(&fixture.public, &detached)?;
            symlink(&detached, &fixture.public)
        })
        .expect_err("symlink ancestry replacement must fail closed");

    assert!(fixture.public.is_symlink());
    assert_eq!(
        fs::read(detached.join("entry")).expect("read entry through detached directory"),
        b"entry"
    );
}

#[test]
fn directory_enumeration_rejects_name_mutation_between_confirming_scans() {
    let temporary = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let root = ExclusiveDirectory::try_acquire(temporary.path(), Path::new("install.lock"))
        .expect("acquire lock")
        .expect("uncontended lock")
        .root_directory()
        .expect("open root authority");
    let payload = root
        .create_child_directory(Path::new("payload"))
        .expect("create payload directory");
    fs::write(temporary.path().join("payload/before"), b"before").expect("write initial entry");

    let error = payload
        .child_names_with(|| fs::write(temporary.path().join("payload/after"), b"after"))
        .expect_err("name mutation must invalidate enumeration");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        fs::read(temporary.path().join("payload/before")).expect("read retained initial entry"),
        b"before"
    );
    assert_eq!(
        fs::read(temporary.path().join("payload/after")).expect("read retained added entry"),
        b"after"
    );
}

#[test]
fn read_only_enumeration_rejects_name_mutation_between_confirming_scans() {
    let temporary = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    fs::write(temporary.path().join("before"), b"before").expect("write initial entry");
    let authority =
        ReadOnlyDirectoryAuthority::open(temporary.path()).expect("open read-only authority");

    let error = authority
        .child_names_with(|| fs::write(temporary.path().join("after"), b"after"))
        .expect_err("name mutation must invalidate enumeration");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        fs::read(temporary.path().join("before")).expect("read retained initial entry"),
        b"before"
    );
    assert_eq!(
        fs::read(temporary.path().join("after")).expect("read retained added entry"),
        b"after"
    );
}
