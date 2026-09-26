use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::FutureExt as _;
use futures_util::future::LocalBoxFuture;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::super::InstallPlatformError;
use super::manager_bus::{ManagerBus, ManagerCallError};
use super::model::{LinuxServiceIdentity, LinuxServiceWatch, error};

const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
/// The user manager's own socket, relative to `XDG_RUNTIME_DIR`.
///
/// It serves the manager's D-Bus API peer to peer, so reaching systemd never
/// requires the session bus or anything else that listens on it.
const PRIVATE_SOCKET: &str = "systemd/private";
const SERVICE: &str = "hypercolor.service";
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxSystemdConnection {
    runtime_directory: PathBuf,
    manager_socket: PathBuf,
    manager_address: String,
    manager_uid: u32,
}

impl LinuxSystemdConnection {
    pub(super) fn from_environment() -> Result<Self, InstallPlatformError> {
        let runtime_directory = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| error("XDG_RUNTIME_DIR is required for the user systemd manager"))?;
        let current_uid = current_uid()?;
        Self::from_runtime_directory(Path::new(&runtime_directory), current_uid)
    }

    /// Bind one exact user-manager runtime directory owned by `expected_uid`.
    ///
    /// The manager is reached through its private socket
    /// (`systemd/private`), never the session bus, so a sandbox that denies
    /// the session bus can still drive the service.
    ///
    /// # Errors
    ///
    /// Returns an error for a noncanonical path, unsafe mode or owner, or a
    /// missing, foreign, or non-socket private manager endpoint.
    pub fn from_runtime_directory(
        runtime_directory: &Path,
        expected_uid: u32,
    ) -> Result<Self, InstallPlatformError> {
        validate_runtime_path(runtime_directory)?;
        let metadata = fs::symlink_metadata(runtime_directory).map_err(io_error)?;
        if !metadata.is_dir()
            || metadata.uid() != expected_uid
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(error(
                "XDG_RUNTIME_DIR is not an exact private directory owned by the current uid",
            ));
        }
        let manager_directory = runtime_directory.join("systemd");
        let directory_metadata = fs::symlink_metadata(&manager_directory)
            .map_err(|source| error(format!("XDG_RUNTIME_DIR systemd is unavailable: {source}")))?;
        if !directory_metadata.is_dir()
            || directory_metadata.uid() != expected_uid
            || directory_metadata.permissions().mode() & 0o022 != 0
        {
            return Err(error(
                "XDG_RUNTIME_DIR systemd is not a directory only the current uid can write",
            ));
        }
        let socket = runtime_directory.join(PRIVATE_SOCKET);
        let socket_metadata = fs::symlink_metadata(&socket).map_err(|source| {
            error(format!(
                "XDG_RUNTIME_DIR systemd/private is unavailable: {source}"
            ))
        })?;
        if !socket_metadata.file_type().is_socket() || socket_metadata.uid() != expected_uid {
            return Err(error(
                "XDG_RUNTIME_DIR systemd/private is not an owned Unix socket",
            ));
        }
        let runtime_text = runtime_directory
            .to_str()
            .ok_or_else(|| error("XDG_RUNTIME_DIR is not UTF-8"))?;
        Ok(Self {
            runtime_directory: runtime_directory.to_owned(),
            manager_socket: socket,
            manager_address: format!("unix:path={runtime_text}/{PRIVATE_SOCKET}"),
            manager_uid: expected_uid,
        })
    }

    /// Environment for `systemctl --user` subprocesses.
    ///
    /// `systemctl` reaches the manager through `XDG_RUNTIME_DIR` and its
    /// private socket first, and falls back to the session bus only when
    /// that fails. Pointing the session bus address at the private socket
    /// too leaves that fallback nowhere else to go.
    #[must_use]
    pub fn command_environment(&self) -> [(&'static str, &OsStr); 2] {
        [
            ("XDG_RUNTIME_DIR", self.runtime_directory.as_os_str()),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                OsStr::new(self.manager_address.as_str()),
            ),
        ]
    }
}

#[derive(Debug, Clone)]
pub(super) struct LinuxRuntimeManager {
    connection: LinuxSystemdConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeJobOutcome {
    Done,
    Cancelled,
}

/// Whether `hypercolor.service` reached a phase a checkpoint can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxRuntimeSettlement {
    /// No job is queued or running and the service is running or stopped.
    Settled,
    /// A job outlived its unit-derived deadline, or the service is waiting to
    /// restart on its own. Callers treat it as running, never as stopped.
    Unsettled,
}

impl LinuxRuntimeManager {
    pub(super) fn new(connection: LinuxSystemdConnection) -> Self {
        Self { connection }
    }

    /// Start or stop the service and fence the job at the unit's deadline.
    ///
    /// A failed service is reset before it starts, so a start limit left by
    /// a crash-looping release never blocks the next start.
    pub(super) fn set_runtime(
        &self,
        running: bool,
    ) -> Result<RuntimeJobOutcome, InstallPlatformError> {
        let connection = self.connection.clone();
        on_worker(move || run_runtime_job(&connection, running))
    }

    /// Wait for every queued or running job on the service to finish,
    /// bounded by the deadlines its own unit declares.
    pub(super) fn settle(&self) -> Result<LinuxRuntimeSettlement, InstallPlatformError> {
        let connection = self.connection.clone();
        on_worker(move || run_settle(&connection))
    }

    /// Hold the service to `expected` for `window`, returning at the first
    /// change to its `ActiveState`, `SubState`, `InvocationID` or `MainPID`.
    ///
    /// The wait is event-driven: the service is read again only when the
    /// manager signals a change to it, never on a timer.
    pub(super) fn watch(
        &self,
        expected: LinuxServiceIdentity,
        window: Duration,
    ) -> Result<LinuxServiceWatch, InstallPlatformError> {
        let connection = self.connection.clone();
        on_worker(move || run_watch(&connection, &expected, window))
    }
}

/// Run one manager conversation on its own thread and runtime, so callers
/// inside an async context never block their executor.
fn on_worker<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, InstallPlatformError> + Send + 'static,
) -> Result<T, InstallPlatformError> {
    std::thread::spawn(work)
        .join()
        .map_err(|_| error("systemd D-Bus job worker panicked"))?
}

fn current_thread_runtime() -> Result<tokio::runtime::Runtime, InstallPlatformError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_error)
}

/// The longest any single start or stop job may run, whatever the unit says.
const MAX_JOB_DEADLINE: Duration = Duration::from_mins(3);
/// Slack past the unit's own timeout, so systemd's timeout (and the kill it
/// sends) always resolves the job before this fence cancels it.
const JOB_GRACE: Duration = Duration::from_secs(10);

/// Start and stop deadlines derived from the unit's own timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ServiceDeadlines {
    pub(super) start: Duration,
    pub(super) stop: Duration,
}

impl ServiceDeadlines {
    /// Derive deadlines from `TimeoutStartUSec` and `TimeoutStopUSec`.
    ///
    /// Zero and `u64::MAX` both mean the unit waits forever; those, and any
    /// timeout above the bound, fence at the bound instead.
    pub(super) fn from_unit(timeout_start_usec: u64, timeout_stop_usec: u64) -> Self {
        Self {
            start: unit_deadline(timeout_start_usec),
            stop: unit_deadline(timeout_stop_usec),
        }
    }
}

fn unit_deadline(usec: u64) -> Duration {
    if usec == 0 || usec == u64::MAX {
        return MAX_JOB_DEADLINE;
    }
    (Duration::from_micros(usec) + JOB_GRACE).min(MAX_JOB_DEADLINE)
}

