//! Adopting a real bundle on a real filesystem, with launchd and the owner
//! coordinator faked at their traits.

use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fakes::{Boundaries, SharedBoundaries, boundary, lock, survive};
use super::fixture::{INSTALLED_INVENTORY, UnitTree, reopen_tree, stage};
use crate::macos_bundle::native::MacosBundleNativeExecutor;
use crate::macos_bundle::{
    MACOS_BUNDLE_DAEMON_LABEL, MACOS_BUNDLE_INSTALLATION_FILE, MACOS_BUNDLE_STAGING_ROOT_NAME,
    MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL, MACOS_BUNDLE_UPDATE_PREPARE_LABEL,
    MACOS_BUNDLE_UPDATE_RECOVER_LABEL, MacosBundleAdoptionHost, MacosBundleAdoptionOutcome,
    MacosBundleAdoptionRequest, MacosBundleConfig, MacosBundleExecutor as _,
    MacosBundleInstallPlatform, MacosBundleInstallation, MacosBundleRoots,
    MacosBundleRunningDaemon, MacosBundleService, MacosBundleServiceObservation, MacosBundleSlot,
    adopt_macos_bundle, macos_bundle_daemon_agent, macos_bundle_update_agents,
};
use crate::{
    InstallCoordinator, InstallDisposition, InstallJournalV1, InstallLock, InstallOutcome,
    InstallPlatformError, InstallRequest, InstallStore, InstallTargetPolicy, InstallTransactionId,
    MAX_MANAGED_INSTALL_JOURNAL_BYTES, UnitId, UnitRecord, retain_macos_bundle_unit,
};

const VERSION: &str = "0.9.0";

/// The daemon launchd would run: whatever the installed bundle holds when it
/// starts, and it stays that code if the bundle is replaced under it.
#[derive(Debug)]
struct Launchd {
    daemon: PathBuf,
    definition: PathBuf,
    enabled: bool,
    running: Option<MacosBundleRunningDaemon>,
    next_pid: u32,
    failing_proof: bool,
    /// Every cdhash this launchd started.
    started: Vec<String>,
    proofs: Vec<Duration>,
}

type SharedLaunchd = Arc<Mutex<Launchd>>;

fn fail(detail: &str) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

fn installed_cdhash(daemon: &Path) -> String {
    let mut file = fs::File::open(daemon).expect("installed daemon");
    let size = file.metadata().expect("daemon size").len();
    crate::macho_cdhash::thin_macho_cdhash(&mut file, size).expect("daemon cdhash")
}

struct DiskService {
    launchd: SharedLaunchd,
    boundaries: SharedBoundaries,
}

impl MacosBundleService for DiskService {
    fn observe(&mut self) -> Result<MacosBundleServiceObservation, InstallPlatformError> {
        let launchd = lock(&self.launchd);
        Ok(MacosBundleServiceObservation {
            registered: launchd.definition.is_file(),
            enabled: launchd.enabled,
            running: launchd.running.clone(),
        })
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "enable");
        lock(&self.launchd).enabled = enabled;
        Ok(())
    }

    fn start(&mut self, cdhash: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "start:before");
        {
            let mut launchd = lock(&self.launchd);
            if !launchd.enabled {
                return Err(fail("launchd refuses to load a disabled agent"));
            }
            if !launchd.definition.is_file() {
                return Err(fail("launchd has no definition to load"));
            }
            if installed_cdhash(&launchd.daemon) != cdhash {
                return Err(fail("the installed bundle runs another daemon"));
            }
            launchd.running = Some(MacosBundleRunningDaemon {
                pid: launchd.next_pid,
                executable: launchd.daemon.clone(),
                cdhash: cdhash.to_owned(),
            });
            launchd.next_pid += 1;
            launchd.started.push(cdhash.to_owned());
        }
        boundary(&self.boundaries, "start:after");
        Ok(())
    }

    fn stop(
        &mut self,
        daemon: Option<&MacosBundleRunningDaemon>,
    ) -> Result<(), InstallPlatformError> {
        if lock(&self.launchd).running.as_ref() != daemon {
            return Err(fail("stop names another daemon"));
        }
        boundary(&self.boundaries, "stop");
        lock(&self.launchd).running = None;
        Ok(())
    }

    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn prove(
        &mut self,
        _daemon: &MacosBundleRunningDaemon,
        version: &str,
        probation: Duration,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "prove");
        let mut launchd = lock(&self.launchd);
        launchd.proofs.push(probation);
        if launchd.failing_proof || version.is_empty() {
            return Err(fail("the daemon never became ready"));
        }
        Ok(())
    }
}

#[derive(Default)]
struct HostLog {
    loaded: Vec<(String, bool)>,
    unloaded: Vec<String>,
    handovers: usize,
    refuse_handover: bool,
    /// Make this directory read-only as the handover ends, so every record
    /// written after it fails.
    seal_after_handover: Option<PathBuf>,
}

