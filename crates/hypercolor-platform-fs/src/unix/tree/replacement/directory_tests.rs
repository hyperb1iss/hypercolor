use std::cell::Cell;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::unix::tree::{ExactDirectoryEntry, ExclusiveDirectory};

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

    fn authority(
        &self,
    ) -> (
        ExclusiveDirectory,
        crate::unix::tree::PublicDirectoryAuthority,
    ) {
        let lock = ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
            .expect("acquire lock")
            .expect("uncontended lock");
        let authority = lock
            .open_public_directory(&self.public)
            .expect("open public directory");
        (lock, authority)
    }
}

fn matching_names(path: &Path, prefix: &str) -> Vec<OsString> {
    fs::read_dir(path)
        .expect("read directory")
        .filter_map(|entry| {
            let name = entry.expect("read directory entry").file_name();
            name.to_string_lossy().starts_with(prefix).then_some(name)
        })
        .collect()
}

fn observed_empty(
    authority: &crate::unix::tree::PublicDirectoryAuthority,
    name: &str,
) -> ExactDirectoryEntry {
    authority
        .observe_empty_child_directory(Path::new(name))
        .expect("observe empty directory")
}

#[test]
fn empty_directory_observation_detects_name_and_parent_replacement() {
    for replace_parent in [false, true] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.public.join("child")).expect("create child");
        let (_lock, authority) = fixture.authority();
        let detached = fixture.public.with_extension("detached");
        authority
            .observe_empty_child_directory_with(Path::new("child"), || {
                if replace_parent {
                    fs::rename(&fixture.public, &detached)?;
                    fs::create_dir(&fixture.public)?;
                    fs::create_dir(fixture.public.join("child"))
                } else {
                    fs::rename(
                        fixture.public.join("child"),
                        fixture.public.join("detached-child"),
                    )?;
                    fs::create_dir(fixture.public.join("child"))
                }
            })
            .expect_err("observation replacement must fail closed");
    }
}

#[test]
fn empty_directory_removal_rejects_previsibility_drift() {
    for drift in ["inode", "mode", "content"] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.public.join("child")).expect("create child");
        let (_lock, authority) = fixture.authority();
        let expected = observed_empty(&authority, "child");
        authority
            .remove_empty_child_directory_with(
                Path::new("child"),
                &expected,
                |_, _| match drift {
                    "inode" => {
                        fs::rename(
                            fixture.public.join("child"),
                            fixture.public.join("original"),
                        )?;
                        fs::create_dir(fixture.public.join("child"))
                    }
                    "mode" => fs::set_permissions(
                        fixture.public.join("child"),
                        fs::Permissions::from_mode(0o700),
                    ),
                    "content" => fs::write(fixture.public.join("child/attacker"), b"attacker"),
                    _ => unreachable!(),
                },
                || Ok(()),
                |directory| directory.sync_all(),
            )
            .expect_err("previsibility drift must be rejected");

        assert!(fixture.public.join("child").is_dir());
        assert!(
            matching_names(&fixture.public, ".hypercolor-public-directory-recovery-").is_empty()
        );
        if drift == "content" {
            assert_eq!(
                fs::read(fixture.public.join("child/attacker")).expect("read attacker content"),
                b"attacker"
            );
        }
    }
}

#[test]
fn destination_reappearance_is_quarantined_before_exact_restore() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.public.join("child")).expect("create child");
    let expected_inode = fs::metadata(fixture.public.join("child"))
        .expect("inspect expected child")
        .ino();
    let (_lock, authority) = fixture.authority();
    let expected = observed_empty(&authority, "child");
    authority
        .remove_empty_child_directory_with(
            Path::new("child"),
            &expected,
            |_, _| Ok(()),
            || fs::create_dir(fixture.public.join("child")),
            |directory| directory.sync_all(),
        )
        .expect_err("reappeared destination must trigger rollback");

    assert_eq!(
        fs::metadata(fixture.public.join("child"))
            .expect("inspect restored child")
            .ino(),
        expected_inode
    );
    assert_eq!(
        matching_names(&fixture.public, ".hypercolor-public-directory-quarantine-").len(),
        1
    );
    assert!(matching_names(&fixture.public, ".hypercolor-public-directory-recovery-").is_empty());
}

