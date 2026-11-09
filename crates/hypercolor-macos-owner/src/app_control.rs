//! The desktop app's part in an update of its own bundle.
//!
//! An update swaps the bundle the app runs from, so the app leaves first and
//! comes back after. Three things in the installation's update state root
//! carry that, and the root's owner-only directory bounds who reaches them:
//!
//! - `app.sock`, the running app's control socket;
//! - `relaunch.json`, the exact bundle to open again, recorded before the app
//!   is asked to leave, so a crash at any later point still brings it back;
//! - `pending`, present while a transaction is open; an app that starts then
//!   neither starts nor adopts a daemon.
//!
//! Everything but opening a bundle is plain unix, so the protocol runs
//! anywhere; the macOS calls sit behind [`MacosAppHost`].

use std::fs;
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use hypercolor_platform_fs::is_macos_translocated_path;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::MacosAppStartupHold;
use crate::store_io::write_json_atomic;
use crate::{MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosOwnerExecutionError};

/// Present in the update state root while an update transaction is open.
pub const MACOS_UPDATE_PENDING_FILE_NAME: &str = "pending";
/// The running app's control socket in the update state root.
pub const MACOS_APP_CONTROL_SOCKET_NAME: &str = "app.sock";
/// The bundle to open again once a transaction is terminal.
pub const MACOS_APP_RELAUNCH_FILE_NAME: &str = "relaunch.json";

const RELAUNCH_RECORD: &str = "macOS app relaunch record";
const MAX_CONTROL_LINE_BYTES: u64 = 8 * 1024;
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// The update state root below an account's Application Support directory.
#[must_use]
pub fn macos_update_state_root(application_support: &Path) -> PathBuf {
    application_support.join("Hypercolor/update")
}

/// The update state root of the account running this process.
#[must_use]
pub fn macos_user_update_state_root() -> Option<PathBuf> {
    hypercolor_platform_fs::user_dirs::data_base_dir().map(|base| macos_update_state_root(&base))
}

/// The `.app` an executable belongs to, when it runs from one.
#[must_use]
pub fn macos_app_bundle_of(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    let bundle = contents.parent()?;
    (executable.parent()?.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Why an app that just started must not touch a daemon, if it must not.
///
/// A translocated app can never proceed, since nothing it recorded would
/// outlive it, so that wins over an update, which ends on its own, and holds
/// whether or not the account's update state root can be resolved.
#[must_use]
pub fn macos_app_startup_hold(
    executable: &Path,
    update_root: Option<&Path>,
) -> Option<MacosAppStartupHold> {
    if is_macos_translocated_path(executable) {
        return Some(MacosAppStartupHold::MoveToApplications);
    }
    update_root
        .is_some_and(|root| root.join(MACOS_UPDATE_PENDING_FILE_NAME).exists())
        .then_some(MacosAppStartupHold::UpdateInProgress)
}

/// Block until no update transaction is open.
///
/// The wait is a file event on the update state root, not a poll; recovery
/// removes the marker when the transaction turns terminal.
///
/// # Errors
///
/// Returns an error when the root cannot be watched.
pub fn wait_for_macos_update_end(update_root: &Path) -> Result<(), MacosOwnerExecutionError> {
    use notify::{RecursiveMode, Watcher as _};

    let marker = update_root.join(MACOS_UPDATE_PENDING_FILE_NAME);
    let (signal_tx, signal_rx) = std::sync::mpsc::sync_channel(1);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            // A full queue already carries a pending look at the marker.
            let _ = signal_tx.try_send(());
        }
    })
    .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    watcher
        .watch(update_root, RecursiveMode::NonRecursive)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    while marker.exists() {
        signal_rx
            .recv()
            .map_err(|_| MacosOwnerExecutionError::new("the update state watch disconnected"))?;
    }
    Ok(())
}