struct FakeHost {
    log: Arc<Mutex<HostLog>>,
    boundaries: SharedBoundaries,
}

impl MacosBundleAdoptionHost for FakeHost {
    fn load_agent(
        &mut self,
        label: &str,
        definition: &Path,
        replace: bool,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "load");
        assert!(definition.is_file(), "launchd reads a real path");
        lock(&self.log).loaded.push((label.to_owned(), replace));
        Ok(())
    }

    fn unload_agent(&mut self, label: &str) -> Result<(), InstallPlatformError> {
        lock(&self.log).unloaded.push(label.to_owned());
        Ok(())
    }

    fn hand_over_daemon(&mut self) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "handover");
        let mut log = lock(&self.log);
        if log.refuse_handover {
            return Err(fail("the owner coordinator rolled back"));
        }
        log.handovers += 1;
        if let Some(directory) = &log.seal_after_handover {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o500))
                .expect("seal the state root");
        }
        Ok(())
    }
}

struct Mac {
    root: tempfile::TempDir,
    store: InstallStore,
    roots: MacosBundleRoots,
    lock: InstallLock,
    launchd: SharedLaunchd,
    host: Arc<Mutex<HostLog>>,
    boundaries: SharedBoundaries,
}

impl Drop for Mac {
    fn drop(&mut self) {
        reopen_tree(self.root.path());
    }
}

impl Mac {
    /// A Mac whose user dragged `tree` into Applications and never ran it.
    fn with_bundle(tree: &UnitTree) -> Self {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("root");
        let roots = MacosBundleRoots {
            release_root: root.path().join("Hypercolor/releases"),
            state_root: root.path().join("Hypercolor/update"),
        };
        for directory in [&roots.release_root, &roots.state_root] {
            fs::create_dir_all(directory).expect("root directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private root");
        }
        fs::create_dir(root.path().join("Applications")).expect("applications");
        let store = InstallStore::with_roots(
            &roots.release_root,
            &roots.state_root,
            MAX_MANAGED_INSTALL_JOURNAL_BYTES,
        )
        .expect("store");
        let lock_handle = store.acquire_lock().expect("lock");
        let launchd = Arc::new(Mutex::new(Launchd {
            daemon: root
                .path()
                .join("Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon"),
            definition: root
                .path()
                .join(format!("LaunchAgents/{MACOS_BUNDLE_DAEMON_LABEL}.plist")),
            enabled: true,
            running: None,
            next_pid: 100,
            failing_proof: false,
            started: Vec::new(),
            proofs: Vec::new(),
        }));
        let mac = Self {
            root,
            store,
            roots,
            lock: lock_handle,
            launchd,
            host: Arc::new(Mutex::new(HostLog::default())),
            boundaries: Arc::new(Mutex::new(Boundaries::default())),
        };
        mac.drag_in(tree);
        mac
    }

    fn fresh() -> Self {
        Self::with_bundle(&UnitTree::new("dmg", "dmg", VERSION))
    }

    /// Replace the installed bundle the way Finder does.
    fn drag_in(&self, tree: &UnitTree) {
        let applications = self.applications();
        match fs::remove_dir_all(applications.join("Hypercolor.app")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove the old bundle: {error}"),
        }
        tree.write_installed(&applications);
    }

    fn applications(&self) -> PathBuf {
        self.root.path().join("Applications")
    }

    fn installed_bundle(&self) -> PathBuf {
        self.applications().join("Hypercolor.app")
    }

    fn config(&self) -> MacosBundleConfig {
        MacosBundleConfig {
            installed_bundle: self.installed_bundle(),
            release_root: self.roots.release_root.clone(),
            probation: Duration::ZERO,
        }
    }

    fn request(&self) -> MacosBundleAdoptionRequest {
        MacosBundleAdoptionRequest {
            config: self.config(),
            version: VERSION.to_owned(),
            launch_agents: self.root.path().join("LaunchAgents"),
            logs: self.root.path().join("Logs"),
            data_root: self.root.path().join("hypercolor"),
        }
    }

    fn service(&self) -> DiskService {
        DiskService {
            launchd: self.launchd.clone(),
            boundaries: self.boundaries.clone(),
        }
    }

    fn adopt(&self) -> Result<MacosBundleAdoptionOutcome, InstallPlatformError> {
        let mut executor = MacosBundleNativeExecutor::new(&self.config()).expect("executor");
        let mut host = FakeHost {
            log: self.host.clone(),
            boundaries: self.boundaries.clone(),
        };
        adopt_macos_bundle(
            &self.store,
            &self.lock,
            &mut executor,
            &mut self.service(),
            &mut host,
            &self.request(),
        )
    }

