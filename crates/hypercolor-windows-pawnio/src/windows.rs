use std::env;
use std::ffi::{CStr, c_char, c_void};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::{Library, Symbol};
use thiserror::Error;
use tracing::{debug, trace};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_FAILED};
use windows_sys::Win32::System::Threading::{
    CreateMutexA, INFINITE, ReleaseMutex, WaitForSingleObject,
};

const PAWNIO_INSTALL_ENV: &str = "HYPERCOLOR_PAWNIO_HOME";
const PAWNIO_MODULE_ENV: &str = "HYPERCOLOR_PAWNIO_MODULE_DIR";
const LOCAL_MODULE_SUBDIR: &[&str] = &["hypercolor", "pawnio", "modules"];

const PAWNIO_DLL_NAME: &str = "PawnIOLib.dll";
const IOCTL_IDENTITY: &CStr = c"ioctl_identity";
const IOCTL_SMBUS_XFER: &CStr = c"ioctl_smbus_xfer";
const IOCTL_PIIX4_PORT_SEL: &CStr = c"ioctl_piix4_port_sel";
const IOCTL_SET_SLEEP_MODE: &CStr = c"ioctl_set_sleep_mode";

const PAWNIO_SLEEP_ALWAYS_SLEEP: u64 = 2;
const GLOBAL_SMBUS_MUTEX_NAME: &[u8] = b"Global\\Access_SMBUS.HTP.Method\0";

const I2C_SMBUS_READ: u64 = 1;
const I2C_SMBUS_WRITE: u64 = 0;
const I2C_SMBUS_QUICK: u64 = 0;
const I2C_SMBUS_BYTE: u64 = 1;
const I2C_SMBUS_BYTE_DATA: u64 = 2;
const I2C_SMBUS_WORD_DATA: u64 = 3;
const I2C_SMBUS_BLOCK_DATA: u64 = 5;
const I2C_SMBUS_BLOCK_MAX: usize = 32;

const S_OK: i32 = 0;
const ERROR_NOT_SUPPORTED: i32 = 50;
const ERROR_ACCESS_DENIED: i32 = 5;
const HRESULT_FACILITY_WIN32: i32 = 7;
const HRESULT_SEVERITY_ERROR: i32 = -2_147_483_648_i32;

const SMBUS_MODULES: &[SmBusModuleSpec] = &[
    SmBusModuleSpec {
        path_prefix: "i801",
        module_name: "SmbusI801.bin",
        ports: &[None],
    },
    SmBusModuleSpec {
        path_prefix: "piix4",
        module_name: "SmbusPIIX4.bin",
        ports: &[Some(0), Some(1)],
    },
    SmBusModuleSpec {
        path_prefix: "nct6793",
        module_name: "SmbusNCT6793.bin",
        ports: &[None],
    },
];

