//! The native executor on a real filesystem.
//!
//! On macOS these run on the APFS volume holding the temporary directory,
//! so they exercise `renamex_np` with `RENAME_SWAP`, `clonefile` and
//! `F_FULLFSYNC` themselves. Lib tests on Linux run the same code over
//! `renameat2` and plain copies.

use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::coordinator_tests::identity_of;
use super::fakes::{Boundaries, SharedBoundaries, boundary, lock, survive};
use super::fixture::Fixture;
use crate::macos_bundle::native::MacosBundleNativeExecutor;
use crate::macos_bundle::{
    MACOS_BUNDLE_STAGING_ROOT_NAME, MacosBundleConfig, MacosBundleExecutor,
    MacosBundleInstallPlatform, MacosBundleMaterialization, MacosBundleObservation,
    MacosBundleRunningDaemon, MacosBundleService, MacosBundleServiceObservation, MacosBundleSlot,
    MacosBundleUnit,
};
use crate::{
    InstallCoordinator, InstallOutcome, InstallPlatformError, InstallRequest, InstallTargetPolicy,
    InstallTransactionId, UnitRecord,
};

/// Files in a fixture bundle: a plist, an icon, a seal and three programs.
const BUNDLE_FILES: u64 = 6;

fn bound(unit: &UnitRecord) -> MacosBundleUnit {
    crate::payload::bind_macos_bundle_unit(unit).expect("bind unit")
}

fn executor(fixture: &Fixture) -> MacosBundleNativeExecutor {
    MacosBundleNativeExecutor::new(&fixture.config()).expect("native executor")
}

/// Put the prior's bundle in place the way a first install would.
fn install_prior(fixture: &Fixture) {
    let mut executor = executor(fixture);
    let name = fixture.prior.id().as_str();
    executor
        .materialize(&fixture.prior, &bound(&fixture.prior), name)
        .expect("materialize the prior");
    executor
        .install_staged(name)
        .expect("move the prior into place");
    executor
        .discard(name)
        .expect("remove the empty staging name");
}

fn staging_entries(fixture: &Fixture) -> Vec<String> {
    let mut names = fs::read_dir(fixture.applications().join(MACOS_BUNDLE_STAGING_ROOT_NAME))
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

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("metadata").mode() & 0o7777
}

#[test]
fn materializes_a_verified_copy_and_swaps_it_into_place() {
    let fixture = Fixture::new();
    install_prior(&fixture);
    let (prior, candidate) = (identity_of(&fixture.prior), identity_of(&fixture.candidate));
    let mut executor = executor(&fixture);
    let name = fixture.candidate.id().as_str();
    let observed = executor.observe().expect("observe");
    assert_eq!(observed.installed, MacosBundleSlot::Bundle(prior.clone()));
    assert!(observed.staged.is_empty() && observed.interrupted.is_empty());

    let made = executor
        .materialize(&fixture.candidate, &bound(&fixture.candidate), name)
        .expect("materialize");
    assert_eq!(made.cloned_files + made.copied_files, BUNDLE_FILES);
    #[cfg(target_os = "macos")]
    assert_eq!(made.cloned_files, BUNDLE_FILES, "APFS clones every file");
    let staged = fixture.config().staged_bundle(name);
    assert_eq!(
        mode(&staged.join("Contents/MacOS/hypercolor-daemon")),
        0o755
    );
    assert_eq!(mode(&staged.join("Contents/Info.plist")), 0o644);
    assert_eq!(mode(&staged), 0o755);
    assert_eq!(
        executor.observe().expect("observe").staged_slot(name),
        &MacosBundleSlot::Bundle(candidate.clone())
    );

    let installed_inode = fs::metadata(&fixture.config().installed_bundle)
        .expect("installed")
        .ino();
    executor.exchange(name).expect("exchange");
    let swapped = executor.observe().expect("observe");
    assert_eq!(swapped.installed, MacosBundleSlot::Bundle(candidate));
    assert_eq!(swapped.staged_slot(name), &MacosBundleSlot::Bundle(prior));
    assert_eq!(
        fs::metadata(&staged).expect("displaced").ino(),
        installed_inode,
        "the displaced bundle is the same directory, moved"
    );

    executor.discard(name).expect("discard");
    assert!(staging_entries(&fixture).is_empty());
    let settled = executor.observe().expect("observe");
    assert!(settled.staged.is_empty() && settled.interrupted.is_empty());
}

