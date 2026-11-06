//! USB CDC-ACM serial transport for line-oriented protocols.
//!
//! A send hands its bytes to the kernel tty layer and returns without
//! waiting for the device to drain them. `flush` on a tokio-serial stream is
//! a synchronous `tcdrain` that parks the calling runtime worker until the
//! port drains, has no timeout, and discards its own error, so this
//! transport never calls it. The tty layer keeps bytes in write order, a
//! write deadline catches a device that stops accepting data, and protocols
//! that need a barrier get one from the device's reply. Ports are released
//! off the runtime by the [`release`] service.

mod release;

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio_serial::{SerialPortBuilderExt, SerialPortType};
use tracing::{debug, trace, warn};

use crate::transport::{Transport, TransportError};
use release::ReleaseService;

/// Default bound on one send before the port counts as wedged.
///
/// A send waits only while the kernel's output queue is full, which a
/// healthy CDC-ACM device clears within a few milliseconds, so this budget
/// trips only when the device has stopped taking data.
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Bound on [`Transport::close`] for this transport.
///
/// Closing a tty waits in the kernel until queued output drains, for up to
/// the driver's closing wait (30 seconds by default) when the device is
/// wedged. The release runs on its own thread; once this bound passes,
/// close returns and the release finishes in the background.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Bound on how long [`UsbSerialTransport::open`] waits for an earlier
/// session's release of the same port to finish.
///
/// It stays well inside the default five second connect budget, so a port
/// whose old descriptor is still draining fails the attempt with a clear
/// error and the lifecycle retries later.
pub const RELEASE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

trait AsyncSerialIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncSerialIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

type SerialStreamSlot = Option<Box<dyn AsyncSerialIo>>;

/// USB CDC-ACM serial transport used by Focus-class devices.
pub struct UsbSerialTransport {
    path: String,
    /// `None` once [`Transport::close`] has released the port.
    stream: tokio::sync::Mutex<SerialStreamSlot>,
    /// Flips to `true` on close or after a failed write, and wakes any
    /// operation still waiting on the port.
    closed: watch::Sender<bool>,
    write_timeout: Duration,
    release: &'static ReleaseService,
}