type PawnIoHandle = *mut c_void;
type PawnIoVersion = unsafe extern "system" fn(*mut u32) -> i32;
type PawnIoOpen = unsafe extern "system" fn(*mut PawnIoHandle) -> i32;
type PawnIoLoad = unsafe extern "system" fn(PawnIoHandle, *const u8, usize) -> i32;
type PawnIoExecute = unsafe extern "system" fn(
    PawnIoHandle,
    *const c_char,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type PawnIoClose = unsafe extern "system" fn(PawnIoHandle) -> i32;

/// PawnIO result type.
pub type PawnIoResult<T> = Result<T, PawnIoError>;

/// PawnIO integration errors.
#[derive(Debug, Error)]
pub enum PawnIoError {
    /// PawnIO installation could not be found.
    #[error(
        "PawnIO is not installed or {PAWNIO_DLL_NAME} was not found; install namazso.PawnIO or set {PAWNIO_INSTALL_ENV}"
    )]
    PawnIoNotInstalled,
    /// PawnIO module blob was not found.
    #[error("PawnIO SMBus module {module_name} was not found; set {PAWNIO_MODULE_ENV}")]
    ModuleNotFound {
        /// Module blob filename.
        module_name: &'static str,
    },
    /// Dynamic library failed to load.
    #[error("failed to load PawnIO library {path}: {source}")]
    LoadLibrary {
        /// Library path.
        path: PathBuf,
        /// Load error.
        source: libloading::Error,
    },
    /// Dynamic symbol failed to resolve.
    #[error("failed to resolve PawnIO symbol {symbol}: {source}")]
    LoadSymbol {
        /// Symbol name.
        symbol: &'static str,
        /// Symbol load error.
        source: libloading::Error,
    },
    /// PawnIO call failed.
    #[error("PawnIO {operation} failed with HRESULT 0x{hresult:08X}: {detail}")]
    PawnIoCall {
        /// Operation name.
        operation: &'static str,
        /// Raw HRESULT.
        hresult: u32,
        /// Human-readable detail.
        detail: String,
    },
    /// I/O operation failed.
    #[error("PawnIO SMBus {operation} failed on {bus_path} address 0x{address:02X}: {detail}")]
    SmBusIo {
        /// Operation name.
        operation: &'static str,
        /// Bus path.
        bus_path: String,
        /// SMBus address.
        address: u8,
        /// Human-readable detail.
        detail: String,
    },
    /// Invalid input.
    #[error("{detail}")]
    InvalidInput {
        /// Human-readable detail.
        detail: String,
    },
}

/// SMBus transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmBusDirection {
    /// SMBus read.
    Read,
    /// SMBus write.
    Write,
}

impl SmBusDirection {
    const fn wire_value(self) -> u64 {
        match self {
            Self::Read => I2C_SMBUS_READ,
            Self::Write => I2C_SMBUS_WRITE,
        }
    }
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
    /// Returns [`PawnIoError`] when the payload exceeds the SMBus block size.
    pub fn new(data: &[u8]) -> PawnIoResult<Self> {
        if data.len() > I2C_SMBUS_BLOCK_MAX {
            return Err(PawnIoError::InvalidInput {
                detail: format!(
                    "SMBus block payload has {} bytes, max is {I2C_SMBUS_BLOCK_MAX}",
                    data.len()
                ),
            });
        }

        Ok(Self {
            data: data.to_vec(),
        })
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
    Byte {
        /// Byte payload for writes, result byte for reads.
        value: u8,
    },
    /// SMBus byte-data transaction.
    ByteData {
        /// Byte payload for writes, result byte for reads.
        value: u8,
    },
    /// SMBus word-data transaction.
    WordData {
        /// Word payload for writes, result word for reads.
        value: u16,
    },
    /// SMBus block-data transaction.
    BlockData {
        /// Block payload for writes, result block for reads.
        data: SmBusBlockData,
    },
}

impl SmBusTransaction {
    const fn wire_size(&self) -> u64 {
        match self {
            Self::Quick => I2C_SMBUS_QUICK,
            Self::Byte { .. } => I2C_SMBUS_BYTE,
            Self::ByteData { .. } => I2C_SMBUS_BYTE_DATA,
            Self::WordData { .. } => I2C_SMBUS_WORD_DATA,
            Self::BlockData { .. } => I2C_SMBUS_BLOCK_DATA,
        }
    }
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
pub struct WindowsSmBusBus {
    runtime: Arc<PawnIoRuntime>,
    handle: PawnIoHandle,
    info: WindowsSmBusBusInfo,
    global_mutex: Option<GlobalSmBusMutex>,
}

// SAFETY: PawnIO handles are executor handles owned by this wrapper. HAL keeps
// frame operations serialized, and the loaded library stays alive through
// `runtime` for the lifetime of every handle.
unsafe impl Send for WindowsSmBusBus {}
// SAFETY: Shared access cannot mutate Rust-owned state without passing through
// PawnIO calls, and HAL serializes those calls with a transport mutex.
unsafe impl Sync for WindowsSmBusBus {}

impl WindowsSmBusBus {
    /// Return bus metadata.
    #[must_use]
    pub fn info(&self) -> &WindowsSmBusBusInfo {
        &self.info
    }

