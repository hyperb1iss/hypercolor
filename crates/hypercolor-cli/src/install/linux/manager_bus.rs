//! A minimal D-Bus client for the systemd user manager's private socket.
//!
//! The user manager serves its API peer to peer on
//! `$XDG_RUNTIME_DIR/systemd/private` and stamps every message it sends with
//! the sender `org.freedesktop.systemd1`. zbus 5 treats a sender that is not
//! a unique connection name as corrupt and panics while routing replies, so
//! this client frames messages itself: zbus builds outgoing calls, and
//! zvariant decodes incoming headers and bodies without judging the sender.

use std::collections::VecDeque;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zbus::zvariant::serialized::{Context, Data};
use zbus::zvariant::{DynamicType, Endian, OwnedValue, Value};

use super::super::InstallPlatformError;
use super::model::error;

const SYSTEMD_DESTINATION: &str = "org.freedesktop.systemd1";
/// Far above anything the manager sends about one unit. A larger signal is
/// skipped; a larger reply fails its call.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_AUTH_LINE_BYTES: usize = 512;
const READ_CHUNK_BYTES: usize = 64 * 1024;

const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;

/// One decoded message from the manager.
#[derive(Debug, Clone)]
pub(super) struct ManagerMessage {
    pub(super) kind: u8,
    /// Only a peer that answers calls reads it; the client matches replies
    /// by their reply serial.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) serial: u32,
    pub(super) reply_serial: Option<u32>,
    pub(super) path: Option<String>,
    pub(super) interface: Option<String>,
    pub(super) member: Option<String>,
    pub(super) error_name: Option<String>,
    signature: String,
    endian: Endian,
    body_start: usize,
    bytes: Vec<u8>,
}

impl ManagerMessage {
    /// Decode the body with the signature the message declares.
    pub(super) fn body<T: DeserializeOwned>(&self) -> Result<T, InstallPlatformError> {
        let data = Data::new(
            &self.bytes[self.body_start..],
            Context::new_dbus(self.endian, self.body_start),
        );
        data.deserialize_for_signature::<_, T>(self.signature.as_str())
            .map(|(value, _)| value)
            .map_err(|source| error(format!("systemd message body: {source}")))
    }

    pub(super) fn is_signal(&self, interface: &str, member: &str) -> bool {
        self.kind == SIGNAL
            && self.interface.as_deref() == Some(interface)
            && self.member.as_deref() == Some(member)
    }
}

/// A method call the manager refused.
#[derive(Debug)]
pub(super) enum ManagerCallError {
    /// The manager answered with this D-Bus error name.
    Refused {
        name: String,
        message: String,
    },
    Transport(InstallPlatformError),
}

impl ManagerCallError {
    pub(super) fn into_platform(self) -> InstallPlatformError {
        match self {
            Self::Refused { name, message } => {
                error(format!("systemd refused the call: {name}: {message}"))
            }
            Self::Transport(source) => source,
        }
    }
}

impl From<InstallPlatformError> for ManagerCallError {
    fn from(source: InstallPlatformError) -> Self {
        Self::Transport(source)
    }
}

/// Reassembles messages from a stream.
///
/// Bytes accumulate in a buffer that only completed reads extend, so a read
/// abandoned at a deadline never loses part of a frame: the next read picks
/// up exactly where the stream stands.
#[derive(Debug, Default)]
pub(super) struct FrameReader {
    buffer: Vec<u8>,
    /// Bytes of an oversized signal still to throw away.
    discard: usize,
}

impl FrameReader {
    /// Read the next complete message.
    pub(super) async fn next(
        &mut self,
        stream: &mut tokio::net::UnixStream,
    ) -> Result<ManagerMessage, InstallPlatformError> {
        loop {
            if let Some(message) = self.take()? {
                return Ok(message);
            }
            self.fill(stream).await?;
        }
    }

