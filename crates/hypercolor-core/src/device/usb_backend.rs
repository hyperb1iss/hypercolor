//! USB backend that bridges HAL protocols to the core `DeviceBackend` trait.

mod actor;

use std::cmp::min;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_driver_api::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceDisplaySink, DeviceFrameSink, DeviceLifecyclePolicy,
    DiscoveredDevice,
};
use hypercolor_hal::database::{DeviceDescriptor, TransportType};
use hypercolor_hal::protocol::Protocol;
use hypercolor_hal::protocol_config::{
    ProtocolRuntimeConfig, runtime_config_for_attachment_profile,
};
use hypercolor_hal::registry::{
    TransportConnectExecution, UsbTransportBinding, UsbTransportOpenRequest,
};
use hypercolor_hal::transport::bulk::UsbBulkTransport;
use hypercolor_hal::transport::control::UsbControlTransport;
use hypercolor_hal::transport::hid::UsbHidTransport;
use hypercolor_hal::transport::hidapi::UsbHidApiTransport;
use hypercolor_hal::transport::serial::UsbSerialTransport;
use hypercolor_hal::transport::vendor::UsbVendorTransport;
use hypercolor_hal::transport::{HidRawOpenRequest, Transport, TransportError};
use hypercolor_types::attachment::DeviceComponentProfile;
use hypercolor_types::device::{
    DeviceError, DeviceId, DeviceInfo, OwnedDisplayFramePayload, USB_OUTPUT_BACKEND_ID,
};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace};

use super::transport_error::{DeviceTransportOperation, map_hal_transport_error};
use crate::attachment::ComponentRegistry;

const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRIES: u8 = 3;
/// How long to wait for a report left queued by an attempt that is being
/// retried. Short on purpose: the device has usually said all it intends to,
/// and the resend is what recovers the exchange.
const DRAIN_REPORT_TIMEOUT: Duration = Duration::from_millis(20);
const USB_ACTOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_PENDING: u8 = 0;
const DELIVERY_STARTED: u8 = 1;
const DELIVERY_REJECTED: u8 = 2;
const DELIVERY_TERMINAL: u8 = 3;

