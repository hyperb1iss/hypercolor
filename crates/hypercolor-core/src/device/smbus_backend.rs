//! `SMBus` backend for HAL-managed controllers.

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use hypercolor_driver_api::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome,
    DiscoveredDevice,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::smbus_registry::build_smbus_protocol;
use hypercolor_hal::transport::smbus::{SmBusBusArbiter, SmBusTransport};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{DeviceId, DeviceInfo, SMBUS_OUTPUT_BACKEND_ID};
use tokio::sync::Mutex;
use tracing::{debug, trace, warn};

use super::transport_error::{DeviceTransportOperation, map_hal_transport_error};
use hypercolor_types::device::DeviceError;

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;

#[derive(Clone)]
struct PendingSmBusDevice {
    bus_path: String,
    address: u16,
    protocol_id: String,
    info_template: DeviceInfo,
}

struct ConnectedSmBusDevice {
    info_template: DeviceInfo,
    target_fps: Option<u32>,
    io: Arc<Mutex<SmBusDeviceIo>>,
    connected: AtomicBool,
}

struct SmBusDeviceIo {
    protocol: Box<dyn Protocol>,
    transport: Box<dyn Transport>,
    frame_commands: Vec<ProtocolCommand>,
}

type SmBusTransportFactory =
    Arc<dyn Fn(&str, u16, SmBusBusArbiter) -> Result<Box<dyn Transport>> + Send + Sync>;

struct SmBusDeviceFrameSink {
    device_id: DeviceId,
    device: Arc<ConnectedSmBusDevice>,
}

/// Core `SMBus` backend for HAL-managed ENE controllers.
pub struct SmBusBackend {
    pending: StdRwLock<HashMap<DeviceId, PendingSmBusDevice>>,
    connected: StdRwLock<HashMap<DeviceId, Arc<ConnectedSmBusDevice>>>,
    bus_arbiters: StdMutex<HashMap<String, SmBusBusArbiter>>,
    transport_factory: SmBusTransportFactory,
}

impl SmBusBackend {
    /// Create an empty `SMBus` backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn default_transport_factory() -> SmBusTransportFactory {
        Arc::new(|bus_path, address, bus_arbiter| {
            Ok(Box::new(
                SmBusTransport::open_with_arbiter(bus_path, address, bus_arbiter).with_context(
                    || {
                        format!(
                            "failed to open SMBus transport at {bus_path} address 0x{address:02X}"
                        )
                    },
                )?,
            ))
        })
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_transport_factory<F>(transport_factory: F) -> Self
    where
        F: Fn(&str, u16, SmBusBusArbiter) -> Result<Box<dyn Transport>> + Send + Sync + 'static,
    {
        Self {
            pending: StdRwLock::new(HashMap::new()),
            connected: StdRwLock::new(HashMap::new()),
            bus_arbiters: StdMutex::new(HashMap::new()),
            transport_factory: Arc::new(transport_factory),
        }
    }
}

impl Default for SmBusBackend {
    fn default() -> Self {
        Self {
            pending: StdRwLock::new(HashMap::new()),
            connected: StdRwLock::new(HashMap::new()),
            bus_arbiters: StdMutex::new(HashMap::new()),
            transport_factory: Self::default_transport_factory(),
        }
    }
}

