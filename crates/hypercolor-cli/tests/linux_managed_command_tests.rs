#![cfg(target_os = "linux")]

//! Command-level topology, adoption, recovery and uninstall coverage.
//!
//! These suites drive the exact orchestration behind `__install-release` and
//! `__uninstall-release` through a simulated systemd user manager. Real
//! stores, recorded roots, locks, journals and immutable units live on disk;
//! only service, launcher and public-layout effects are modelled, and the
//! simulated daemon runs whatever file its launcher names.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_cli::install::{
    DEFAULT_PROBATION_WINDOW, DirectoryRefusal, InstallAction, InstallCoordinator,
    InstallDisposition, InstallJournalV1, InstallLock, InstallOutcome, InstallPlatformError,
    InstallRequest, InstallStore, InstallStoreError, InstallTargetPolicy, InstallTransactionId,
    LINUX_LAYOUT_ITEMS, LinuxDirectoryItem, LinuxDirectoryState, LinuxExactEntry,
    LinuxFilePublication, LinuxHttpResponse, LinuxInstallCheckpoint, LinuxInstallCommandError,
    LinuxInstallConfig, LinuxInstallElection, LinuxInstallExecutor, LinuxInstallHost,
    LinuxInstallLocation, LinuxInstallObservation, LinuxInstallPlatform, LinuxInstallRequest,
    LinuxLaunchError, LinuxLaunchPlan, LinuxLaunchRequest, LinuxLaunchRole, LinuxLaunchSelection,
    LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacyFile, LinuxLocatorError,
    LinuxObservationError, LinuxPlatformInputs, LinuxProcessExecutable, LinuxPublicTree,
    LinuxRuntimeSettlement, LinuxServiceIdentity, LinuxServiceWatch, LinuxUninstallCheckpoint,
    LinuxUninstallHost, OwnershipPolicy, PlatformTransactionRecord, PrincipalDatabase,
    PrincipalGroup, PrincipalUser, RestoredRelease, UnitCollection, UnitId, UnitRecord,
    bind_linux_platform, elect_linux_installation_with, ensure_linux_launcher,
    ensure_linux_update_directories, linux_layout_directories, observe_linux_installation,
    plan_linux_launch, prepare_linux_launch_roots, run_linux_install, run_linux_recovery,
    run_linux_uninstall, stage_release_payload,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

const MAX_JOURNAL: usize = 64 * 1024;

// ── Principal databases ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Principals {
    users: Vec<PrincipalUser>,
    groups: Vec<PrincipalGroup>,
    failing: bool,
}

impl Principals {
    fn private(uid: u32, gid: u32) -> Self {
        Self {
            users: vec![
                PrincipalUser {
                    name: "root".to_owned(),
                    uid: 0,
                    primary_gid: 0,
                },
                PrincipalUser {
                    name: "installer".to_owned(),
                    uid,
                    primary_gid: gid,
                },
                PrincipalUser {
                    name: "neighbor".to_owned(),
                    uid: uid.wrapping_add(1),
                    primary_gid: gid.wrapping_add(1),
                },
            ],
            groups: vec![
                PrincipalGroup {
                    name: "installer".to_owned(),
                    gid,
                    members: Vec::new(),
                },
                PrincipalGroup {
                    name: "neighbor".to_owned(),
                    gid: gid.wrapping_add(1),
                    members: Vec::new(),
                },
            ],
            failing: false,
        }
    }

    fn shared(uid: u32, gid: u32) -> Self {
        let mut principals = Self::private(uid, gid);
        principals.groups[0].members = vec!["installer".to_owned(), "neighbor".to_owned()];
        principals
    }

    fn unreachable(uid: u32, gid: u32) -> Self {
        Self {
            failing: true,
            ..Self::private(uid, gid)
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.failing {
            Err(io::Error::other("the directory service is unreachable"))
        } else {
            Ok(())
        }
    }
}

impl PrincipalDatabase for Principals {
    fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>> {
        self.check()?;
        Ok(self.users.iter().find(|user| user.uid == uid).cloned())
    }
    fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>> {
        self.check()?;
        Ok(self.groups.iter().find(|group| group.gid == gid).cloned())
    }
    fn all_users(&self) -> io::Result<Vec<PrincipalUser>> {
        self.check()?;
        Ok(self.users.clone())
    }
    fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>> {
        self.check()?;
        Ok(self.groups.clone())
    }
}

fn policy(principals: Principals) -> OwnershipPolicy {
    OwnershipPolicy::with_private_groups(Arc::new(principals))
}

// ── Simulated systemd user session ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crash {
    Before(usize),
    After(usize),
}

#[derive(Debug, Clone)]
struct Process {
    path: String,
    sha256: String,
    device: u64,
    inode: u64,
    arguments: Vec<String>,
}

#[derive(Debug)]
struct World {
    home: PathBuf,
    fragment: String,
    legacy_units: PathBuf,
    launcher: LinuxExactEntry,
    launcher_bytes: Vec<u8>,
    layout: BTreeMap<LinuxLayoutItem, LinuxExactEntry>,
    loaded: bool,
    active: bool,
    enabled: bool,
    exec_start: String,
    pid: u32,
    last_pid: u32,
    invocation: u32,
    process: Option<Process>,
    historical: Vec<UnitRecord>,
    effects: Vec<String>,
    crash: Option<Crash>,
    fault: Option<String>,
    /// A start job the manager queued on its own (login autostart); it runs
    /// when the installer settles the service.
    queued_start: bool,
    /// Waiting to restart after a crash: `activating/auto-restart`.
    auto_restart: bool,
    /// Ended in failure: `failed/failed`.
    failed: bool,
    /// Versions whose daemon never reaches readiness, so every start job
    /// for them fails and the service waits to restart.
    failing_starts: BTreeSet<String>,
    /// A failing start ends `failed` (its start limit hit) instead of
    /// waiting to restart.
    failing_start_ends_failed: bool,
    /// systemd before 254: a start job for a service that never becomes
    /// ready stays queued across every automatic restart, so the installer's
    /// fence cancels it and the next restart queues another.
    start_job_persists: bool,
    resets: usize,
    shows: usize,
    crash_at_show: Option<usize>,
    /// What happens when the installer next asks the daemon of this version
    /// for `/health`: the installer dies, or the daemon crashes under it.
    at_health: Option<(&'static str, HealthEvent)>,
    /// What happens while the installer next holds the daemon of this
    /// version through its probation window.
    in_probation: Option<(&'static str, ProbationEvent)>,
    /// Every probation window held: the running version, the identity it
    /// was held to, and the window.
    watches: Vec<(String, LinuxServiceIdentity, Duration)>,
    /// The process is lost right after the next effect of this name.
    crash_after_effect: Option<String>,
    /// A daemon of this version starts with these arguments instead of the
    /// ones its unit gives it.
    argument_override: Option<(String, Vec<String>)>,
}

/// An interruption this long into a probation window. One at or past the
/// window's end happens after the installer stopped watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbationEvent {
    /// The daemon crashes and `Restart=on-failure` brings it back.
    Crashes(Duration),
    /// The installer dies mid-window; the daemon keeps running.
    InstallerDies(Duration),
    /// Power is lost mid-window; the user manager comes back and queues an
    /// enabled service's start from disk.
    PowerCut(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthEvent {
    InstallerDies,
    Restarts,
    CrashLoops,
}

type Shared = Rc<RefCell<World>>;

impl World {
    fn new(home: &Path) -> Shared {
        Rc::new(RefCell::new(Self {
            home: home.to_path_buf(),
            fragment: home
                .join(".config/systemd/user/hypercolor.service")
                .to_str()
                .expect("UTF-8 home")
                .to_owned(),
            legacy_units: home.join(".local/lib/hypercolor/units"),
            launcher: LinuxExactEntry::Absent,
            launcher_bytes: Vec::new(),
            layout: LINUX_LAYOUT_ITEMS
                .into_iter()
                .map(|item| (item, LinuxExactEntry::Absent))
                .collect(),
            loaded: false,
            active: false,
            enabled: false,
            exec_start: String::new(),
            pid: 0,
            last_pid: 0,
            invocation: 0,
            process: None,
            historical: Vec::new(),
            effects: Vec::new(),
            crash: None,
            fault: None,
            queued_start: false,
            auto_restart: false,
            failed: false,
            failing_starts: BTreeSet::new(),
            failing_start_ends_failed: false,
            start_job_persists: false,
            resets: 0,
            shows: 0,
            crash_at_show: None,
            at_health: None,
            in_probation: None,
            watches: Vec::new(),
            crash_after_effect: None,
            argument_override: None,
        }))
    }

    fn effect(&mut self, name: String) -> Result<Option<usize>, InstallPlatformError> {
        self.effects.push(name.clone());
        let count = self.effects.len();
        if self.fault.as_deref() == Some(name.as_str()) {
            self.fault = None;
            return Err(InstallPlatformError::new(format!(
                "scripted failure at {name}"
            )));
        }
        if self.crash == Some(Crash::Before(count)) {
            self.crash = None;
            panic!("simulated crash before effect {count} ({name})");
        }
        if self.crash_after_effect.as_deref() == Some(name.as_str()) {
            self.crash_after_effect = None;
            return Ok(Some(count));
        }
        Ok((self.crash == Some(Crash::After(count))).then_some(count))
    }

    fn settle(&mut self, crash_after: Option<usize>) {
        if let Some(count) = crash_after {
            self.crash = None;
            panic!("simulated crash after effect {count}");
        }
    }

    fn show(&self) -> Vec<u8> {
        let exec_start = if self.loaded && !self.exec_start.is_empty() {
            let executable = self
                .exec_start
                .split_ascii_whitespace()
                .next()
                .expect("executable");
            // Like user systemd, a stopped service keeps its last exec status
            // until the unit reloads.
            let (times, pid, code) = match (self.active, self.last_pid) {
                (true, _) => (
                    "start_time=[Sat 2026-09-26 04:17:52 UTC] ; stop_time=[n/a]",
                    self.pid,
                    "code=(null) ; status=0/0",
                ),
                (false, 0) => (
                    "start_time=[n/a] ; stop_time=[n/a]",
                    0,
                    "code=(null) ; status=0/0",
                ),
                (false, last) => (
                    "start_time=[Sat 2026-09-26 04:17:52 UTC] ; stop_time=[Sat 2026-09-26 04:17:54 UTC]",
                    last,
                    "code=exited ; status=0",
                ),
            };
            format!(
                "{{ path={executable} ; argv[]={command} ; ignore_errors=no ; {times} ; pid={pid} ; {code} }}",
                command = self.exec_start,
            )
        } else {
            String::new()
        };
        let (active_state, sub_state) = if self.active {
            ("active", "running")
        } else if self.auto_restart {
            ("activating", "auto-restart")
        } else if self.failed {
            ("failed", "failed")
        } else {
            ("inactive", "dead")
        };
        format!(
            "LoadState={}\nActiveState={}\nSubState={}\nUnitFileState={}\nFragmentPath={}\nExecStart={}\nMainPID={}\nInvocationID={}\n",
            if self.loaded { "loaded" } else { "not-found" },
            active_state,
            sub_state,
            if !self.loaded {
                ""
            } else if self.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if self.loaded { self.fragment.as_str() } else { "" },
            exec_start,
            self.pid,
            if self.active {
                format!("{:032x}", self.invocation)
            } else {
                String::new()
            },
        )
        .into_bytes()
    }

    /// The daemon a start runs now and its arguments. A managed service
    /// starts the installation's launcher, which this runs for real: it
    /// reads `active` once and names the daemon, UI and effects of that one
    /// release. A direct unit runs its `ExecStart` through `active`.
    fn launch(&self) -> (PathBuf, Vec<String>) {
        let words: Vec<&str> = self.exec_start.split_ascii_whitespace().collect();
        if let [launcher, "__launch", "--role", "daemon"] = words.as_slice() {
            let plan = plan_linux_launch(&LinuxLaunchRequest {
                home: &self.home,
                role: LinuxLaunchRole::Daemon,
                arguments: Vec::new(),
                launcher: Path::new(launcher),
            })
            .expect("the launcher selects a release");
            let mut arguments = vec![plan.program.to_str().expect("UTF-8").to_owned()];
            arguments.extend(
                plan.arguments
                    .iter()
                    .map(|argument| argument.to_str().expect("UTF-8").to_owned()),
            );
            return (plan.program, arguments);
        }
        let resolved = fs::canonicalize(words[0]).expect("launcher executable exists");
        (
            resolved,
            words.iter().map(|word| (*word).to_owned()).collect(),
        )
    }

    /// The version of the daemon the launcher would start right now.
    fn launch_version(&self) -> String {
        unit_version(&self.launch().0)
    }

    /// Run one start job the way the manager would. A daemon that never
    /// reaches readiness fails the job, then waits to restart or, past its
    /// start limit, ends failed.
    fn start_job(&mut self) -> Result<(), InstallPlatformError> {
        self.failed = false;
        self.auto_restart = false;
        if self.failing_starts.contains(&self.launch_version()) {
            self.invocation += 1;
            self.last_pid = 4000 + self.invocation;
            if self.failing_start_ends_failed {
                self.failed = true;
            } else {
                self.auto_restart = true;
            }
            if self.start_job_persists && !self.failed {
                self.queued_start = true;
                return Err(InstallPlatformError::new(
                    "systemd runtime job was cancelled at its deadline",
                ));
            }
            return Err(InstallPlatformError::new(
                "systemd job failed with exact result failed",
            ));
        }
        self.start();
        Ok(())
    }

    /// The daemon dies and `Restart=on-failure` brings it back under a
    /// fresh invocation.
    fn restart_service(&mut self) {
        assert!(self.active, "only a running daemon can restart");
        self.stop();
        self.start();
    }

    /// The daemon dies and the manager is waiting to restart it.
    fn crash_to_auto_restart(&mut self) {
        assert!(self.active, "only a running daemon can crash");
        self.stop();
        self.auto_restart = true;
    }

    /// The daemon dies and hits its start limit.
    fn crash_to_failed(&mut self) {
        assert!(self.active, "only a running daemon can crash");
        self.stop();
        self.failed = true;
    }

    /// Power loss: every process is gone, and the user manager comes back,
    /// loads the on-disk fragment and queues an enabled service's start.
    fn power_cycle(&mut self) {
        self.active = false;
        self.pid = 0;
        self.last_pid = 0;
        self.process = None;
        self.auto_restart = false;
        self.failed = false;
        if matches!(self.launcher, LinuxExactEntry::Absent) {
            self.loaded = false;
            self.exec_start.clear();
        } else {
            self.loaded = true;
            self.exec_start = launcher_exec(&self.launcher_bytes);
        }
        self.queued_start = self.loaded && self.enabled;
    }

    fn start(&mut self) {
        let (resolved, mut arguments) = self.launch();
        if let Some((version, replacement)) = &self.argument_override
            && unit_version(&resolved) == *version
        {
            arguments.clone_from(replacement);
        }
        let bytes = fs::read(&resolved).expect("daemon bytes");
        let metadata = fs::metadata(&resolved).expect("daemon metadata");
        self.process = Some(Process {
            path: resolved.to_str().expect("UTF-8").to_owned(),
            sha256: sha256(&bytes),
            device: metadata.dev(),
            inode: metadata.ino(),
            arguments,
        });
        self.active = true;
        self.invocation += 1;
        self.pid = 4000 + self.invocation;
    }

    fn stop(&mut self) {
        if self.active {
            self.last_pid = self.pid;
        }
        self.active = false;
        self.auto_restart = false;
        self.queued_start = false;
        self.pid = 0;
        self.process = None;
    }

    fn running_version(&self) -> String {
        let process = self.process.as_ref().expect("running daemon");
        unit_version(Path::new(&process.path))
    }
}

/// The manifest version of the unit whose daemon lives at `daemon`.
fn unit_version(daemon: &Path) -> String {
    let unit = daemon.parent().and_then(Path::parent).expect("unit root");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(unit.join("manifest.json")).expect("manifest"))
            .expect("manifest JSON");
    manifest["version"].as_str().expect("version").to_owned()
}

struct SimExecutor {
    world: Shared,
    active_root: Option<PathBuf>,
}

impl LinuxInstallExecutor for SimExecutor {
    fn validate_topology(
        &mut self,
        config: &LinuxInstallConfig,
    ) -> Result<(), InstallPlatformError> {
        assert_eq!(config.direct_fragment_path, self.world.borrow().fragment);
        self.active_root = Some(config.active_root.clone());
        Ok(())
    }

    fn validate_unit_authority(&mut self, _unit: &UnitRecord) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn prior_units_root(&self, unit: &UnitRecord) -> Result<PathBuf, InstallPlatformError> {
        let world = self.world.borrow();
        if world.historical.iter().any(|known| known == unit) {
            Ok(world.legacy_units.clone())
        } else {
            Err(InstallPlatformError::new("unretained historical prior"))
        }
    }

    fn retain_prior_units(&mut self) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn retain_recorded_prior(
        &mut self,
        record: &PlatformTransactionRecord,
    ) -> Result<Option<UnitRecord>, InstallPlatformError> {
        let PlatformTransactionRecord::Linux { payload, .. } = record else {
            return Ok(None);
        };
        let decoded: serde_json::Value = serde_json::from_slice(payload)
            .map_err(|error| InstallPlatformError::new(error.to_string()))?;
        let Some(prior) = decoded.get("prior").filter(|prior| !prior.is_null()) else {
            return Ok(None);
        };
        let world = self.world.borrow();
        let historical = prior["daemon_path"]
            .as_str()
            .is_some_and(|path| Path::new(path).starts_with(&world.legacy_units));
        if !historical {
            return Ok(None);
        }
        Ok(world
            .historical
            .iter()
            .find(|unit| Some(unit.id().as_str()) == prior["unit"].as_str())
            .cloned())
    }

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        let root = self.active_root.as_ref().expect("validated topology");
        match fs::read_link(root) {
            Ok(target) => UnitId::new(
                target
                    .file_name()
                    .expect("unit name")
                    .to_str()
                    .expect("UTF-8"),
            )
            .map(Some)
            .map_err(|error| InstallPlatformError::new(error.to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(InstallPlatformError::new(error.to_string())),
        }
    }

    fn settle_runtime(&mut self) -> Result<LinuxRuntimeSettlement, InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        if world.queued_start {
            world.queued_start = false;
            // A failed boot start leaves the service waiting to restart; one
            // that persists stays queued past the settle deadline.
            let _ = world.start_job();
        }
        Ok(if world.auto_restart || world.queued_start {
            LinuxRuntimeSettlement::Unsettled
        } else {
            LinuxRuntimeSettlement::Settled
        })
    }

    fn systemd_show(&mut self, max_bytes: usize) -> Result<Vec<u8>, InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        world.shows += 1;
        if world.crash_at_show == Some(world.shows) {
            world.crash_at_show = None;
            panic!(
                "simulated installer loss at systemd observation {}",
                world.shows
            );
        }
        let output = world.show();
        assert!(output.len() <= max_bytes);
        Ok(output)
    }

    fn launcher_entry(
        &mut self,
        _max_bytes: usize,
    ) -> Result<(LinuxExactEntry, Vec<u8>), InstallPlatformError> {
        let world = self.world.borrow();
        Ok((world.launcher.clone(), world.launcher_bytes.clone()))
    }

    fn layout_entry(
        &mut self,
        item: LinuxLayoutItem,
    ) -> Result<LinuxExactEntry, InstallPlatformError> {
        Ok(self.world.borrow().layout[&item].clone())
    }

    fn directory_state(
        &mut self,
        _item: LinuxDirectoryItem,
    ) -> Result<LinuxDirectoryState, InstallPlatformError> {
        Ok(LinuxDirectoryState::Present)
    }

    fn legacy_inventory(&mut self) -> Result<Vec<LinuxLegacyFile>, InstallPlatformError> {
        Ok(Vec::new())
    }

    fn replace_launcher(
        &mut self,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxFilePublication>,
    ) -> Result<(), InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        assert_eq!(&world.launcher, expected);
        let crash = world.effect("launcher".to_owned())?;
        if let Some(file) = replacement {
            world.launcher_bytes.clone_from(&file.contents);
            world.launcher = LinuxExactEntry::RegularFile {
                mode: file.mode,
                sha256: sha256(&file.contents),
                snapshot_unit: None,
                snapshot_path: None,
            };
        } else {
            world.launcher = LinuxExactEntry::Absent;
            world.launcher_bytes.clear();
        }
        world.settle(crash);
        Ok(())
    }