impl UsbSerialTransport {
    /// Discover and open a serial port for one USB device.
    ///
    /// Before opening the port, this starts the release service and waits
    /// up to [`RELEASE_WAIT_TIMEOUT`] for an earlier session's release of the
    /// same path to finish.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] if no matching serial port can be found,
    /// the release service cannot start, an earlier release of the port is
    /// still running ([`TransportError::NotReady`]), or the port cannot be
    /// opened.
    pub async fn open(
        vendor_id: u16,
        product_id: u16,
        baud_rate: u32,
        serial: Option<&str>,
    ) -> Result<Self, TransportError> {
        let release = release::service()?;
        let ports = tokio_serial::available_ports().map_err(|error| TransportError::IoError {
            detail: format!("failed to enumerate serial ports: {error}"),
        })?;

        let mut candidates = ports
            .into_iter()
            .filter_map(|port| match port.port_type {
                SerialPortType::UsbPort(info)
                    if info.vid == vendor_id
                        && info.pid == product_id
                        && serial
                            .is_none_or(|wanted| info.serial_number.as_deref() == Some(wanted)) =>
                {
                    Some(port.port_name)
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        candidates.sort();

        if candidates.len() > 1 && serial.is_none() {
            return Err(TransportError::NotFound {
                detail: format!(
                    "multiple serial ports matched {:04X}:{:04X}; serial number required (candidates={})",
                    vendor_id,
                    product_id,
                    candidates.join(", ")
                ),
            });
        }

        let Some(path) = candidates.into_iter().next() else {
            return Err(TransportError::NotFound {
                detail: format!(
                    "serial port not found for {:04X}:{:04X} (serial={})",
                    vendor_id,
                    product_id,
                    serial.unwrap_or("<none>")
                ),
            });
        };

        release::wait_for_release(&path, RELEASE_WAIT_TIMEOUT).await?;
        let stream = tokio_serial::new(path.clone(), baud_rate)
            .open_native_async()
            .map_err(|error| map_serial_open_error(&error, &path))?;

        debug!(
            vendor_id = format_args!("{vendor_id:04X}"),
            product_id = format_args!("{product_id:04X}"),
            baud_rate,
            path = %path,
            serial = serial.unwrap_or("<none>"),
            "opened USB serial transport"
        );

        Ok(Self::with_release(path, stream, release))
    }

    /// Construct a serial transport from an existing async stream.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::IoError`] when the process cannot start its
    /// serial release service. `stream` is then dropped on the calling
    /// thread before anything has been written to it; [`Self::open`] starts
    /// the service before it opens a port, so it never does this.
    pub fn from_stream(
        path: impl Into<String>,
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
    ) -> Result<Self, TransportError> {
        let release = release::service()?;
        Ok(Self::with_release(path, stream, release))
    }

    fn with_release(
        path: impl Into<String>,
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
        release: &'static ReleaseService,
    ) -> Self {
        Self {
            path: path.into(),
            stream: tokio::sync::Mutex::new(Some(Box::new(stream))),
            closed: watch::Sender::new(false),
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            release,
        }
    }

    /// Wait until every release of `path` handed off so far has closed its
    /// descriptor, for at most `timeout`. [`Self::open`] runs this before it
    /// opens a port.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::NotReady`] when a release is still running
    /// after `timeout`.
    pub async fn wait_for_release(path: &str, timeout: Duration) -> Result<(), TransportError> {
        release::wait_for_release(path, timeout).await
    }

    /// Bound each send by `timeout` instead of [`DEFAULT_WRITE_TIMEOUT`].
    ///
    /// A send that misses its deadline fails the transport, because the
    /// device may already hold part of the packet.
    #[must_use]
    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if *self.closed.borrow() {
            return Err(TransportError::Closed);
        }
        Ok(())
    }

    /// Run `operation` until it finishes or the transport closes.
    async fn until_closed<T>(
        &self,
        operation: impl Future<Output = Result<T, TransportError>>,
    ) -> Result<T, TransportError> {
        let mut closed = self.closed.subscribe();
        tokio::select! {
            biased;
            // The transport owns the sender, so this only resolves on close.
            _ = closed.wait_for(|closed| *closed) => Err(TransportError::Closed),
            result = operation => result,
        }
    }

    async fn send_locked(
        &self,
        slot: &mut SerialStreamSlot,
        data: &[u8],
    ) -> Result<(), TransportError> {
        trace!(
            path = %self.path,
            packet_len = data.len(),
            payload = %String::from_utf8_lossy(data),
            "usb serial send"
        );
        let stream = slot.as_mut().ok_or(TransportError::Closed)?;
        let write_timeout = self.write_timeout;
        let result = self
            .until_closed(async {
                match tokio::time::timeout(write_timeout, stream.write_all(data)).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(map_io_error(&error, "write")),
                    Err(_) => Err(TransportError::Timeout {
                        timeout_ms: duration_ms(write_timeout),
                    }),
                }
            })
            .await;

        match result {
            Ok(()) | Err(TransportError::Closed) => result,
            Err(error) => {
                // A write that failed or stalled partway may have left part of
                // a packet queued for the device, so nothing after it would
                // frame correctly. Fail the transport and let close release
                // the port.
                debug!(
                    path = %self.path,
                    error = %error,
                    "usb serial write failed; transport is no longer usable"
                );
                self.closed.send_replace(true);
                Err(error)
            }
        }
    }

    async fn receive_locked(
        &self,
        slot: &mut SerialStreamSlot,
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        debug!(
            path = %self.path,
            timeout_ms = timeout.as_millis(),
            "usb serial receive requested"
        );
        let stream = slot.as_mut().ok_or(TransportError::Closed)?;

        let response = self
            .until_closed(async {
                tokio::time::timeout(timeout, read_focus_reply(stream))
                    .await
                    .map_err(|_| TransportError::Timeout {
                        timeout_ms: duration_ms(timeout),
                    })?
            })
            .await?;

        trace!(
            path = %self.path,
            response_len = response.len(),
            payload = %String::from_utf8_lossy(&response),
            "usb serial response received"
        );

        Ok(response)
    }
}

impl Drop for UsbSerialTransport {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.get_mut().take() {
            drop(self.release.release(&self.path, stream));
        }
    }
}

