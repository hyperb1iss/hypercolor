use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use super::child::require_same_filesystem;
use super::exact::MAX_EXACT_ENTRY_BYTES;
use super::operation::{remove_entry_with, replace_entry_with};
use super::staging::stage_replacement_with;
use crate::unix::tree::{EntryReplacement, ExactEntry, ExclusiveDirectory};

struct Fixture {
    _temporary: tempfile::TempDir,
    lock_root: PathBuf,
    public: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary directory");
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

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

fn matching_names(path: &Path, prefix: &str) -> Vec<OsString> {
    fs::read_dir(path)
        .expect("read public directory")
        .filter_map(|entry| {
            let name = entry.expect("read directory entry").file_name();
            name.to_string_lossy().starts_with(prefix).then_some(name)
        })
        .collect()
}

fn only_stage(path: &Path) -> PathBuf {
    let names = matching_names(path, ".hypercolor-public-stage-");
    assert_eq!(names.len(), 1, "one staged entry must exist at the hook");
    path.join(&names[0])
}

#[test]
fn parent_replacement_during_each_visibility_phase_fails_closed() {
    for after_visibility in [false, true] {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let detached = fixture.public.with_extension("detached");
        let replace_parent = || {
            fs::rename(&fixture.public, &detached)?;
            fs::create_dir(&fixture.public)
        };
        let result = replace_entry_with(
            &authority,
            OsStr::new("entry"),
            &ExactEntry::Absent,
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"published",
            },
            || {
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
        );

        result.expect_err("renamed public parent must fail closed");
        assert!(!fixture.public.join("entry").exists());
        assert!(!detached.join("entry").exists());
    }
}

#[test]
fn staged_name_replacement_and_hardlink_attacks_never_publish_or_delete_attacker_state() {
    for hardlink_attack in [false, true] {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let result = replace_entry_with(
            &authority,
            OsStr::new("entry"),
            &ExactEntry::Absent,
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"trusted",
            },
            || {
                let stage = only_stage(&fixture.public);
                if hardlink_attack {
                    fs::hard_link(&stage, fixture.public.join("attacker-hardlink"))?;
                } else {
                    fs::remove_file(&stage)?;
                    symlink("attacker-target", &stage)?;
                }
                Ok(())
            },
            || Ok(()),
            |directory| directory.sync_all(),
        );

        result.expect_err("staged entry drift must fail closed");
        assert!(!fixture.public.join("entry").exists());
        let stage = only_stage(&fixture.public);
        if hardlink_attack {
            assert!(fixture.public.join("attacker-hardlink").exists());
            assert_eq!(
                fs::read(stage).expect("read retained attacker link"),
                b"trusted"
            );
        } else {
            assert_eq!(
                fs::read_link(stage).expect("read retained attacker symlink"),
                Path::new("attacker-target")
            );
        }
    }
}

#[test]
fn construction_time_stage_attacks_are_never_accepted_or_deleted() {
    for (symlink_stage, hardlink_attack) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let fixture = Fixture::new();
        let attacker_link = fixture.public.join("attacker-hardlink");
        let replacement = if symlink_stage {
            EntryReplacement::Symlink {
                target: Path::new("trusted-target"),
            }
        } else {
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"trusted",
            }
        };
        let result = stage_replacement_with(
            &File::open(&fixture.public).expect("open public"),
            replacement,
            |name| {
                let stage = fixture.public.join(name);
                if hardlink_attack {
                    fs::hard_link(&stage, &attacker_link)?;
                } else {
                    fs::remove_file(&stage)?;
                    symlink("attacker-target", &stage)?;
                }
                Ok(())
            },
        );

        result.expect_err("construction-time stage attack must fail closed");
        let stage = only_stage(&fixture.public);
        if hardlink_attack {
            assert!(attacker_link.symlink_metadata().is_ok());
        } else {
            assert_eq!(
                fs::read_link(stage).expect("read preserved attacker symlink"),
                Path::new("attacker-target")
            );
        }
    }
}

#[test]
fn destination_drift_during_exchange_is_reversed_without_deleting_drift() {
    for drift in ["kind", "inode", "mode", "size", "digest"] {
        let fixture = Fixture::new();
        let destination = fixture.public.join("entry");
        write_mode(&destination, b"before", 0o644);
        let (_lock, authority) = fixture.authority();
        let expected = authority
            .observe_entry(Path::new("entry"))
            .expect("observe expected entry");
        let result = replace_entry_with(
            &authority,
            OsStr::new("entry"),
            &expected,
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"trusted",
            },
            || {
                match drift {
                    "kind" => {
                        fs::remove_file(&destination)?;
                        symlink("attacker", &destination)?;
                    }
                    "inode" => {
                        let attacker = fixture.public.join("attacker");
                        write_mode(&attacker, b"before", 0o644);
                        fs::rename(attacker, &destination)?;
                    }
                    "mode" => fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?,
                    "size" => fs::write(&destination, b"longer")?,
                    "digest" => fs::write(&destination, b"differ")?,
                    _ => unreachable!(),
                }
                Ok(())
            },
            || Ok(()),
            |directory| directory.sync_all(),
        );

        result.expect_err("destination drift must fail closed");
        if drift == "kind" {
            assert_eq!(
                fs::read_link(&destination).expect("read restored drift symlink"),
                Path::new("attacker")
            );
        } else {
            assert_ne!(
                fs::read(&destination).expect("read restored drift"),
                b"trusted"
            );
        }
        assert!(matching_names(&fixture.public, ".hypercolor-recovery-").is_empty());
    }
}