    /// Read one line of the authentication exchange.
    async fn line(
        &mut self,
        stream: &mut tokio::net::UnixStream,
    ) -> Result<String, InstallPlatformError> {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\r\n") {
                let line: Vec<u8> = self.buffer.drain(..end + 2).collect();
                return String::from_utf8(line)
                    .map_err(|_| error("systemd manager authentication is not UTF-8"));
            }
            if self.buffer.len() >= MAX_AUTH_LINE_BYTES {
                return Err(error("systemd manager authentication line is too long"));
            }
            self.fill(stream).await?;
        }
    }

    async fn fill(
        &mut self,
        stream: &mut tokio::net::UnixStream,
    ) -> Result<(), InstallPlatformError> {
        self.buffer.reserve(READ_CHUNK_BYTES);
        // `read_buf` is cancel safe: if the caller's deadline drops this
        // future, no byte has left the socket without landing here.
        let read = stream.read_buf(&mut self.buffer).await.map_err(io_error)?;
        if read == 0 {
            return Err(error("systemd manager closed the connection"));
        }
        Ok(())
    }

    fn take(&mut self) -> Result<Option<ManagerMessage>, InstallPlatformError> {
        if self.discard > 0 {
            let dropped = self.discard.min(self.buffer.len());
            self.buffer.drain(..dropped);
            self.discard -= dropped;
            if self.discard > 0 {
                return Ok(None);
            }
        }
        let Some(fixed) = self.buffer.get(..16) else {
            return Ok(None);
        };
        let endian = match fixed[0] {
            b'l' => Endian::Little,
            b'B' => Endian::Big,
            _ => return Err(error("systemd message has an unknown byte order")),
        };
        if fixed[3] != 1 {
            return Err(error("systemd message has an unknown protocol version"));
        }
        let word = |at: usize| {
            let bytes = [fixed[at], fixed[at + 1], fixed[at + 2], fixed[at + 3]];
            u64::from(match endian {
                Endian::Little => u32::from_le_bytes(bytes),
                Endian::Big => u32::from_be_bytes(bytes),
            })
        };
        let kind = fixed[1];
        // Both lengths are u32, so this cannot overflow in u64.
        let fields_len = word(12);
        let total = (16 + fields_len).next_multiple_of(8) + word(4);
        let bound = u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX);
        if total > bound {
            // An oversized broadcast about some other unit must not break
            // this conversation; throw it away as it arrives.
            return match usize::try_from(total) {
                Ok(total) if kind == SIGNAL => {
                    self.discard = total;
                    self.take()
                }
                _ => Err(error("systemd message exceeds its byte bound")),
            };
        }
        let (Ok(total), Ok(fields_len)) = (usize::try_from(total), usize::try_from(fields_len))
        else {
            return Err(error("systemd message exceeds its byte bound"));
        };
        if self.buffer.len() < total {
            return Ok(None);
        }
        let header_end = 16 + fields_len;
        let bytes: Vec<u8> = self.buffer.drain(..total).collect();
        decode(bytes, endian, header_end, header_end.next_multiple_of(8)).map(Some)
    }
}

/// An authenticated peer-to-peer connection to the user manager.
pub(super) struct ManagerBus {
    stream: tokio::net::UnixStream,
    frames: FrameReader,
    signals: VecDeque<ManagerMessage>,
}

impl ManagerBus {
    /// Connect, prove the peer runs as `expected_uid`, and authenticate.
    pub(super) async fn connect(
        socket: &Path,
        expected_uid: u32,
    ) -> Result<Self, InstallPlatformError> {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(io_error)?;
        require_manager_peer(stream.peer_cred().map_err(io_error)?.uid(), expected_uid)?;
        // EXTERNAL names the uid as hex-encoded decimal text. BEGIN goes out
        // with AUTH, the way systemd's own clients send it, so the manager
        // leaves authentication before the first call arrives.
        let hex = hex::encode(expected_uid.to_string());
        stream
            .write_all(format!("\0AUTH EXTERNAL {hex}\r\nBEGIN\r\n").as_bytes())
            .await
            .map_err(io_error)?;
        let mut frames = FrameReader::default();
        let reply = frames.line(&mut stream).await?;
        if !reply.starts_with("OK ") {
            return Err(error(format!(
                "systemd manager refused authentication: {}",
                reply.trim_end()
            )));
        }
        Ok(Self {
            stream,
            frames,
            signals: VecDeque::new(),
        })
    }