    /// Execute an SMBus transaction.
    ///
    /// # Errors
    ///
    /// Returns [`PawnIoError`] when PawnIO rejects the transaction.
    pub fn smbus_xfer(
        &self,
        address: u8,
        direction: SmBusDirection,
        command: u8,
        transaction: &mut SmBusTransaction,
    ) -> PawnIoResult<()> {
        let mut in_args = [0_u64; 9];
        in_args[0] = u64::from(address);
        in_args[1] = direction.wire_value();
        in_args[2] = u64::from(command);
        in_args[3] = transaction.wire_size();
        pack_transaction_data(transaction, &mut in_args[4..])?;

        let _global_guard = self
            .global_mutex
            .as_ref()
            .map(GlobalSmBusMutex::lock)
            .transpose()?;
        let mut out = [0_u64; 5];
        let mut returned = 0_usize;
        let status = self.runtime.execute(
            self.handle,
            IOCTL_SMBUS_XFER,
            &in_args,
            &mut out,
            &mut returned,
        );
        if status != S_OK {
            return Err(PawnIoError::SmBusIo {
                operation: "ioctl_smbus_xfer",
                bus_path: self.info.path.clone(),
                address,
                detail: hresult_detail(status),
            });
        }

        unpack_transaction_data(transaction, &out)?;
        Ok(())
    }

    /// Probe address with SMBus quick write.
    ///
    /// # Errors
    ///
    /// Returns [`PawnIoError`] when PawnIO fails before reaching the device.
    pub fn probe_quick_write(&self, address: u8) -> PawnIoResult<bool> {
        let mut transaction = SmBusTransaction::Quick;
        Ok(self
            .smbus_xfer(address, SmBusDirection::Write, 0, &mut transaction)
            .is_ok())
    }

    /// Probe address with simple read fallbacks.
    ///
    /// # Errors
    ///
    /// Returns [`PawnIoError`] when PawnIO fails before reaching the device.
    pub fn probe_presence(&self, address: u8) -> PawnIoResult<bool> {
        if self.probe_quick_write(address)? {
            return Ok(true);
        }

        let mut read_byte = SmBusTransaction::Byte { value: 0 };
        if self
            .smbus_xfer(address, SmBusDirection::Read, 0, &mut read_byte)
            .is_ok()
        {
            return Ok(true);
        }

        let mut read_byte_data = SmBusTransaction::ByteData { value: 0 };
        Ok(self
            .smbus_xfer(address, SmBusDirection::Read, 0, &mut read_byte_data)
            .is_ok())
    }

