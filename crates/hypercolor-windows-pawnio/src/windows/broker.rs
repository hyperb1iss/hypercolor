use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::watch;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use super::{
    PawnIoError, PawnIoResult, SmBusBatchOperation, SmBusBlockData, SmBusDirection,
    SmBusTransaction, WindowsSmBusBus, WindowsSmBusBusInfo, direct_enumerate_smbus_buses,
    direct_open_smbus_bus, module_name_from_wire,
};

const SERVICE_NAME: &str = "HypercolorSmBus";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const SERVICE_START_WAIT_HINT: Duration = Duration::from_secs(15);
const SERVICE_STOP_WAIT_HINT: Duration = Duration::from_secs(10);
const PIPE_NAME: &str = r"\\.\pipe\hypercolor-smbus-v1";
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)";
const MAX_FRAME_BYTES: usize = 64 * 1024;
const CLIENT_CONNECT_ATTEMPTS: usize = 20;
const CLIENT_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(10);
const DIRECT_ENV: &str = "HYPERCOLOR_PAWNIO_DIRECT";

define_windows_service!(ffi_service_main, service_main);

pub(super) fn direct_mode_enabled() -> bool {
    std::env::var_os(DIRECT_ENV).is_some_and(|value| value != "0")
}

pub(super) fn enumerate_smbus_buses() -> PawnIoResult<Vec<WindowsSmBusBusInfo>> {
    let response = send_request(BrokerRequest::EnumerateBuses, "enumerate_buses")?;
    let BrokerResponse::Buses { buses } = response else {
        return Err(unexpected_response("enumerate_buses"));
    };

    buses.into_iter().map(TryInto::try_into).collect()
}

pub(super) fn open_smbus_bus(path: &str) -> PawnIoResult<WindowsSmBusBus> {
    let response = send_request(
        BrokerRequest::OpenBus {
            path: path.to_owned(),
        },
        "open_bus",
    )?;
    let BrokerResponse::Bus { bus } = response else {
        return Err(unexpected_response("open_bus"));
    };

    Ok(WindowsSmBusBus::brokered(bus.try_into()?))
}

pub(super) fn smbus_xfer(
    path: &str,
    address: u8,
    direction: SmBusDirection,
    command: u8,
    transaction: &mut SmBusTransaction,
) -> PawnIoResult<()> {
    let response = send_request(
        BrokerRequest::SmBusXfer {
            path: path.to_owned(),
            address,
            direction: direction.into(),
            command,
            transaction: BrokerSmBusTransaction::from(&*transaction),
        },
        "smbus_xfer",
    )?;
    let BrokerResponse::Transaction {
        transaction: returned,
    } = response
    else {
        return Err(unexpected_response("smbus_xfer"));
    };

    *transaction = returned.try_into()?;
    Ok(())
}

pub(super) fn smbus_xfer_batch(
    path: &str,
    address: u8,
    operations: &mut [SmBusBatchOperation],
) -> PawnIoResult<()> {
    let broker_operations = operations
        .iter()
        .map(BrokerSmBusBatchOperation::from)
        .collect::<Vec<_>>();
    let response = send_request(
        BrokerRequest::SmBusXferBatch {
            path: path.to_owned(),
            address,
            operations: broker_operations,
        },
        "smbus_xfer_batch",
    )?;
    let BrokerResponse::Batch {
        operations: returned,
    } = response
    else {
        return Err(unexpected_response("smbus_xfer_batch"));
    };
    if returned.len() != operations.len() {
        return Err(PawnIoError::BrokerCall {
            operation: "smbus_xfer_batch",
            detail: format!(
                "broker returned {} batch operations for {} requests",
                returned.len(),
                operations.len()
            ),
        });
    }

    for (target, returned) in operations.iter_mut().zip(returned) {
        if let (
            SmBusBatchOperation::Transfer { transaction, .. },
            BrokerSmBusBatchOperation::Transfer {
                transaction: returned,
                ..
            },
        ) = (target, returned)
        {
            *transaction = returned.try_into()?;
        }
    }

    Ok(())
}

