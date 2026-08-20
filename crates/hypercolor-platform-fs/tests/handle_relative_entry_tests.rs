#![cfg(unix)]

use std::fs::{self, File};
use std::io::Read as _;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{DirectoryAuthority, ExclusiveDirectory};

struct Fixture {
    _temporary: tempfile::TempDir,
    lock_root: PathBuf,
    public_root: PathBuf,
    public: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let canonical = fs::canonicalize(temporary.path()).expect("canonical temporary directory");
        let lock_root = canonical.join("lock-root");
        let public_root = canonical.join("public-root");
        let public = public_root.join("store");
        fs::create_dir(&lock_root).expect("create lock root");
        fs::create_dir(&public_root).expect("create public root");
        fs::create_dir(&public).expect("create public store");
        Self {
            _temporary: temporary,
            lock_root,
            public_root,
            public,
        }
    }

    fn lock(&self) -> ExclusiveDirectory {
        ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
            .expect("acquire global lock")
            .expect("uncontended global lock")
    }

    fn downgrade(&self, lock: &ExclusiveDirectory) -> DirectoryAuthority {
        lock.open_public_directory(&self.public)
            .expect("open public authority")
            .into_directory_authority()
            .expect("downgrade to handle-relative authority")
    }
}

fn read_all(mut file: File) -> Vec<u8> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read retained file");
    bytes
}

#[test]
fn downgrade_stays_bound_after_public_ancestor_replacement() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let directory = fixture.downgrade(&lock);
    let detached = fixture.public_root.with_extension("detached");
    fs::rename(&fixture.public_root, &detached).expect("detach public ancestry");
    fs::create_dir(&fixture.public_root).expect("replace public root");
    fs::create_dir(&fixture.public).expect("replace public store");

    directory
        .write_secret(Path::new("journal.next"), b"retained")
        .expect("write through retained authority");
    directory
        .durable_replace_file(Path::new("journal.next"), Path::new("journal"))
        .expect("publish through retained authority");

    assert_eq!(
        fs::read(detached.join("store/journal")).expect("read retained incarnation"),
        b"retained"
    );
    assert!(!fixture.public.join("journal").exists());
}

#[test]
fn downgrade_requires_one_final_ancestry_proof() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let public = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let detached = fixture.public_root.with_extension("detached");
    fs::rename(&fixture.public_root, &detached).expect("detach public ancestry");
    fs::create_dir(&fixture.public_root).expect("replace public root");
    fs::create_dir(&fixture.public).expect("replace public store");

    public
        .into_directory_authority()
        .expect_err("changed ancestry must prevent downgrade");

    assert!(
        fs::read_dir(&fixture.public)
            .expect("read replacement")
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(detached.join("store"))
            .expect("read detached store")
            .next()
            .is_none()
    );
}

#[test]
fn downgraded_authority_retains_flock_after_original_capabilities_drop() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let public = lock
        .open_public_directory(&fixture.public)
        .expect("open public authority");
    let directory = public
        .into_directory_authority()
        .expect("downgrade public authority");
    drop(lock);

    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("probe retained lock")
            .is_none()
    );
    drop(directory);
    assert!(
        ExclusiveDirectory::try_acquire(&fixture.lock_root, Path::new("install.lock"))
            .expect("reacquire released lock")
            .is_some()
    );
}

#[test]
fn handle_relative_entry_operations_support_journal_and_active_switches() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let directory = fixture.downgrade(&lock);

    directory
        .write_secret(Path::new("journal.next"), b"cursor-1")
        .expect("write staged journal");
    directory
        .durable_replace_file(Path::new("journal.next"), Path::new("journal"))
        .expect("publish journal");
    assert_eq!(
        read_all(
            directory
                .open_file(Path::new("journal"))
                .expect("open journal")
        ),
        b"cursor-1"
    );

    directory
        .durable_replace_symlink(Path::new("units/v1"), Path::new("active"))
        .expect("publish first active link");
    assert_eq!(
        directory
            .read_symlink(Path::new("active"))
            .expect("read first active link"),
        Some(PathBuf::from("units/v1"))
    );
    directory
        .durable_replace_symlink(Path::new("units/v2"), Path::new("active"))
        .expect("switch active link");
    assert_eq!(
        directory
            .read_symlink(Path::new("active"))
            .expect("read switched active link"),
        Some(PathBuf::from("units/v2"))
    );
    assert!(
        directory
            .durable_remove_file(Path::new("active"))
            .expect("remove active link")
    );
    assert_eq!(
        directory
            .read_symlink(Path::new("active"))
            .expect("observe absent active link"),
        None
    );
    assert!(
        !directory
            .durable_remove_file(Path::new("active"))
            .expect("replay absent active removal")
    );
}