    fn replace_layout(
        &mut self,
        item: LinuxLayoutItem,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxLayoutPublication>,
    ) -> Result<(), InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        assert!(same_entry(&world.layout[&item], expected));
        let crash = world.effect(format!("layout:{item:?}"))?;
        let next = match replacement {
            Some(LinuxLayoutPublication::Symlink(target)) => LinuxExactEntry::Symlink {
                target: target.clone(),
            },
            Some(LinuxLayoutPublication::RegularFile(file)) => LinuxExactEntry::RegularFile {
                mode: file.mode,
                sha256: sha256(&file.contents),
                snapshot_unit: None,
                snapshot_path: None,
            },
            None => LinuxExactEntry::Absent,
        };
        world.layout.insert(item, next);
        world.settle(crash);
        Ok(())
    }

    fn replace_directory(
        &mut self,
        _item: LinuxDirectoryItem,
        _expected: LinuxDirectoryState,
        _create: bool,
    ) -> Result<(), InstallPlatformError> {
        panic!("the simulated session already has every public directory");
    }

    fn reload_manager(&mut self) -> Result<(), InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        let crash = world.effect("manager".to_owned())?;
        if matches!(world.launcher, LinuxExactEntry::Absent) {
            world.loaded = false;
            world.exec_start.clear();
        } else {
            world.loaded = true;
            world.exec_start = launcher_exec(&world.launcher_bytes);
        }
        // A reload rebuilds the exec command and forgets the last run.
        if !world.active {
            world.last_pid = 0;
        }
        world.settle(crash);
        Ok(())
    }

    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        let crash = world.effect(format!("autostart:{enabled}"))?;
        world.enabled = enabled;
        world.settle(crash);
        Ok(())
    }

    fn set_runtime(&mut self, running: bool) -> Result<(), InstallPlatformError> {
        let mut world = self.world.borrow_mut();
        let crash = world.effect(format!("runtime:{running}"))?;
        let result = if running {
            if world.failed {
                world.resets += 1;
            }
            world.start_job()
        } else {
            world.stop();
            Ok(())
        };
        world.settle(crash);
        result
    }

    fn watch_service(
        &mut self,
        expected: &LinuxServiceIdentity,
        window: Duration,
    ) -> Result<LinuxServiceWatch, InstallPlatformError> {
        let event = {
            let mut world = self.world.borrow_mut();
            let version = world
                .process
                .as_ref()
                .map(|_| world.running_version())
                .unwrap_or_default();
            world
                .watches
                .push((version.clone(), expected.clone(), window));
            let current = LinuxServiceIdentity {
                invocation_id: format!("{:032x}", world.invocation),
                main_pid: world.pid,
            };
            if !world.active || &current != expected {
                return Ok(LinuxServiceWatch::Changed {
                    after: Duration::ZERO,
                    observed: "another service identity".to_owned(),
                });
            }
            if world
                .in_probation
                .is_some_and(|(targeted, _)| targeted == version)
            {
                world.in_probation.take().map(|(_, event)| event)
            } else {
                None
            }
        };
        match event {
            Some(ProbationEvent::Crashes(after)) if after < window => {
                self.world.borrow_mut().restart_service();
                Ok(LinuxServiceWatch::Changed {
                    after,
                    observed: "active/running under a new invocation".to_owned(),
                })
            }
            Some(ProbationEvent::InstallerDies(after)) if after < window => {
                panic!("simulated installer loss {after:?} into probation")
            }
            Some(ProbationEvent::PowerCut(after)) if after < window => {
                self.world.borrow_mut().power_cycle();
                panic!("simulated power loss {after:?} into probation")
            }
            _ => Ok(LinuxServiceWatch::Steady),
        }
    }

    fn process_executable(
        &mut self,
        pid: u32,
        _max_bytes: u64,
    ) -> Result<LinuxProcessExecutable, InstallPlatformError> {
        let world = self.world.borrow();
        assert_eq!(pid, world.pid);
        let process = world
            .process
            .clone()
            .ok_or_else(|| InstallPlatformError::new("no running daemon"))?;
        Ok(LinuxProcessExecutable {
            path: process.path,
            sha256: process.sha256,
            device: process.device,
            inode: process.inode,
            arguments: process.arguments,
        })
    }

    fn http_get(
        &mut self,
        path: &'static str,
        max_bytes: usize,
    ) -> Result<LinuxHttpResponse, InstallPlatformError> {
        let targeted = {
            let world = self.world.borrow();
            path == "/health"
                && world.process.is_some()
                && world
                    .at_health
                    .is_some_and(|(version, _)| world.running_version() == version)
        };
        if targeted {
            let event = self
                .world
                .borrow_mut()
                .at_health
                .take()
                .map(|(_, event)| event);
            match event {
                Some(HealthEvent::InstallerDies) => panic!("simulated installer loss in a proof"),
                Some(HealthEvent::Restarts) => self.world.borrow_mut().restart_service(),
                Some(HealthEvent::CrashLoops) => self.world.borrow_mut().crash_to_auto_restart(),
                None => {}
            }
        }
        if self.world.borrow().process.is_none() {
            return Err(InstallPlatformError::new("daemon HTTP connection refused"));
        }
        let version = self.world.borrow().running_version();
        let value = match path {
            "/health" => json!({"status":"healthy","version":version}),
            "/api/v1/system" => json!({"data":{"identity":{
                "instance_id":"local","instance_name":"Hypercolor","version":version
            },"status":null}}),
            _ => unreachable!("fixed proof endpoint"),
        };
        let body = serde_json::to_vec(&value).expect("HTTP JSON");
        assert!(body.len() <= max_bytes);
        Ok(LinuxHttpResponse { status: 200, body })
    }

    fn snapshot_legacy_unit(
        &mut self,
        _snapshot: &hypercolor_cli::install::LinuxLegacySnapshot,
    ) -> Result<UnitRecord, InstallPlatformError> {
        Err(InstallPlatformError::new("no legacy regular-file layout"))
    }
}

// ── Hosts ───────────────────────────────────────────────────────────────

struct Release {
    source: PathBuf,
    id: UnitId,
}

impl Release {
    fn write(root: &Path, version: &'static str, daemon: &[u8]) -> Self {
        let source = root.join(format!("source-{version}"));
        write_release(&source, version, daemon);
        let id = UnitId::new(sha256(
            &fs::read(source.join("manifest.json")).expect("manifest"),
        ))
        .expect("unit ID");
        Self { source, id }
    }

    fn stage(&self, store: &InstallStore, lock: &InstallLock) -> UnitRecord {
        let executable = File::open(self.source.join("bin/hypercolor")).expect("candidate");
        stage_release_payload(store, lock, &self.source, &executable, &self.id)
            .expect("stage release")
    }

    fn request(&self, policy: InstallTargetPolicy) -> LinuxInstallRequest {
        LinuxInstallRequest {
            candidate: self.id.clone(),
            transaction_id: InstallTransactionId::new(format!(
                "release-{}",
                &self.id.as_str()[..16]
            ))
            .expect("transaction"),
            target_policy: policy,
            probation: DEFAULT_PROBATION_WINDOW,
        }
    }
}

struct Host<'a> {
    world: Shared,
    release: &'a Release,
    proposal: Option<LinuxInstallLocation>,
    proposals: usize,
    stop_at: Option<LinuxInstallCheckpoint>,
    seen: Vec<LinuxInstallCheckpoint>,
    on_checkpoint: Option<Box<dyn FnMut(LinuxInstallCheckpoint) + 'a>>,
}

impl<'a> Host<'a> {
    fn new(world: &Shared, release: &'a Release, proposal: Option<LinuxInstallLocation>) -> Self {
        Self {
            world: Rc::clone(world),
            release,
            proposal,
            proposals: 0,
            stop_at: None,
            seen: Vec::new(),
            on_checkpoint: None,
        }
    }
}

impl LinuxInstallHost for Host<'_> {
    type Executor = SimExecutor;

    fn propose_location(
        &mut self,
        _home: &Path,
        _uid: u32,
    ) -> Result<LinuxInstallLocation, InstallPlatformError> {
        self.proposals += 1;
        self.proposal
            .clone()
            .ok_or_else(|| InstallPlatformError::new("no environment proposal expected"))
    }

    fn stage_candidate(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
    ) -> Result<UnitRecord, InstallPlatformError> {
        Ok(self.release.stage(store, lock))
    }

    fn executor(
        &mut self,
        _store: &InstallStore,
        _lock: &InstallLock,
        _tree: LinuxPublicTree,
    ) -> Result<SimExecutor, InstallPlatformError> {
        Ok(SimExecutor {
            world: Rc::clone(&self.world),
            active_root: None,
        })
    }

    fn checkpoint(
        &mut self,
        checkpoint: LinuxInstallCheckpoint,
    ) -> Result<(), InstallPlatformError> {
        self.seen.push(checkpoint);
        if let Some(observe) = self.on_checkpoint.as_mut() {
            observe(checkpoint);
        }
        if self.stop_at == Some(checkpoint) {
            return Err(InstallPlatformError::new("simulated process loss"));
        }
        Ok(())
    }
}

struct UninstallHost {
    world: Shared,
    stop_at: Option<LinuxUninstallCheckpoint>,
}

impl LinuxUninstallHost for UninstallHost {
    type Executor = SimExecutor;

    fn executor(
        &mut self,
        _store: &InstallStore,
        _lock: &InstallLock,
        _tree: LinuxPublicTree,
    ) -> Result<SimExecutor, InstallPlatformError> {
        Ok(SimExecutor {
            world: Rc::clone(&self.world),
            active_root: None,
        })
    }

    fn checkpoint(
        &mut self,
        checkpoint: LinuxUninstallCheckpoint,
    ) -> Result<(), InstallPlatformError> {
        if self.stop_at == Some(checkpoint) {
            return Err(InstallPlatformError::new("simulated process loss"));
        }
        Ok(())
    }
}

// ── Fixture ─────────────────────────────────────────────────────────────

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    uid: u32,
    gid: u32,
    world: Shared,
    v1: Release,
    v2: Release,
    v3: Release,
}

impl Fixture {
    fn new() -> Self {
        Self::with_home(None)
    }

    /// `home_bytes` pads HOME to an exact byte length to probe path bounds.
    fn with_home(home_bytes: Option<usize>) -> Self {
        // Short roots: the platform transaction record embeds every root.
        let temporary = tempfile::Builder::new()
            .prefix(".hc-")
            .tempdir_in(std::env::var_os("HOME").expect("owned test home"))
            .expect("fixture");
        let root = fs::canonicalize(temporary.path()).expect("canonical fixture");
        let home = match home_bytes {
            Some(bytes) => {
                let prefix = root.to_str().expect("UTF-8").len() + 1;
                root.join("h".repeat(bytes - prefix))
            }
            None => root.join("home"),
        };
        fs::create_dir(&home).expect("home");
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).expect("home mode");
        let metadata = fs::metadata(&home).expect("home metadata");
        let v1 = Release::write(&root, "9.8.7", b"daemon-one");
        let v2 = Release::write(&root, "9.8.8", b"daemon-two");
        let v3 = Release::write(&root, "9.8.9", b"daemon-three");
        Self {
            world: World::new(&home),
            uid: metadata.uid(),
            gid: metadata.gid(),
            _temporary: temporary,
            home,
            v1,
            v2,
            v3,
        }
    }

    fn private(&self) -> OwnershipPolicy {
        policy(Principals::private(self.uid, self.gid))
    }

    fn location(&self, data: &str, state: &str, config: &str) -> LinuxInstallLocation {
        LinuxInstallLocation::new(
            &self.home,
            &self.home.join(data),
            &self.home.join(state),
            &self.home.join(config),
            self.uid,
        )
        .expect("proposed location")
    }

    fn default_location(&self) -> LinuxInstallLocation {
        self.location(".local/share", ".local/state", ".config")
    }

    fn legacy(&self) -> InstallStore {
        InstallStore::new(self.home.join(".local/lib/hypercolor"), MAX_JOURNAL)
            .with_ownership_policy(self.private())
    }

    /// Install `release` the way the historical lib-root installer did.
    fn legacy_install(&self, release: &Release) -> UnitRecord {
        let old = self.legacy();
        let lock = old.acquire_anchored_lock(&self.home).expect("legacy lock");
        let unit = release.stage(&old, &lock);
        drop(lock);
        self.legacy_activate(unit)
    }

    /// Install `release` the way a historical installer from before the
    /// managed package contract did: its manifest carries no block.
    fn legacy_install_before_contract(&self, release: &Release) -> UnitRecord {
        let scratch = release
            .source
            .parent()
            .expect("fixture root")
            .join("scratch-store");
        let staging =
            InstallStore::new(&scratch, MAX_JOURNAL).with_ownership_policy(self.private());
        let staging_lock = staging.acquire_lock().expect("scratch lock");
        let staged = release.stage(&staging, &staging_lock);
        let root = staging.unit_path(staged.id());
        drop(staged);
        let path = root.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("manifest JSON");
        manifest
            .as_object_mut()
            .expect("object")
            .remove("managed_package");
        let bytes = serde_json::to_vec_pretty(&manifest).expect("encode");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("thaw");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("thaw");
        fs::write(&path, &bytes).expect("rewrite manifest");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("freeze");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).expect("freeze");
        let id = UnitId::new(sha256(&bytes)).expect("digest");
        fs::rename(&root, staging.unit_path(&id)).expect("rename");
        let retained = hypercolor_cli::install::retain_linux_unit(&staging, &staging_lock, &id)
            .expect("retain");
        let old = self.legacy();
        let lock = old.acquire_anchored_lock(&self.home).expect("legacy lock");
        let unit = hypercolor_cli::install::copy_installed_release_unit(&old, &lock, &retained)
            .expect("the historical installer held a release from before the contract");
        drop(lock);
        self.legacy_activate(unit)
    }

    fn legacy_activate(&self, unit: UnitRecord) -> UnitRecord {
        let old = self.legacy();
        let mut lock = old.acquire_anchored_lock(&self.home).expect("legacy lock");
        self.world.borrow_mut().historical.push(unit.clone());
        let config = LinuxInstallConfig {
            direct_fragment_path: self.world.borrow().fragment.clone(),
            immutable_units_root: old.root().join("units"),
            active_root: old.active_path(),
            probation: Duration::ZERO,
            managed: None,
        };
        let executor = SimExecutor {
            world: Rc::clone(&self.world),
            active_root: None,
        };
        let known: Vec<UnitRecord> = old
            .active_unit(&lock)
            .expect("historical pointer")
            .map(|id| hypercolor_cli::install::retain_linux_unit(&old, &lock, &id).expect("prior"))
            .into_iter()
            .collect();
        let mut platform =
            LinuxInstallPlatform::new(executor, config, known).expect("legacy platform");
        let outcome = InstallCoordinator::new(&old, &mut platform)
            .install_with_lock(
                InstallRequest {
                    transaction_id: InstallTransactionId::new("historical").expect("id"),
                    candidate: unit.clone(),
                    target_policy: InstallTargetPolicy::EnableOnFirstInstall,
                },
                &mut lock,
            )
            .expect("legacy install");
        assert_eq!(
            outcome,
            InstallOutcome::Committed {
                active_unit: unit.id().clone()
            }
        );
        unit
    }

    fn run(
        &self,
        release: &Release,
        proposal: Option<LinuxInstallLocation>,
        ownership: &OwnershipPolicy,
    ) -> Result<hypercolor_cli::install::LinuxInstallRun, LinuxInstallCommandError> {
        let mut host = Host::new(&self.world, release, proposal);
        run_linux_install(
            &self.home,
            &release.request(InstallTargetPolicy::EnableOnFirstInstall),
            ownership,
            &mut host,
        )
    }

    fn snapshot(&self) -> (Vec<u8>, BTreeMap<LinuxLayoutItem, LinuxExactEntry>, String) {
        let world = self.world.borrow();
        (
            world.launcher_bytes.clone(),
            world.layout.clone(),
            world
                .process
                .as_ref()
                .map(|process| format!("{}:{}:{}", process.path, process.device, process.inode))
                .unwrap_or_default(),
        )
    }

    /// Prove exactly one managed authority at `location` running `expected`.
    fn assert_managed(&self, location: &LinuxInstallLocation, expected: &UnitId) {
        let LinuxInstallElection::Managed {
            store,
            lock,
            authority,
        } = elect_linux_installation_with(&self.home, &self.private()).expect("managed election")
        else {
            panic!("expected managed authority");
        };
        assert_eq!(authority.location().release_root(), location.release_root());
        assert_eq!(authority.location().state_root(), location.state_root());
        assert_eq!(
            store.active_unit(&lock).expect("active"),
            Some(expected.clone())
        );
        let journal = store
            .load_journal(&lock)
            .expect("journal")
            .expect("present");
        assert!(matches!(
            journal.disposition,
            InstallDisposition::Committed | InstallDisposition::RolledBack
        ));
        authority.confirm_durable().expect("durable authority");
        drop((store, lock, authority));
        let old = self.legacy();
        let old_lock = old.acquire_lock().expect("historical lock is free");
        assert!(
            old.load_journal(&old_lock).is_err(),
            "the historical decoder is permanently fenced"
        );
        let world = self.world.borrow();
        let process = world.process.as_ref().expect("daemon running");
        assert!(
            Path::new(&process.path).starts_with(
                location
                    .release_root()
                    .join("units")
                    .join(expected.as_str())
            ),
            "running {} instead of the recorded release root",
            process.path
        );
        let launcher = location.release_root().join("launcher/hypercolor");
        assert_eq!(
            launcher_exec(&world.launcher_bytes),
            format!("{} __launch --role daemon", launcher.display()),
            "the service starts the installation's launcher"
        );
        let release = location
            .release_root()
            .join("units")
            .join(expected.as_str());
        assert_eq!(
            process.arguments,
            [
                release.join("bin/hypercolor-daemon"),
                PathBuf::from("--ui-dir"),
                release.join("share/hypercolor/ui"),
                PathBuf::from("--effects-dir"),
                release.join("share/hypercolor/effects/bundled"),
            ]
            .map(|argument| argument.to_str().expect("UTF-8").to_owned()),
            "the daemon, its UI and its effects all come from the active release"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        writable(&self.home);
    }
}

// ── Umask 002 defaults and the private-group rule ───────────────────────

const UMASK_CHILD: &str = "HYPERCOLOR_MANAGED_UMASK_CHILD";

/// Rerun one test inside `sh` with umask 002, as Ubuntu user sessions do.
fn run_under_umask_002(test: &str) -> bool {
    if std::env::var_os(UMASK_CHILD).is_some() {
        return false;
    }
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg("umask 002 && exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("test executable"))
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(UMASK_CHILD, "1")
        .output()
        .expect("run umask 002 child");
    assert!(
        output.status.success(),
        "umask 002 child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "umask 002 child did not run {test}"
    );
    true
}

/// Leave the ancestors a umask 002 session and a pre-installer daemon leave.
fn umask_002_session(fixture: &Fixture) {
    for directory in [
        ".local",
        ".local/share",
        ".config",
        ".local/share/hypercolor",
        ".config/hypercolor",
    ] {
        let path = fixture.home.join(directory);
        fs::create_dir_all(&path).expect("session directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o775)).expect("session mode");
    }
    fs::write(
        fixture.home.join(".local/share/hypercolor/scenes.json"),
        b"{}",
    )
    .expect("user data");
    fs::write(fixture.home.join(".config/hypercolor/hypercolor.toml"), b"").expect("user config");
}

#[test]
fn umask_002_fresh_install_adopts_default_roots_with_explicit_modes() {
    if run_under_umask_002("umask_002_fresh_install_adopts_default_roots_with_explicit_modes") {
        return;
    }
    let fixture = Fixture::new();
    let probe = fixture.home.join("umask-probe");
    fs::create_dir(&probe).expect("probe");
    assert_eq!(mode(&probe), 0o775, "the child must run under umask 002");
    fs::remove_dir(&probe).expect("probe cleanup");
    umask_002_session(&fixture);
    let location = fixture.default_location();

    let strict = fixture.run(
        &fixture.v1,
        Some(location.clone()),
        &OwnershipPolicy::strict(),
    );
    assert!(
        matches!(
            strict,
            Err(LinuxInstallCommandError::Election(LinuxLocatorError::Store(
                InstallStoreError::UnsafeBootstrapDirectory(ref path, DirectoryRefusal::SharedGroup { .. })
            ))) if path == &fixture.home.join(".local")
        ),
        "the strict policy must refuse the 0775 ancestor: {strict:?}"
    );
    assert!(!fixture.home.join(".local/lib").exists());

    let run = fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("umask 002 fresh install");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v1.id.clone()
        }
    );
    fixture.assert_managed(&location, &fixture.v1.id);
    for (path, expected) in [
        (".local/share/hypercolor/releases", 0o755),
        (".local/state", 0o700),
        (".local/state/hypercolor", 0o700),
        (".local/state/hypercolor/update", 0o700),
        (".local/lib", 0o755),
        (".local/lib/hypercolor", 0o755),
        (".local/share/hypercolor", 0o775),
        (".config/hypercolor", 0o775),
    ] {
        assert_eq!(mode(&fixture.home.join(path)), expected, "{path}");
    }
    assert_eq!(
        fs::read(fixture.home.join(".local/share/hypercolor/scenes.json")).expect("user data"),
        b"{}"
    );
}