#[test]
fn changed_recovery_directory_remains_quarantined() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.public.join("child")).expect("create child");
    let (_lock, authority) = fixture.authority();
    let expected = observed_empty(&authority, "child");
    let error = authority
        .remove_empty_child_directory_with(
            Path::new("child"),
            &expected,
            |_, _| Ok(()),
            || {
                let names =
                    matching_names(&fixture.public, ".hypercolor-public-directory-recovery-");
                assert_eq!(names.len(), 1);
                fs::write(fixture.public.join(&names[0]).join("attacker"), b"attacker")
            },
            |directory| directory.sync_all(),
        )
        .expect_err("changed recovery must fail closed");

    assert!(error.to_string().contains("remains quarantined"));
    assert!(!fixture.public.join("child").exists());
    let names = matching_names(&fixture.public, ".hypercolor-public-directory-recovery-");
    assert_eq!(names.len(), 1);
    assert_eq!(
        fs::read(fixture.public.join(&names[0]).join("attacker"))
            .expect("read quarantined attacker content"),
        b"attacker"
    );
}

#[test]
fn parent_replacement_before_and_after_directory_visibility_fails_closed() {
    for after_visibility in [false, true] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.public.join("child")).expect("create child");
        let (_lock, authority) = fixture.authority();
        let expected = observed_empty(&authority, "child");
        let detached = fixture.public.with_extension("detached");
        let replace_parent = || {
            fs::rename(&fixture.public, &detached)?;
            fs::create_dir(&fixture.public)
        };
        authority
            .remove_empty_child_directory_with(
                Path::new("child"),
                &expected,
                |_, _| {
                    if after_visibility {
                        Ok(())
                    } else {
                        replace_parent()
                    }
                },
                || {
                    if after_visibility {
                        replace_parent()
                    } else {
                        Ok(())
                    }
                },
                |directory| directory.sync_all(),
            )
            .expect_err("parent replacement must fail closed");

        assert!(!fixture.public.join("child").exists());
        assert!(detached.join("child").is_dir());
    }
}

#[test]
fn directory_parent_sync_failure_restores_exact_child() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.public.join("child")).expect("create child");
    let expected_inode = fs::metadata(fixture.public.join("child"))
        .expect("inspect expected child")
        .ino();
    let (_lock, authority) = fixture.authority();
    let expected = observed_empty(&authority, "child");
    let calls = Cell::new(0_u8);
    authority
        .remove_empty_child_directory_with(
            Path::new("child"),
            &expected,
            |_, _| Ok(()),
            || Ok(()),
            |_| {
                calls.set(calls.get() + 1);
                Err(io::Error::other("injected directory parent sync failure"))
            },
        )
        .expect_err("parent sync failure must restore exact child");

    assert_eq!(calls.get(), 1);
    assert_eq!(
        fs::metadata(fixture.public.join("child"))
            .expect("inspect restored child")
            .ino(),
        expected_inode
    );
    assert!(matching_names(&fixture.public, ".hypercolor-public-directory-recovery-").is_empty());
}

#[test]
fn successful_removal_retains_one_exact_empty_tombstone() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.public.join("child")).expect("create child");
    let expected_inode = fs::metadata(fixture.public.join("child"))
        .expect("inspect expected child")
        .ino();
    let (_lock, authority) = fixture.authority();
    let expected = observed_empty(&authority, "child");

    authority
        .durable_remove_empty_child_directory(Path::new("child"), &expected)
        .expect("durably remove child to exact tombstone");

    assert!(!fixture.public.join("child").exists());
    let names = matching_names(&fixture.public, ".hypercolor-public-directory-recovery-");
    assert_eq!(names.len(), 1);
    let tombstone = fixture.public.join(&names[0]);
    assert_eq!(
        fs::metadata(&tombstone)
            .expect("inspect exact tombstone")
            .ino(),
        expected_inode
    );
    assert_eq!(fs::read_dir(tombstone).expect("read tombstone").count(), 0);
}

#[test]
fn recovery_name_attack_never_moves_the_public_child() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.public.join("child")).expect("create child");
    let expected_inode = fs::metadata(fixture.public.join("child"))
        .expect("inspect expected child")
        .ino();
    let (_lock, authority) = fixture.authority();
    let expected = observed_empty(&authority, "child");
    authority
        .remove_empty_child_directory_with(
            Path::new("child"),
            &expected,
            |recovery, _| fs::create_dir(fixture.public.join(recovery)),
            || Ok(()),
            |directory| directory.sync_all(),
        )
        .expect_err("occupied reserved recovery name must fail closed");

    assert_eq!(
        fs::metadata(fixture.public.join("child"))
            .expect("inspect preserved child")
            .ino(),
        expected_inode
    );
    assert_eq!(
        matching_names(&fixture.public, ".hypercolor-public-directory-recovery-").len(),
        1
    );
}