#[test]
fn opened_file_stays_bound_after_pathname_replacement() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let directory = fixture.downgrade(&lock);
    directory
        .write_secret(Path::new("selected"), b"selected")
        .expect("write selected file");
    let opened = directory
        .open_file(Path::new("selected"))
        .expect("open selected file");
    fs::rename(
        fixture.public.join("selected"),
        fixture.public.join("displaced"),
    )
    .expect("displace selected name");
    fs::write(fixture.public.join("selected"), b"replacement").expect("replace selected name");

    assert_eq!(read_all(opened), b"selected");
    assert_eq!(
        fs::read(fixture.public.join("selected")).expect("read replacement"),
        b"replacement"
    );
}

#[test]
fn downgraded_lock_parent_rejects_lock_entry_mutation() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let root = lock
        .open_public_directory(&fixture.lock_root)
        .expect("open anchored lock parent")
        .into_directory_authority()
        .expect("downgrade lock parent");

    root.write_secret(Path::new("install.lock"), b"replacement")
        .expect_err("held lock name must not be created");
    root.durable_replace_symlink(Path::new("units/v1"), Path::new("install.lock"))
        .expect_err("held lock name must not become a symlink");
    root.durable_remove_file(Path::new("install.lock"))
        .expect_err("held lock name must not be removed");

    assert!(fixture.lock_root.join("install.lock").is_file());
}

#[test]
fn child_authority_rejects_hardlink_alias_of_held_lock() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let root = lock.root_directory().expect("open lock root authority");
    let child = root
        .create_child_directory(Path::new("child"))
        .expect("create child directory");
    let alias = fixture.lock_root.join("child/lock-alias");
    fs::hard_link(fixture.lock_root.join("install.lock"), &alias).expect("create lock hardlink");
    child
        .write_secret(Path::new("replacement"), b"replacement")
        .expect("write replacement source");

    child
        .durable_replace_file(Path::new("replacement"), Path::new("lock-alias"))
        .expect_err("replacement must not overwrite a lock alias");
    child
        .durable_remove_file(Path::new("lock-alias"))
        .expect_err("removal must not unlink a lock alias");

    assert!(alias.is_file());
    assert!(fixture.lock_root.join("install.lock").is_file());
    assert_eq!(fs::read(alias).expect("read lock alias"), b"");
}

#[test]
fn one_entry_apis_reject_symlink_types_and_non_normal_names() {
    let fixture = Fixture::new();
    let lock = fixture.lock();
    let directory = fixture.downgrade(&lock);
    symlink("target", fixture.public.join("link")).expect("create symlink");
    fs::write(fixture.public.join("regular"), b"regular").expect("create regular file");
    fs::create_dir(fixture.public.join("directory")).expect("create directory");

    directory
        .open_file(Path::new("link"))
        .expect_err("open file must not follow symlink");
    directory
        .read_symlink(Path::new("regular"))
        .expect_err("symlink read must reject regular file");
    directory
        .durable_replace_symlink(Path::new("units/v1"), Path::new("regular"))
        .expect_err("symlink replace must reject regular destination");
    directory
        .durable_remove_file(Path::new("directory"))
        .expect_err("file removal must reject directory");
    directory
        .write_secret(Path::new("../escape"), b"escape")
        .expect_err("entry APIs must reject traversal names");

    assert!(!fixture.public_root.join("escape").exists());
    assert_eq!(
        fs::read(fixture.public.join("regular")).expect("read regular"),
        b"regular"
    );
}