struct AbortTaskOnDrop(tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

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

struct UsbFramePayload {
    colors: Arc<Vec<[u8; 3]>>,
    delivery_id: Option<DeviceDeliveryId>,
    delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    delivery_tx: StdMutex<Option<oneshot::Sender<DeviceDeliveryAck>>>,
    delivery_state: AtomicU8,
}

impl UsbFramePayload {
    fn untracked(colors: Arc<Vec<[u8; 3]>>) -> Self {
        Self {
            colors,
            delivery_id: None,
            delivery_observer: None,
            delivery_tx: StdMutex::new(None),
            delivery_state: AtomicU8::new(DELIVERY_PENDING),
        }
    }

    fn tracked(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        Self::tracked_observed(id, colors, None)
    }

    fn tracked_observed(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        let (delivery_tx, delivery_rx) = oneshot::channel();
        (
            Self {
                colors,
                delivery_id: Some(id),
                delivery_observer,
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

    fn reject_pending(&self, error: DeviceError) {
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
        let Some(id) = self.delivery_id else {
            return true;
        };
        if self
            .delivery_state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        if let Some(observer) = &self.delivery_observer {
            observer.transport_started(id);
        }
        true
    }
}

struct UsbDisplayPayload {
    payload: Arc<OwnedDisplayFramePayload>,
    delivery_id: Option<DeviceDeliveryId>,
    delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    delivery_tx: StdMutex<Option<oneshot::Sender<DeviceDeliveryAck>>>,
    delivery_state: AtomicU8,
}

impl UsbDisplayPayload {
    fn untracked(payload: Arc<OwnedDisplayFramePayload>) -> Self {
        Self {
            payload,
            delivery_id: None,
            delivery_observer: None,
            delivery_tx: StdMutex::new(None),
            delivery_state: AtomicU8::new(DELIVERY_PENDING),
        }
    }

    fn tracked(
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        Self::tracked_observed(id, payload, None)
    }

    fn tracked_observed(
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
        delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        let (delivery_tx, delivery_rx) = oneshot::channel();
        (
            Self {
                payload,
                delivery_id: Some(id),
                delivery_observer,
                delivery_tx: StdMutex::new(Some(delivery_tx)),
                delivery_state: AtomicU8::new(DELIVERY_PENDING),
            },
            delivery_rx,
        )
    }

    fn acknowledge(&self, ack: DeviceDeliveryAck) {
        if self
            .delivery_state
            .swap(DELIVERY_TERMINAL, Ordering::AcqRel)
            == DELIVERY_TERMINAL
        {
            return;
        }
        if let Some(observer) = &self.delivery_observer {
            observer.delivery_terminal(&ack);
        }
        if let Ok(mut delivery_tx) = self.delivery_tx.lock()
            && let Some(delivery_tx) = delivery_tx.take()
        {
            let _ = delivery_tx.send(ack);
        }
    }

    fn reject_pending(&self, error: DeviceError) {
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

    fn reject_unacknowledged(&self, error: DeviceError) {
        let Some(id) = self.delivery_id else {
            return;
        };
        let state = self
            .delivery_state
            .swap(DELIVERY_REJECTED, Ordering::AcqRel);
        if matches!(state, DELIVERY_REJECTED | DELIVERY_TERMINAL) {
            return;
        }
        self.acknowledge(DeviceDeliveryAck::failed(
            id,
            state == DELIVERY_STARTED,
            Duration::ZERO,
            error,
        ));
    }

    fn mark_transport_started(&self) -> bool {
        let Some(id) = self.delivery_id else {
            return true;
        };
        if self
            .delivery_state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        if let Some(observer) = &self.delivery_observer {
            observer.transport_started(id);
        }
        true
    }
}

enum UsbDeviceCommand {
    SetBrightness {
        brightness: u8,
        response_tx: oneshot::Sender<std::result::Result<(), DeviceError>>,
    },
    Shutdown {
        led_count: usize,
        response_tx: oneshot::Sender<std::result::Result<(), DeviceError>>,
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
    lifecycle_gate: Arc<StdMutex<()>>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
    info_template: DeviceInfo,
    frame_diagnostics_emitted: bool,
    non_black_frame_diagnostics_emitted: bool,
}

impl UsbDevice {
    async fn ensure_actor_ready(
        &mut self,
        device_id: DeviceId,
    ) -> std::result::Result<(), DeviceError> {
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
            self.store_async_error(
                DeviceError::protocol(device_id, format!("USB device actor join failed: {error}")),
                device_id,
            )?;
        }

        if let Some(error) = self.last_async_error(device_id)? {
            return Err(error);
        }

        if self.actor_task.is_none() {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }

        Ok(())
    }

    fn queue_colors(&self, device_id: DeviceId, colors: Arc<Vec<[u8; 3]>>) {
        let previous = self
            .frame_tx
            .send_replace(Some(Arc::new(UsbFramePayload::untracked(colors))));
        if let Some(previous) = previous {
            previous.reject_pending(DeviceError::write(
                device_id,
                "USB frame was superseded before transport started",
            ));
        }
    }

    fn frame_sink(&self, device_id: DeviceId) -> Arc<dyn DeviceFrameSink> {
        Arc::new(UsbFrameSink {
            device_id,
            frame_tx: self.frame_tx.clone(),
            active: Arc::clone(&self.active),
            lifecycle_gate: Arc::clone(&self.lifecycle_gate),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn display_sink(&self, device_id: DeviceId) -> Arc<dyn DeviceDisplaySink> {
        Arc::new(UsbDisplaySink {
            device_id,
            display_tx: self.display_tx.clone(),
            active: Arc::clone(&self.active),
            lifecycle_gate: Arc::clone(&self.lifecycle_gate),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn queue_display_frame(&self, payload: Arc<OwnedDisplayFramePayload>) {
        if let Some(previous) = self
            .display_tx
            .send_replace(Some(Arc::new(UsbDisplayPayload::untracked(payload))))
        {
            previous.reject_pending(DeviceError::write(
                self.info_template.id,
                "USB display frame was superseded before transport started",
            ));
        }
    }

    async fn set_brightness(
        &mut self,
        device_id: DeviceId,
        brightness: u8,
    ) -> std::result::Result<(), DeviceError> {
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
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }

        response_rx.await.map_err(|_| DeviceError::Disconnected {
            device: device_id.to_string(),
        })??;

        self.ensure_actor_ready(device_id).await
    }

    async fn shutdown(&mut self, device_id: DeviceId) -> std::result::Result<(), DeviceError> {
        {
            let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
            self.active.store(false, Ordering::Release);
            if let Some(pending) = self.frame_tx.send_replace(None) {
                pending.reject_pending(DeviceError::Disconnected {
                    device: device_id.to_string(),
                });
            }
        }
        let Some(mut actor_task) = self.actor_task.take() else {
            if let Some(error) = self.last_async_error(device_id)? {
                return Err(error);
            }
            return Ok(());
        };
        let _actor_abort = AbortTaskOnDrop(actor_task.abort_handle());

        let (response_tx, response_rx) = oneshot::channel();
        let command_sent = self
            .command_tx
            .send(UsbDeviceCommand::Shutdown {
                led_count: self.resolved_led_count,
                response_tx,
            })
            .is_ok();

        let shutdown_result = if command_sent {
            match tokio::time::timeout(USB_ACTOR_SHUTDOWN_TIMEOUT, response_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(DeviceError::Disconnected {
                    device: device_id.to_string(),
                }),
                Err(_) => Err(DeviceError::Timeout {
                    after: USB_ACTOR_SHUTDOWN_TIMEOUT,
                }),
            }
        } else {
            Ok(())
        };

        if matches!(&shutdown_result, Err(DeviceError::Timeout { .. })) {
            if let Some(pending) = self.display_tx.send_replace(None) {
                pending.reject_unacknowledged(DeviceError::Timeout {
                    after: USB_ACTOR_SHUTDOWN_TIMEOUT,
                });
            }
            actor_task.abort();
        }

        match tokio::time::timeout(USB_ACTOR_SHUTDOWN_TIMEOUT, &mut actor_task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => {
                self.store_async_error(
                    DeviceError::protocol(
                        device_id,
                        format!("USB device actor join failed: {error}"),
                    ),
                    device_id,
                )?;
            }
            Err(_) => {
                if let Some(pending) = self.display_tx.send_replace(None) {
                    pending.reject_unacknowledged(DeviceError::Timeout {
                        after: USB_ACTOR_SHUTDOWN_TIMEOUT,
                    });
                }
                actor_task.abort();
                let _ = actor_task.await;
                return Err(DeviceError::Timeout {
                    after: USB_ACTOR_SHUTDOWN_TIMEOUT,
                });
            }
        }

        if let Err(error) = shutdown_result {
            self.store_async_error(error.clone(), device_id)?;
            return Err(error);
        }

        if let Some(error) = self.last_async_error(device_id)? {
            return Err(error);
        }

        Ok(())
    }

    fn last_async_error(
        &self,
        device_id: DeviceId,
    ) -> std::result::Result<Option<DeviceError>, DeviceError> {
        self.last_async_error
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| {
                DeviceError::protocol(device_id, "USB device async error state lock poisoned")
            })
    }

    fn store_async_error(
        &self,
        error: DeviceError,
        device_id: DeviceId,
    ) -> std::result::Result<(), DeviceError> {
        let mut slot = self.last_async_error.lock().map_err(|_| {
            DeviceError::protocol(device_id, "USB device async error state lock poisoned")
        })?;
        *slot = Some(error);
        Ok(())
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
        self.active.store(false, Ordering::Release);
        let device_id = self.info_template.id;
        if let Some(pending) = self.frame_tx.send_replace(None) {
            pending.reject_pending(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if let Some(pending) = self.display_tx.send_replace(None) {
            pending.reject_unacknowledged(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if let Some(actor_task) = self.actor_task.take() {
            actor_task.abort();
        }
    }
}

struct UsbFrameSink {
    device_id: DeviceId,
    frame_tx: watch::Sender<Option<Arc<UsbFramePayload>>>,
    active: Arc<AtomicBool>,
    lifecycle_gate: Arc<StdMutex<()>>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for UsbFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        self.publish(Arc::new(UsbFramePayload::untracked(colors)))
    }

    async fn deliver_colors_shared(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        let (payload, delivery_rx) = UsbFramePayload::tracked(id, colors);
        if let Err(error) = self.publish(Arc::new(payload)) {
            return DeviceDeliveryAck::rejected(id, error);
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let (payload, delivery_rx) = UsbFramePayload::tracked_observed(id, colors, Some(observer));
        if let Err(error) = self.publish(Arc::new(payload)) {
            return DeviceDeliveryAck::rejected(id, error);
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }
}

impl UsbFrameSink {
    fn publish(&self, payload: Arc<UsbFramePayload>) -> Result<(), DeviceError> {
        let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
        self.ensure_ready()?;
        if let Some(previous) = self.frame_tx.send_replace(Some(payload)) {
            previous.reject_pending(DeviceError::write(
                self.device_id,
                "USB frame was superseded before transport started",
            ));
        }
        Ok(())
    }

    fn ensure_ready(&self) -> Result<(), DeviceError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: self.device_id.to_string(),
            });
        }

        if let Some(error) = self
            .last_async_error
            .lock()
            .map_err(|_| {
                DeviceError::protocol(self.device_id, "USB async error state lock poisoned")
            })?
            .clone()
        {
            return Err(error);
        }
        Ok(())
    }
}

fn lock_lifecycle_gate(gate: &StdMutex<()>) -> std::sync::MutexGuard<'_, ()> {
    match gate.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct UsbDisplaySink {
    device_id: DeviceId,
    display_tx: watch::Sender<Option<Arc<UsbDisplayPayload>>>,
    active: Arc<AtomicBool>,
    lifecycle_gate: Arc<StdMutex<()>>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
}

#[async_trait::async_trait]
impl DeviceDisplaySink for UsbDisplaySink {
    async fn write_display_payload_owned(
        &self,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<(), DeviceError> {
        self.publish(Arc::new(UsbDisplayPayload::untracked(payload)))
    }

    async fn deliver_display_payload_owned(
        &self,
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> DeviceDeliveryAck {
        let (payload, delivery_rx) = UsbDisplayPayload::tracked(id, payload);
        if let Err(error) = self.publish(Arc::new(payload)) {
            return DeviceDeliveryAck::rejected(id, error);
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }

    async fn deliver_display_payload_owned_observed(
        &self,
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let terminal_observer = Arc::clone(&observer);
        let (payload, delivery_rx) =
            UsbDisplayPayload::tracked_observed(id, payload, Some(observer));
        if let Err(error) = self.publish(Arc::new(payload)) {
            let ack = DeviceDeliveryAck::rejected(id, error);
            terminal_observer.delivery_terminal(&ack);
            return ack;
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }
}

impl UsbDisplaySink {
    fn publish(&self, payload: Arc<UsbDisplayPayload>) -> Result<(), DeviceError> {
        let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
        self.ensure_ready()?;
        if let Some(previous) = self.display_tx.send_replace(Some(payload)) {
            previous.reject_pending(DeviceError::write(
                self.device_id,
                "USB display frame was superseded before transport started",
            ));
        }
        Ok(())
    }

    fn ensure_ready(&self) -> Result<(), DeviceError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: self.device_id.to_string(),
            });
        }

        if let Some(error) = self
            .last_async_error
            .lock()
            .map_err(|_| {
                DeviceError::protocol(self.device_id, "USB async error state lock poisoned")
            })?
            .clone()
        {
            return Err(error);
        }
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
    pending: StdRwLock<HashMap<DeviceId, PendingUsbDevice>>,
    connected: StdRwLock<HashMap<DeviceId, Arc<ConnectedUsbDevice>>>,
    protocol_configs: UsbProtocolConfigStore,
}

struct ConnectedUsbDevice {
    device: tokio::sync::Mutex<UsbDevice>,
    info_template: DeviceInfo,
    protocol: Arc<dyn Protocol>,
    target_fps: Option<u32>,
    frame_sink: Arc<dyn DeviceFrameSink>,
    display_sink: Option<Arc<dyn DeviceDisplaySink>>,
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
        if let Some(protocol) = self
            .configured_protocol(pending.descriptor.protocol.id, device_id)
            .await
        {
            return protocol;
        }

        (pending.descriptor.protocol.build)()
    }

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
                let transport = hypercolor_hal::transport::open_hid_raw_transport(
                    HidRawOpenRequest {
                        vendor_id: pending.vendor_id,
                        product_id: pending.product_id,
                        interface,
                        report_id,
                        report_mode,
                        serial: pending.serial.clone(),
                        usb_path: pending.usb_path.clone(),
                        usage_page,
                        usage,
                    },
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
                Ok(transport)
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
            TransportType::DriverUsb { binding } => {
                Self::open_driver_usb_transport(pending, usb, binding).await
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

    async fn open_driver_usb_transport(
        pending: &PendingUsbDevice,
        usb: &nusb::DeviceInfo,
        binding: UsbTransportBinding,
    ) -> Result<Box<dyn Transport>> {
        let device = Self::open_usb_device(pending, usb).await?;
        (binding.open)(UsbTransportOpenRequest {
            device,
            vendor_id: pending.vendor_id,
            product_id: pending.product_id,
            serial: pending.serial.clone(),
            usb_path: pending.usb_path.clone(),
        })
        .await
        .with_context(|| {
            format!(
                "driver transport {} failed to open for {:04X}:{:04X}",
                binding.id, pending.vendor_id, pending.product_id
            )
        })
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
            // A family whose firmware reports one shared serial for every unit
            // cannot be told apart by serial, and HID enumeration exposes a USB
            // path only on Linux. Opening "the first match" there would bind two
            // discovered devices to one panel and leave another one dark.
            pending.descriptor.serial_quirk.is_some(),
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
    let TransportType::DriverUsb { binding } = transport else {
        return DeviceLifecyclePolicy::default();
    };
    let mut policy = DeviceLifecyclePolicy::default();
    if let Some(connect_timeout) = binding.lifecycle.connect_timeout {
        policy = policy.with_connect_timeout(connect_timeout);
    }
    if binding.lifecycle.connect_execution == TransportConnectExecution::Background {
        policy = policy.with_connect_execution(ConnectExecution::Background);
    }
    if !binding.lifecycle.retry_on_connect_timeout {
        policy = policy.without_connect_timeout_retry();
    }

    policy
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
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&info.id)
            .map_or_else(DeviceLifecyclePolicy::default, |pending| {
                lifecycle_policy_for_transport(pending.descriptor.transport)
            })
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

    #[allow(
        clippy::too_many_lines,
        reason = "USB connect owns discovery handoff, init, diagnostics, and actor startup"
    )]
    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let connected = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(connected) = connected {
            let mut device = connected.device.lock().await;
            device.ensure_actor_ready(*id).await?;
            debug!(device_id = %id, "USB device already connected; skipping duplicate connect");
            return Ok(());
        }

        let pending = {
            let pending_guard = self.pending.read().unwrap_or_else(PoisonError::into_inner);
            let pending_ids = pending_guard
                .keys()
                .take(4)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            pending_guard.get(id).cloned().ok_or_else(|| {
                debug!(
                    device_id = %id,
                    pending_cache_size = pending_guard.len(),
                    sample_ids = %pending_ids,
                    "USB connect refused a device without an adopted descriptor"
                );
                DeviceError::NotAdopted { device_id: *id }
            })?
        };

        let result: Result<()> = async {
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
        let segment_summary = resolved_info
            .segments
            .iter()
            .map(|segment| {
                format!(
                    "{}:{}:{:?}",
                    segment.name, segment.led_count, segment.topology
                )
            })
            .collect::<Vec<_>>();
        debug!(
            device_id = %id,
            descriptor = pending.descriptor.name,
            protocol = protocol.name(),
            transport = transport_name,
            total_leds = resolved_info.total_led_count(),
            segment_count = resolved_info.segments.len(),
            segments = ?segment_summary,
            "USB connect resolved protocol topology"
        );
        let target_fps = fps_from_frame_interval(protocol.frame_interval());
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<UsbFramePayload>>);
        let (display_tx, display_rx) = watch::channel(None::<Arc<UsbDisplayPayload>>);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let active = Arc::new(AtomicBool::new(true));
        let lifecycle_gate = Arc::new(StdMutex::new(()));
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
            Arc::clone(&active),
            Arc::clone(&lifecycle_gate),
            frame_tx.clone(),
            frame_rx,
            display_tx.clone(),
            display_rx,
            command_rx,
            Arc::clone(&last_async_error),
        );

        let device = UsbDevice {
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
            lifecycle_gate,
            last_async_error,
            info_template: pending.info_template,
            frame_diagnostics_emitted: false,
            non_black_frame_diagnostics_emitted: false,
        };
        let frame_sink = device.frame_sink(*id);
        let display_sink = device
            .info_template
            .display_surface()
            .map(|_| device.display_sink(*id));
        let connected = ConnectedUsbDevice {
            info_template: device.info_template.clone(),
            protocol: Arc::clone(&device.protocol),
            target_fps: device.target_fps,
            frame_sink,
            display_sink,
            device: tokio::sync::Mutex::new(device),
        };
        self.connected
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(connected));

            Ok(())
        }
        .await;
        result.map_err(|error| {
            map_hal_transport_error(
                *id,
                USB_OUTPUT_BACKEND_ID,
                DeviceTransportOperation::Connect,
                &error,
            )
        })
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let connected = self
            .connected
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let Some(connected) = connected else {
            self.pending
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            return Ok(());
        };

        let mut device = connected.device.lock().await;
        let disconnect_result = device.shutdown(*id).await;
        self.pending
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        disconnect_result
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.write_colors_shared(id, Arc::new(colors.to_vec()))
            .await
    }

    async fn write_colors_shared(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<(), DeviceError> {
        let connected = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(connected) = connected else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        let mut device = connected.device.lock().await;

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

        device.queue_colors(*id, colors);
        Ok(())
    }

    async fn write_display_payload_owned(
        &self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<(), DeviceError> {
        let connected = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(connected) = connected else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        let mut device = connected.device.lock().await;

        if device.info_template.display_surface().is_none() {
            return Err(DeviceError::Unsupported {
                backend: USB_OUTPUT_BACKEND_ID.to_owned(),
                operation: "device display output",
            });
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

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let connected = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(connected) = connected else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };

        let mut device = connected.device.lock().await;
        device.set_brightness(*id, brightness).await
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        let connected = self
            .connected
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(device) = connected.get(id) else {
            return Ok(None);
        };

        Ok(Some(build_connected_device_info(
            *id,
            &device.info_template,
            device.protocol.as_ref(),
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
            .map(|device| Arc::clone(&device.frame_sink))
    }

    fn display_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceDisplaySink>> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| device.display_sink.as_ref().map(Arc::clone))
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
    info.segments = protocol.zones();
    info.capabilities = protocol.capabilities();
    info.sync_display_capabilities();
    info
}

#[cfg(test)]
mod tests;