    fn open(
        runtime: Arc<PawnIoRuntime>,
        spec: SmBusModuleSpec,
        port: Option<u8>,
    ) -> PawnIoResult<Self> {
        let module_path = resolve_module_path(spec.module_name)?;
        let handle = runtime.open_loaded_module(&module_path)?;
        if let Some(port) = port {
            select_piix4_port(runtime.as_ref(), handle, port)?;
        }
        set_sleep_mode(runtime.as_ref(), handle);

        let identity = read_identity(runtime.as_ref(), handle)?;
        let info = WindowsSmBusBusInfo {
            path: bus_path(spec.path_prefix, port),
            module_name: spec.module_name,
            port,
            name: identity.name,
            pci_vendor: identity.pci_vendor,
            pci_device: identity.pci_device,
            pci_subsystem_vendor: identity.pci_subsystem_vendor,
            pci_subsystem_device: identity.pci_subsystem_device,
            module_path,
        };

        Ok(Self {
            runtime,
            handle,
            info,
            global_mutex: open_global_smbus_mutex(),
        })
    }
}

impl Drop for WindowsSmBusBus {
    fn drop(&mut self) {
        let _ = self.runtime.close(self.handle);
    }
}

#[derive(Clone, Copy)]
struct SmBusModuleSpec {
    path_prefix: &'static str,
    module_name: &'static str,
    ports: &'static [Option<u8>],
}

struct PawnIoRuntime {
    _library: Library,
    open: PawnIoOpen,
    load: PawnIoLoad,
    execute: PawnIoExecute,
    close: PawnIoClose,
}

impl PawnIoRuntime {
    fn load() -> PawnIoResult<Arc<Self>> {
        let library_path = resolve_pawnio_library_path()?;
        let library = unsafe {
            // SAFETY: The path is resolved from configured PawnIO install locations.
            // Loading may run DLL initialization; all loaded symbols are checked below.
            Library::new(&library_path)
        }
        .map_err(|source| PawnIoError::LoadLibrary {
            path: library_path.clone(),
            source,
        })?;

        let version =
            load_symbol::<PawnIoVersion>(&library, b"pawnio_version\0", "pawnio_version")?;
        let open = load_symbol::<PawnIoOpen>(&library, b"pawnio_open\0", "pawnio_open")?;
        let load = load_symbol::<PawnIoLoad>(&library, b"pawnio_load\0", "pawnio_load")?;
        let execute =
            load_symbol::<PawnIoExecute>(&library, b"pawnio_execute\0", "pawnio_execute")?;
        let close = load_symbol::<PawnIoClose>(&library, b"pawnio_close\0", "pawnio_close")?;

        let mut raw_version = 0_u32;
        let status = unsafe {
            // SAFETY: `version` is resolved from PawnIOLib and receives a valid out pointer.
            version(&mut raw_version)
        };
        check_pawnio_status("pawnio_version", status)?;
        debug!(
            version = format_args!(
                "{}.{}.{}",
                raw_version >> 16,
                (raw_version >> 8) & 0xff,
                raw_version & 0xff
            ),
            path = %library_path.display(),
            "loaded PawnIO runtime"
        );

        Ok(Arc::new(Self {
            _library: library,
            open,
            load,
            execute,
            close,
        }))
    }

    fn open_loaded_module(&self, module_path: &Path) -> PawnIoResult<PawnIoHandle> {
        let blob = std::fs::read(module_path).map_err(|error| PawnIoError::InvalidInput {
            detail: format!(
                "failed to read PawnIO module {}: {error}",
                module_path.display()
            ),
        })?;

        let mut handle = std::ptr::null_mut();
        let status = unsafe {
            // SAFETY: `open` is resolved from PawnIOLib and receives a valid handle out pointer.
            (self.open)(&mut handle)
        };
        check_pawnio_status("pawnio_open", status)?;

        let status = unsafe {
            // SAFETY: The handle came from `pawnio_open`; blob pointer/len are valid for this call.
            (self.load)(handle, blob.as_ptr(), blob.len())
        };
        if let Err(error) = check_pawnio_status("pawnio_load", status) {
            let _ = self.close(handle);
            return Err(PawnIoError::InvalidInput {
                detail: format!(
                    "failed to load PawnIO module {}: {error}",
                    module_path.display()
                ),
            });
        }

        Ok(handle)
    }

    fn execute(
        &self,
        handle: PawnIoHandle,
        name: &CStr,
        input: &[u64],
        output: &mut [u64],
        returned: &mut usize,
    ) -> i32 {
        unsafe {
            // SAFETY: The handle is owned by `WindowsSmBusBus`; C string and buffers
            // are valid for the duration of the synchronous PawnIO call.
            (self.execute)(
                handle,
                name.as_ptr(),
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                returned,
            )
        }
    }

