//! SMBus transport framing and Linux transport support.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::transport::{Transport, TransportError};

const SMBUS_OP_WRITE_WORD_DATA: u8 = 0x01;
const SMBUS_OP_WRITE_BYTE_DATA: u8 = 0x02;
const SMBUS_OP_READ_BYTE_DATA: u8 = 0x03;
const SMBUS_OP_WRITE_BLOCK_DATA: u8 = 0x04;
const SMBUS_OP_DELAY: u8 = 0x05;

/// One framed SMBus operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmBusOperation {
    /// `i2c_smbus_write_word_data(register, value)`.
    WriteWordData {
        /// SMBus command/register byte.
        register: u8,
        /// 16-bit payload.
        value: u16,
    },
    /// `i2c_smbus_write_byte_data(register, value)`.
    WriteByteData {
        /// SMBus command/register byte.
        register: u8,
        /// Byte payload.
        value: u8,
    },
    /// `i2c_smbus_read_byte_data(register)`.
    ReadByteData {
        /// SMBus command/register byte.
        register: u8,
    },
    /// `i2c_smbus_write_block_data(register, data)`.
    WriteBlockData {
        /// SMBus command/register byte.
        register: u8,
        /// Block payload.
        data: Vec<u8>,
    },
    /// Delay between bus operations.
    Delay {
        /// Delay duration.
        duration: Duration,
    },
}

/// Serialize a sequence of SMBus operations into transport bytes.
///
/// # Errors
///
/// Returns [`TransportError`] when one operation cannot be represented.
pub fn encode_operations(operations: &[SmBusOperation]) -> Result<Vec<u8>, TransportError> {
    let mut encoded = Vec::new();

    for operation in operations {
        match operation {
            SmBusOperation::WriteWordData { register, value } => {
                encoded.push(SMBUS_OP_WRITE_WORD_DATA);
                encoded.push(*register);
                encoded.extend_from_slice(&value.to_le_bytes());
            }
            SmBusOperation::WriteByteData { register, value } => {
                encoded.push(SMBUS_OP_WRITE_BYTE_DATA);
                encoded.push(*register);
                encoded.push(*value);
            }
            SmBusOperation::ReadByteData { register } => {
                encoded.push(SMBUS_OP_READ_BYTE_DATA);
                encoded.push(*register);
            }
            SmBusOperation::WriteBlockData { register, data } => {
                let len = u8::try_from(data.len()).map_err(|_| TransportError::IoError {
                    detail: "SMBus block payload exceeds u8 length".to_owned(),
                })?;
                encoded.push(SMBUS_OP_WRITE_BLOCK_DATA);
                encoded.push(*register);
                encoded.push(len);
                encoded.extend_from_slice(data);
            }
            SmBusOperation::Delay { duration } => {
                let millis =
                    u16::try_from(duration.as_millis()).map_err(|_| TransportError::IoError {
                        detail: "SMBus delay exceeds u16 millisecond range".to_owned(),
                    })?;
                encoded.push(SMBUS_OP_DELAY);
                encoded.extend_from_slice(&millis.to_le_bytes());
            }
        }
    }

    Ok(encoded)
}

/// Decode one framed SMBus command sequence.
///
/// # Errors
///
/// Returns [`TransportError`] when the byte stream is malformed.
pub fn decode_operations(data: &[u8]) -> Result<Vec<SmBusOperation>, TransportError> {
    let mut operations = Vec::new();
    let mut cursor = 0_usize;

    while cursor < data.len() {
        let opcode = data[cursor];
        cursor += 1;

        match opcode {
            SMBUS_OP_WRITE_WORD_DATA => {
                let register = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "SMBus write-word frame missing register".to_owned(),
                })?;
                let bytes =
                    data.get(cursor + 1..cursor + 3)
                        .ok_or_else(|| TransportError::IoError {
                            detail: "SMBus write-word frame missing value".to_owned(),
                        })?;
                operations.push(SmBusOperation::WriteWordData {
                    register,
                    value: u16::from_le_bytes([bytes[0], bytes[1]]),
                });
                cursor += 3;
            }
            SMBUS_OP_WRITE_BYTE_DATA => {
                let register = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "SMBus write-byte frame missing register".to_owned(),
                })?;
                let value = *data
                    .get(cursor + 1)
                    .ok_or_else(|| TransportError::IoError {
                        detail: "SMBus write-byte frame missing value".to_owned(),
                    })?;
                operations.push(SmBusOperation::WriteByteData { register, value });
                cursor += 2;
            }
            SMBUS_OP_READ_BYTE_DATA => {
                let register = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "SMBus read-byte frame missing register".to_owned(),
                })?;
                operations.push(SmBusOperation::ReadByteData { register });
                cursor += 1;
            }
            SMBUS_OP_WRITE_BLOCK_DATA => {
                let register = *data.get(cursor).ok_or_else(|| TransportError::IoError {
                    detail: "SMBus write-block frame missing register".to_owned(),
                })?;
                let len =
                    usize::from(
                        *data
                            .get(cursor + 1)
                            .ok_or_else(|| TransportError::IoError {
                                detail: "SMBus write-block frame missing length".to_owned(),
                            })?,
                    );
                let payload = data.get(cursor + 2..cursor + 2 + len).ok_or_else(|| {
                    TransportError::IoError {
                        detail: "SMBus write-block frame truncated".to_owned(),
                    }
                })?;
                operations.push(SmBusOperation::WriteBlockData {
                    register,
                    data: payload.to_vec(),
                });
                cursor += 2 + len;
            }
            SMBUS_OP_DELAY => {
                let bytes =
                    data.get(cursor..cursor + 2)
                        .ok_or_else(|| TransportError::IoError {
                            detail: "SMBus delay frame missing milliseconds".to_owned(),
                        })?;
                operations.push(SmBusOperation::Delay {
                    duration: Duration::from_millis(u64::from(u16::from_le_bytes([
                        bytes[0], bytes[1],
                    ]))),
                });
                cursor += 2;
            }
            other => {
                return Err(TransportError::IoError {
                    detail: format!("unknown SMBus opcode 0x{other:02X}"),
                });
            }
        }
    }

    Ok(operations)
}

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "linux")]
use i2cdev::core::I2CDevice;
#[cfg(target_os = "linux")]
use i2cdev::linux::{LinuxI2CDevice, LinuxI2CError};

