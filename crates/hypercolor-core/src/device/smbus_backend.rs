//! `SMBus` backend for HAL-managed controllers.

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use hypercolor_driver_api::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceFrameSink,
    DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice,
};
use hypercolor_driver_support::{DeviceSession, FrameDelivery, Session, SessionTable};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::smbus_registry::build_smbus_protocol;
use hypercolor_hal::transport::smbus::{SmBusBusArbiter, SmBusTransport};
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{DeviceId, DeviceInfo, SMBUS_OUTPUT_BACKEND_ID};
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

/// What a connect fixed about one device.
struct SmBusFacts {
    info_template: DeviceInfo,
    target_fps: Option<u32>,
}

/// One open device on the bus.
struct SmBusSession {
    device_id: DeviceId,
    protocol: Box<dyn Protocol>,
    transport: Box<dyn Transport>,
    frame_commands: Vec<ProtocolCommand>,
}

impl SmBusSession {
    async fn write(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<FrameWrite, DeviceError> {
        self.protocol
            .encode_frame_into(colors, &mut self.frame_commands);
        if self.frame_commands.is_empty() {
            return Ok(FrameWrite::Unchanged);
        }
        let payload_bytes = self
            .protocol
            .written_frame_bytes()
            .unwrap_or_else(|| colors.len().saturating_mul(3));
        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        let result = run_commands(
            self.protocol.as_ref(),
            self.transport.as_ref(),
            self.frame_commands.as_slice(),
        )
        .await;
        self.protocol.frame_write_finished(result.is_ok());
        result
            .map(|()| FrameWrite::Written { payload_bytes })
            .map_err(|error| {
                map_hal_transport_error(
                    self.device_id,
                    SMBUS_OUTPUT_BACKEND_ID,
                    DeviceTransportOperation::Write,
                    &error,
                )
            })
    }
}

impl DeviceSession for SmBusSession {
    type Facts = SmBusFacts;

    /// Init transactions run on blocking threads with the native handle, and
    /// dropping the wait would detach them.
    const OPEN: Duration = Duration::MAX;

    /// A shutdown sequence retries on a busy bus.
    const CLEANUP: Duration = Duration::from_secs(2);

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.write(colors, delivery)
            .await
            .map(|_| DeviceWriteOutcome::Sent)
    }

    async fn deliver_frame(
        &mut self,
        delivery: FrameDelivery,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        let started_at = Instant::now();
        let mut payload_bytes = 0;
        let result =
            self.write(colors.as_slice(), Some(&delivery))
                .await
                .map(|write| match write {
                    FrameWrite::Unchanged => DeviceWriteOutcome::SuppressedDuplicate,
                    FrameWrite::Written {
                        payload_bytes: bytes,
                    } => {
                        payload_bytes = bytes;
                        DeviceWriteOutcome::Sent
                    }
                });
        DeviceDeliveryAck::from_write_result(
            delivery.id(),
            payload_bytes,
            started_at.elapsed(),
            result,
        )
    }

    async fn finish(&mut self) -> Result<(), DeviceError> {
        let shutdown_sequence = self.protocol.shutdown_sequence();
        if let Err(error) = run_commands(
            self.protocol.as_ref(),
            self.transport.as_ref(),
            shutdown_sequence.as_slice(),
        )
        .await
        {
            warn!(device_id = %self.device_id, error = %error, "SMBus shutdown sequence failed");
        }
        Ok(())
    }

    /// The native handle goes with the session, whatever else holds a sink.
    /// A transaction cancelled past settlement still runs on its blocking
    /// thread with that handle, and closing the transport waits it out.
    async fn release(self) -> Result<(), DeviceError> {
        self.transport
            .close()
            .await
            .map_err(map_transport_error)
            .map_err(|error| {
                map_hal_transport_error(
                    self.device_id,
                    SMBUS_OUTPUT_BACKEND_ID,
                    DeviceTransportOperation::Connect,
                    &error,
                )
            })
    }
}

type SmBusTransportFactory =
    Arc<dyn Fn(&str, u16, SmBusBusArbiter) -> Result<Box<dyn Transport>> + Send + Sync>;

/// Core `SMBus` backend for HAL-managed ENE controllers.
pub struct SmBusBackend {
    pending: StdRwLock<HashMap<DeviceId, PendingSmBusDevice>>,
    sessions: SessionTable<SmBusSession>,
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
                SmBusTransport::open_device(bus_path, address, bus_arbiter).with_context(|| {
                    format!("failed to open SMBus transport at {bus_path} address 0x{address:02X}")
                })?,
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
            sessions: SessionTable::new(),
            transport_factory: Arc::new(transport_factory),
        }
    }
}

impl Default for SmBusBackend {
    fn default() -> Self {
        Self {
            pending: StdRwLock::new(HashMap::new()),
            sessions: SessionTable::new(),
            transport_factory: Self::default_transport_factory(),
        }
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
        let id = *id;
        self.sessions
            .open(&id, || {
                let pending = self
                    .pending
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&id)
                    .cloned();
                let transport_factory = Arc::clone(&self.transport_factory);
                async move {
                    let pending = pending.ok_or(DeviceError::NotAdopted { device_id: id })?;
                    debug!(
                        device_id = %id,
                        bus_path = pending.bus_path,
                        address = format_args!("0x{:02X}", pending.address),
                        "attempting SMBus connect"
                    );
                    let bus_arbiter = SmBusBusArbiter::for_bus(&pending.bus_path);
                    connect_pending_device(id, &pending, &transport_factory, bus_arbiter)
                        .await
                        .map_err(|error| {
                            map_hal_transport_error(
                                id,
                                SMBUS_OUTPUT_BACKEND_ID,
                                DeviceTransportOperation::Disconnect,
                                &error,
                            )
                        })
                }
            })
            .await
    }

    async fn retire(&self) -> Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        match self.sessions.close(id).await {
            Err(DeviceError::Disconnected { .. }) => Ok(()),
            closed => closed,
        }
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        let (Some(facts), Some(sink)) = (self.sessions.facts(id), self.sessions.sink(id)) else {
            return Ok(None);
        };
        let Ok(session) = sink.admit().await else {
            return Ok(None);
        };

        Ok(Some(build_connected_device_info(
            *id,
            &facts.info_template,
            session.protocol.as_ref(),
        )))
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.sessions.facts(id).and_then(|facts| facts.target_fps)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
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
    device_id: DeviceId,
    pending: &PendingSmBusDevice,
    transport_factory: &SmBusTransportFactory,
    bus_arbiter: SmBusBusArbiter,
) -> Result<Session<SmBusSession>> {
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
    Ok(Session::new(
        device_id,
        SmBusFacts {
            info_template: pending.info_template.clone(),
            target_fps,
        },
        SmBusSession {
            device_id,
            protocol,
            transport,
            frame_commands: Vec::new(),
        },
    ))
}

/// What one frame write put on the bus.
#[derive(Debug, Clone, Copy)]
enum FrameWrite {
    /// The protocol encoded nothing to send, so no transport I/O ran.
    Unchanged,
    /// The transport wrote the frame's commands.
    Written {
        /// Pixel bytes the commands carried.
        payload_bytes: usize,
    },
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
            // SMBus reads only ever answer the transaction that asked, so
            // a protocol calling one unsolicited is wrong, not waiting.
            if parsed.status == ResponseStatus::Unsolicited {
                return Err(anyhow!(
                    "{} marked an SMBus reply unsolicited; SMBus has no unprompted reports",
                    protocol.name()
                ));
            }

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