    fn close(&self, handle: PawnIoHandle) -> PawnIoResult<()> {
        let status = unsafe {
            // SAFETY: The handle was returned from `pawnio_open`; PawnIO owns close semantics.
            (self.close)(handle)
        };
        check_pawnio_status("pawnio_close", status)
    }
}

struct PawnIoIdentity {
    name: String,
    pci_vendor: u16,
    pci_device: u16,
    pci_subsystem_vendor: u16,
    pci_subsystem_device: u16,
}

struct GlobalSmBusMutex {
    handle: HANDLE,
}

impl GlobalSmBusMutex {
    fn lock(&self) -> PawnIoResult<GlobalSmBusMutexGuard<'_>> {
        let status = unsafe {
            // SAFETY: `handle` was returned by `CreateMutexA` and is valid until
            // this wrapper is dropped. The wait does not access Rust memory.
            WaitForSingleObject(self.handle, INFINITE)
        };
        if status == WAIT_FAILED {
            return Err(PawnIoError::InvalidInput {
                detail: format!(
                    "failed to wait for global SMBus mutex: Win32 error {}",
                    last_win32_error()
                ),
            });
        }

        Ok(GlobalSmBusMutexGuard { mutex: self })
    }
}

impl Drop for GlobalSmBusMutex {
    fn drop(&mut self) {
        let closed = unsafe {
            // SAFETY: `handle` was returned by `CreateMutexA`; closing is idempotent
            // from Rust's perspective because Drop runs once for this wrapper.
            CloseHandle(self.handle)
        };
        if closed == 0 {
            trace!(
                error = last_win32_error(),
                "failed to close global SMBus mutex"
            );
        }
    }
}

struct GlobalSmBusMutexGuard<'a> {
    mutex: &'a GlobalSmBusMutex,
}

impl Drop for GlobalSmBusMutexGuard<'_> {
    fn drop(&mut self) {
        let released = unsafe {
            // SAFETY: The current thread owns the mutex after a successful wait.
            ReleaseMutex(self.mutex.handle)
        };
        if released == 0 {
            trace!(
                error = last_win32_error(),
                "failed to release global SMBus mutex"
            );
        }
    }
}

fn open_global_smbus_mutex() -> Option<GlobalSmBusMutex> {
    let handle = unsafe {
        // SAFETY: Security attributes are null, initial owner is false, and the
        // name is a static NUL-terminated string.
        CreateMutexA(std::ptr::null(), 0, GLOBAL_SMBUS_MUTEX_NAME.as_ptr())
    };
    if handle.is_null() {
        trace!(
            error = last_win32_error(),
            "failed to create global SMBus mutex; continuing with process-local serialization"
        );
        return None;
    }

    Some(GlobalSmBusMutex { handle })
}

fn last_win32_error() -> u32 {
    unsafe {
        // SAFETY: `GetLastError` reads thread-local OS state only.
        GetLastError()
    }
}

/// Enumerate PawnIO SMBus buses.
///
/// # Errors
///
/// Returns [`PawnIoError`] when PawnIO is unavailable or no module can be loaded.
pub fn enumerate_smbus_buses() -> PawnIoResult<Vec<WindowsSmBusBusInfo>> {
    let runtime = PawnIoRuntime::load()?;
    let mut buses = Vec::new();

    for spec in SMBUS_MODULES {
        for &port in spec.ports {
            match WindowsSmBusBus::open(Arc::clone(&runtime), *spec, port) {
                Ok(bus) => {
                    debug!(
                        bus_path = bus.info.path,
                        name = bus.info.name,
                        pci_vendor = format_args!("0x{:04X}", bus.info.pci_vendor),
                        pci_device = format_args!("0x{:04X}", bus.info.pci_device),
                        "discovered PawnIO SMBus bus"
                    );
                    buses.push(bus.info().clone());
                }
                Err(error) => {
                    trace!(
                        module = spec.module_name,
                        port,
                        error = %error,
                        "PawnIO SMBus module did not load"
                    );
                }
            }
        }
    }

    Ok(buses)
}

/// Open one PawnIO SMBus bus by path.
///
/// # Errors
///
/// Returns [`PawnIoError`] when the bus path cannot be opened.
pub fn open_smbus_bus(path: &str) -> PawnIoResult<WindowsSmBusBus> {
    let (spec, port) = parse_bus_path(path)?;
    WindowsSmBusBus::open(PawnIoRuntime::load()?, spec, port)
}