#[test]
fn shared_group_world_writable_sticky_and_unresolvable_ancestors_fail_closed() {
    // A 0775 ancestor whose group has another member.
    let fixture = Fixture::new();
    umask_002_session(&fixture);
    let shared = policy(Principals::shared(fixture.uid, fixture.gid));
    let error = fixture
        .run(&fixture.v1, Some(fixture.default_location()), &shared)
        .expect_err("shared group");
    assert!(error.to_string().contains("neighbor"), "{error}");
    assert!(!fixture.home.join(".local/lib").exists());
    assert!(!fixture.home.join(".local/state").exists());

    // The same ancestor when the directory service cannot answer.
    let unreachable = policy(Principals::unreachable(fixture.uid, fixture.gid));
    let error = fixture
        .run(&fixture.v1, Some(fixture.default_location()), &unreachable)
        .expect_err("unresolvable group");
    assert!(error.to_string().contains("unreachable"), "{error}");
    assert!(!fixture.home.join(".local/lib").exists());

    // A world-writable parent of a root the adoption must create.
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".local/share")).expect("share");
    fs::set_permissions(
        fixture.home.join(".local/share"),
        fs::Permissions::from_mode(0o777),
    )
    .expect("world-writable share");
    let error = fixture
        .run(
            &fixture.v1,
            Some(fixture.default_location()),
            &fixture.private(),
        )
        .expect_err("world-writable ancestor");
    assert!(
        matches!(
            error,
            LinuxInstallCommandError::Adoption(hypercolor_cli::install::LinuxAdoptionError::Locator(
                LinuxLocatorError::UnsafeDirectory(ref path, DirectoryRefusal::WorldWritable)
            )) if path == &fixture.home.join(".local/share")
        ),
        "{error:?}"
    );
    assert!(!fixture.home.join(".local/share/hypercolor").exists());
    assert_legacy_untouched(&fixture);

    // A sticky /tmp-like XDG base.
    let fixture = Fixture::new();
    fs::create_dir(fixture.home.join("shared-tmp")).expect("sticky base");
    fs::set_permissions(
        fixture.home.join("shared-tmp"),
        fs::Permissions::from_mode(0o1777),
    )
    .expect("sticky mode");
    let sticky = fixture.location("shared-tmp", ".local/state", ".config");
    assert!(
        fixture
            .run(&fixture.v1, Some(sticky), &fixture.private())
            .is_err()
    );
    assert!(!fixture.home.join("shared-tmp/hypercolor").exists());
    assert_legacy_untouched(&fixture);
    fs::set_permissions(
        fixture.home.join("shared-tmp"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("cleanup");
}

fn assert_legacy_untouched(fixture: &Fixture) {
    let world = fixture.world.borrow();
    assert!(world.effects.is_empty(), "{:?}", world.effects);
    assert!(
        !fixture
            .home
            .join(".local/lib/hypercolor/install-journal.json")
            .exists(),
        "no authority may be published"
    );
}

// ── Recorded authority over a changed environment ───────────────────────

#[test]
fn changed_xdg_after_install_follows_the_recorded_roots() {
    let fixture = Fixture::new();
    let recorded = fixture.location("xdg-a/data", "xdg-a/state", "xdg-a/config");
    fixture
        .run(&fixture.v1, Some(recorded.clone()), &fixture.private())
        .expect("fresh install");
    fixture.assert_managed(&recorded, &fixture.v1.id);
    let changed = fixture.location("xdg-b/data", "xdg-b/state", "xdg-b/config");

    // Update: the environment now names other roots.
    let mut host = Host::new(&fixture.world, &fixture.v2, Some(changed.clone()));
    let run = run_linux_install(
        &fixture.home,
        &fixture.v2.request(InstallTargetPolicy::Preserve),
        &fixture.private(),
        &mut host,
    )
    .expect("update under changed XDG");
    assert_eq!(
        host.proposals, 0,
        "managed runs never consult the environment"
    );
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    fixture.assert_managed(&recorded, &fixture.v2.id);

    // Recover: lose the process mid-rollback-to-v1, then settle under B.
    let performed = fixture.world.borrow().effects.len();
    fixture.world.borrow_mut().crash = Some(Crash::After(performed + 3));
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        let mut host = Host::new(&fixture.world, &fixture.v1, Some(changed.clone()));
        run_linux_install(
            &fixture.home,
            &fixture.v1.request(InstallTargetPolicy::Preserve),
            &fixture.private(),
            &mut host,
        )
    }));
    assert!(
        crashed.is_err(),
        "the scripted crash must interrupt the run"
    );
    let mut host = Host::new(&fixture.world, &fixture.v1, Some(changed.clone()));
    let run = run_linux_install(
        &fixture.home,
        &fixture.v1.request(InstallTargetPolicy::Preserve),
        &fixture.private(),
        &mut host,
    )
    .expect("cold recovery under changed XDG");
    assert!(run.recovered);
    assert_eq!(host.proposals, 0);
    fixture.assert_managed(&recorded, &fixture.v1.id);
    assert!(
        !fixture.home.join("xdg-b").exists(),
        "the new environment was never used"
    );

    // Uninstall also follows the record.
    let mut uninstall = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    run_linux_uninstall(&fixture.home, &fixture.private(), &mut uninstall).expect("uninstall");
    assert!(!recorded.release_root().exists());
    assert!(!recorded.state_root().exists());
    assert!(!fixture.home.join("xdg-b").exists());
}

// ── Crash at every migration checkpoint ─────────────────────────────────

const ADOPTION_CHECKPOINTS: [LinuxInstallCheckpoint; 10] = [
    LinuxInstallCheckpoint::LegacyElected,
    LinuxInstallCheckpoint::RootsBootstrapped,
    LinuxInstallCheckpoint::IntentRecorded,
    LinuxInstallCheckpoint::PriorCopied,
    LinuxInstallCheckpoint::PriorActivated,
    LinuxInstallCheckpoint::CandidateStaged,
    LinuxInstallCheckpoint::AdoptionReceipt,
    LinuxInstallCheckpoint::StateJournal,
    LinuxInstallCheckpoint::LocatorPublished,
    LinuxInstallCheckpoint::ManagedElected,
];

#[test]
fn loss_after_every_migration_checkpoint_recovers_one_authority() {
    for checkpoint in ADOPTION_CHECKPOINTS {
        let fixture = Fixture::new();
        let original = fixture.legacy_install(&fixture.v1);
        let (legacy_launcher, legacy_layout, legacy_process) = fixture.snapshot();
        let location = fixture.default_location();

        let mut host = Host::new(&fixture.world, &fixture.v2, Some(location.clone()));
        host.stop_at = Some(checkpoint);
        let stopped = run_linux_install(
            &fixture.home,
            &fixture.v2.request(InstallTargetPolicy::Preserve),
            &fixture.private(),
            &mut host,
        );
        if checkpoint == LinuxInstallCheckpoint::ManagedElected {
            // Adoption never re-elects; the checkpoint belongs to later runs.
            stopped.expect("adoption does not pass through managed election");
            fixture.assert_managed(&location, &fixture.v2.id);
            continue;
        }
        assert!(
            matches!(stopped, Err(LinuxInstallCommandError::Stopped(seen, _)) if seen == checkpoint)
                || matches!(stopped, Err(LinuxInstallCommandError::Adoption(_))),
            "{checkpoint:?}: {stopped:?}"
        );
        let published = checkpoint == LinuxInstallCheckpoint::LocatorPublished;
        let elected = elect_linux_installation_with(&fixture.home, &fixture.private())
            .expect("exactly one authority after loss");
        assert_eq!(
            matches!(elected, LinuxInstallElection::Managed { .. }),
            published,
            "{checkpoint:?}: the locator is the only commit point"
        );
        drop(elected);
        if !published {
            // Nothing visible changed before publication.
            assert_eq!(
                fixture.snapshot(),
                (
                    legacy_launcher.clone(),
                    legacy_layout.clone(),
                    legacy_process.clone()
                )
            );
        }

        let mut host = Host::new(&fixture.world, &fixture.v2, Some(location.clone()));
        let run = run_linux_install(
            &fixture.home,
            &fixture.v2.request(InstallTargetPolicy::Preserve),
            &fixture.private(),
            &mut host,
        )
        .unwrap_or_else(|error| panic!("{checkpoint:?}: cold replay failed: {error}"));
        assert_eq!(
            run.outcome,
            InstallOutcome::Committed {
                active_unit: fixture.v2.id.clone()
            },
            "{checkpoint:?}"
        );
        fixture.assert_managed(&location, &fixture.v2.id);
        let old = fixture.legacy();
        let lock = old.acquire_lock().expect("historical lock");
        assert_eq!(
            old.active_unit(&lock).expect("historical pointer"),
            Some(original.id().clone()),
            "{checkpoint:?}: the historical pointer is retained"
        );
    }
}

#[test]
fn loss_at_every_activation_effect_after_publication_settles_forward_or_back_exactly() {
    let probe = Fixture::new();
    probe.legacy_install(&probe.v1);
    let before = probe.world.borrow().effects.len();
    probe
        .run(&probe.v2, Some(probe.default_location()), &probe.private())
        .expect("uninterrupted adoption");
    let activation = probe.world.borrow().effects.len() - before;
    assert!(activation > 5, "adoption activation performs real effects");

    for effect in 1..=activation {
        for crash in [
            Crash::Before(before + effect),
            Crash::After(before + effect),
        ] {
            let fixture = Fixture::new();
            let original = fixture.legacy_install(&fixture.v1);
            let legacy = fixture.snapshot();
            let location = fixture.default_location();
            fixture.world.borrow_mut().crash = Some(crash);
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                fixture.run(&fixture.v2, Some(location.clone()), &fixture.private())
            }));
            assert!(crashed.is_err(), "{crash:?} did not interrupt the run");
            let run = fixture
                .run(&fixture.v2, Some(location.clone()), &fixture.private())
                .unwrap_or_else(|error| panic!("{crash:?}: recovery failed: {error}"));
            assert!(run.recovered, "{crash:?}");
            match run.outcome {
                InstallOutcome::Committed { active_unit } => {
                    assert_eq!(active_unit, fixture.v2.id, "{crash:?}");
                    fixture.assert_managed(&location, &fixture.v2.id);
                }
                InstallOutcome::RolledBack { .. } => {
                    assert_eq!(fixture.snapshot().0, legacy.0, "{crash:?}: launcher bytes");
                    assert_eq!(fixture.snapshot().1, legacy.1, "{crash:?}: layout");
                    let process = fixture.world.borrow().process.clone().expect("running");
                    let daemon = fixture
                        .home
                        .join(".local/lib/hypercolor/units")
                        .join(original.id().as_str())
                        .join("bin/hypercolor-daemon");
                    assert_eq!(process.path, daemon.to_str().expect("UTF-8"), "{crash:?}");
                    assert_eq!(
                        process.inode,
                        fs::metadata(&daemon).expect("original").ino(),
                        "{crash:?}: the original inode runs again"
                    );
                    let next = fixture
                        .run(&fixture.v2, None, &fixture.private())
                        .expect("next attempt after rollback");
                    assert_eq!(
                        next.outcome,
                        InstallOutcome::Committed {
                            active_unit: fixture.v2.id.clone()
                        }
                    );
                    fixture.assert_managed(&location, &fixture.v2.id);
                }
            }
        }
    }
}

#[test]
fn failed_adoption_activation_rolls_back_to_the_exact_original_then_retries() {
    let fixture = Fixture::new();
    let original = fixture.legacy_install(&fixture.v1);
    let (launcher, layout, process) = fixture.snapshot();
    let location = fixture.default_location();
    fixture.world.borrow_mut().fault = Some("runtime:true".to_owned());
    let run = fixture
        .run(&fixture.v2, Some(location.clone()), &fixture.private())
        .expect("settled adoption");
    assert!(matches!(run.outcome, InstallOutcome::RolledBack { .. }));
    assert_eq!(fixture.snapshot().0, launcher);
    assert_eq!(fixture.snapshot().1, layout);
    assert_eq!(
        fixture.snapshot().2,
        process,
        "the original daemon inode runs again"
    );
    // Rollback keeps managed authority; the historical pointer never moved.
    let old = fixture.legacy();
    let lock = old.acquire_lock().expect("historical lock");
    assert_eq!(
        old.active_unit(&lock).expect("pointer"),
        Some(original.id().clone())
    );
    assert!(old.load_journal(&lock).is_err());
    drop(lock);
    let next = fixture
        .run(&fixture.v2, None, &fixture.private())
        .expect("retry after rollback");
    assert_eq!(
        next.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    fixture.assert_managed(&location, &fixture.v2.id);
}

#[test]
fn pending_historical_journal_recovers_with_legacy_semantics_before_adoption() {
    let probe = Fixture::new();
    probe.legacy_install(&probe.v1);
    let installed = probe.world.borrow().effects.len();

    for effect in 1..=installed {
        let fixture = Fixture::new();
        fixture.world.borrow_mut().crash = Some(Crash::After(effect));
        let crashed = catch_unwind(AssertUnwindSafe(|| fixture.legacy_install(&fixture.v1)));
        assert!(crashed.is_err());
        let location = fixture.default_location();
        let first = fixture
            .run(&fixture.v2, Some(location.clone()), &fixture.private())
            .expect("legacy recovery");
        assert!(first.recovered, "effect {effect}");
        assert!(
            !fixture.home.join(".local/state/hypercolor").exists(),
            "effect {effect}: recovery must precede any adoption write"
        );
        assert!(matches!(
            elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election"),
            LinuxInstallElection::Legacy { .. }
        ));
        let second = fixture
            .run(&fixture.v2, Some(location.clone()), &fixture.private())
            .expect("adoption after recovery");
        assert_eq!(
            second.outcome,
            InstallOutcome::Committed {
                active_unit: fixture.v2.id.clone()
            }
        );
        fixture.assert_managed(&location, &fixture.v2.id);
    }
}

#[test]
fn duplicate_invocation_is_refused_while_a_run_holds_authority() {
    let fixture = Fixture::new();
    fixture.legacy_install(&fixture.v1);
    let location = fixture.default_location();
    let refusals = RefCell::new(Vec::new());
    let mut host = Host::new(&fixture.world, &fixture.v2, Some(location.clone()));
    host.on_checkpoint = Some(Box::new(|checkpoint| {
        if matches!(
            checkpoint,
            LinuxInstallCheckpoint::AdoptionReceipt | LinuxInstallCheckpoint::LocatorPublished
        ) {
            let before = fixture.snapshot();
            let result = fixture.run(&fixture.v1, Some(location.clone()), &fixture.private());
            assert_eq!(
                fixture.snapshot(),
                before,
                "a refused duplicate never writes"
            );
            refusals.borrow_mut().push(format!("{result:?}"));
        }
    }));
    run_linux_install(
        &fixture.home,
        &fixture.v2.request(InstallTargetPolicy::Preserve),
        &fixture.private(),
        &mut host,
    )
    .expect("the first invocation finishes");
    drop(host);
    let refusals = refusals.into_inner();
    assert_eq!(refusals.len(), 2);
    for refusal in refusals {
        assert!(refusal.contains("LockContended"), "{refusal}");
    }
    fixture.assert_managed(&location, &fixture.v2.id);
}

// ── Uninstall ───────────────────────────────────────────────────────────

#[test]
fn uninstall_follows_recorded_roots_and_preserves_user_data() {
    let fixture = Fixture::new();
    umask_002_session(&fixture);
    let location = fixture.location(".local/share", ".local/state", ".config");
    fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("install");
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect("uninstall");
    assert!(run.recovered.is_none());
    for removed in [
        location.release_root().to_path_buf(),
        location.state_root().to_path_buf(),
        fixture.home.join(".local/state/hypercolor"),
        fixture.home.join(".local/lib/hypercolor"),
    ] {
        assert!(
            run.removed.contains(&removed),
            "{removed:?} in {:?}",
            run.removed
        );
        assert!(!removed.exists(), "{removed:?}");
    }
    assert_eq!(
        run.preserved,
        vec![
            location.data_root().to_path_buf(),
            location.config_root().to_path_buf()
        ]
    );
    assert_eq!(
        fs::read(fixture.home.join(".local/share/hypercolor/scenes.json")).expect("user data"),
        b"{}"
    );
    assert!(
        fixture
            .home
            .join(".config/hypercolor/hypercolor.toml")
            .exists()
    );
    let world = fixture.world.borrow();
    assert!(!world.loaded && !world.active && !world.enabled);
    assert!(matches!(world.launcher, LinuxExactEntry::Absent));
    assert!(
        world
            .layout
            .values()
            .all(|entry| matches!(entry, LinuxExactEntry::Absent))
    );
    drop(world);
    let again = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect("uninstall is idempotent");
    assert!(again.removed.is_empty());
    assert!(!fixture.home.join(".local/lib/hypercolor").exists());
}

#[test]
fn uninstall_resumes_after_loss_at_every_checkpoint() {
    for checkpoint in [
        LinuxUninstallCheckpoint::Settled,
        LinuxUninstallCheckpoint::PlatformRemoved,
        LinuxUninstallCheckpoint::ReleasesRemoved,
        LinuxUninstallCheckpoint::StateRemoved,
    ] {
        let fixture = Fixture::new();
        let location = fixture.default_location();
        fixture.legacy_install(&fixture.v1);
        fixture
            .run(&fixture.v2, Some(location.clone()), &fixture.private())
            .expect("adopted install");
        let mut host = UninstallHost {
            world: Rc::clone(&fixture.world),
            stop_at: Some(checkpoint),
        };
        assert!(
            run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).is_err(),
            "{checkpoint:?}"
        );
        // An installer never elects an independent store meanwhile: the
        // recorded authority stays elected while both roots exist, and once
        // either is gone election refuses rather than falling back.
        let both_roots = matches!(
            checkpoint,
            LinuxUninstallCheckpoint::Settled | LinuxUninstallCheckpoint::PlatformRemoved
        );
        match elect_linux_installation_with(&fixture.home, &fixture.private()) {
            Ok(LinuxInstallElection::Managed { authority, .. }) if both_roots => {
                assert_eq!(authority.location().state_root(), location.state_root());
            }
            Err(_) if !both_roots => {}
            Ok(_) => panic!("{checkpoint:?}: unexpected authority"),
            Err(error) => panic!("{checkpoint:?}: unexpected refusal {error}"),
        }
        host.stop_at = None;
        run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
            .unwrap_or_else(|error| panic!("{checkpoint:?}: resume failed: {error}"));
        assert!(!fixture.home.join(".local/lib/hypercolor").exists());
        assert!(!location.release_root().exists());
        assert!(!location.state_root().exists());
        let world = fixture.world.borrow();
        assert!(!world.loaded && !world.active);
    }
}

#[test]
fn uninstall_settles_an_interrupted_transaction_first() {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("install");
    let installed = fixture.world.borrow().effects.len();
    fixture.world.borrow_mut().crash = Some(Crash::After(installed + 1));
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        fixture.run(&fixture.v2, None, &fixture.private())
    }));
    assert!(crashed.is_err(), "{crashed:?}");
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect("uninstall after settling");
    assert!(run.recovered.is_some());
    assert!(!fixture.home.join(".local/lib/hypercolor").exists());
    assert!(!location.release_root().exists());
}

#[test]
fn uninstall_refuses_foreign_service_launcher_or_layout_without_writes() {
    for foreign in ["launcher", "layout", "fragment"] {
        let fixture = Fixture::new();
        let location = fixture.default_location();
        fixture
            .run(&fixture.v1, Some(location.clone()), &fixture.private())
            .expect("install");
        {
            let mut world = fixture.world.borrow_mut();
            match foreign {
                "launcher" => {
                    world.launcher_bytes.extend_from_slice(b"# local edit\n");
                    let bytes = world.launcher_bytes.clone();
                    world.launcher = LinuxExactEntry::RegularFile {
                        mode: 0o644,
                        sha256: sha256(&bytes),
                        snapshot_unit: None,
                        snapshot_path: None,
                    };
                }
                "layout" => {
                    world.layout.insert(
                        LinuxLayoutItem::HypercolorTui,
                        LinuxExactEntry::Symlink {
                            target: "/usr/bin/hypercolor-tui".to_owned(),
                        },
                    );
                }
                _ => world.fragment = "/usr/lib/systemd/user/hypercolor.service".to_owned(),
            }
        }
        let before = fixture.snapshot();
        let effects = fixture.world.borrow().effects.len();
        let mut host = UninstallHost {
            world: Rc::clone(&fixture.world),
            stop_at: None,
        };
        let error =
            run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect_err(foreign);
        assert!(
            matches!(error, LinuxInstallCommandError::ForeignInstallation(_)),
            "{foreign}: {error}"
        );
        assert_eq!(fixture.snapshot(), before, "{foreign}");
        assert_eq!(fixture.world.borrow().effects.len(), effects, "{foreign}");
        assert!(location.release_root().exists(), "{foreign}");
        assert!(location.state_root().exists(), "{foreign}");
        assert!(
            fixture
                .home
                .join(".local/lib/hypercolor/install-journal.json")
                .exists()
        );
    }
}

#[test]
fn uninstall_without_any_recorded_install_writes_nothing() {
    let fixture = Fixture::new();
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect("empty");
    assert_eq!(run, hypercolor_cli::install::LinuxUninstallRun::default());
    assert!(!fixture.home.join(".local").exists());
}

