//! Which release the launcher runs, against real units in a real store.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::fixture::{UnitTree, reopen_tree, stage};
use crate::macos_bundle::launcher::update_executor_command;
use crate::macos_bundle::{
    MacosBundleInstallation, MacosBundleLauncherError, MacosBundleRoots,
    select_macos_bundle_update_executor,
};
use crate::release_selection::ReleaseSelectionError;
use crate::{InstallLock, InstallStore, MAX_MANAGED_INSTALL_JOURNAL_BYTES, UnitRecord};

struct Records {
    root: tempfile::TempDir,
    roots: MacosBundleRoots,
    store: InstallStore,
    lock: InstallLock,
    prior: UnitRecord,
    candidate: UnitRecord,
}

impl Drop for Records {
    fn drop(&mut self) {
        reopen_tree(self.root.path());
    }
}

impl Records {
    /// A store holding two units, with the prior active.
    fn new() -> Self {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("root");
        let roots = MacosBundleRoots {
            release_root: root.path().join("releases"),
            state_root: root.path().join("update"),
        };
        for directory in [&roots.release_root, &roots.state_root] {
            fs::create_dir(directory).expect("root directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private root");
        }
        let store = InstallStore::with_roots(
            &roots.release_root,
            &roots.state_root,
            MAX_MANAGED_INSTALL_JOURNAL_BYTES,
        )
        .expect("store");
        let lock = store.acquire_lock().expect("lock");
        let prior = stage(&store, &lock, &UnitTree::new("prior", "prior", "0.9.0"));
        let candidate = stage(&store, &lock, &UnitTree::new("next", "next", "0.9.1"));
        store
            .set_active(Some(prior.id()), &lock)
            .expect("activate the prior");
        Self {
            root,
            roots,
            store,
            lock,
            prior,
            candidate,
        }
    }

    fn cli_of(&self, unit: &UnitRecord) -> PathBuf {
        self.roots
            .release_root
            .join("units")
            .join(unit.id().as_str())
            .join("Hypercolor.app/Contents/MacOS/hypercolor")
    }

    /// Write an open journal over `candidate`, its active pointer already
    /// moved, the way the transaction looks from outside after its switch.
    fn open_transaction(&self, prior: Option<&UnitRecord>) {
        fs::write(
            self.store.journal_path(),
            serde_json::json!({
                "disposition": "forward",
                "prior_active_unit": prior.map(|unit| unit.id().as_str()),
                "candidate_unit": self.candidate.id().as_str(),
            })
            .to_string(),
        )
        .expect("journal");
        self.store
            .set_active(Some(self.candidate.id()), &self.lock)
            .expect("activate the candidate");
    }
}

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn with_nothing_open_the_active_release_runs() {
    let records = Records::new();
    assert_eq!(
        select_macos_bundle_update_executor(&records.roots).expect("select"),
        records.cli_of(&records.prior)
    );
}

#[test]
fn an_open_transaction_runs_its_prior_whatever_the_pointer_names() {
    let records = Records::new();
    records.open_transaction(Some(&records.prior));
    assert_eq!(
        select_macos_bundle_update_executor(&records.roots).expect("select"),
        records.cli_of(&records.prior)
    );
}

#[test]
fn a_prior_whose_cli_changed_never_runs_and_nothing_runs_in_its_place() {
    let records = Records::new();
    records.open_transaction(Some(&records.prior));
    let cli = records.cli_of(&records.prior);
    fs::set_permissions(
        cli.parent().expect("MacOS"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("open the unit directory");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("open the CLI");
    fs::write(&cli, b"not the program the manifest records").expect("alter the CLI");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o555)).expect("close the CLI");
    assert!(matches!(
        select_macos_bundle_update_executor(&records.roots),
        Err(MacosBundleLauncherError::Unverified(_))
    ));
}

#[test]
fn an_open_first_install_has_no_release_to_run() {
    let records = Records::new();
    records.open_transaction(None);
    assert!(matches!(
        select_macos_bundle_update_executor(&records.roots),
        Err(MacosBundleLauncherError::Selection(
            ReleaseSelectionError::NoPrior
        ))
    ));
}

#[test]
fn a_pointer_that_names_no_unit_is_refused() {
    let records = Records::new();
    records
        .store
        .set_active(None, &records.lock)
        .expect("clear");
    assert!(matches!(
        select_macos_bundle_update_executor(&records.roots),
        Err(MacosBundleLauncherError::Selection(
            ReleaseSelectionError::NoActive
        ))
    ));
    std::os::unix::fs::symlink("units", records.store.active_path()).expect("short link");
    assert!(matches!(
        select_macos_bundle_update_executor(&records.roots),
        Err(MacosBundleLauncherError::Selection(
            ReleaseSelectionError::InvalidPointer(_)
        ))
    ));
}

#[test]
fn the_role_takes_exactly_its_own_grammar() {
    let good = arguments(&[
        "--role",
        "update-executor",
        "--",
        "update",
        "activator",
        "recover",
    ]);
    assert_eq!(
        update_executor_command(&good),
        Some(&good[3..]),
        "the command passes through untouched"
    );
    for wrong in [
        arguments(&["--role", "update-executor"]),
        arguments(&["--role", "update-executor", "--"]),
        arguments(&["--role", "daemon", "--", "update"]),
        arguments(&["--role", "update-executor", "update"]),
    ] {
        assert_eq!(update_executor_command(&wrong), None, "{wrong:?}");
    }
}

#[test]
fn the_launcher_finds_its_roots_from_where_it_stands() {
    let support = Path::new("/Users/u/Library/Application Support/Hypercolor");
    let roots = MacosBundleRoots {
        release_root: support.join("releases"),
        state_root: support.join("update"),
    };
    assert_eq!(
        MacosBundleRoots::of_launcher(&roots.launcher()),
        Some(roots.clone())
    );
    assert_eq!(
        MacosBundleRoots::of_state_root(&roots.state_root),
        Some(roots.clone())
    );
    for elsewhere in [
        "/usr/local/bin/hypercolor",
        "/Applications/Hypercolor.app/Contents/MacOS/hypercolor",
        "/tmp/launcher",
    ] {
        assert_eq!(MacosBundleRoots::of_launcher(Path::new(elsewhere)), None);
    }
    assert_eq!(MacosBundleRoots::of_state_root(support), None);
}

#[test]
fn an_installation_record_survives_only_as_this_build_wrote_it() {
    let roots = MacosBundleRoots {
        release_root: PathBuf::from("/support/releases"),
        state_root: PathBuf::from("/support/update"),
    };
    let record = MacosBundleInstallation::new(
        uuid::Uuid::from_u128(7),
        PathBuf::from("/Applications/Hypercolor.app"),
        &roots,
        PathBuf::from("/support/hypercolor"),
    );
    let bytes = record.encode().expect("encode");
    assert_eq!(MacosBundleInstallation::decode(&bytes), Ok(record));

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    value["launcher_contract"] = serde_json::json!(2);
    MacosBundleInstallation::decode(value.to_string().as_bytes()).expect_err("another contract");
    value["launcher_contract"] = serde_json::json!(1);
    value["installation_id"] = serde_json::json!(uuid::Uuid::nil());
    MacosBundleInstallation::decode(value.to_string().as_bytes()).expect_err("a nil identity");
    value["installation_id"] = serde_json::json!(uuid::Uuid::from_u128(7));
    value["unknown"] = serde_json::json!(true);
    MacosBundleInstallation::decode(value.to_string().as_bytes()).expect_err("an unknown field");
}
