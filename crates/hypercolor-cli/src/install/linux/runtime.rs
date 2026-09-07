use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::future::LocalBoxFuture;
use futures_util::{FutureExt as _, StreamExt as _};
use zbus::zvariant::OwnedObjectPath;

use super::super::InstallPlatformError;
use super::model::error;

const SYSTEMD_DESTINATION: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const SERVICE: &str = "hypercolor.service";
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);
const JOB_TIMEOUT: Duration = Duration::from_secs(10);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxSystemdConnection {
    runtime_directory: PathBuf,
    bus_address: String,
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
    /// # Errors
    ///
    /// Returns an error for a noncanonical path, unsafe mode or owner, or a
    /// missing, foreign, or non-socket session bus endpoint.
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
        let bus = runtime_directory.join("bus");
        let bus_metadata = fs::symlink_metadata(&bus).map_err(io_error)?;
        if !bus_metadata.file_type().is_socket() || bus_metadata.uid() != expected_uid {
            return Err(error("XDG_RUNTIME_DIR bus is not an owned Unix socket"));
        }
        let runtime_text = runtime_directory
            .to_str()
            .ok_or_else(|| error("XDG_RUNTIME_DIR is not UTF-8"))?;
        Ok(Self {
            runtime_directory: runtime_directory.to_owned(),
            bus_address: format!("unix:path={runtime_text}/bus"),
        })
    }

    #[must_use]
    pub fn command_environment(&self) -> (&'static str, &OsStr) {
        ("XDG_RUNTIME_DIR", self.runtime_directory.as_os_str())
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
        let address = self.connection.bus_address.clone();
        let worker = std::thread::spawn(move || run_runtime_job(&address, running));
        worker
            .join()
            .map_err(|_| error("systemd D-Bus job worker panicked"))?
    }
}

fn run_runtime_job(
    address: &str,
    running: bool,
) -> Result<RuntimeJobOutcome, InstallPlatformError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_error)?;
    runtime.block_on(async move {
        let connection = tokio::time::timeout(
            METHOD_TIMEOUT,
            zbus::connection::Builder::address(address)
                .map_err(zbus_error)?
                .method_timeout(METHOD_TIMEOUT)
                .build(),
        )
        .await
        .map_err(|_| error("user systemd D-Bus connection exceeded its deadline"))?
        .map_err(zbus_error)?;
        let proxy = zbus::Proxy::new(
            &connection,
            SYSTEMD_DESTINATION,
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
        )
        .await
        .map_err(zbus_error)?;
        let mut removed = proxy
            .receive_signal_with_args("JobRemoved", &[(2, SERVICE)])
            .await
            .map_err(zbus_error)?;
        let method = if running { "StartUnit" } else { "StopUnit" };
        let job_path: OwnedObjectPath = proxy
            .call(method, &(SERVICE, "fail"))
            .await
            .map_err(zbus_error)?;
        let job = owned_job(job_path)?;
        let mut boundary = ZbusJobBoundary {
            proxy: &proxy,
            removed: &mut removed,
        };
        fence_owned_job(&mut boundary, &job).await
    })
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

struct ZbusJobBoundary<'a, 'proxy> {
    proxy: &'a zbus::Proxy<'proxy>,
    removed: &'a mut zbus::proxy::SignalStream<'proxy>,
}

impl RuntimeJobBoundary for ZbusJobBoundary<'_, '_> {
    fn wait<'a>(
        &'a mut self,
        job: &'a OwnedJob,
        timeout: Duration,
    ) -> LocalBoxFuture<'a, Result<Option<String>, InstallPlatformError>> {
        async move {
            match tokio::time::timeout(timeout, wait_for_job(self.removed, job)).await {
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
            self.proxy
                .call::<_, _, ()>("CancelJob", &(job.id,))
                .await
                .map_err(zbus_error)
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
    removed: &mut zbus::proxy::SignalStream<'_>,
    expected: &OwnedJob,
) -> Result<String, InstallPlatformError> {
    while let Some(message) = removed.next().await {
        let (id, path, unit, result): (u32, OwnedObjectPath, String, String) =
            message.body().deserialize().map_err(zbus_error)?;
        if let Some(result) = removed_job_result(id, &path, &unit, result, expected)? {
            return Ok(result);
        }
    }
    Err(error(
        "systemd JobRemoved stream ended before the owned job",
    ))
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

fn zbus_error(source: zbus::Error) -> InstallPlatformError {
    error(source.to_string())
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

    use futures_util::FutureExt as _;
    use zbus::zvariant::OwnedObjectPath;

    use super::{
        InstallPlatformError, LinuxSystemdConnection, OwnedJob, RuntimeJobBoundary,
        RuntimeJobOutcome, SERVICE, fence_owned_job, owned_job, removed_job_result,
    };

    #[test]
    fn user_manager_coordinate_is_exact_owned_and_private() {
        let fixture = tempfile::tempdir().expect("fixture");
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700))
            .expect("runtime mode");
        let _bus = UnixListener::bind(fixture.path().join("bus")).expect("bus socket");
        let uid = fs::metadata(fixture.path()).expect("metadata").uid();
        let connection = LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid)
            .expect("connection");
        let (name, value) = connection.command_environment();
        assert_eq!(name, "XDG_RUNTIME_DIR");
        assert_eq!(value, fixture.path().as_os_str());

        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o755))
            .expect("unsafe runtime mode");
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid).is_err());
        assert!(LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid + 1).is_err());
        assert!(
            LinuxSystemdConnection::from_runtime_directory(&fixture.path().join(".."), uid)
                .is_err()
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
