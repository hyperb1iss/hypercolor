//! `SMBus` transport framing and transport support for Linux and Windows.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::transport::{Transport, TransportError};

const SMBUS_OP_WRITE_WORD_DATA: u8 = 0x01;
const SMBUS_OP_WRITE_BYTE_DATA: u8 = 0x02;
const SMBUS_OP_READ_BYTE_DATA: u8 = 0x03;
const SMBUS_OP_WRITE_BLOCK_DATA: u8 = 0x04;
const SMBUS_OP_DELAY: u8 = 0x05;

/// Shared transaction lock for every device on one physical `SMBus` bus.
#[derive(Clone, Default)]
pub struct SmBusBusArbiter {
    transaction_lock: Arc<tokio::sync::Mutex<()>>,
}

static SMBUS_BUS_ARBITERS: LazyLock<Mutex<HashMap<String, SmBusBusArbiter>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl SmBusBusArbiter {
    /// Resolve the process-wide transaction arbiter for one physical bus.
    #[must_use]
    pub fn for_bus(bus_path: &str) -> Self {
        SMBUS_BUS_ARBITERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(bus_path.to_owned())
            .or_default()
            .clone()
    }

    /// Acquire exclusive ownership of one physical bus transaction segment.
    pub async fn acquire_transaction(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.transaction_lock).lock_owned().await
    }

    /// Run one blocking transaction while retaining physical bus ownership.
    ///
    /// The owned guard moves into the blocking task so cancelling the async
    /// waiter cannot expose a bus transaction that is still in progress.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the transaction fails or its blocking
    /// task cannot be joined.
    pub async fn run_blocking<R, F>(&self, operation: F) -> Result<R, TransportError>
    where
        R: Send + 'static,
        F: FnOnce() -> Result<R, TransportError> + Send + 'static,
    {
        let transaction = self.acquire_transaction().await;
        tokio::task::spawn_blocking(move || {
            let _transaction = transaction;
            operation()
        })
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("SMBus transaction task failed: {error}"),
        })?
    }
}