#[async_trait::async_trait]
impl DeviceFrameSink for SmBusDeviceFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        write_connected_device(
            self.device_id,
            self.device.as_ref(),
            colors.as_slice(),
            None,
        )
        .await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = write_connected_device(
            self.device_id,
            self.device.as_ref(),
            colors.as_slice(),
            Some((id, observer.as_ref())),
        )
        .await
        .map(|()| DeviceWriteOutcome::Sent);
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for SmBusBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: SMBUS_OUTPUT_BACKEND_ID.to_owned(),
            name: "SMBus (HAL)".to_owned(),
            description: "Native SMBus/I2C devices via HAL protocol + transport".to_owned(),
        }
    }

    fn lifecycle_policy(&self, _info: &DeviceInfo) -> DeviceLifecyclePolicy {
        DeviceLifecyclePolicy::default().with_connect_execution(ConnectExecution::Background)
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let pending = pending_from_discovered(discovered).ok_or(DeviceError::NotAdopted {
            device_id: discovered.info.id,
        })?;
        self.pending
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, pending);
        Ok(())
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            debug!(device_id = %id, "SMBus device already connected; skipping duplicate connect");
            return Ok(());
        }

        let pending = {
            let pending_guard = self.pending.read().unwrap_or_else(PoisonError::into_inner);
            pending_guard
                .get(id)
                .cloned()
                .ok_or(DeviceError::NotAdopted { device_id: *id })?
        };

        debug!(
            device_id = %id,
            bus_path = pending.bus_path,
            address = format_args!("0x{:02X}", pending.address),
            "attempting SMBus connect"
        );

        let bus_arbiter = self
            .bus_arbiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(pending.bus_path.clone())
            .or_default()
            .clone();
        let device = connect_pending_device(&pending, &self.transport_factory, bus_arbiter)
            .await
            .map_err(|error| {
                map_hal_transport_error(
                    *id,
                    SMBUS_OUTPUT_BACKEND_ID,
                    DeviceTransportOperation::Disconnect,
                    &error,
                )
            })?;
        self.connected
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(device));

        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let device = self
            .connected
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let Some(device) = device else {
            return Ok(());
        };

        device.connected.store(false, Ordering::Release);
        let io = device.io.lock().await;
        let shutdown_sequence = io.protocol.shutdown_sequence();
        if let Err(error) = run_commands(
            io.protocol.as_ref(),
            io.transport.as_ref(),
            shutdown_sequence.as_slice(),
        )
        .await
        {
            warn!(device_id = %id, error = %error, "SMBus shutdown sequence failed");
        }

        io.transport
            .close()
            .await
            .map_err(map_transport_error)
            .map_err(|error| {
                map_hal_transport_error(
                    *id,
                    SMBUS_OUTPUT_BACKEND_ID,
                    DeviceTransportOperation::Connect,
                    &error,
                )
            })
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let device = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or_else(|| DeviceError::Disconnected {
                device: id.to_string(),
            })?;
        write_connected_device(*id, device.as_ref(), colors, None).await
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        let device = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Ok(None);
        };
        let io = device.io.lock().await;

        Ok(Some(build_connected_device_info(
            *id,
            &device.info_template,
            io.protocol.as_ref(),
        )))
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| device.target_fps)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|device| {
                Arc::new(SmBusDeviceFrameSink {
                    device_id: *id,
                    device: Arc::clone(device),
                }) as Arc<dyn DeviceFrameSink>
            })
    }
}

fn pending_from_discovered(discovered: &DiscoveredDevice) -> Option<PendingSmBusDevice> {
    let bus_path = discovered.metadata.get("bus_path")?.clone();
    let address = parse_u16_hex(discovered.metadata.get("smbus_address")?)?;

    Some(PendingSmBusDevice {
        bus_path,
        address,
        protocol_id: discovered.info.origin.protocol_id.clone()?,
        info_template: discovered.info.clone(),
    })
}

async fn connect_pending_device(
    pending: &PendingSmBusDevice,
    transport_factory: &SmBusTransportFactory,
    bus_arbiter: SmBusBusArbiter,
) -> Result<ConnectedSmBusDevice> {
    let transport = transport_factory(&pending.bus_path, pending.address, bus_arbiter.clone())?;
    let protocol = build_smbus_protocol(&pending.protocol_id).with_context(|| {
        format!(
            "SMBus protocol '{}' is not registered for {} at {} address 0x{:02X}",
            pending.protocol_id, pending.info_template.name, pending.bus_path, pending.address
        )
    })?;
    run_init_sequence(
        protocol.as_ref(),
        transport.as_ref(),
        &pending.info_template.name,
        &pending.bus_path,
        pending.address,
    )
    .await?;

    let target_fps = fps_from_frame_interval(protocol.frame_interval());
    Ok(ConnectedSmBusDevice {
        info_template: pending.info_template.clone(),
        target_fps,
        io: Arc::new(Mutex::new(SmBusDeviceIo {
            protocol,
            transport,
            frame_commands: Vec::new(),
        })),
        connected: AtomicBool::new(true),
    })
}

async fn write_connected_device(
    device_id: DeviceId,
    device: &ConnectedSmBusDevice,
    colors: &[[u8; 3]],
    delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
) -> Result<(), DeviceError> {
    if !device.connected.load(Ordering::Acquire) {
        return Err(DeviceError::Disconnected {
            device: device_id.to_string(),
        });
    }
    let mut io = device.io.lock().await;
    if !device.connected.load(Ordering::Acquire) {
        return Err(DeviceError::Disconnected {
            device: device_id.to_string(),
        });
    }
    let SmBusDeviceIo {
        protocol,
        transport,
        frame_commands,
    } = &mut *io;
    protocol.encode_frame_into(colors, frame_commands);
    if let Some((id, observer)) = delivery {
        observer.transport_started(id);
    }
    run_commands(
        protocol.as_ref(),
        transport.as_ref(),
        frame_commands.as_slice(),
    )
    .await
    .map_err(|error| {
        map_hal_transport_error(
            device_id,
            SMBUS_OUTPUT_BACKEND_ID,
            DeviceTransportOperation::Write,
            &error,
        )
    })
}