pub(super) fn run_smbus_service() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    if args.any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    init_tracing();

    if std::env::args_os().any(|arg| arg == "--console") {
        return run_console();
    }

    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("failed to start Hypercolor SMBus broker service dispatcher")
}

fn print_help() {
    println!(
        "Hypercolor SMBus broker\n\nUSAGE:\n    hypercolor-smbus-service.exe [--console]\n\nRuns the narrow PawnIO SMBus broker used by the regular Hypercolor daemon."
    );
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hypercolor_windows_pawnio=info,info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}

fn run_console() -> Result<()> {
    let runtime = build_runtime()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    runtime.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });

    runtime.block_on(serve_until_shutdown(shutdown_rx))
}

fn service_main(_arguments: Vec<OsString>) {
    init_tracing();
    if let Err(error) = run_service() {
        error!(error = %format_args!("{error:#}"), "Hypercolor SMBus broker service failed");
    }
}

fn run_service() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = shutdown_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .context("failed to register Hypercolor SMBus broker control handler")?;
    report_status(
        &status_handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
        SERVICE_START_WAIT_HINT,
    )?;

    let runtime = build_runtime()?;
    report_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
        Duration::ZERO,
    )?;

    let run_result = runtime.block_on(serve_until_shutdown(shutdown_rx));
    let exit_code = u32::from(run_result.is_err());
    report_status(
        &status_handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit_code,
        SERVICE_STOP_WAIT_HINT,
    )?;

    run_result
}

fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hypercolor-smbus-broker")
        .enable_all()
        .build()
        .context("failed to build Hypercolor SMBus broker runtime")
}

fn report_status(
    status_handle: &ServiceStatusHandle,
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: u32,
    wait_hint: Duration,
) -> Result<()> {
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(exit_code),
            checkpoint: 0,
            wait_hint,
            process_id: None,
        })
        .context("failed to update Hypercolor SMBus broker service status")
}

async fn serve_until_shutdown(mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let state = Arc::new(BrokerState::default());
    info!(pipe = PIPE_NAME, "Hypercolor SMBus broker listening");

    while !*shutdown.borrow() {
        let server = match create_server_pipe() {
            Ok(server) => server,
            Err(error) => {
                warn!(
                    error = %format_args!("{error:#}"),
                    "failed to create SMBus broker pipe; retrying"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };
        tokio::select! {
            connect_result = server.connect() => {
                match connect_result {
                    Ok(()) => {
                        tokio::spawn(handle_connected_client(server, Arc::clone(&state)));
                    }
                    Err(error) => {
                        warn!(%error, "SMBus broker pipe connection failed; accepting next client");
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }

    info!("Hypercolor SMBus broker stopped");
    Ok(())
}

async fn handle_connected_client(mut server: NamedPipeServer, state: Arc<BrokerState>) {
    let result = async {
        let request = read_frame(&mut server).await?;
        let response = match serde_json::from_slice::<BrokerRequest>(&request) {
            Ok(request) => BrokerEnvelope::ok(state.handle_request(request)),
            Err(error) => BrokerEnvelope::error(format!("invalid broker request: {error}")),
        };
        let response_bytes =
            serde_json::to_vec(&response).context("failed to encode SMBus broker response")?;
        write_frame(&mut server, &response_bytes).await
    }
    .await;

    if let Err(error) = result {
        trace!(error = %format_args!("{error:#}"), "SMBus broker client request failed");
    }
}

fn create_server_pipe() -> Result<NamedPipeServer> {
    let mut security = PipeSecurity::new().context("failed to build SMBus broker pipe ACL")?;
    let mut options = ServerOptions::new();
    options.reject_remote_clients(true);
    let pipe = unsafe {
        // SAFETY: `PipeSecurity` owns a valid SECURITY_ATTRIBUTES structure for
        // the duration of this CreateNamedPipeW call. Tokio copies the raw
        // pointer into the immediate OS call and does not retain it.
        options.create_with_security_attributes_raw(PIPE_NAME, security.as_mut_ptr())
    }?;
    Ok(pipe)
}

async fn read_frame(stream: &mut NamedPipeServer) -> Result<Vec<u8>> {
    let mut len_bytes = [0_u8; 4];
    stream
        .read_exact(&mut len_bytes)
        .await
        .context("failed to read SMBus broker frame length")?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "SMBus broker frame has {len} bytes, max is {MAX_FRAME_BYTES}"
        ));
    }

    let mut bytes = vec![0_u8; len];
    stream
        .read_exact(&mut bytes)
        .await
        .context("failed to read SMBus broker frame")?;
    Ok(bytes)
}

async fn write_frame(stream: &mut NamedPipeServer, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).context("SMBus broker response exceeds u32 length")?;
    stream
        .write_all(&len.to_le_bytes())
        .await
        .context("failed to write SMBus broker response length")?;
    stream
        .write_all(bytes)
        .await
        .context("failed to write SMBus broker response")
}

fn send_request(request: BrokerRequest, operation: &'static str) -> PawnIoResult<BrokerResponse> {
    let request_bytes = serde_json::to_vec(&request).map_err(|error| PawnIoError::BrokerCall {
        operation,
        detail: format!("failed to encode request: {error}"),
    })?;
    let response_bytes = send_request_bytes(&request_bytes)?;
    let envelope = serde_json::from_slice::<BrokerEnvelope>(&response_bytes).map_err(|error| {
        PawnIoError::BrokerCall {
            operation,
            detail: format!("failed to decode response: {error}"),
        }
    })?;

    envelope.into_result(operation)
}

fn send_request_bytes(request: &[u8]) -> PawnIoResult<Vec<u8>> {
    let mut stream = open_pipe_client()?;
    write_blocking_frame(&mut stream, request)?;
    read_blocking_frame(&mut stream)
}

fn open_pipe_client() -> PawnIoResult<std::fs::File> {
    let mut last_error = None;
    for attempt in 0..CLIENT_CONNECT_ATTEMPTS {
        match OpenOptions::new().read(true).write(true).open(PIPE_NAME) {
            Ok(file) => return Ok(file),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < CLIENT_CONNECT_ATTEMPTS {
                    thread::sleep(CLIENT_CONNECT_RETRY_DELAY);
                }
            }
        }
    }

    Err(PawnIoError::BrokerUnavailable {
        detail: last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "named pipe did not open".to_owned()),
    })
}

