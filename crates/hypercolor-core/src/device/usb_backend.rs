//! USB backend that bridges HAL protocols to the core `DeviceBackend` trait.

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_hal::database::{DeviceDescriptor, TransportType};
use hypercolor_hal::drivers::prismrgb::{PrismRgbModel, PrismRgbProtocol, PrismSConfig};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::transport::bulk::UsbBulkTransport;
use hypercolor_hal::transport::control::UsbControlTransport;
use hypercolor_hal::transport::hid::UsbHidTransport;
use hypercolor_hal::transport::serial::UsbSerialTransport;
use hypercolor_hal::transport::vendor::UsbVendorTransport;
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::device::{DeviceId, DeviceInfo, ZoneInfo};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, trace, warn};

#[cfg(target_os = "linux")]
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

use super::discovery::TransportScanner;
use super::traits::{BackendInfo, DeviceBackend};
use super::usb_scanner::UsbScanner;

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;

#[derive(Clone)]
struct PendingUsbDevice {
    vendor_id: u16,
    product_id: u16,
    serial: Option<String>,
    usb_path: Option<String>,
    descriptor: &'static DeviceDescriptor,
    info_template: DeviceInfo,
}

#[derive(Debug)]
struct UsbFramePayload {
    colors: Vec<[u8; 3]>,
}

#[derive(Debug)]
struct UsbDisplayPayload {
    jpeg_data: Vec<u8>,
}

enum UsbDeviceCommand {
    SetBrightness {
        brightness: u8,
        response_tx: oneshot::Sender<std::result::Result<(), String>>,
    },
    Shutdown {
        led_count: usize,
        response_tx: oneshot::Sender<std::result::Result<(), String>>,
    },
}

struct UsbDevice {
    protocol: Arc<dyn Protocol>,
    transport_name: &'static str,
    target_fps: Option<u32>,
    frame_tx: watch::Sender<Option<Arc<UsbFramePayload>>>,
    display_tx: watch::Sender<Option<Arc<UsbDisplayPayload>>>,
    command_tx: mpsc::UnboundedSender<UsbDeviceCommand>,
    actor_task: Option<JoinHandle<()>>,
    last_async_error: Arc<StdMutex<Option<String>>>,
    info_template: DeviceInfo,
    frame_diagnostics_emitted: bool,
    non_black_frame_diagnostics_emitted: bool,
}

impl UsbDevice {
    async fn ensure_actor_ready(&mut self, device_id: DeviceId) -> Result<()> {
        let finished_actor = if self
            .actor_task
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            self.actor_task.take()
        } else {
            None
        };

        if let Some(actor_task) = finished_actor
            && let Err(error) = actor_task.await
        {
            self.store_async_error(format!(
                "USB device actor join failed for device {device_id}: {error}"
            ))?;
        }

        if let Some(error) = self.last_async_error()? {
            bail!("{error}");
        }

        if self.actor_task.is_none() {
            bail!("USB device actor is not running for device {device_id}");
        }