#[test]
fn uninstall_removes_a_historical_lib_root_install() {
    let fixture = Fixture::new();
    fixture.legacy_install(&fixture.v1);
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect("legacy");
    assert_eq!(
        run.removed,
        vec![fixture.home.join(".local/lib/hypercolor")]
    );
    assert!(run.preserved.is_empty());
    assert!(!fixture.home.join(".local/lib/hypercolor").exists());
    assert!(!fixture.world.borrow().active);
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("mode").permissions().mode() & 0o7777
}

fn writable(root: &Path) {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o755));
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            writable(&entry.path());
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn launcher_exec(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .expect("launcher UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .unwrap_or_default()
        .to_owned()
}

fn same_entry(left: &LinuxExactEntry, right: &LinuxExactEntry) -> bool {
    match (left, right) {
        (LinuxExactEntry::Absent, LinuxExactEntry::Absent) => true,
        (LinuxExactEntry::Symlink { target: left }, LinuxExactEntry::Symlink { target: right }) => {
            left == right
        }
        (
            LinuxExactEntry::RegularFile {
                mode: left_mode,
                sha256: left_digest,
                ..
            },
            LinuxExactEntry::RegularFile {
                mode: right_mode,
                sha256: right_digest,
                ..
            },
        ) => left_mode == right_mode && left_digest == right_digest,
        _ => false,
    }
}

fn write_release(root: &Path, version: &str, daemon: &[u8]) {
    write_release_with(root, version, daemon, |_| {});
}

/// A release whose manifest `edit` changed before it was written.
fn write_release_with(
    root: &Path,
    version: &str,
    daemon: &[u8],
    edit: impl FnOnce(&mut serde_json::Value),
) {
    let directories = [
        "bin",
        "share",
        "share/hypercolor",
        "share/hypercolor/ui",
        "share/hypercolor/effects",
        "share/hypercolor/effects/bundled",
        "share/hypercolor/docs",
        "share/hypercolor/agents",
        "share/hypercolor/agents/skills",
        "share/hypercolor/agents/agents",
        "share/hypercolor/skills",
        "share/hypercolor/site",
    ];
    let cli = format!("hypercolor cli {version}");
    let files = [
        ("bin/hypercolor-daemon", daemon),
        ("bin/hypercolor", cli.as_bytes()),
        ("bin/hypercolor-app", b"app".as_slice()),
        ("bin/hypercolor-tui", b"tui".as_slice()),
        ("bin/hypercolor-open", b"open".as_slice()),
        ("share/hypercolor/ui/index.html", b"ui".as_slice()),
        (
            "share/hypercolor/effects/bundled/effect.html",
            b"effect".as_slice(),
        ),
        (
            "share/hypercolor/agents/skills/skill.md",
            b"skill".as_slice(),
        ),
        (
            "share/hypercolor/agents/agents/agent.md",
            b"agent".as_slice(),
        ),
        ("share/hypercolor/skills/skill.md", b"user skill".as_slice()),
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755)).expect("mode");
        members.push(json!({"path":directory,"type":"directory","mode":0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), bytes).expect("file");
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode)).expect("mode");
        members.push(json!({
            "path":path,"type":"file","mode":mode,"size":bytes.len(),"sha256":sha256(bytes)
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let mut manifest = json!({
        "name":"hypercolor","version":version,"platform":"linux-x86_64",
        "rust_target":"x86_64-unknown-linux-gnu",
        "binaries":["hypercolor-daemon","hypercolor","hypercolor-app","hypercolor-tui","hypercolor-open"],
        "assets":{"ui_files":1,"bundled_effect_files":1,"docs_files":0,"skill_files":1,
            "user_skill_files":1,"agent_files":1,"site_files":0},
        "managed_package":{
            "schema_version":1,"owner":"linux-user-tarball","launcher_contract":1,
            "components":{"daemon":"bin/hypercolor-daemon","cli":"bin/hypercolor",
                "ui":"share/hypercolor/ui","bundled_effects":"share/hypercolor/effects/bundled"},
            "compatibility":{"stores":[{"name":"config","storage_format":"toml",
                "readable_schema_min":4,"readable_schema_max":5,"written_schema":5,
                "migration_mode":"backward_compatible"}]},
        },
        "members":members,
    });
    edit(&mut manifest);
    let manifest = serde_json::to_vec_pretty(&manifest).expect("manifest JSON");
    fs::write(root.join("manifest.json"), manifest).expect("manifest");
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("manifest mode");
}

#[test]
fn longest_supported_roots_fit_the_transaction_record_and_longer_ones_refuse_early() {
    let fixture = Fixture::with_home(Some(256));
    assert_eq!(fixture.home.as_os_str().len(), 256);
    let home = fixture.home.to_str().expect("UTF-8").len();
    let pad = |letter: &str, root_suffix: usize| letter.repeat(512 - root_suffix - home - 1);
    let location = fixture.location(
        &pad("d", "/hypercolor/releases".len()),
        &pad("s", "/hypercolor/update".len()),
        &pad("c", "/hypercolor".len()),
    );
    for root in [
        location.release_root(),
        location.state_root(),
        location.config_root(),
    ] {
        assert_eq!(root.as_os_str().len(), 512);
    }
    fixture.legacy_install(&fixture.v1);
    let run = fixture
        .run(&fixture.v2, Some(location.clone()), &fixture.private())
        .expect("longest supported roots adopt");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    fixture.assert_managed(&location, &fixture.v2.id);
    let journal = fs::read(location.state_root().join("install-journal.json")).expect("journal");
    let value: serde_json::Value = serde_json::from_slice(&journal).expect("journal JSON");
    let record = value["platform_record"]["payload"]
        .as_array()
        .expect("record bytes")
        .len();
    eprintln!(
        "longest-root record {record} bytes, journal {} bytes",
        journal.len()
    );
    assert!(
        record * 4 < hypercolor_cli::install::MAX_LINUX_TRANSACTION_RECORD_BYTES * 3,
        "the longest record must keep a quarter of its budget free: {record}"
    );
    assert!(
        journal.len() * 4 < hypercolor_cli::install::MAX_MANAGED_INSTALL_JOURNAL_BYTES * 3,
        "the longest journal must keep a quarter of its budget free: {}",
        journal.len()
    );

    let too_long = LinuxInstallLocation::new(
        &fixture.home,
        &fixture
            .home
            .join(pad("d", "/hypercolor/releases".len() - 1)),
        &fixture.home.join("state"),
        &fixture.home.join("config"),
        fixture.uid,
    );
    assert!(matches!(
        too_long,
        Err(hypercolor_cli::install::InstallLocationError::PathTooLong { limit: 512, .. })
    ));
    let long_home = fixture.home.join("x");
    assert!(matches!(
        LinuxInstallLocation::new(
            &long_home,
            &long_home.join("data"),
            &long_home.join("state"),
            &long_home.join("config"),
            fixture.uid,
        ),
        Err(hypercolor_cli::install::InstallLocationError::PathTooLong { limit: 256, .. })
    ));
}

// ── Existing ancestors above recorded roots ─────────────────────────────

fn existing_ancestor_fixture(ancestor: &str, mode: u32, root: &str) -> Fixture {
    let fixture = Fixture::new();
    let root = fixture.home.join(root);
    fs::create_dir_all(&root).expect("existing root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("root mode");
    fs::set_permissions(
        fixture.home.join(ancestor),
        fs::Permissions::from_mode(mode),
    )
    .expect("ancestor mode");
    fixture
}

#[test]
fn existing_ancestors_above_recorded_roots_must_be_trusted() {
    // World-writable above an already existing data root.
    let fixture = existing_ancestor_fixture(".local/share", 0o777, ".local/share/hypercolor");
    let error = fixture
        .run(
            &fixture.v1,
            Some(fixture.default_location()),
            &fixture.private(),
        )
        .expect_err("world-writable ancestor");
    assert!(
        error.to_string().contains("writable by every user"),
        "{error}"
    );
    assert_legacy_untouched(&fixture);
    fs::set_permissions(
        fixture.home.join(".local/share"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("cleanup");

    // A custom XDG base inside a world-writable, non-sticky directory.
    let fixture = existing_ancestor_fixture("shared", 0o777, "shared/data");
    let custom = fixture.location("shared/data", ".local/state", ".config");
    let error = fixture
        .run(&fixture.v1, Some(custom), &fixture.private())
        .expect_err("world-writable custom base");
    assert!(error.to_string().contains("shared"), "{error}");
    assert_legacy_untouched(&fixture);
    fs::set_permissions(
        fixture.home.join("shared"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("cleanup");

    // A shared group above an existing root, and the same mode when the
    // group is private.
    let fixture = existing_ancestor_fixture(".local/share", 0o775, ".local/share/hypercolor");
    let shared = policy(Principals::shared(fixture.uid, fixture.gid));
    let error = fixture
        .run(&fixture.v1, Some(fixture.default_location()), &shared)
        .expect_err("shared-group ancestor");
    assert!(error.to_string().contains("neighbor"), "{error}");
    assert_legacy_untouched(&fixture);
    fixture
        .run(
            &fixture.v1,
            Some(fixture.default_location()),
            &fixture.private(),
        )
        .expect("private-group ancestor");
    fixture.assert_managed(&fixture.default_location(), &fixture.v1.id);

    // An ancestor that turns world-writable after install stops the next run
    // and the uninstall before either writes.
    fs::set_permissions(
        fixture.home.join(".local/share"),
        fs::Permissions::from_mode(0o777),
    )
    .expect("later world-writable ancestor");
    let before = fixture.snapshot();
    assert!(fixture.run(&fixture.v2, None, &fixture.private()).is_err());
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let error = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect_err("uninstall under an untrusted ancestor");
    assert!(
        matches!(error, LinuxInstallCommandError::UnsafeDirectory(..)),
        "{error}"
    );
    assert_eq!(fixture.snapshot(), before, "nothing changed");
    assert!(fixture.default_location().release_root().exists());
    fs::set_permissions(
        fixture.home.join(".local/share"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("cleanup");
}

// ── Loss before publication, then the world changes ─────────────────────

const UNPUBLISHED: [LinuxInstallCheckpoint; 7] = [
    LinuxInstallCheckpoint::RootsBootstrapped,
    LinuxInstallCheckpoint::IntentRecorded,
    LinuxInstallCheckpoint::PriorCopied,
    LinuxInstallCheckpoint::PriorActivated,
    LinuxInstallCheckpoint::CandidateStaged,
    LinuxInstallCheckpoint::AdoptionReceipt,
    LinuxInstallCheckpoint::StateJournal,
];

impl Fixture {
    /// Adopt with `release` into `location`, stopping at `checkpoint`.
    fn interrupted_adoption(
        &self,
        release: &Release,
        location: &LinuxInstallLocation,
        checkpoint: LinuxInstallCheckpoint,
    ) {
        let mut host = Host::new(&self.world, release, Some(location.clone()));
        host.stop_at = Some(checkpoint);
        assert!(
            run_linux_install(
                &self.home,
                &release.request(InstallTargetPolicy::Preserve),
                &self.private(),
                &mut host,
            )
            .is_err(),
            "{checkpoint:?} did not stop the run"
        );
        assert!(matches!(
            elect_linux_installation_with(&self.home, &self.private()).expect("election"),
            LinuxInstallElection::Legacy { .. }
        ));
    }

    fn adopt(&self, release: &Release, proposal: Option<LinuxInstallLocation>) -> usize {
        let mut host = Host::new(&self.world, release, proposal);
        let run = run_linux_install(
            &self.home,
            &release.request(InstallTargetPolicy::Preserve),
            &self.private(),
            &mut host,
        )
        .unwrap_or_else(|error| {
            panic!(
                "adoption after an unpublished loss: {error:?}; seen {:?}",
                host.seen
            )
        });
        assert_eq!(
            run.outcome,
            InstallOutcome::Committed {
                active_unit: release.id.clone()
            }
        );
        host.proposals
    }
}

#[test]
fn unpublished_loss_then_a_service_restart_prepares_again() {
    for checkpoint in UNPUBLISHED {
        let fixture = Fixture::new();
        fixture.legacy_install(&fixture.v1);
        let location = fixture.default_location();
        fixture.interrupted_adoption(&fixture.v2, &location, checkpoint);
        // A reboot or crash restarts the historical daemon under a new
        // invocation, so a recorded prior no longer matches the platform.
        {
            let mut world = fixture.world.borrow_mut();
            world.stop();
            world.start();
        }
        fixture.adopt(&fixture.v2, Some(location.clone()));
        fixture.assert_managed(&location, &fixture.v2.id);
    }
}

#[test]
fn unpublished_loss_then_changed_xdg_resumes_the_recorded_target() {
    for checkpoint in UNPUBLISHED {
        let fixture = Fixture::new();
        fixture.legacy_install(&fixture.v1);
        let recorded = fixture.location("xdg-a/data", "xdg-a/state", "xdg-a/config");
        fixture.interrupted_adoption(&fixture.v2, &recorded, checkpoint);
        let changed = fixture.location("xdg-b/data", "xdg-b/state", "xdg-b/config");
        if checkpoint == LinuxInstallCheckpoint::RootsBootstrapped {
            // The target is recorded only after its roots are proven, so a
            // loss before that leaves the environment in charge and the
            // bootstrapped roots inert: no journal, receipt or unit.
            assert_eq!(fixture.adopt(&fixture.v2, Some(changed.clone())), 1);
            fixture.assert_managed(&changed, &fixture.v2.id);
            assert!(!recorded.state_root().join("install-journal.json").exists());
            assert!(!recorded.release_root().join("units").exists());
            continue;
        }
        assert_eq!(
            fixture.adopt(&fixture.v2, Some(changed)),
            0,
            "{checkpoint:?}"
        );
        fixture.assert_managed(&recorded, &fixture.v2.id);
        assert!(!fixture.home.join("xdg-b").exists(), "{checkpoint:?}");
    }
}

#[test]
fn unpublished_loss_then_another_candidate_or_historical_install_prepares_again() {
    for checkpoint in UNPUBLISHED {
        // A newer candidate arrives before the rerun.
        let fixture = Fixture::new();
        fixture.legacy_install(&fixture.v1);
        let location = fixture.default_location();
        fixture.interrupted_adoption(&fixture.v2, &location, checkpoint);
        fixture.adopt(&fixture.v3, Some(location.clone()));
        fixture.assert_managed(&location, &fixture.v3.id);

        // An older installer changes the historical install in between.
        let fixture = Fixture::new();
        let location = fixture.default_location();
        fixture.legacy_install(&fixture.v1);
        fixture.interrupted_adoption(&fixture.v2, &location, checkpoint);
        fixture.legacy_install(&fixture.v3);
        fixture.adopt(&fixture.v2, Some(location.clone()));
        fixture.assert_managed(&location, &fixture.v2.id);
        let old = fixture.legacy();
        let lock = old.acquire_lock().expect("historical lock");
        assert_eq!(
            old.active_unit(&lock).expect("pointer"),
            Some(fixture.v3.id.clone()),
            "{checkpoint:?}: the historical pointer the older installer set is kept"
        );
    }
}

#[test]
fn uninstall_after_an_unpublished_loss_removes_the_prepared_roots() {
    for checkpoint in UNPUBLISHED {
        let fixture = Fixture::new();
        fixture.legacy_install(&fixture.v1);
        let location = fixture.location("xdg-a/data", "xdg-a/state", "xdg-a/config");
        fixture.interrupted_adoption(&fixture.v2, &location, checkpoint);
        let mut host = UninstallHost {
            world: Rc::clone(&fixture.world),
            stop_at: None,
        };
        let run =
            run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect("uninstall");
        let legacy = fixture.home.join(".local/lib/hypercolor");
        assert!(!legacy.exists(), "{checkpoint:?}");
        assert!(run.removed.contains(&legacy), "{checkpoint:?}");
        if checkpoint == LinuxInstallCheckpoint::RootsBootstrapped {
            // Unrecorded, the bootstrapped roots stay behind holding only
            // empty directories and their identity, never a journal.
            assert!(!location.state_root().join("install-journal.json").exists());
        } else {
            let state = location.state_root().to_path_buf();
            assert!(!state.exists(), "{checkpoint:?}");
            assert!(
                run.removed.contains(&state),
                "{checkpoint:?}: {:?}",
                run.removed
            );
            assert!(!location.release_root().exists(), "{checkpoint:?}");
        }
        assert!(
            location.data_root().exists(),
            "{checkpoint:?}: data is preserved"
        );
        // A fresh install afterwards is not blocked by leftovers.
        let fresh = fixture.location("xdg-a/data", "xdg-a/state", "xdg-a/config");
        fixture
            .run(&fixture.v2, Some(fresh.clone()), &fixture.private())
            .expect("fresh install after uninstall");
        fixture.assert_managed(&fresh, &fixture.v2.id);
    }
}

#[test]
fn uninstall_finishes_a_hidden_historical_root_and_never_blocks_a_new_install() {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("install");
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: Some(LinuxUninstallCheckpoint::StateRemoved),
    };
    assert!(run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).is_err());
    // Model a loss right after the historical root was renamed away.
    let lib = fixture.home.join(".local/lib");
    fs::rename(
        lib.join("hypercolor"),
        lib.join(".hypercolor-removing-hypercolor.77-1"),
    )
    .expect("tombstone");
    host.stop_at = None;
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect("finish the hidden root");
    assert_eq!(run.removed, vec![lib.join("hypercolor")]);
    assert_eq!(
        fs::read_dir(&lib).expect("lib").count(),
        0,
        "no tombstone remains"
    );

    // A tombstone never blocks a new install, and the next uninstall sweeps it.
    fs::create_dir(lib.join(".hypercolor-removing-hypercolor.78-1")).expect("tombstone");
    fixture
        .run(&fixture.v2, Some(location.clone()), &fixture.private())
        .expect("fresh install beside a tombstone");
    fixture.assert_managed(&location, &fixture.v2.id);
    run_linux_uninstall(&fixture.home, &fixture.private(), &mut host).expect("uninstall");
    assert_eq!(fs::read_dir(&lib).expect("lib").count(), 0);
}

#[test]
fn uninstall_removes_an_install_whose_journal_cannot_settle() {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("install");
    let performed = fixture.world.borrow().effects.len();
    fixture.world.borrow_mut().crash = Some(Crash::After(performed + 1));
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        fixture.run(&fixture.v2, None, &fixture.private())
    }));
    assert!(crashed.is_err());
    // Someone toggles autostart behind the journal's back. Unlike a service
    // restart, no recovery rule explains a changed service definition.
    let enabled = fixture.world.borrow().enabled;
    fixture.world.borrow_mut().enabled = !enabled;
    assert!(
        fixture.run(&fixture.v2, None, &fixture.private()).is_err(),
        "recovery cannot settle this drift"
    );
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect("uninstall despite the unsettled journal");
    assert!(run.unsettled.is_some());
    assert!(!location.release_root().exists());
    assert!(!location.state_root().exists());
    assert!(!fixture.home.join(".local/lib/hypercolor").exists());
    let world = fixture.world.borrow();
    assert!(!world.loaded && !world.active);
}

#[test]
fn a_refused_proposal_never_pins_later_attempts_or_blocks_uninstall() {
    let fixture = Fixture::new();
    fixture.legacy_install(&fixture.v1);
    fs::create_dir_all(fixture.home.join("shared/data")).expect("unsafe base");
    fs::set_permissions(
        fixture.home.join("shared"),
        fs::Permissions::from_mode(0o777),
    )
    .expect("world-writable parent");
    let unsafe_target = fixture.location("shared/data", ".local/state", ".config");
    assert!(
        fixture
            .run(&fixture.v2, Some(unsafe_target), &fixture.private())
            .is_err()
    );
    assert!(
        !fixture
            .home
            .join(".local/lib/hypercolor/managed-adoption.json")
            .exists(),
        "a refused proposal is never recorded"
    );
    let safe = fixture.default_location();
    let mut host = Host::new(&fixture.world, &fixture.v2, Some(safe.clone()));
    run_linux_install(
        &fixture.home,
        &fixture.v2.request(InstallTargetPolicy::Preserve),
        &fixture.private(),
        &mut host,
    )
    .expect("a safe environment proceeds");
    fixture.assert_managed(&safe, &fixture.v2.id);

    // The same refusal against a still-historical install leaves uninstall
    // free to remove it.
    let fixture = Fixture::new();
    fixture.legacy_install(&fixture.v1);
    fs::create_dir_all(fixture.home.join("shared/data")).expect("unsafe base");
    fs::set_permissions(
        fixture.home.join("shared"),
        fs::Permissions::from_mode(0o777),
    )
    .expect("world-writable parent");
    let unsafe_target = fixture.location("shared/data", ".local/state", ".config");
    assert!(
        fixture
            .run(&fixture.v2, Some(unsafe_target), &fixture.private())
            .is_err()
    );
    let mut uninstall = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    run_linux_uninstall(&fixture.home, &fixture.private(), &mut uninstall)
        .expect("uninstall the historical install");
    assert!(!fixture.home.join(".local/lib/hypercolor").exists());
    fs::set_permissions(
        fixture.home.join("shared"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("cleanup");
}

#[test]
fn public_directories_writable_by_another_account_refuse_before_any_platform_write() {
    for public in [".local/bin", ".config/systemd/user"] {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.home.join(public)).expect("public directory");
        fs::set_permissions(fixture.home.join(public), fs::Permissions::from_mode(0o777))
            .expect("world-writable public directory");
        let error = fixture
            .run(
                &fixture.v1,
                Some(fixture.default_location()),
                &fixture.private(),
            )
            .expect_err("unsafe public tree");
        assert!(error.to_string().contains(public), "{public}: {error}");
        assert_legacy_untouched(&fixture);
        fs::set_permissions(fixture.home.join(public), fs::Permissions::from_mode(0o755))
            .expect("restore");

        // Installed first, then made unsafe: uninstall refuses unchanged.
        fixture
            .run(
                &fixture.v1,
                Some(fixture.default_location()),
                &fixture.private(),
            )
            .expect("install");
        fs::set_permissions(fixture.home.join(public), fs::Permissions::from_mode(0o777))
            .expect("later unsafe public directory");
        let before = fixture.snapshot();
        let mut host = UninstallHost {
            world: Rc::clone(&fixture.world),
            stop_at: None,
        };
        let error = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
            .expect_err("uninstall under an unsafe public directory");
        assert!(error.to_string().contains(public), "{public}: {error}");
        assert_eq!(fixture.snapshot(), before, "{public}");
        assert!(fixture.default_location().release_root().exists());
        fs::set_permissions(fixture.home.join(public), fs::Permissions::from_mode(0o755))
            .expect("cleanup");
    }
}

// ── Restarted, autostarted and transitional services ───────────────────

/// A committed managed install of v1 at the default roots, running.
fn managed_v1() -> (Fixture, LinuxInstallLocation) {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    let run = fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("managed install");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v1.id.clone()
        }
    );
    (fixture, location)
}

impl Fixture {
    fn journal(&self) -> InstallJournalV1 {
        let LinuxInstallElection::Managed { store, lock, .. } =
            elect_linux_installation_with(&self.home, &self.private()).expect("managed election")
        else {
            panic!("expected managed authority");
        };
        store
            .load_journal(&lock)
            .expect("journal")
            .expect("present")
    }

    fn update(
        &self,
        release: &Release,
    ) -> Result<hypercolor_cli::install::LinuxInstallRun, LinuxInstallCommandError> {
        self.run(release, None, &self.private())
    }

    /// The service runs, steadily, whatever recovery just did.
    fn assert_settled_service(&self, context: &str) {
        let world = self.world.borrow();
        assert!(world.active, "{context}: a release runs");
        assert!(
            !world.auto_restart && !world.failed && !world.queued_start,
            "{context}: nothing is left mid-transition"
        );
    }
}

type WorldEvent = fn(&mut World);

/// What happens to a running candidate after its owner receipt.
fn candidate_events() -> [(&'static str, WorldEvent); 4] {
    [
        ("restarts under a new invocation", World::restart_service),
        ("crash loops", World::crash_to_auto_restart),
        ("ends failed at its start limit", World::crash_to_failed),
        ("is stopped", World::stop),
    ]
}

#[test]
fn a_candidate_that_restarts_or_stops_after_its_receipt_rolls_back() {
    for (name, event) in candidate_events() {
        let (fixture, location) = managed_v1();
        fixture.world.borrow_mut().at_health = Some(("9.8.8", HealthEvent::InstallerDies));
        let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
        assert!(crashed.is_err(), "{name}: the installer died in the proof");
        let pending = fixture.journal();
        assert_eq!(
            (pending.disposition, pending.next_action),
            (
                InstallDisposition::Forward,
                Some(InstallAction::ProveCandidate)
            ),
            "{name}"
        );
        assert!(pending.candidate_owner_receipt.is_some(), "{name}");

        event(&mut fixture.world.borrow_mut());
        let run = fixture
            .update(&fixture.v2)
            .unwrap_or_else(|error| panic!("{name}: recovery stopped: {error}"));
        assert!(run.recovered, "{name}");
        assert!(
            matches!(
                run.outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "{name}: {:?}",
            run.outcome
        );
        fixture.assert_managed(&location, &fixture.v1.id);
        fixture.assert_settled_service(name);
        assert_eq!(
            fixture.world.borrow().resets,
            usize::from(name.contains("failed")),
            "{name}: only a failed service needs a reset before the prior starts"
        );

        let retry = fixture.update(&fixture.v2).expect("retry after rollback");
        assert_eq!(
            retry.outcome,
            InstallOutcome::Committed {
                active_unit: fixture.v2.id.clone()
            },
            "{name}"
        );
        fixture.assert_managed(&location, &fixture.v2.id);
    }
}

#[test]
fn a_candidate_that_crashes_during_its_proof_rolls_back_in_the_same_run() {
    for event in [HealthEvent::Restarts, HealthEvent::CrashLoops] {
        let (fixture, location) = managed_v1();
        fixture.world.borrow_mut().at_health = Some(("9.8.8", event));
        let run = fixture
            .update(&fixture.v2)
            .unwrap_or_else(|error| panic!("{event:?}: the run stopped: {error}"));
        assert!(!run.recovered, "{event:?}");
        assert!(
            matches!(
                run.outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "{event:?}: {:?}",
            run.outcome
        );
        fixture.assert_managed(&location, &fixture.v1.id);
        fixture.assert_settled_service(&format!("{event:?}"));
    }
}

#[test]
fn a_candidate_that_never_becomes_ready_rolls_back_and_resets_a_failed_service() {
    // A daemon that exits before readiness, or outlives its unit's start
    // timeout, fails its start job and then waits to restart or, past its
    // start limit, ends failed.
    for ends_failed in [false, true] {
        let (fixture, location) = managed_v1();
        {
            let mut world = fixture.world.borrow_mut();
            world.failing_starts.insert("9.8.8".to_owned());
            world.failing_start_ends_failed = ends_failed;
        }
        let run = fixture
            .update(&fixture.v2)
            .unwrap_or_else(|error| panic!("ends_failed={ends_failed}: {error}"));
        assert!(
            matches!(
                run.outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "ends_failed={ends_failed}: {:?}",
            run.outcome
        );
        fixture.assert_managed(&location, &fixture.v1.id);
        fixture.assert_settled_service("never ready");
        assert_eq!(
            fixture.world.borrow().resets,
            usize::from(ends_failed),
            "the prior starts only after a failed service is reset"
        );
    }
}

#[test]
fn a_prior_that_restarted_before_it_was_unloaded_abandons_without_any_effect() {
    let (fixture, location) = managed_v1();
    let before = fixture.world.borrow().effects.len();
    // Die right after the prior's stop, before the journal records it.
    fixture.world.borrow_mut().crash = Some(Crash::After(before + 1));
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err());
    assert_eq!(
        fixture.world.borrow().effects[before],
        "runtime:false",
        "the first update effect unloads the prior"
    );
    let pending = fixture.journal();
    assert_eq!(pending.next_action, Some(InstallAction::UnloadPrior));
    // The prior comes back under a new invocation, so the baseline the
    // transaction proved is gone.
    fixture.world.borrow_mut().start();
    let effects = fixture.world.borrow().effects.len();

    let run = fixture.update(&fixture.v2).expect("abandoned recovery");
    assert!(run.recovered);
    let InstallOutcome::RolledBack {
        active_unit,
        failure,
        abandoned,
        restored,
    } = run.outcome
    else {
        panic!("expected an abandoned rollback: {:?}", run.outcome);
    };
    assert!(abandoned);
    assert_eq!(restored, None, "an abandonment proves no restored release");
    assert_eq!(active_unit, Some(fixture.v1.id.clone()));
    assert!(failure.contains("abandoned at UnloadPrior"), "{failure}");
    assert_eq!(
        fixture.world.borrow().effects.len(),
        effects,
        "abandonment changes nothing"
    );
    let journal = fixture.journal();
    assert!(journal.abandoned);
    assert_eq!(journal.disposition, InstallDisposition::RolledBack);
    fixture.assert_managed(&location, &fixture.v1.id);

    let retry = fixture
        .update(&fixture.v2)
        .expect("retry after abandonment");
    assert_eq!(
        retry.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    assert!(!fixture.journal().abandoned);
}

#[test]
fn a_prior_waiting_to_restart_at_unload_is_drift_until_it_settles() {
    let (fixture, location) = managed_v1();
    let before = fixture.world.borrow().effects.len();
    // Die right after the prior's stop, before the journal records it.
    fixture.world.borrow_mut().crash = Some(Crash::After(before + 1));
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err());
    let pending = fixture.journal();
    assert_eq!(pending.next_action, Some(InstallAction::UnloadPrior));
    // The prior came back and crashed again, so the manager is waiting to
    // restart it: not stopped, and not running under a new invocation
    // either, so nothing proves the transaction never changed anything.
    {
        let mut world = fixture.world.borrow_mut();
        world.start();
        world.crash_to_auto_restart();
    }
    let effects = fixture.world.borrow().effects.len();
    let error = fixture
        .update(&fixture.v2)
        .expect_err("a restarting prior is neither untouched nor unloaded");
    assert!(error.to_string().contains("drift"), "{error}");
    assert_eq!(fixture.journal(), pending, "the journal did not move");
    assert_eq!(fixture.world.borrow().effects.len(), effects);
    assert!(fixture.world.borrow().auto_restart, "nothing was stopped");

    // The prior hits its start limit and ends failed, which is the stopped
    // state the unload step expects, so the update resumes and commits.
    {
        let mut world = fixture.world.borrow_mut();
        world.auto_restart = false;
        world.failed = true;
    }
    let run = fixture
        .update(&fixture.v2)
        .expect("resume once the prior settled");
    assert!(run.recovered);
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    assert!(!fixture.journal().abandoned);
    fixture.assert_managed(&location, &fixture.v2.id);
    fixture.assert_settled_service("after the prior settled");
}

#[test]
fn stop_first_refuses_a_service_that_is_not_the_on_disk_unit() {
    let (fixture, _) = managed_v1();
    let before = fixture.world.borrow().effects.len();
    // Die before the second effect. Between releases of one root the
    // launcher and layout are already exact, so the prior is stopped, the
    // pointer names the candidate and the manager reload is next.
    fixture.world.borrow_mut().crash = Some(Crash::Before(before + 2));
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err());
    let pending = fixture.journal();
    assert_eq!(
        pending.next_action,
        Some(InstallAction::ReloadCandidateManager)
    );
    // Something runs as hypercolor.service whose process is not the daemon
    // the on-disk unit names.
    {
        let mut world = fixture.world.borrow_mut();
        world.start();
        world
            .process
            .as_mut()
            .expect("running")
            .sha256
            .replace_range(.., &"0".repeat(64));
    }
    let effects = fixture.world.borrow().effects.len();
    let error = fixture
        .update(&fixture.v2)
        .expect_err("a foreign process is never stopped as ours");
    assert!(
        error
            .to_string()
            .contains("not the daemon its on-disk unit names"),
        "{error}"
    );
    assert!(
        fixture.world.borrow().active,
        "the foreign process still runs"
    );
    assert_eq!(fixture.world.borrow().effects.len(), effects);
    assert_eq!(fixture.journal(), pending, "the journal did not move");
}

#[test]
fn a_service_still_restarting_blocks_preparation_without_platform_writes() {
    let (fixture, _) = managed_v1();
    fixture.world.borrow_mut().crash_to_auto_restart();
    let effects = fixture.world.borrow().effects.len();
    let committed = fixture.journal();
    let error = fixture
        .update(&fixture.v2)
        .expect_err("a transitional prior cannot be a baseline");
    assert!(
        error
            .to_string()
            .contains("still starting, stopping or restarting"),
        "{error}"
    );
    assert_eq!(fixture.world.borrow().effects.len(), effects);
    assert_eq!(fixture.journal(), committed);

    // Once the manager brings it back, the update proceeds.
    {
        let mut world = fixture.world.borrow_mut();
        world.auto_restart = false;
        world.start();
    }
    let run = fixture.update(&fixture.v2).expect("update after settling");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
}

#[test]
fn uninstall_stops_a_service_that_is_waiting_to_restart() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().crash_to_auto_restart();
    let mut host = UninstallHost {
        world: Rc::clone(&fixture.world),
        stop_at: None,
    };
    let run = run_linux_uninstall(&fixture.home, &fixture.private(), &mut host)
        .expect("uninstall a crash-looping service");
    assert!(run.unsettled.is_none());
    assert!(!location.release_root().exists());
    let world = fixture.world.borrow();
    assert!(!world.active && !world.auto_restart && !world.loaded);
}

#[derive(Debug, Clone, Copy)]
enum LossPoint {
    BeforeEffect(usize),
    AfterEffect(usize),
    AtObservation(usize),
}

/// Probe one update's effects and systemd observations.
fn update_extent(failing_candidate: bool) -> (usize, usize) {
    let (probe, _) = managed_v1();
    if failing_candidate {
        probe
            .world
            .borrow_mut()
            .failing_starts
            .insert("9.8.8".to_owned());
    }
    let (effects, shows) = {
        let world = probe.world.borrow();
        (world.effects.len(), world.shows)
    };
    probe.update(&probe.v2).expect("uninterrupted update");
    let world = probe.world.borrow();
    (world.effects.len() - effects, world.shows - shows)
}

/// How recoveries in the loss matrix ended.
#[derive(Debug, Default)]
struct RecoveryTally {
    committed: usize,
    rolled_back: usize,
    abandoned: usize,
    /// Recoveries that first stopped a service the manager had started on
    /// its own, then went forward to commit.
    stopped_then_committed: usize,
}

impl RecoveryTally {
    fn add(&mut self, other: &Self) {
        self.committed += other.committed;
        self.rolled_back += other.rolled_back;
        self.abandoned += other.abandoned;
        self.stopped_then_committed += other.stopped_then_committed;
    }
}

/// Lose the installer at `point` during an update, optionally lose power
/// too, then recover and prove exactly one release runs, settled.
fn recover_after_loss(
    point: LossPoint,
    power_loss: bool,
    failing_candidate: bool,
) -> RecoveryTally {
    let context =
        format!("{point:?} power_loss={power_loss} failing_candidate={failing_candidate}");
    let (fixture, location) = managed_v1();
    {
        let mut world = fixture.world.borrow_mut();
        if failing_candidate {
            world.failing_starts.insert("9.8.8".to_owned());
        }
        let (effects, shows) = (world.effects.len(), world.shows);
        match point {
            LossPoint::BeforeEffect(effect) => world.crash = Some(Crash::Before(effects + effect)),
            LossPoint::AfterEffect(effect) => world.crash = Some(Crash::After(effects + effect)),
            LossPoint::AtObservation(show) => world.crash_at_show = Some(shows + show),
        }
    }
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err(), "{context}: the loss interrupted the run");
    {
        let mut world = fixture.world.borrow_mut();
        world.crash = None;
        world.crash_at_show = None;
        if power_loss {
            world.power_cycle();
        }
    }
    let effects = fixture.world.borrow().effects.len();
    let run = fixture
        .update(&fixture.v2)
        .unwrap_or_else(|error| panic!("{context}: recovery stopped: {error}"));
    let recovery_effects = fixture.world.borrow().effects[effects..].to_vec();
    let mut tally = RecoveryTally::default();
    match run.outcome {
        InstallOutcome::Committed { active_unit } => {
            assert!(
                !failing_candidate,
                "{context}: a failing candidate committed"
            );
            assert_eq!(active_unit, fixture.v2.id, "{context}");
            fixture.assert_managed(&location, &fixture.v2.id);
            tally.committed += 1;
            if recovery_effects.first().map(String::as_str) == Some("runtime:false")
                && recovery_effects
                    .iter()
                    .any(|effect| effect == "runtime:true")
            {
                tally.stopped_then_committed += 1;
            }
        }
        InstallOutcome::RolledBack { abandoned, .. } => {
            // Losing only the installer never costs a healthy candidate its
            // update: the service it left behind still proves or resumes.
            assert!(
                failing_candidate || power_loss,
                "{context}: a healthy candidate rolled back after an installer loss"
            );
            fixture.assert_managed(&location, &fixture.v1.id);
            if abandoned {
                assert!(
                    recovery_effects.is_empty(),
                    "{context}: abandonment changes nothing, saw {recovery_effects:?}"
                );
                tally.abandoned += 1;
            } else {
                tally.rolled_back += 1;
            }
        }
    }
    fixture.assert_settled_service(&context);
    assert!(
        matches!(
            fixture.journal().disposition,
            InstallDisposition::Committed | InstallDisposition::RolledBack
        ),
        "{context}"
    );
    tally
}

#[test]
fn loss_and_power_loss_at_every_update_boundary_recover_to_exactly_one_release() {
    let mut cases = Vec::new();
    for failing_candidate in [false, true] {
        let (effects, observations) = update_extent(failing_candidate);
        assert!(
            effects >= 3 && observations > effects,
            "{effects} effects, {observations} observations"
        );
        let points = (1..=effects)
            .flat_map(|effect| {
                [
                    LossPoint::BeforeEffect(effect),
                    LossPoint::AfterEffect(effect),
                ]
            })
            .chain((1..=observations).map(LossPoint::AtObservation));
        for point in points {
            for power_loss in [false, true] {
                cases.push((point, power_loss, failing_candidate));
            }
        }
    }
    // Every case owns its fixture, so the matrix runs in parallel.
    let workers = std::thread::available_parallelism().map_or(4, |count| count.get().min(8));
    let tally = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let cases = &cases;
                scope.spawn(move || {
                    let mut tally = RecoveryTally::default();
                    for (point, power_loss, failing_candidate) in
                        cases.iter().skip(worker).step_by(workers)
                    {
                        tally.add(&recover_after_loss(*point, *power_loss, *failing_candidate));
                    }
                    tally
                })
            })
            .collect();
        let mut tally = RecoveryTally::default();
        for handle in handles {
            tally.add(&handle.join().expect("loss matrix worker"));
        }
        tally
    });
    // Every recovery rule is exercised: going forward, rolling back, the
    // abandonment of an unstarted transaction, and stopping an autostarted
    // service before going forward.
    assert!(tally.committed > 0, "{tally:?}");
    assert!(tally.rolled_back > 0, "{tally:?}");
    assert!(tally.abandoned > 0, "{tally:?}");
    assert!(tally.stopped_then_committed > 0, "{tally:?}");
    eprintln!("{} loss cases recovered: {tally:?}", cases.len());
}

