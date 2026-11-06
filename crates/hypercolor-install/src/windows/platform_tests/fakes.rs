//! An in-memory machine whose every effect is a numbered boundary a test
//! can crash at, over a real store with real unit directories.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;
use sha2::{Digest as _, Sha256};

use crate::windows::{
    WINDOWS_APP_MEMBER, WINDOWS_BROKER_MEMBER, WINDOWS_DAEMON_MEMBER, WindowsAppStart,
    WindowsBrokerObservation, WindowsFileIdentity, WindowsFirewallObservation, WindowsFirewallRule,
    WindowsInstallConfig, WindowsInstallExecutor, WindowsProcessId, WindowsProcessImage,
    WindowsRunningProcess, WindowsRuntimeObservation, WindowsSessionHost, WindowsUnitBinding,
};
use crate::{
    InstallLock, InstallPlatformError, InstallStore, LocalHttpResponse, UnitId, UnitRecord,
};

pub(super) const UNITS_ROOT: &str = r"C:\Program Files\Hypercolor\units";
pub(super) const PRIOR_VERSION: &str = "0.9.0";
pub(super) const CANDIDATE_VERSION: &str = "0.9.1";
pub(super) const VOLUME: u64 = 0x1234;

/// The unwinding payload of a simulated crash.
#[derive(Debug)]
pub(super) struct Crash;

#[derive(Debug, Default)]
pub(super) struct Boundaries {
    pub(super) passed: usize,
    pub(super) crash_at: Option<usize>,
    pub(super) log: Vec<String>,
}

pub(super) type SharedBoundaries = Arc<Mutex<Boundaries>>;

pub(super) fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run `body`, turning a simulated crash into `Err(Crash)`.
pub(super) fn survive<R>(body: impl FnOnce() -> R) -> Result<R, Crash> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result) => Ok(result),
        Err(payload) => match payload.downcast::<Crash>() {
            Ok(_) => Err(Crash),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    }
}

/// Pass one boundary, crashing there when the test asked for it.
pub(super) fn boundary(boundaries: &SharedBoundaries, label: &str) {
    let crash = {
        let mut boundaries = lock(boundaries);
        boundaries.passed += 1;
        boundaries.log.push(label.to_owned());
        boundaries.crash_at == Some(boundaries.passed)
    };
    if crash {
        std::panic::resume_unwind(Box::new(Crash));
    }
}

/// The path `unit` keeps `member` at.
pub(super) fn member_path(unit: &UnitId, member: &str) -> String {
    WindowsInstallConfig::new(UNITS_ROOT).member_path(unit, member)
}