        Ok(())
    }

    fn queue_colors(&self, colors: &[[u8; 3]]) {
        self.frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: colors.to_vec(),
        })));
    }

    fn queue_display_frame(&self, jpeg_data: &[u8]) {
        self.display_tx
            .send_replace(Some(Arc::new(UsbDisplayPayload {
                jpeg_data: jpeg_data.to_vec(),
            })));
    }

    async fn set_brightness(&mut self, device_id: DeviceId, brightness: u8) -> Result<()> {
        self.ensure_actor_ready(device_id).await?;

        let (response_tx, response_rx) = oneshot::channel();
        if self
            .command_tx
            .send(UsbDeviceCommand::SetBrightness {
                brightness,
                response_tx,
            })
            .is_err()
        {
            self.ensure_actor_ready(device_id).await?;
            bail!("USB device actor is unavailable for device {device_id}");
        }

        let response = response_rx.await.map_err(|_| {
            anyhow!("USB device actor terminated while setting brightness for device {device_id}")
        })?;

        if let Err(error) = response {
            bail!("{error}");
        }

        self.ensure_actor_ready(device_id).await
    }

    async fn shutdown(&mut self, device_id: DeviceId) -> Result<()> {
        let Some(actor_task) = self.actor_task.take() else {
            if let Some(error) = self.last_async_error()? {
                bail!("{error}");
            }
            return Ok(());
        };

        let led_count = usize::try_from(self.info_template.total_led_count()).unwrap_or_default();
        let (response_tx, response_rx) = oneshot::channel();
        let command_sent = self
            .command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count,
                response_tx,
            })
            .is_ok();

        let shutdown_result = if command_sent {
            response_rx.await.map_err(|_| {
                anyhow!("USB device actor terminated while shutting down device {device_id}")
            })?
        } else {
            Ok(())
        };

        if let Err(error) = actor_task.await {
            self.store_async_error(format!(
                "USB device actor join failed for device {device_id}: {error}"
            ))?;
        }

        if let Err(error) = shutdown_result {
            self.store_async_error(error.clone())?;
            bail!("{error}");
        }

        if let Some(error) = self.last_async_error()? {
            bail!("{error}");
        }

        Ok(())
    }

    fn last_async_error(&self) -> Result<Option<String>> {
        self.last_async_error
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| anyhow!("USB device async error state lock poisoned"))
    }

    fn store_async_error(&self, error: String) -> Result<()> {
        let mut slot = self
            .last_async_error
            .lock()
            .map_err(|_| anyhow!("USB device async error state lock poisoned"))?;
        *slot = Some(error);
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct UsbProtocolConfigStore {
    prism_s: Arc<RwLock<HashMap<DeviceId, PrismSConfig>>>,
}

impl UsbProtocolConfigStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_prism_s_config(&self, device_id: DeviceId, config: PrismSConfig) {
        let mut prism_s = self.prism_s.write().await;
        prism_s.insert(device_id, config);
    }

    pub async fn prism_s_config(&self, device_id: DeviceId) -> Option<PrismSConfig> {
        let prism_s = self.prism_s.read().await;
        prism_s.get(&device_id).copied()
    }

    pub async fn remove_device(&self, device_id: DeviceId) {
        let mut prism_s = self.prism_s.write().await;
        prism_s.remove(&device_id);
    }
}

/// Core USB backend for HAL-managed device families.
pub struct UsbBackend {
    pending: HashMap<DeviceId, PendingUsbDevice>,
    connected: HashMap<DeviceId, UsbDevice>,
    protocol_configs: UsbProtocolConfigStore,
}

impl Default for UsbBackend {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            connected: HashMap::new(),
            protocol_configs: UsbProtocolConfigStore::default(),
        }
    }
}