fn load_symbol<T: Copy>(
    library: &Library,
    symbol: &'static [u8],
    name: &'static str,
) -> PawnIoResult<T> {
    let loaded: Symbol<'_, T> = unsafe {
        // SAFETY: Caller supplies the expected symbol type from PawnIOLib.h.
        library.get(symbol)
    }
    .map_err(|source| PawnIoError::LoadSymbol {
        symbol: name,
        source,
    })?;
    Ok(*loaded)
}

fn select_piix4_port(runtime: &PawnIoRuntime, handle: PawnIoHandle, port: u8) -> PawnIoResult<()> {
    let input = [u64::from(port)];
    let mut output = [0_u64; 1];
    let mut returned = 0_usize;
    let status = runtime.execute(
        handle,
        IOCTL_PIIX4_PORT_SEL,
        &input,
        &mut output,
        &mut returned,
    );
    check_pawnio_status("ioctl_piix4_port_sel", status)
}

fn set_sleep_mode(runtime: &PawnIoRuntime, handle: PawnIoHandle) {
    let input = [PAWNIO_SLEEP_ALWAYS_SLEEP];
    let mut returned = 0_usize;
    let status = runtime.execute(handle, IOCTL_SET_SLEEP_MODE, &input, &mut [], &mut returned);
    if status != S_OK {
        trace!(
            status = format_args!("0x{:08X}", status as u32),
            "PawnIO sleep mode ioctl failed"
        );
    }
}

fn read_identity(runtime: &PawnIoRuntime, handle: PawnIoHandle) -> PawnIoResult<PawnIoIdentity> {
    let input = [0_u64];
    let mut output = [0_u64; 3];
    let mut returned = 0_usize;
    let status = runtime.execute(handle, IOCTL_IDENTITY, &input, &mut output, &mut returned);
    check_pawnio_status("ioctl_identity", status)?;

    Ok(PawnIoIdentity {
        name: decode_identity_name(output[0]),
        pci_vendor: (output[2] & 0x0000_0000_0000_FFFF) as u16,
        pci_device: ((output[2] & 0x0000_0000_FFFF_0000) >> 16) as u16,
        pci_subsystem_vendor: ((output[2] & 0x0000_FFFF_0000_0000) >> 32) as u16,
        pci_subsystem_device: ((output[2] & 0xFFFF_0000_0000_0000) >> 48) as u16,
    })
}