    /// Call one method and decode its reply. Signals that arrive meanwhile
    /// are kept for [`Self::next_signal`].
    pub(super) async fn call<B, R>(
        &mut self,
        path: &str,
        interface: &str,
        member: &str,
        body: &B,
    ) -> Result<R, ManagerCallError>
    where
        B: Serialize + DynamicType,
        R: DeserializeOwned,
    {
        let message = zbus::message::Message::method_call(path, member)
            .and_then(|builder| builder.destination(SYSTEMD_DESTINATION))
            .and_then(|builder| builder.interface(interface))
            .and_then(|builder| builder.build(body))
            .map_err(|source| error(format!("encode systemd call {member}: {source}")))?;
        let serial = message.primary_header().serial_num().get();
        self.stream
            .write_all(message.data())
            .await
            .map_err(io_error)?;
        loop {
            let reply = self.frames.next(&mut self.stream).await?;
            match reply.kind {
                METHOD_RETURN if reply.reply_serial == Some(serial) => {
                    return reply.body().map_err(ManagerCallError::Transport);
                }
                ERROR if reply.reply_serial == Some(serial) => {
                    // systemd errors carry one human-readable string.
                    let message = reply.body::<String>().unwrap_or_default();
                    return Err(ManagerCallError::Refused {
                        name: reply.error_name.unwrap_or_default(),
                        message,
                    });
                }
                SIGNAL => self.signals.push_back(reply),
                // Method calls (the manager never sends one) and stale
                // replies belong to nothing this connection waits for.
                _ => {}
            }
        }
    }

    /// The next signal, queued or read from the socket.
    pub(super) async fn next_signal(&mut self) -> Result<ManagerMessage, InstallPlatformError> {
        if let Some(signal) = self.signals.pop_front() {
            return Ok(signal);
        }
        loop {
            let message = self.frames.next(&mut self.stream).await?;
            if message.kind == SIGNAL {
                return Ok(message);
            }
        }
    }
}

type RawHeader = (u8, u8, u8, u8, u32, u32, Vec<(u8, OwnedValue)>);

fn decode(
    bytes: Vec<u8>,
    endian: Endian,
    header_end: usize,
    body_start: usize,
) -> Result<ManagerMessage, InstallPlatformError> {
    let header = Data::new(&bytes[..header_end], Context::new_dbus(endian, 0));
    let ((_, kind, _, _, _, serial, fields), _): (RawHeader, usize) = header
        .deserialize()
        .map_err(|source| error(format!("systemd message header: {source}")))?;
    let mut message = ManagerMessage {
        kind,
        serial,
        reply_serial: None,
        path: None,
        interface: None,
        member: None,
        error_name: None,
        signature: String::new(),
        endian,
        body_start,
        bytes: Vec::new(),
    };
    for (code, value) in fields {
        let text = || match &*value {
            Value::Str(text) => Some(text.as_str().to_owned()),
            Value::ObjectPath(path) => Some(path.as_str().to_owned()),
            Value::Signature(signature) => Some(signature.to_string_no_parens()),
            _ => None,
        };
        match code {
            1 => message.path = text(),
            2 => message.interface = text(),
            3 => message.member = text(),
            4 => message.error_name = text(),
            5 => {
                message.reply_serial = match &*value {
                    Value::U32(serial) => Some(*serial),
                    _ => return Err(error("systemd reply serial is not a u32")),
                };
            }
            8 => message.signature = text().unwrap_or_default(),
            // Destination, sender (a well-known name on this socket) and
            // descriptor counts carry nothing this client acts on.
            _ => {}
        }
    }
    message.bytes = bytes;
    Ok(message)
}