impl UsbBackend {
    /// Create an empty USB backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_protocol_config_store(protocol_configs: UsbProtocolConfigStore) -> Self {
        Self {
            protocol_configs,
            ..Self::default()
        }
    }

    async fn build_protocol(
        &self,
        pending: &PendingUsbDevice,
        device_id: DeviceId,
    ) -> Box<dyn Protocol> {
        if pending.info_template.model.as_deref() == Some("prism_s")
            && let Some(config) = self.protocol_configs.prism_s_config(device_id).await
        {
            return Box::new(
                PrismRgbProtocol::new(PrismRgbModel::PrismS).with_prism_s_config(config),
            );
        }

        (pending.descriptor.protocol.build)()
    }

    async fn build_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
    ) -> Result<Box<dyn Transport>> {
        match pending.descriptor.transport {
            TransportType::UsbHidRaw {
                interface,
                report_id,
                usage_page,
                usage,
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
                        usage_page,
                        usage,
                    )
                    .with_context(|| {
                        format!(
                            "failed to open hidraw transport for {:04X}:{:04X} interface {} (report_id=0x{report_id:02X}, usage_page={}, usage={})",
                            pending.vendor_id,
                            pending.product_id,
                            interface,
                            usage_page
                                .map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                            usage.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}"))
                        )
                    })?;

                    debug!(
                        vendor_id = format_args!("{:04X}", pending.vendor_id),
                        product_id = format_args!("{:04X}", pending.product_id),
                        interface,
                        report_id = format_args!("0x{report_id:02X}"),
                        usage_page = usage_page
                            .map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                        usage = usage
                            .map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
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
            } => Self::open_control_transport(pending, usb, interface, report_id).await,
            TransportType::UsbHid { interface } => {
                Self::open_hid_transport(pending, usb, interface).await
            }
            TransportType::UsbBulk {
                interface,
                report_id,
            } => Self::open_bulk_transport(pending, usb, interface, report_id).await,
            TransportType::UsbSerial { baud_rate } => {
                Self::open_serial_transport(pending, baud_rate)
            }
            TransportType::I2cSmBus { address } => {
                let _ = usb;
                bail!(
                    "SMBus transport 0x{address:02X} is not supported by the USB backend; use a dedicated SMBus backend"
                );
            }
            TransportType::UsbVendor => Ok(Box::new(UsbVendorTransport::new())),
        }
    }

    async fn open_control_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
        interface: u8,
        report_id: u8,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        let transport = UsbControlTransport::new(device, interface, report_id)
            .await
            .with_context(|| {
                format!(
                    "failed to claim USB interface {interface} for control transport (report_id=0x{report_id:02X}); interface may be busy (kernel or another userspace driver)"
                )
            })?;
        Ok(Box::new(transport))
    }

    async fn open_hid_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
        interface: u8,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        let transport = UsbHidTransport::new(device, interface)
            .await
            .with_context(|| {
                format!("failed to claim USB interface {interface} for HID interrupt transport")
            })?;
        Ok(Box::new(transport))
    }

    async fn open_bulk_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
        interface: u8,
        report_id: u8,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        let transport = UsbBulkTransport::new(device, interface, report_id)
            .await
            .with_context(|| {
                format!(
                    "failed to claim USB interface {interface} for bulk transport (report_id=0x{report_id:02X})"
                )
            })?;
        Ok(Box::new(transport))
    }

    fn open_serial_transport(
        pending: &PendingUsbDevice,
        baud_rate: u32,
    ) -> Result<Box<dyn Transport>> {
        let transport = UsbSerialTransport::open(
            pending.vendor_id,
            pending.product_id,
            baud_rate,
            pending.serial.as_deref(),
        )
        .with_context(|| {
            format!(
                "failed to open serial transport for {:04X}:{:04X} (serial={})",
                pending.vendor_id,
                pending.product_id,
                pending.serial.as_deref().unwrap_or("<none>")
            )
        })?;
        Ok(Box::new(transport))
    }

    async fn open_usb_device(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
    ) -> Result<nusb::Device> {
        usb.open().await.with_context(|| {
            format!(
                "failed to open USB device {:04X}:{:04X}",
                pending.vendor_id, pending.product_id
            )
        })
    }

    fn spawn_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
        last_async_error: Arc<StdMutex<Option<String>>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let transport_name = transport.name();

            let actor_result = Self::run_device_actor(
                device_id,
                device_name,
                protocol.clone(),
                transport.clone(),
                frame_rx,
                display_rx,
                command_rx,
            )
            .await;

            if let Err(error) = actor_result {
                Self::store_actor_error(&last_async_error, error.to_string());
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport_name,
                    error = %error,
                    "USB device actor failed"
                );
            }

            if let Err(error) = transport.close().await.map_err(map_transport_error) {
                Self::store_actor_error(&last_async_error, error.to_string());
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport_name,
                    error = %error,
                    "failed to close USB transport after actor shutdown"
                );
            }
        })
    }

    async fn run_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        mut display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        mut command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        let mut keepalive_interval = protocol.keepalive().map(|keepalive| {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + keepalive.interval,
                keepalive.interval,
            );
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval
        });

        loop {
            tokio::select! {
                biased;
                Some(command) = command_rx.recv() => {
                    match command {
                        UsbDeviceCommand::SetBrightness {
                            brightness,
                            response_tx,
                        } => {
                            let result = if let Some(commands) = protocol.encode_brightness(brightness) {
                                let first_packet = commands.first().map_or_else(
                                    || "<none>".to_owned(),
                                    |command| describe_packet(&command.data),
                                );
                                debug!(
                                    device_id = %device_id,
                                    device = device_name,
                                    protocol = protocol.name(),
                                    transport = transport.name(),
                                    brightness,
                                    command_count = commands.len(),
                                    first_packet = %first_packet,
                                    "usb brightness write requested"
                                );

                                Self::run_commands(protocol.as_ref(), transport.as_ref(), commands)
                                    .await
                                    .with_context(|| format!("USB brightness write failed for device {device_id}"))
                            } else {
                                Err(anyhow!(
                                    "USB protocol does not support brightness for device {device_id}"
                                ))
                            };

                            let response = result.as_ref().map(|_| ()).map_err(ToString::to_string);
                            let _ = response_tx.send(response);
                            result?;
                        }
                        UsbDeviceCommand::Shutdown {
                            led_count,
                            response_tx,
                        } => {
                            let result = Self::run_shutdown_sequence(
                                device_id,
                                device_name,
                                led_count,
                                protocol.as_ref(),
                                transport.as_ref(),
                            )
                            .await;
                            let response = result.as_ref().map(|_| ()).map_err(ToString::to_string);
                            let _ = response_tx.send(response);
                            return result;
                        }
                    }
                }
                _ = async {
                    if let Some(interval) = keepalive_interval.as_mut() {
                        interval.tick().await;
                    }
                }, if keepalive_interval.is_some() => {
                    let commands = protocol.keepalive_commands();
                    if commands.is_empty() {
                        continue;
                    }

                    trace!(
                        device_id = %device_id,
                        device = device_name,
                        protocol = protocol.name(),
                        transport = transport.name(),
                        command_count = commands.len(),
                        "usb keepalive tick"
                    );

                    Self::run_commands(protocol.as_ref(), transport.as_ref(), commands)
                        .await
                        .with_context(|| format!("USB keepalive failed for device {device_id}"))?;
                }
                changed = display_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let Some(frame) = display_rx.borrow_and_update().clone() else {
                        continue;
                    };

                    Self::run_device_display_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                    )
                    .await?;
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    let Some(frame) = frame_rx.borrow_and_update().clone() else {
                        continue;
                    };

                    Self::run_device_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                    )
                    .await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    async fn run_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbFramePayload,
    ) -> Result<()> {
        let commands = protocol.encode_frame(&frame.colors);
        let first_packet = commands.first().map_or_else(
            || "<none>".to_owned(),
            |command| describe_packet(&command.data),
        );

        trace!(
            device_id = %device_id,
            protocol = protocol.name(),
            transport = transport.name(),
            led_count = frame.colors.len(),
            command_count = commands.len(),
            first_packet = %first_packet,
            "usb frame write requested"
        );

        Self::run_commands(protocol, transport, commands)
            .await
            .with_context(|| format!("USB frame write failed for device {device_id}"))
    }

    async fn run_device_display_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbDisplayPayload,
    ) -> Result<()> {
        let commands = protocol
            .encode_display_frame(&frame.jpeg_data)
            .with_context(|| {
                format!("USB protocol does not support display output for device {device_id}")
            })?;
        let first_packet = commands.first().map_or_else(
            || "<none>".to_owned(),
            |command| describe_packet(&command.data),
        );

        trace!(
            device_id = %device_id,
            protocol = protocol.name(),
            transport = transport.name(),
            jpeg_bytes = frame.jpeg_data.len(),
            command_count = commands.len(),
            first_packet = %first_packet,
            "usb display write requested"
        );

        Self::run_commands(protocol, transport, commands)
            .await
            .with_context(|| format!("USB display write failed for device {device_id}"))
    }

    async fn run_shutdown_sequence(
        device_id: DeviceId,
        device_name: &'static str,
        led_count: usize,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
    ) -> Result<()> {
        if led_count > 0 {
            let black_frame = vec![[0, 0, 0]; led_count];
            if let Err(error) = Self::run_device_frame(
                device_id,
                protocol,
                transport,
                &UsbFramePayload {
                    colors: black_frame,
                },
            )
            .await
            {
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    "USB final clear frame failed during shutdown"
                );
            }
        }

        let shutdown = protocol.shutdown_sequence();
        if shutdown.is_empty() {
            return Ok(());
        }

        if let Err(error) = Self::run_commands(protocol, transport, shutdown).await {
            warn!(
                device_id = %device_id,
                device = device_name,
                protocol = protocol.name(),
                transport = transport.name(),
                error = %error,
                "USB shutdown sequence failed"
            );
        }

        Ok(())
    }

    fn store_actor_error(last_async_error: &Arc<StdMutex<Option<String>>>, error: String) {
        if let Ok(mut slot) = last_async_error.lock() {
            *slot = Some(error);
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
            transfer_type = ?command.transfer_type,
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
                    transfer_type = ?command.transfer_type,
                    "usb send starting"
                );
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
            trace!(
                protocol = protocol.name(),
                transport = transport.name(),
                command_index = command_position,
                total_commands,
                attempt = *attempt + 1,
                transfer_type = ?command.transfer_type,
                "usb send_receive starting"
            );
            transport
                .send_receive_with_type(
                    &command.data,
                    protocol.response_timeout(),
                    command.transfer_type,
                )
                .await
                .map_err(map_transport_error)?
        } else {
            trace!(
                protocol = protocol.name(),
                transport = transport.name(),
                command_index = command_position,
                total_commands,
                attempt = *attempt + 1,
                transfer_type = ?command.transfer_type,
                response_delay_us = command.response_delay.as_micros(),
                "usb send starting with delayed response read"
            );
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

        let protocol: Arc<dyn Protocol> = Arc::from(self.build_protocol(&pending, *id).await);
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

        let transport_name = transport.name();
        let resolved_info =
            build_connected_device_info(*id, &pending.info_template, protocol.as_ref());
        let zone_summary = resolved_info
            .zones
            .iter()
            .map(|zone| format!("{}:{}:{:?}", zone.name, zone.led_count, zone.topology))
            .collect::<Vec<_>>();
        debug!(
            device_id = %id,
            descriptor = pending.descriptor.name,
            protocol = protocol.name(),
            transport = transport_name,
            total_leds = resolved_info.total_led_count(),
            zone_count = resolved_info.zones.len(),
            zones = ?zone_summary,
            "USB connect resolved protocol topology"
        );
        let target_fps = fps_from_frame_interval(protocol.frame_interval());
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let last_async_error = Arc::new(StdMutex::new(None));

        if let Some(keepalive) = protocol.keepalive() {
            debug!(
                device_id = %id,
                protocol = protocol.name(),
                transport = transport.name(),
                interval_ms = keepalive.interval.as_millis(),
                command_count = keepalive.commands.len(),
                "starting USB device actor with keepalive"
            );
        }

        let actor_task = Self::spawn_device_actor(
            *id,
            pending.descriptor.name,
            protocol.clone(),
            transport,
            frame_rx,
            display_rx,
            command_rx,
            Arc::clone(&last_async_error),
        );

        self.connected.insert(
            *id,
            UsbDevice {
                protocol,
                transport_name,
                target_fps,
                frame_tx,
                display_tx,
                command_tx,
                actor_task: Some(actor_task),
                last_async_error,
                info_template: pending.info_template,
                frame_diagnostics_emitted: false,
                non_black_frame_diagnostics_emitted: false,
            },
        );

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(mut device) = self.connected.remove(id) else {
            self.pending.remove(id);
            return Ok(());
        };

        let disconnect_result = device.shutdown(*id).await;
        self.pending.remove(id);
        disconnect_result
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let Some(device) = self.connected.get_mut(id) else {
            bail!("device {id} is not connected");
        };

        device.ensure_actor_ready(*id).await?;

        let frame_stats = summarize_frame(colors);
        if !device.frame_diagnostics_emitted {
            debug!(
                device_id = %id,
                protocol = device.protocol.name(),
                transport = device.transport_name,
                led_count = colors.len(),
                lit_led_count = frame_stats.lit_led_count,
                max_channel = frame_stats.max_channel,
                first_lit = frame_stats.first_lit.as_deref().unwrap_or("<none>"),
                sample = %frame_stats.sample,
                "usb first frame diagnostics"
            );
            device.frame_diagnostics_emitted = true;
        }
        if frame_stats.lit_led_count > 0 && !device.non_black_frame_diagnostics_emitted {
            info!(
                device_id = %id,
                protocol = device.protocol.name(),
                transport = device.transport_name,
                led_count = colors.len(),
                lit_led_count = frame_stats.lit_led_count,
                max_channel = frame_stats.max_channel,
                first_lit = frame_stats.first_lit.as_deref().unwrap_or("<none>"),
                sample = %frame_stats.sample,
                "usb first non-black frame observed"
            );
            device.non_black_frame_diagnostics_emitted = true;
        }
        trace!(
            device_id = %id,
            protocol = device.protocol.name(),
            transport = device.transport_name,
            led_count = colors.len(),
            lit_led_count = frame_stats.lit_led_count,
            "usb frame queued for device actor"
        );

        device.queue_colors(colors);
        Ok(())
    }

    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let Some(device) = self.connected.get_mut(id) else {
            bail!("device {id} is not connected");
        };

        if !device.info_template.capabilities.has_display {
            bail!("USB protocol does not support display output for device {id}");
        }

        device.ensure_actor_ready(*id).await?;
        trace!(
            device_id = %id,
            protocol = device.protocol.name(),
            transport = device.transport_name,
            jpeg_bytes = jpeg_data.len(),
            "usb display frame queued for device actor"
        );

        device.queue_display_frame(jpeg_data);
        Ok(())
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let Some(device) = self.connected.get_mut(id) else {
            bail!("device {id} is not connected");
        };

        device.set_brightness(*id, brightness).await
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

