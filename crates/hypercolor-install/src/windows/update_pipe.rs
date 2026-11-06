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
//! The host calls sit behind [`UpdatePipeHost`], so the check runs against
//! fakes on any platform; `WindowsUpdatePipeHost` is the host that makes
//! the Win32 calls.

use std::io::{self, Read, Write};

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::WindowsUpdatePipeHost;

/// The service whose own process must serve the pipe.
pub const UPDATE_SERVICE_NAME: &str = "HypercolorUpdate";
/// Every update pipe name is this prefix and 32 lowercase hex digits.
const PIPE_NAME_PREFIX: &str = r"\\.\pipe\hypercolor-update-";
const PIPE_NAME_RANDOM_DIGITS: usize = 32;

/// A pipe name the update service publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePipeName(String);

impl UpdatePipeName {
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
}

/// What the client needs from the host it runs on.
pub trait UpdatePipeHost {
    /// A connected client end of a pipe.
    type Pipe: Read + Write;

    /// Connect to the pipe as a client.
    ///
    /// # Errors
    ///
    /// Returns the host's error when nothing serves the name or every
    /// instance is busy.
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

#[cfg(test)]
#[path = "update_pipe_tests.rs"]
mod tests;