#[test]
fn symlink_target_drift_during_exchange_is_reversed() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    symlink("before", &destination).expect("create expected symlink");
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected symlink");
    replace_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        EntryReplacement::Symlink {
            target: Path::new("trusted"),
        },
        || {
            fs::remove_file(&destination)?;
            symlink("attacker", &destination)
        },
        || Ok(()),
        |directory| directory.sync_all(),
    )
    .expect_err("target drift must fail closed");

    assert_eq!(
        fs::read_link(destination).expect("read restored target drift"),
        Path::new("attacker")
    );
}

#[test]
fn destination_swap_after_visibility_is_quarantined_before_restoring_expected_entry() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    write_mode(&destination, b"expected", 0o644);
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected entry");
    let error = replace_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        EntryReplacement::RegularFile {
            mode: 0o644,
            contents: b"trusted",
        },
        || Ok(()),
        || {
            fs::remove_file(&destination)?;
            write_mode(&destination, b"attacker", 0o644);
            Ok(())
        },
        |directory| directory.sync_all(),
    )
    .expect_err("post-visibility destination swap must fail closed");

    assert!(error.to_string().contains("quarantined"));
    assert_eq!(
        fs::read(&destination).expect("read restored entry"),
        b"expected"
    );
    let quarantined = matching_names(&fixture.public, ".hypercolor-recovery-");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        fs::read(fixture.public.join(&quarantined[0])).expect("read quarantined attacker"),
        b"attacker"
    );
}

#[test]
fn oversized_destination_swap_after_visibility_is_quarantined() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    write_mode(&destination, b"expected", 0o644);
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected entry");
    replace_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        EntryReplacement::RegularFile {
            mode: 0o644,
            contents: b"trusted",
        },
        || Ok(()),
        || {
            fs::remove_file(&destination)?;
            let attacker = File::create(&destination)?;
            attacker.set_len(MAX_EXACT_ENTRY_BYTES + 1)
        },
        |directory| directory.sync_all(),
    )
    .expect_err("oversized post-visibility swap must fail closed");

    assert_eq!(
        fs::read(&destination).expect("read restored entry"),
        b"expected"
    );
    let quarantined = matching_names(&fixture.public, ".hypercolor-recovery-");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        fs::symlink_metadata(fixture.public.join(&quarantined[0]))
            .expect("inspect quarantined oversized attacker")
            .len(),
        MAX_EXACT_ENTRY_BYTES + 1
    );
}

#[test]
fn displaced_stage_swap_after_visibility_never_becomes_public() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    write_mode(&destination, b"expected", 0o644);
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected entry");
    let error = replace_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        EntryReplacement::RegularFile {
            mode: 0o644,
            contents: b"trusted",
        },
        || Ok(()),
        || {
            let stage = only_stage(&fixture.public);
            fs::remove_file(&stage)?;
            write_mode(&stage, b"attacker", 0o644);
            Ok(())
        },
        |directory| directory.sync_all(),
    )
    .expect_err("post-visibility displaced-stage swap must fail closed");

    assert!(
        error
            .to_string()
            .contains("exact replacement remains published")
    );
    assert_eq!(
        fs::read(&destination).expect("read public entry"),
        b"trusted"
    );
    let quarantined = matching_names(&fixture.public, ".hypercolor-recovery-");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        fs::read(fixture.public.join(&quarantined[0])).expect("read quarantined attacker"),
        b"attacker"
    );
}

#[test]
fn removal_destination_swap_is_quarantined_and_expected_entry_is_restored() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    write_mode(&destination, b"expected", 0o644);
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected entry");
    let result = remove_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        || Ok(()),
        || {
            fs::remove_file(&destination)?;
            write_mode(&destination, b"attacker", 0o644);
            Ok(())
        },
        |directory| directory.sync_all(),
    );

    let error = result.expect_err("swapped removal destination must fail closed");
    assert!(error.to_string().contains("quarantined"));
    assert_eq!(
        fs::read(&destination).expect("read restored entry"),
        b"expected"
    );
    let quarantined = matching_names(&fixture.public, ".hypercolor-recovery-");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        fs::read(fixture.public.join(&quarantined[0])).expect("read quarantined attacker"),
        b"attacker"
    );
}