fn decode_identity_name(raw: u64) -> String {
    let bytes = raw.to_le_bytes();
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

fn pack_transaction_data(transaction: &SmBusTransaction, output: &mut [u64]) -> PawnIoResult<()> {
    output.fill(0);

    match transaction {
        SmBusTransaction::Quick => {}
        SmBusTransaction::Byte { value } | SmBusTransaction::ByteData { value } => {
            output[0] = u64::from(*value);
        }
        SmBusTransaction::WordData { value } => {
            output[0] = u64::from(*value);
        }
        SmBusTransaction::BlockData { data } => {
            let payload = data.as_slice();
            output[0] = u64::try_from(payload.len()).map_err(|_| PawnIoError::InvalidInput {
                detail: "SMBus block payload length does not fit u64".to_owned(),
            })?;
            for (index, byte) in payload.iter().enumerate() {
                let slot = index + 1;
                let word_index = slot / 8;
                let byte_shift = (slot % 8) * 8;
                output[word_index] |= u64::from(*byte) << byte_shift;
            }
        }
    }

    Ok(())
}

fn unpack_transaction_data(transaction: &mut SmBusTransaction, input: &[u64]) -> PawnIoResult<()> {
    match transaction {
        SmBusTransaction::Quick => {}
        SmBusTransaction::Byte { value } | SmBusTransaction::ByteData { value } => {
            *value = (input[0] & 0xff) as u8;
        }
        SmBusTransaction::WordData { value } => {
            *value = (input[0] & 0xffff) as u16;
        }
        SmBusTransaction::BlockData { data } => {
            let len = usize::try_from(input[0] & 0xff).map_err(|_| PawnIoError::InvalidInput {
                detail: "SMBus block response length does not fit usize".to_owned(),
            })?;
            if len > I2C_SMBUS_BLOCK_MAX {
                return Err(PawnIoError::InvalidInput {
                    detail: format!(
                        "SMBus block response has {len} bytes, max is {I2C_SMBUS_BLOCK_MAX}"
                    ),
                });
            }

            let mut bytes = Vec::with_capacity(len);
            for index in 0..len {
                let slot = index + 1;
                let word_index = slot / 8;
                let byte_shift = (slot % 8) * 8;
                bytes.push(((input[word_index] >> byte_shift) & 0xff) as u8);
            }
            *data = SmBusBlockData::new(&bytes)?;
        }
    }

    Ok(())
}

fn resolve_pawnio_library_path() -> PawnIoResult<PathBuf> {
    pawnio_install_dirs()
        .into_iter()
        .map(|dir| dir.join(PAWNIO_DLL_NAME))
        .find(|path| path.is_file())
        .ok_or(PawnIoError::PawnIoNotInstalled)
}

fn resolve_module_path(module_name: &'static str) -> PawnIoResult<PathBuf> {
    pawnio_module_dirs()
        .into_iter()
        .map(|dir| dir.join(module_name))
        .find(|path| path.is_file())
        .ok_or(PawnIoError::ModuleNotFound { module_name })
}

fn pawnio_install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_env_path(&mut dirs, PAWNIO_INSTALL_ENV);
    if let Ok(key) = windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO")
        && let Ok(path) = key.get_string("InstallLocation")
    {
        push_unique_path(&mut dirs, PathBuf::from(path));
    }
    push_env_child(&mut dirs, "ProgramFiles", "PawnIO");
    push_env_child(&mut dirs, "ProgramFiles(x86)", "PawnIO");
    dirs
}

fn pawnio_module_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_env_path(&mut dirs, PAWNIO_MODULE_ENV);
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        let mut path = PathBuf::from(local_app_data);
        for component in LOCAL_MODULE_SUBDIR {
            path.push(component);
        }
        push_unique_path(&mut dirs, path);
    }
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        push_unique_path(&mut dirs, parent.to_path_buf());
        push_unique_path(&mut dirs, parent.join("pawnio"));
        push_unique_path(&mut dirs, parent.join("pawnio").join("modules"));
    }
    for dir in pawnio_install_dirs() {
        push_unique_path(&mut dirs, dir.clone());
        push_unique_path(&mut dirs, dir.join("modules"));
    }
    dirs
}

fn push_env_path(paths: &mut Vec<PathBuf>, key: &str) {
    if let Some(value) = env::var_os(key) {
        push_unique_path(paths, PathBuf::from(value));
    }
}

