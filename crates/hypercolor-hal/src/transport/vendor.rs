//! USB vendor-control transport framing and execution.

use std::convert::TryFrom;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

#[cfg(any(target_os = "android", target_os = "linux", target_os = "macos"))]
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};

use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const VENDOR_OP_WRITE: u8 = 0x01;
const VENDOR_OP_READ: u8 = 0x02;
const VENDOR_OP_DELAY: u8 = 0x03;

/// One vendor-control operation encoded into one protocol command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorControlOperation {
    /// Vendor-specific control write.
    Write {
        /// Setup packet `bRequest`.
        request: u8,
        /// Setup packet `wValue`.
        value: u16,
        /// Setup packet `wIndex`.
        index: u16,
        /// Payload bytes written to the device.
        data: Vec<u8>,
    },

    /// Vendor-specific control read.
    Read {
        /// Setup packet `bRequest`.
        request: u8,
        /// Setup packet `wValue`.
        value: u16,
        /// Setup packet `wIndex`.
        index: u16,
        /// Response length to read.
        length: u16,
    },

    /// Delay between control transfers.
    Delay {
        /// Delay duration.
        duration: Duration,
    },
}

/// Serialize vendor-control operations into transport bytes.
///
/// # Errors
///
/// Returns [`TransportError`] when one operation cannot be represented.
pub fn encode_operations(operations: &[VendorControlOperation]) -> Result<Vec<u8>, TransportError> {
    let mut encoded = Vec::new();

    for operation in operations {
        match operation {
            VendorControlOperation::Write {
                request,
                value,
                index,
                data,
            } => {
                let length = u16::try_from(data.len()).map_err(|_| TransportError::IoError {
                    detail: "vendor write payload exceeds u16 length".to_owned(),
                })?;
                encoded.push(VENDOR_OP_WRITE);
                encoded.push(*request);
                encoded.extend_from_slice(&value.to_le_bytes());
                encoded.extend_from_slice(&index.to_le_bytes());
                encoded.extend_from_slice(&length.to_le_bytes());
                encoded.extend_from_slice(data);
            }
            VendorControlOperation::Read {
                request,
                value,
                index,
                length,
            } => {
                encoded.push(VENDOR_OP_READ);
                encoded.push(*request);
                encoded.extend_from_slice(&value.to_le_bytes());
                encoded.extend_from_slice(&index.to_le_bytes());
                encoded.extend_from_slice(&length.to_le_bytes());
            }
            VendorControlOperation::Delay { duration } => {
                let millis =
                    u16::try_from(duration.as_millis()).map_err(|_| TransportError::IoError {
                        detail: "vendor delay exceeds u16 millisecond range".to_owned(),
                    })?;
                encoded.push(VENDOR_OP_DELAY);
                encoded.extend_from_slice(&millis.to_le_bytes());
            }
        }
    }

    Ok(encoded)
}

/// Decode vendor-control operations from transport bytes.
///
/// # Errors
///
/// Returns [`TransportError`] when the byte stream is malformed.
pub fn decode_operations(data: &[u8]) -> Result<Vec<VendorControlOperation>, TransportError> {
    let mut operations = Vec::new();
    let mut cursor = 0_usize;

    while cursor < data.len() {
        let opcode = data[cursor];
        cursor += 1;

        match opcode {
            VENDOR_OP_WRITE => {
                let request = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "vendor write frame missing request".to_owned(),
                })?;
                let value = read_u16(data, cursor + 1, "vendor write frame missing wValue")?;
                let index = read_u16(data, cursor + 3, "vendor write frame missing wIndex")?;
                let length = usize::from(read_u16(
                    data,
                    cursor + 5,
                    "vendor write frame missing payload length",
                )?);
                let payload = data.get(cursor + 7..cursor + 7 + length).ok_or_else(|| {
                    TransportError::IoError {
                        detail: "vendor write frame truncated".to_owned(),
                    }
                })?;
                operations.push(VendorControlOperation::Write {
                    request,
                    value,
                    index,
                    data: payload.to_vec(),
                });
                cursor += 7 + length;
            }
            VENDOR_OP_READ => {
                let request = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "vendor read frame missing request".to_owned(),
                })?;
                let value = read_u16(data, cursor + 1, "vendor read frame missing wValue")?;
                let index = read_u16(data, cursor + 3, "vendor read frame missing wIndex")?;
                let length = read_u16(data, cursor + 5, "vendor read frame missing length")?;
                operations.push(VendorControlOperation::Read {
                    request,
                    value,
                    index,
                    length,
                });
                cursor += 7;
            }
            VENDOR_OP_DELAY => {
                let millis = read_u16(data, cursor, "vendor delay frame missing milliseconds")?;
                operations.push(VendorControlOperation::Delay {
                    duration: Duration::from_millis(u64::from(millis)),
                });
                cursor += 2;
            }
            other => {
                return Err(TransportError::IoError {
                    detail: format!("unknown vendor-control opcode 0x{other:02X}"),
                });
            }
        }
    }

    Ok(operations)
}