#[test]
fn a_start_job_that_outlives_its_deadline_on_older_systemd_still_rolls_back() {
    // Before systemd 254 a start job for a service that never becomes ready
    // stays queued across every automatic restart. The installer's fence
    // cancels it, a stop replaces the next one, and rollback proceeds.
    for power_loss in [false, true] {
        let (fixture, location) = managed_v1();
        {
            let mut world = fixture.world.borrow_mut();
            world.failing_starts.insert("9.8.8".to_owned());
            world.start_job_persists = true;
        }
        if power_loss {
            // An update's effects are the prior's stop, the manager reload
            // and the candidate's start. Die before that start, then lose
            // power: the boot queues the candidate from the switched pointer.
            let before = fixture.world.borrow().effects.len();
            fixture.world.borrow_mut().crash = Some(Crash::Before(before + 3));
            let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
            assert!(crashed.is_err());
            assert_eq!(
                fixture.journal().next_action,
                Some(InstallAction::RestoreCandidateRuntime)
            );
            fixture.world.borrow_mut().power_cycle();
        }
        let run = fixture
            .update(&fixture.v2)
            .unwrap_or_else(|error| panic!("power_loss={power_loss}: {error}"));
        assert!(
            matches!(
                run.outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "power_loss={power_loss}: {:?}",
            run.outcome
        );
        fixture.assert_managed(&location, &fixture.v1.id);
        fixture.assert_settled_service("persistent start job");
    }
}

#[test]
fn a_changed_service_definition_is_drift_and_never_stops_the_candidate() {
    let (fixture, _) = managed_v1();
    fixture.world.borrow_mut().at_health = Some(("9.8.8", HealthEvent::InstallerDies));
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err());
    let pending = fixture.journal();
    assert_eq!(pending.next_action, Some(InstallAction::ProveCandidate));
    // The candidate still runs, but someone disabled the service: stopping
    // it would not reach any checkpoint, so recovery must not stop it.
    fixture.world.borrow_mut().enabled = false;
    let effects = fixture.world.borrow().effects.len();
    let error = fixture
        .update(&fixture.v2)
        .expect_err("a changed definition is drift");
    assert!(error.to_string().contains("drift"), "{error}");
    let world = fixture.world.borrow();
    assert!(world.active, "the healthy candidate keeps running");
    assert_eq!(world.running_version(), "9.8.8");
    assert_eq!(world.effects.len(), effects, "nothing was stopped");
    drop(world);
    assert_eq!(fixture.journal(), pending, "the journal did not move");
}

/// Arm one loss point on a fixture, relative to its effects and observations
/// so far.
fn arm_loss(fixture: &Fixture, point: LossPoint) {
    let mut world = fixture.world.borrow_mut();
    let (effects, shows) = (world.effects.len(), world.shows);
    match point {
        LossPoint::BeforeEffect(effect) => world.crash = Some(Crash::Before(effects + effect)),
        LossPoint::AfterEffect(effect) => world.crash = Some(Crash::After(effects + effect)),
        LossPoint::AtObservation(show) => world.crash_at_show = Some(shows + show),
    }
}

