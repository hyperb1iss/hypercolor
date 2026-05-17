//! USB backend that bridges HAL protocols to the core `DeviceBackend` trait.

use std::cmp::min;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_hal::database::{DeviceDescriptor, TransportType};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, ResponseStatus};
use hypercolor_hal::protocol_config::{
    ProtocolRuntimeConfig, runtime_config_for_attachment_profile,
};
use hypercolor_hal::transport::bulk::UsbBulkTransport;
use hypercolor_hal::transport::control::UsbControlTransport;
use hypercolor_hal::transport::hid::UsbHidTransport;
use hypercolor_hal::transport::hidapi::UsbHidApiTransport;
use hypercolor_hal::transport::midi::Push2Transport;
use hypercolor_hal::transport::serial::UsbSerialTransport;
use hypercolor_hal::transport::vendor::UsbVendorTransport;
use hypercolor_hal::transport::{Transport, TransportError};
use hypercolor_types::attachment::DeviceAttachmentProfile;
use hypercolor_types::device::{
    DeviceId, DeviceInfo, OwnedDisplayFramePayload, USB_OUTPUT_BACKEND_ID, ZoneInfo,
};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, trace, warn};

#[cfg(target_os = "linux")]
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

use super::traits::{BackendInfo, DeviceBackend, DeviceDisplaySink, DeviceFrameSink};
use super::usb_scanner::UsbScanner;
use super::{DiscoveredDevice, TransportScanner};
use crate::attachment::AttachmentRegistry;

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsbActorMetricsSnapshot {
    pub display_frames_total: u64,
    pub display_frames_delayed_for_led_total: u64,
    pub display_led_priority_wait_total_us: u64,
    pub display_led_priority_wait_max_us: u64,
}

static USB_DISPLAY_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static USB_DISPLAY_FRAMES_DELAYED_FOR_LED_TOTAL: AtomicU64 = AtomicU64::new(0);
static USB_DISPLAY_LED_PRIORITY_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static USB_DISPLAY_LED_PRIORITY_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);

#[must_use]
pub fn usb_actor_metrics_snapshot() -> UsbActorMetricsSnapshot {
    UsbActorMetricsSnapshot {
        display_frames_total: USB_DISPLAY_FRAMES_TOTAL.load(Ordering::Relaxed),
        display_frames_delayed_for_led_total: USB_DISPLAY_FRAMES_DELAYED_FOR_LED_TOTAL
            .load(Ordering::Relaxed),
        display_led_priority_wait_total_us: USB_DISPLAY_LED_PRIORITY_WAIT_TOTAL_US
            .load(Ordering::Relaxed),
        display_led_priority_wait_max_us: USB_DISPLAY_LED_PRIORITY_WAIT_MAX_US
            .load(Ordering::Relaxed),
    }
}

fn record_usb_display_lane(wait_for_led: Duration, delayed_for_led: bool) {
    USB_DISPLAY_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);

    if !delayed_for_led {
        return;
    }

    let wait_us = duration_micros(wait_for_led);
    USB_DISPLAY_FRAMES_DELAYED_FOR_LED_TOTAL.fetch_add(1, Ordering::Relaxed);
    USB_DISPLAY_LED_PRIORITY_WAIT_TOTAL_US.fetch_add(wait_us, Ordering::Relaxed);
    USB_DISPLAY_LED_PRIORITY_WAIT_MAX_US.fetch_max(wait_us, Ordering::Relaxed);
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

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
    colors: Arc<Vec<[u8; 3]>>,
}

#[derive(Debug)]
struct UsbDisplayPayload {
    payload: Arc<OwnedDisplayFramePayload>,
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
    active: Arc<AtomicBool>,
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

    fn queue_colors(&self, colors: Arc<Vec<[u8; 3]>>) {
        self.frame_tx
            .send_replace(Some(Arc::new(UsbFramePayload { colors })));
    }

