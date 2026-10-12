//! Safe wrapper around PawnIO SMBus modules on Windows.

/// Stable error classification preserved across the local broker boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    target_os = "windows",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum PawnIoErrorKind {
    /// Host policy denied the operation.
    PermissionDenied,
    /// Required PawnIO installation, module, or device was not found.
    NotFound,
    /// The local broker transport is unavailable.
    Unavailable,
    /// The caller supplied invalid input.
    InvalidInput,
    /// All other PawnIO I/O failures.
    Io,
}

/// Service Control Manager name the installer registers the SMBus broker
/// under.
pub const BROKER_SERVICE_NAME: &str = "HypercolorSmBus";

/// Service-specific exit code the broker reports when its request loop
/// failed, such as when it could not create its pipe for ten seconds.
pub const BROKER_EXIT_SERVE_FAILED: u32 = 1;

/// Service-specific exit code the broker reports when it could not start
/// its async runtime.
pub const BROKER_EXIT_RUNTIME_FAILED: u32 = 2;

/// `ERROR_SERVICE_SPECIFIC_ERROR`: the service reported its own exit code.
pub(crate) const ERROR_SERVICE_SPECIFIC_ERROR: u32 = 1066;
/// `ERROR_SERVICE_REQUEST_TIMEOUT`.
const ERROR_SERVICE_REQUEST_TIMEOUT: u32 = 1053;
/// `ERROR_PROCESS_ABORTED`.
const ERROR_PROCESS_ABORTED: u32 = 1067;
/// `ERROR_SERVICE_NEVER_STARTED`.
const ERROR_SERVICE_NEVER_STARTED: u32 = 1077;

/// Explain a stopped broker's last exit, as the Service Control Manager
/// recorded it, in words a person can act on.
///
/// `win32_exit_code` is `dwWin32ExitCode`; `service_specific_exit_code` is
/// `dwServiceSpecificExitCode`, which only means something when the Win32
/// code is 1066.
#[must_use]
pub fn describe_broker_exit(win32_exit_code: u32, service_specific_exit_code: u32) -> String {
    match win32_exit_code {
        0 => "it last stopped cleanly (exit code 0), so it was stopped on purpose or by a \
              Windows shutdown and has not been started since"
            .to_owned(),
        ERROR_SERVICE_NEVER_STARTED => {
            "it has not been started since Windows booted (exit code 1077)".to_owned()
        }
        ERROR_SERVICE_SPECIFIC_ERROR => match service_specific_exit_code {
            BROKER_EXIT_SERVE_FAILED => {
                "its request loop failed (broker exit code 1); its log says why".to_owned()
            }
            BROKER_EXIT_RUNTIME_FAILED => {
                "it could not start its runtime (broker exit code 2); its log says why".to_owned()
            }
            code => format!("it stopped with broker exit code {code}"),
        },
        ERROR_PROCESS_ABORTED => {
            "its process ended without reporting a stop (exit code 1067), which is a crash"
                .to_owned()
        }
        ERROR_SERVICE_REQUEST_TIMEOUT => {
            "it did not answer Windows in time when starting (exit code 1053)".to_owned()
        }
        code => format!("it last exited with Windows error {code}"),
    }
}

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{
    BrokerServiceState, BrokerServiceStatus, CpuTempReader, CpuVendor, PawnIoError, PawnIoResult,
    SmBusBatchOperation, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBus,
    WindowsSmBusBusInfo, broker_log_path, broker_service_status, enumerate_smbus_buses,
    open_smbus_bus, run_smbus_service,
};

#[cfg(not(target_os = "windows"))]
mod stubs {
    use std::path::PathBuf;
    use std::time::Duration;

    use thiserror::Error;

    use crate::PawnIoErrorKind;

    /// PawnIO result type.
    pub type PawnIoResult<T> = Result<T, PawnIoError>;

    /// PawnIO integration errors.
    #[derive(Debug, Error)]
    pub enum PawnIoError {
        /// PawnIO is only supported on Windows.
        #[error("PawnIO SMBus support is only available on Windows")]
        UnsupportedPlatform,
    }

    impl PawnIoError {
        /// Return the stable error classification.
        #[must_use]
        pub const fn kind(&self) -> PawnIoErrorKind {
            PawnIoErrorKind::Unavailable
        }
    }

