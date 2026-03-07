//! USB backend that bridges HAL protocols to the core `DeviceBackend` trait.

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_hal::database::{DeviceDescriptor, TransportType};
use hypercolor_hal::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ResponseStatus,
};
use hypercolor_hal::transport::control::UsbControlTransport;
use hypercolor_hal::transport::hid::UsbHidTransport;
use hypercolor_hal::transport::vendor::UsbVendorTransport;
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::DeviceId;
use tracing::{debug, trace, warn};

#[cfg(target_os = "linux")]
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

use super::discovery::TransportScanner;
use super::traits::{BackendInfo, DeviceBackend};
use super::usb_scanner::UsbScanner;

const RESPONSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;

#[derive(Clone)]
struct PendingUsbDevice {
    vendor_id: u16,
    product_id: u16,
    serial: Option<String>,
    usb_path: Option<String>,
    descriptor: &'static DeviceDescriptor,
}

struct UsbDevice {
    protocol: Arc<dyn Protocol>,
    transport: Arc<dyn Transport>,
    keepalive_task: Option<tokio::task::JoinHandle<()>>,
}

/// Core USB backend for HAL-managed device families.
#[derive(Default)]
pub struct UsbBackend {
    pending: HashMap<DeviceId, PendingUsbDevice>,
    connected: HashMap<DeviceId, UsbDevice>,
}

impl UsbBackend {
    /// Create an empty USB backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn build_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
    ) -> Result<Box<dyn Transport>> {
        match pending.descriptor.transport {
            TransportType::UsbHidRaw {
                interface,
                report_id,
            } => {
                #[cfg(target_os = "linux")]
                {
                    let transport = UsbHidRawTransport::open(
                        pending.vendor_id,
                        pending.product_id,
                        interface,
                        report_id,
                        pending.serial.as_deref(),
                        pending.usb_path.as_deref(),
                    )
                    .with_context(|| {
                        format!(
                            "failed to open hidraw transport for {:04X}:{:04X} interface {} (report_id=0x{report_id:02X})",
                            pending.vendor_id, pending.product_id, interface
                        )
                    })?;

                    debug!(
                        vendor_id = format_args!("{:04X}", pending.vendor_id),
                        product_id = format_args!("{:04X}", pending.product_id),
                        interface,
                        report_id = format_args!("0x{report_id:02X}"),
                        "using hidraw transport"
                    );
                    Ok(Box::new(transport))
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let _ = usb;
                    bail!("hidraw transport is only supported on Linux");
                }
            }
            TransportType::UsbControl {
                interface,
                report_id,
            } => {
                let device = usb.open().await.with_context(|| {
                    format!(
                        "failed to open USB device {:04X}:{:04X}",
                        pending.vendor_id, pending.product_id
                    )
                })?;
                let transport = UsbControlTransport::new(device, interface, report_id)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to claim USB interface {interface} for control transport (report_id=0x{report_id:02X}); interface may be busy (kernel or another userspace driver)"
                        )
                })?;
                Ok(Box::new(transport))
            }
            TransportType::UsbHid { interface } => {
                let device = usb.open().await.with_context(|| {
                    format!(
                        "failed to open USB device {:04X}:{:04X}",
                        pending.vendor_id, pending.product_id
                    )
                })?;
                let transport = UsbHidTransport::new(device, interface).await.with_context(
                    || {
                        format!(
                            "failed to claim USB interface {interface} for HID interrupt transport"
                        )
                    },
                )?;
                Ok(Box::new(transport))
            }
            TransportType::UsbVendor => Ok(Box::new(UsbVendorTransport::new())),
        }
    }

    async fn run_commands(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        commands: Vec<ProtocolCommand>,
    ) -> Result<()> {
        let total_commands = commands.len();

        for (index, command) in commands.into_iter().enumerate() {
            let command_position = index + 1;
            Self::trace_queued_command(
                protocol,
                transport,
                &command,
                command_position,
                total_commands,
            );
            Self::run_command(
                protocol,
                transport,
                &command,
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

    fn trace_queued_command(
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        command: &ProtocolCommand,
        command_position: usize,
        total_commands: usize,
    ) {
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            expects_response = command.expects_response,
            post_delay_ms = command.post_delay.as_millis(),
            packet = %describe_packet(&command.data),
            "usb command queued"
        );
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            packet_hex = %format_hex_preview(&command.data, 32),
            "usb command bytes"
        );
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
                if Self::run_response_command(
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
                trace!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    attempt = attempt + 1,
                    "usb send starting"
                );
                transport
                    .send(&command.data)
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
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            attempt = *attempt + 1,
            "usb send_receive starting"
        );
        let response = transport
            .send_receive(&command.data, RESPONSE_TIMEOUT)
            .await
            .map_err(map_transport_error)?;

        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            response = %describe_packet(&response),
            "usb response received"
        );
        trace!(
            protocol = protocol.name(),
            transport = transport.name(),
            command_index = command_position,
            total_commands,
            response_hex = %format_hex_preview(&response, 32),
            "usb response bytes"
        );

        match protocol.parse_response(&response) {
            Ok(parsed) => {
                trace!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    status = ?parsed.status,
                    parsed_data_len = parsed.data.len(),
                    parsed_data = %format_hex_preview(&parsed.data, 24),
                    "usb response parsed"
                );
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
                        "command not supported by device; continuing"
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
            Err(error) => {
                warn!(
                    protocol = protocol.name(),
                    transport = transport.name(),
                    command_index = command_position,
                    total_commands,
                    error = %error,
                    response = %describe_packet(&response),
                    response_hex = %format_hex_preview(&response, 32),
                    "protocol response parse failed"
                );
                Err(anyhow!("protocol response parse failed: {error}"))
            }
        }
    }

    fn spawn_keepalive(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        keepalive: ProtocolKeepalive,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(keepalive.interval).await;
                trace!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    interval_ms = keepalive.interval.as_millis(),
                    command_count = keepalive.commands.len(),
                    "usb keepalive tick"
                );

                if let Err(error) = Self::run_commands(
                    protocol.as_ref(),
                    transport.as_ref(),
                    keepalive.commands.clone(),
                )
                .await
                {
                    warn!(
                        device_id = %device_id,
                        device = device_name,
                        protocol = protocol.name(),
                        transport = transport.name(),
                        error = %error,
                        "usb keepalive failed"
                    );
                }
            }
        })
    }
}

