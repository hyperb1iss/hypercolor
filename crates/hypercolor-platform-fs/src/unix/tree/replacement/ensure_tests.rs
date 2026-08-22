use std::cell::Cell;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use rustix::fs::Mode;

use crate::unix::tree::{ExclusiveDirectory, PublicDirectoryAuthority};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailurePhase {
    AfterMkdir,
    AfterMode,
    AfterChildSync,
    ParentSync,
}

impl FailurePhase {
    fn error(self, phase: Self) -> io::Result<()> {
        if self == phase {
            Err(io::Error::other("injected monotone ensure failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn every_post_create_failure_retains_a_healable_directory() {
    for phase in [
        FailurePhase::AfterMkdir,
        FailurePhase::AfterMode,
        FailurePhase::AfterChildSync,
        FailurePhase::ParentSync,
    ] {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let sync_calls = Cell::new(0_u8);
        authority
            .ensure_child_directory_with(
                Path::new("scaffold"),
                0o755,
                || phase.error(FailurePhase::AfterMkdir),
                || phase.error(FailurePhase::AfterMode),
                || phase.error(FailurePhase::AfterChildSync),
                |directory| {
                    let call = sync_calls.get() + 1;
                    sync_calls.set(call);
                    if phase == FailurePhase::ParentSync && call == 2 {
                        return Err(io::Error::other("injected monotone ensure failure"));
                    }
                    directory.sync_all()
                },
            )
            .expect_err("injected ensure failure must propagate");

        assert!(fixture.public.join("scaffold").is_dir());
        let child = authority
            .durable_ensure_child_directory(Path::new("scaffold"), 0o755)
            .expect("replay must heal retained scaffold");
        assert_eq!(
            fs::symlink_metadata(fixture.public.join("scaffold"))
                .expect("inspect healed scaffold")
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );
        child
            .validate_ancestry()
            .expect("healed authority must retain exact ancestry");
    }
}

#[test]
fn parent_replacement_after_creation_fails_without_deleting_the_scaffold() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let detached = fixture.public.with_extension("detached");
    authority
        .ensure_child_directory_with(
            Path::new("scaffold"),
            0o755,
            || Ok(()),
            || {
                fs::rename(&fixture.public, &detached)?;
                fs::create_dir(&fixture.public)
            },
            || Ok(()),
            |directory| directory.sync_all(),
        )
        .expect_err("replaced parent ancestry must be rejected");

    assert!(detached.join("scaffold").is_dir());
    assert!(!fixture.public.join("scaffold").exists());
}

#[test]
fn destination_swap_after_open_is_detected_without_deletion() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    let detached = fixture.public.join("detached-scaffold");
    authority
        .ensure_child_directory_with(
            Path::new("scaffold"),
            0o755,
            || Ok(()),
            || {
                fs::rename(fixture.public.join("scaffold"), &detached)?;
                fs::write(fixture.public.join("scaffold"), b"replacement")
            },
            || Ok(()),
            |directory| directory.sync_all(),
        )
        .expect_err("destination swap must fail identity proof");

    assert!(detached.is_dir());
    assert_eq!(
        fs::read(fixture.public.join("scaffold")).expect("read preserved replacement"),
        b"replacement"
    );
}

#[test]
fn unsupported_names_modes_and_entry_kinds_are_rejected() {
    let fixture = Fixture::new();
    let (_lock, authority) = fixture.authority();
    fs::write(fixture.public.join("file"), b"file").expect("create regular file");
    symlink("file", fixture.public.join("link")).expect("create symlink");
    fs::create_dir(fixture.public.join("wrong-mode")).expect("create wrong-mode directory");
    fs::set_permissions(
        fixture.public.join("wrong-mode"),
        fs::Permissions::from_mode(0o750),
    )
    .expect("set wrong safe mode");
    fs::create_dir(fixture.public.join("special-mode")).expect("create special-mode directory");
    fs::set_permissions(
        fixture.public.join("special-mode"),
        fs::Permissions::from_mode(0o1700),
    )
    .expect("set sticky mode");

    for name in ["file", "link", "wrong-mode", "special-mode"] {
        authority
            .durable_ensure_child_directory(Path::new(name), 0o755)
            .expect_err("unsupported existing state must be rejected");
    }
    authority
        .durable_ensure_child_directory(Path::new("nested/name"), 0o755)
        .expect_err("nested names must be rejected");
    for mode in [0o655, 0o1755, 0o4755] {
        authority
            .durable_ensure_child_directory(Path::new("unsafe-mode"), mode)
            .expect_err("unsafe requested mode must be rejected");
    }
    assert!(!fixture.public.join("unsafe-mode").exists());
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn restrictive_umask_creation_is_normalized_and_durable() {
    const CHILD_ENV: &str = "HYPERCOLOR_ENSURE_DIRECTORY_UMASK_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        let fixture = Fixture::new();
        let (_lock, authority) = fixture.authority();
        let prior_umask = rustix::process::umask(Mode::from_raw_mode(0o777));
        let interrupted = authority.ensure_child_directory_with(
            Path::new("scaffold"),
            0o755,
            || Err(io::Error::other("injected crash after mkdir")),
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        );
        let replay = authority.durable_ensure_child_directory(Path::new("scaffold"), 0o755);
        rustix::process::umask(prior_umask);
        interrupted.expect_err("injected crash must interrupt restrictive creation");
        replay.expect("replay under restrictive umask");
        assert_eq!(
            fs::symlink_metadata(fixture.public.join("scaffold"))
                .expect("inspect scaffold")
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );
        return;
    }

    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "unix::tree::replacement::ensure_tests::restrictive_umask_creation_is_normalized_and_durable",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("run isolated restrictive-umask test");
    assert!(
        output.status.success(),
        "restrictive-umask child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