/// One framed `SMBus` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmBusOperation {
    /// `i2c_smbus_write_word_data(register, value)`.
    WriteWordData {
        /// `SMBus` command/register byte.
        register: u8,
        /// 16-bit payload.
        value: u16,
    },
    /// `i2c_smbus_write_byte_data(register, value)`.
    WriteByteData {
        /// `SMBus` command/register byte.
        register: u8,
        /// Byte payload.
        value: u8,
    },
    /// `i2c_smbus_read_byte_data(register)`.
    ReadByteData {
        /// `SMBus` command/register byte.
        register: u8,
    },
    /// `i2c_smbus_write_block_data(register, data)`.
    WriteBlockData {
        /// `SMBus` command/register byte.
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

/// Serialize a sequence of `SMBus` operations into transport bytes.
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

/// Decode one framed `SMBus` command sequence.
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
use i2cdev::core::I2CDevice;
#[cfg(target_os = "linux")]
use i2cdev::linux::{LinuxI2CDevice, LinuxI2CError};

#[cfg(target_os = "windows")]
use hypercolor_windows_pawnio::{
    PawnIoError, PawnIoErrorKind, SmBusBatchOperation, SmBusBlockData, SmBusDirection,
    SmBusTransaction, WindowsSmBusBus, WindowsSmBusBusInfo, enumerate_smbus_buses, open_smbus_bus,
};

/// Linux `SMBus` transport backed by `/dev/i2c-*`.
#[cfg(target_os = "linux")]
pub struct SmBusTransport {
    path: String,
    address: u16,
    device: Arc<Mutex<LinuxI2CDevice>>,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
    bus_arbiter: SmBusBusArbiter,
}

#[cfg(target_os = "linux")]
impl SmBusTransport {
    /// Open one `SMBus` slave on one Linux I2C bus path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the device path cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P, address: u16) -> Result<Self, TransportError> {
        let bus_arbiter = SmBusBusArbiter::for_bus(&path.as_ref().display().to_string());
        Self::open_with_arbiter(path, address, bus_arbiter)
    }

    /// Open one `SMBus` slave with a transaction arbiter shared by its bus.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the device path cannot be opened.
    pub fn open_with_arbiter<P: AsRef<Path>>(
        path: P,
        address: u16,
        bus_arbiter: SmBusBusArbiter,
    ) -> Result<Self, TransportError> {
        let path_string = path.as_ref().display().to_string();
        let device = LinuxI2CDevice::new(path.as_ref(), address)
            .map_err(|error| map_linux_i2c_error(&path_string, address, &error))?;

        Ok(Self {
            path: path_string,
            address,
            device: Arc::new(Mutex::new(device)),
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
            bus_arbiter,
        })
    }

    /// Probe whether one `SMBus` address responds on one Linux I2C bus.
    ///
    /// This first attempts a quick-write probe, then falls back to simple
    /// byte reads because some ENE devices reject quick writes while still
    /// responding to read transactions.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the bus path itself cannot be opened.
    pub async fn probe_presence<P: AsRef<Path>>(
        path: P,
        address: u16,
    ) -> Result<bool, TransportError> {
        let path_string = path.as_ref().display().to_string();
        let bus_arbiter = SmBusBusArbiter::for_bus(&path_string);
        bus_arbiter
            .run_blocking(move || {
                let mut device = LinuxI2CDevice::new(&path_string, address)
                    .map_err(|error| map_linux_i2c_error(&path_string, address, &error))?;

                if device.smbus_write_quick(false).is_ok() {
                    return Ok(true);
                }

                if device.smbus_read_byte().is_ok() {
                    return Ok(true);
                }

                Ok(device.smbus_read_byte_data(0x00).is_ok())
            })
            .await
    }

    /// Probe whether one `SMBus` address acknowledges a quick-write transaction.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the bus path itself cannot be opened.
    pub async fn probe_quick_write<P: AsRef<Path>>(
        path: P,
        address: u16,
    ) -> Result<bool, TransportError> {
        let path_string = path.as_ref().display().to_string();
        let bus_arbiter = SmBusBusArbiter::for_bus(&path_string);
        bus_arbiter
            .run_blocking(move || {
                let mut device = LinuxI2CDevice::new(&path_string, address)
                    .map_err(|error| map_linux_i2c_error(&path_string, address, &error))?;

                Ok(device.smbus_write_quick(false).is_ok())
            })
            .await
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }

    fn execute_batch_locked(
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
                    .map_err(|error| map_linux_i2c_error(path, address, &error))?,
                SmBusOperation::WriteByteData { register, value } => device
                    .smbus_write_byte_data(*register, *value)
                    .map_err(|error| map_linux_i2c_error(path, address, &error))?,
                SmBusOperation::ReadByteData { register } => reads.push(
                    device
                        .smbus_read_byte_data(*register)
                        .map_err(|error| map_linux_i2c_error(path, address, &error))?,
                ),
                SmBusOperation::WriteBlockData { register, data } => device
                    .smbus_write_block_data(*register, data)
                    .map_err(|error| map_linux_i2c_error(path, address, &error))?,
                SmBusOperation::Delay { duration } => std::thread::sleep(*duration),
            }
        }

        Ok(reads)
    }

    /// Run one device transaction as a single blocking segment,
    /// inter-operation delays included.
    ///
    /// The delays run inside the closure rather than between fragments so the
    /// ENE address-set and its data-write stay inside one bus-arbiter hold.
    async fn execute_operations(
        &self,
        operations: Vec<SmBusOperation>,
    ) -> Result<Vec<u8>, TransportError> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }

        let device = Arc::clone(&self.device);
        let path = self.path.clone();
        let address = self.address;

        self.bus_arbiter
            .run_blocking(move || {
                Self::execute_batch_locked(device.as_ref(), &path, address, &operations)
            })
            .await
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
        self.execute_operations(operations).await?;

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
        self.execute_operations(operations).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

/// Windows `SMBus` transport backed by PawnIO modules.
#[cfg(target_os = "windows")]
pub struct SmBusTransport {
    path: String,
    address: u16,
    bus: Arc<Mutex<WindowsSmBusBus>>,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
    bus_arbiter: SmBusBusArbiter,
}

#[cfg(target_os = "windows")]
impl SmBusTransport {
    /// Open one `SMBus` slave on one PawnIO bus path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when PawnIO cannot open the bus.
    pub fn open(path: &str, address: u16) -> Result<Self, TransportError> {
        Self::open_with_arbiter(path, address, SmBusBusArbiter::for_bus(path))
    }

    /// Open one `SMBus` slave with a transaction arbiter shared by its bus.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when PawnIO cannot open the bus.
    pub fn open_with_arbiter(
        path: &str,
        address: u16,
        bus_arbiter: SmBusBusArbiter,
    ) -> Result<Self, TransportError> {
        let bus = open_smbus_bus(path).map_err(map_windows_pawnio_error)?;

        Ok(Self {
            path: path.to_owned(),
            address,
            bus: Arc::new(Mutex::new(bus)),
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
            bus_arbiter,
        })
    }