#[async_trait::async_trait]
impl DeviceBackend for UsbBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "USB HID (HAL)".to_owned(),
            description: "Native USB devices via HAL protocol + transport".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<hypercolor_types::device::DeviceInfo>> {
        let mut scanner = UsbScanner::new();
        let discovered = scanner.scan().await?;

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
        let pending_ids = self
            .pending
            .keys()
            .take(4)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let pending = self.pending.get(id).cloned().with_context(|| {
            format!(
                "device {id} has no pending USB descriptor; run discover() (pending_cache_size={}, sample_ids=[{}])",
                self.pending.len(),
                pending_ids
            )
        })?;
        debug!(
            device_id = %id,
            vendor_id = format_args!("{:04X}", pending.vendor_id),
            product_id = format_args!("{:04X}", pending.product_id),
            usb_path = pending.usb_path.as_deref().unwrap_or("<unknown>"),
            serial = pending.serial.as_deref().unwrap_or("<none>"),
            descriptor = pending.descriptor.name,
            "attempting USB connect"
        );

        let mut devices = nusb::list_devices()
            .await
            .context("failed to enumerate USB devices for connect")?;
        let usb = devices
            .find(|candidate| matches_usb_device(candidate, &pending))
            .with_context(|| {
                format!(
                    "USB device {:04X}:{:04X} is no longer present (serial={}, usb_path={})",
                    pending.vendor_id,
                    pending.product_id,
                    pending.serial.as_deref().unwrap_or("<none>"),
                    pending.usb_path.as_deref().unwrap_or("<unknown>")
                )
            })?;

        let protocol: Arc<dyn Protocol> = Arc::from((pending.descriptor.protocol.build)());
        let transport: Arc<dyn Transport> = Arc::from(Self::build_transport(&pending, &usb).await?);
        let init_sequence = protocol.init_sequence();
        let first_init_packet = init_sequence.first().map_or_else(
            || "<none>".to_owned(),
            |command| describe_packet(&command.data),
        );

        debug!(
            device_id = %id,
            protocol = protocol.name(),
            transport = transport.name(),
            init_commands = init_sequence.len(),
            first_init_packet = %first_init_packet,
            "running USB init sequence"
        );

        Self::run_commands(protocol.as_ref(), transport.as_ref(), init_sequence)
            .await
            .with_context(|| {
                format!(
                    "failed to run init sequence for {}",
                    pending.descriptor.name
                )
            })?;

        let keepalive_task = protocol.keepalive().map(|keepalive| {
            debug!(
                device_id = %id,
                protocol = protocol.name(),
                transport = transport.name(),
                interval_ms = keepalive.interval.as_millis(),
                command_count = keepalive.commands.len(),
                "starting USB keepalive task"
            );
            Self::spawn_keepalive(
                *id,
                pending.descriptor.name,
                protocol.clone(),
                transport.clone(),
                keepalive,
            )
        });

        self.connected.insert(
            *id,
            UsbDevice {
                protocol,
                transport,
                keepalive_task,
            },
        );

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(mut device) = self.connected.remove(id) else {
            self.pending.remove(id);
            return Ok(());
        };

        if let Some(task) = device.keepalive_task.take() {
            task.abort();
            let _ = task.await;
        }

        let shutdown = device.protocol.shutdown_sequence();
        if !shutdown.is_empty()
            && let Err(error) = Self::run_commands(
                device.protocol.as_ref(),
                device.transport.as_ref(),
                shutdown,
            )
            .await
        {
            warn!(device_id = %id, error = %error, "USB shutdown sequence failed");
        }

        device
            .transport
            .as_ref()
            .close()
            .await
            .map_err(map_transport_error)
            .context("failed to close USB transport")?;

        self.pending.remove(id);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let Some(device) = self.connected.get(id) else {
            bail!("device {id} is not connected");
        };

        let commands = device.protocol.encode_frame(colors);
        let first_packet = commands.first().map_or_else(
            || "<none>".to_owned(),
            |command| describe_packet(&command.data),
        );
        trace!(
            device_id = %id,
            protocol = device.protocol.name(),
            transport = device.transport.name(),
            led_count = colors.len(),
            command_count = commands.len(),
            first_packet = %first_packet,
            "usb frame write requested"
        );
        Self::run_commands(
            device.protocol.as_ref(),
            device.transport.as_ref(),
            commands,
        )
        .await
        .with_context(|| format!("USB frame write failed for device {id}"))
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let Some(device) = self.connected.get(id) else {
            bail!("device {id} is not connected");
        };

        let commands = device
            .protocol
            .encode_brightness(brightness)
            .with_context(|| format!("USB protocol does not support brightness for device {id}"))?;
        let first_packet = commands.first().map_or_else(
            || "<none>".to_owned(),
            |command| describe_packet(&command.data),
        );
        debug!(
            device_id = %id,
            protocol = device.protocol.name(),
            transport = device.transport.name(),
            brightness,
            command_count = commands.len(),
            first_packet = %first_packet,
            "usb brightness write requested"
        );

        Self::run_commands(
            device.protocol.as_ref(),
            device.transport.as_ref(),
            commands,
        )
        .await
        .with_context(|| format!("USB brightness write failed for device {id}"))
    }
}