#[test]
fn refuses_a_tampered_unit_and_leaves_no_copy() {
    let fixture = Fixture::new();
    install_prior(&fixture);
    let unit = bound(&fixture.candidate);
    let icon = fixture
        .store
        .unit_path(fixture.candidate.id())
        .join("Hypercolor.app/Contents/Resources/icon.icns");
    fs::set_permissions(&icon, fs::Permissions::from_mode(0o644)).expect("unlock");
    fs::write(&icon, b"icon candidaTE").expect("tamper, same size");
    let mut executor = executor(&fixture);
    let error = executor
        .materialize(&fixture.candidate, &unit, fixture.candidate.id().as_str())
        .expect_err("a copy that differs from its unit");
    assert!(error.to_string().contains("differs"), "{error}");
    assert!(
        staging_entries(&fixture).is_empty(),
        "{:?}",
        staging_entries(&fixture)
    );
}

#[test]
fn leftovers_never_count_and_are_cleared_by_the_next_step() {
    let fixture = Fixture::new();
    install_prior(&fixture);
    let name = fixture.candidate.id().as_str();
    let root = fixture.applications().join(MACOS_BUNDLE_STAGING_ROOT_NAME);
    let partial = root.join(format!("{name}.partial/Hypercolor.app/Contents"));
    let discard = root.join(format!("{name}.discard/Hypercolor.app"));
    fs::create_dir_all(&partial).expect("partial");
    fs::create_dir_all(&discard).expect("discard");
    fs::write(partial.join("torn"), b"half").expect("torn file");
    fs::write(discard.join("left"), b"left").expect("leftover file");
    fs::set_permissions(&discard, fs::Permissions::from_mode(0o555)).expect("lock leftover");
    let mut executor = executor(&fixture);
    let observed = executor.observe().expect("observe");
    assert!(observed.staged.is_empty());
    assert_eq!(observed.interrupted.len(), 2);

    executor
        .materialize(&fixture.candidate, &bound(&fixture.candidate), name)
        .expect("materialize over leftovers");
    assert_eq!(staging_entries(&fixture), vec![name.to_owned()]);
}

#[test]
fn never_follows_a_link_in_the_staging_root() {
    let fixture = Fixture::new();
    install_prior(&fixture);
    let name = fixture.candidate.id().as_str();
    let elsewhere = fixture.root.path().join("elsewhere");
    fs::create_dir_all(elsewhere.join("Hypercolor.app")).expect("decoy");
    std::os::unix::fs::symlink(
        &elsewhere,
        fixture
            .applications()
            .join(MACOS_BUNDLE_STAGING_ROOT_NAME)
            .join(name),
    )
    .expect("planted link");
    let mut executor = executor(&fixture);
    assert!(matches!(
        executor.observe().expect("observe").staged_slot(name),
        MacosBundleSlot::Unrecognized(_)
    ));
    assert!(executor.exchange(name).is_err());
    assert_eq!(
        executor.observe().expect("observe").installed,
        MacosBundleSlot::Bundle(identity_of(&fixture.prior))
    );
}

#[test]
fn moves_into_and_out_of_an_empty_slot() {
    let fixture = Fixture::new();
    let mut executor = executor(&fixture);
    let name = fixture.candidate.id().as_str();
    executor
        .materialize(&fixture.candidate, &bound(&fixture.candidate), name)
        .expect("materialize");
    assert!(
        executor.exchange(name).is_err(),
        "nothing installed to swap with"
    );
    executor.install_staged(name).expect("install");
    assert!(executor.install_staged(name).is_err(), "never replaces");
    executor.withdraw_installed(name).expect("withdraw");
    let observed = executor.observe().expect("observe");
    assert!(observed.installed.is_absent());
    assert_eq!(
        observed.staged_slot(name),
        &MacosBundleSlot::Bundle(identity_of(&fixture.candidate))
    );
}

/// A service that runs whatever daemon the installed bundle holds on disk.
struct DiskService {
    config: MacosBundleConfig,
    boundaries: SharedBoundaries,
    state: Arc<Mutex<DiskServiceState>>,
}