fn push_env_child(paths: &mut Vec<PathBuf>, key: &str, child: &str) {
    if let Some(value) = env::var_os(key) {
        push_unique_path(paths, PathBuf::from(value).join(child));
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn parse_bus_path(path: &str) -> PawnIoResult<(SmBusModuleSpec, Option<u8>)> {
    let Some(raw) = path.strip_prefix("pawnio:") else {
        return Err(PawnIoError::InvalidInput {
            detail: format!("invalid PawnIO SMBus path '{path}'"),
        });
    };
    let mut parts = raw.split(':');
    let prefix = parts.next().ok_or_else(|| PawnIoError::InvalidInput {
        detail: format!("invalid PawnIO SMBus path '{path}'"),
    })?;
    let port = parts.next().map(|raw_port| {
        raw_port
            .parse::<u8>()
            .map_err(|error| PawnIoError::InvalidInput {
                detail: format!("invalid PawnIO SMBus port '{raw_port}': {error}"),
            })
    });
    if parts.next().is_some() {
        return Err(PawnIoError::InvalidInput {
            detail: format!("invalid PawnIO SMBus path '{path}'"),
        });
    }

    let port = port.transpose()?;
    let Some(spec) = SMBUS_MODULES
        .iter()
        .copied()
        .find(|candidate| candidate.path_prefix == prefix)
    else {
        return Err(PawnIoError::InvalidInput {
            detail: format!("unknown PawnIO SMBus module prefix '{prefix}'"),
        });
    };
    if !spec.ports.contains(&port) {
        return Err(PawnIoError::InvalidInput {
            detail: format!("PawnIO SMBus module '{prefix}' does not support port {port:?}"),
        });
    }

    Ok((spec, port))
}

fn bus_path(prefix: &str, port: Option<u8>) -> String {
    match port {
        Some(port) => format!("pawnio:{prefix}:{port}"),
        None => format!("pawnio:{prefix}"),
    }
}

fn check_pawnio_status(operation: &'static str, status: i32) -> PawnIoResult<()> {
    if status == S_OK {
        return Ok(());
    }

    Err(PawnIoError::PawnIoCall {
        operation,
        hresult: status as u32,
        detail: hresult_detail(status),
    })
}

fn hresult_detail(status: i32) -> String {
    match status {
        ERROR_NOT_SUPPORTED => "operation is not supported by this PawnIO module".to_owned(),
        ERROR_ACCESS_DENIED => {
            "access denied; run Hypercolor as Administrator or configure PawnIO device ACLs"
                .to_owned()
        }
        other if other == hresult_from_win32(ERROR_NOT_SUPPORTED) => {
            "operation is not supported by this PawnIO module".to_owned()
        }
        other if other == hresult_from_win32(ERROR_ACCESS_DENIED) => {
            "access denied; run Hypercolor as Administrator or configure PawnIO device ACLs"
                .to_owned()
        }
        other => {
            let mut detail = String::new();
            let _ = write!(&mut detail, "raw HRESULT 0x{:08X}", other as u32);
            detail
        }
    }
}

const fn hresult_from_win32(error: i32) -> i32 {
    if error <= 0 {
        error
    } else {
        HRESULT_SEVERITY_ERROR | (HRESULT_FACILITY_WIN32 << 16) | error
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SmBusBlockData, SmBusTransaction, bus_path, pack_transaction_data, parse_bus_path,
        unpack_transaction_data,
    };

    #[test]
    fn bus_path_round_trips_module_and_port() {
        let (spec, port) = parse_bus_path("pawnio:piix4:1").expect("path should parse");
        assert_eq!(spec.module_name, "SmbusPIIX4.bin");
        assert_eq!(port, Some(1));
        assert_eq!(bus_path(spec.path_prefix, port), "pawnio:piix4:1");
    }

    #[test]
    fn block_data_packs_with_length_prefix() {
        let mut packed = [0_u64; 5];
        let transaction = SmBusTransaction::BlockData {
            data: SmBusBlockData::new(&[0xAA, 0xBB, 0xCC]).expect("valid block"),
        };

        pack_transaction_data(&transaction, &mut packed).expect("pack should succeed");

        assert_eq!(packed[0] & 0xffff_ffff, 0xCC_BB_AA_03);
    }

    #[test]
    fn block_data_unpacks_length_prefixed_payload() {
        let mut transaction = SmBusTransaction::BlockData {
            data: SmBusBlockData::new(&[]).expect("valid block"),
        };
        let packed = [0xCC_BB_AA_03_u64, 0, 0, 0, 0];

        unpack_transaction_data(&mut transaction, &packed).expect("unpack should succeed");

        let SmBusTransaction::BlockData { data } = transaction else {
            panic!("expected block transaction");
        };
        assert_eq!(data.as_slice(), &[0xAA, 0xBB, 0xCC]);
    }
}