    /// SMBus transfer direction.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SmBusDirection {
        /// SMBus read.
        Read,
        /// SMBus write.
        Write,
    }

    /// SMBus block payload.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SmBusBlockData {
        data: Vec<u8>,
    }

    impl SmBusBlockData {
        /// Build block data from payload bytes.
        ///
        /// # Errors
        ///
        /// Returns [`PawnIoError`] on unsupported platforms.
        pub fn new(_data: &[u8]) -> PawnIoResult<Self> {
            Err(PawnIoError::UnsupportedPlatform)
        }

        /// Return the block payload bytes.
        #[must_use]
        pub fn as_slice(&self) -> &[u8] {
            &self.data
        }
    }

    /// SMBus transaction payload.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SmBusTransaction {
        /// SMBus quick transaction.
        Quick,
        /// SMBus byte transaction.
        Byte { value: u8 },
        /// SMBus byte-data transaction.
        ByteData { value: u8 },
        /// SMBus word-data transaction.
        WordData { value: u16 },
        /// SMBus block-data transaction.
        BlockData { data: SmBusBlockData },
    }

    /// One batchable SMBus bus action.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SmBusBatchOperation {
        /// Execute a single SMBus transaction.
        Transfer {
            /// SMBus transfer direction.
            direction: SmBusDirection,
            /// SMBus command/register byte.
            command: u8,
            /// Transaction payload. Read transactions are updated in place.
            transaction: SmBusTransaction,
        },
        /// Sleep between bus operations.
        Delay {
            /// Delay duration.
            duration: Duration,
        },
    }

    /// Discovered PawnIO SMBus bus metadata.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowsSmBusBusInfo {
        /// Stable Hypercolor bus path.
        pub path: String,
        /// PawnIO module blob filename.
        pub module_name: &'static str,
        /// Optional PIIX4 port selection.
        pub port: Option<u8>,
        /// Human-readable bus name.
        pub name: String,
        /// PCI vendor ID reported by the module.
        pub pci_vendor: u16,
        /// PCI device ID reported by the module.
        pub pci_device: u16,
        /// PCI subsystem vendor ID reported by the module.
        pub pci_subsystem_vendor: u16,
        /// PCI subsystem device ID reported by the module.
        pub pci_subsystem_device: u16,
        /// Resolved PawnIO module path.
        pub module_path: PathBuf,
    }

    /// Open PawnIO SMBus bus.
    pub struct WindowsSmBusBus;

    impl WindowsSmBusBus {
        /// Return bus metadata.
        #[must_use]
        pub fn info(&self) -> &WindowsSmBusBusInfo {
            unreachable!("unsupported platform")
        }

        /// Execute an SMBus transaction.
        ///
        /// # Errors
        ///
        /// Returns [`PawnIoError`] on unsupported platforms.
        pub fn smbus_xfer(
            &self,
            _address: u8,
            _direction: SmBusDirection,
            _command: u8,
            _transaction: &mut SmBusTransaction,
        ) -> PawnIoResult<()> {
            Err(PawnIoError::UnsupportedPlatform)
        }

        /// Execute a batch of SMBus operations against one address.
        ///
        /// # Errors
        ///
        /// Returns [`PawnIoError`] on unsupported platforms.
        pub fn smbus_xfer_batch(
            &self,
            _address: u8,
            _operations: &mut [SmBusBatchOperation],
        ) -> PawnIoResult<()> {
            Err(PawnIoError::UnsupportedPlatform)
        }

        /// Probe address with SMBus quick write.
        ///
        /// # Errors
        ///
        /// Returns [`PawnIoError`] on unsupported platforms.
        pub fn probe_quick_write(&self, _address: u8) -> PawnIoResult<bool> {
            Err(PawnIoError::UnsupportedPlatform)
        }

        /// Probe address with simple read fallbacks.
        ///
        /// # Errors
        ///
        /// Returns [`PawnIoError`] on unsupported platforms.
        pub fn probe_presence(&self, _address: u8) -> PawnIoResult<bool> {
            Err(PawnIoError::UnsupportedPlatform)
        }
    }

    /// Enumerate PawnIO SMBus buses.
    ///
    /// # Errors
    ///
    /// Returns [`PawnIoError`] on unsupported platforms.
    pub fn enumerate_smbus_buses() -> PawnIoResult<Vec<WindowsSmBusBusInfo>> {
        Err(PawnIoError::UnsupportedPlatform)
    }

    /// Open one PawnIO SMBus bus by path.
    ///
    /// # Errors
    ///
    /// Returns [`PawnIoError`] on unsupported platforms.
    pub fn open_smbus_bus(_path: &str) -> PawnIoResult<WindowsSmBusBus> {
        Err(PawnIoError::UnsupportedPlatform)
    }
}

#[cfg(not(target_os = "windows"))]
pub use stubs::{
    PawnIoError, PawnIoResult, SmBusBatchOperation, SmBusBlockData, SmBusDirection,
    SmBusTransaction, WindowsSmBusBus, WindowsSmBusBusInfo, enumerate_smbus_buses, open_smbus_bus,
};
