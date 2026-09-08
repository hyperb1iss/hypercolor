//! Authenticated local control for the app and the persistent CLI owner.

use std::fs::{File, OpenOptions};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// The desktop app's local control registration.
pub const APP_CONTROL: &str = "openrgb-app";
/// The explicit CLI owner's local control registration.
pub const OWNER_CONTROL: &str = "openrgb-owner";
const MESSAGE_LIMIT: u64 = 1_048_576;
const RESPONSE_WAIT: Duration = Duration::from_secs(40);

#[derive(Debug, Serialize, Deserialize)]
struct Endpoint {
    address: SocketAddr,
    token: String,
}

/// One authenticated lifecycle operation. Payloads carry only current facts,
/// never the daemon API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub action: String,
    pub payload: Value,
}

#[derive(Serialize, Deserialize)]
struct AuthenticatedRequest {
    token: String,
    request: ControlRequest,
}

/// A completed operation or a specific reported failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlReply {
    pub status: Value,
    pub error: Option<String>,
}

/// A listener registered while its owner holds an OS file lock. No pid is
/// trusted to establish ownership or terminate a process.
/// Unix records are mode 0600; Windows records inherit the data directory
/// ACL, so that directory must remain restricted to its owning user.
pub struct ControlServer {
    listener: TcpListener,
    endpoint: Endpoint,
    record: PathBuf,
    lock: File,
    pending: tokio::task::JoinSet<Option<PendingControl>>,
}

/// An authenticated request and the connection awaiting its reply.
pub struct PendingControl {
    pub request: ControlRequest,
    stream: TcpStream,
}

impl ControlServer {
    /// Register a local owner. Returns `WouldBlock` if that owner is alive.
    pub async fn bind(directory: &Path, name: &str) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let lock = private_file(&directory.join(format!("{name}.lock")), false)?;
        lock.try_lock().map_err(io::Error::from)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = Endpoint {
            address: listener.local_addr()?,
            token: uuid::Uuid::new_v4().to_string(),
        };
        let record = directory.join(format!("{name}.json"));
        let bytes = serde_json::to_vec(&endpoint)?;
        hypercolor_persistence::AtomicFileWriter::with_file_mode(&record, 0o600)
            .and_then(|writer| writer.write(&bytes))
            .map_err(io::Error::other)?;
        Ok(Self {
            listener,
            endpoint,
            record,
            lock,
            pending: tokio::task::JoinSet::new(),
        })
    }

    /// Wait for one authorized request. Invalid clients receive no authority.
    pub async fn accept(&mut self) -> io::Result<PendingControl> {
        loop {
            tokio::select! {
                result = self.pending.join_next(), if !self.pending.is_empty() => {
                    if let Some(Ok(Some(request))) = result { return Ok(request); }
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted?;
                    let token = self.endpoint.token.clone();
                    self.pending.spawn(async move {
                        let read = tokio::time::timeout(Duration::from_secs(5), read_message::<AuthenticatedRequest>(stream)).await;
                        let Ok(Ok((authenticated, stream))) = read else { return None; };
                        if authenticated.token != token { return None; }
                        Some(PendingControl { request: authenticated.request, stream })
                    });
                }
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.record);
        let _ = self.lock.unlock();
    }
}

impl PendingControl {
    /// Send the operation result before releasing the request connection.
    pub async fn respond(mut self, reply: &ControlReply) -> io::Result<()> {
        write_message(&mut self.stream, reply).await
    }
}

/// Send to a registered living owner. `None` means no owner holds the lock;
/// a living owner that fails to answer is an error, never a spawn fallback.
pub async fn send_control(
    directory: &Path,
    name: &str,
    request: ControlRequest,
) -> io::Result<Option<ControlReply>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match send_registered(directory, name, request.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(error)
                if tokio::time::Instant::now() < deadline
                    && matches!(
                        error.kind(),
                        io::ErrorKind::NotFound
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::InvalidData
                    ) =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn send_registered(
    directory: &Path,
    name: &str,
    request: ControlRequest,
) -> io::Result<Option<ControlReply>> {
    let lock_path = directory.join(format!("{name}.lock"));
    let lock = match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match lock.try_lock() {
        Ok(()) => {
            lock.unlock()?;
            return Ok(None);
        }
        Err(std::fs::TryLockError::WouldBlock) => {}
        Err(error) => return Err(error.into()),
    }
    let endpoint: Endpoint =
        serde_json::from_reader(File::open(directory.join(format!("{name}.json")))?)?;
    if !endpoint.address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "OpenRGB control must remain on loopback",
        ));
    }
    tokio::time::timeout(RESPONSE_WAIT, async {
        let mut stream = TcpStream::connect(endpoint.address).await?;
        write_message(
            &mut stream,
            &AuthenticatedRequest {
                token: endpoint.token,
                request,
            },
        )
        .await?;
        let (reply, _) = read_message(stream).await?;
        Ok(Some(reply))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "OpenRGB owner did not answer"))?
}

fn private_file(path: &Path, truncate: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

async fn write_message<T: Serialize>(stream: &mut TcpStream, message: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await
}

async fn read_message<T: serde::de::DeserializeOwned>(
    stream: TcpStream,
) -> io::Result<(T, TcpStream)> {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    (&mut reader)
        .take(MESSAGE_LIMIT)
        .read_until(b'\n', &mut bytes)
        .await?;
    if bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unterminated OpenRGB control request",
        ));
    }
    let value = serde_json::from_slice(&bytes)?;
    Ok((value, reader.into_inner()))
}

/// Retained server authority. Explicit unlock prevents a concurrent fork's
/// inherited descriptor from extending ownership after the child tree stops.
#[derive(Debug)]
pub struct ServerClaim(File);

impl Drop for ServerClaim {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::warn!(%error, "OpenRGB server ownership release failed");
        }
    }
}

/// Claim child ownership across the app and CLI holder before partitioning or
/// spawning. Retain the returned claim until the child exits or is stopped.
pub fn try_claim_server(directory: &Path, endpoint: SocketAddr) -> io::Result<Option<ServerClaim>> {
    let control = directory.join("control");
    std::fs::create_dir_all(&control)?;
    // Every local bind spelling shares authority for a port, preventing two
    // supervisors from treating IPv4 and IPv6 loopback as separate hardware.
    let lock = private_file(
        &control.join(format!("openrgb-server-{}.lock", endpoint.port())),
        false,
    )?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(ServerClaim(lock))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::try_claim_server;

    #[test]
    fn claim_drop_releases_authority_while_a_fork_description_survives() {
        let directory = tempfile::tempdir().expect("directory");
        let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
        let claim = try_claim_server(directory.path(), endpoint)
            .expect("claim")
            .expect("owner");
        // dup and fork retain the same kernel open-file description. Keep a
        // second reference alive to reproduce inheritance before child exec.
        let inherited = claim.0.try_clone().expect("inherited description");
        drop(claim);
        let successor = try_claim_server(directory.path(), endpoint).expect("successor");
        assert!(
            successor.is_some(),
            "owner release must not depend on another process reaching exec"
        );
        drop(inherited);
        assert!(
            try_claim_server(directory.path(), endpoint)
                .expect("still retained")
                .is_none()
        );
    }
}