#[async_trait]
impl Transport for UsbSerialTransport {
    fn name(&self) -> &'static str {
        "USB Serial"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        let mut slot = self.stream.lock().await;
        self.send_locked(&mut slot, data).await
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        let mut slot = self.stream.lock().await;
        self.receive_locked(&mut slot, timeout).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        let mut slot = self.stream.lock().await;
        // No drain between the two halves: the reply cannot arrive before the
        // device has read the whole command, so the reply is the barrier.
        self.send_locked(&mut slot, data).await?;
        self.receive_locked(&mut slot, timeout).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        let deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
        // Wakes an operation that is still waiting on the port, so it hands
        // the lock back instead of holding close behind a stalled write.
        self.closed.send_replace(true);

        let Ok(mut slot) = tokio::time::timeout_at(deadline, self.stream.lock()).await else {
            warn!(
                path = %self.path,
                bound_ms = CLOSE_TIMEOUT.as_millis(),
                "usb serial close could not reclaim the port in time; it is released when the transport drops"
            );
            return Ok(());
        };
        let Some(stream) = slot.take() else {
            return Ok(());
        };
        drop(slot);

        let released = self.release.release(&self.path, stream);
        if tokio::time::timeout_at(deadline, released).await.is_err() {
            debug!(
                path = %self.path,
                bound_ms = CLOSE_TIMEOUT.as_millis(),
                "usb serial port is still draining on close; finishing in the background"
            );
        }
        Ok(())
    }
}

/// Read one Focus reply: every line up to a lone `.` terminator line.
async fn read_focus_reply(stream: &mut Box<dyn AsyncSerialIo>) -> Result<Vec<u8>, TransportError> {
    let mut payload = Vec::new();
    let mut line = Vec::new();

    loop {
        let mut byte = [0_u8; 1];
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|error| map_io_error(&error, "read"))?;
        if read == 0 {
            return Err(TransportError::Closed);
        }

        line.push(byte[0]);
        if byte[0] != b'\n' {
            continue;
        }

        if is_terminator_line(&line) {
            break;
        }

        let stripped = strip_line_endings(&line);
        if !stripped.is_empty() {
            if !payload.is_empty() {
                payload.push(b'\n');
            }
            payload.extend_from_slice(stripped);
        }
        line.clear();
    }

    Ok(payload)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn map_serial_open_error(error: &tokio_serial::Error, path: &str) -> TransportError {
    let detail = format!("failed to open serial port {path}: {error}");
    match error.kind() {
        tokio_serial::ErrorKind::NoDevice => TransportError::NotFound { detail },
        tokio_serial::ErrorKind::Io(kind) => map_io_error_kind(kind, detail),
        _ => TransportError::IoError { detail },
    }
}

fn map_io_error(error: &std::io::Error, operation: &str) -> TransportError {
    let detail = format!("serial {operation} failed: {error}");
    map_io_error_kind(error.kind(), detail)
}

fn map_io_error_kind(kind: std::io::ErrorKind, detail: String) -> TransportError {
    match kind {
        std::io::ErrorKind::PermissionDenied => TransportError::PermissionDenied { detail },
        std::io::ErrorKind::TimedOut => TransportError::Timeout { timeout_ms: 0 },
        std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::UnexpectedEof
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::NotFound => TransportError::NotFound { detail },
        _ => TransportError::IoError { detail },
    }
}

fn strip_line_endings(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    &line[..end]
}

fn is_terminator_line(line: &[u8]) -> bool {
    let stripped = strip_line_endings(line);
    let Some(start) = stripped.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    let Some(end) = stripped
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
    else {
        return false;
    };

    stripped[start..=end] == *b"."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_open_classification_uses_error_kind_not_message() {
        let misleading = tokio_serial::Error::new(
            tokio_serial::ErrorKind::Unknown,
            "permission denied and device not found",
        );
        assert!(matches!(
            map_serial_open_error(&misleading, "COM-test"),
            TransportError::IoError { .. }
        ));

        let permission = tokio_serial::Error::new(
            tokio_serial::ErrorKind::Io(std::io::ErrorKind::PermissionDenied),
            "harmless display text",
        );
        assert!(matches!(
            map_serial_open_error(&permission, "COM-test"),
            TransportError::PermissionDenied { .. }
        ));
    }
}