/// The manager's view of the service at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ServiceSnapshot {
    pub(super) active_state: String,
    pub(super) sub_state: String,
    /// The type of the queued or running job, if any.
    pub(super) job: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettleWait {
    Steady,
    Wait(Duration),
    Unsettled,
}

fn settle_wait(snapshot: &ServiceSnapshot, deadlines: ServiceDeadlines) -> SettleWait {
    let Some(job) = snapshot.job.as_deref() else {
        return match (snapshot.active_state.as_str(), snapshot.sub_state.as_str()) {
            ("active", "running") | ("inactive", "dead") | ("failed", "failed") => {
                SettleWait::Steady
            }
            // The main process is gone and the manager is collecting the
            // rest of the service; that ends within the stop timeout.
            ("deactivating", _) => SettleWait::Wait(deadlines.stop),
            // Waiting to restart on its own (or otherwise moving without a
            // job): nothing here bounds it, so report it as it is.
            _ => SettleWait::Unsettled,
        };
    };
    match job {
        "start" | "verify-active" | "reload" => SettleWait::Wait(deadlines.start),
        "stop" => SettleWait::Wait(deadlines.stop),
        _ => SettleWait::Wait(deadlines.start + deadlines.stop),
    }
}

/// What settling needs from the manager.
trait SettleBoundary {
    fn snapshot(&mut self) -> LocalBoxFuture<'_, Result<ServiceSnapshot, InstallPlatformError>>;

    fn deadlines(&mut self) -> LocalBoxFuture<'_, Result<ServiceDeadlines, InstallPlatformError>>;

    /// Wait for the next change to the service. `Ok(false)` means the
    /// deadline passed first.
    fn changed(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> LocalBoxFuture<'_, Result<bool, InstallPlatformError>>;
}

async fn settle_service(
    boundary: &mut impl SettleBoundary,
) -> Result<LinuxRuntimeSettlement, InstallPlatformError> {
    let first = boundary.snapshot().await?;
    // Only a pending job or a stop in flight needs the unit's timeouts.
    let deadlines = if first.job.is_some() || first.active_state == "deactivating" {
        boundary.deadlines().await?
    } else {
        ServiceDeadlines::from_unit(0, 0)
    };
    let deadline = match settle_wait(&first, deadlines) {
        SettleWait::Steady => return Ok(LinuxRuntimeSettlement::Settled),
        SettleWait::Unsettled => return Ok(LinuxRuntimeSettlement::Unsettled),
        SettleWait::Wait(wait) => tokio::time::Instant::now() + wait,
    };
    loop {
        if !boundary.changed(deadline).await? {
            return Ok(LinuxRuntimeSettlement::Unsettled);
        }
        match settle_wait(&boundary.snapshot().await?, deadlines) {
            SettleWait::Steady => return Ok(LinuxRuntimeSettlement::Settled),
            SettleWait::Unsettled => return Ok(LinuxRuntimeSettlement::Unsettled),
            SettleWait::Wait(_) => {}
        }
    }
}

/// The service's runtime identity at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ServiceIdentity {
    pub(super) active_state: String,
    pub(super) sub_state: String,
    /// Lowercase hex, as `systemctl show` prints it.
    pub(super) invocation_id: String,
    pub(super) main_pid: u32,
}

impl ServiceIdentity {
    fn holds(&self, expected: &LinuxServiceIdentity) -> bool {
        self.active_state == "active"
            && self.sub_state == "running"
            && self.invocation_id == expected.invocation_id
            && self.main_pid == expected.main_pid
    }

    fn describe(&self) -> String {
        format!(
            "{}/{} under invocation {} with main pid {}",
            self.active_state,
            self.sub_state,
            if self.invocation_id.is_empty() {
                "none"
            } else {
                self.invocation_id.as_str()
            },
            self.main_pid
        )
    }
}

/// What a probation watch needs from the manager.
trait WatchBoundary {
    fn identity(&mut self) -> LocalBoxFuture<'_, Result<ServiceIdentity, InstallPlatformError>>;

    /// Wait for the next change to the service. `Ok(false)` means the
    /// deadline passed first.
    fn changed(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> LocalBoxFuture<'_, Result<bool, InstallPlatformError>>;
}

/// Hold the service to `expected` until `window` passes or it changes.
///
/// Every signal about the service rereads its identity, so a change that
/// leaves and returns between two reads (a restart) still shows as a new
/// invocation. A change arriving between a read and the wait stays queued
/// in the connection, so none is lost.
async fn watch_service(
    boundary: &mut impl WatchBoundary,
    expected: &LinuxServiceIdentity,
    window: Duration,
) -> Result<LinuxServiceWatch, InstallPlatformError> {
    let started = tokio::time::Instant::now();
    let deadline = started + window;
    loop {
        let identity = boundary.identity().await?;
        if !identity.holds(expected) {
            return Ok(LinuxServiceWatch::Changed {
                after: started.elapsed(),
                observed: identity.describe(),
            });
        }
        if !boundary.changed(deadline).await? {
            return Ok(LinuxServiceWatch::Steady);
        }
    }
}

const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";
const SERVICE_INTERFACE: &str = "org.freedesktop.systemd1.Service";
const JOB_INTERFACE: &str = "org.freedesktop.systemd1.Job";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

struct BusSettleBoundary<'a> {
    bus: &'a mut ManagerBus,
    unit: OwnedObjectPath,
}

impl SettleBoundary for BusSettleBoundary<'_> {
    fn snapshot(&mut self) -> LocalBoxFuture<'_, Result<ServiceSnapshot, InstallPlatformError>> {
        async move { read_snapshot(self.bus, &self.unit).await }.boxed_local()
    }

    fn deadlines(&mut self) -> LocalBoxFuture<'_, Result<ServiceDeadlines, InstallPlatformError>> {
        async move { read_deadlines(self.bus, &self.unit).await }.boxed_local()
    }

    fn changed(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> LocalBoxFuture<'_, Result<bool, InstallPlatformError>> {
        next_service_change(self.bus, &self.unit, deadline).boxed_local()
    }
}

impl WatchBoundary for BusSettleBoundary<'_> {
    fn identity(&mut self) -> LocalBoxFuture<'_, Result<ServiceIdentity, InstallPlatformError>> {
        async move { read_identity(self.bus, &self.unit).await }.boxed_local()
    }

    fn changed(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> LocalBoxFuture<'_, Result<bool, InstallPlatformError>> {
        next_service_change(self.bus, &self.unit, deadline).boxed_local()
    }
}

/// Wait for a `PropertiesChanged` on the unit or a `JobRemoved` for the
/// service. `Ok(false)` means `deadline` passed first.
async fn next_service_change(
    bus: &mut ManagerBus,
    unit: &OwnedObjectPath,
    deadline: tokio::time::Instant,
) -> Result<bool, InstallPlatformError> {
    let unit = unit.as_str().to_owned();
    let next = async {
        loop {
            let signal = bus.next_signal().await?;
            let unit_changed = signal.path.as_deref() == Some(unit.as_str())
                && signal.is_signal(PROPERTIES_INTERFACE, "PropertiesChanged");
            let job_removed = signal.path.as_deref() == Some(SYSTEMD_PATH)
                && signal.is_signal(SYSTEMD_MANAGER, "JobRemoved")
                && signal
                    .body::<(u32, OwnedObjectPath, String, String)>()
                    .is_ok_and(|(_, _, removed, _)| removed == SERVICE);
            if unit_changed || job_removed {
                return Ok::<(), InstallPlatformError>(());
            }
        }
    };
    match tokio::time::timeout_at(deadline, next).await {
        Err(_) => Ok(false),
        Ok(result) => result.map(|()| true),
    }
}

