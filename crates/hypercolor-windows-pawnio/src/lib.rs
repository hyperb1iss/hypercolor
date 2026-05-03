//! Safe wrapper around PawnIO SMBus modules on Windows.

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{
    PawnIoError, PawnIoResult, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBus,
    WindowsSmBusBusInfo, enumerate_smbus_buses, open_smbus_bus,
};

#[cfg(not(target_os = "windows"))]
mod stubs {
    use std::path::PathBuf;

    use thiserror::Error;

    /// PawnIO result type.
    pub type PawnIoResult<T> = Result<T, PawnIoError>;

    /// PawnIO integration errors.
    #[derive(Debug, Error)]
    pub enum PawnIoError {
        /// PawnIO is only supported on Windows.
        #[error("PawnIO SMBus support is only available on Windows")]
        UnsupportedPlatform,
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
    PawnIoError, PawnIoResult, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBus,
    WindowsSmBusBusInfo, enumerate_smbus_buses, open_smbus_bus,
};