fn pending_from_discovered(
    discovered: &super::discovery::DiscoveredDevice,
) -> Option<PendingUsbDevice> {
    let vendor_id = parse_u16_hex(discovered.metadata.get("vendor_id")?)?;
    let product_id = parse_u16_hex(discovered.metadata.get("product_id")?)?;
    let descriptor = hypercolor_hal::database::ProtocolDatabase::lookup(vendor_id, product_id)?;

    Some(PendingUsbDevice {
        vendor_id,
        product_id,
        serial: discovered.metadata.get("serial").cloned(),
        usb_path: discovered.metadata.get("usb_path").cloned(),
        descriptor,
    })
}

fn parse_u16_hex(raw: &str) -> Option<u16> {
    let trimmed = raw.trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(trimmed, 16).ok()
}

fn matches_usb_device(device: &nusb::DeviceInfo, pending: &PendingUsbDevice) -> bool {
    if device.vendor_id() != pending.vendor_id || device.product_id() != pending.product_id {
        return false;
    }

    if let Some(serial) = &pending.serial
        && device.serial_number() != Some(serial.as_str())
    {
        return false;
    }

    if let Some(path) = &pending.usb_path
        && usb_path(device) != *path
    {
        return false;
    }

    true
}

fn usb_path(usb: &nusb::DeviceInfo) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let ports = usb
            .port_chain()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(".");

        if ports.is_empty() {
            usb.bus_id().to_owned()
        } else {
            format!("{}-{ports}", usb.bus_id())
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = usb;
        String::new()
    }
}

fn map_transport_error(error: TransportError) -> anyhow::Error {
    anyhow!(error)
}

fn describe_packet(data: &[u8]) -> String {
    if data.len() >= 89 {
        let args_len = usize::from(data[5]);
        let arg_end = min(8 + args_len, data.len());
        let args = if arg_end > 8 {
            format_hex_preview(&data[8..arg_end], 24)
        } else {
            "<none>".to_owned()
        };

        return format!(
            "len={} status=0x{:02X} tx=0x{:02X} size={} class=0x{:02X} cmd=0x{:02X} crc=0x{:02X} args={}",
            data.len(),
            data[0],
            data[1],
            args_len,
            data[6],
            data[7],
            data[88],
            args
        );
    }

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
