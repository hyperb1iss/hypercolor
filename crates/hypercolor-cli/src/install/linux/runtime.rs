use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::FutureExt as _;
use futures_util::future::LocalBoxFuture;
use zbus::zvariant::OwnedObjectPath;

use super::super::InstallPlatformError;
use super::manager_bus::{ManagerBus, ManagerCallError};
use super::model::error;

const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
/// The user manager's own socket, relative to `XDG_RUNTIME_DIR`.
///
/// It serves the manager's D-Bus API peer to peer, so reaching systemd never
/// requires the session bus or anything else that listens on it.
const PRIVATE_SOCKET: &str = "systemd/private";
const SERVICE: &str = "hypercolor.service";
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);
const JOB_TIMEOUT: Duration = Duration::from_secs(10);
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

impl LinuxRuntimeManager {
    pub(super) fn new(connection: LinuxSystemdConnection) -> Self {
        Self { connection }
    }

    pub(super) fn set_runtime(
        &self,
        running: bool,
    ) -> Result<RuntimeJobOutcome, InstallPlatformError> {
        let connection = self.connection.clone();
        let worker = std::thread::spawn(move || run_runtime_job(&connection, running));
        worker
            .join()
            .map_err(|_| error("systemd D-Bus job worker panicked"))?
    }
}

fn run_runtime_job(
    manager: &LinuxSystemdConnection,
    running: bool,
) -> Result<RuntimeJobOutcome, InstallPlatformError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_error)?;
    runtime.block_on(async move {
        let mut bus = connect_manager(manager).await?;
        let method = if running { "StartUnit" } else { "StopUnit" };
        // A direct manager connection receives every manager signal without
        // a bus match or `Subscribe`; the client keeps any that arrive
        // before the reply, so the job's JobRemoved cannot be missed.
        let job_path = within_method_deadline(bus.call::<_, OwnedObjectPath>(
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            method,
            &(SERVICE, "fail"),
        ))
        .await?
        .map_err(ManagerCallError::into_platform)?;
        let job = owned_job(job_path)?;
        let mut boundary = BusJobBoundary { bus: &mut bus };
        fence_owned_job(&mut boundary, &job).await
    })
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
        async move {
            within_method_deadline(self.bus.call::<_, ()>(
                SYSTEMD_PATH,
                SYSTEMD_MANAGER,
                "CancelJob",
                &(job.id,),
            ))
            .await?
            .map_err(ManagerCallError::into_platform)
        }
        .boxed_local()
    }
}

async fn fence_owned_job(
    boundary: &mut impl RuntimeJobBoundary,
    job: &OwnedJob,
) -> Result<RuntimeJobOutcome, InstallPlatformError> {
    if let Some(result) = boundary.wait(job, JOB_TIMEOUT).await? {
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
        InstallPlatformError, LinuxRuntimeManager, LinuxSystemdConnection, OwnedJob,
        RuntimeJobBoundary, RuntimeJobOutcome, SERVICE, fence_owned_job, owned_job,
        removed_job_result,
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

    /// Serve `StartUnit` and `StopUnit` peer to peer with an independent
    /// D-Bus implementation: no bus daemon and no `Hello`.
    struct FakeManager {
        calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeManager {
        async fn start_unit(
            &self,
            name: String,
            mode: String,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            self.job("StartUnit", name, mode, connection).await
        }

        async fn stop_unit(
            &self,
            name: String,
            mode: String,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            self.job("StopUnit", name, mode, connection).await
        }
    }

    impl FakeManager {
        async fn job(
            &self,
            method: &str,
            name: String,
            mode: String,
            connection: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            self.calls.lock().expect("fake manager calls").push((
                method.to_owned(),
                name.clone(),
                mode,
            ));
            let unrelated = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/41")
                .expect("unrelated job path");
            let owned = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42")
                .expect("owned job path");
            for (id, path) in [(41_u32, unrelated), (42_u32, owned.clone())] {
                connection
                    .emit_signal(
                        None::<&str>,
                        "/org/freedesktop/systemd1",
                        "org.freedesktop.systemd1.Manager",
                        "JobRemoved",
                        &(id, path, name.as_str(), "done"),
                    )
                    .await
                    .map_err(|source| zbus::fdo::Error::Failed(source.to_string()))?;
            }
            Ok(owned)
        }
    }

    #[test]
    fn runtime_jobs_run_over_the_private_socket_without_a_session_bus() {
        let (fixture, uid) = runtime_fixture();
        let socket = fixture.path().join("systemd/private");
        let listener = UnixListener::bind(&socket).expect("private socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let served = std::sync::Arc::clone(&calls);
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::UnixListener::from_std(listener).expect("tokio listener");
                for _ in 0..2 {
                    let (stream, _) = listener.accept().await.expect("manager client");
                    let connection = zbus::connection::Builder::unix_stream(stream)
                        .server(zbus::Guid::generate())
                        .expect("server guid")
                        .p2p()
                        .serve_at(
                            "/org/freedesktop/systemd1",
                            FakeManager {
                                calls: std::sync::Arc::clone(&served),
                            },
                        )
                        .expect("serve manager")
                        .build()
                        .await
                        .expect("peer-to-peer manager connection");
                    // Keep serving until the client hangs up.
                    let mut messages = zbus::MessageStream::from(&connection);
                    while messages.next().await.is_some() {}
                }
            });
        });

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
            *calls.lock().expect("recorded calls"),
            [
                (
                    "StartUnit".to_owned(),
                    SERVICE.to_owned(),
                    "fail".to_owned()
                ),
                ("StopUnit".to_owned(), SERVICE.to_owned(), "fail".to_owned()),
            ]
        );
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
        };
        let job = OwnedJob {
            path: OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").expect("job path"),
            id: 42,
        };
        assert_eq!(
            runtime
                .block_on(fence_owned_job(&mut boundary, &job))
                .expect("cancelled terminal job"),
            RuntimeJobOutcome::Cancelled
        );
        assert_eq!(boundary.cancelled, [42]);
        assert!(boundary.waits.is_empty());
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
                .block_on(fence_owned_job(&mut boundary, &job))
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
            _timeout: std::time::Duration,
        ) -> futures_util::future::LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>>
        {
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
}
