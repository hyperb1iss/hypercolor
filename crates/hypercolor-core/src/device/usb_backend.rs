//! USB backend that bridges HAL protocols to the core `DeviceBackend` trait.

mod actor;

use std::cmp::min;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_hal::database::{DeviceDescriptor, TransportType};
use hypercolor_hal::protocol::Protocol;
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
use hypercolor_types::attachment::DeviceComponentProfile;
use hypercolor_types::device::{
    DeviceId, DeviceInfo, OwnedDisplayFramePayload, USB_OUTPUT_BACKEND_ID, ZoneInfo,
};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace};

#[cfg(target_os = "linux")]
use hypercolor_hal::transport::hidraw::UsbHidRawTransport;

use super::traits::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDisplaySink, DeviceFrameSink, DeviceLifecyclePolicy,
};
use super::usb_scanner::UsbScanner;
use super::{DiscoveredDevice, TransportScanner};
use crate::attachment::ComponentRegistry;

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;
const USB_MIDI_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DELIVERY_PENDING: u8 = 0;
const DELIVERY_STARTED: u8 = 1;
const DELIVERY_REJECTED: u8 = 2;

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
    delivery_id: Option<DeviceDeliveryId>,
    delivery_tx: StdMutex<Option<oneshot::Sender<DeviceDeliveryAck>>>,
    delivery_state: AtomicU8,
}

impl UsbFramePayload {
    fn untracked(colors: Arc<Vec<[u8; 3]>>) -> Self {
        Self {
            colors,
            delivery_id: None,
            delivery_tx: StdMutex::new(None),
            delivery_state: AtomicU8::new(DELIVERY_PENDING),
        }
    }

    fn tracked(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        let (delivery_tx, delivery_rx) = oneshot::channel();
        (
            Self {
                colors,
                delivery_id: Some(id),
                delivery_tx: StdMutex::new(Some(delivery_tx)),
                delivery_state: AtomicU8::new(DELIVERY_PENDING),
            },
            delivery_rx,
        )
    }

    fn acknowledge(&self, ack: DeviceDeliveryAck) {
        if let Ok(mut delivery_tx) = self.delivery_tx.lock()
            && let Some(delivery_tx) = delivery_tx.take()
        {
            let _ = delivery_tx.send(ack);
        }
    }

    fn reject_pending(&self, error: impl Into<String>) {
        let Some(id) = self.delivery_id else {
            return;
        };
        if self
            .delivery_state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_REJECTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        self.acknowledge(DeviceDeliveryAck::rejected(id, error));
    }

    fn mark_transport_started(&self) -> bool {
        self.delivery_id.is_none()
            || self
                .delivery_state
                .compare_exchange(
                    DELIVERY_PENDING,
                    DELIVERY_STARTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
    }
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
    resolved_led_count: usize,
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
        let previous = self
            .frame_tx
            .send_replace(Some(Arc::new(UsbFramePayload::untracked(colors))));
        if let Some(previous) = previous {
            previous.reject_pending("USB frame was superseded before transport started");
        }
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

        let (response_tx, response_rx) = oneshot::channel();
        let command_sent = self
            .command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: self.resolved_led_count,
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
        self.ensure_ready()?;

        let previous = self
            .frame_tx
            .send_replace(Some(Arc::new(UsbFramePayload::untracked(colors))));
        if let Some(previous) = previous {
            previous.reject_pending("USB frame was superseded before transport started");
        }
        Ok(())
    }

    async fn deliver_colors_shared(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        if let Err(error) = self.ensure_ready() {
            return DeviceDeliveryAck::rejected(id, error.to_string());
        }

        let (payload, delivery_rx) = UsbFramePayload::tracked(id, colors);
        let previous = self.frame_tx.send_replace(Some(Arc::new(payload)));
        if let Some(previous) = previous {
            previous.reject_pending("USB frame was superseded before transport started");
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                format!(
                    "USB device actor terminated before acknowledging delivery for device {}",
                    self.device_id
                ),
            )
        })
    }
}

impl UsbFrameSink {
    fn ensure_ready(&self) -> Result<()> {
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
        profile: &DeviceComponentProfile,
        registry: &ComponentRegistry,
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

fn lifecycle_policy_for_transport(transport: TransportType) -> DeviceLifecyclePolicy {
    if matches!(transport, TransportType::UsbMidi { .. }) {
        return DeviceLifecyclePolicy::default()
            .with_connect_timeout(USB_MIDI_CONNECT_TIMEOUT)
            .with_connect_execution(ConnectExecution::Background)
            .without_connect_timeout_retry();
    }

    DeviceLifecyclePolicy::default()
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

    fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        info.capabilities.supports_direct && info.total_led_count() > 0
    }

    fn lifecycle_policy(&self, info: &DeviceInfo) -> DeviceLifecyclePolicy {
        self.pending
            .get(&info.id)
            .map_or_else(DeviceLifecyclePolicy::default, |pending| {
                lifecycle_policy_for_transport(pending.descriptor.transport)
            })
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
                resolved_led_count: usize::try_from(resolved_info.total_led_count())
                    .unwrap_or_default(),
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
mod tests;