/// The manager's socket must be served by the installing uid; anything else
/// on that path is not this user's systemd.
pub(super) fn require_manager_peer(
    observed_uid: u32,
    expected_uid: u32,
) -> Result<(), InstallPlatformError> {
    if observed_uid == expected_uid {
        Ok(())
    } else {
        Err(error(
            "user systemd private socket is served by another uid",
        ))
    }
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    if source.kind() == std::io::ErrorKind::UnexpectedEof {
        return error("systemd manager closed the connection");
    }
    error(source.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde::Serialize;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use zbus::zvariant::serialized::Context;
    use zbus::zvariant::{DynamicType, Endian, ObjectPath, OwnedObjectPath, Signature, Value};

    use super::{FrameReader, ManagerBus, ManagerCallError, require_manager_peer};

    const SENDER: &str = "org.freedesktop.systemd1";

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn a_manager_socket_served_by_another_uid_is_refused() {
        assert!(require_manager_peer(1100, 1100).is_ok());
        for foreign in [0, 1101] {
            let error = require_manager_peer(foreign, 1100)
                .expect_err("another uid is not this user's manager");
            assert!(error.to_string().contains("another uid"), "{error}");
        }
    }

    /// Encode a message the way the user manager does on its private
    /// socket: every message carries the well-known sender.
    fn stamped<B: Serialize + DynamicType>(
        endian: Endian,
        kind: u8,
        serial: u32,
        mut fields: Vec<(u8, Value<'static>)>,
        signature: &str,
        body: &B,
    ) -> Vec<u8> {
        let body = if signature.is_empty() {
            Vec::new()
        } else {
            zbus::zvariant::to_bytes_for_signature(Context::new_dbus(endian, 0), signature, body)
                .expect("encode body")
                .to_vec()
        };
        fields.push((7, Value::from(SENDER)));
        if !signature.is_empty() {
            fields.push((
                8,
                Value::Signature(Signature::try_from(signature).expect("signature")),
            ));
        }
        let marker = match endian {
            Endian::Little => b'l',
            Endian::Big => b'B',
        };
        let header = (
            marker,
            kind,
            0_u8,
            1_u8,
            u32::try_from(body.len()).expect("body length"),
            serial,
            fields,
        );
        let mut bytes = zbus::zvariant::to_bytes(Context::new_dbus(endian, 0), &header)
            .expect("encode header")
            .to_vec();
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes.extend_from_slice(&body);
        bytes
    }

    fn manager_signal(endian: Endian, serial: u32, id: u32, result: &str) -> Vec<u8> {
        let job = format!("/org/freedesktop/systemd1/job/{id}");
        stamped(
            endian,
            4,
            serial,
            vec![
                (
                    1,
                    Value::ObjectPath(
                        ObjectPath::try_from("/org/freedesktop/systemd1").expect("path"),
                    ),
                ),
                (2, Value::from("org.freedesktop.systemd1.Manager")),
                (3, Value::from("JobRemoved")),
            ],
            "uoss",
            &(
                id,
                ObjectPath::try_from(job.as_str()).expect("job path"),
                "hypercolor.service",
                result,
            ),
        )
    }

    fn job_removed(message: &super::ManagerMessage) -> (u32, String, String) {
        assert!(message.is_signal("org.freedesktop.systemd1.Manager", "JobRemoved"));
        let (id, _, unit, result): (u32, OwnedObjectPath, String, String) =
            message.body().expect("JobRemoved body");
        (id, unit, result)
    }

    /// Accept one client and answer its authentication with `reply`.
    async fn accept_authenticated(
        listener: &tokio::net::UnixListener,
        reply: &[u8],
    ) -> tokio::net::UnixStream {
        let (mut stream, _) = listener.accept().await.expect("client");
        let mut auth = Vec::new();
        while !auth.ends_with(b"\r\n") {
            auth.push(stream.read_u8().await.expect("auth byte"));
        }
        assert!(auth.starts_with(b"\0AUTH EXTERNAL "), "{auth:?}");
        let mut begin = [0_u8; 7];
        stream.read_exact(&mut begin).await.expect("begin");
        assert_eq!(&begin, b"BEGIN\r\n");
        stream.write_all(reply).await.expect("auth reply");
        stream
    }

    /// A manager that stamps its sender on every message, answers
    /// `StartUnit` after announcing an unrelated job, refuses `GetUnit`, and
    /// accepts `CancelJob`.
    async fn serve_stamped_manager(listener: tokio::net::UnixListener) {
        let mut stream =
            accept_authenticated(&listener, b"OK 0123456789abcdef0123456789abcdef\r\n").await;
        let mut frames = FrameReader::default();
        let mut serial = 100;
        while let Ok(call) = frames.next(&mut stream).await {
            serial += 1;
            let reply_to = || (5, Value::U32(call.serial));
            let mut out = Vec::new();
            match call.member.as_deref() {
                Some("StartUnit") => {
                    assert_eq!(call.path.as_deref(), Some("/org/freedesktop/systemd1"));
                    assert_eq!(
                        call.body::<(String, String)>()
                            .expect("StartUnit arguments"),
                        ("hypercolor.service".to_owned(), "fail".to_owned())
                    );
                    // A signal before the reply must not be lost, and a
                    // big-endian peer must decode too.
                    out.extend(manager_signal(Endian::Big, serial, 41, "done"));
                    out.extend(stamped(
                        Endian::Little,
                        2,
                        serial + 1,
                        vec![reply_to()],
                        "o",
                        &ObjectPath::try_from("/org/freedesktop/systemd1/job/42")
                            .expect("job path"),
                    ));
                    out.extend(manager_signal(Endian::Little, serial + 2, 42, "done"));
                    serial += 2;
                }
                Some("GetUnit") => out.extend(stamped(
                    Endian::Big,
                    3,
                    serial,
                    vec![
                        reply_to(),
                        (4, Value::from("org.freedesktop.systemd1.NoSuchUnit")),
                    ],
                    "s",
                    &"Unit hypercolor.service not loaded.",
                )),
                Some("CancelJob") => out.extend(stamped(
                    Endian::Little,
                    2,
                    serial,
                    vec![reply_to()],
                    "",
                    &(),
                )),
                other => panic!("unexpected call {other:?}"),
            }
            stream.write_all(&out).await.expect("reply");
        }
    }

    fn socket_fixture() -> (tempfile::TempDir, std::path::PathBuf, u32) {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("private");
        let uid = std::fs::metadata(directory.path())
            .map(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata))
            .expect("owner");
        (directory, socket, uid)
    }

    #[test]
    fn calls_and_signals_survive_the_managers_sender_stamping() {
        let (_directory, socket, uid) = socket_fixture();
        runtime().block_on(async {
            let listener = tokio::net::UnixListener::bind(&socket).expect("listener");
            let server = tokio::spawn(serve_stamped_manager(listener));
            let mut bus = ManagerBus::connect(&socket, uid).await.expect("connect");

            let job: OwnedObjectPath = bus
                .call(
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.systemd1.Manager",
                    "StartUnit",
                    &("hypercolor.service", "fail"),
                )
                .await
                .expect("StartUnit reply");
            assert_eq!(job.as_str(), "/org/freedesktop/systemd1/job/42");
            for expected in [41_u32, 42] {
                let signal = bus.next_signal().await.expect("JobRemoved");
                assert_eq!(
                    job_removed(&signal),
                    (expected, "hypercolor.service".to_owned(), "done".to_owned())
                );
            }

            let refused = bus
                .call::<_, OwnedObjectPath>(
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.systemd1.Manager",
                    "GetUnit",
                    &("hypercolor.service",),
                )
                .await
                .expect_err("an unloaded unit is refused");
            let ManagerCallError::Refused { name, message } = refused else {
                panic!("expected a refusal");
            };
            assert_eq!(name, "org.freedesktop.systemd1.NoSuchUnit");
            assert!(message.contains("not loaded"), "{message}");

            bus.call::<_, ()>(
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "CancelJob",
                &(42_u32,),
            )
            .await
            .expect("empty reply");
            drop(bus);
            server.await.expect("fake manager");
        });
    }

    #[test]
    fn a_refused_authentication_or_a_foreign_peer_never_reaches_a_call() {
        let (_directory, socket, uid) = socket_fixture();
        runtime().block_on(async {
            let listener = tokio::net::UnixListener::bind(&socket).expect("listener");
            let server = tokio::spawn(async move {
                let _stream = accept_authenticated(&listener, b"REJECTED EXTERNAL\r\n").await;
                // A peer checked as foreign hangs up before sending anything.
                let (mut foreign, _) = listener.accept().await.expect("second client");
                let mut sent = Vec::new();
                foreign.read_to_end(&mut sent).await.expect("hang up");
                sent
            });
            let refused = ManagerBus::connect(&socket, uid)
                .await
                .err()
                .expect("REJECTED ends the connection");
            assert!(
                refused.to_string().contains("refused authentication"),
                "{refused}"
            );
            let foreign = ManagerBus::connect(&socket, uid + 1)
                .await
                .err()
                .expect("a manager served by another uid is refused");
            assert!(foreign.to_string().contains("another uid"), "{foreign}");
            assert!(
                server.await.expect("fake manager").is_empty(),
                "no byte reaches a foreign peer"
            );
        });
    }

    #[test]
    fn a_read_abandoned_at_a_deadline_resumes_mid_frame() {
        runtime().block_on(async {
            let (mut client, mut server) = tokio::net::UnixStream::pair().expect("pair");
            let frame = manager_signal(Endian::Little, 7, 42, "done");
            let mut frames = FrameReader::default();
            server.write_all(&frame[..24]).await.expect("first half");
            let abandoned =
                tokio::time::timeout(Duration::from_millis(50), frames.next(&mut client)).await;
            assert!(abandoned.is_err(), "half a frame is not a message");
            server.write_all(&frame[24..]).await.expect("second half");
            let resumed = frames.next(&mut client).await.expect("the same frame");
            assert_eq!(
                job_removed(&resumed),
                (42, "hypercolor.service".to_owned(), "done".to_owned())
            );
        });
    }

    #[test]
    fn oversized_signals_are_skipped_and_oversized_replies_refused() {
        runtime().block_on(async {
            let (mut client, mut server) = tokio::net::UnixStream::pair().expect("pair");
            let mut frames = FrameReader::default();
            // A 2 MiB broadcast about some other unit, then a normal one.
            let mut big = vec![0_u8; 16];
            big[0] = b'l';
            big[1] = 4;
            big[3] = 1;
            big[4..8].copy_from_slice(&(2_u32 * 1024 * 1024 - 16).to_le_bytes());
            let writer = tokio::spawn(async move {
                server.write_all(&big).await.expect("big header");
                server
                    .write_all(&vec![0_u8; 2 * 1024 * 1024 - 16])
                    .await
                    .expect("big body");
                server
                    .write_all(&manager_signal(Endian::Little, 8, 43, "done"))
                    .await
                    .expect("next frame");
                server
            });
            let next = frames.next(&mut client).await.expect("the frame after it");
            assert_eq!(
                job_removed(&next),
                (43, "hypercolor.service".to_owned(), "done".to_owned())
            );
            let mut server = writer.await.expect("writer");

            let mut reply = [0_u8; 16];
            reply[0] = b'l';
            reply[1] = 2;
            reply[3] = 1;
            reply[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
            server.write_all(&reply).await.expect("oversized reply");
            let error = frames.next(&mut client).await.expect_err("bounded reply");
            assert!(error.to_string().contains("byte bound"), "{error}");
        });
    }

    #[test]
    fn malformed_frames_and_hangups_are_refused() {
        runtime().block_on(async {
            for (header, expected) in [
                (
                    *b"xl\x00\x01\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00",
                    "byte order",
                ),
                (
                    *b"l\x02\x00\x02\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00",
                    "protocol version",
                ),
            ] {
                let (mut client, mut server) = tokio::net::UnixStream::pair().expect("pair");
                server.write_all(&header).await.expect("header");
                let error = FrameReader::default()
                    .next(&mut client)
                    .await
                    .expect_err("malformed frame");
                assert!(error.to_string().contains(expected), "{error}");
            }
            let (mut client, server) = tokio::net::UnixStream::pair().expect("pair");
            drop(server);
            let error = FrameReader::default()
                .next(&mut client)
                .await
                .expect_err("closed connection");
            assert!(
                error.to_string().contains("closed the connection"),
                "{error}"
            );
        });
    }
}