fn read_u16(data: &[u8], offset: usize, detail: &'static str) -> Result<u16, TransportError> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| TransportError::IoError {
            detail: detail.to_owned(),
        })?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// USB vendor-specific control transfer transport.
pub struct UsbVendorTransport {
    #[allow(dead_code, reason = "device handle keeps the USB device open")]
    device: nusb::Device,
    closed: AtomicBool,
    pending_response: StdMutex<Option<Vec<u8>>>,
    op_lock: tokio::sync::Mutex<()>,
}

impl UsbVendorTransport {
    /// Create a vendor control transport wrapper around one opened USB device.
    #[must_use]
    pub fn new(device: nusb::Device) -> Self {
        Self {
            device,
            closed: AtomicBool::new(false),
            pending_response: StdMutex::new(None),
            op_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }

        Ok(())
    }

    fn take_pending_response(&self) -> Result<Vec<u8>, TransportError> {
        let mut pending = self
            .pending_response
            .lock()
            .map_err(|_| TransportError::IoError {
                detail: "vendor transport response state lock poisoned".to_owned(),
            })?;

        pending.take().ok_or(TransportError::Timeout {
            timeout_ms: u64::try_from(DEFAULT_IO_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
        })
    }

    fn store_pending_response(&self, response: Option<Vec<u8>>) -> Result<(), TransportError> {
        let mut pending = self
            .pending_response
            .lock()
            .map_err(|_| TransportError::IoError {
                detail: "vendor transport response state lock poisoned".to_owned(),
            })?;
        *pending = response;
        Ok(())
    }

    #[cfg(any(target_os = "android", target_os = "linux", target_os = "macos"))]
    async fn execute_operations(&self, data: &[u8]) -> Result<Option<Vec<u8>>, TransportError> {
        let operations = decode_operations(data)?;
        let mut last_response = None;

        for operation in operations {
            match operation {
                VendorControlOperation::Write {
                    request,
                    value,
                    index,
                    data,
                } => {
                    self.device
                        .control_out(
                            ControlOut {
                                control_type: ControlType::Vendor,
                                recipient: Recipient::Device,
                                request,
                                value,
                                index,
                                data: &data,
                            },
                            DEFAULT_IO_TIMEOUT,
                        )
                        .await
                        .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))?;
                    last_response = None;
                }
                VendorControlOperation::Read {
                    request,
                    value,
                    index,
                    length,
                } => {
                    let response = self
                        .device
                        .control_in(
                            ControlIn {
                                control_type: ControlType::Vendor,
                                recipient: Recipient::Device,
                                request,
                                value,
                                index,
                                length,
                            },
                            DEFAULT_IO_TIMEOUT,
                        )
                        .await
                        .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))?;
                    last_response = Some(response);
                }
                VendorControlOperation::Delay { duration } => {
                    tokio::time::sleep(duration).await;
                }
            }
        }

        Ok(last_response)
    }

    #[cfg(not(any(target_os = "android", target_os = "linux", target_os = "macos")))]
    #[allow(clippy::unused_async)]
    async fn execute_operations(&self, _data: &[u8]) -> Result<Option<Vec<u8>>, TransportError> {
        Err(TransportError::IoError {
            detail: "USB vendor transport is not supported on this platform".to_owned(),
        })
    }
}

#[async_trait]
impl Transport for UsbVendorTransport {
    fn name(&self) -> &'static str {
        "USB Vendor Control"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let response = self.execute_operations(data).await?;
        self.store_pending_response(response)
    }

    async fn receive(&self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        self.take_pending_response()
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let response = self.execute_operations(data).await?;
        if let Some(response) = response {
            return Ok(response);
        }

        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        self.store_pending_response(None)
    }
}

#[cfg(any(target_os = "android", target_os = "linux", target_os = "macos"))]
fn map_transfer_error(error: TransferError, timeout: Duration) -> TransportError {
    match error {
        TransferError::Cancelled => TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        },
        TransferError::Disconnected => TransportError::NotFound {
            detail: error.to_string(),
        },
        _ => TransportError::IoError {
            detail: error.to_string(),
        },
    }
}