    fn frame_sink(&self, device_id: DeviceId) -> Arc<dyn DeviceFrameSink> {
        Arc::new(UsbFrameSink {
            device_id,
            frame_tx: self.frame_tx.clone(),
            active: Arc::clone(&self.active),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn display_sink(&self, device_id: DeviceId) -> Arc<dyn DeviceDisplaySink> {
        Arc::new(UsbDisplaySink {
            device_id,
            display_tx: self.display_tx.clone(),
            active: Arc::clone(&self.active),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn queue_display_frame(&self, payload: Arc<OwnedDisplayFramePayload>) {
        self.display_tx
            .send_replace(Some(Arc::new(UsbDisplayPayload { payload })));
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
        self.active.store(false, Ordering::Release);
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

struct UsbFrameSink {
    device_id: DeviceId,
    frame_tx: watch::Sender<Option<Arc<UsbFramePayload>>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for UsbFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            bail!(
                "USB device actor is not running for device {}",
                self.device_id
            );
        }

        if let Some(error) = self
            .last_async_error
            .lock()
            .map_err(|_| anyhow!("USB device async error state lock poisoned"))?
            .clone()
        {
            bail!("{error}");
        }

        self.frame_tx
            .send_replace(Some(Arc::new(UsbFramePayload { colors })));
        Ok(())
    }
}

struct UsbDisplaySink {
    device_id: DeviceId,
    display_tx: watch::Sender<Option<Arc<UsbDisplayPayload>>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl DeviceDisplaySink for UsbDisplaySink {
    async fn write_display_payload_owned(
        &self,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            bail!(
                "USB device actor is not running for device {}",
                self.device_id
            );
        }

        if let Some(error) = self
            .last_async_error
            .lock()
            .map_err(|_| anyhow!("USB device async error state lock poisoned"))?
            .clone()
        {
            bail!("{error}");
        }

        self.display_tx
            .send_replace(Some(Arc::new(UsbDisplayPayload { payload })));
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct UsbProtocolConfigStore {
    configs: Arc<RwLock<HashMap<DeviceId, ProtocolRuntimeConfig>>>,
}

impl UsbProtocolConfigStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_config(&self, device_id: DeviceId, config: ProtocolRuntimeConfig) {
        let mut configs = self.configs.write().await;
        configs.insert(device_id, config);
    }

    pub async fn config(&self, device_id: DeviceId) -> Option<ProtocolRuntimeConfig> {
        let configs = self.configs.read().await;
        configs.get(&device_id).copied()
    }

    pub async fn remove_device(&self, device_id: DeviceId) {
        let mut configs = self.configs.write().await;
        configs.remove(&device_id);
    }

    pub async fn apply_attachment_profile(
        &self,
        device_id: DeviceId,
        device: &DeviceInfo,
        profile: &DeviceAttachmentProfile,
        registry: &AttachmentRegistry,
    ) -> bool {
        let Some(config) = runtime_config_for_attachment_profile(device, profile, |binding| {
            registry
                .get(&binding.template_id)
                .map(|template| binding.effective_led_count(template))
        }) else {
            return false;
        };

        self.set_config(device_id, config).await;
        true
    }
}

impl UsbBackend {
    async fn configured_protocol(
        &self,
        protocol_id: &str,
        device_id: DeviceId,
    ) -> Option<Box<dyn Protocol>> {
        let config = self.protocol_configs.config(device_id).await?;
        (config.protocol_id() == protocol_id).then(|| config.build_protocol())
    }
}

/// Core USB backend for HAL-managed device families.
#[derive(Default)]
pub struct UsbBackend {
    pending: HashMap<DeviceId, PendingUsbDevice>,
    connected: HashMap<DeviceId, UsbDevice>,
    protocol_configs: UsbProtocolConfigStore,
    enabled_driver_ids: Option<BTreeSet<String>>,
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

    #[must_use]
    pub fn with_protocol_config_store_and_enabled_driver_ids(
        protocol_configs: UsbProtocolConfigStore,
        enabled_driver_ids: BTreeSet<String>,
    ) -> Self {
        Self {
            protocol_configs,
            enabled_driver_ids: Some(enabled_driver_ids),
            ..Self::default()
        }
    }

    async fn build_protocol(
        &self,
        pending: &PendingUsbDevice,
        device_id: DeviceId,
    ) -> Box<dyn Protocol> {
        if let Some(protocol) = self
            .configured_protocol(pending.descriptor.protocol.id, device_id)
            .await
        {
            return protocol;
        }

        (pending.descriptor.protocol.build)()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "transport construction dispatches across multiple backend types"
    )]
    async fn build_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
    ) -> Result<Box<dyn Transport>> {
        match pending.descriptor.transport {
            TransportType::UsbHidApi {
                interface,
                report_id,
                report_mode,
                max_report_len,
                usage_page,
                usage,
            } => Self::open_hidapi_transport(
                pending,
                interface,
                report_id,
                report_mode,
                max_report_len,
                usage_page,
                usage,
            ),
            TransportType::UsbHidRaw {
                interface,
                report_id,
                report_mode,
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
                        report_mode,
                        pending.serial.as_deref(),
                        pending.usb_path.as_deref(),
                        usage_page,
                        usage,
                    )
                    .await
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
                        report_mode = ?report_mode,
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
                    let _ = (interface, report_id, report_mode, usage_page, usage, usb);
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
            TransportType::UsbMidi {
                midi_interface,
                display_interface,
                display_endpoint,
            } => {
                Self::open_midi_transport(
                    pending,
                    usb,
                    midi_interface,
                    display_interface,
                    display_endpoint,
                )
                .await
            }
            TransportType::UsbSerial { baud_rate } => {
                Self::open_serial_transport(pending, baud_rate)
            }
            TransportType::I2cSmBus { address } => {
                let _ = usb;
                bail!(
                    "SMBus transport 0x{address:02X} is not supported by the USB backend; use a dedicated SMBus backend"
                );
            }
            TransportType::UsbVendor => Self::open_vendor_transport(pending, usb).await,
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

    async fn open_midi_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
        midi_interface: u8,
        display_interface: u8,
        display_endpoint: u8,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        let transport = Push2Transport::new(
            device,
            pending.vendor_id,
            pending.product_id,
            pending.serial.as_deref(),
            pending.usb_path.as_deref(),
            midi_interface,
            display_interface,
            display_endpoint,
        )
        .await
        .with_context(|| {
            format!(
                "failed to open USB MIDI transport for {:04X}:{:04X} (midi_interface={}, display_interface={}, display_endpoint=0x{display_endpoint:02X})",
                pending.vendor_id, pending.product_id, midi_interface, display_interface
            )
        })?;
        Ok(Box::new(transport))
    }

    async fn open_vendor_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        Ok(Box::new(UsbVendorTransport::new(device)))
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

    #[expect(
        clippy::too_many_arguments,
        reason = "actor bootstrap needs the transport, channels, ids, and shared error sink together"
    )]
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
            let parallel_transfer_lanes = transport.supports_parallel_transfer_lanes();

            let actor_result = if parallel_transfer_lanes {
                Self::run_parallel_device_actor(
                    device_id,
                    device_name,
                    protocol.clone(),
                    transport.clone(),
                    frame_rx,
                    display_rx,
                    command_rx,
                )
                .await
            } else {
                Self::run_device_actor(
                    device_id,
                    device_name,
                    protocol.clone(),
                    transport.clone(),
                    frame_rx,
                    display_rx,
                    command_rx,
                )
                .await
            };

            if let Err(error) = actor_result {
                Self::store_actor_error(&last_async_error, error.to_string());
                warn!(
                    device_id = %device_id,
                    device = device_name,
                    protocol = protocol.name(),
                    transport = transport_name,
                    parallel_transfer_lanes,
                    error = %error,
                    error_chain = %format_error_chain(&error),
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

    async fn run_parallel_device_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
        display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
        command_rx: mpsc::UnboundedReceiver<UsbDeviceCommand>,
    ) -> Result<()> {
        let mut control_task = tokio::spawn(Self::run_device_control_actor(
            device_id,
            device_name,
            Arc::clone(&protocol),
            Arc::clone(&transport),
            frame_rx,
            command_rx,
        ));
        let mut display_task = tokio::spawn(Self::run_device_display_actor(
            device_id, protocol, transport, display_rx,
        ));

        tokio::select! {
            result = &mut control_task => {
                display_task.abort();
                let _ = display_task.await;
                Self::flatten_actor_result(result, "USB control actor")
            }
            result = &mut display_task => {
                match Self::flatten_actor_result(result, "USB display actor") {
                    Ok(()) => debug!(
                        device_id = %device_id,
                        device = device_name,
                        "USB display actor exited; control lane remains active"
                    ),
                    Err(error) => warn!(
                        device_id = %device_id,
                        device = device_name,
                        error = %error,
                        error_chain = %format_error_chain(&error),
                        "USB display actor failed; keeping control lane active"
                    ),
                }
                Self::flatten_actor_result(control_task.await, "USB control actor")
            }
        }
    }

    fn flatten_actor_result(
        result: std::result::Result<Result<()>, tokio::task::JoinError>,
        lane_name: &'static str,
    ) -> Result<()> {
        result.unwrap_or_else(|error| Err(anyhow!("{lane_name} task failed: {error}")))
    }

    async fn run_device_control_actor(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut frame_rx: watch::Receiver<Option<Arc<UsbFramePayload>>>,
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
        let mut frame_commands = Vec::new();

        loop {
            tokio::select! {
                biased;
                Some(command) = command_rx.recv() => {
                    match command {
                        UsbDeviceCommand::SetBrightness {
                            brightness,
                            response_tx,
                        } => {
                            let result = Self::run_brightness_command(
                                device_id,
                                device_name,
                                protocol.as_ref(),
                                transport.as_ref(),
                                brightness,
                            )
                            .await;

                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
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
                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            return result;
                        }
                    }
                }
                () = async {
                    if let Some(interval) = keepalive_interval.as_mut() {
                        interval.tick().await;
                    }
                }, if keepalive_interval.is_some() => {
                    Self::run_keepalive_commands(
                        device_id,
                        device_name,
                        protocol.as_ref(),
                        transport.as_ref(),
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
                        &mut frame_commands,
                    )
                    .await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    async fn run_device_display_actor(
        device_id: DeviceId,
        protocol: Arc<dyn Protocol>,
        transport: Arc<dyn Transport>,
        mut display_rx: watch::Receiver<Option<Arc<UsbDisplayPayload>>>,
    ) -> Result<()> {
        let mut display_commands = Vec::new();

        loop {
            let changed = display_rx.changed().await;
            if changed.is_err() {
                break;
            }

            let Some(frame) = display_rx.borrow_and_update().clone() else {
                continue;
            };

            record_usb_display_lane(Duration::ZERO, false);
            if let Err(error) = Self::run_device_display_frame(
                device_id,
                protocol.as_ref(),
                transport.as_ref(),
                &frame,
                &mut display_commands,
            )
            .await
            {
                warn!(
                    device_id = %device_id,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    error_chain = %format_error_chain(&error),
                    "USB display frame write failed; display lane will continue"
                );
            }
        }

        Ok(())
    }

    async fn run_brightness_command(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        brightness: u8,
    ) -> Result<()> {
        if let Some(commands) = protocol.encode_brightness(brightness) {
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

            Self::run_commands(protocol, transport, commands.as_slice())
                .await
                .with_context(|| format!("USB brightness write failed for device {device_id}"))
        } else {
            Err(anyhow!(
                "USB protocol does not support brightness for device {device_id}"
            ))
        }
    }

    async fn run_keepalive_commands(
        device_id: DeviceId,
        device_name: &'static str,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
    ) -> Result<()> {
        let commands = protocol.keepalive_commands();
        if commands.is_empty() {
            return Ok(());
        }

        trace!(
            device_id = %device_id,
            device = device_name,
            protocol = protocol.name(),
            transport = transport.name(),
            command_count = commands.len(),
            "usb keepalive tick"
        );

        Self::run_commands(protocol, transport, commands.as_slice())
            .await
            .with_context(|| format!("USB keepalive failed for device {device_id}"))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "device actor loop coordinates command, keepalive, frame, and display streams in one place"
    )]
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
        let mut frame_commands = Vec::new();
        let mut display_commands = Vec::new();

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

                                Self::run_commands(
                                    protocol.as_ref(),
                                    transport.as_ref(),
                                    commands.as_slice(),
                                )
                                    .await
                                    .with_context(|| format!("USB brightness write failed for device {device_id}"))
                            } else {
                                Err(anyhow!(
                                    "USB protocol does not support brightness for device {device_id}"
                                ))
                            };

                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
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
                            let response =
                                result.as_ref().map_err(ToString::to_string).copied();
                            let _ = response_tx.send(response);
                            return result;
                        }
                    }
                }
                () = async {
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

                    Self::run_commands(
                        protocol.as_ref(),
                        transport.as_ref(),
                        commands.as_slice(),
                    )
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

                    let wait_for_led_started = Instant::now();
                    let delayed_for_led = Self::run_overdue_device_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &mut frame_rx,
                        &mut frame_commands,
                    )
                    .await?;
                    record_usb_display_lane(wait_for_led_started.elapsed(), delayed_for_led);

                    if let Err(error) = Self::run_device_display_frame(
                        device_id,
                        protocol.as_ref(),
                        transport.as_ref(),
                        &frame,
                        &mut display_commands,
                    )
                    .await
                    {
                        warn!(
                            device_id = %device_id,
                            device = device_name,
                            protocol = protocol.name(),
                            transport = transport.name(),
                            error = %error,
                            error_chain = %format_error_chain(&error),
                            "USB display frame write failed; LED lane will continue"
                        );
                    }
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
                        &mut frame_commands,
                    )
                    .await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    async fn run_overdue_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame_rx: &mut watch::Receiver<Option<Arc<UsbFramePayload>>>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<bool> {
        if !frame_rx.has_changed().unwrap_or(false) {
            return Ok(false);
        }

        let Some(frame) = frame_rx.borrow_and_update().clone() else {
            return Ok(false);
        };

        Self::run_device_frame(device_id, protocol, transport, &frame, commands)
            .await
            .map(|()| true)
    }

    async fn run_device_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbFramePayload,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<()> {
        protocol.encode_frame_into(frame.colors.as_slice(), commands);
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

        Self::run_commands(protocol, transport, commands.as_slice())
            .await
            .with_context(|| format!("USB frame write failed for device {device_id}"))
    }

    async fn run_device_display_frame(
        device_id: DeviceId,
        protocol: &dyn Protocol,
        transport: &dyn Transport,
        frame: &UsbDisplayPayload,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<()> {
        protocol
            .encode_display_payload_into(frame.payload.as_borrowed(), commands)
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
            display_format = %frame.payload.format,
            display_bytes = frame.payload.data.len(),
            command_count = commands.len(),
            first_packet = %first_packet,
            "usb display write requested"
        );

        Self::run_commands(protocol, transport, commands.as_slice())
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
            let mut commands = Vec::new();
            if let Err(error) = Self::run_device_frame(
                device_id,
                protocol,
                transport,
                &UsbFramePayload {
                    colors: Arc::new(black_frame),
                },
                &mut commands,
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

        if let Err(error) = Self::run_commands(protocol, transport, shutdown.as_slice()).await {
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
        commands: &[ProtocolCommand],
    ) -> Result<()> {
        let total_commands = commands.len();

        for (index, command) in commands.iter().enumerate() {
            let command_position = index + 1;
            Self::trace_queued_command(
                protocol,
                transport,
                command,
                command_position,
                total_commands,
            );
            Self::run_command(
                protocol,
                transport,
                command,
                command_position,
                total_commands,
            )
            .await?;
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
                if !command.post_delay.is_zero() {
                    tokio::time::sleep(command.post_delay).await;
                }
                return Ok(());
            }

            if !command.post_delay.is_zero() {
                tokio::time::sleep(command.post_delay).await;
            }

            return Ok(());
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "response handling keeps retry, delayed reads, parsing, and tracing in one place"
    )]
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
                    attempt = *attempt + 1,
                    transfer_type = ?command.transfer_type,
                    expects_response = command.expects_response,
                    command = %describe_packet(&command.data),
                    command_hex = %format_hex_preview(&command.data, 32),
                    response_len = response.len(),
                    error = %error,
                    response = %describe_packet(&response),
                    response_hex = %format_hex_preview(&response, 32),
                    "protocol response parse failed"
                );
                Err(anyhow!("protocol response parse failed: {error}"))
            }
        }
    }