async fn run_init_sequence(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    device_name: &str,
    bus_path: &str,
    address: u16,
) -> Result<()> {
    let init_sequence = protocol.init_sequence();
    run_commands(protocol, transport, init_sequence.as_slice())
        .await
        .with_context(|| {
            format!(
                "failed to run SMBus init sequence for {device_name} at {bus_path} address 0x{address:02X}"
            )
        })
}

async fn run_commands(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    commands: &[ProtocolCommand],
) -> Result<()> {
    let total_commands = commands.len();

    for (index, command) in commands.iter().enumerate() {
        let command_position = index + 1;
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            expects_response = command.expects_response,
            transfer_type = ?command.transfer_type,
            packet = %describe_packet(&command.data),
            "SMBus command queued"
        );
        run_command(
            protocol,
            transport,
            command,
            command_position,
            total_commands,
        )
        .await?;

        if !command.post_delay.is_zero() {
            tokio::time::sleep(command.post_delay).await;
        }
    }

    Ok(())
}

async fn run_command(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    command: &ProtocolCommand,
    command_position: usize,
    total_commands: usize,
) -> Result<()> {
    let mut attempt = 0_u8;

    loop {
        if command.expects_response {
            if run_response_command(
                protocol,
                transport,
                command,
                command_position,
                total_commands,
                &mut attempt,
            )
            .await?
            {
                continue;
            }
        } else {
            transport
                .send_with_type(&command.data, command.transfer_type)
                .await
                .map_err(map_transport_error)?;
        }

        return Ok(());
    }
}

async fn run_response_command(
    protocol: &dyn Protocol,
    transport: &dyn Transport,
    command: &ProtocolCommand,
    command_position: usize,
    total_commands: usize,
    attempt: &mut u8,
) -> Result<bool> {
    let response = if command.response_delay.is_zero() {
        transport
            .send_receive_with_type(
                &command.data,
                protocol.response_timeout(),
                command.transfer_type,
            )
            .await
            .map_err(map_transport_error)?
    } else {
        transport
            .send_with_type(&command.data, command.transfer_type)
            .await
            .map_err(map_transport_error)?;
        tokio::time::sleep(command.response_delay).await;
        transport
            .receive_with_type(protocol.response_timeout(), command.transfer_type)
            .await
            .map_err(map_transport_error)?
    };

    trace!(
        protocol = protocol.name(),
        transport = transport.name(),
        command_index = command_position,
        total_commands,
        response = %describe_packet(&response),
        "SMBus response received"
    );

    match protocol.parse_response(&response) {
        Ok(parsed) => {
            if matches!(
                parsed.status,
                ResponseStatus::Busy | ResponseStatus::Timeout
            ) && *attempt < MAX_RETRIES
            {
                *attempt = attempt.saturating_add(1);
                tokio::time::sleep(RETRY_BACKOFF).await;
                return Ok(true);
            }

            if parsed.status == ResponseStatus::Unsupported {
                warn!(
                    protocol = protocol.name(),
                    "SMBus command not supported by device"
                );
            }

            Ok(false)
        }
        Err(ProtocolError::DeviceError {
            status: ResponseStatus::Busy | ResponseStatus::Timeout,
        }) if *attempt < MAX_RETRIES => {
            *attempt = attempt.saturating_add(1);
            tokio::time::sleep(RETRY_BACKOFF).await;
            Ok(true)
        }
        Err(error) => Err(anyhow!("protocol response parse failed: {error}")),
    }
}

fn parse_u16_hex(raw: &str) -> Option<u16> {
    let trimmed = raw.trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(trimmed, 16).ok()
}

fn map_transport_error(error: TransportError) -> anyhow::Error {
    anyhow!(error)
}

fn describe_packet(data: &[u8]) -> String {
    format!("len={} bytes={}", data.len(), format_hex_preview(data, 24))
}

fn format_hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    let preview_len = min(bytes.len(), max_bytes);
    let mut rendered = bytes
        .iter()
        .take(preview_len)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    if bytes.len() > preview_len {
        let extra_bytes = bytes.len() - preview_len;
        let _ = write!(&mut rendered, " ... (+{extra_bytes} bytes)");
    }

    if rendered.is_empty() {
        "<empty>".to_owned()
    } else {
        rendered
    }
}

fn build_connected_device_info(
    device_id: DeviceId,
    template: &DeviceInfo,
    protocol: &dyn Protocol,
) -> DeviceInfo {
    let mut info = template.clone();
    info.id = device_id;
    info.segments = protocol.zones();
    info.capabilities = protocol.capabilities();
    info
}

fn fps_from_frame_interval(frame_interval: Duration) -> Option<u32> {
    let nanos = frame_interval.as_nanos();
    if nanos == 0 {
        return None;
    }

    let frames_per_second = (1_000_000_000_u128 / nanos).max(1);
    Some(u32::try_from(frames_per_second).unwrap_or(u32::MAX))
}
