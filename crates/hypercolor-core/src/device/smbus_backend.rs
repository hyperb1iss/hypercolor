//! `SMBus` backend for ASUS Aura ENE controllers.

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use hypercolor_hal::drivers::asus::AuraSmBusProtocol;
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::transport::smbus::SmBusTransport;
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{DeviceId, DeviceInfo, ZoneInfo};
use tracing::{debug, trace, warn};

use super::discovery::{DiscoveredDevice, TransportScanner};
use super::smbus_scanner::SmBusScanner;
use super::traits::{BackendInfo, DeviceBackend};

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;

#[derive(Clone)]
struct PendingSmBusDevice {
    bus_path: String,
    address: u16,
    info_template: DeviceInfo,
}

struct ConnectedSmBusDevice {
    protocol: Box<dyn Protocol>,
    transport: Box<dyn Transport>,
    info_template: DeviceInfo,
    target_fps: Option<u32>,
    frame_commands: Vec<ProtocolCommand>,
}

/// Core `SMBus` backend for HAL-managed ENE controllers.
pub struct SmBusBackend {
    scanner: SmBusScanner,
    pending: HashMap<DeviceId, PendingSmBusDevice>,
    connected: HashMap<DeviceId, ConnectedSmBusDevice>,
}

impl SmBusBackend {
    /// Create an empty `SMBus` backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_scanner(scanner: SmBusScanner) -> Self {
        Self {
            scanner,
            pending: HashMap::new(),
            connected: HashMap::new(),
        }
    }
}

impl Default for SmBusBackend {
    fn default() -> Self {
        Self::with_scanner(SmBusScanner::default())
    }
}

#[async_trait::async_trait]
impl DeviceBackend for SmBusBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "smbus".to_owned(),
            name: "SMBus (HAL)".to_owned(),
            description: "Native SMBus/I2C devices via HAL protocol + transport".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let discovered = self.scanner.scan().await?;

        self.pending.clear();

        let mut info = Vec::with_capacity(discovered.len());
        for discovered_device in discovered {
            if let Some(pending) = pending_from_discovered(&discovered_device) {
                self.pending.insert(discovered_device.info.id, pending);
            }
            info.push(discovered_device.info);
        }

        Ok(info)
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let pending = self.pending.get(id).cloned().with_context(|| {
            format!(
                "device {id} has no pending SMBus descriptor; run discover() (pending_cache_size={})",
                self.pending.len()
            )
        })?;

        debug!(
            device_id = %id,
            bus_path = pending.bus_path,
            address = format_args!("0x{:02X}", pending.address),
            "attempting SMBus connect"
        );

        let transport: Box<dyn Transport> = Box::new(
            SmBusTransport::open(&pending.bus_path, pending.address).with_context(|| {
                format!(
                    "failed to open SMBus transport at {} address 0x{:02X}",
                    pending.bus_path, pending.address
                )
            })?,
        );
        let protocol: Box<dyn Protocol> = Box::new(AuraSmBusProtocol::new());

        let init_sequence = protocol.init_sequence();
        run_commands(
            protocol.as_ref(),
            transport.as_ref(),
            init_sequence.as_slice(),
        )
        .await
        .with_context(|| {
            format!(
                "failed to run SMBus init sequence for {} at {} address 0x{:02X}",
                pending.info_template.name, pending.bus_path, pending.address
            )
        })?;

        let target_fps = fps_from_frame_interval(protocol.frame_interval());
        self.connected.insert(
            *id,
            ConnectedSmBusDevice {
                protocol,
                transport,
                info_template: pending.info_template,
                target_fps,
                frame_commands: Vec::new(),
            },
        );

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(device) = self.connected.remove(id) else {
            return Ok(());
        };

        let shutdown_sequence = device.protocol.shutdown_sequence();
        if let Err(error) = run_commands(
            device.protocol.as_ref(),
            device.transport.as_ref(),
            shutdown_sequence.as_slice(),
        )
        .await
        {
            warn!(device_id = %id, error = %error, "SMBus shutdown sequence failed");
        }

        device.transport.close().await.map_err(map_transport_error)
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let device = self
            .connected
            .get_mut(id)
            .with_context(|| format!("device {id} is not connected on SMBus backend"))?;

        device
            .protocol
            .encode_frame_into(colors, &mut device.frame_commands);
        run_commands(
            device.protocol.as_ref(),
            device.transport.as_ref(),
            device.frame_commands.as_slice(),
        )
        .await
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let Some(device) = self.connected.get(id) else {
            return Ok(None);
        };

        Ok(Some(build_connected_device_info(
            *id,
            &device.info_template,
            device.protocol.as_ref(),
        )))
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.connected.get(id).and_then(|device| device.target_fps)
    }
}

fn pending_from_discovered(discovered: &DiscoveredDevice) -> Option<PendingSmBusDevice> {
    let bus_path = discovered.metadata.get("bus_path")?.clone();
    let address = parse_u16_hex(discovered.metadata.get("smbus_address")?)?;

    Some(PendingSmBusDevice {
        bus_path,
        address,
        info_template: discovered.info.clone(),
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
    info.zones = protocol
        .zones()
        .into_iter()
        .map(protocol_zone_to_zone_info)
        .collect();
    info.capabilities = protocol.capabilities();
    info
}

fn protocol_zone_to_zone_info(zone: hypercolor_hal::protocol::ProtocolZone) -> ZoneInfo {
    ZoneInfo {
        name: zone.name,
        led_count: zone.led_count,
        topology: zone.topology,
        color_format: zone.color_format,
    }
}

fn fps_from_frame_interval(frame_interval: Duration) -> Option<u32> {
    let nanos = frame_interval.as_nanos();
    if nanos == 0 {
        return None;
    }

    let frames_per_second = (1_000_000_000_u128 / nanos).max(1);
    Some(u32::try_from(frames_per_second).unwrap_or(u32::MAX))
}