fn run_settle(
    manager: &LinuxSystemdConnection,
) -> Result<LinuxRuntimeSettlement, InstallPlatformError> {
    current_thread_runtime()?.block_on(async move {
        // A direct manager connection receives every manager signal without
        // a bus match or `Subscribe`, and the client keeps every signal that
        // arrives while it reads, so no change can fall between the first
        // read and the wait.
        let mut bus = connect_manager(manager).await?;
        let unit = match within_method_deadline(bus.call::<_, OwnedObjectPath>(
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            "GetUnit",
            &(SERVICE,),
        ))
        .await?
        {
            Ok(unit) => unit,
            // An unloaded unit has no job and no process to wait for.
            Err(ManagerCallError::Refused { name, .. }) if name == NO_SUCH_UNIT => {
                return Ok(LinuxRuntimeSettlement::Settled);
            }
            Err(refused) => return Err(refused.into_platform()),
        };
        let mut boundary = BusSettleBoundary {
            bus: &mut bus,
            unit,
        };
        settle_service(&mut boundary).await
    })
}

fn run_watch(
    manager: &LinuxSystemdConnection,
    expected: &LinuxServiceIdentity,
    window: Duration,
) -> Result<LinuxServiceWatch, InstallPlatformError> {
    current_thread_runtime()?.block_on(async move {
        // A direct manager connection receives every manager signal without
        // `Subscribe` or a bus match, so the watch misses no change between
        // its first read and its first wait.
        let mut bus = connect_manager(manager).await?;
        let unit = match within_method_deadline(bus.call::<_, OwnedObjectPath>(
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            "GetUnit",
            &(SERVICE,),
        ))
        .await?
        {
            Ok(unit) => unit,
            // An unloaded unit runs nothing, so the proven service is gone.
            Err(ManagerCallError::Refused { name, .. }) if name == NO_SUCH_UNIT => {
                return Ok(LinuxServiceWatch::Changed {
                    after: Duration::ZERO,
                    observed: "hypercolor.service is no longer loaded".to_owned(),
                });
            }
            Err(refused) => return Err(refused.into_platform()),
        };
        let mut boundary = BusSettleBoundary {
            bus: &mut bus,
            unit,
        };
        watch_service(&mut boundary, expected, window).await
    })
}

fn run_runtime_job(
    manager: &LinuxSystemdConnection,
    running: bool,
) -> Result<RuntimeJobOutcome, InstallPlatformError> {
    current_thread_runtime()?.block_on(async move {
        let mut bus = connect_manager(manager).await?;
        let unit = manager_call::<_, OwnedObjectPath>(&mut bus, "LoadUnit", &(SERVICE,)).await?;
        let deadlines = read_deadlines(&mut bus, &unit).await?;
        if running {
            // Clears a failed state and the start-limit counter a crash loop
            // left, which a start would otherwise hit. A unit the manager has
            // already unloaded again has nothing to reset.
            match within_method_deadline(bus.call::<_, ()>(
                SYSTEMD_PATH,
                SYSTEMD_MANAGER,
                "ResetFailedUnit",
                &(SERVICE,),
            ))
            .await?
            {
                Ok(()) => {}
                Err(ManagerCallError::Refused { name, .. }) if name == NO_SUCH_UNIT => {}
                Err(refused) => return Err(refused.into_platform()),
            }
        }
        // A stop replaces a start still queued for a service that keeps
        // failing before readiness (systemd before 254 keeps that start job
        // across every automatic restart), where "fail" would be refused.
        let (method, mode) = if running {
            ("StartUnit", "fail")
        } else {
            ("StopUnit", "replace")
        };
        let job_path =
            manager_call::<_, OwnedObjectPath>(&mut bus, method, &(SERVICE, mode)).await?;
        let job = owned_job(job_path)?;
        let mut boundary = BusJobBoundary { bus: &mut bus };
        let deadline = if running {
            deadlines.start
        } else {
            deadlines.stop
        };
        fence_owned_job(&mut boundary, &job, deadline).await
    })
}

const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";

/// Call one manager method within the method deadline.
async fn manager_call<B, R>(
    bus: &mut ManagerBus,
    method: &str,
    body: &B,
) -> Result<R, InstallPlatformError>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: serde::de::DeserializeOwned,
{
    within_method_deadline(bus.call(SYSTEMD_PATH, SYSTEMD_MANAGER, method, body))
        .await?
        .map_err(ManagerCallError::into_platform)
}

async fn property<T>(
    bus: &mut ManagerBus,
    object: &OwnedObjectPath,
    interface: &str,
    name: &str,
) -> Result<T, InstallPlatformError>
where
    T: TryFrom<OwnedValue>,
    T::Error: std::fmt::Display,
{
    let value = within_method_deadline(bus.call::<_, OwnedValue>(
        object.as_str(),
        PROPERTIES_INTERFACE,
        "Get",
        &(interface, name),
    ))
    .await?
    .map_err(ManagerCallError::into_platform)?;
    T::try_from(value).map_err(|source| error(format!("systemd property {name}: {source}")))
}

async fn read_snapshot(
    bus: &mut ManagerBus,
    unit: &OwnedObjectPath,
) -> Result<ServiceSnapshot, InstallPlatformError> {
    let active_state = property::<String>(bus, unit, UNIT_INTERFACE, "ActiveState").await?;
    let sub_state = property::<String>(bus, unit, UNIT_INTERFACE, "SubState").await?;
    let (job_id, job_path): (u32, OwnedObjectPath) =
        property(bus, unit, UNIT_INTERFACE, "Job").await?;
    let job = if job_id == 0 {
        None
    } else {
        // A job can finish between the two reads; an unreadable type still
        // counts as pending and gets the widest deadline.
        Some(
            property::<String>(bus, &job_path, JOB_INTERFACE, "JobType")
                .await
                .unwrap_or_default(),
        )
    };
    Ok(ServiceSnapshot {
        active_state,
        sub_state,
        job,
    })
}

async fn read_identity(
    bus: &mut ManagerBus,
    unit: &OwnedObjectPath,
) -> Result<ServiceIdentity, InstallPlatformError> {
    let active_state = property::<String>(bus, unit, UNIT_INTERFACE, "ActiveState").await?;
    let sub_state = property::<String>(bus, unit, UNIT_INTERFACE, "SubState").await?;
    let invocation = property::<Vec<u8>>(bus, unit, UNIT_INTERFACE, "InvocationID").await?;
    let main_pid = property::<u32>(bus, unit, SERVICE_INTERFACE, "MainPID").await?;
    Ok(ServiceIdentity {
        active_state,
        sub_state,
        invocation_id: hex::encode(invocation),
        main_pid,
    })
}

async fn read_deadlines(
    bus: &mut ManagerBus,
    unit: &OwnedObjectPath,
) -> Result<ServiceDeadlines, InstallPlatformError> {
    let start = property::<u64>(bus, unit, SERVICE_INTERFACE, "TimeoutStartUSec").await?;
    let stop = property::<u64>(bus, unit, SERVICE_INTERFACE, "TimeoutStopUSec").await?;
    Ok(ServiceDeadlines::from_unit(start, stop))
}

/// Open the user manager's private socket, proving the peer runs as the
/// expected uid before any D-Bus traffic.
async fn connect_manager(
    manager: &LinuxSystemdConnection,
) -> Result<ManagerBus, InstallPlatformError> {
    within_method_deadline(ManagerBus::connect(
        &manager.manager_socket,
        manager.manager_uid,
    ))
    .await?
}

async fn within_method_deadline<T>(
    work: impl std::future::Future<Output = T>,
) -> Result<T, InstallPlatformError> {
    tokio::time::timeout(METHOD_TIMEOUT, work)
        .await
        .map_err(|_| error("user systemd manager call exceeded its deadline"))
}

struct OwnedJob {
    path: OwnedObjectPath,
    id: u32,
}