/// The identity of a unit member on the fake volume: a function of where it
/// lives, the way a retained immutable unit's file identity is.
pub(super) fn identity_of(path: &str) -> WindowsFileIdentity {
    let digest = hex::encode(Sha256::digest(path.as_bytes()));
    WindowsFileIdentity {
        volume_serial: VOLUME,
        file_id: digest[..32].to_owned(),
        sha256: digest,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Broker {
    /// The registered command line, quoted the way the service control
    /// manager holds it.
    pub(super) image: String,
    pub(super) process: Option<WindowsProcessId>,
}

/// What the fake machine holds.
#[derive(Debug)]
pub(super) struct World {
    pub(super) store_root: PathBuf,
    pub(super) broker: Option<Broker>,
    pub(super) firewall: Option<WindowsFirewallObservation>,
    pub(super) apps: Vec<WindowsRunningProcess>,
    pub(super) daemons: Vec<WindowsRunningProcess>,
    pub(super) next_pid: u32,
    pub(super) clock: u64,
    /// The recorded user is signed in.
    pub(super) session: bool,
    /// The app holds the update pipe open.
    pub(super) app_reachable: bool,
    /// The app exits when it is asked to.
    pub(super) app_obeys: bool,
    /// Daemons of this version never answer on their local API.
    pub(super) unhealthy_version: Option<String>,
    /// Apps of this version start, but their daemons never get ready.
    pub(super) never_ready_version: Option<String>,
    /// A broker running from this path never starts.
    pub(super) failing_broker: Option<String>,
    pub(super) failing_rule: Option<WindowsFirewallRule>,
    /// The daemon exits partway through its probation window.
    pub(super) dies_in_probation: bool,
    /// The local API stops answering partway through the window.
    pub(super) hangs_in_probation: bool,
    /// A process running this path reports another file's identity.
    pub(super) swapped_image: Option<String>,
    /// A process running the first path reports the second as its image,
    /// with the first file's identity.
    pub(super) relocated_image: Option<(String, String)>,
    /// How many deadlines were spent waiting on processes that never quit.
    pub(super) waited_out_deadlines: usize,
    pub(super) probation_windows: Vec<Duration>,
}

impl World {
    fn spawn(&mut self, image: String) -> WindowsRunningProcess {
        let pid = self.next_pid;
        self.next_pid += 1;
        self.clock += 1;
        WindowsRunningProcess {
            id: WindowsProcessId {
                pid,
                created: self.clock,
            },
            image,
        }
    }

    /// Start `unit`'s app and its daemon, the way a sign-in does.
    pub(super) fn launch(&mut self, unit: &UnitId) {
        let app = self.spawn(member_path(unit, WINDOWS_APP_MEMBER));
        let daemon = self.spawn(member_path(unit, WINDOWS_DAEMON_MEMBER));
        self.apps.push(app);
        self.daemons.push(daemon);
    }

    fn start_broker_process(&mut self) -> Result<WindowsProcessId, InstallPlatformError> {
        let image = self
            .broker
            .as_ref()
            .ok_or_else(|| InstallPlatformError::new("no broker registered"))?
            .image
            .clone();
        if let Some(process) = self.broker.as_ref().and_then(|broker| broker.process) {
            return Ok(process);
        }
        let program = image.trim_matches('"').to_owned();
        if self.failing_broker.as_deref() == Some(program.as_str()) {
            return Err(InstallPlatformError::new("the broker stopped at once"));
        }
        let process = self.spawn(program).id;
        if let Some(broker) = &mut self.broker {
            broker.process = Some(process);
        }
        Ok(process)
    }

    /// Lose power: every process dies, nobody is signed in, and the service
    /// control manager starts the broker from whatever its image names.
    pub(super) fn reboot(&mut self) {
        self.apps.clear();
        self.daemons.clear();
        self.session = false;
        if let Some(broker) = &mut self.broker {
            broker.process = None;
        }
        if self.broker.is_some() {
            drop(self.start_broker_process());
        }
    }

    fn version_of(unit_image: &str) -> Result<&'static str, InstallPlatformError> {
        if unit_image.contains(&"a".repeat(64)) {
            Ok(PRIOR_VERSION)
        } else if unit_image.contains(&"b".repeat(64)) {
            Ok(CANDIDATE_VERSION)
        } else {
            Err(InstallPlatformError::new("not a known unit's image"))
        }
    }

    fn find(&self, id: WindowsProcessId) -> Option<&WindowsRunningProcess> {
        self.apps
            .iter()
            .chain(&self.daemons)
            .find(|process| process.id == id)
    }

    fn is_broker(&self, id: WindowsProcessId) -> bool {
        self.broker
            .as_ref()
            .is_some_and(|broker| broker.process == Some(id))
    }

    pub(super) fn end(&mut self, processes: &[WindowsProcessId]) {
        self.apps.retain(|app| !processes.contains(&app.id));
        self.daemons
            .retain(|daemon| !processes.contains(&daemon.id));
    }
}

pub(super) type SharedWorld = Arc<Mutex<World>>;

fn fail(detail: &str) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

pub(super) struct FakeExecutor {
    pub(super) world: SharedWorld,
    pub(super) boundaries: SharedBoundaries,
}

impl WindowsInstallExecutor for FakeExecutor {
    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        let link = lock(&self.world).store_root.join("active");
        match fs::read_link(link) {
            Ok(target) => {
                let name = target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| fail("active pointer names nothing"))?;
                UnitId::new(name)
                    .map(Some)
                    .map_err(|source| InstallPlatformError::new(source.to_string()))
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(InstallPlatformError::new(source.to_string())),
        }
    }

    fn bind_unit(&mut self, unit: &UnitRecord) -> Result<WindowsUnitBinding, InstallPlatformError> {
        let version = World::version_of(unit.id().as_str())?;
        let identity = |member| identity_of(&member_path(unit.id(), member));
        Ok(WindowsUnitBinding {
            unit: unit.id().clone(),
            version: version.to_owned(),
            app: identity(WINDOWS_APP_MEMBER),
            daemon: identity(WINDOWS_DAEMON_MEMBER),
            broker: identity(WINDOWS_BROKER_MEMBER),
        })
    }

    fn observe_broker(&mut self) -> Result<Option<WindowsBrokerObservation>, InstallPlatformError> {
        Ok(lock(&self.world)
            .broker
            .as_ref()
            .map(|broker| WindowsBrokerObservation {
                image: broker.image.clone(),
                running: broker.process.is_some(),
            }))
    }

    fn switch_broker(&mut self, program: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "broker-stop:before");
        {
            let mut world = lock(&self.world);
            let broker = world
                .broker
                .as_mut()
                .ok_or_else(|| fail("no broker registered"))?;
            broker.process = None;
        }
        boundary(&self.boundaries, "broker-stop:after");
        {
            let mut world = lock(&self.world);
            let broker = world
                .broker
                .as_mut()
                .ok_or_else(|| fail("no broker registered"))?;
            broker.image = format!("\"{program}\"");
        }
        boundary(&self.boundaries, "broker-switch:after");
        Ok(())
    }

    fn start_broker(&mut self) -> Result<WindowsProcessId, InstallPlatformError> {
        boundary(&self.boundaries, "broker-start:before");
        let process = lock(&self.world).start_broker_process()?;
        boundary(&self.boundaries, "broker-start:after");
        Ok(process)
    }

    fn observe_firewall(
        &mut self,
    ) -> Result<Option<WindowsFirewallObservation>, InstallPlatformError> {
        Ok(lock(&self.world).firewall.clone())
    }

    fn set_firewall_rule(
        &mut self,
        rule: WindowsFirewallRule,
        program: Option<&str>,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "firewall:before");
        {
            let mut world = lock(&self.world);
            if world.failing_rule == Some(rule) {
                return Err(fail("the firewall refused"));
            }
            let firewall = world
                .firewall
                .as_mut()
                .ok_or_else(|| fail("no firewall is running"))?;
            let held = match rule {
                WindowsFirewallRule::Daemon => &mut firewall.daemon,
                WindowsFirewallRule::App => &mut firewall.app,
            };
            *held = program.map(str::to_owned);
        }
        boundary(&self.boundaries, "firewall:after");
        Ok(())
    }

    fn observe_runtime(&mut self) -> Result<WindowsRuntimeObservation, InstallPlatformError> {
        let world = lock(&self.world);
        Ok(WindowsRuntimeObservation {
            apps: world.apps.clone(),
            daemons: world.daemons.clone(),
        })
    }

    fn process_image(
        &mut self,
        process: WindowsProcessId,
    ) -> Result<Option<WindowsProcessImage>, InstallPlatformError> {
        let world = lock(&self.world);
        let path = if world.is_broker(process) {
            world
                .broker
                .as_ref()
                .map(|broker| broker.image.trim_matches('"').to_owned())
        } else {
            world.find(process).map(|running| running.image.clone())
        };
        Ok(path.map(|path| {
            let identity = if world.swapped_image.as_deref() == Some(path.as_str()) {
                identity_of("another file altogether")
            } else {
                identity_of(&path)
            };
            let path = match &world.relocated_image {
                Some((from, to)) if *from == path => to.clone(),
                _ => path,
            };
            WindowsProcessImage { path, identity }
        }))
    }

    fn wait_for_every_exit(
        &mut self,
        processes: &[WindowsProcessId],
        _timeout: Duration,
    ) -> Result<bool, InstallPlatformError> {
        let mut world = lock(&self.world);
        let exited = processes.iter().all(|id| world.find(*id).is_none());
        if !exited {
            world.waited_out_deadlines += 1;
        }
        Ok(exited)
    }

    fn wait_for_any_exit(
        &mut self,
        processes: &[WindowsProcessId],
        timeout: Duration,
    ) -> Result<Option<WindowsProcessId>, InstallPlatformError> {
        boundary(&self.boundaries, "probation");
        let mut world = lock(&self.world);
        world.probation_windows.push(timeout);
        if world.dies_in_probation {
            let daemons: Vec<_> = world.daemons.iter().map(|daemon| daemon.id).collect();
            world.end(&daemons);
        }
        if world.hangs_in_probation {
            world.unhealthy_version = Some(CANDIDATE_VERSION.to_owned());
        }
        Ok(processes
            .iter()
            .find(|id| world.find(**id).is_none())
            .copied())
    }

    fn terminate(&mut self, processes: &[WindowsProcessId]) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "terminate:before");
        lock(&self.world).end(processes);
        boundary(&self.boundaries, "terminate:after");
        Ok(())
    }

    fn wait_for_daemon(
        &mut self,
        _app: WindowsProcessId,
        _timeout: Duration,
    ) -> Result<(), InstallPlatformError> {
        let world = lock(&self.world);
        let daemon = world
            .daemons
            .first()
            .ok_or_else(|| fail("the app exited before its daemon was ready"))?;
        if world.never_ready_version.as_deref() == Some(World::version_of(&daemon.image)?) {
            return Err(fail("the daemon was not ready in time"));
        }
        Ok(())
    }

    fn http_get(
        &mut self,
        path: &str,
        _max_bytes: usize,
    ) -> Result<LocalHttpResponse, InstallPlatformError> {
        boundary(&self.boundaries, "http");
        let world = lock(&self.world);
        let daemon = world
            .daemons
            .first()
            .ok_or_else(|| fail("nothing answers on the local API"))?;
        let version = World::version_of(&daemon.image)?;
        if world.unhealthy_version.as_deref() == Some(version) {
            return Err(fail("the local API does not answer"));
        }
        let body = match path {
            "/health" => serde_json::json!({"status": "healthy", "version": version}),
            "/api/v1/system" => serde_json::json!({"data": {"identity": {
                "version": version,
                "instance_id": "instance",
                "instance_name": "lights",
            }}}),
            other => panic!("unexpected proof request {other}"),
        };
        Ok(LocalHttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).expect("encode a proof response"),
        })
    }
}