/// One app, as it answers for itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosRunningApp {
    pub pid: u32,
    /// The bundle the app runs from.
    pub bundle: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlRequest {
    Identify,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum ControlReply {
    Running(MacosRunningApp),
    Quitting,
}

/// The app's end of the control socket.
pub struct MacosAppControlServer {
    listener: UnixListener,
    path: PathBuf,
}

impl MacosAppControlServer {
    /// Claim the socket in `update_root`, replacing what an app that crashed
    /// left there. Only one app runs per account, so nothing live is taken.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket cannot be bound or made private.
    pub fn bind(update_root: &Path) -> io::Result<Self> {
        let path = update_root.join(MACOS_APP_CONTROL_SOCKET_NAME);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self { listener, path })
    }

    /// Answer requests until one asks this app to quit, and return once that
    /// has been acknowledged, leaving the quitting to the caller.
    ///
    /// A client that misbehaves loses its connection and nothing else.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket stops accepting.
    pub fn serve_until_quit(self, app: &MacosRunningApp) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept()?;
            if answer(stream, app).unwrap_or(false) {
                return Ok(());
            }
        }
    }
}

impl Drop for MacosAppControlServer {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.path));
    }
}

/// Answer one connection; returns whether it asked the app to quit.
fn answer(stream: UnixStream, app: &MacosRunningApp) -> io::Result<bool> {
    stream.set_read_timeout(Some(CONTROL_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    while let Some(request) = read_line::<ControlRequest>(&mut reader)? {
        let reply = match request {
            ControlRequest::Identify => ControlReply::Running(app.clone()),
            ControlRequest::Quit => ControlReply::Quitting,
        };
        write_line(&mut writer, &reply)?;
        if request == ControlRequest::Quit {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_line<T: DeserializeOwned>(reader: &mut BufReader<UnixStream>) -> io::Result<Option<T>> {
    let mut line = String::new();
    let read = reader
        .by_ref()
        .take(MAX_CONTROL_LINE_BYTES)
        .read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_line<T: Serialize>(writer: &mut UnixStream, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// The macOS calls an update makes on the app's behalf.
pub trait MacosAppHost {
    /// Wait up to `timeout` for process `pid` to exit, and report whether it
    /// did. A process that is already gone has exited.
    ///
    /// # Errors
    ///
    /// Returns an error when the process cannot be watched.
    fn wait_for_exit(
        &mut self,
        pid: u32,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Open exactly the bundle at `bundle` through LaunchServices, never
    /// whichever bundle answers to its identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when LaunchServices refuses.
    fn open_bundle(&mut self, bundle: &Path) -> Result<(), MacosOwnerExecutionError>;
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelaunchRecord {
    bundle: PathBuf,
}

/// The updater's end of the app's control socket and relaunch record.
#[derive(Debug, Clone)]
pub struct MacosAppControl {
    root: PathBuf,
}

impl MacosAppControl {
    /// Address the app of the installation with this update state root.
    #[must_use]
    pub fn new(update_root: &Path) -> Self {
        Self {
            root: update_root.to_path_buf(),
        }
    }

    /// Have a running app leave, and return it once it has exited. `None`
    /// means no app is running. A live app that does not answer is an error.
    ///
    /// The app's bundle is durably recorded before it is asked to quit, so
    /// [`Self::relaunch_recorded`] brings it back however this run ends. A
    /// translocated app is asked to quit and never recorded: its path would
    /// be gone by the time anything opened it.
    ///
    /// # Errors
    ///
    /// Returns an error when the app answers wrongly or outlives `timeout`.
    pub fn quit_for_update(
        &self,
        host: &mut impl MacosAppHost,
        timeout: Duration,
    ) -> Result<Option<MacosRunningApp>, MacosOwnerExecutionError> {
        let Some(mut connection) = self.connect()? else {
            return Ok(None);
        };
        let ControlReply::Running(app) = connection.request(ControlRequest::Identify)? else {
            return Err(MacosOwnerExecutionError::new(
                "the app answered its identity request with something else",
            ));
        };
        if validate_relaunch_bundle(&app.bundle).is_ok() {
            self.record_relaunch(&app.bundle)?;
        }
        if connection.request(ControlRequest::Quit)? != ControlReply::Quitting {
            return Err(MacosOwnerExecutionError::new(
                "the app did not acknowledge its quit request",
            ));
        }
        if !host.wait_for_exit(app.pid, timeout)? {
            return Err(MacosOwnerExecutionError::new(
                "the app did not quit in time",
            ));
        }
        Ok(Some(app))
    }

    /// Open the recorded bundle, and clear the record once it has opened.
    /// Returns the bundle it opened; `None` means nothing was recorded.
    ///
    /// Run this after every terminal transaction and whenever no transaction
    /// is open, so an app that left for an update which never began comes
    /// back too. A repeat after a crash finds the app running, and opening a
    /// running app only brings it forward.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is unusable or the bundle will not
    /// open; the record stays for the next attempt.
    pub fn relaunch_recorded(
        &self,
        host: &mut impl MacosAppHost,
    ) -> Result<Option<PathBuf>, MacosOwnerExecutionError> {
        let path = self.root.join(MACOS_APP_RELAUNCH_FILE_NAME);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MacosOwnerExecutionError::new(error.to_string())),
        };
        let record: RelaunchRecord = serde_json::from_slice(&bytes)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        validate_relaunch_bundle(&record.bundle)?;
        host.open_bundle(&record.bundle)?;
        fs::remove_file(&path).map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        Ok(Some(record.bundle))
    }

    fn record_relaunch(&self, bundle: &Path) -> Result<(), MacosOwnerExecutionError> {
        write_json_atomic(
            &self.root,
            &self.root.join(MACOS_APP_RELAUNCH_FILE_NAME),
            RELAUNCH_RECORD,
            &RelaunchRecord {
                bundle: bundle.to_path_buf(),
            },
        )
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
    }

    fn connect(&self) -> Result<Option<Connection>, MacosOwnerExecutionError> {
        let stream = match UnixStream::connect(self.root.join(MACOS_APP_CONTROL_SOCKET_NAME)) {
            Ok(stream) => stream,
            // A socket file with no listener is what a crashed app leaves.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(MacosOwnerExecutionError::new(error.to_string())),
        };
        Connection::new(stream)
            .map(Some)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
    }
}

struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Connection {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_read_timeout(Some(CONTROL_IO_TIMEOUT))?;
        stream.set_write_timeout(Some(CONTROL_IO_TIMEOUT))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    fn request(
        &mut self,
        request: ControlRequest,
    ) -> Result<ControlReply, MacosOwnerExecutionError> {
        let mut exchange = || -> io::Result<Option<ControlReply>> {
            write_line(&mut self.writer, &request)?;
            read_line(&mut self.reader)
        };
        exchange()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .ok_or_else(|| {
                MacosOwnerExecutionError::new("the app closed its control socket unanswered")
            })
    }
}

/// A bundle may be opened again only when its path is exact, names an app
/// and outlives the process that reported it.
fn validate_relaunch_bundle(bundle: &Path) -> Result<(), MacosOwnerExecutionError> {
    let refused = |detail| {
        Err(MacosOwnerExecutionError::new(format!(
            "{} {detail}",
            bundle.display()
        )))
    };
    if !bundle.is_absolute()
        || bundle.as_os_str().len() > MAX_MACOS_EXECUTABLE_PATH_BYTES
        || bundle
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return refused("is not a bounded canonical absolute path");
    }
    if bundle
        .extension()
        .is_none_or(|extension| extension != "app")
    {
        return refused("is not an app bundle");
    }
    if is_macos_translocated_path(bundle) {
        return refused("is a translocated path, which vanishes with the app");
    }
    Ok(())
}

/// The native [`MacosAppHost`].
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeMacosAppHost;

#[cfg(target_os = "macos")]
impl MacosAppHost for NativeMacosAppHost {
    fn wait_for_exit(
        &mut self,
        pid: u32,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        crate::wait_for_process_exit_within(pid, timeout)
    }

    fn open_bundle(&mut self, bundle: &Path) -> Result<(), MacosOwnerExecutionError> {
        crate::direct_launchd::open_macos_bundle(bundle)
    }
}