fn write_blocking_frame(stream: &mut std::fs::File, bytes: &[u8]) -> PawnIoResult<()> {
    let len = u32::try_from(bytes.len()).map_err(|error| PawnIoError::BrokerCall {
        operation: "write_frame",
        detail: format!("request frame is too large: {error}"),
    })?;
    stream
        .write_all(&len.to_le_bytes())
        .and_then(|()| stream.write_all(bytes))
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to write request to broker: {error}"),
        })
}

fn read_blocking_frame(stream: &mut std::fs::File) -> PawnIoResult<Vec<u8>> {
    let mut len_bytes = [0_u8; 4];
    stream
        .read_exact(&mut len_bytes)
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to read response length from broker: {error}"),
        })?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(PawnIoError::BrokerCall {
            operation: "read_frame",
            detail: format!("response frame has {len} bytes, max is {MAX_FRAME_BYTES}"),
        });
    }

    let mut bytes = vec![0_u8; len];
    stream
        .read_exact(&mut bytes)
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to read response from broker: {error}"),
        })?;
    Ok(bytes)
}

#[derive(Default)]
struct BrokerState {
    buses: Mutex<HashMap<String, WindowsSmBusBus>>,
}

impl BrokerState {
    fn handle_request(&self, request: BrokerRequest) -> PawnIoResult<BrokerResponse> {
        match request {
            BrokerRequest::EnumerateBuses => Ok(BrokerResponse::Buses {
                buses: direct_enumerate_smbus_buses()?
                    .iter()
                    .map(BrokerBusInfo::from)
                    .collect(),
            }),
            BrokerRequest::OpenBus { path } => Ok(BrokerResponse::Bus {
                bus: BrokerBusInfo::from(&self.open_or_cached_bus(&path)?),
            }),
            BrokerRequest::SmBusXfer {
                path,
                address,
                direction,
                command,
                transaction,
            } => {
                let mut transaction = transaction.try_into()?;
                self.smbus_xfer(&path, address, direction.into(), command, &mut transaction)?;
                Ok(BrokerResponse::Transaction {
                    transaction: BrokerSmBusTransaction::from(&transaction),
                })
            }
            BrokerRequest::SmBusXferBatch {
                path,
                address,
                mut operations,
            } => {
                self.smbus_xfer_batch(&path, address, &mut operations)?;
                Ok(BrokerResponse::Batch { operations })
            }
        }
    }

