//! The installation's launcher is a copy of this CLI, so the role has to work
//! from a real copy of the real binary, before clap sees the arguments.

#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use hypercolor_install::{
    InstallStore, MAX_MANAGED_INSTALL_JOURNAL_BYTES, UnitId, describe_macos_bundle_unit,
    stage_macos_bundle_unit,
};

const RECORDER: &str = "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > \"$HYPERCOLOR_TEST_RECORD\"\n";

struct Installation {
    root: tempfile::TempDir,
    launcher: PathBuf,
    record: PathBuf,
    unit: UnitId,
    store: InstallStore,
}

/// A release root holding one active unit whose CLI records how it ran, with
/// this test's CLI copied in as the launcher.
fn installation() -> Installation {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("root");
    let product = root.path().join("Hypercolor");
    let release_root = product.join("releases");
    let state_root = product.join("update");
    for directory in [&release_root, &state_root] {
        fs::create_dir_all(directory).expect("root directory");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).expect("private root");
    }
    let tree = root.path().join("unit");
    write_unit(&tree);
    let digest = describe_macos_bundle_unit(&tree, "0.9.0").expect("describe");
    let store = InstallStore::with_roots(
        &release_root,
        &state_root,
        MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    )
    .expect("store");
    let lock = store.acquire_lock().expect("lock");
    let unit = stage_macos_bundle_unit(&store, &lock, &tree, &digest).expect("stage");
    store.set_active(Some(unit.id()), &lock).expect("activate");
    drop(lock);

    let launcher = release_root.join("launcher");
    fs::copy(env!("CARGO_BIN_EXE_hypercolor"), &launcher).expect("copy the CLI");
    Installation {
        record: root.path().join("record"),
        launcher,
        unit: unit.id().clone(),
        store,
        root,
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        reopen_tree(self.root.path());
    }
}

/// Units hold no write bits, which a temporary directory cannot remove.
fn reopen_tree(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            drop(fs::set_permissions(
                entry.path(),
                fs::Permissions::from_mode(0o700),
            ));
            reopen_tree(&entry.path());
        }
    }
}

fn write_unit(tree: &Path) {
    let programs = tree.join("Hypercolor.app/Contents/MacOS");
    fs::create_dir_all(&programs).expect("bundle");
    for (name, body) in [
        ("hypercolor", RECORDER),
        ("hypercolor-daemon", "#!/bin/sh\n"),
        ("hypercolor-app", "#!/bin/sh\n"),
    ] {
        let program = programs.join(name);
        fs::write(&program, body).expect("program");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let inventory = tree.join("share/hypercolor/durable-stores.json");
    fs::create_dir_all(inventory.parent().expect("share")).expect("share");
    fs::write(inventory, br#"{"stores":[]}"#).expect("inventory");
}

#[test]
fn the_copied_cli_becomes_the_active_release_with_the_command_untouched() {
    let installation = installation();

    let status = Command::new(&installation.launcher)
        .args(["--role", "update-executor", "--"])
        .args(["update", "activator", "recover"])
        .env("HYPERCOLOR_TEST_RECORD", &installation.record)
        .status()
        .expect("launcher");

    assert!(status.success());
    let cli = installation
        .store
        .unit_path(&installation.unit)
        .join("Hypercolor.app/Contents/MacOS/hypercolor");
    assert_eq!(
        fs::read_to_string(&installation.record).expect("record"),
        format!("{}\nupdate\nactivator\nrecover\n", cli.display())
    );
}

#[test]
fn the_role_runs_only_from_an_installation() {
    let status = Command::new(env!("CARGO_BIN_EXE_hypercolor"))
        .args(["--role", "update-executor", "--", "update"])
        .status()
        .expect("cli");
    assert_eq!(status.code(), Some(64));
}

#[test]
fn a_launcher_asked_for_anything_else_runs_nothing() {
    let installation = installation();

    let status = Command::new(&installation.launcher)
        .args(["--role", "daemon", "--", "update"])
        .env("HYPERCOLOR_TEST_RECORD", &installation.record)
        .status()
        .expect("launcher");

    assert_eq!(status.code(), Some(64));
    assert!(!installation.record.exists());
}
