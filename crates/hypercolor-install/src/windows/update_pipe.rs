//! The app's end of the update service's pipe (RFC 57, B1.4.4).
//!
//! At each start the `HypercolorUpdate` service creates a pipe whose name
//! carries 128 random bits and publishes the name only to the installation's
//! recorded user. The name alone proves nothing, though: a process that
//! learned it could serve a pipe of that name before or after the service.
//! The app therefore asks which process serves the pipe it reached and
//! which process the service control manager runs the service in, and
//! refuses the pipe unless they are the same, before it reads or sends one
//! byte.
//!
//! The service is not always running: it starts for an update or a
//! recovery and stops when that is done. The app follows it with
//! [`follow_update_service`], which connects to each run's pipe as the run
//! starts and returns once a run writes [`UPDATE_PIPE_QUIT`].
//!
//! The host calls sit behind [`UpdatePipeHost`], so the check and the
//! following run against fakes on any platform; `WindowsUpdatePipeHost` is
//! the host that makes the Win32 calls.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::WindowsUpdatePipeHost;

/// The service whose own process must serve the pipe.
pub const UPDATE_SERVICE_NAME: &str = "HypercolorUpdate";
/// The record the service publishes the name in, under `activator\`.
pub const UPDATE_PIPE_RECORD: &str = "pipe.json";
const UPDATE_PIPE_RECORD_SCHEMA_VERSION: u32 = 1;
/// The one request the service writes: quit, so an update can stop this
/// app and its daemon.
pub const UPDATE_PIPE_QUIT: &[u8] = b"quit\n";
/// The most a host reads of a record, which is a schema version and a name
/// of fixed length.
pub const MAX_UPDATE_PIPE_RECORD_BYTES: u64 = 1024;
/// Every update pipe name is this prefix and 32 lowercase hex digits.
const PIPE_NAME_PREFIX: &str = r"\\.\pipe\hypercolor-update-";
const PIPE_NAME_RANDOM_DIGITS: usize = 32;

/// A pipe name the update service publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePipeName(String);

/// `pipe.json`: where the pipe of the running service is.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePipeRecord {
    schema_version: u32,
    name: String,
}

impl UpdatePipeName {
    /// The name that carries these 128 random bits.
    #[must_use]
    pub fn from_random(random: [u8; 16]) -> Self {
        let digits = hex::encode(random);
        Self(format!("{PIPE_NAME_PREFIX}{digits}"))
    }

    /// Accept exactly `\\.\pipe\hypercolor-update-` followed by 128 random
    /// bits as 32 lowercase hex digits, which keeps a published name from
    /// reaching any other pipe, a remote one or a file.
    ///
    /// # Errors
    ///
    /// Returns [`UpdatePipeError::InvalidName`] for any other text.
    pub fn parse(text: &str) -> Result<Self, UpdatePipeError> {
        let valid = text.strip_prefix(PIPE_NAME_PREFIX).is_some_and(|random| {
            random.len() == PIPE_NAME_RANDOM_DIGITS
                && random
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(UpdatePipeError::InvalidName(text.to_owned()))
        }
    }

    /// The full pipe path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The record that publishes this name.
    ///
    /// # Errors
    ///
    /// Returns the encoder's error.
    pub fn encode_record(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(&UpdatePipeRecord {
            schema_version: UPDATE_PIPE_RECORD_SCHEMA_VERSION,
            name: self.0.clone(),
        })
    }

    /// The name a published record holds.
    ///
    /// # Errors
    ///
    /// Returns [`UpdatePipeError::InvalidRecord`] for a record of another
    /// shape or schema, and [`UpdatePipeError::InvalidName`] for one whose
    /// name reaches anything but an update pipe.
    pub fn decode_record(bytes: &[u8]) -> Result<Self, UpdatePipeError> {
        let record: UpdatePipeRecord = serde_json::from_slice(bytes)
            .map_err(|error| UpdatePipeError::InvalidRecord(error.to_string()))?;
        if record.schema_version != UPDATE_PIPE_RECORD_SCHEMA_VERSION {
            return Err(UpdatePipeError::InvalidRecord(format!(
                "unsupported schema {}",
                record.schema_version
            )));
        }
        Self::parse(&record.name)
    }
}

/// What the client needs from the host it runs on.
pub trait UpdatePipeHost {
    /// A connected client end of a pipe.
    type Pipe: Read + Write;