    fn open_hidapi_transport(
        pending: &PendingUsbDevice,
        interface: Option<u8>,
        report_id: u8,
        report_mode: hypercolor_hal::registry::HidRawReportMode,
        max_report_len: usize,
        usage_page: Option<u16>,
        usage: Option<u16>,
    ) -> Result<Box<dyn Transport>> {
        let transport = UsbHidApiTransport::open(
            pending.vendor_id,
            pending.product_id,
            interface,
            report_id,
            report_mode,
            max_report_len,
            pending.serial.as_deref(),
            pending.usb_path.as_deref(),
            usage_page,
            usage,
        )
        .with_context(|| {
            format!(
                "failed to open HIDAPI transport for {:04X}:{:04X} interface {} (report_id=0x{report_id:02X}, usage_page={}, usage={})",
                pending.vendor_id,
                pending.product_id,
                interface.map_or_else(|| "<any>".to_owned(), |value| value.to_string()),
                usage_page
                    .map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                usage.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}"))
            )
        })?;
        Ok(Box::new(transport))
    }
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .skip(1)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | caused_by: ")
}

#[async_trait::async_trait]
impl DeviceBackend for UsbBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: USB_OUTPUT_BACKEND_ID.to_owned(),
            name: "USB HID (HAL)".to_owned(),
            description: "Native USB devices via HAL protocol + transport".to_owned(),
        }
    }

    fn supports_host_attachment_profiles(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn discover(&mut self) -> Result<Vec<hypercolor_types::device::DeviceInfo>> {
        let mut scanner = self
            .enabled_driver_ids
            .as_ref()
            .map_or_else(UsbScanner::new, |ids| {
                UsbScanner::with_enabled_driver_ids(ids.clone())
            });
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

    fn remember_discovered_device(&mut self, discovered: &DiscoveredDevice) {
        if let Some(pending) = pending_from_discovered(discovered) {
            self.pending.insert(discovered.info.id, pending);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "USB connect owns discovery handoff, init, diagnostics, and actor startup"
    )]
    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if let Some(device) = self.connected.get_mut(id) {
            device.ensure_actor_ready(*id).await.with_context(|| {
                format!("USB device {id} is already connected but its actor is unhealthy")
            })?;
            debug!(device_id = %id, "USB device already connected; skipping duplicate connect");
            return Ok(());
        }

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

        Self::run_commands(
            protocol.as_ref(),
            transport.as_ref(),
            init_sequence.as_slice(),
        )
        .await
        .with_context(|| {
            format!(
                "failed to run init sequence for {}",
                pending.descriptor.name
            )
        })?;

        let connection_diagnostics = protocol.connection_diagnostics();
        if !connection_diagnostics.is_empty() {
            debug!(
                device_id = %id,
                descriptor = pending.descriptor.name,
                protocol = protocol.name(),
                transport = transport.name(),
                command_count = connection_diagnostics.len(),
                "running USB post-connect diagnostic probe for write-only path"
            );

            match Self::run_commands(
                protocol.as_ref(),
                transport.as_ref(),
                connection_diagnostics.as_slice(),
            )
            .await
            {
                Ok(()) => debug!(
                    device_id = %id,
                    descriptor = pending.descriptor.name,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    "USB post-connect diagnostic probe succeeded"
                ),
                Err(error) => debug!(
                    device_id = %id,
                    descriptor = pending.descriptor.name,
                    protocol = protocol.name(),
                    transport = transport.name(),
                    error = %error,
                    "USB post-connect diagnostic probe failed; first frame diagnostics will confirm write path"
                ),
            }
        }

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
        let active = Arc::new(AtomicBool::new(true));
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
                active,
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
        self.write_colors_shared(id, Arc::new(colors.to_vec()))
            .await
    }

    async fn write_colors_shared(
        &mut self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<()> {
        let Some(device) = self.connected.get_mut(id) else {
            bail!("device {id} is not connected");
        };

        device.ensure_actor_ready(*id).await?;

        let frame_stats = summarize_frame(colors.as_slice());
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
        self.write_display_frame_owned(id, Arc::new(jpeg_data.to_vec()))
            .await
    }

    async fn write_display_frame_owned(
        &mut self,
        id: &DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let payload = Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, jpeg_data));
        self.write_display_payload_owned(id, payload).await
    }

    async fn write_display_payload_owned(
        &mut self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
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
            display_format = %payload.format,
            display_bytes = payload.data.len(),
            "usb display frame queued for device actor"
        );

        device.queue_display_frame(payload);
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

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.connected.get(id).map(|device| device.frame_sink(*id))
    }

    fn display_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceDisplaySink>> {
        self.connected
            .get(id)
            .filter(|device| {
                device.info_template.capabilities.has_display
                    && device.active.load(Ordering::Acquire)
                    && device
                        .actor_task
                        .as_ref()
                        .is_some_and(|task| !task.is_finished())
            })
            .map(|device| device.display_sink(*id))
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

fn pending_from_discovered(discovered: &DiscoveredDevice) -> Option<PendingUsbDevice> {
    let vendor_id = parse_u16_hex(discovered.metadata.get("vendor_id")?)?;
    let product_id = parse_u16_hex(discovered.metadata.get("product_id")?)?;
    let descriptor = hypercolor_hal::database::ProtocolDatabase::lookup_with_firmware(
        vendor_id,
        product_id,
        discovered
            .metadata
            .get("product_string")
            .map(String::as_str),
    )?;

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
        layout_hint: zone.layout_hint,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use async_trait::async_trait;
    use hypercolor_hal::protocol::{ProtocolResponse, ProtocolZone, TransferType};
    use hypercolor_types::device::DeviceCapabilities;
    use tokio::sync::Mutex as AsyncMutex;
    use tokio::time::timeout;

    use super::*;

    static USB_ACTOR_METRICS_TEST_LOCK: LazyLock<AsyncMutex<()>> =
        LazyLock::new(|| AsyncMutex::new(()));

    #[tokio::test]
    async fn display_branch_services_pending_led_frame_before_display_frame() {
        let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
        let before = usb_actor_metrics_snapshot();
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: Arc::new(vec![[0x11, 0x22, 0x33]]),
        })));
        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
        })));

        let transport =
            Arc::new(RecordingTransport::default().with_send_delay(Duration::from_millis(5)));
        let actor_protocol: Arc<dyn Protocol> = Arc::new(FairnessProtocol);
        let actor_transport: Arc<dyn Transport> = transport.clone();

        let actor = tokio::spawn(UsbBackend::run_device_actor(
            DeviceId::new(),
            "fairness-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ));

        let writes = wait_for_writes(&transport, 2).await;
        let (response_tx, response_rx) = oneshot::channel();
        command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: 0,
                response_tx,
            })
            .expect("actor command channel should still be open");

        response_rx
            .await
            .expect("shutdown response should be delivered")
            .expect("shutdown should succeed");
        actor
            .await
            .expect("actor task should join")
            .expect("actor should exit cleanly");

        assert_eq!(writes, vec![vec![0x11], vec![0xD1]]);

        let after = usb_actor_metrics_snapshot();
        assert!(after.display_frames_total > before.display_frames_total);
        assert!(
            after.display_frames_delayed_for_led_total
                > before.display_frames_delayed_for_led_total
        );
        assert!(
            after.display_led_priority_wait_total_us > before.display_led_priority_wait_total_us
        );
        assert!(after.display_led_priority_wait_max_us >= before.display_led_priority_wait_max_us);
    }

    #[tokio::test]
    async fn display_load_services_new_led_before_next_display_frame() {
        let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
        let before = usb_actor_metrics_snapshot();
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
        })));

        let transport =
            Arc::new(RecordingTransport::default().with_send_delay(Duration::from_millis(5)));
        let actor_protocol: Arc<dyn Protocol> = Arc::new(FairnessProtocol);
        let actor_transport: Arc<dyn Transport> = transport.clone();

        let actor = tokio::spawn(UsbBackend::run_device_actor(
            DeviceId::new(),
            "display-load-fairness-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ));

        let writes = wait_for_writes(&transport, 1).await;
        assert_eq!(writes, vec![vec![0xD1]]);

        frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: Arc::new(vec![[0x22, 0x33, 0x44]]),
        })));
        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD2]))),
        })));

        let writes = wait_for_writes(&transport, 3).await;
        let (response_tx, response_rx) = oneshot::channel();
        command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: 0,
                response_tx,
            })
            .expect("actor command channel should still be open");

        response_rx
            .await
            .expect("shutdown response should be delivered")
            .expect("shutdown should succeed");
        actor
            .await
            .expect("actor task should join")
            .expect("actor should exit cleanly");

        assert_eq!(writes, vec![vec![0xD1], vec![0x22], vec![0xD2]]);

        let after = usb_actor_metrics_snapshot();
        assert!(after.display_frames_total >= before.display_frames_total + 2);
        assert!(
            after.display_frames_delayed_for_led_total
                > before.display_frames_delayed_for_led_total
        );
    }

    #[tokio::test]
    async fn parallel_transfer_lanes_do_not_wait_for_pending_led_frame_before_display() {
        let _metrics_guard = USB_ACTOR_METRICS_TEST_LOCK.lock().await;
        let before = usb_actor_metrics_snapshot();
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: Arc::new(vec![[0x11, 0x22, 0x33]]),
        })));
        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
        })));

        let transport = Arc::new(
            RecordingTransport::default()
                .with_parallel_transfer_lanes()
                .with_primary_send_delay(Duration::from_millis(200)),
        );
        let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
        let actor_transport: Arc<dyn Transport> = transport.clone();

        let actor = tokio::spawn(UsbBackend::run_parallel_device_actor(
            DeviceId::new(),
            "parallel-fairness-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ));

        let writes = wait_for_writes(&transport, 1).await;
        assert_eq!(writes, vec![vec![0xD1]]);

        let writes = wait_for_writes(&transport, 2).await;
        let (response_tx, response_rx) = oneshot::channel();
        command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: 0,
                response_tx,
            })
            .expect("actor command channel should still be open");

        response_rx
            .await
            .expect("shutdown response should be delivered")
            .expect("shutdown should succeed");
        actor
            .await
            .expect("actor task should join")
            .expect("actor should exit cleanly");

        assert_eq!(writes, vec![vec![0xD1], vec![0x11]]);

        let after = usb_actor_metrics_snapshot();
        assert!(after.display_frames_total > before.display_frames_total);
        assert_eq!(
            after.display_frames_delayed_for_led_total,
            before.display_frames_delayed_for_led_total
        );
    }

    #[tokio::test]
    async fn display_write_failure_does_not_stop_single_lane_led_actor() {
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
        })));

        let transport =
            Arc::new(RecordingTransport::default().with_failed_transfer_type(TransferType::Bulk));
        let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
        let actor_transport: Arc<dyn Transport> = transport.clone();

        let actor = tokio::spawn(UsbBackend::run_device_actor(
            DeviceId::new(),
            "display-failure-single-lane-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: Arc::new(vec![[0x22, 0x33, 0x44]]),
        })));

        assert_eq!(wait_for_writes(&transport, 1).await, vec![vec![0x22]]);

        let (response_tx, response_rx) = oneshot::channel();
        command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: 0,
                response_tx,
            })
            .expect("actor command channel should stay open after display failure");
        response_rx
            .await
            .expect("shutdown response should be delivered")
            .expect("shutdown should succeed");
        actor
            .await
            .expect("actor task should join")
            .expect("actor should exit cleanly");
    }

    #[tokio::test]
    async fn parallel_display_write_failure_does_not_stop_control_lane() {
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        display_tx.send_replace(Some(Arc::new(UsbDisplayPayload {
            payload: Arc::new(OwnedDisplayFramePayload::jpeg(0, 0, Arc::new(vec![0xD1]))),
        })));

        let transport = Arc::new(
            RecordingTransport::default()
                .with_parallel_transfer_lanes()
                .with_failed_transfer_type(TransferType::Bulk),
        );
        let actor_protocol: Arc<dyn Protocol> = Arc::new(ParallelFairnessProtocol);
        let actor_transport: Arc<dyn Transport> = transport.clone();

        let actor = tokio::spawn(UsbBackend::run_parallel_device_actor(
            DeviceId::new(),
            "display-failure-parallel-test-device",
            actor_protocol,
            actor_transport,
            frame_rx,
            display_rx,
            command_rx,
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        frame_tx.send_replace(Some(Arc::new(UsbFramePayload {
            colors: Arc::new(vec![[0x33, 0x44, 0x55]]),
        })));

        assert_eq!(wait_for_writes(&transport, 1).await, vec![vec![0x33]]);

        let (response_tx, response_rx) = oneshot::channel();
        command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: 0,
                response_tx,
            })
            .expect("control command channel should stay open after display failure");
        response_rx
            .await
            .expect("shutdown response should be delivered")
            .expect("shutdown should succeed");
        actor
            .await
            .expect("actor task should join")
            .expect("actor should exit cleanly");
    }

    async fn wait_for_writes(transport: &RecordingTransport, count: usize) -> Vec<Vec<u8>> {
        timeout(Duration::from_secs(1), async {
            loop {
                let writes = transport.writes();
                if writes.len() >= count {
                    return writes;
                }

                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("transport writes should arrive before timeout")
    }

    struct FairnessProtocol;

    impl Protocol for FairnessProtocol {
        fn name(&self) -> &'static str {
            "fairness-test"
        }

        fn init_sequence(&self) -> Vec<ProtocolCommand> {
            Vec::new()
        }

        fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
            Vec::new()
        }

        fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
            vec![test_command(colors.first().map_or(0x11, |color| color[0]))]
        }

        fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
            Some(vec![test_command(
                jpeg_data.first().copied().unwrap_or(0xD1),
            )])
        }

        fn parse_response(
            &self,
            _data: &[u8],
        ) -> std::result::Result<ProtocolResponse, ProtocolError> {
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: Vec::new(),
            })
        }

        fn zones(&self) -> Vec<ProtocolZone> {
            Vec::new()
        }

        fn capabilities(&self) -> DeviceCapabilities {
            DeviceCapabilities::default()
        }

        fn total_leds(&self) -> u32 {
            1
        }

        fn frame_interval(&self) -> Duration {
            Duration::from_millis(16)
        }
    }

    struct ParallelFairnessProtocol;

    impl Protocol for ParallelFairnessProtocol {
        fn name(&self) -> &'static str {
            "parallel-fairness-test"
        }

        fn init_sequence(&self) -> Vec<ProtocolCommand> {
            Vec::new()
        }

        fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
            Vec::new()
        }

        fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
            vec![test_command_with_transfer(
                colors.first().map_or(0x11, |color| color[0]),
                TransferType::Primary,
            )]
        }

        fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
            Some(vec![test_command_with_transfer(
                jpeg_data.first().copied().unwrap_or(0xD1),
                TransferType::Bulk,
            )])
        }

        fn parse_response(
            &self,
            _data: &[u8],
        ) -> std::result::Result<ProtocolResponse, ProtocolError> {
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: Vec::new(),
            })
        }

        fn zones(&self) -> Vec<ProtocolZone> {
            Vec::new()
        }

        fn capabilities(&self) -> DeviceCapabilities {
            DeviceCapabilities::default()
        }

        fn total_leds(&self) -> u32 {
            1
        }

        fn frame_interval(&self) -> Duration {
            Duration::from_millis(16)
        }
    }

    #[derive(Default)]
    struct RecordingTransport {
        writes: Mutex<Vec<Vec<u8>>>,
        send_delay: Duration,
        primary_send_delay: Option<Duration>,
        bulk_send_delay: Option<Duration>,
        parallel_transfer_lanes: bool,
        failed_transfer_type: Option<TransferType>,
    }

    impl RecordingTransport {
        fn with_send_delay(mut self, send_delay: Duration) -> Self {
            self.send_delay = send_delay;
            self
        }

        fn with_primary_send_delay(mut self, send_delay: Duration) -> Self {
            self.primary_send_delay = Some(send_delay);
            self
        }

        fn with_parallel_transfer_lanes(mut self) -> Self {
            self.parallel_transfer_lanes = true;
            self
        }

        const fn with_failed_transfer_type(mut self, transfer_type: TransferType) -> Self {
            self.failed_transfer_type = Some(transfer_type);
            self
        }

        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes
                .lock()
                .expect("recording transport mutex should not be poisoned")
                .clone()
        }

        async fn record_send(&self, data: &[u8], send_delay: Duration) {
            if !send_delay.is_zero() {
                tokio::time::sleep(send_delay).await;
            }
            self.writes
                .lock()
                .expect("recording transport mutex should not be poisoned")
                .push(data.to_vec());
        }

        fn send_delay_for(&self, transfer_type: TransferType) -> Duration {
            match transfer_type {
                TransferType::Primary => self.primary_send_delay.unwrap_or(self.send_delay),
                TransferType::Bulk => self.bulk_send_delay.unwrap_or(self.send_delay),
                TransferType::HidReport => self.send_delay,
            }
        }
    }

    #[async_trait]
    impl Transport for RecordingTransport {
        fn name(&self) -> &'static str {
            "recording-test"
        }

        fn supports_parallel_transfer_lanes(&self) -> bool {
            self.parallel_transfer_lanes
        }

        async fn send(&self, data: &[u8]) -> std::result::Result<(), TransportError> {
            self.record_send(data, self.send_delay).await;
            Ok(())
        }

        async fn send_with_type(
            &self,
            data: &[u8],
            transfer_type: TransferType,
        ) -> std::result::Result<(), TransportError> {
            if self.failed_transfer_type == Some(transfer_type) {
                return Err(TransportError::IoError {
                    detail: format!("injected {transfer_type:?} failure"),
                });
            }
            self.record_send(data, self.send_delay_for(transfer_type))
                .await;
            Ok(())
        }

        async fn receive(
            &self,
            _timeout: Duration,
        ) -> std::result::Result<Vec<u8>, TransportError> {
            Ok(Vec::new())
        }

        async fn close(&self) -> std::result::Result<(), TransportError> {
            Ok(())
        }
    }

    fn test_command(byte: u8) -> ProtocolCommand {
        test_command_with_transfer(byte, TransferType::Primary)
    }

    fn test_command_with_transfer(byte: u8, transfer_type: TransferType) -> ProtocolCommand {
        ProtocolCommand {
            data: vec![byte],
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type,
        }
    }
}