    fn open_or_cached_bus(&self, path: &str) -> PawnIoResult<WindowsSmBusBusInfo> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            let bus = direct_open_smbus_bus(path)?;
            debug!(
                bus_path = %bus.info().path,
                name = %bus.info().name,
                "opened SMBus broker bus"
            );
            buses.insert(path.to_owned(), bus);
        }

        buses
            .get(path)
            .map(|bus| bus.info().clone())
            .ok_or_else(|| PawnIoError::InvalidInput {
                detail: format!("SMBus broker bus '{path}' was not cached after open"),
            })
    }

    fn smbus_xfer(
        &self,
        path: &str,
        address: u8,
        direction: SmBusDirection,
        command: u8,
        transaction: &mut SmBusTransaction,
    ) -> PawnIoResult<()> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            buses.insert(path.to_owned(), direct_open_smbus_bus(path)?);
        }
        let bus = buses.get(path).ok_or_else(|| PawnIoError::InvalidInput {
            detail: format!("SMBus broker bus '{path}' was not cached after open"),
        })?;

        bus.smbus_xfer(address, direction, command, transaction)
    }

    fn smbus_xfer_batch(
        &self,
        path: &str,
        address: u8,
        operations: &mut [BrokerSmBusBatchOperation],
    ) -> PawnIoResult<()> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            buses.insert(path.to_owned(), direct_open_smbus_bus(path)?);
        }
        let bus = buses.get(path).ok_or_else(|| PawnIoError::InvalidInput {
            detail: format!("SMBus broker bus '{path}' was not cached after open"),
        })?;

        for operation in operations {
            match operation {
                BrokerSmBusBatchOperation::Transfer {
                    direction,
                    command,
                    transaction,
                } => {
                    let mut transaction_value = SmBusTransaction::try_from(std::mem::replace(
                        transaction,
                        BrokerSmBusTransaction::Quick,
                    ))?;
                    bus.smbus_xfer(
                        address,
                        (*direction).into(),
                        *command,
                        &mut transaction_value,
                    )?;
                    *transaction = BrokerSmBusTransaction::from(&transaction_value);
                }
                BrokerSmBusBatchOperation::Delay { duration_ms } => {
                    thread::sleep(Duration::from_millis(u64::from(*duration_ms)));
                }
            }
        }

        Ok(())
    }
}