struct DiskServiceState {
    running: Option<MacosBundleRunningDaemon>,
    next_pid: u32,
    failing_proof: Option<String>,
}

impl DiskService {
    fn installed_daemon(&self) -> MacosBundleRunningDaemon {
        let path = self.config.installed_daemon();
        let mut file = fs::File::open(&path).expect("installed daemon");
        let size = file.metadata().expect("size").len();
        let cdhash = crate::macho_cdhash::thin_macho_cdhash(&mut file, size).expect("cdhash");
        let mut state = lock(&self.state);
        state.next_pid += 1;
        MacosBundleRunningDaemon {
            pid: state.next_pid,
            executable: path,
            cdhash,
        }
    }
}

impl MacosBundleService for DiskService {
    fn observe(&mut self) -> Result<MacosBundleServiceObservation, InstallPlatformError> {
        Ok(MacosBundleServiceObservation {
            registered: true,
            enabled: true,
            running: lock(&self.state).running.clone(),
        })
    }

    fn set_enabled(&mut self, _enabled: bool) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn start(&mut self, cdhash: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "start:before");
        let daemon = self.installed_daemon();
        assert_eq!(daemon.cdhash, cdhash, "the installed bundle's daemon");
        lock(&self.state).running = Some(daemon);
        boundary(&self.boundaries, "start:after");
        Ok(())
    }

    fn stop(
        &mut self,
        daemon: Option<&MacosBundleRunningDaemon>,
    ) -> Result<(), InstallPlatformError> {
        let Some(daemon) = daemon else {
            return Ok(());
        };
        boundary(&self.boundaries, "stop:before");
        {
            let mut state = lock(&self.state);
            assert_eq!(state.running.as_ref(), Some(daemon));
            state.running = None;
        }
        boundary(&self.boundaries, "stop:after");
        Ok(())
    }

    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn prove(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        _version: &str,
        _probation: Duration,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "prove");
        if lock(&self.state).failing_proof.as_deref() == Some(daemon.cdhash.as_str()) {
            return Err(InstallPlatformError::new("not ready"));
        }
        Ok(())
    }
}

struct DiskHarness {
    fixture: Fixture,
    boundaries: SharedBoundaries,
    service: Arc<Mutex<DiskServiceState>>,
}

impl DiskHarness {
    fn new(failing: bool) -> Self {
        let fixture = Fixture::new();
        install_prior(&fixture);
        let prior_daemon = MacosBundleRunningDaemon {
            pid: 100,
            executable: fixture.config().installed_daemon(),
            cdhash: identity_of(&fixture.prior).daemon,
        };
        let failing_proof = failing.then(|| identity_of(&fixture.candidate).daemon);
        Self {
            fixture,
            boundaries: Arc::new(Mutex::new(Boundaries::default())),
            service: Arc::new(Mutex::new(DiskServiceState {
                running: Some(prior_daemon),
                next_pid: 100,
                failing_proof,
            })),
        }
    }

    fn platform(
        &self,
    ) -> MacosBundleInstallPlatform<Crashing<MacosBundleNativeExecutor>, DiskService> {
        MacosBundleInstallPlatform::new(
            Crashing {
                inner: executor(&self.fixture),
                boundaries: Arc::clone(&self.boundaries),
            },
            DiskService {
                config: self.fixture.config(),
                boundaries: Arc::clone(&self.boundaries),
                state: Arc::clone(&self.service),
            },
            self.fixture.config(),
            [
                self.fixture.retain(self.fixture.prior.id()),
                self.fixture.retain(self.fixture.candidate.id()),
            ],
        )
        .expect("platform")
    }

    fn run(&self, crash_at: Option<usize>) -> InstallOutcome {
        lock(&self.boundaries).crash_at = crash_at;
        let request = InstallRequest {
            transaction_id: InstallTransactionId::new("disk-transaction").expect("id"),
            candidate: self.fixture.candidate.clone(),
            target_policy: InstallTargetPolicy::Preserve,
        };
        let mut platform = self.platform();
        let mut outcome = survive(|| {
            self.fixture.with_lock(|lock| {
                InstallCoordinator::new(&self.fixture.store, &mut platform)
                    .install_with_lock(request, lock)
                    .map(Some)
            })
        });
        for _ in 0..16 {
            if let Ok(result) = outcome {
                let outcome = result.expect("transaction").expect("terminal");
                if survive(|| self.fixture.settle(self.platform())).is_err() {
                    lock(&self.boundaries).crash_at = None;
                    self.fixture.settle(self.platform()).expect("settle again");
                }
                return outcome;
            }
            lock(&self.boundaries).crash_at = None;
            let mut platform = self.platform();
            outcome = survive(|| {
                self.fixture.with_lock(|lock| {
                    InstallCoordinator::new(&self.fixture.store, &mut platform)
                        .recover_with_lock(lock)
                })
            });
        }
        panic!("recovery never converged");
    }

