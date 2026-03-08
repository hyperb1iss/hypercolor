//! USB CDC-ACM serial transport for line-oriented protocols.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialPortType};
use tracing::{debug, trace};

use crate::transport::{Transport, TransportError};

trait AsyncSerialIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncSerialIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

struct UsbSerialTransportInner {
    stream: Box<dyn AsyncSerialIo>,
}

/// USB CDC-ACM serial transport used by Focus-class devices.
pub struct UsbSerialTransport {
    path: String,
    inner: tokio::sync::Mutex<UsbSerialTransportInner>,
    closed: AtomicBool,
}

impl UsbSerialTransport {
    /// Discover and open a serial port for one USB device.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] if no matching serial port can be found or
    /// the port cannot be opened.
    pub fn open(
        vendor_id: u16,
        product_id: u16,
        baud_rate: u32,
        serial: Option<&str>,
    ) -> Result<Self, TransportError> {
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

        Ok(Self::from_stream(path, stream))
    }

    /// Construct a serial transport from an existing async stream.
    #[must_use]
    pub fn from_stream(
        path: impl Into<String>,
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self {
            path: path.into(),
            inner: tokio::sync::Mutex::new(UsbSerialTransportInner {
                stream: Box::new(stream),
            }),
            closed: AtomicBool::new(false),
        }
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }

    async fn send_locked(
        &self,
        inner: &mut UsbSerialTransportInner,
        data: &[u8],
    ) -> Result<(), TransportError> {
        trace!(
            path = %self.path,
            packet_len = data.len(),
            payload = %String::from_utf8_lossy(data),
            "usb serial send"
        );
        inner
            .stream
            .write_all(data)
            .await
            .map_err(|error| map_io_error(&error, "write"))?;
        inner
            .stream
            .flush()
            .await
            .map_err(|error| map_io_error(&error, "flush"))
    }

    async fn receive_locked(
        &self,
        inner: &mut UsbSerialTransportInner,
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        debug!(
            path = %self.path,
            timeout_ms = timeout.as_millis(),
            "usb serial receive requested"
        );

        let response = tokio::time::timeout(timeout, async {
            let mut payload = Vec::new();
            let mut line = Vec::new();

            loop {
                let mut byte = [0_u8; 1];
                let read = inner
                    .stream
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
        })
        .await
        .map_err(|_| TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })??;

        trace!(
            path = %self.path,
            response_len = response.len(),
            payload = %String::from_utf8_lossy(&response),
            "usb serial response received"
        );

        Ok(response)
    }
}

#[async_trait]
impl Transport for UsbSerialTransport {
    fn name(&self) -> &'static str {
        "USB Serial"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        let mut inner = self.inner.lock().await;
        self.send_locked(&mut inner, data).await
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        let mut inner = self.inner.lock().await;
        self.receive_locked(&mut inner, timeout).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        let mut inner = self.inner.lock().await;
        self.send_locked(&mut inner, data).await?;
        self.receive_locked(&mut inner, timeout).await
    }
    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn map_serial_open_error(error: &tokio_serial::Error, path: &str) -> TransportError {
    let detail = format!("failed to open serial port {path}: {error}");
    if detail.to_ascii_lowercase().contains("permission") {
        TransportError::PermissionDenied { detail }
    } else {
        TransportError::IoError { detail }
    }
}

fn map_io_error(error: &std::io::Error, operation: &str) -> TransportError {
    let detail = format!("serial {operation} failed: {error}");
    match error.kind() {
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

    stripped[start..=end] == [b'.']
}