/// Lose the installer (and power) at `point` during a run of `release`.
fn lose_during(fixture: &Fixture, release: &Release, point: LossPoint) -> bool {
    arm_loss(fixture, point);
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(release))).is_err();
    let mut world = fixture.world.borrow_mut();
    world.crash = None;
    world.crash_at_show = None;
    world.power_cycle();
    crashed
}

#[test]
fn a_second_power_loss_during_recovery_still_recovers_to_exactly_one_release() {
    let mut cases = Vec::new();
    for failing_candidate in [false, true] {
        let (effects, _) = update_extent(failing_candidate);
        for first in (1..=effects).flat_map(|effect| {
            [
                LossPoint::BeforeEffect(effect),
                LossPoint::AfterEffect(effect),
            ]
        }) {
            // Probe how many effects the first recovery performs.
            let (probe, _) = managed_v1();
            if failing_candidate {
                probe
                    .world
                    .borrow_mut()
                    .failing_starts
                    .insert("9.8.8".to_owned());
            }
            assert!(lose_during(&probe, &probe.v2, first));
            let before = probe.world.borrow().effects.len();
            probe.update(&probe.v2).expect("first recovery");
            let recovery_effects = probe.world.borrow().effects.len() - before;
            for second in (1..=recovery_effects).flat_map(|effect| {
                [
                    LossPoint::BeforeEffect(effect),
                    LossPoint::AfterEffect(effect),
                ]
            }) {
                cases.push((first, second, failing_candidate));
            }
        }
    }
    assert!(!cases.is_empty());
    let workers = std::thread::available_parallelism().map_or(4, |count| count.get().min(8));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let cases = &cases;
                scope.spawn(move || {
                    for (first, second, failing_candidate) in
                        cases.iter().skip(worker).step_by(workers)
                    {
                        let context = format!(
                            "first {first:?} second {second:?} failing_candidate={failing_candidate}"
                        );
                        let (fixture, location) = managed_v1();
                        if *failing_candidate {
                            fixture
                                .world
                                .borrow_mut()
                                .failing_starts
                                .insert("9.8.8".to_owned());
                        }
                        assert!(lose_during(&fixture, &fixture.v2, *first), "{context}");
                        assert!(lose_during(&fixture, &fixture.v2, *second), "{context}");
                        let run = fixture
                            .update(&fixture.v2)
                            .unwrap_or_else(|error| panic!("{context}: recovery stopped: {error}"));
                        let running = match run.outcome {
                            InstallOutcome::Committed { .. } => {
                                assert!(!failing_candidate, "{context}");
                                &fixture.v2.id
                            }
                            InstallOutcome::RolledBack { .. } => &fixture.v1.id,
                        };
                        fixture.assert_managed(&location, running);
                        fixture.assert_settled_service(&context);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("double loss worker");
        }
    });
    eprintln!("{} double-loss cases recovered", cases.len());
}

// ── Probation ───────────────────────────────────────────────────────────

impl Fixture {
    /// The release the running service proves to be, as a rollback reports it.
    fn running_release(&self, release: &Release) -> RestoredRelease {
        let world = self.world.borrow();
        let process = world.process.as_ref().expect("a release runs");
        RestoredRelease {
            unit: release.id.clone(),
            version: world.running_version(),
            instance: format!("{:032x}", world.invocation),
            process_id: world.pid,
            executable_sha256: process.sha256.clone(),
        }
    }

    /// The probation windows held for `version`.
    fn watches_for(&self, version: &str) -> Vec<(LinuxServiceIdentity, Duration)> {
        self.world
            .borrow()
            .watches
            .iter()
            .filter(|(watched, _, _)| watched == version)
            .map(|(_, identity, window)| (identity.clone(), *window))
            .collect()
    }
}

#[test]
fn the_raw_installer_holds_every_started_candidate_for_the_default_window() {
    let (fixture, location) = managed_v1();
    let run = fixture.update(&fixture.v2).expect("update");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    fixture.assert_managed(&location, &fixture.v2.id);
    let running = fixture.running_release(&fixture.v2);
    assert_eq!(
        fixture.watches_for("9.8.8"),
        [(
            LinuxServiceIdentity {
                invocation_id: running.instance,
                main_pid: running.process_id,
            },
            Duration::from_secs(90)
        )],
        "one 90 s window over the identity the candidate still runs under"
    );
    assert_eq!(
        fixture.watches_for("9.8.7").len(),
        1,
        "the first install's release was held through its window too"
    );
}

#[test]
fn a_candidate_that_crashes_89_seconds_into_probation_rolls_back_and_names_the_prior() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().in_probation =
        Some(("9.8.8", ProbationEvent::Crashes(Duration::from_secs(89))));
    let run = fixture
        .update(&fixture.v2)
        .expect("the same run settles the crash");
    assert!(!run.recovered);
    let InstallOutcome::RolledBack {
        failure,
        abandoned,
        restored,
        ..
    } = run.outcome
    else {
        panic!("expected a rollback: {:?}", run.outcome);
    };
    assert!(!abandoned);
    assert!(
        failure.contains("probation 89.0 s into its 90 s window"),
        "{failure}"
    );
    fixture.assert_managed(&location, &fixture.v1.id);
    fixture.assert_settled_service("crash 89 s into probation");
    assert_eq!(
        restored,
        Some(fixture.running_release(&fixture.v1)),
        "the rollback names exactly the prior that runs again"
    );
}

#[test]
fn an_installer_lost_during_probation_holds_the_candidate_for_a_whole_window_again() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().in_probation = Some((
        "9.8.8",
        ProbationEvent::InstallerDies(Duration::from_secs(30)),
    ));
    let lost = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(lost.is_err(), "the installer died in the window");
    let pending = fixture.journal();
    assert_eq!(
        (pending.disposition, pending.next_action),
        (
            InstallDisposition::Forward,
            Some(InstallAction::ProveCandidate)
        )
    );
    assert!(pending.candidate_owner_receipt.is_some());
    let effects = fixture.world.borrow().effects.len();

    let run = fixture.update(&fixture.v2).expect("recovery");
    assert!(run.recovered);
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        },
        "an installer loss alone never rolls back a healthy candidate"
    );
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fixture.world.borrow().effects.len(),
        effects,
        "recovery only watched and proved; it changed nothing"
    );
    let watches = fixture.watches_for("9.8.8");
    assert_eq!(watches.len(), 2, "the recovery held a second window");
    assert_eq!(
        watches[0], watches[1],
        "both windows held the same candidate, under the same receipt"
    );
    assert_eq!(
        watches[1].1,
        Duration::from_secs(90),
        "a whole window again"
    );
}

#[test]
fn a_power_cut_during_probation_rolls_back_to_the_prior_and_names_it() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().in_probation =
        Some(("9.8.8", ProbationEvent::PowerCut(Duration::from_secs(20))));
    let lost = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(lost.is_err(), "power was lost in the window");
    assert!(
        fixture.world.borrow().queued_start,
        "the user manager queued the candidate's autostart"
    );

    let run = fixture.update(&fixture.v2).expect("recovery");
    assert!(run.recovered);
    let InstallOutcome::RolledBack {
        abandoned,
        restored,
        ..
    } = run.outcome
    else {
        panic!("expected a rollback: {:?}", run.outcome);
    };
    // The candidate that autostarted runs under a new invocation, not the
    // one its receipt and probation named.
    assert!(!abandoned);
    fixture.assert_managed(&location, &fixture.v1.id);
    fixture.assert_settled_service("power cut in probation");
    assert_eq!(restored, Some(fixture.running_release(&fixture.v1)));
}

// ── Read-only observation ───────────────────────────────────────────────

fn daemon_path(location: &LinuxInstallLocation, unit: &UnitId) -> PathBuf {
    location
        .release_root()
        .join("units")
        .join(unit.as_str())
        .join("bin/hypercolor-daemon")
}

#[test]
fn observation_reads_an_empty_home_then_the_managed_install_it_records() {
    let fixture = Fixture::new();
    assert_eq!(
        observe_linux_installation(&fixture.home).expect("observe"),
        LinuxInstallObservation::Absent
    );

    let (fixture, location) = managed_v1();
    let observation = observe_linux_installation(&fixture.home).expect("observe");
    let LinuxInstallObservation::Managed {
        location: observed,
        records,
    } = &observation
    else {
        panic!("expected a managed installation: {observation:?}");
    };
    assert_eq!(observed, &location);
    assert_eq!(records.release_root, location.release_root());
    assert_eq!(records.active_unit.as_ref(), Some(&fixture.v1.id));
    assert_eq!(
        records.journal.as_ref().map(|journal| journal.disposition),
        Some(InstallDisposition::Committed)
    );
    assert_eq!(records.pending_transaction(), None);
    assert_eq!(
        records.runnable_units(),
        std::slice::from_ref(&fixture.v1.id)
    );
    assert_eq!(
        records.daemon_unit(&daemon_path(&location, &fixture.v1.id)),
        Some(fixture.v1.id.clone())
    );
    // Only a runnable unit's daemon, by its exact resolved path, counts.
    for foreign in [
        daemon_path(&location, &fixture.v2.id),
        location.release_root().join("active/bin/hypercolor-daemon"),
        daemon_path(&location, &fixture.v1.id).with_file_name("hypercolor"),
        PathBuf::from("/usr/bin/hypercolor-daemon"),
    ] {
        assert_eq!(records.daemon_unit(&foreign), None, "{}", foreign.display());
    }
}

#[test]
fn observation_names_both_units_mid_transaction_and_never_takes_the_lock() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().in_probation = Some((
        "9.8.8",
        ProbationEvent::InstallerDies(Duration::from_secs(5)),
    ));
    assert!(catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2))).is_err());

    // An installer holds the installation lock while the daemon observes.
    let held = elect_linux_installation_with(&fixture.home, &fixture.private())
        .expect("an installer holds the lock");
    let state_before = directory_listing(location.state_root());
    let observation = observe_linux_installation(&fixture.home).expect("observe under the lock");
    assert_eq!(
        directory_listing(location.state_root()),
        state_before,
        "observation writes nothing"
    );
    drop(held);

    let records = observation.records().expect("records");
    let pending = records.pending_transaction().expect("in flight");
    assert_eq!(pending.next_action, Some(InstallAction::ProveCandidate));
    assert_eq!(records.active_unit.as_ref(), Some(&fixture.v2.id));
    assert_eq!(
        records.runnable_units(),
        [fixture.v2.id.clone(), fixture.v1.id.clone()],
        "either side of the transaction may be running"
    );
    for unit in [&fixture.v1.id, &fixture.v2.id] {
        assert_eq!(
            records.daemon_unit(&daemon_path(&location, unit)).as_ref(),
            Some(unit)
        );
    }
}

#[test]
fn observation_of_a_historical_install_is_legacy() {
    let fixture = Fixture::new();
    fixture.legacy_install(&fixture.v1);
    let observation = observe_linux_installation(&fixture.home).expect("observe");
    let LinuxInstallObservation::Legacy(records) = observation else {
        panic!("expected a historical install: {observation:?}");
    };
    assert_eq!(
        records.release_root,
        fixture.home.join(".local/lib/hypercolor")
    );
    assert_eq!(records.active_unit, Some(fixture.v1.id.clone()));
}

#[test]
fn observation_refuses_a_managed_locator_without_its_recorded_identity() {
    let (fixture, location) = managed_v1();
    let identity = location.state_root().join("installation.json");
    let saved = fs::read(&identity).expect("identity");
    fs::remove_file(&identity).expect("remove identity");
    assert!(matches!(
        observe_linux_installation(&fixture.home),
        Err(LinuxObservationError::Unprepared(_))
    ));
    fs::write(&identity, saved).expect("restore identity");
}

/// Names, sizes and modification times of a directory's entries.
fn directory_listing(root: &Path) -> Vec<(PathBuf, u64, i64)> {
    let mut entries: Vec<_> = fs::read_dir(root)
        .expect("list")
        .map(|entry| {
            let entry = entry.expect("entry");
            let metadata = entry.metadata().expect("metadata");
            (entry.path(), metadata.len(), metadata.mtime())
        })
        .collect();
    entries.sort();
    entries
}

// ── Release collection ──────────────────────────────────────────────────

/// Every entry in the release root's units directory.
fn unit_entries(location: &LinuxInstallLocation) -> BTreeSet<String> {
    fs::read_dir(location.release_root().join("units"))
        .expect("units directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("UTF-8 name")
        })
        .collect()
}

fn names(units: &[&UnitId]) -> BTreeSet<String> {
    units.iter().map(|unit| unit.as_str().to_owned()).collect()
}

fn collected(run: &hypercolor_cli::install::LinuxInstallRun) -> &UnitCollection {
    run.collection
        .as_ref()
        .expect("a settled managed run collects")
        .as_ref()
        .expect("collection succeeded")
}

#[test]
fn a_settled_upgrade_keeps_only_the_active_release_and_the_one_it_replaced() {
    let (fixture, location) = managed_v1();
    let run = fixture.update(&fixture.v2).expect("second release");
    assert!(collected(&run).removed_units.is_empty());
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v1.id, &fixture.v2.id])
    );

    let run = fixture.update(&fixture.v3).expect("third release");
    assert_eq!(
        collected(&run).removed_units,
        std::slice::from_ref(&fixture.v1.id)
    );
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v2.id, &fixture.v3.id])
    );
    fixture.assert_managed(&location, &fixture.v3.id);
}

#[test]
fn a_rollback_keeps_both_sides_for_the_next_install_and_removes_older_releases() {
    let (fixture, location) = managed_v1();
    fixture.update(&fixture.v2).expect("second release");
    fixture
        .world
        .borrow_mut()
        .failing_starts
        .insert("9.8.9".to_owned());
    let run = fixture
        .update(&fixture.v3)
        .expect("the failing release settles");
    assert!(matches!(run.outcome, InstallOutcome::RolledBack { .. }));
    // The next install proves its prior against this rolled-back record,
    // which binds both of its units by file identity.
    assert_eq!(
        collected(&run).removed_units,
        std::slice::from_ref(&fixture.v1.id)
    );
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v2.id, &fixture.v3.id])
    );
    fixture.assert_managed(&location, &fixture.v2.id);

    fixture.world.borrow_mut().failing_starts.clear();
    let run = fixture.update(&fixture.v3).expect("retry");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v3.id.clone()
        }
    );
    assert!(collected(&run).removed_units.is_empty());
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v2.id, &fixture.v3.id])
    );
}

#[test]
fn collection_finishes_interrupted_staging_and_removal_and_leaves_foreign_entries() {
    let (fixture, location) = managed_v1();
    let units = location.release_root().join("units");
    let staging = units.join(".hypercolor-stage-payload-4242-7");
    fs::create_dir_all(staging.join("bin")).expect("interrupted staging");
    fs::write(staging.join("bin/hypercolor-daemon"), b"partial").expect("staged file");
    fs::set_permissions(staging.join("bin"), fs::Permissions::from_mode(0o555))
        .expect("finalized staging mode");
    let tombstone = units.join(format!(".hypercolor-removing-{}.4242-3", "e".repeat(64)));
    fs::create_dir_all(tombstone.join("share")).expect("interrupted removal");
    fs::write(units.join("notes.txt"), b"not ours").expect("foreign file");
    // A file with a unit's name refuses removal, and collection goes on.
    let odd = units.join("f".repeat(64));
    fs::write(&odd, b"not a unit").expect("file named like a unit");
    fs::create_dir(units.join("keep-me")).expect("foreign directory");
    let journal_stage = location.state_root().join(".install-journal.json.4242.9");
    fs::write(&journal_stage, b"{}").expect("interrupted journal write");

    let run = fixture.update(&fixture.v2).expect("second release");
    let collection = collected(&run);
    for leftover in [&staging, &tombstone, &journal_stage] {
        assert!(
            collection.removed_leftovers.contains(leftover),
            "{} not reported in {:?}",
            leftover.display(),
            collection.removed_leftovers
        );
        assert!(!leftover.exists(), "{} survived", leftover.display());
    }
    assert_eq!(
        collection
            .refused
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>(),
        std::slice::from_ref(&odd)
    );
    assert!(odd.exists());
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v1.id, &fixture.v2.id])
            .into_iter()
            .chain(["f".repeat(64), "keep-me".to_owned(), "notes.txt".to_owned()])
            .collect()
    );
}

#[test]
fn collection_never_removes_a_unit_an_unsettled_transaction_names() {
    let (fixture, location) = managed_v1();
    fixture.update(&fixture.v2).expect("second release");
    fixture.world.borrow_mut().in_probation = Some((
        "9.8.9",
        ProbationEvent::InstallerDies(Duration::from_secs(5)),
    ));
    assert!(catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v3))).is_err());

    let LinuxInstallElection::Managed { store, lock, .. } =
        elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election")
    else {
        panic!("expected managed authority");
    };
    let referenced = store.referenced_units(&lock).expect("referenced units");
    for unit in [&fixture.v2.id, &fixture.v3.id] {
        assert!(referenced.contains(unit), "{} is referenced", unit.as_str());
        assert!(matches!(
            store.remove_unit(&lock, unit),
            Err(InstallStoreError::UnitReferenced(_))
        ));
    }
    let collection = store.collect_units(&lock, &[]).expect("collect");
    assert_eq!(
        collection.removed_units,
        std::slice::from_ref(&fixture.v1.id)
    );
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v2.id, &fixture.v3.id])
    );
    assert!(
        !store
            .remove_unit(&lock, &fixture.v1.id)
            .expect("already gone"),
        "removing a unit that is not installed reports false"
    );
    drop((store, lock));

    let run = fixture.update(&fixture.v3).expect("recovery");
    assert_eq!(
        run.outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v3.id.clone()
        }
    );
    assert_eq!(
        unit_entries(&location),
        names(&[&fixture.v2.id, &fixture.v3.id])
    );
}

// ── Library use ─────────────────────────────────────────────────────────

#[test]
fn a_library_caller_prepares_binds_writes_and_drives_its_own_transaction() {
    // The shape an update activator uses: elect authority, stage the
    // candidate, bind the platform, prepare a journal under its own
    // transaction ID, bind the exact initial journal to its own record,
    // write it, then let the coordinator drive it. No CLI involved.
    let (fixture, location) = managed_v1();
    let LinuxInstallElection::Managed {
        store,
        mut lock,
        authority,
    } = elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election")
    else {
        panic!("expected managed authority");
    };
    let candidate = fixture.v2.stage(&store, &lock);
    // The launcher and update directories exist from the first install;
    // a library host proves them before it binds.
    let launcher = ensure_linux_launcher(&store, &lock, authority.location(), &candidate)
        .expect("the installation's launcher");
    assert!(!launcher.published(), "the first install published it");
    ensure_linux_update_directories(&lock, authority.location()).expect("update directories");
    let world = Rc::clone(&fixture.world);
    let mut platform = bind_linux_platform(
        &fixture.home,
        |_, _, _| {
            Ok(SimExecutor {
                world,
                active_root: None,
            })
        },
        &store,
        &lock,
        LinuxPlatformInputs {
            candidate: Some(&candidate),
            journal: None,
            managed: Some(authority.location()),
            original: None,
            probation: DEFAULT_PROBATION_WINDOW,
        },
    )
    .expect("bind the platform");
    authority.confirm_durable().expect("durable authority");

    let mut coordinator = InstallCoordinator::new(&store, &mut platform);
    let journal = coordinator
        .prepare_with_lock(
            InstallRequest {
                transaction_id: InstallTransactionId::new("update-01JZQ3V8ZK6F2N7QK9X4W5T1AB")
                    .expect("transaction ID"),
                candidate,
                target_policy: InstallTargetPolicy::Preserve,
            },
            &lock,
        )
        .expect("prepare");
    let bound = sha256(&serde_json::to_vec(&journal).expect("encode the initial journal"));
    store.write_journal(&journal, &lock).expect("write");
    let written = store.load_journal(&lock).expect("read").expect("present");
    assert_eq!(
        sha256(&serde_json::to_vec(&written).expect("encode")),
        bound,
        "the written journal is exactly the one the caller bound"
    );
    let outcome = coordinator
        .recover_with_lock(&mut lock)
        .expect("drive")
        .expect("outcome");
    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: fixture.v2.id.clone()
        }
    );
    drop((platform, authority, lock, store));
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fixture.watches_for("9.8.8").len(),
        1,
        "the library path holds the same probation window"
    );
    assert_eq!(
        fixture.journal().transaction_id.as_str(),
        "update-01JZQ3V8ZK6F2N7QK9X4W5T1AB"
    );
}