#[test]
fn removal_destination_drift_before_visibility_is_preserved() {
    let fixture = Fixture::new();
    let destination = fixture.public.join("entry");
    write_mode(&destination, b"expected", 0o644);
    let (_lock, authority) = fixture.authority();
    let expected = authority
        .observe_entry(Path::new("entry"))
        .expect("observe expected entry");
    remove_entry_with(
        &authority,
        OsStr::new("entry"),
        &expected,
        || {
            fs::remove_file(&destination)?;
            write_mode(&destination, b"attacker", 0o644);
            Ok(())
        },
        || Ok(()),
        |directory| directory.sync_all(),
    )
    .expect_err("pre-visibility removal drift must fail closed");

    assert_eq!(
        fs::read(destination).expect("read preserved drift"),
        b"attacker"
    );
    assert!(matching_names(&fixture.public, ".hypercolor-recovery-").is_empty());
}

#[test]
fn parent_sync_failure_before_and_after_visibility_restores_absence() {
    for fail_call in [1, 2] {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let calls = Cell::new(0_u8);
        let result = replace_entry_with(
            &authority,
            OsStr::new("entry"),
            &ExactEntry::Absent,
            EntryReplacement::RegularFile {
                mode: 0o644,
                contents: b"trusted",
            },
            || Ok(()),
            || Ok(()),
            |_| {
                let call = calls.get() + 1;
                calls.set(call);
                if call == fail_call {
                    Err(io::Error::other("injected parent sync failure"))
                } else {
                    Ok(())
                }
            },
        );

        result.expect_err("parent sync failure must fail closed");
        assert!(!fixture.public.join("entry").exists());
        assert!(matching_names(&fixture.public, ".hypercolor-public-stage-").is_empty());
        assert!(matching_names(&fixture.public, ".hypercolor-recovery-").is_empty());
    }
}

#[test]
fn child_creation_detects_parent_replacement_during_mutation() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let detached = fixture.public.with_extension("detached");
    authority
        .create_child_directory_with(
            Path::new("child"),
            0o755,
            || {
                fs::rename(&fixture.public, &detached)?;
                fs::create_dir(&fixture.public)
            },
            || Ok(()),
            |directory| directory.sync_all(),
        )
        .expect_err("parent replacement must fail child creation");

    assert!(!fixture.public.join("child").exists());
    assert!(!detached.join("child").exists());
}

#[test]
fn child_staging_name_swap_is_detected_before_publication() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    authority
        .create_child_directory_with(
            Path::new("child"),
            0o755,
            || {
                let names =
                    matching_names(&fixture.lock_root, ".hypercolor-public-directory-stage-");
                assert_eq!(names.len(), 1);
                let stage = fixture.lock_root.join(&names[0]);
                fs::rename(&stage, fixture.lock_root.join("detached-created"))?;
                fs::create_dir(&stage)
            },
            || Ok(()),
            |directory| directory.sync_all(),
        )
        .expect_err("staged child name swap must fail closed");

    assert!(!fixture.public.join("child").exists());
    assert!(fixture.lock_root.join("detached-created").is_dir());
    let staged = matching_names(&fixture.lock_root, ".hypercolor-public-directory-stage-");
    assert_eq!(staged.len(), 1, "attacker directory must remain untouched");
}

#[test]
fn changed_child_after_visibility_remains_public_and_never_enters_private_authority() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let error = authority
        .create_child_directory_with(
            Path::new("child"),
            0o755,
            || Ok(()),
            || fs::write(fixture.public.join("child/attacker"), b"attacker"),
            |directory| directory.sync_all(),
        )
        .expect_err("changed child must not be rolled into private authority");

    assert!(error.to_string().contains("remains untouched"));
    assert_eq!(
        fs::read(fixture.public.join("child/attacker")).expect("read public attacker entry"),
        b"attacker"
    );
    assert!(matching_names(&fixture.lock_root, ".hypercolor-public-directory-stage-").is_empty());
}

#[test]
fn child_publication_rejects_cross_filesystem_devices_before_staging() {
    require_same_filesystem(7, 9).expect_err("cross-filesystem staging must fail preflight");
    require_same_filesystem(7, 7).expect("same-filesystem staging must pass preflight");
}

#[test]
fn child_parent_sync_failures_before_and_after_visibility_restore_absence() {
    for fail_call in [1, 2, 3] {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let calls = Cell::new(0_u8);
        authority
            .create_child_directory_with(
                Path::new("child"),
                0o755,
                || Ok(()),
                || Ok(()),
                |_| {
                    let call = calls.get() + 1;
                    calls.set(call);
                    if call == fail_call {
                        Err(io::Error::other("injected child parent sync failure"))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("every child parent sync failure must fail closed");

        assert!(!fixture.public.join("child").exists());
        assert!(
            matching_names(&fixture.lock_root, ".hypercolor-public-directory-stage-").is_empty()
        );
    }
}