    /// Connect to the pipe as a client, waiting while another client holds
    /// its one instance.
    ///
    /// # Errors
    ///
    /// Returns the host's error when nothing serves the name, which is also
    /// how a wait for a busy instance ends when the pipe goes away.
    fn connect(&self, name: &UpdatePipeName) -> io::Result<Self::Pipe>;

    /// The process that created the server end of `pipe`.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the server cannot be named.
    fn server_process_id(&self, pipe: &Self::Pipe) -> io::Result<u32>;

    /// The process the update service runs in, or `None` when the service
    /// is not running or not registered.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the service control manager cannot be
    /// asked.
    fn service_process_id(&self) -> io::Result<Option<u32>>;

    /// The record the running service published, at most
    /// [`MAX_UPDATE_PIPE_RECORD_BYTES`] of it.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the record cannot be read, or holds
    /// more than that.
    fn read_record(&self) -> io::Result<Vec<u8>>;

    /// Block until a run of the update service starts that this host has
    /// not reported yet, which is at once when one already runs, or has
    /// started since the last report. A run's pipe is published by the time
    /// it is reported. Returns `false` when the service is not registered or
    /// is being deleted, since no run will start.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the service control manager cannot be
    /// asked.
    fn wait_for_run(&self) -> io::Result<bool>;
}

/// Why [`follow_update_service`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateServiceEnd {
    /// A run asked this app to quit.
    Quit,
    /// The service is not registered, or is being deleted, so no run will.
    Unregistered,
}

/// A pipe whose server is the update service's own process.
#[derive(Debug)]
pub struct VerifiedUpdatePipe<P> {
    pipe: P,
    server_process_id: u32,
}

impl<P> VerifiedUpdatePipe<P> {
    /// The update service process that serves the pipe.
    #[must_use]
    pub fn server_process_id(&self) -> u32 {
        self.server_process_id
    }

    /// The verified pipe itself.
    #[must_use]
    pub fn into_inner(self) -> P {
        self.pipe
    }
}

impl<P: Read> Read for VerifiedUpdatePipe<P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.pipe.read(buffer)
    }
}

impl<P: Write> Write for VerifiedUpdatePipe<P> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.pipe.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.pipe.flush()
    }
}