pub(super) struct FakeSession {
    pub(super) world: SharedWorld,
    pub(super) boundaries: SharedBoundaries,
}

impl WindowsSessionHost for FakeSession {
    fn ask_app_to_quit(&mut self) -> bool {
        boundary(&self.boundaries, "quit:before");
        let reachable = {
            let mut world = lock(&self.world);
            if world.app_reachable && world.app_obeys {
                world.apps.clear();
                world.daemons.clear();
            }
            world.app_reachable
        };
        boundary(&self.boundaries, "quit:after");
        reachable
    }

    fn start_app(
        &mut self,
        app: &str,
        arguments: &[&str],
    ) -> Result<WindowsAppStart, InstallPlatformError> {
        assert_eq!(arguments, ["--minimized"]);
        boundary(&self.boundaries, "start:before");
        let started = {
            let mut world = lock(&self.world);
            if !world.session {
                return Ok(WindowsAppStart::NoSession);
            }
            let unit = app
                .strip_prefix(UNITS_ROOT)
                .and_then(|rest| rest.strip_prefix('\\'))
                .and_then(|rest| rest.split('\\').next())
                .ok_or_else(|| fail("the app is not in a unit"))?;
            let unit = UnitId::new(unit)
                .map_err(|source| InstallPlatformError::new(source.to_string()))?;
            world.launch(&unit);
            world.apps[0].clone()
        };
        boundary(&self.boundaries, "start:after");
        Ok(WindowsAppStart::Started(started))
    }
}