    /// Probe whether one `SMBus` address responds on one PawnIO bus.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the bus path itself cannot be opened.
    pub async fn probe_presence(path: &str, address: u16) -> Result<bool, TransportError> {
        let address = u8_address(address)?;
        let bus_arbiter = SmBusBusArbiter::for_bus(path);
        let path = path.to_owned();
        bus_arbiter
            .run_blocking(move || {
                let bus = open_smbus_bus(&path).map_err(map_windows_pawnio_error)?;
                bus.probe_presence(address)
                    .map_err(map_windows_pawnio_error)
            })
            .await
    }

    /// Probe whether one `SMBus` address acknowledges a quick-write transaction.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the bus path itself cannot be opened.
    pub async fn probe_quick_write(path: &str, address: u16) -> Result<bool, TransportError> {
        let address = u8_address(address)?;
        let bus_arbiter = SmBusBusArbiter::for_bus(path);
        let path = path.to_owned();
        bus_arbiter
            .run_blocking(move || {
                let bus = open_smbus_bus(&path).map_err(map_windows_pawnio_error)?;
                bus.probe_quick_write(address)
                    .map_err(map_windows_pawnio_error)
            })
            .await
    }

    /// Enumerate PawnIO SMBus buses.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when PawnIO cannot be loaded.
    pub fn enumerate_buses() -> Result<Vec<WindowsSmBusBusInfo>, TransportError> {
        enumerate_smbus_buses().map_err(map_windows_pawnio_error)
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }

    fn execute_batch_locked(
        bus: &Mutex<WindowsSmBusBus>,
        path: &str,
        address: u16,
        operations: &[SmBusOperation],
    ) -> Result<Vec<u8>, TransportError> {
        let address = u8_address(address)?;
        let bus = bus.lock().map_err(|_| TransportError::IoError {
            detail: "SMBus bus lock poisoned".to_owned(),
        })?;
        let mut reads = Vec::new();

        let mut batch = operations
            .iter()
            .map(|operation| match operation {
                SmBusOperation::WriteWordData { register, value } => {
                    Ok(SmBusBatchOperation::Transfer {
                        direction: SmBusDirection::Write,
                        command: *register,
                        transaction: SmBusTransaction::WordData { value: *value },
                    })
                }
                SmBusOperation::WriteByteData { register, value } => {
                    Ok(SmBusBatchOperation::Transfer {
                        direction: SmBusDirection::Write,
                        command: *register,
                        transaction: SmBusTransaction::ByteData { value: *value },
                    })
                }
                SmBusOperation::ReadByteData { register } => Ok(SmBusBatchOperation::Transfer {
                    direction: SmBusDirection::Read,
                    command: *register,
                    transaction: SmBusTransaction::ByteData { value: 0 },
                }),
                SmBusOperation::WriteBlockData { register, data } => {
                    Ok(SmBusBatchOperation::Transfer {
                        direction: SmBusDirection::Write,
                        command: *register,
                        transaction: SmBusTransaction::BlockData {
                            data: SmBusBlockData::new(data).map_err(map_windows_pawnio_error)?,
                        },
                    })
                }
                SmBusOperation::Delay { duration } => Ok(SmBusBatchOperation::Delay {
                    duration: *duration,
                }),
            })
            .collect::<Result<Vec<_>, TransportError>>()?;

        bus.smbus_xfer_batch(address, &mut batch)
            .map_err(|error| map_windows_smbus_io_error(path, address, error))?;

        for operation in batch {
            if let SmBusBatchOperation::Transfer {
                direction: SmBusDirection::Read,
                transaction: SmBusTransaction::ByteData { value },
                ..
            } = operation
            {
                reads.push(value);
            }
        }

        Ok(reads)
    }

    /// Run one device transaction as a single batch, inter-operation delays
    /// included.
    ///
    /// The delays are carried in the batch rather than awaited between
    /// fragments so the ENE address-set and its data-write stay inside one
    /// bus-arbiter hold, and so a frame costs one broker round trip instead of
    /// one per delay. Splitting here also pushed every delay onto the tokio
    /// timer, which quantizes to the ~15.6ms Windows tick.
    async fn execute_operations(
        &self,
        operations: Vec<SmBusOperation>,
    ) -> Result<Vec<u8>, TransportError> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }

        let bus = Arc::clone(&self.bus);
        let path = self.path.clone();
        let address = self.address;

        self.bus_arbiter
            .run_blocking(move || {
                Self::execute_batch_locked(bus.as_ref(), &path, address, &operations)
            })
            .await
    }
}