/// Why a pipe was refused.
#[derive(Debug, thiserror::Error)]
pub enum UpdatePipeError {
    /// The published name is not an update pipe name.
    #[error("{0:?} is not an update pipe name")]
    InvalidName(String),
    /// The published record cannot be read.
    #[error("the update pipe record is invalid: {0}")]
    InvalidRecord(String),
    /// Nothing could be reached at the name.
    #[error("could not connect to the update pipe: {0}")]
    Connect(#[source] io::Error),
    /// The pipe's server could not be named.
    #[error("could not name the process serving the update pipe: {0}")]
    ServerIdentity(#[source] io::Error),
    /// The service control manager could not be asked about the service.
    #[error("could not ask the service control manager about {UPDATE_SERVICE_NAME}: {0}")]
    ServiceQuery(#[source] io::Error),
    /// The update service is not running, so nothing it published is live.
    #[error("{UPDATE_SERVICE_NAME} is not running")]
    ServiceNotRunning,
    /// Another process serves the pipe.
    #[error(
        "process {server} serves the update pipe, but {UPDATE_SERVICE_NAME} runs in process {service}"
    )]
    NotTheService { server: u32, service: u32 },
    /// The published record could not be read.
    #[error("could not read the update pipe record: {0}")]
    ReadRecord(#[source] io::Error),
    /// The verified pipe failed while the client waited on it.
    #[error("the update pipe failed: {0}")]
    Read(#[source] io::Error),
    /// The service wrote something other than a quit request.
    #[error("the update service sent {0:?}, which is not a request")]
    UnknownRequest(String),
}

/// Connect to the update pipe at `name` and return it only when the update
/// service's own process serves it.
///
/// The service is asked after the server is named, so its answer is no
/// older than the pipe it vouches for. A process ID is only an identity
/// while its process lives; the check leans on the pipe's name reaching
/// nobody but the recorded user for the rest.
///
/// # Errors
///
/// Returns an error when the pipe cannot be reached or its server named,
/// when the service is not running or cannot be asked, or when any other
/// process serves the pipe. A refused pipe is closed unread.
pub fn connect_update_pipe<H: UpdatePipeHost>(
    host: &H,
    name: &UpdatePipeName,
) -> Result<VerifiedUpdatePipe<H::Pipe>, UpdatePipeError> {
    let pipe = host.connect(name).map_err(UpdatePipeError::Connect)?;
    let server = host
        .server_process_id(&pipe)
        .map_err(UpdatePipeError::ServerIdentity)?;
    let service = match host
        .service_process_id()
        .map_err(UpdatePipeError::ServiceQuery)?
    {
        Some(service) if service != 0 => service,
        _ => return Err(UpdatePipeError::ServiceNotRunning),
    };
    if server != service {
        return Err(UpdatePipeError::NotTheService { server, service });
    }
    Ok(VerifiedUpdatePipe {
        pipe,
        server_process_id: server,
    })
}

/// Wait on a verified pipe until the service asks this app to quit, which
/// returns `true`, or ends its run without asking, which returns `false`.
///
/// # Errors
///
/// Returns an error when the pipe fails, and when the service writes
/// anything but [`UPDATE_PIPE_QUIT`], whole.
pub fn await_quit_request(pipe: impl Read) -> Result<bool, UpdatePipeError> {
    let mut request = Vec::with_capacity(UPDATE_PIPE_QUIT.len());
    pipe.take(UPDATE_PIPE_QUIT.len() as u64)
        .read_to_end(&mut request)
        .map_err(UpdatePipeError::Read)?;
    match request.as_slice() {
        [] => Ok(false),
        quit if quit == UPDATE_PIPE_QUIT => Ok(true),
        other => Err(UpdatePipeError::UnknownRequest(
            String::from_utf8_lossy(other).into_owned(),
        )),
    }
}

/// Take part in the run of the update service that is running now: read
/// the name it published, connect once its own process proves to serve the
/// pipe, and wait there. Returns whether the run asked this app to quit; a
/// run whose pipe is gone before the client reaches it asked nothing.
///
/// # Errors
///
/// Returns an error when the record cannot be read, the pipe is refused or
/// fails, or the service asks for anything else.
pub fn answer_update_run<H: UpdatePipeHost>(host: &H) -> Result<bool, UpdatePipeError> {
    let record = host.read_record().map_err(UpdatePipeError::ReadRecord)?;
    let name = UpdatePipeName::decode_record(&record)?;
    match connect_update_pipe(host, &name) {
        Ok(pipe) => await_quit_request(pipe),
        Err(UpdatePipeError::Connect(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Follow the update service for as long as it is registered, answering
/// each run as it starts, and return once a run asks this app to quit.
///
/// Whatever ends a run for the client, a refusal included, the client next
/// waits for a run the host has not reported, so each run is answered once,
/// a run that started while the client was busy with the last is still
/// answered, and nothing is retried on a timer. `lost` hears every run that
/// ended in an error.
///
/// # Errors
///
/// Returns the host's error when the service control manager cannot be
/// asked.
pub fn follow_update_service<H: UpdatePipeHost>(
    host: &H,
    mut lost: impl FnMut(UpdatePipeError),
) -> io::Result<UpdateServiceEnd> {
    while host.wait_for_run()? {
        match answer_update_run(host) {
            Ok(true) => return Ok(UpdateServiceEnd::Quit),
            Ok(false) => {}
            Err(error) => lost(error),
        }
    }
    Ok(UpdateServiceEnd::Unregistered)
}

#[cfg(test)]
#[path = "update_pipe_tests.rs"]
mod tests;
