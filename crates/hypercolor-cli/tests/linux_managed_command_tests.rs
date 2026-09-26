#![cfg(target_os = "linux")]

//! Command-level topology, adoption, recovery and uninstall coverage.
//!
//! These suites drive the exact orchestration behind `__install-release` and
//! `__uninstall-release` through a simulated systemd user manager. Real
//! stores, recorded roots, locks, journals and immutable units live on disk;
//! only service, launcher and public-layout effects are modelled, and the
//! simulated daemon runs whatever file its launcher names.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;

use hypercolor_cli::install::{
    DirectoryRefusal, InstallCoordinator, InstallDisposition, InstallLock, InstallOutcome,
    InstallPlatformError, InstallRequest, InstallStore, InstallStoreError, InstallTargetPolicy,
    InstallTransactionId, LINUX_LAYOUT_ITEMS, LinuxDirectoryItem, LinuxDirectoryState,
    LinuxExactEntry, LinuxFilePublication, LinuxHttpResponse, LinuxInstallCheckpoint,
    LinuxInstallCommandError, LinuxInstallConfig, LinuxInstallElection, LinuxInstallExecutor,
    LinuxInstallHost, LinuxInstallLocation, LinuxInstallPlatform, LinuxInstallRequest,
    LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacyFile, LinuxLocatorError,
    LinuxProcessExecutable, LinuxPublicTree, LinuxRuntimeSettlement, LinuxUninstallCheckpoint,
    LinuxUninstallHost, OwnershipPolicy, PlatformTransactionRecord, PrincipalDatabase,
    PrincipalGroup, PrincipalUser, UnitId, UnitRecord, elect_linux_installation_with,
    run_linux_install, run_linux_uninstall, stage_release_payload,
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
}

#[derive(Debug)]
struct World {
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
}

type Shared = Rc<RefCell<World>>;

impl World {
    fn new(home: &Path) -> Shared {
        Rc::new(RefCell::new(Self {
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
        format!(
            "LoadState={}\nActiveState={}\nSubState={}\nUnitFileState={}\nFragmentPath={}\nExecStart={}\nMainPID={}\nInvocationID={}\n",
            if self.loaded { "loaded" } else { "not-found" },
            if self.active { "active" } else { "inactive" },
            if self.active { "running" } else { "dead" },
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

    fn start(&mut self) {
        let executable = Path::new(
            self.exec_start
                .split_ascii_whitespace()
                .next()
                .expect("launcher executable"),
        );
        let resolved = fs::canonicalize(executable).expect("launcher executable exists");
        let bytes = fs::read(&resolved).expect("daemon bytes");
        let metadata = fs::metadata(&resolved).expect("daemon metadata");
        self.process = Some(Process {
            path: resolved.to_str().expect("UTF-8").to_owned(),
            sha256: sha256(&bytes),
            device: metadata.dev(),
            inode: metadata.ino(),
        });
        self.active = true;
        self.invocation += 1;
        self.pid = 4000 + self.invocation;
    }

    fn stop(&mut self) {
        self.active = false;
        self.last_pid = self.pid;
        self.pid = 0;
        self.process = None;
    }

    fn running_version(&self) -> String {
        let process = self.process.as_ref().expect("running daemon");
        let unit = Path::new(&process.path)
            .parent()
            .and_then(Path::parent)
            .expect("unit root");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(unit.join("manifest.json")).expect("manifest"))
                .expect("manifest JSON");
        manifest["version"].as_str().expect("version").to_owned()
    }
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
        Ok(LinuxRuntimeSettlement::Settled)
    }

    fn systemd_show(&mut self, max_bytes: usize) -> Result<Vec<u8>, InstallPlatformError> {
        let output = self.world.borrow().show();
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
        if running {
            world.start();
        } else {
            world.stop();
        }
        world.settle(crash);
        Ok(())
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
        })
    }

    fn http_get(
        &mut self,
        path: &'static str,
        max_bytes: usize,
    ) -> Result<LinuxHttpResponse, InstallPlatformError> {
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
        let mut lock = old.acquire_anchored_lock(&self.home).expect("legacy lock");
        let unit = release.stage(&old, &lock);
        self.world.borrow_mut().historical.push(unit.clone());
        let config = LinuxInstallConfig {
            direct_fragment_path: self.world.borrow().fragment.clone(),
            immutable_units_root: old.root().join("units"),
            active_root: old.active_path(),
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
                active_unit: release.id.clone()
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
        assert!(
            launcher_exec(&world.launcher_bytes).starts_with(
                location
                    .release_root()
                    .join("active/bin/hypercolor-daemon")
                    .to_str()
                    .expect("UTF-8")
            )
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
    let files = [
        ("bin/hypercolor-daemon", daemon),
        ("bin/hypercolor", b"candidate".as_slice()),
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
    let manifest = serde_json::to_vec_pretty(&json!({
        "name":"hypercolor","version":version,"platform":"linux-x86_64",
        "rust_target":"x86_64-unknown-linux-gnu",
        "binaries":["hypercolor-daemon","hypercolor","hypercolor-app","hypercolor-tui","hypercolor-open"],
        "assets":{"ui_files":1,"bundled_effect_files":1,"docs_files":0,"skill_files":1,
            "user_skill_files":1,"agent_files":1,"site_files":0},
        "members":members,
    }))
    .expect("manifest JSON");
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
    // A restart brings the stopped service back behind the journal's back.
    fixture.world.borrow_mut().start();
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