/// Linux SMBus transport backed by `/dev/i2c-*`.
#[cfg(target_os = "linux")]
pub struct SmBusTransport {
    path: String,
    address: u16,
    device: Arc<Mutex<LinuxI2CDevice>>,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
}

#[cfg(target_os = "linux")]
impl SmBusTransport {
    /// Open one SMBus slave on one Linux I2C bus path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the device path cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P, address: u16) -> Result<Self, TransportError> {
        let path_string = path.as_ref().display().to_string();
        let device = LinuxI2CDevice::new(path.as_ref(), address)
            .map_err(|error| map_linux_i2c_error(&path_string, address, error))?;

        Ok(Self {
            path: path_string,
            address,
            device: Arc::new(Mutex::new(device)),
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }

    fn execute_operations_locked(
        device: &Mutex<LinuxI2CDevice>,
        path: &str,
        address: u16,
        operations: &[SmBusOperation],
    ) -> Result<Vec<u8>, TransportError> {
        let mut device = device.lock().map_err(|_| TransportError::IoError {
            detail: "SMBus device lock poisoned".to_owned(),
        })?;
        let mut reads = Vec::new();

        for operation in operations {
            match operation {
                SmBusOperation::WriteWordData { register, value } => device
                    .smbus_write_word_data(*register, *value)
                    .map_err(|error| map_linux_i2c_error(path, address, error))?,
                SmBusOperation::WriteByteData { register, value } => device
                    .smbus_write_byte_data(*register, *value)
                    .map_err(|error| map_linux_i2c_error(path, address, error))?,
                SmBusOperation::ReadByteData { register } => reads.push(
                    device
                        .smbus_read_byte_data(*register)
                        .map_err(|error| map_linux_i2c_error(path, address, error))?,
                ),
                SmBusOperation::WriteBlockData { register, data } => device
                    .smbus_write_block_data(*register, data)
                    .map_err(|error| map_linux_i2c_error(path, address, error))?,
                SmBusOperation::Delay { duration } => std::thread::sleep(*duration),
            }
        }

        Ok(reads)
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl Transport for SmBusTransport {
    fn name(&self) -> &'static str {
        "Linux SMBus"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        let operations = decode_operations(data)?;
        let _guard = self.op_lock.lock().await;
        let device = Arc::clone(&self.device);
        let path = self.path.clone();
        let address = self.address;

        tokio::task::spawn_blocking(move || {
            Self::execute_operations_locked(device.as_ref(), &path, address, &operations)
        })
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("SMBus send task failed: {error}"),
        })??;

        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn send_receive(
        &self,
        data: &[u8],
        _timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        let operations = decode_operations(data)?;
        let _guard = self.op_lock.lock().await;
        let device = Arc::clone(&self.device);
        let path = self.path.clone();
        let address = self.address;

        tokio::task::spawn_blocking(move || {
            Self::execute_operations_locked(device.as_ref(), &path, address, &operations)
        })
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("SMBus send/receive task failed: {error}"),
        })?
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub struct SmBusTransport;

#[cfg(not(target_os = "linux"))]
impl SmBusTransport {
    /// SMBus transport is only available on Linux.
    pub fn open(_path: &str, _address: u16) -> Result<Self, TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux".to_owned(),
        })
    }
}

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl Transport for SmBusTransport {
    fn name(&self) -> &'static str {
        "Linux SMBus"
    }

    async fn send(&self, _data: &[u8]) -> Result<(), TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux".to_owned(),
        })
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn map_linux_i2c_error(path: &str, address: u16, error: LinuxI2CError) -> TransportError {
    let detail = format!("{error} (path={path}, address=0x{address:02X})");
    let lowered = detail.to_ascii_lowercase();

    if lowered.contains("permission") || lowered.contains("denied") {
        return TransportError::PermissionDenied { detail };
    }

    if lowered.contains("no such file") || lowered.contains("not found") {
        return TransportError::NotFound { detail };
    }

    TransportError::IoError { detail }
}