    fn assert_installed(&self, unit: &UnitRecord) {
        let mut executor = executor(&self.fixture);
        let observed = executor.observe().expect("observe");
        assert_eq!(
            observed.installed,
            MacosBundleSlot::Bundle(identity_of(unit))
        );
        assert!(observed.staged.is_empty() && observed.interrupted.is_empty());
        assert!(staging_entries(&self.fixture).is_empty());
        assert_eq!(self.fixture.active_unit().as_ref(), Some(unit.id()));
        assert_eq!(
            lock(&self.service)
                .running
                .as_ref()
                .map(|daemon| daemon.cdhash.clone()),
            Some(identity_of(unit).daemon)
        );
    }
}

#[test]
fn transaction_recovers_from_a_crash_at_every_boundary() {
    let total = {
        let clean = DiskHarness::new(false);
        assert!(matches!(clean.run(None), InstallOutcome::Committed { .. }));
        clean.assert_installed(&clean.fixture.candidate);
        lock(&clean.boundaries).passed
    };
    for crash in 1..=total {
        let harness = DiskHarness::new(false);
        let outcome = harness.run(Some(crash));
        assert!(
            matches!(outcome, InstallOutcome::Committed { .. }),
            "crash {crash}: {outcome:?}"
        );
        harness.assert_installed(&harness.fixture.candidate);
    }
}

#[test]
fn rollback_recovers_from_a_crash_at_every_boundary() {
    let total = {
        let clean = DiskHarness::new(true);
        assert!(matches!(clean.run(None), InstallOutcome::RolledBack { .. }));
        clean.assert_installed(&clean.fixture.prior);
        lock(&clean.boundaries).passed
    };
    for crash in 1..=total {
        let harness = DiskHarness::new(true);
        let outcome = harness.run(Some(crash));
        assert!(
            matches!(
                outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "crash {crash}: {outcome:?}"
        );
        harness.assert_installed(&harness.fixture.prior);
    }
}

/// Adds numbered boundaries before and after every effect of a real
/// executor, so a test can crash a native transaction at each of them.
struct Crashing<T> {
    inner: T,
    boundaries: SharedBoundaries,
}

impl<T> Crashing<T> {
    fn around<R>(&mut self, label: &str, effect: impl FnOnce(&mut T) -> R) -> R {
        boundary(&self.boundaries, &format!("{label}:before"));
        let result = effect(&mut self.inner);
        boundary(&self.boundaries, &format!("{label}:after"));
        result
    }
}

impl<T: MacosBundleExecutor> MacosBundleExecutor for Crashing<T> {
    fn observe(&mut self) -> Result<MacosBundleObservation, InstallPlatformError> {
        self.inner.observe()
    }

    fn available_bytes(&mut self) -> Result<u64, InstallPlatformError> {
        self.inner.available_bytes()
    }

    fn materialize(
        &mut self,
        source: &UnitRecord,
        unit: &MacosBundleUnit,
        name: &str,
    ) -> Result<MacosBundleMaterialization, InstallPlatformError> {
        self.around("materialize", |inner| inner.materialize(source, unit, name))
    }

    fn snapshot_installed(&mut self, destination: &Path) -> Result<(), InstallPlatformError> {
        self.inner.snapshot_installed(destination)
    }

    fn exchange(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        self.around("exchange", |inner| inner.exchange(name))
    }

    fn install_staged(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        self.around("install", |inner| inner.install_staged(name))
    }

    fn withdraw_installed(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        self.around("withdraw", |inner| inner.withdraw_installed(name))
    }

    fn retire_staged(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        self.around("retire", |inner| inner.retire_staged(name))
    }

    fn discard(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        self.around("discard", |inner| inner.discard(name))
    }
}