/// A real store holding two real unit directories, the prior active.
///
/// The store's lock is taken once and every transaction shares it, the way
/// one activator process holds it.
pub(super) struct Fixture {
    _root: tempfile::TempDir,
    pub(super) store: InstallStore,
    lock: RefCell<InstallLock>,
    pub(super) prior: UnitRecord,
    pub(super) candidate: UnitRecord,
}

fn unit_record(store: &InstallStore, seed: char) -> UnitRecord {
    let id = UnitId::new(seed.to_string().repeat(64)).expect("unit id");
    let path = store.root().join("units").join(id.as_str());
    fs::create_dir_all(&path).expect("unit directory");
    let directory = ReadOnlyDirectoryAuthority::open(&path).expect("open unit");
    UnitRecord::new(id, path, directory).expect("unit record")
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().expect("fixture root");
        let store = InstallStore::new(root.path().join("releases"), 64 * 1024);
        let lock = store.acquire_lock().expect("fixture lock");
        let prior = unit_record(&store, 'a');
        let candidate = unit_record(&store, 'b');
        store
            .set_active(Some(prior.id()), &lock)
            .expect("activate the prior");
        Self {
            _root: root,
            store,
            lock: RefCell::new(lock),
            prior,
            candidate,
        }
    }

    /// Run `body` under the fixture's store lock.
    pub(super) fn with_lock<R>(&self, body: impl FnOnce(&mut InstallLock) -> R) -> R {
        body(&mut self.lock.borrow_mut())
    }

    pub(super) fn active_unit(&self) -> Option<UnitId> {
        self.with_lock(|lock| self.store.active_unit(lock).expect("active unit"))
    }

    pub(super) fn journal(&self) -> Option<crate::InstallJournalV1> {
        self.with_lock(|lock| self.store.load_journal(lock).expect("load journal"))
    }

    pub(super) fn deactivate(&self) {
        self.with_lock(|lock| self.store.set_active(None, lock).expect("deactivate"));
    }
}