trait RuntimeJobBoundary {
    fn wait<'a>(
        &'a mut self,
        job: &'a OwnedJob,
        timeout: Duration,
    ) -> LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>>;

    fn cancel<'a>(
        &'a mut self,
        job: &'a OwnedJob,
    ) -> LocalBoxFuture<'a, Result<(), InstallPlatformError>>;
}

struct BusJobBoundary<'a> {
    bus: &'a mut ManagerBus,
}

impl RuntimeJobBoundary for BusJobBoundary<'_> {
    fn wait<'a>(
        &'a mut self,
        job: &'a OwnedJob,
        timeout: Duration,
    ) -> LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>> {
        async move {
            match tokio::time::timeout(timeout, wait_for_job(self.bus, job)).await {
                Ok(result) => result.map(Some),
                Err(_) => Ok(None),
            }
        }
        .boxed_local()
    }

    fn cancel<'a>(
        &'a mut self,
        job: &'a OwnedJob,
    ) -> LocalBoxFuture<'a, Result<(), InstallPlatformError>> {
        async move { manager_call::<_, ()>(self.bus, "CancelJob", &(job.id,)).await }.boxed_local()
    }
}

async fn fence_owned_job(
    boundary: &mut impl RuntimeJobBoundary,
    job: &OwnedJob,
    deadline: Duration,
) -> Result<RuntimeJobOutcome, InstallPlatformError> {
    if let Some(result) = boundary.wait(job, deadline).await? {
        require_job_result(&result, "done")?;
        return Ok(RuntimeJobOutcome::Done);
    }
    boundary.cancel(job).await?;
    let result = boundary
        .wait(job, CANCEL_TIMEOUT)
        .await?
        .ok_or_else(|| error("cancelled systemd job did not reach a terminal state"))?;
    if !matches!(result.as_str(), "canceled" | "done") {
        return Err(error("cancelled systemd job reported an unsafe result"));
    }
    Ok(RuntimeJobOutcome::Cancelled)
}

async fn wait_for_job(
    bus: &mut ManagerBus,
    expected: &OwnedJob,
) -> Result<String, InstallPlatformError> {
    loop {
        let signal = bus.next_signal().await?;
        if signal.path.as_deref() != Some(SYSTEMD_PATH)
            || !signal.is_signal(SYSTEMD_MANAGER, "JobRemoved")
        {
            continue;
        }
        let (id, path, unit, result): (u32, OwnedObjectPath, String, String) = signal.body()?;
        if let Some(result) = removed_job_result(id, &path, &unit, result, expected)? {
            return Ok(result);
        }
    }
}

fn removed_job_result(
    id: u32,
    path: &OwnedObjectPath,
    unit: &str,
    result: String,
    expected: &OwnedJob,
) -> Result<Option<String>, InstallPlatformError> {
    if path != &expected.path {
        return Ok(None);
    }
    if id != expected.id {
        return Err(error("systemd job path and numeric identity disagree"));
    }
    if unit != SERVICE {
        return Err(error("systemd job identity changed units"));
    }
    Ok(Some(result))
}

fn owned_job(path: OwnedObjectPath) -> Result<OwnedJob, InstallPlatformError> {
    let id = job_id(&path)?;
    Ok(OwnedJob { path, id })
}

fn require_job_result(result: &str, expected: &str) -> Result<(), InstallPlatformError> {
    if result == expected {
        Ok(())
    } else {
        Err(error(format!(
            "systemd job failed with exact result {result}"
        )))
    }
}

fn job_id(path: &OwnedObjectPath) -> Result<u32, InstallPlatformError> {
    let text = path.as_str();
    let id = text
        .strip_prefix("/org/freedesktop/systemd1/job/")
        .ok_or_else(|| error("systemd returned a foreign job object path"))?;
    let parsed = id
        .parse::<u32>()
        .map_err(|_| error("systemd returned a noncanonical job object path"))?;
    if id != parsed.to_string() {
        return Err(error("systemd returned a noncanonical job object path"));
    }
    Ok(parsed)
}