fn lock_poisoned() -> PawnIoError {
    PawnIoError::InvalidInput {
        detail: "SMBus broker state lock poisoned".to_owned(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    EnumerateBuses,
    OpenBus {
        path: String,
    },
    SmBusXfer {
        path: String,
        address: u8,
        direction: BrokerSmBusDirection,
        command: u8,
        transaction: BrokerSmBusTransaction,
    },
    SmBusXferBatch {
        path: String,
        address: u8,
        operations: Vec<BrokerSmBusBatchOperation>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerResponse {
    Buses {
        buses: Vec<BrokerBusInfo>,
    },
    Bus {
        bus: BrokerBusInfo,
    },
    Transaction {
        transaction: BrokerSmBusTransaction,
    },
    Batch {
        operations: Vec<BrokerSmBusBatchOperation>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct BrokerEnvelope {
    ok: bool,
    data: Option<BrokerResponse>,
    error: Option<String>,
}

impl BrokerEnvelope {
    fn ok(result: PawnIoResult<BrokerResponse>) -> Self {
        match result {
            Ok(response) => Self {
                ok: true,
                data: Some(response),
                error: None,
            },
            Err(error) => Self::error(error.to_string()),
        }
    }

    fn error(error: String) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error),
        }
    }

    fn into_result(self, operation: &'static str) -> PawnIoResult<BrokerResponse> {
        if self.ok {
            return self.data.ok_or_else(|| PawnIoError::BrokerCall {
                operation,
                detail: "success response did not include data".to_owned(),
            });
        }

        Err(PawnIoError::BrokerCall {
            operation,
            detail: self
                .error
                .unwrap_or_else(|| "broker returned failure without an error".to_owned()),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BrokerBusInfo {
    path: String,
    module_name: String,
    port: Option<u8>,
    name: String,
    pci_vendor: u16,
    pci_device: u16,
    pci_subsystem_vendor: u16,
    pci_subsystem_device: u16,
    module_path: String,
}

impl From<&WindowsSmBusBusInfo> for BrokerBusInfo {
    fn from(info: &WindowsSmBusBusInfo) -> Self {
        Self {
            path: info.path.clone(),
            module_name: info.module_name.to_owned(),
            port: info.port,
            name: info.name.clone(),
            pci_vendor: info.pci_vendor,
            pci_device: info.pci_device,
            pci_subsystem_vendor: info.pci_subsystem_vendor,
            pci_subsystem_device: info.pci_subsystem_device,
            module_path: info.module_path.display().to_string(),
        }
    }
}

impl TryFrom<BrokerBusInfo> for WindowsSmBusBusInfo {
    type Error = PawnIoError;

    fn try_from(info: BrokerBusInfo) -> PawnIoResult<Self> {
        Ok(Self {
            path: info.path,
            module_name: module_name_from_wire(&info.module_name)?,
            port: info.port,
            name: info.name,
            pci_vendor: info.pci_vendor,
            pci_device: info.pci_device,
            pci_subsystem_vendor: info.pci_subsystem_vendor,
            pci_subsystem_device: info.pci_subsystem_device,
            module_path: PathBuf::from(info.module_path),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BrokerSmBusDirection {
    Read,
    Write,
}

impl From<SmBusDirection> for BrokerSmBusDirection {
    fn from(direction: SmBusDirection) -> Self {
        match direction {
            SmBusDirection::Read => Self::Read,
            SmBusDirection::Write => Self::Write,
        }
    }
}

impl From<BrokerSmBusDirection> for SmBusDirection {
    fn from(direction: BrokerSmBusDirection) -> Self {
        match direction {
            BrokerSmBusDirection::Read => Self::Read,
            BrokerSmBusDirection::Write => Self::Write,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerSmBusBatchOperation {
    Transfer {
        direction: BrokerSmBusDirection,
        command: u8,
        transaction: BrokerSmBusTransaction,
    },
    Delay {
        duration_ms: u64,
    },
}

impl From<&SmBusBatchOperation> for BrokerSmBusBatchOperation {
    fn from(operation: &SmBusBatchOperation) -> Self {
        match operation {
            SmBusBatchOperation::Transfer {
                direction,
                command,
                transaction,
            } => Self::Transfer {
                direction: (*direction).into(),
                command: *command,
                transaction: BrokerSmBusTransaction::from(transaction),
            },
            SmBusBatchOperation::Delay { duration } => Self::Delay {
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerSmBusTransaction {
    Quick,
    Byte { value: u8 },
    ByteData { value: u8 },
    WordData { value: u16 },
    BlockData { data: Vec<u8> },
}

impl From<&SmBusTransaction> for BrokerSmBusTransaction {
    fn from(transaction: &SmBusTransaction) -> Self {
        match transaction {
            SmBusTransaction::Quick => Self::Quick,
            SmBusTransaction::Byte { value } => Self::Byte { value: *value },
            SmBusTransaction::ByteData { value } => Self::ByteData { value: *value },
            SmBusTransaction::WordData { value } => Self::WordData { value: *value },
            SmBusTransaction::BlockData { data } => Self::BlockData {
                data: data.as_slice().to_vec(),
            },
        }
    }
}

impl TryFrom<BrokerSmBusTransaction> for SmBusTransaction {
    type Error = PawnIoError;

    fn try_from(transaction: BrokerSmBusTransaction) -> PawnIoResult<Self> {
        match transaction {
            BrokerSmBusTransaction::Quick => Ok(Self::Quick),
            BrokerSmBusTransaction::Byte { value } => Ok(Self::Byte { value }),
            BrokerSmBusTransaction::ByteData { value } => Ok(Self::ByteData { value }),
            BrokerSmBusTransaction::WordData { value } => Ok(Self::WordData { value }),
            BrokerSmBusTransaction::BlockData { data } => Ok(Self::BlockData {
                data: SmBusBlockData::new(&data)?,
            }),
        }
    }
}

fn unexpected_response(operation: &'static str) -> PawnIoError {
    PawnIoError::BrokerCall {
        operation,
        detail: "broker returned the wrong response type".to_owned(),
    }
}

struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attrs: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    fn new() -> Result<Self> {
        let sddl = wide_null(PIPE_SDDL);
        let mut descriptor = std::ptr::null_mut();
        let converted = unsafe {
            // SAFETY: `sddl` is NUL-terminated UTF-16, and `descriptor` is a
            // valid out pointer owned by the returned `PipeSecurity`.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(anyhow!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW failed"
            ));
        }

        Ok(Self {
            descriptor,
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.cast::<c_void>(),
                bInheritHandle: 0,
            },
        })
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        (&mut self.attrs as *mut SECURITY_ATTRIBUTES).cast::<c_void>()
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            let _ = unsafe {
                // SAFETY: The descriptor was allocated by
                // ConvertStringSecurityDescriptorToSecurityDescriptorW and is
                // released once when this owner is dropped.
                LocalFree(self.descriptor.cast::<c_void>())
            };
        }
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        BrokerBusInfo, BrokerSmBusBatchOperation, BrokerSmBusDirection, BrokerSmBusTransaction,
        SmBusBatchOperation, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBusInfo,
    };

    #[test]
    fn transaction_dto_round_trips_block_data() {
        let transaction = SmBusTransaction::BlockData {
            data: SmBusBlockData::new(&[1, 2, 3]).expect("valid block data"),
        };
        let dto = BrokerSmBusTransaction::from(&transaction);

        let decoded = SmBusTransaction::try_from(dto).expect("dto should decode");

        assert_eq!(decoded, transaction);
    }

    #[test]
    fn direction_dto_round_trips() {
        let dto = BrokerSmBusDirection::from(SmBusDirection::Read);
        assert!(matches!(SmBusDirection::from(dto), SmBusDirection::Read));
    }

    #[test]
    fn batch_operation_dto_preserves_transfers_and_delays() {
        let transfer = SmBusBatchOperation::Transfer {
            direction: SmBusDirection::Write,
            command: 0x03,
            transaction: SmBusTransaction::BlockData {
                data: SmBusBlockData::new(&[1, 2, 3]).expect("valid block data"),
            },
        };
        let delay = SmBusBatchOperation::Delay {
            duration: Duration::from_millis(7),
        };

        let transfer_dto = BrokerSmBusBatchOperation::from(&transfer);
        let delay_dto = BrokerSmBusBatchOperation::from(&delay);

        assert!(matches!(
            transfer_dto,
            BrokerSmBusBatchOperation::Transfer {
                direction: BrokerSmBusDirection::Write,
                command: 0x03,
                transaction: BrokerSmBusTransaction::BlockData { .. }
            }
        ));
        assert!(matches!(
            delay_dto,
            BrokerSmBusBatchOperation::Delay { duration_ms: 7 }
        ));
    }

    #[test]
    fn bus_info_dto_preserves_known_module_name() {
        let info = WindowsSmBusBusInfo {
            path: "pawnio:i801".to_owned(),
            module_name: "SmbusI801.bin",
            port: None,
            name: "I801".to_owned(),
            pci_vendor: 1,
            pci_device: 2,
            pci_subsystem_vendor: 3,
            pci_subsystem_device: 4,
            module_path: PathBuf::from(r"C:\PawnIO\SmbusI801.bin"),
        };

        let decoded =
            WindowsSmBusBusInfo::try_from(BrokerBusInfo::from(&info)).expect("dto should decode");

        assert_eq!(decoded, info);
    }
}