#[test]
fn a_library_caller_cannot_bind_a_candidate_the_launcher_would_not_run() {
    let (fixture, _location, _) = managed_v1_without_launcher();
    let source = fixture
        .v1
        .source
        .parent()
        .expect("fixture root")
        .join("source-undeclared");
    write_release_with(&source, "9.9.1", b"daemon-undeclared", |manifest| {
        manifest["platform"] = json!("macos-arm64");
        manifest["rust_target"] = json!("aarch64-apple-darwin");
        manifest
            .as_object_mut()
            .expect("object")
            .remove("managed_package");
    });
    let undeclared = Release {
        id: UnitId::new(sha256(
            &fs::read(source.join("manifest.json")).expect("manifest"),
        ))
        .expect("unit ID"),
        source,
    };
    let LinuxInstallElection::Managed {
        store,
        lock,
        authority,
    } = elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election")
    else {
        panic!("expected managed authority");
    };
    let bind = |candidate: &UnitRecord| {
        let world = Rc::clone(&fixture.world);
        bind_linux_platform(
            &fixture.home,
            |_, _, _| {
                Ok(SimExecutor {
                    world,
                    active_root: None,
                })
            },
            &store,
            &lock,
            LinuxPlatformInputs {
                candidate: Some(candidate),
                journal: None,
                managed: Some(authority.location()),
                original: None,
                probation: DEFAULT_PROBATION_WINDOW,
            },
        )
        .map(drop)
        .map_err(|error| error.to_string())
    };

    let error = bind(&undeclared.stage(&store, &lock))
        .expect_err("a release that declares no contract never binds");
    assert!(
        error.contains("must declare its managed_package contract"),
        "{error}"
    );
    let candidate = fixture.v2.stage(&store, &lock);
    let error = bind(&candidate).expect_err("no launcher is published yet");
    assert!(
        error.contains("publish the installation's launcher"),
        "{error}"
    );
    ensure_linux_launcher(&store, &lock, authority.location(), &candidate)
        .expect("publish the launcher");
    bind(&candidate).expect("with its launcher published, the candidate binds");
}

#[test]
fn a_managed_store_never_binds_without_its_recorded_location() {
    let (fixture, _location) = managed_v1();
    let LinuxInstallElection::Managed { store, lock, .. } =
        elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election")
    else {
        panic!("expected managed authority");
    };
    let candidate = fixture.v2.stage(&store, &lock);
    let world = Rc::clone(&fixture.world);
    let error = bind_linux_platform(
        &fixture.home,
        |_, _, _| {
            Ok(SimExecutor {
                world,
                active_root: None,
            })
        },
        &store,
        &lock,
        LinuxPlatformInputs {
            candidate: Some(&candidate),
            journal: None,
            managed: None,
            original: None,
            probation: DEFAULT_PROBATION_WINDOW,
        },
    )
    .map(drop)
    .expect_err("without its location the unit would skip the launcher and sandbox")
    .to_string();
    assert!(error.contains("recorded location"), "{error}");
}

#[test]
fn the_linux_installer_refuses_a_release_without_a_managed_package_whatever_its_label() {
    let (fixture, location) = managed_v1();
    let root = fixture
        .v1
        .source
        .parent()
        .expect("fixture root")
        .to_path_buf();
    let source = root.join("source-labelled-macos");
    write_release_with(&source, "9.9.0", b"daemon-macos", |manifest| {
        manifest["platform"] = json!("macos-arm64");
        manifest["rust_target"] = json!("aarch64-apple-darwin");
        manifest
            .as_object_mut()
            .expect("object")
            .remove("managed_package");
    });
    let release = Release {
        id: UnitId::new(sha256(
            &fs::read(source.join("manifest.json")).expect("manifest"),
        ))
        .expect("unit ID"),
        source,
    };
    let before = fixture.snapshot();
    let error = fixture
        .update(&release)
        .expect_err("a release that declares no managed package never installs")
        .to_string();
    assert!(
        error.contains("must declare its managed_package contract"),
        "{error}"
    );
    assert_eq!(fixture.snapshot(), before, "nothing changed");
    fixture.assert_managed(&location, &fixture.v1.id);

    let adopting = Fixture::new();
    adopting.legacy_install(&adopting.v1);
    let error = adopting
        .run(
            &release,
            Some(adopting.default_location()),
            &adopting.private(),
        )
        .expect_err("adoption refuses it too")
        .to_string();
    assert!(
        error.contains("must declare its managed_package contract"),
        "{error}"
    );
}

#[test]
fn an_install_from_before_the_contract_adopts_and_rolls_back_exactly() {
    for failing in [false, true] {
        let fixture = Fixture::new();
        let original = fixture.legacy_install_before_contract(&fixture.v1);
        assert_eq!(
            hypercolor_cli::install::read_declared_compatibility(&original).expect("read"),
            hypercolor_cli::install::DeclaredCompatibility::Undeclared,
            "the historical release declares nothing"
        );
        let snapshot = fixture.snapshot();
        if failing {
            fixture.world.borrow_mut().fault = Some("runtime:true".to_owned());
        }
        let location = fixture.default_location();
        let run = fixture
            .run(&fixture.v2, Some(location.clone()), &fixture.private())
            .expect("the adoption settles");
        if failing {
            assert!(matches!(run.outcome, InstallOutcome::RolledBack { .. }));
            assert_eq!(
                fixture.snapshot(),
                snapshot,
                "the release from before the contract runs again, exactly"
            );
        } else {
            assert_eq!(
                run.outcome,
                InstallOutcome::Committed {
                    active_unit: fixture.v2.id.clone()
                }
            );
            fixture.assert_managed(&location, &fixture.v2.id);
            let run = fixture.update(&fixture.v3).expect("a later update");
            assert_eq!(
                run.outcome,
                InstallOutcome::Committed {
                    active_unit: fixture.v3.id.clone()
                }
            );
        }
    }
}

// ── Launcher and sandbox ────────────────────────────────────────────────

fn launcher_path(location: &LinuxInstallLocation) -> PathBuf {
    location.release_root().join("launcher/hypercolor")
}

fn plan(
    fixture: &Fixture,
    location: &LinuxInstallLocation,
    role: LinuxLaunchRole,
) -> Result<LinuxLaunchPlan, LinuxLaunchError> {
    plan_linux_launch(&LinuxLaunchRequest {
        home: &fixture.home,
        role,
        arguments: Vec::new(),
        launcher: &launcher_path(location),
    })
}

fn release_cli(release: &Release) -> Vec<u8> {
    fs::read(release.source.join("bin/hypercolor")).expect("release CLI")
}

/// The generated unit a managed installation at `location` runs under.
fn sandboxed_unit(location: &LinuxInstallLocation) -> String {
    let path = |path: &Path| path.to_str().expect("UTF-8").to_owned();
    format!(
        "[Unit]\nDescription=Hypercolor RGB Lighting Daemon\nAfter=graphical-session.target dbus.socket\nWants=graphical-session.target\n\n[Service]\nType=notify\nExecStartPre=+{launcher} __launch --role prepare-roots\nExecStart={launcher} __launch --role daemon\nWatchdogSec=30\nRestart=on-failure\nRestartSec=3\nEnvironment=HYPERCOLOR_LOG=info\nEnvironment=RUST_BACKTRACE=1\nEnvironment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\nProtectSystem=strict\nProtectHome=read-only\nPrivateTmp=true\nNoNewPrivileges=true\nReadWritePaths={config} {data} {daemon_state} -{state}/coordinator\nReadOnlyPaths={releases} {state}\n\n[Install]\nWantedBy=default.target\n",
        launcher = path(&launcher_path(location)),
        config = path(location.config_root()),
        data = path(location.data_root()),
        daemon_state = path(location.state_root().parent().expect("state parent")),
        state = path(location.state_root()),
        releases = path(location.release_root()),
    )
}

fn directives<'a>(unit: &'a str, key: &str) -> Vec<&'a str> {
    unit.lines()
        .filter_map(|line| line.strip_prefix(key))
        .flat_map(str::split_ascii_whitespace)
        .collect()
}

#[test]
fn a_managed_install_runs_its_daemon_through_the_launcher_inside_the_recorded_sandbox() {
    for (data, state, config) in [
        (".local/share", ".local/state", ".config"),
        ("xdg/data", "xdg/state", "xdg/config"),
    ] {
        let fixture = Fixture::new();
        let location = fixture.location(data, state, config);
        fixture
            .run(&fixture.v1, Some(location.clone()), &fixture.private())
            .expect("managed install");
        fixture.assert_managed(&location, &fixture.v1.id);
        let unit = String::from_utf8(fixture.world.borrow().launcher_bytes.clone()).expect("UTF-8");
        assert_eq!(unit, sandboxed_unit(&location));

        // Writable: exactly the recorded configuration, data and daemon
        // state roots, and the coordinator's directory. Nothing under the
        // command links, libraries, releases or the rest of update state.
        let writable = directives(&unit, "ReadWritePaths=");
        let read_only = directives(&unit, "ReadOnlyPaths=");
        let state_root = location.state_root().to_str().expect("UTF-8");
        let release_root = location.release_root().to_str().expect("UTF-8");
        for path in &writable {
            let path = Path::new(path.trim_start_matches('-'));
            for forbidden in [".local/bin", ".local/lib"] {
                assert!(
                    !path.starts_with(fixture.home.join(forbidden))
                        && !fixture.home.join(forbidden).starts_with(path),
                    "{} would make {forbidden} writable",
                    path.display()
                );
            }
            assert!(!path.starts_with(release_root), "{}", path.display());
            assert!(
                !path.starts_with(state_root) || path.ends_with("coordinator"),
                "{}",
                path.display()
            );
        }
        assert_eq!(read_only, [release_root, state_root]);
        for nested in [release_root, state_root] {
            assert!(
                writable
                    .iter()
                    .any(|path| Path::new(nested).starts_with(path.trim_start_matches('-'))),
                "{nested} is read-only beneath a writable root, so the nesting matters"
            );
        }

        // Update state directories exist, private, before the daemon runs.
        for name in ["coordinator", "activator"] {
            assert_eq!(mode(&location.state_root().join(name)), 0o700, "{name}");
        }
    }
}

#[test]
fn the_first_install_publishes_the_launcher_and_no_update_rewrites_it() {
    let (fixture, location) = managed_v1();
    let launcher = launcher_path(&location);
    assert_eq!(
        fs::read(&launcher).expect("launcher"),
        release_cli(&fixture.v1)
    );
    assert_eq!(mode(&launcher), 0o555);
    assert_eq!(mode(launcher.parent().expect("launcher directory")), 0o555);
    let contract: serde_json::Value = serde_json::from_slice(
        &fs::read(location.release_root().join("launcher/contract.json")).expect("contract"),
    )
    .expect("contract JSON");
    assert_eq!(contract["launcher_contract"], 1);
    assert_eq!(contract["source_unit"], fixture.v1.id.as_str());
    assert_eq!(
        contract["program_sha256"],
        sha256(&release_cli(&fixture.v1))
    );
    let inode = fs::metadata(&launcher).expect("launcher").ino();

    for release in [&fixture.v2, &fixture.v3] {
        let run = fixture.update(release).expect("update");
        assert_eq!(
            run.outcome,
            InstallOutcome::Committed {
                active_unit: release.id.clone()
            }
        );
        fixture.assert_managed(&location, &release.id);
        assert_eq!(
            fs::read(&launcher).expect("launcher"),
            release_cli(&fixture.v1),
            "an update never rewrites the launcher"
        );
        assert_eq!(fs::metadata(&launcher).expect("launcher").ino(), inode);
    }
    assert_ne!(release_cli(&fixture.v1), release_cli(&fixture.v3));
}

#[test]
fn a_changed_launcher_refuses_the_next_install_before_any_service_change() {
    let (fixture, location) = managed_v1();
    let launcher = launcher_path(&location);
    let directory = launcher.parent().expect("launcher directory").to_path_buf();
    for tamper in ["bytes", "mode", "contract"] {
        let original = fs::read(&launcher).expect("launcher");
        let contract = fs::read(directory.join("contract.json")).expect("contract");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("thaw");
        match tamper {
            "bytes" => {
                fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).expect("thaw");
                fs::write(&launcher, b"not the launcher").expect("tamper bytes");
                fs::set_permissions(&launcher, fs::Permissions::from_mode(0o555)).expect("mode");
            }
            "mode" => {
                fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).expect("mode");
            }
            _ => {
                fs::set_permissions(
                    directory.join("contract.json"),
                    fs::Permissions::from_mode(0o644),
                )
                .expect("thaw");
                let mut record: serde_json::Value =
                    serde_json::from_slice(&contract).expect("contract JSON");
                record["launcher_contract"] = json!(2);
                fs::write(
                    directory.join("contract.json"),
                    serde_json::to_vec(&record).expect("encode"),
                )
                .expect("tamper contract");
                fs::set_permissions(
                    directory.join("contract.json"),
                    fs::Permissions::from_mode(0o444),
                )
                .expect("freeze");
            }
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).expect("freeze");
        let before = fixture.snapshot();
        let effects = fixture.world.borrow().effects.len();
        let error = fixture
            .update(&fixture.v2)
            .expect_err("a changed launcher refuses the install")
            .to_string();
        assert!(error.contains("launcher"), "{tamper}: {error}");
        assert_eq!(fixture.snapshot(), before, "{tamper}: nothing changed");
        assert_eq!(fixture.world.borrow().effects.len(), effects, "{tamper}");

        // Restore the published launcher exactly, and the install proceeds.
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("thaw");
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).expect("thaw");
        fs::write(&launcher, &original).expect("restore bytes");
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o555)).expect("mode");
        fs::set_permissions(
            directory.join("contract.json"),
            fs::Permissions::from_mode(0o644),
        )
        .expect("thaw");
        fs::write(directory.join("contract.json"), &contract).expect("restore contract");
        fs::set_permissions(
            directory.join("contract.json"),
            fs::Permissions::from_mode(0o444),
        )
        .expect("freeze");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).expect("freeze");
    }
    fixture
        .update(&fixture.v2)
        .expect("the restored launcher proves");
    fixture.assert_managed(&location, &fixture.v2.id);
}

#[test]
fn a_launcher_lost_to_the_user_is_published_again_and_stale_stages_go() {
    let (fixture, location) = managed_v1();
    let directory = location.release_root().join("launcher");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("thaw");
    fs::remove_dir_all(&directory).expect("user removes the launcher");
    let stale = location
        .release_root()
        .join(".hypercolor-stage-launcher-1-0");
    fs::create_dir(&stale).expect("stale stage");
    fs::write(stale.join("hypercolor"), b"half").expect("stale program");
    // A crash after sealing a stage but before publishing it leaves a
    // read-only stage behind.
    let sealed = location
        .release_root()
        .join(".hypercolor-stage-launcher-2-0");
    fs::create_dir(&sealed).expect("sealed stage");
    fs::write(sealed.join("hypercolor"), b"whole").expect("sealed program");
    fs::set_permissions(sealed.join("hypercolor"), fs::Permissions::from_mode(0o555))
        .expect("seal program");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o555)).expect("seal stage");
    fixture.update(&fixture.v2).expect("update republishes");
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        release_cli(&fixture.v2)
    );
    assert!(!stale.exists(), "the stale stage is removed");
    assert!(!sealed.exists(), "the sealed stage is removed");
}

#[test]
fn an_install_lost_right_after_publishing_the_launcher_resumes_cleanly() {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    let mut host = Host::new(&fixture.world, &fixture.v1, Some(location.clone()));
    host.stop_at = Some(LinuxInstallCheckpoint::LauncherReady);
    run_linux_install(
        &fixture.home,
        &fixture
            .v1
            .request(InstallTargetPolicy::EnableOnFirstInstall),
        &fixture.private(),
        &mut host,
    )
    .expect_err("the run stops once the launcher is ready");
    assert!(launcher_path(&location).exists());
    assert!(
        fixture.world.borrow().effects.is_empty(),
        "no service change yet"
    );
    fixture
        .run(&fixture.v1, Some(location.clone()), &fixture.private())
        .expect("the rerun installs");
    fixture.assert_managed(&location, &fixture.v1.id);
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        release_cli(&fixture.v1)
    );
}

#[test]
fn the_daemon_role_names_one_release_for_executable_ui_and_effects() {
    let (fixture, location) = managed_v1();
    let daemon = plan(&fixture, &location, LinuxLaunchRole::Daemon).expect("daemon plan");
    let release = location
        .release_root()
        .join("units")
        .join(fixture.v1.id.as_str());
    assert_eq!(daemon.unit, fixture.v1.id);
    assert_eq!(daemon.selection, LinuxLaunchSelection::Active);
    assert_eq!(daemon.program, release.join("bin/hypercolor-daemon"));
    assert_eq!(
        daemon.arguments,
        [
            PathBuf::from("--ui-dir"),
            release.join("share/hypercolor/ui"),
            PathBuf::from("--effects-dir"),
            release.join("share/hypercolor/effects/bundled"),
        ]
        .map(std::ffi::OsString::from)
        .to_vec()
    );
    let state_base = location
        .state_root()
        .parent()
        .and_then(Path::parent)
        .expect("state base");
    assert_eq!(
        daemon.environment,
        [
            ("XDG_CONFIG_HOME", fixture.home.join(".config")),
            ("XDG_DATA_HOME", fixture.home.join(".local/share")),
            ("XDG_STATE_HOME", state_base.to_path_buf()),
            ("XDG_CACHE_HOME", state_base.join("hypercolor/cache")),
        ]
        .map(|(name, value)| (name.into(), value.into_os_string()))
        .to_vec(),
        "the daemon resolves exactly the recorded roots, and caches inside them"
    );
    let cli = plan(&fixture, &location, LinuxLaunchRole::Cli).expect("CLI plan");
    assert_eq!(cli.program, release.join("bin/hypercolor"));
    assert!(cli.environment.is_empty());
}

#[test]
fn the_launcher_resolves_active_once_while_it_swaps_underneath() {
    let (fixture, location) = managed_v1();
    fixture.update(&fixture.v2).expect("second release");
    let releases = location.release_root().to_path_buf();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        let releases = releases.clone();
        let units = [fixture.v1.id.clone(), fixture.v2.id.clone()];
        std::thread::spawn(move || {
            let mut swaps = 0_usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let target = Path::new("units").join(units[swaps % 2].as_str());
                let staged = releases.join(".active-swap");
                let _ = fs::remove_file(&staged);
                std::os::unix::fs::symlink(&target, &staged).expect("stage pointer");
                fs::rename(&staged, releases.join("active")).expect("swap pointer");
                swaps += 1;
            }
            swaps
        })
    };
    let mut selected = BTreeSet::new();
    for _ in 0..2000 {
        let plan = plan(&fixture, &location, LinuxLaunchRole::Daemon).expect("a plan");
        let release = releases.join("units").join(plan.unit.as_str());
        assert_eq!(plan.program, release.join("bin/hypercolor-daemon"));
        assert_eq!(
            plan.arguments[1],
            release.join("share/hypercolor/ui").into_os_string(),
            "the UI comes from the release whose daemon runs"
        );
        assert_eq!(
            plan.arguments[3],
            release
                .join("share/hypercolor/effects/bundled")
                .into_os_string(),
            "the effects come from the release whose daemon runs"
        );
        for argument in &plan.arguments {
            assert!(
                !Path::new(argument).starts_with(releases.join("active")),
                "no path traverses the pointer after selection"
            );
        }
        selected.insert(plan.unit.as_str().to_owned());
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let swaps = swapper.join().expect("swapper");
    assert!(swaps > 100, "the pointer really swapped ({swaps} times)");
    assert_eq!(
        selected.len(),
        2,
        "both releases were selected under the swaps"
    );
    let _ = fs::remove_file(releases.join("active"));
    std::os::unix::fs::symlink(
        Path::new("units").join(fixture.v2.id.as_str()),
        releases.join("active"),
    )
    .expect("restore the pointer");
}

#[test]
fn the_update_executor_runs_the_prior_while_an_install_is_unsettled() {
    let (fixture, location) = managed_v1();
    fixture.world.borrow_mut().crash_after_effect = Some("runtime:true".to_owned());
    let crashed = catch_unwind(AssertUnwindSafe(|| fixture.update(&fixture.v2)));
    assert!(crashed.is_err(), "the installer is lost after starting v2");
    let pointer = fs::read_link(location.release_root().join("active")).expect("active");
    assert_eq!(pointer, Path::new("units").join(fixture.v2.id.as_str()));
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(location.state_root().join("install-journal.json")).expect("journal"),
    )
    .expect("journal JSON");
    assert_eq!(journal["disposition"], "forward");
    assert_eq!(journal["prior_active_unit"], fixture.v1.id.as_str());

    let executor = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor).expect("executor");
    assert_eq!(
        executor.unit, fixture.v1.id,
        "recovery runs the prior's code"
    );
    assert_eq!(
        executor.selection,
        LinuxLaunchSelection::PendingTransactionPrior
    );
    assert_eq!(
        executor.program,
        location
            .release_root()
            .join("units")
            .join(fixture.v1.id.as_str())
            .join("bin/hypercolor")
    );
    let cli = plan(&fixture, &location, LinuxLaunchRole::Cli).expect("CLI");
    assert_eq!(
        cli.unit, fixture.v2.id,
        "ordinary commands run the active release"
    );

    // What the recovery unit runs: settle, staging and proposing nothing.
    let mut host = Host::new(&fixture.world, &fixture.v3, None);
    let run = run_linux_recovery(
        &fixture.home,
        DEFAULT_PROBATION_WINDOW,
        &fixture.private(),
        &mut host,
    )
    .expect("recovery settles")
    .expect("a transaction was pending");
    assert!(run.recovered);
    assert_eq!(host.proposals, 0);
    assert!(
        !host.seen.contains(&LinuxInstallCheckpoint::CandidateStaged),
        "recovery stages nothing"
    );
    assert!(
        !location
            .release_root()
            .join("units")
            .join(fixture.v3.id.as_str())
            .exists(),
        "no release but the transaction's own is touched"
    );
    fixture.assert_settled_service("after recovery");
    let mut host = Host::new(&fixture.world, &fixture.v3, None);
    assert!(
        run_linux_recovery(
            &fixture.home,
            DEFAULT_PROBATION_WINDOW,
            &fixture.private(),
            &mut host,
        )
        .expect("nothing to recover")
        .is_none(),
        "a settled install has nothing to recover"
    );
    let executor = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor).expect("executor");
    assert_eq!(executor.selection, LinuxLaunchSelection::Active);
    let active = fs::read_link(location.release_root().join("active")).expect("active");
    assert_eq!(
        Path::new("units").join(executor.unit.as_str()),
        active,
        "once settled, the executor is the active release"
    );
}