    fn adopted(&self) -> UnitId {
        match self.adopt().expect("adopt") {
            MacosBundleAdoptionOutcome::Adopted { unit } => unit,
            other => panic!("expected an adoption, not {other:?}"),
        }
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.roots.state_root.join(name)
    }

    fn installation(&self) -> Option<MacosBundleInstallation> {
        fs::read(self.state_file(MACOS_BUNDLE_INSTALLATION_FILE))
            .ok()
            .map(|bytes| MacosBundleInstallation::decode(&bytes).expect("installation"))
    }

    fn journal(&self) -> Option<InstallJournalV1> {
        self.store.load_journal(&self.lock).expect("journal")
    }

    fn active(&self) -> Option<UnitId> {
        self.store.active_unit(&self.lock).expect("active")
    }

    fn definitions(&self) -> Vec<String> {
        let mut names = fs::read_dir(self.root.path().join("LaunchAgents"))
            .map(|entries| {
                entries
                    .map(|entry| {
                        entry
                            .expect("entry")
                            .file_name()
                            .into_string()
                            .expect("utf-8")
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn retain(&self, unit: &UnitId) -> UnitRecord {
        retain_macos_bundle_unit(&self.store, &self.lock, unit).expect("retain unit")
    }

    fn platform(
        &self,
        units: Vec<UnitRecord>,
    ) -> MacosBundleInstallPlatform<MacosBundleNativeExecutor, DiskService> {
        let executor = MacosBundleNativeExecutor::new(&self.config()).expect("executor");
        MacosBundleInstallPlatform::new(executor, self.service(), self.config(), units)
            .expect("platform")
    }

    /// Run a real update to `candidate` through the coordinator.
    fn update_to(&mut self, active: &UnitId, candidate: &UnitRecord) -> InstallOutcome {
        let mut platform = self.platform(vec![self.retain(active), candidate.clone()]);
        InstallCoordinator::new(&self.store, &mut platform)
            .install_with_lock(
                InstallRequest {
                    transaction_id: InstallTransactionId::new("update").expect("transaction"),
                    candidate: candidate.clone(),
                    target_policy: InstallTargetPolicy::Preserve,
                },
                &mut self.lock,
            )
            .expect("update")
    }

    fn crash_at(&self, boundary: Option<usize>) {
        let mut boundaries = lock(&self.boundaries);
        boundaries.crash_at = boundary;
        boundaries.passed = 0;
        boundaries.log.clear();
    }

    /// What a finished adoption of `unit` must leave, whatever the path there.
    fn assert_adopted(&self, unit: &UnitId) {
        assert_eq!(self.active().as_ref(), Some(unit));
        let installation = self.installation().expect("installation.json");
        assert_eq!(installation.installed_bundle, self.installed_bundle());
        assert_eq!(installation.release_root, self.roots.release_root);
        assert_eq!(installation.state_root, self.roots.state_root);
        assert_eq!(installation.data_root, self.request().data_root);
        let journal = self.journal().expect("journal");
        assert_eq!(journal.disposition, InstallDisposition::Committed);
        assert_eq!(&journal.candidate_unit, unit);
        assert_eq!(journal.prior_active_unit, None);
        let retained = self.retain(unit);
        let launcher = fs::read(self.roots.launcher()).expect("launcher");
        assert_eq!(
            launcher,
            fs::read(
                self.store
                    .unit_path(unit)
                    .join("Hypercolor.app/Contents/MacOS/hypercolor")
            )
            .expect("the unit's CLI"),
            "the launcher is the unit's CLI, byte for byte"
        );
        assert_eq!(
            crate::verified_macos_bundle_cli(&self.roots.release_root, retained.id()).ok(),
            Some(
                self.store
                    .unit_path(unit)
                    .join("Hypercolor.app/Contents/MacOS/hypercolor")
            )
        );
        assert_eq!(
            fs::metadata(self.roots.launcher())
                .expect("launcher")
                .mode()
                & 0o7777,
            0o555
        );
        assert_eq!(
            self.definitions(),
            [
                format!("{MACOS_BUNDLE_DAEMON_LABEL}.plist"),
                format!("{MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL}.plist"),
                format!("{MACOS_BUNDLE_UPDATE_PREPARE_LABEL}.plist"),
                format!("{MACOS_BUNDLE_UPDATE_RECOVER_LABEL}.plist"),
            ]
        );
        assert!(
            !self.state_file("adoption").exists(),
            "the scratch copy is gone"
        );
        for directory in ["coordinator", "activator"] {
            assert_eq!(
                fs::metadata(self.state_file(directory))
                    .expect("state directory")
                    .mode()
                    & 0o7777,
                0o700
            );
        }
        let launchd = lock(&self.launchd);
        let running = launchd.running.as_ref().expect("the daemon runs");
        assert_eq!(running.cdhash, installed_cdhash(&launchd.daemon));
    }
}

fn bundle(seed: &str, version: &str) -> UnitTree {
    UnitTree::new(seed, seed, version)
}

#[test]
fn a_dragged_bundle_becomes_an_installation_with_its_record_last() {
    let mac = Mac::fresh();
    let unit = mac.adopted();
    mac.assert_adopted(&unit);

    let host = lock(&mac.host);
    assert_eq!(host.handovers, 1);
    assert_eq!(
        host.loaded,
        [
            (MACOS_BUNDLE_UPDATE_PREPARE_LABEL.to_owned(), true),
            (MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL.to_owned(), true),
            (MACOS_BUNDLE_UPDATE_RECOVER_LABEL.to_owned(), true),
        ]
    );
    assert_eq!(
        lock(&mac.launchd).proofs,
        [Duration::ZERO],
        "adoption proves the daemon without a probation window"
    );
    assert_eq!(
        fs::metadata(
            mac.root
                .path()
                .join(format!("LaunchAgents/{MACOS_BUNDLE_DAEMON_LABEL}.plist"))
        )
        .expect("definition")
        .mode()
            & 0o022,
        0,
        "launchd loads no definition another user could write"
    );
    assert_eq!(
        fs::read(
            mac.store
                .unit_path(&unit)
                .join("share/hypercolor/durable-stores.json")
        )
        .expect("the unit's inventory"),
        INSTALLED_INVENTORY,
        "the unit carries the inventory the bundle sealed"
    );
    assert_eq!(
        crate::bind_macos_bundle_unit(&mac.retain(&unit))
            .expect("bind")
            .version(),
        VERSION
    );
}

#[test]
fn the_next_launch_finds_nothing_to_do_and_starts_nothing() {
    let mac = Mac::fresh();
    let unit = mac.adopted();
    let before = mac.installation();
    mac.crash_at(None);

    assert_eq!(
        mac.adopt().expect("adopt"),
        MacosBundleAdoptionOutcome::Current
    );

    assert!(lock(&mac.boundaries).log.is_empty(), "no effect ran");
    assert_eq!(mac.installation(), before);
    mac.assert_adopted(&unit);
}

#[test]
fn a_crash_at_any_step_finishes_on_the_next_launch_without_a_second_identity() {
    let mut crash_at = 1;
    loop {
        let mac = Mac::fresh();
        mac.crash_at(Some(crash_at));
        if survive(|| mac.adopt()).is_ok() {
            assert!(crash_at > 3, "the fixture passes real boundaries");
            break;
        }
        assert!(
            mac.installation().is_none(),
            "nothing claims the installation at boundary {crash_at}"
        );
        // A power cut loses launchd's jobs; the next login reloads the agent
        // and starts what the definition names.
        lock(&mac.launchd).running = None;
        mac.crash_at(None);
        let unit = mac.adopted();
        mac.assert_adopted(&unit);
        crash_at += 1;
    }
}

#[test]
fn a_run_rebuilds_whatever_is_missing_and_keeps_the_identity_it_has() {
    type Damage = fn(&Mac, &UnitId);
    let damages: [(&str, Damage); 6] = [
        ("the journal", |mac, _| {
            fs::remove_file(mac.store.journal_path()).expect("journal");
        }),
        ("the active pointer", |mac, _| {
            mac.store.set_active(None, &mac.lock).expect("clear");
        }),
        ("the launcher", |mac, _| {
            fs::remove_file(mac.roots.launcher()).expect("launcher");
        }),
        ("the daemon agent", |mac, _| {
            fs::remove_file(
                mac.root
                    .path()
                    .join(format!("LaunchAgents/{MACOS_BUNDLE_DAEMON_LABEL}.plist")),
            )
            .expect("definition");
        }),
        ("the running daemon", |mac, _| {
            fs::remove_file(mac.store.journal_path()).expect("journal");
            lock(&mac.launchd).running = None;
        }),
        ("the record of a daemon launchd lost", |mac, _| {
            fs::remove_file(mac.store.journal_path()).expect("journal");
            lock(&mac.launchd).enabled = false;
            lock(&mac.launchd).running = None;
        }),
    ];
    for (name, damage) in damages {
        let mac = Mac::fresh();
        let unit = mac.adopted();
        let identity = mac.installation().expect("record").installation_id;
        damage(&mac, &unit);

        assert_eq!(mac.adopted(), unit, "{name}");

        mac.assert_adopted(&unit);
        assert_eq!(
            mac.installation().expect("record").installation_id,
            identity,
            "{name}"
        );
    }
}

#[test]
fn an_open_transaction_keeps_the_bundle_and_every_record() {
    let mac = Mac::fresh();
    let unit = mac.adopted();
    let next = stage(&mac.store, &mac.lock, &bundle("next", "0.9.1"));
    let mut platform = mac.platform(vec![mac.retain(&unit), next.clone()]);
    let journal = InstallCoordinator::new(&mac.store, &mut platform)
        .prepare_with_lock(
            InstallRequest {
                transaction_id: InstallTransactionId::new("update").expect("transaction"),
                candidate: next,
                target_policy: InstallTargetPolicy::Preserve,
            },
            &mac.lock,
        )
        .expect("the adopted installation prepares");
    mac.store
        .write_journal(&journal, &mac.lock)
        .expect("journal");
    mac.drag_in(&bundle("dragged", "0.9.2"));
    mac.crash_at(None);

    assert_eq!(
        mac.adopt().expect("adopt"),
        MacosBundleAdoptionOutcome::Deferred
    );

    assert!(lock(&mac.boundaries).log.is_empty());
    assert_eq!(mac.active(), Some(unit));
}

#[test]
fn a_bundle_dragged_over_an_update_is_adopted_and_the_stuck_terminal_journal_with_it() {
    let mut mac = Mac::fresh();
    let first = mac.adopted();
    let identity = mac.installation().expect("record").installation_id;

    let next = stage(&mac.store, &mac.lock, &bundle("next", "0.9.1"));
    let committed = mac.update_to(&first, &next);
    assert!(matches!(committed, InstallOutcome::Committed { .. }));
    assert!(
        !mac.applications()
            .join(MACOS_BUNDLE_STAGING_ROOT_NAME)
            .read_dir()
            .expect("the displaced bundle waits to be settled")
            .collect::<Vec<_>>()
            .is_empty()
    );

    mac.drag_in(&bundle("dragged", VERSION));
    let mut platform = mac.platform(vec![mac.retain(next.id())]);
    let refused = platform
        .settle(&mac.store, &mac.lock)
        .expect_err("the old transaction cannot settle against a bundle it never saw");
    assert!(refused.to_string().contains("differ"), "{refused}");
    drop(platform);

    let dragged = mac.adopted();

    assert_ne!(dragged, first);
    assert_ne!(&dragged, next.id());
    mac.assert_adopted(&dragged);
    assert_eq!(
        mac.installation().expect("record").installation_id,
        identity,
        "the installation stays the one plans are bound to"
    );
    assert!(
        fs::read_dir(mac.applications().join(MACOS_BUNDLE_STAGING_ROOT_NAME))
            .expect("staging root")
            .next()
            .is_none(),
        "nothing of the old transaction is left beside the bundle"
    );
    let mut platform = mac.platform(vec![mac.retain(&dragged)]);
    assert!(
        !platform
            .settle(&mac.store, &mac.lock)
            .expect("the adoption's journal settles"),
        "and there is nothing to remove"
    );
    drop(platform);

    let after = stage(&mac.store, &mac.lock, &bundle("after", "0.9.3"));
    assert!(matches!(
        mac.update_to(&dragged, &after),
        InstallOutcome::Committed { .. }
    ));
}

#[test]
fn a_replaced_bundle_restarts_a_daemon_still_running_the_old_code() {
    let mac = Mac::fresh();
    let first = mac.adopted();
    let old = lock(&mac.launchd).running.clone().expect("running");

    mac.drag_in(&bundle("dragged", VERSION));
    let dragged = mac.adopted();

    assert_ne!(dragged, first);
    let launchd = lock(&mac.launchd);
    let running = launchd.running.as_ref().expect("running");
    assert_ne!(running.pid, old.pid);
    assert_ne!(running.cdhash, old.cdhash);
    assert_eq!(running.cdhash, installed_cdhash(&launchd.daemon));
}

#[test]
fn an_update_runs_from_what_adoption_left() {
    let mut mac = Mac::fresh();
    let adopted = mac.adopted();
    let next = stage(&mac.store, &mac.lock, &bundle("next", "0.9.1"));

    let outcome = mac.update_to(&adopted, &next);

    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: next.id().clone()
        }
    );
    assert_eq!(mac.active().as_ref(), Some(next.id()));
    assert_eq!(
        lock(&mac.launchd)
            .running
            .as_ref()
            .map(|running| running.cdhash.clone()),
        Some(
            crate::bind_macos_bundle_unit(&next)
                .expect("bind")
                .identity()
                .daemon
                .clone()
        )
    );
}

#[test]
fn a_first_adoption_whose_daemon_fails_leaves_the_app_as_it_was() {
    let mac = Mac::fresh();
    lock(&mac.launchd).failing_proof = true;

    let failure = mac.adopt().expect_err("the proof fails");
    assert!(failure.to_string().contains("ready"), "{failure}");

    assert_eq!(mac.installation(), None);
    assert_eq!(mac.active(), None);
    assert_eq!(mac.journal(), None);
    assert!(
        mac.definitions().is_empty(),
        "no login starts a daemon that never proved itself"
    );
    assert_eq!(lock(&mac.launchd).running, None);
    assert_eq!(
        lock(&mac.host).unloaded,
        [
            MACOS_BUNDLE_UPDATE_PREPARE_LABEL,
            MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL,
            MACOS_BUNDLE_UPDATE_RECOVER_LABEL,
        ]
    );

    lock(&mac.launchd).failing_proof = false;
    let unit = mac.adopted();
    mac.assert_adopted(&unit);
}

#[test]
fn a_refused_handover_takes_the_first_adoption_back_out() {
    let mac = Mac::fresh();
    lock(&mac.host).refuse_handover = true;

    mac.adopt().expect_err("the handover fails");

    assert_eq!(mac.installation(), None);
    assert!(mac.definitions().is_empty());
    assert_eq!(lock(&mac.launchd).running, None);
}

#[test]
fn a_failure_after_the_handover_leaves_the_daemon_with_its_agent_for_the_next_launch() {
    let mac = Mac::fresh();
    lock(&mac.host).seal_after_handover = Some(mac.roots.state_root.clone());

    let failure = mac.adopt();
    fs::set_permissions(&mac.roots.state_root, fs::Permissions::from_mode(0o700))
        .expect("open the state root");
    failure.expect_err("the records cannot be written");

    assert_eq!(mac.installation(), None);
    assert_eq!(mac.active(), None);
    assert_eq!(
        mac.definitions().len(),
        4,
        "the supervisor now waits on this owner, so its agent stays"
    );
    assert!(lock(&mac.host).unloaded.is_empty());
    let running = lock(&mac.launchd).running.clone().expect("the daemon runs");

    lock(&mac.host).seal_after_handover = None;
    let unit = mac.adopted();

    mac.assert_adopted(&unit);
    assert_eq!(
        lock(&mac.launchd).running.as_ref(),
        Some(&running),
        "the next launch finishes the records around the daemon it finds"
    );
}

#[test]
fn a_definition_never_writes_through_a_planted_link() {
    let mac = Mac::fresh();
    let bait = mac.root.path().join("bait");
    fs::write(&bait, b"untouched").expect("bait");
    let agents = mac.root.path().join("LaunchAgents");
    fs::create_dir(&agents).expect("agents directory");
    std::os::unix::fs::symlink(
        &bait,
        agents.join(format!(
            "{MACOS_BUNDLE_DAEMON_LABEL}.plist.{}",
            std::process::id()
        )),
    )
    .expect("planted link");

    let unit = mac.adopted();

    mac.assert_adopted(&unit);
    assert_eq!(fs::read(&bait).expect("bait"), b"untouched");
}

#[test]
fn a_failed_readoption_keeps_the_installation_and_its_agents() {
    let mac = Mac::fresh();
    let first = mac.adopted();
    let identity = mac.installation().expect("record").installation_id;
    mac.drag_in(&bundle("dragged", VERSION));
    lock(&mac.launchd).failing_proof = true;

    mac.adopt().expect_err("the proof fails");

    assert_eq!(
        mac.installation().expect("record").installation_id,
        identity
    );
    assert_eq!(mac.definitions().len(), 4);
    assert_eq!(mac.active(), Some(first));
    lock(&mac.launchd).failing_proof = false;
    let dragged = mac.adopted();
    mac.assert_adopted(&dragged);
}

#[test]
fn another_installers_agent_is_left_alone() {
    let mac = Mac::fresh();
    let definition = mac
        .root
        .path()
        .join(format!("LaunchAgents/{MACOS_BUNDLE_DAEMON_LABEL}.plist"));
    fs::create_dir_all(definition.parent().expect("agents")).expect("agents directory");
    fs::write(&definition, b"<plist>the raw install</plist>").expect("foreign definition");

    let failure = mac.adopt().expect_err("foreign agent");
    assert!(failure.to_string().contains("unmanaged"), "{failure}");

    assert_eq!(
        fs::read(&definition).expect("definition"),
        b"<plist>the raw install</plist>"
    );
    assert_eq!(mac.active(), None);
    assert_eq!(mac.installation(), None);
}

#[test]
fn a_daemon_the_agent_runs_from_elsewhere_is_left_alone() {
    let mac = Mac::fresh();
    lock(&mac.launchd).running = Some(MacosBundleRunningDaemon {
        pid: 7,
        executable: PathBuf::from("/Users/u/.local/bin/hypercolor-daemon"),
        cdhash: "a".repeat(40),
    });

    let failure = mac.adopt().expect_err("foreign daemon");
    assert!(failure.to_string().contains("unmanaged"), "{failure}");

    assert_eq!(mac.active(), None);
    assert_eq!(
        lock(&mac.launchd)
            .running
            .as_ref()
            .map(|running| running.pid),
        Some(7)
    );
}

#[test]
fn a_group_writable_bundle_becomes_a_unit_with_no_group_write_and_stays_as_found() {
    let mac = Mac::fresh();
    let bundle_path = mac.installed_bundle();
    let info = bundle_path.join("Contents/Info.plist");
    for (path, mode) in [(&bundle_path, 0o775), (&info, 0o664)] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("group write");
    }

    let unit = mac.adopted();

    mac.assert_adopted(&unit);
    assert_eq!(
        fs::metadata(&bundle_path).expect("bundle").mode() & 0o7777,
        0o775
    );
    assert_eq!(fs::metadata(&info).expect("info").mode() & 0o7777, 0o664);
    let unit_info = mac
        .store
        .unit_path(&unit)
        .join("Hypercolor.app/Contents/Info.plist");
    assert_eq!(
        fs::metadata(&unit_info).expect("unit info").mode() & 0o7777,
        0o444
    );
}

#[test]
fn a_bundle_holding_a_link_is_not_adopted() {
    let mac = Mac::fresh();
    std::os::unix::fs::symlink("Info.plist", mac.installed_bundle().join("Contents/link"))
        .expect("link");

    let failure = mac.adopt().expect_err("a link");
    assert!(failure.to_string().contains("neither"), "{failure}");

    assert_eq!(mac.active(), None);
    assert!(
        !mac.state_file("adoption").exists(),
        "the scratch copy is gone"
    );
}

#[test]
fn a_bundle_without_a_sealed_inventory_is_not_adopted() {
    let mac = Mac::fresh();
    fs::remove_file(
        mac.installed_bundle()
            .join(crate::macos_bundle::MACOS_BUNDLE_INVENTORY),
    )
    .expect("inventory");

    let failure = mac.adopt().expect_err("no inventory");
    assert!(failure.to_string().contains("inventory"), "{failure}");
    assert_eq!(mac.installation(), None);
}

#[test]
fn a_store_that_is_not_the_requests_is_refused() {
    let mac = Mac::fresh();
    let mut request = mac.request();
    request.config.release_root = mac.root.path().join("elsewhere");
    let mut executor = MacosBundleNativeExecutor::new(&mac.config()).expect("executor");
    let mut host = FakeHost {
        log: mac.host.clone(),
        boundaries: mac.boundaries.clone(),
    };
    let failure = adopt_macos_bundle(
        &mac.store,
        &mac.lock,
        &mut executor,
        &mut mac.service(),
        &mut host,
        &request,
    )
    .expect_err("another store");
    assert!(failure.to_string().contains("release root"), "{failure}");
}

#[test]
fn the_agents_run_the_installed_bundle_and_the_launcher_and_survive_any_path() {
    let config = MacosBundleConfig {
        installed_bundle: PathBuf::from("/Users/r&d/Applications/Hypercolor.app"),
        release_root: PathBuf::from("/Users/r&d/Library/Application Support/Hypercolor/releases"),
        probation: Duration::ZERO,
    };
    let logs = Path::new("/Users/r&d/Library/Logs/hypercolor");
    let daemon = String::from_utf8(
        macos_bundle_daemon_agent(&config, logs)
            .expect("daemon agent")
            .bytes,
    )
    .expect("utf-8");
    for expected in [
        "<string>/Users/r&amp;d/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon</string>",
        "<string>--macos-owner</string><string>direct-launchd</string>",
        "<key>RunAtLoad</key><true/>",
        "<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>",
        "<string>user_service:launchd:tech.hyperbliss.hypercolor</string>",
        "<key>AssociatedBundleIdentifiers</key><string>tech.hyperbliss.hypercolor</string>",
    ] {
        assert!(daemon.contains(expected), "{expected}\n{daemon}");
    }

    let launcher = config.release_root.join("launcher");
    let pending = Path::new("/Users/r&d/Library/Application Support/Hypercolor/update/pending");
    let [prepare, activate, recover] =
        macos_bundle_update_agents(&launcher, pending, logs).expect("update agents");
    for (definition, mode, label) in [
        (&prepare, "prepare", MACOS_BUNDLE_UPDATE_PREPARE_LABEL),
        (&activate, "activate", MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL),
        (&recover, "recover", MACOS_BUNDLE_UPDATE_RECOVER_LABEL),
    ] {
        assert_eq!(definition.label, label);
        let text = std::str::from_utf8(&definition.bytes).expect("utf-8");
        let program = format!(
            "<string>/Users/r&amp;d/Library/Application Support/Hypercolor/releases/launcher\
             </string><string>--role</string><string>update-executor</string>\
             <string>--</string><string>update</string><string>activator</string>\
             <string>{mode}</string>"
        );
        assert!(text.contains(&program), "{text}");
        assert!(
            !text.contains("RunAtLoad"),
            "only a kickstart or the marker starts it"
        );
    }
    for definition in [&prepare, &activate] {
        let text = std::str::from_utf8(&definition.bytes).expect("utf-8");
        assert!(!text.contains("KeepAlive"), "{text}");
    }
    let recover = std::str::from_utf8(&recover.bytes).expect("utf-8");
    assert!(
        recover.contains(
            "<key>KeepAlive</key><dict><key>PathState</key><dict>\
             <key>/Users/r&amp;d/Library/Application Support/Hypercolor/update/pending</key>\
             <true/></dict></dict>"
        ),
        "{recover}"
    );
}

#[test]
fn a_refused_swap_says_who_refused_it() {
    let refused =
        crate::macos_bundle::native::swap_refused("swap the bundles")(rustix::io::Errno::PERM);
    assert!(
        refused.to_string().contains("signed by the app's own team"),
        "{refused}"
    );
    let full =
        crate::macos_bundle::native::swap_refused("swap the bundles")(rustix::io::Errno::NOSPC);
    assert!(!full.to_string().contains("signed"), "{full}");
}

#[test]
fn only_an_applications_folder_holds_an_installation() {
    let home = Path::new("/Users/u");
    for (executable, expected) in [
        (
            "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-app",
            Some("/Applications/Hypercolor.app"),
        ),
        (
            "/Users/u/Applications/Hypercolor.app/Contents/MacOS/hypercolor-app",
            Some("/Users/u/Applications/Hypercolor.app"),
        ),
        (
            "/Volumes/Hypercolor/Hypercolor.app/Contents/MacOS/hypercolor-app",
            None,
        ),
        (
            "/Users/u/Downloads/Hypercolor.app/Contents/MacOS/hypercolor-app",
            None,
        ),
        (
            "/Applications/Hypercolor.app/Contents/MacOS/hypercolor",
            None,
        ),
        (
            "/Applications/Other.app/Contents/MacOS/hypercolor-app",
            None,
        ),
    ] {
        assert_eq!(
            crate::macos_bundle::macos_bundle_in_applications(Path::new(executable), home),
            expected.map(PathBuf::from),
            "{executable}"
        );
    }
}

#[test]
fn the_installed_bundle_is_only_read() {
    let mac = Mac::fresh();
    let before = fs::metadata(mac.installed_bundle()).expect("bundle").ino();
    mac.adopted();
    assert_eq!(
        fs::metadata(mac.installed_bundle()).expect("bundle").ino(),
        before
    );
    let mut executor = MacosBundleNativeExecutor::new(&mac.config()).expect("executor");
    assert!(matches!(
        executor.observe().expect("observe").installed,
        MacosBundleSlot::Bundle(_)
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn quarantine_on_the_dragged_bundle_reaches_neither_the_unit_nor_the_launcher() {
    use std::process::Command;

    let mac = Mac::fresh();
    let cli = mac.installed_bundle().join("Contents/MacOS/hypercolor");
    let attribute = "com.apple.quarantine";
    let marked = Command::new("/usr/bin/xattr")
        .args(["-w", attribute, "0081;00000000;Safari;"])
        .arg(&cli)
        .status()
        .expect("xattr");
    assert!(marked.success());

    let unit = mac.adopted();

    for path in [
        mac.roots.launcher(),
        mac.store
            .unit_path(&unit)
            .join("Hypercolor.app/Contents/MacOS/hypercolor"),
    ] {
        let listed = Command::new("/usr/bin/xattr")
            .arg(&path)
            .output()
            .expect("xattr");
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(!listed.contains(attribute), "{}: {listed}", path.display());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn every_rendered_agent_is_a_property_list_launchd_can_read() {
    use std::process::Command;

    let mac = Mac::fresh();
    mac.adopted();
    for name in mac.definitions() {
        let path = mac.root.path().join("LaunchAgents").join(&name);
        let linted = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .status()
            .expect("plutil");
        assert!(linted.success(), "{name}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn the_constants_shared_with_the_owner_crate_agree() {
    assert_eq!(
        MACOS_BUNDLE_DAEMON_LABEL,
        hypercolor_macos_owner::MACOS_DIRECT_LAUNCHD_LABEL
    );
    assert_eq!(
        "pending",
        hypercolor_macos_owner::MACOS_UPDATE_PENDING_FILE_NAME
    );
    let support = Path::new("/Users/u/Library/Application Support");
    assert!(
        MacosBundleRoots::of_state_root(&hypercolor_macos_owner::macos_update_state_root(support))
            .is_some()
    );
}