struct FrameStats {
    lit_led_count: usize,
    max_channel: u8,
    first_lit: Option<String>,
    sample: String,
}

fn summarize_frame(colors: &[[u8; 3]]) -> FrameStats {
    let lit_led_count = colors
        .iter()
        .filter(|color| color.iter().any(|component| *component > 0))
        .count();
    let max_channel = colors
        .iter()
        .flat_map(|color| color.iter())
        .copied()
        .max()
        .unwrap_or(0);
    let first_lit = colors.iter().enumerate().find_map(|(index, color)| {
        color
            .iter()
            .any(|component| *component > 0)
            .then(|| format!("#{index}={:02X}{:02X}{:02X}", color[0], color[1], color[2]))
    });
    let sample = colors
        .iter()
        .take(4)
        .enumerate()
        .map(|(index, color)| format!("#{index}={:02X}{:02X}{:02X}", color[0], color[1], color[2]))
        .collect::<Vec<_>>()
        .join(", ");

    FrameStats {
        lit_led_count,
        max_channel,
        first_lit,
        sample,
    }
}

fn fps_from_frame_interval(frame_interval: Duration) -> Option<u32> {
    if frame_interval.is_zero() {
        return None;
    }

    let nanos = frame_interval.as_nanos();
    if nanos == 0 {
        return None;
    }

    let frames_per_second = (1_000_000_000_u128 / nanos).max(1);
    Some(u32::try_from(frames_per_second).unwrap_or(u32::MAX))
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
        info_template: discovered.info.clone(),
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