#[cfg(target_os = "windows")]
#[async_trait]
impl Transport for SmBusTransport {
    fn name(&self) -> &'static str {
        "Windows PawnIO SMBus"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        let operations = decode_operations(data)?;
        let _guard = self.op_lock.lock().await;
        self.execute_operations(operations).await?;

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
        self.execute_operations(operations).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn u8_address(address: u16) -> Result<u8, TransportError> {
    u8::try_from(address).map_err(|_| TransportError::IoError {
        detail: format!("SMBus address 0x{address:02X} exceeds u8 range"),
    })
}

#[cfg(target_os = "windows")]
fn map_windows_smbus_io_error(path: &str, address: u8, error: PawnIoError) -> TransportError {
    let kind = error.kind();
    let detail = error.to_string();
    match kind {
        PawnIoErrorKind::PermissionDenied => TransportError::PermissionDenied { detail },
        PawnIoErrorKind::NotFound => TransportError::NotFound { detail },
        PawnIoErrorKind::Unavailable => TransportError::Disconnected { detail },
        PawnIoErrorKind::InvalidInput | PawnIoErrorKind::Io => TransportError::IoError {
            detail: format!("{detail} (path={path}, address=0x{address:02X})"),
        },
    }
}

#[cfg(target_os = "windows")]
fn map_windows_pawnio_error(error: PawnIoError) -> TransportError {
    let kind = error.kind();
    let detail = error.to_string();
    match kind {
        PawnIoErrorKind::PermissionDenied => TransportError::PermissionDenied { detail },
        PawnIoErrorKind::NotFound => TransportError::NotFound { detail },
        PawnIoErrorKind::Unavailable => TransportError::Disconnected { detail },
        PawnIoErrorKind::InvalidInput | PawnIoErrorKind::Io => TransportError::IoError { detail },
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub struct SmBusTransport;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl SmBusTransport {
    /// `SMBus` transport is only available on Linux and Windows.
    pub fn open(_path: &str, _address: u16) -> Result<Self, TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux and Windows".to_owned(),
        })
    }

    /// `SMBus` transport is only available on Linux and Windows.
    pub fn open_with_arbiter(
        _path: &str,
        _address: u16,
        _bus_arbiter: SmBusBusArbiter,
    ) -> Result<Self, TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux and Windows".to_owned(),
        })
    }

    /// `SMBus` transport is only available on Linux and Windows.
    pub async fn probe_presence(_path: &str, _address: u16) -> Result<bool, TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux and Windows".to_owned(),
        })
    }

    /// `SMBus` transport is only available on Linux and Windows.
    pub async fn probe_quick_write(_path: &str, _address: u16) -> Result<bool, TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux and Windows".to_owned(),
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[async_trait]
impl Transport for SmBusTransport {
    fn name(&self) -> &'static str {
        "SMBus"
    }

    async fn send(&self, _data: &[u8]) -> Result<(), TransportError> {
        Err(TransportError::IoError {
            detail: "SMBus transport is only available on Linux and Windows".to_owned(),
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
fn map_linux_i2c_error(path: &str, address: u16, error: &LinuxI2CError) -> TransportError {
    let detail = format!("{error} (path={path}, address=0x{address:02X})");
    let kind = match error {
        LinuxI2CError::Errno(errno) => std::io::Error::from_raw_os_error(*errno).kind(),
        LinuxI2CError::Io(error) => error.kind(),
    };

    match kind {
        std::io::ErrorKind::PermissionDenied => TransportError::PermissionDenied { detail },
        std::io::ErrorKind::NotFound => TransportError::NotFound { detail },
        _ => TransportError::IoError { detail },
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_i2c_classification_uses_error_kind_not_message() {
        let misleading = LinuxI2CError::Io(std::io::Error::other(
            "permission denied and device not found",
        ));
        assert!(matches!(
            map_linux_i2c_error("/dev/i2c-test", 0x40, &misleading),
            TransportError::IoError { .. }
        ));

        let permission = LinuxI2CError::Errno(nix::libc::EACCES);
        assert!(matches!(
            map_linux_i2c_error("/dev/i2c-test", 0x40, &permission),
            TransportError::PermissionDenied { .. }
        ));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    #[test]
    fn pawnio_mapping_uses_structured_kind_not_detail() {
        let misleading = PawnIoError::BrokerCall {
            operation: "test",
            kind: PawnIoErrorKind::Io,
            hresult: None,
            detail: "access denied and module not found".to_owned(),
        };
        assert!(matches!(
            map_windows_pawnio_error(misleading),
            TransportError::IoError { .. }
        ));

        let permission = PawnIoError::BrokerCall {
            operation: "test",
            kind: PawnIoErrorKind::PermissionDenied,
            hresult: Some(0x8007_0005),
            detail: "harmless display text".to_owned(),
        };
        assert!(matches!(
            map_windows_pawnio_error(permission),
            TransportError::PermissionDenied { .. }
        ));
    }
}