fn validate_runtime_path(path: &Path) -> Result<(), InstallPlatformError> {
    let text = path
        .to_str()
        .ok_or_else(|| error("XDG_RUNTIME_DIR is not UTF-8"))?;
    if !path.is_absolute()
        || text.len() > 4096
        || text.contains(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '.'))
        })
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(error(
            "XDG_RUNTIME_DIR is not a safe canonical absolute path",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn current_uid() -> Result<u32, InstallPlatformError> {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(io_error)
}

#[cfg(not(target_os = "linux"))]
fn current_uid() -> Result<u32, InstallPlatformError> {
    Err(error("native Linux systemd execution requires Linux"))
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::UnixListener;

    use futures_util::{FutureExt as _, StreamExt as _};
    use zbus::zvariant::OwnedObjectPath;

    use super::{
        InstallPlatformError, LinuxRuntimeManager, LinuxRuntimeSettlement, LinuxServiceIdentity,
        LinuxServiceWatch, LinuxSystemdConnection, OwnedJob, RuntimeJobBoundary, RuntimeJobOutcome,
        SERVICE, ServiceDeadlines, ServiceIdentity, ServiceSnapshot, SettleBoundary, WatchBoundary,
        fence_owned_job, owned_job, removed_job_result, settle_service, watch_service,
    };

    /// A runtime directory shaped like a user manager's: private, with a
    /// `systemd` directory holding the `private` socket.
    fn runtime_fixture() -> (tempfile::TempDir, u32) {
        let fixture = tempfile::tempdir().expect("fixture");
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700))
            .expect("runtime mode");
        fs::create_dir(fixture.path().join("systemd")).expect("manager directory");
        fs::set_permissions(
            fixture.path().join("systemd"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("manager directory mode");
        let uid = fs::metadata(fixture.path()).expect("metadata").uid();
        (fixture, uid)
    }

    #[test]
    fn user_manager_coordinate_is_exact_owned_and_private() {
        let (fixture, uid) = runtime_fixture();
        let _private =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        let connection = LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
            .expect("connection");
        let address = format!("unix:path={}/systemd/private", fixture.path().display());
        assert_eq!(
            connection.command_environment(),
            [
                ("XDG_RUNTIME_DIR", fixture.path().as_os_str()),
                ("DBUS_SESSION_BUS_ADDRESS", std::ffi::OsStr::new(&address)),
            ]
        );
        assert_eq!(connection.manager_address, address);

        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o755))
            .expect("unsafe runtime mode");
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid).is_err());
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700))
            .expect("private runtime mode");
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid + 1).is_err());
        assert!(
            LinuxSystemdConnection::from_runtime_directory(&fixture.path().join(".."), uid)
                .is_err()
        );
    }

    #[test]
    fn session_bus_alone_never_satisfies_the_manager_coordinate() {
        let (fixture, uid) = runtime_fixture();
        let _bus = UnixListener::bind(fixture.path().join("bus")).expect("session bus socket");
        let error = LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
            .expect_err("a session bus is not the user manager");
        assert!(
            error.to_string().contains("systemd/private is unavailable"),
            "{error}"
        );
    }

    #[test]
    fn manager_directory_must_be_a_real_directory_only_the_owner_writes() {
        let (fixture, uid) = runtime_fixture();
        let _private =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        fs::set_permissions(
            fixture.path().join("systemd"),
            fs::Permissions::from_mode(0o775),
        )
        .expect("group-writable manager directory");
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid).is_err());

        fs::set_permissions(
            fixture.path().join("systemd"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("owner-only manager directory");
        fs::rename(
            fixture.path().join("systemd"),
            fixture.path().join("elsewhere"),
        )
        .expect("move manager directory");
        std::os::unix::fs::symlink("elsewhere", fixture.path().join("systemd"))
            .expect("substituted manager directory");
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid).is_err());

        fs::remove_file(fixture.path().join("systemd")).expect("remove substitution");
        fs::create_dir(fixture.path().join("systemd")).expect("manager directory");
        fs::set_permissions(
            fixture.path().join("systemd"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("owner-only manager directory");
        fs::write(fixture.path().join("systemd/private"), b"not a socket")
            .expect("regular file in place of the socket");
        let error = LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
            .expect_err("a regular file is not the manager socket");
        assert!(
            error.to_string().contains("not an owned Unix socket"),
            "{error}"
        );
    }

    /// What the fake manager knows about `hypercolor.service`.
    #[derive(Debug)]
    struct FakeService {
        active_state: String,
        sub_state: String,
        job: Option<(u32, String)>,
        timeout_start_usec: u64,
        timeout_stop_usec: u64,
        calls: Vec<(String, String)>,
        /// The manager has garbage-collected the unit between `LoadUnit`
        /// and `ResetFailedUnit`, as a fresh install sees it.
        unloaded_for_reset: bool,
        invocation: Vec<u8>,
        main_pid: u32,
        /// The daemon crashes this long after a client connects, and
        /// `Restart=on-failure` brings it back under a new invocation.
        restart_after: Option<std::time::Duration>,
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.systemd1")]
    enum FakeManagerError {
        #[zbus(error)]
        ZBus(zbus::Error),
        NoSuchUnit(String),
    }

    type SharedService = std::sync::Arc<std::sync::Mutex<FakeService>>;

    const UNIT_PATH: &str = "/org/freedesktop/systemd1/unit/hypercolor_2eservice";

    /// Serve the manager peer to peer the way systemd serves its private
    /// socket: no bus daemon, no `Hello`, and signals without a sender.
    struct FakeManager {
        service: SharedService,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeManager {
        async fn start_unit(
            &self,
            name: String,
            mode: String,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            self.record("StartUnit", &name, &mode);
            {
                let mut service = self.service.lock().expect("fake service");
                "active".clone_into(&mut service.active_state);
                "running".clone_into(&mut service.sub_state);
            }
            job_removed(connection, &name).await
        }

        async fn stop_unit(
            &self,
            name: String,
            mode: String,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            self.record("StopUnit", &name, &mode);
            {
                let mut service = self.service.lock().expect("fake service");
                "inactive".clone_into(&mut service.active_state);
                "dead".clone_into(&mut service.sub_state);
            }
            job_removed(connection, &name).await
        }

        fn load_unit(&self, name: String) -> OwnedObjectPath {
            self.record("LoadUnit", &name, "");
            OwnedObjectPath::try_from(UNIT_PATH).expect("unit path")
        }

        fn get_unit(&self, name: String) -> OwnedObjectPath {
            self.record("GetUnit", &name, "");
            OwnedObjectPath::try_from(UNIT_PATH).expect("unit path")
        }

        fn reset_failed_unit(&self, name: String) -> Result<(), FakeManagerError> {
            self.record("ResetFailedUnit", &name, "");
            let mut service = self.service.lock().expect("fake service");
            if service.unloaded_for_reset {
                return Err(FakeManagerError::NoSuchUnit(format!(
                    "Unit {name} not loaded."
                )));
            }
            "inactive".clone_into(&mut service.active_state);
            "dead".clone_into(&mut service.sub_state);
            Ok(())
        }
    }

    impl FakeManager {
        fn record(&self, method: &str, name: &str, mode: &str) {
            assert_eq!(name, SERVICE);
            self.service
                .lock()
                .expect("fake service")
                .calls
                .push((method.to_owned(), mode.to_owned()));
        }
    }

    async fn job_removed(
        connection: &zbus::Connection,
        name: &str,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let unrelated = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/41")
            .expect("unrelated job path");
        let owned =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").expect("owned job path");
        for (id, path) in [(41_u32, unrelated), (42_u32, owned.clone())] {
            connection
                .emit_signal(
                    None::<&str>,
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.systemd1.Manager",
                    "JobRemoved",
                    &(id, path, name, "done"),
                )
                .await
                .map_err(|source| zbus::fdo::Error::Failed(source.to_string()))?;
        }
        Ok(owned)
    }

    struct FakeUnit {
        service: SharedService,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl FakeUnit {
        #[zbus(property)]
        fn active_state(&self) -> String {
            self.service
                .lock()
                .expect("fake service")
                .active_state
                .clone()
        }

        #[zbus(property)]
        fn sub_state(&self) -> String {
            self.service.lock().expect("fake service").sub_state.clone()
        }

        #[zbus(property, name = "InvocationID")]
        fn invocation_id(&self) -> Vec<u8> {
            self.service
                .lock()
                .expect("fake service")
                .invocation
                .clone()
        }

        #[zbus(property)]
        fn job(&self) -> (u32, OwnedObjectPath) {
            let service = self.service.lock().expect("fake service");
            let (id, path) = service.job.as_ref().map_or((0, "/".to_owned()), |(id, _)| {
                (*id, format!("/org/freedesktop/systemd1/job/{id}"))
            });
            (id, OwnedObjectPath::try_from(path).expect("job path"))
        }
    }

    struct FakeServiceTimeouts {
        service: SharedService,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Service")]
    impl FakeServiceTimeouts {
        #[zbus(property, name = "TimeoutStartUSec")]
        fn timeout_start_usec(&self) -> u64 {
            self.service
                .lock()
                .expect("fake service")
                .timeout_start_usec
        }

        #[zbus(property, name = "TimeoutStopUSec")]
        fn timeout_stop_usec(&self) -> u64 {
            self.service.lock().expect("fake service").timeout_stop_usec
        }

        #[zbus(property, name = "MainPID")]
        fn main_pid(&self) -> u32 {
            self.service.lock().expect("fake service").main_pid
        }
    }

    struct FakeJob {
        service: SharedService,
        observed: std::sync::Arc<tokio::sync::Notify>,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Job")]
    impl FakeJob {
        #[zbus(property)]
        fn job_type(&self) -> String {
            // The client has now seen the pending job, so it is waiting.
            self.observed.notify_one();
            self.service
                .lock()
                .expect("fake service")
                .job
                .as_ref()
                .map(|(_, kind)| kind.clone())
                .unwrap_or_default()
        }
    }

    /// Serve `connections` manager conversations on the private socket.
    ///
    /// When the service has a pending job, the fake finishes it only after
    /// the client has read its type, then announces the change the way
    /// systemd does: `PropertiesChanged` on the unit and `JobRemoved`.
    fn serve_fake_manager(
        listener: std::os::unix::net::UnixListener,
        service: SharedService,
        connections: usize,
    ) -> std::thread::JoinHandle<()> {
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::UnixListener::from_std(listener).expect("tokio listener");
                for _ in 0..connections {
                    let (stream, _) = listener.accept().await.expect("manager client");
                    let observed = std::sync::Arc::new(tokio::sync::Notify::new());
                    let connection = zbus::connection::Builder::unix_stream(stream)
                        .server(zbus::Guid::generate())
                        .expect("server guid")
                        .p2p()
                        .serve_at(
                            "/org/freedesktop/systemd1",
                            FakeManager {
                                service: std::sync::Arc::clone(&service),
                            },
                        )
                        .expect("serve manager")
                        .serve_at(
                            UNIT_PATH,
                            FakeUnit {
                                service: std::sync::Arc::clone(&service),
                            },
                        )
                        .expect("serve unit")
                        .serve_at(
                            UNIT_PATH,
                            FakeServiceTimeouts {
                                service: std::sync::Arc::clone(&service),
                            },
                        )
                        .expect("serve service timeouts")
                        .serve_at(
                            "/org/freedesktop/systemd1/job/7",
                            FakeJob {
                                service: std::sync::Arc::clone(&service),
                                observed: std::sync::Arc::clone(&observed),
                            },
                        )
                        .expect("serve job")
                        .build()
                        .await
                        .expect("peer-to-peer manager connection");
                    let finisher = {
                        let connection = connection.clone();
                        let service = std::sync::Arc::clone(&service);
                        async move {
                            observed.notified().await;
                            {
                                let mut service = service.lock().expect("fake service");
                                service.job = None;
                                "active".clone_into(&mut service.active_state);
                                "running".clone_into(&mut service.sub_state);
                            }
                            let unit = connection
                                .object_server()
                                .interface::<_, FakeUnit>(UNIT_PATH)
                                .await
                                .expect("unit interface");
                            unit.get()
                                .await
                                .active_state_changed(unit.signal_emitter())
                                .await
                                .expect("announce state");
                            connection
                                .emit_signal(
                                    None::<&str>,
                                    "/org/freedesktop/systemd1",
                                    "org.freedesktop.systemd1.Manager",
                                    "JobRemoved",
                                    &(
                                        7_u32,
                                        OwnedObjectPath::try_from(
                                            "/org/freedesktop/systemd1/job/7",
                                        )
                                        .expect("job path"),
                                        SERVICE,
                                        "done",
                                    ),
                                )
                                .await
                                .expect("announce job");
                        }
                    };
                    let restarter = {
                        let connection = connection.clone();
                        let service = std::sync::Arc::clone(&service);
                        async move {
                            let delay = service.lock().expect("fake service").restart_after;
                            let Some(delay) = delay else {
                                return;
                            };
                            tokio::time::sleep(delay).await;
                            {
                                let mut service = service.lock().expect("fake service");
                                service.invocation.iter_mut().for_each(|byte| *byte ^= 0xff);
                                service.main_pid += 1;
                            }
                            // Announce the change by invalidating the
                            // property, which the watch must read again.
                            connection
                                .emit_signal(
                                    None::<&str>,
                                    UNIT_PATH,
                                    "org.freedesktop.DBus.Properties",
                                    "PropertiesChanged",
                                    &(
                                        "org.freedesktop.systemd1.Unit",
                                        std::collections::HashMap::<
                                            &str,
                                            zbus::zvariant::Value<'_>,
                                        >::new(),
                                        vec!["InvocationID"],
                                    ),
                                )
                                .await
                                .expect("announce the new invocation");
                        }
                    };
                    // Keep serving until the client hangs up.
                    let mut messages = zbus::MessageStream::from(&connection);
                    let serve = async { while messages.next().await.is_some() {} };
                    tokio::select! {
                        () = serve => {}
                        () = async { finisher.await; std::future::pending::<()>().await } => {}
                        () = async { restarter.await; std::future::pending::<()>().await } => {}
                    }
                }
            });
        })
    }

    fn fake_service(active_state: &str, sub_state: &str) -> SharedService {
        std::sync::Arc::new(std::sync::Mutex::new(FakeService {
            active_state: active_state.to_owned(),
            sub_state: sub_state.to_owned(),
            job: None,
            timeout_start_usec: 90_000_000,
            timeout_stop_usec: 90_000_000,
            calls: Vec::new(),
            unloaded_for_reset: false,
            invocation: vec![0x11; 16],
            main_pid: 4242,
            restart_after: None,
        }))
    }

    #[test]
    fn runtime_jobs_run_over_the_private_socket_without_a_session_bus() {
        let (fixture, uid) = runtime_fixture();
        let listener =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        let service = fake_service("inactive", "dead");
        let server = serve_fake_manager(listener, std::sync::Arc::clone(&service), 2);

        let connection = LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
            .expect("private manager coordinate");
        let manager = LinuxRuntimeManager::new(connection);
        assert_eq!(
            manager.set_runtime(true).expect("start job"),
            RuntimeJobOutcome::Done
        );
        assert_eq!(
            manager.set_runtime(false).expect("stop job"),
            RuntimeJobOutcome::Done
        );
        server.join().expect("fake manager thread");
        assert_eq!(
            service.lock().expect("fake service").calls,
            [
                ("LoadUnit".to_owned(), String::new()),
                ("ResetFailedUnit".to_owned(), String::new()),
                ("StartUnit".to_owned(), "fail".to_owned()),
                ("LoadUnit".to_owned(), String::new()),
                ("StopUnit".to_owned(), "replace".to_owned()),
            ]
        );
    }

    #[test]
    fn a_start_proceeds_when_the_unit_has_nothing_loaded_to_reset() {
        // Observed on a fresh Ubuntu 24.04 install: the unit LoadUnit just
        // loaded is collected again before ResetFailedUnit reaches it.
        let (fixture, uid) = runtime_fixture();
        let listener =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        let service = fake_service("inactive", "dead");
        service.lock().expect("fake service").unloaded_for_reset = true;
        let server = serve_fake_manager(listener, std::sync::Arc::clone(&service), 1);
        let manager = LinuxRuntimeManager::new(
            LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
                .expect("private manager coordinate"),
        );
        assert_eq!(
            manager
                .set_runtime(true)
                .expect("start after a refused reset"),
            RuntimeJobOutcome::Done
        );
        server.join().expect("fake manager thread");
        assert_eq!(
            service.lock().expect("fake service").calls,
            [
                ("LoadUnit".to_owned(), String::new()),
                ("ResetFailedUnit".to_owned(), String::new()),
                ("StartUnit".to_owned(), "fail".to_owned()),
            ]
        );
    }

    #[test]
    fn every_start_resets_the_service_first_and_a_stop_replaces_a_queued_start() {
        let (fixture, uid) = runtime_fixture();
        let listener =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        let service = fake_service("failed", "failed");
        let server = serve_fake_manager(listener, std::sync::Arc::clone(&service), 2);
        let manager = LinuxRuntimeManager::new(
            LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
                .expect("private manager coordinate"),
        );
        manager.set_runtime(false).expect("stop a failed service");
        {
            let mut service = service.lock().expect("fake service");
            "failed".clone_into(&mut service.active_state);
            "failed".clone_into(&mut service.sub_state);
        }
        manager.set_runtime(true).expect("start after a crash loop");
        server.join().expect("fake manager thread");
        assert_eq!(
            service.lock().expect("fake service").calls,
            [
                ("LoadUnit".to_owned(), String::new()),
                ("StopUnit".to_owned(), "replace".to_owned()),
                ("LoadUnit".to_owned(), String::new()),
                ("ResetFailedUnit".to_owned(), String::new()),
                ("StartUnit".to_owned(), "fail".to_owned()),
            ]
        );
    }

    #[test]
    fn settle_waits_for_a_queued_start_job_over_the_private_socket() {
        let (fixture, uid) = runtime_fixture();
        let listener =
            UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
        // A login queues the service's start job while it still reads
        // inactive; settling must wait for it rather than call it stopped.
        let service = fake_service("inactive", "dead");
        service.lock().expect("fake service").job = Some((7, "start".to_owned()));
        let server = serve_fake_manager(listener, std::sync::Arc::clone(&service), 1);
        let manager = LinuxRuntimeManager::new(
            LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
                .expect("private manager coordinate"),
        );
        assert_eq!(
            manager.settle().expect("settle"),
            LinuxRuntimeSettlement::Settled
        );
        server.join().expect("fake manager thread");
        let service = service.lock().expect("fake service");
        assert_eq!(
            (service.active_state.as_str(), service.sub_state.as_str()),
            ("active", "running")
        );
        assert_eq!(service.calls, [("GetUnit".to_owned(), String::new())]);
    }

    #[test]
    fn owned_job_outliving_first_wait_is_cancelled_and_terminally_fenced() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut boundary = FakeBoundary {
            waits: VecDeque::from([None, Some("canceled".to_owned())]),
            cancelled: Vec::new(),
            timeouts: Vec::new(),
        };
        let job = OwnedJob {
            path: OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").expect("job path"),
            id: 42,
        };
        let deadlines = ServiceDeadlines::from_unit(90_000_000, 20_000_000);
        assert_eq!(
            runtime
                .block_on(fence_owned_job(&mut boundary, &job, deadlines.start))
                .expect("cancelled terminal job"),
            RuntimeJobOutcome::Cancelled
        );
        assert_eq!(boundary.cancelled, [42]);
        assert!(boundary.waits.is_empty());
        // The job gets the unit's own start timeout plus grace before the
        // cancel, and the cancel gets its short fixed fence.
        assert_eq!(
            boundary.timeouts,
            [
                std::time::Duration::from_secs(100),
                std::time::Duration::from_secs(5)
            ]
        );
    }

    #[test]
    fn returned_job_path_ignores_an_interleaved_same_unit_job() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let unrelated =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/41").expect("job path");
        let returned =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").expect("job path");
        let job = owned_job(returned.clone()).expect("returned job authority");
        let mut boundary = InterleavedBoundary {
            events: VecDeque::from([
                (unrelated, "done".to_owned()),
                (returned, "done".to_owned()),
            ]),
            observed: Vec::new(),
        };
        assert_eq!(
            runtime
                .block_on(fence_owned_job(
                    &mut boundary,
                    &job,
                    std::time::Duration::from_secs(1)
                ))
                .expect("exact returned job"),
            RuntimeJobOutcome::Done
        );
        assert_eq!(boundary.observed, [42]);
        assert!(boundary.events.is_empty());
    }

    #[test]
    fn exact_job_path_with_the_wrong_numeric_id_fails_closed() {
        let unrelated =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/41").expect("job path");
        let returned =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").expect("job path");
        let job = owned_job(returned.clone()).expect("returned job authority");
        let mut events = [
            (41, unrelated, SERVICE, "done".to_owned()),
            (41, returned.clone(), SERVICE, "done".to_owned()),
            (42, returned, SERVICE, "done".to_owned()),
        ]
        .into_iter();
        let (id, path, unit, result) = events.next().expect("unrelated event");
        assert_eq!(
            removed_job_result(id, &path, unit, result, &job).expect("unrelated event"),
            None
        );
        let (id, path, unit, result) = events.next().expect("conflicting event");
        let error = removed_job_result(id, &path, unit, result, &job)
            .expect_err("wrong numeric identity must fail closed");

        assert!(error.to_string().contains("numeric identity disagree"));
        assert_eq!(events.count(), 1, "valid pair remains unconsumed");
    }

    struct FakeBoundary {
        waits: VecDeque<Option<String>>,
        cancelled: Vec<u32>,
        timeouts: Vec<std::time::Duration>,
    }

    struct InterleavedBoundary {
        events: VecDeque<(OwnedObjectPath, String)>,
        observed: Vec<u32>,
    }

    impl RuntimeJobBoundary for InterleavedBoundary {
        fn wait<'a>(
            &'a mut self,
            job: &'a OwnedJob,
            _timeout: std::time::Duration,
        ) -> futures_util::future::LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>>
        {
            self.observed.push(job.id);
            let result = self
                .events
                .drain(..)
                .find_map(|(path, result)| (path == job.path).then_some(result));
            async move { Ok(result) }.boxed_local()
        }

        fn cancel<'a>(
            &'a mut self,
            _job: &'a OwnedJob,
        ) -> futures_util::future::LocalBoxFuture<'a, Result<(), InstallPlatformError>> {
            async { Err(super::error("unexpected cancellation")) }.boxed_local()
        }
    }

    impl RuntimeJobBoundary for FakeBoundary {
        fn wait<'a>(
            &'a mut self,
            _job: &'a OwnedJob,
            timeout: std::time::Duration,
        ) -> futures_util::future::LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>>
        {
            self.timeouts.push(timeout);
            let result = self.waits.pop_front().expect("scripted wait");
            async move { Ok(result) }.boxed_local()
        }

        fn cancel<'a>(
            &'a mut self,
            job: &'a OwnedJob,
        ) -> futures_util::future::LocalBoxFuture<'a, Result<(), InstallPlatformError>> {
            self.cancelled.push(job.id);
            async { Ok(()) }.boxed_local()
        }
    }

    #[test]
    fn job_deadlines_follow_the_unit_timeouts_within_a_fixed_bound() {
        let secs = std::time::Duration::from_secs;
        assert_eq!(
            ServiceDeadlines::from_unit(90_000_000, 45_000_000),
            ServiceDeadlines {
                start: secs(100),
                stop: secs(55)
            }
        );
        // A slow unit keeps its own longer timeout up to the bound.
        assert_eq!(ServiceDeadlines::from_unit(150_000_000, 0).start, secs(160));
        // Zero and infinity both mean "no timeout" to systemd; above the
        // bound, the fence still ends the wait at three minutes.
        for usec in [0, u64::MAX, 175_000_000, 3_600_000_000] {
            assert_eq!(ServiceDeadlines::from_unit(usec, usec).start, secs(180));
            assert_eq!(ServiceDeadlines::from_unit(usec, usec).stop, secs(180));
        }
    }

    /// Scripted snapshots; `changed` reports whether another one exists
    /// before the deadline.
    struct ScriptedService {
        snapshots: VecDeque<ServiceSnapshot>,
        deadline_reads: usize,
        waits: Vec<tokio::time::Instant>,
    }

    fn snapshot(active: &str, sub: &str, job: Option<&str>) -> ServiceSnapshot {
        ServiceSnapshot {
            active_state: active.to_owned(),
            sub_state: sub.to_owned(),
            job: job.map(str::to_owned),
        }
    }

    impl SettleBoundary for ScriptedService {
        fn snapshot(
            &mut self,
        ) -> futures_util::future::LocalBoxFuture<'_, Result<ServiceSnapshot, InstallPlatformError>>
        {
            let next = self.snapshots.pop_front().expect("scripted snapshot");
            async move { Ok(next) }.boxed_local()
        }

        fn deadlines(
            &mut self,
        ) -> futures_util::future::LocalBoxFuture<'_, Result<ServiceDeadlines, InstallPlatformError>>
        {
            self.deadline_reads += 1;
            async { Ok(ServiceDeadlines::from_unit(20_000_000, 30_000_000)) }.boxed_local()
        }

        fn changed(
            &mut self,
            deadline: tokio::time::Instant,
        ) -> futures_util::future::LocalBoxFuture<'_, Result<bool, InstallPlatformError>> {
            // An exhausted script stands for a deadline that passed first.
            self.waits.push(deadline);
            let more = !self.snapshots.is_empty();
            async move { Ok(more) }.boxed_local()
        }
    }

    fn settle(snapshots: Vec<ServiceSnapshot>) -> (LinuxRuntimeSettlement, ScriptedService) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut service = ScriptedService {
            snapshots: snapshots.into(),
            deadline_reads: 0,
            waits: Vec::new(),
        };
        let settled = runtime
            .block_on(settle_service(&mut service))
            .expect("settle");
        (settled, service)
    }

    #[test]
    fn steady_running_stopped_and_failed_services_settle_without_waiting() {
        for (active, sub) in [
            ("active", "running"),
            ("inactive", "dead"),
            ("failed", "failed"),
        ] {
            let (settled, service) = settle(vec![snapshot(active, sub, None)]);
            assert_eq!(settled, LinuxRuntimeSettlement::Settled);
            assert!(service.waits.is_empty());
            assert_eq!(service.deadline_reads, 0);
        }
    }

    #[test]
    fn queued_and_running_jobs_settle_when_they_finish() {
        let (settled, service) = settle(vec![
            snapshot("inactive", "dead", Some("start")),
            snapshot("activating", "start", Some("start")),
            snapshot("active", "running", None),
        ]);
        assert_eq!(settled, LinuxRuntimeSettlement::Settled);
        assert_eq!(service.deadline_reads, 1);
        assert_eq!(service.waits.len(), 2);
        // Every wait shares the first job's deadline, so a chain of jobs
        // cannot extend the settle without bound.
        assert_eq!(service.waits[0], service.waits[1]);

        let (settled, _) = settle(vec![
            snapshot("deactivating", "stop-sigterm", None),
            snapshot("inactive", "dead", None),
        ]);
        assert_eq!(settled, LinuxRuntimeSettlement::Settled);
    }

    #[test]
    fn a_job_outliving_its_unit_deadline_is_reported_unsettled() {
        let (settled, service) = settle(vec![snapshot("activating", "start", Some("start"))]);
        assert_eq!(settled, LinuxRuntimeSettlement::Unsettled);
        assert_eq!(service.waits.len(), 1);
    }

    #[test]
    fn a_service_waiting_to_restart_is_unsettled_without_waiting_for_it() {
        for (active, sub) in [
            ("activating", "auto-restart"),
            ("inactive", "dead-before-auto-restart"),
            ("failed", "failed-before-auto-restart"),
        ] {
            let (settled, service) = settle(vec![snapshot(active, sub, None)]);
            assert_eq!(settled, LinuxRuntimeSettlement::Unsettled, "{active}/{sub}");
            assert!(service.waits.is_empty());
        }
        // A crash after the start job completes lands in auto-restart.
        let (settled, _) = settle(vec![
            snapshot("activating", "start", Some("start")),
            snapshot("activating", "auto-restart", None),
        ]);
        assert_eq!(settled, LinuxRuntimeSettlement::Unsettled);
    }

    const WINDOW: std::time::Duration = std::time::Duration::from_millis(300);
    const LATE: std::time::Duration = std::time::Duration::from_millis(267);

    fn proven() -> LinuxServiceIdentity {
        LinuxServiceIdentity {
            invocation_id: "11".repeat(16),
            main_pid: 4242,
        }
    }

    fn identity(active: &str, sub: &str, invocation: &str, pid: u32) -> ServiceIdentity {
        ServiceIdentity {
            active_state: active.to_owned(),
            sub_state: sub.to_owned(),
            invocation_id: invocation.to_owned(),
            main_pid: pid,
        }
    }

    /// Identities to read in order, and when the manager signals a change.
    struct ScriptedWatch {
        identities: VecDeque<ServiceIdentity>,
        changes: VecDeque<std::time::Duration>,
        started: tokio::time::Instant,
    }

    impl WatchBoundary for ScriptedWatch {
        fn identity(
            &mut self,
        ) -> futures_util::future::LocalBoxFuture<'_, Result<ServiceIdentity, InstallPlatformError>>
        {
            let identity = self.identities.pop_front().expect("scripted identity");
            async move { Ok(identity) }.boxed_local()
        }

        fn changed(
            &mut self,
            deadline: tokio::time::Instant,
        ) -> futures_util::future::LocalBoxFuture<'_, Result<bool, InstallPlatformError>> {
            let change = self.changes.pop_front().map(|offset| self.started + offset);
            async move {
                match change {
                    Some(at) if at < deadline => {
                        tokio::time::sleep_until(at).await;
                        Ok(true)
                    }
                    _ => {
                        tokio::time::sleep_until(deadline).await;
                        Ok(false)
                    }
                }
            }
            .boxed_local()
        }
    }

    fn watch(
        identities: Vec<ServiceIdentity>,
        changes: Vec<std::time::Duration>,
    ) -> (LinuxServiceWatch, std::time::Duration) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut boundary = ScriptedWatch {
                identities: identities.into(),
                changes: changes.into(),
                started: tokio::time::Instant::now(),
            };
            let started = std::time::Instant::now();
            let outcome = watch_service(&mut boundary, &proven(), WINDOW)
                .await
                .expect("watch");
            (outcome, started.elapsed())
        })
    }

    #[test]
    fn a_steady_service_holds_for_the_whole_window() {
        let running = identity("active", "running", &"11".repeat(16), 4242);
        let (outcome, elapsed) = watch(vec![running], Vec::new());
        assert_eq!(outcome, LinuxServiceWatch::Steady);
        assert!(elapsed >= WINDOW, "returned after {elapsed:?}");
    }

    #[test]
    fn a_restart_late_in_the_window_ends_it_at_once() {
        let running = identity("active", "running", &"11".repeat(16), 4242);
        let restarted = identity("active", "running", &"ee".repeat(16), 4243);
        let (outcome, elapsed) = watch(vec![running, restarted], vec![LATE]);
        let LinuxServiceWatch::Changed { after, observed } = outcome else {
            panic!("a restart must end the window: {outcome:?}");
        };
        assert!(after >= LATE && after < WINDOW, "changed after {after:?}");
        assert!(elapsed < WINDOW, "the watch returned after {elapsed:?}");
        assert!(observed.contains(&"ee".repeat(16)), "{observed}");
    }

    #[test]
    fn a_signal_that_leaves_the_identity_unchanged_keeps_watching() {
        let running = identity("active", "running", &"11".repeat(16), 4242);
        let (outcome, elapsed) = watch(
            vec![running.clone(), running],
            vec![std::time::Duration::from_millis(100)],
        );
        assert_eq!(outcome, LinuxServiceWatch::Steady);
        assert!(elapsed >= WINDOW, "returned after {elapsed:?}");
    }

    #[test]
    fn a_service_that_already_changed_fails_the_window_before_it_starts() {
        for changed in [
            identity("activating", "auto-restart", "", 0),
            identity("failed", "failed", &"11".repeat(16), 0),
            identity("active", "running", &"11".repeat(16), 9999),
        ] {
            let (outcome, elapsed) = watch(vec![changed.clone()], Vec::new());
            assert!(
                matches!(outcome, LinuxServiceWatch::Changed { after, .. } if after < WINDOW),
                "{changed:?}: {outcome:?}"
            );
            assert!(elapsed < WINDOW);
        }
    }

    #[test]
    fn probation_watches_the_real_identity_over_the_private_socket() {
        for restart_after in [None, Some(LATE)] {
            let (fixture, uid) = runtime_fixture();
            let listener =
                UnixListener::bind(fixture.path().join("systemd/private")).expect("private socket");
            let service = fake_service("active", "running");
            service.lock().expect("fake service").restart_after = restart_after;
            let server = serve_fake_manager(listener, std::sync::Arc::clone(&service), 1);
            let manager = LinuxRuntimeManager::new(
                LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
                    .expect("private manager coordinate"),
            );
            let started = std::time::Instant::now();
            let outcome = manager.watch(proven(), WINDOW).expect("watch");
            let elapsed = started.elapsed();
            server.join().expect("fake manager thread");
            if restart_after.is_none() {
                assert_eq!(outcome, LinuxServiceWatch::Steady);
                assert!(elapsed >= WINDOW, "returned after {elapsed:?}");
            } else {
                assert!(
                    matches!(outcome, LinuxServiceWatch::Changed { .. }),
                    "{outcome:?}"
                );
                assert!(elapsed < WINDOW, "returned after {elapsed:?}");
            }
            assert_eq!(
                service.lock().expect("fake service").calls,
                [("GetUnit".to_owned(), String::new())],
                "the watch only reads; it never starts, stops or reloads"
            );
        }
    }
}