#[test]
fn the_daemon_role_never_reads_the_journal() {
    let (fixture, location) = managed_v1();
    let path = location.state_root().join("install-journal.json");
    let original = fs::read(&path).expect("journal");
    fs::write(&path, br#"{"disposition":"paused","schema_version":99}"#).expect("future journal");
    let error = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor)
        .expect_err("an unknown disposition stops recovery from guessing");
    assert!(matches!(error, LinuxLaunchError::Journal(_)), "{error}");
    plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect("lighting never depends on the journal format");
    fs::write(&path, original).expect("restore journal");
}

#[test]
fn the_update_executor_refuses_a_prior_it_cannot_prove_rather_than_run_the_candidate() {
    let (fixture, location) = managed_v1();
    fixture
        .update(&fixture.v2)
        .expect("v2 is active, v1 retained");
    let path = location.state_root().join("install-journal.json");
    let original = fs::read(&path).expect("journal");
    let pending = |prior: serde_json::Value| {
        let mut journal: serde_json::Value =
            serde_json::from_slice(&original).expect("journal JSON");
        journal["disposition"] = json!("forward");
        journal["prior_active_unit"] = prior;
        fs::write(&path, serde_json::to_vec(&journal).expect("bytes")).expect("pending journal");
    };

    pending(json!(fixture.v1.id.as_str()));
    let prior = location
        .release_root()
        .join("units")
        .join(fixture.v1.id.as_str());
    fs::set_permissions(&prior, fs::Permissions::from_mode(0o755)).expect("drift");
    let error = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor)
        .expect_err("a prior that fails its proof stops recovery");
    assert!(
        matches!(&error, LinuxLaunchError::Release { unit, .. } if unit == fixture.v1.id.as_str()),
        "{error}"
    );
    fs::set_permissions(&prior, fs::Permissions::from_mode(0o555)).expect("restore");

    for (prior, why) in [
        (json!(null), "a first install has no prior"),
        (
            json!(format!("legacy-{}", "c".repeat(64))),
            "an adoption's prior is the historical root",
        ),
    ] {
        pending(prior);
        let executor = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor).expect(why);
        assert_eq!(
            (executor.unit, executor.selection),
            (
                fixture.v2.id.clone(),
                LinuxLaunchSelection::ActiveWithoutRunnablePrior
            ),
            "{why}"
        );
    }
    fs::write(&path, original).expect("restore journal");
}

#[test]
fn the_service_recreates_recorded_roots_a_user_deleted_before_its_sandbox() {
    let (fixture, location) = managed_v1();
    let prepare = |arguments: Vec<std::ffi::OsString>, launcher: &Path| {
        prepare_linux_launch_roots(&LinuxLaunchRequest {
            home: &fixture.home,
            role: LinuxLaunchRole::PrepareRoots,
            arguments,
            launcher,
        })
    };
    let launcher = launcher_path(&location);
    assert_eq!(
        prepare(Vec::new(), &launcher).expect("nothing missing"),
        Vec::<PathBuf>::new()
    );

    fs::remove_dir_all(location.config_root()).expect("the user resets the configuration");
    assert_eq!(
        prepare(Vec::new(), &launcher).expect("prepare"),
        vec![location.config_root().to_path_buf()]
    );
    let metadata = fs::metadata(location.config_root()).expect("recreated");
    assert!(metadata.is_dir());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);

    fs::remove_dir_all(location.config_root()).expect("reset again");
    fs::write(location.config_root(), b"not a directory").expect("a file in its place");
    assert_eq!(
        prepare(Vec::new(), &launcher).expect("an existing entry is left alone"),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        fs::read(location.config_root()).expect("left alone"),
        b"not a directory"
    );
    fs::remove_file(location.config_root()).expect("remove file");
    fs::create_dir(location.config_root()).expect("restore");

    assert!(matches!(
        prepare(vec!["--force".into()], &launcher),
        Err(LinuxLaunchError::Arguments(_))
    ));
    assert!(matches!(
        prepare(Vec::new(), &fixture.home.join("elsewhere/hypercolor")),
        Err(LinuxLaunchError::ForeignLauncher { .. })
    ));
    assert!(matches!(
        plan(&fixture, &location, LinuxLaunchRole::PrepareRoots),
        Err(LinuxLaunchError::Arguments(_))
    ));
}

#[test]
fn an_update_directory_whose_mode_drifted_gets_it_back() {
    let (fixture, location) = managed_v1();
    let coordinator = location.state_root().join("coordinator");
    fs::set_permissions(&coordinator, fs::Permissions::from_mode(0o755))
        .expect("the daemon widens its directory");
    fixture
        .update(&fixture.v2)
        .expect("the next install proceeds");
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fs::metadata(&coordinator)
            .expect("coordinator")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
}

#[test]
fn the_update_executor_runs_only_a_release_that_declares_its_launcher_contract() {
    let (fixture, location) = managed_v1();
    let undeclared = plant_unit(
        &location,
        &[
            ("bin/hypercolor-daemon", b"daemon"),
            ("bin/hypercolor", b"cli"),
            ("share/hypercolor/ui/index.html", b"ui"),
            ("share/hypercolor/effects/bundled/effect.html", b"fx"),
        ],
        0o555,
    );
    let path = location.state_root().join("install-journal.json");
    let original = fs::read(&path).expect("journal");
    let mut journal: serde_json::Value = serde_json::from_slice(&original).expect("journal JSON");
    journal["disposition"] = json!("forward");
    journal["prior_active_unit"] = json!(undeclared.as_str());
    fs::write(&path, serde_json::to_vec(&journal).expect("bytes")).expect("pending journal");

    let executor = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor).expect("executor");
    assert_eq!(
        (executor.unit, executor.selection),
        (
            fixture.v1.id.clone(),
            LinuxLaunchSelection::ActiveWithoutRunnablePrior
        ),
        "a prior whose CLI has no executor commands is never run as the executor"
    );

    point_active(&location, &format!("units/{}", undeclared.as_str()));
    let error = plan(&fixture, &location, LinuxLaunchRole::UpdateExecutor)
        .expect_err("neither is an active release without the contract");
    assert!(error.to_string().contains("launcher contract"), "{error}");
    assert_eq!(
        plan(&fixture, &location, LinuxLaunchRole::Cli)
            .expect("ordinary commands need no declaration")
            .unit,
        undeclared
    );
    plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect("lighting never depends on the declaration");

    point_active(&location, &format!("units/{}", fixture.v1.id.as_str()));
    fs::write(&path, original).expect("restore journal");
    let root = location
        .release_root()
        .join("units")
        .join(undeclared.as_str());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("thaw");
    fs::remove_dir_all(&root).expect("remove planted unit");
}

/// Plant a directory beneath `units/` named for its own manifest digest.
fn plant_unit(location: &LinuxInstallLocation, files: &[(&str, &[u8])], unit_mode: u32) -> UnitId {
    let manifest = format!("{{\"planted\":{}}}", files.len()).into_bytes();
    let id = UnitId::new(sha256(&manifest)).expect("digest");
    let root = location.release_root().join("units").join(id.as_str());
    fs::set_permissions(
        root.parent().expect("units"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("units writable");
    fs::create_dir(&root).expect("planted unit");
    fs::write(root.join("manifest.json"), &manifest).expect("manifest");
    for (path, bytes) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("directories");
        fs::write(&file, bytes).expect("file");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o555)).expect("mode");
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(unit_mode)).expect("unit mode");
    id
}

fn point_active(location: &LinuxInstallLocation, target: &str) {
    let active = location.release_root().join("active");
    fs::remove_file(&active).expect("remove pointer");
    std::os::unix::fs::symlink(target, &active).expect("pointer");
}

#[test]
fn the_launcher_refuses_what_it_cannot_prove() {
    let (fixture, location) = managed_v1();
    let good = format!("units/{}", fixture.v1.id.as_str());

    let foreign = plan_linux_launch(&LinuxLaunchRequest {
        home: &fixture.home,
        role: LinuxLaunchRole::Daemon,
        arguments: Vec::new(),
        launcher: &fixture.home.join("elsewhere/hypercolor"),
    })
    .expect_err("another copy of the CLI is not this installation's launcher");
    assert!(matches!(foreign, LinuxLaunchError::ForeignLauncher { .. }));

    let arguments = plan_linux_launch(&LinuxLaunchRequest {
        home: &fixture.home,
        role: LinuxLaunchRole::Daemon,
        arguments: vec!["--ui-dir".into(), "/tmp".into()],
        launcher: &launcher_path(&location),
    })
    .expect_err("the daemon's paths are the release's, never the caller's");
    assert!(matches!(arguments, LinuxLaunchError::Arguments(_)));

    let incomplete = plant_unit(&location, &[("bin/hypercolor-daemon", b"daemon")], 0o555);
    point_active(&location, &format!("units/{}", incomplete.as_str()));
    let error = plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect_err("a release without its UI and effects never launches");
    assert!(error.to_string().contains("share/hypercolor/ui"), "{error}");
    let error = plan(&fixture, &location, LinuxLaunchRole::Cli)
        .expect_err("a release without its CLI never launches");
    assert!(error.to_string().contains("bin/hypercolor"), "{error}");

    let writable = plant_unit(
        &location,
        &[
            ("bin/hypercolor-daemon", b"daemon"),
            ("bin/hypercolor", b"cli"),
            ("share/hypercolor/ui/index.html", b"ui"),
            ("share/hypercolor/effects/bundled/effect.html", b"fx"),
        ],
        0o755,
    );
    point_active(&location, &format!("units/{}", writable.as_str()));
    let error = plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect_err("a writable release is not immutable");
    assert!(error.to_string().contains("read-only"), "{error}");

    let misnamed = location.release_root().join("units").join("a".repeat(64));
    fs::create_dir(&misnamed).expect("misnamed unit");
    fs::write(misnamed.join("manifest.json"), b"{}").expect("manifest");
    fs::set_permissions(&misnamed, fs::Permissions::from_mode(0o555)).expect("mode");
    point_active(&location, &format!("units/{}", "a".repeat(64)));
    let error = plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect_err("a directory whose manifest is another release's never launches");
    assert!(error.to_string().contains("named for"), "{error}");

    point_active(&location, &format!("units/legacy-{}", "b".repeat(64)));
    let error = plan(&fixture, &location, LinuxLaunchRole::Daemon)
        .expect_err("a legacy snapshot is not launchable");
    assert!(error.to_string().contains("legacy"), "{error}");

    point_active(&location, "../elsewhere");
    assert!(matches!(
        plan(&fixture, &location, LinuxLaunchRole::Daemon),
        Err(LinuxLaunchError::ActivePointer(_))
    ));

    point_active(&location, &good);
    plan(&fixture, &location, LinuxLaunchRole::Daemon).expect("the real release launches");
    for unit in [&incomplete, &writable] {
        let root = location.release_root().join("units").join(unit.as_str());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("thaw");
        fs::remove_dir_all(&root).expect("remove planted unit");
    }
    fs::set_permissions(&misnamed, fs::Permissions::from_mode(0o755)).expect("thaw");
    fs::remove_dir_all(&misnamed).expect("remove misnamed unit");
}

#[test]
fn only_a_managed_installation_launches() {
    let fixture = Fixture::new();
    let location = fixture.default_location();
    assert!(matches!(
        plan(&fixture, &location, LinuxLaunchRole::Daemon),
        Err(LinuxLaunchError::NotManaged)
    ));
    fixture.legacy_install(&fixture.v1);
    assert!(matches!(
        plan(&fixture, &location, LinuxLaunchRole::Daemon),
        Err(LinuxLaunchError::NotManaged)
    ));
}

#[test]
fn a_daemon_started_with_another_releases_assets_fails_its_proof_and_rolls_back() {
    let (fixture, location) = managed_v1();
    let prior = location
        .release_root()
        .join("units")
        .join(fixture.v1.id.as_str());
    let candidate = location
        .release_root()
        .join("units")
        .join(fixture.v2.id.as_str());
    let mixed = [
        candidate.join("bin/hypercolor-daemon"),
        PathBuf::from("--ui-dir"),
        prior.join("share/hypercolor/ui"),
        PathBuf::from("--effects-dir"),
        candidate.join("share/hypercolor/effects/bundled"),
    ]
    .map(|argument| argument.to_str().expect("UTF-8").to_owned())
    .to_vec();
    fixture.world.borrow_mut().argument_override = Some(("9.8.8".to_owned(), mixed));
    let run = fixture.update(&fixture.v2).expect("the run settles");
    let InstallOutcome::RolledBack { failure, .. } = run.outcome else {
        panic!("a mixed daemon must not commit: {:?}", run.outcome);
    };
    assert!(failure.contains("runs with arguments"), "{failure}");
    fixture.assert_managed(&location, &fixture.v1.id);
}

/// A managed installation from before the launcher: its unit runs the
/// daemon straight through `active`, and it has no launcher directory.
fn managed_v1_without_launcher() -> (Fixture, LinuxInstallLocation, Vec<u8>) {
    let (fixture, location) = managed_v1();
    let directory = location.release_root().join("launcher");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("thaw");
    fs::remove_dir_all(&directory).expect("remove launcher");
    let active = location.release_root().join("active");
    let active = active.to_str().expect("UTF-8");
    let direct = format!(
        "[Unit]\nDescription=Hypercolor RGB Lighting Daemon\nAfter=graphical-session.target dbus.socket\nWants=graphical-session.target\n\n[Service]\nType=notify\nExecStart={active}/bin/hypercolor-daemon --ui-dir {active}/share/hypercolor/ui --effects-dir {active}/share/hypercolor/effects/bundled\nWatchdogSec=30\nRestart=on-failure\nRestartSec=3\nEnvironment=HYPERCOLOR_LOG=info\nEnvironment=RUST_BACKTRACE=1\nEnvironment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\n\n[Install]\nWantedBy=default.target\n"
    )
    .into_bytes();
    {
        let mut world = fixture.world.borrow_mut();
        world.launcher_bytes.clone_from(&direct);
        world.launcher = LinuxExactEntry::RegularFile {
            mode: 0o644,
            sha256: sha256(&direct),
            snapshot_unit: None,
            snapshot_path: None,
        };
        world.exec_start = launcher_exec(&direct);
        world.restart_service();
    }
    (fixture, location, direct)
}

#[test]
fn an_install_from_before_the_launcher_gains_it_and_its_rollback_restores_the_direct_unit() {
    let (fixture, location, _) = managed_v1_without_launcher();
    fixture.update(&fixture.v2).expect("update");
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        release_cli(&fixture.v2)
    );

    let (fixture, location, direct) = managed_v1_without_launcher();
    fixture
        .world
        .borrow_mut()
        .failing_starts
        .insert("9.8.8".to_owned());
    let run = fixture.update(&fixture.v2).expect("the run settles");
    assert!(matches!(run.outcome, InstallOutcome::RolledBack { .. }));
    let world = fixture.world.borrow();
    assert_eq!(world.launcher_bytes, direct, "the direct unit comes back");
    assert_eq!(world.running_version(), "9.8.7");
    assert_eq!(
        world.process.as_ref().expect("prior runs").arguments[0],
        format!(
            "{}/bin/hypercolor-daemon",
            location.release_root().join("active").display()
        ),
        "the restored prior runs with its own direct arguments"
    );
    assert!(
        launcher_path(&location).exists(),
        "the launcher stays until the next install"
    );
    drop(world);
    // No settled service ever started through v2's launcher, so the next
    // install replaces it with its own candidate's CLI; once that install
    // settles through it, it is the installation's for good.
    fixture.update(&fixture.v3).expect("the next install");
    fixture.assert_managed(&location, &fixture.v3.id);
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        release_cli(&fixture.v3),
        "a launcher from a rolled-back candidate is replaced"
    );
    fixture.update(&fixture.v2).expect("a later install");
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        release_cli(&fixture.v3),
        "a settled launcher is never replaced"
    );
}

#[test]
fn a_launcher_settles_at_its_first_commit_and_stays_settled() {
    // An upgrade while the service is stopped still commits the unit that
    // starts the launcher, so the launcher it published is settled.
    let (fixture, location, _) = managed_v1_without_launcher();
    fixture.world.borrow_mut().stop();
    let run = fixture.update(&fixture.v2).expect("upgrade while stopped");
    assert!(matches!(run.outcome, InstallOutcome::Committed { .. }));
    assert!(
        !fixture.world.borrow().active,
        "the stopped service stays stopped"
    );
    let settled = fs::read(launcher_path(&location)).expect("launcher");
    assert_eq!(settled, release_cli(&fixture.v2));
    fixture
        .update(&fixture.v3)
        .expect("another upgrade while stopped");
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        settled,
        "a committed launcher is never replaced"
    );

    // Running, stopping, and upgrading again never unsettle it.
    fixture.world.borrow_mut().start();
    fixture.update(&fixture.v2).expect("upgrade while running");
    fixture.world.borrow_mut().stop();
    fixture
        .update(&fixture.v3)
        .expect("upgrade after the user stopped it");
    fixture.world.borrow_mut().start();
    fixture
        .update(&fixture.v2)
        .expect("upgrade after the user started it");
    fixture.assert_managed(&location, &fixture.v2.id);
    assert_eq!(
        fs::read(launcher_path(&location)).expect("launcher"),
        settled,
        "the launcher stays the installation's"
    );
}

#[test]
fn an_adopted_historical_root_never_binds_without_a_location() {
    let (fixture, _location) = managed_v1();
    let LinuxInstallElection::Managed { lock, .. } =
        elect_linux_installation_with(&fixture.home, &fixture.private()).expect("election")
    else {
        panic!("expected managed authority");
    };
    let historical = InstallStore::new(fixture.home.join(".local/lib/hypercolor"), 64 * 1024);
    let bind = || {
        let world = Rc::clone(&fixture.world);
        bind_linux_platform(
            &fixture.home,
            |_, _, _| {
                Ok(SimExecutor {
                    world,
                    active_root: None,
                })
            },
            &historical,
            &lock,
            LinuxPlatformInputs {
                candidate: None,
                journal: None,
                managed: None,
                original: None,
                probation: DEFAULT_PROBATION_WINDOW,
            },
        )
        .map(drop)
        .map_err(|error| error.to_string())
    };
    let error = bind().expect_err("the locator names a managed installation");
    assert!(error.contains("recorded location"), "{error}");

    // A locator that cannot be read proves nothing about adoption.
    let locator = fixture
        .home
        .join(".local/lib/hypercolor/install-journal.json");
    let mode = fs::metadata(&locator)
        .expect("locator")
        .permissions()
        .mode();
    fs::set_permissions(&locator, fs::Permissions::from_mode(0o600)).expect("thaw");
    let original = fs::read(&locator).expect("locator bytes");
    fs::write(&locator, b"not a locator").expect("corrupt");
    let error = bind().expect_err("an unreadable locator refuses too");
    assert!(error.contains("recorded location"), "{error}");
    fs::write(&locator, original).expect("restore");
    fs::set_permissions(&locator, fs::Permissions::from_mode(mode)).expect("mode");
}

#[test]
fn the_published_layout_directories_are_every_directory_the_installer_writes() {
    let (fixture, location) = managed_v1();
    let directories = linux_layout_directories(&fixture.home);
    let mut expected: Vec<PathBuf> = [
        ".local/bin",
        ".local/share/applications",
        ".local/share/bash-completion/completions",
        ".local/share/zsh/site-functions",
        ".local/share/fish/vendor_completions.d",
        ".local/share/icons/hicolor/48x48/apps",
        ".local/share/icons/hicolor/128x128/apps",
        ".local/share/icons/hicolor/256x256/apps",
        ".config/systemd/user",
    ]
    .iter()
    .map(|path| fixture.home.join(path))
    .collect();
    expected.sort();
    let mut actual = directories.clone();
    actual.sort();
    assert_eq!(actual, expected);
    assert!(
        Path::new(&fixture.world.borrow().fragment)
            .parent()
            .is_some_and(|parent| directories.iter().any(|directory| directory == parent)),
        "the service fragment's directory is listed"
    );
    let _ = location;
}
