//! LIFX output backend: device sessions, frame sink, and the per-device
//! sender (Spec 88 §9 and §11.5).
//!
//! Each connected light gets one sender task with its own latest-frame
//! slot. The host's frame sink and direct writes both hand frames to that
//! slot; the sender always transmits the newest frame, charging every
//! datagram to the message budget as it goes. While a frame's tail is being
//! paced, a newer frame that would finish sooner than the remaining tail
//! replaces it, so stale content never keeps the light busy, and a newer
//! frame that costs as much waits its turn, so an animation of expensive
//! frames still completes every frame it starts.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::color::{HsbkEncoder, clamp_kelvin};
use crate::config::LifxConfig;
use crate::encoder::{FrameEncoder, OutputPlan, StreamAddress};
use crate::pacer::{MessageBudget, frame_interval};
use crate::protocol::{Message, Serial};
use crate::scanner::{max_fps_for, probe_device, profile_metadata, route_from_metadata};
use crate::topology::{LIFX_DRIVER_ID, LifxDeviceProfile};
use crate::transport::{LifxClient, RequestPolicy};

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);
const MIN_FRAME_SILENCE: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const POWER_ON_LEVEL: u16 = u16::MAX;

/// Device a discovery scan handed to this backend.
#[derive(Debug, Clone)]
struct AdoptedDevice {
    addr: SocketAddr,
    serial: Serial,
}

/// One frame waiting in a device's latest-frame slot.
#[derive(Debug)]
struct FrameRequest {
    sequence: u64,
    colors: Arc<Vec<[u8; 3]>>,
}

/// State the sender task and its callers share.
#[derive(Debug, Default)]
struct SenderShared {
    brightness: AtomicU8,
    failure: StdMutex<Option<String>>,
}

/// Connected device: immutable facts plus the handle to its sender task.
struct SessionHandle {
    profile: LifxDeviceProfile,
    info: DeviceInfo,
    cadence: OutputCadence,
    shared: Arc<SenderShared>,
    frames: watch::Sender<Option<Arc<FrameRequest>>>,
    settled: watch::Receiver<u64>,
    next_sequence: AtomicU64,
    task: JoinHandle<()>,
}

impl SessionHandle {
    fn is_closed(&self) -> bool {
        self.task.is_finished()
    }

    /// Hand a frame to the sender, replacing any frame not yet started.
    fn submit(&self, device_id: DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Result<u64, DeviceError> {
        if self.is_closed() {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if let Some(failure) = self
            .shared
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Err(DeviceError::write(device_id, failure));
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.frames
            .send_replace(Some(Arc::new(FrameRequest { sequence, colors })));
        Ok(sequence)
    }

    /// Wait until `sequence`, or a newer frame that replaced it, is on the
    /// wire.
    async fn settle(&self, device_id: DeviceId, sequence: u64) -> Result<(), DeviceError> {
        let mut settled = self.settled.clone();
        settled
            .wait_for(|done| *done >= sequence)
            .await
            .map_err(|_| DeviceError::Disconnected {
                device: device_id.to_string(),
            })?;
        if let Some(failure) = self
            .shared
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Err(DeviceError::write(device_id, failure));
        }
        Ok(())
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The per-device sender task.
struct DeviceSender {
    device_id: DeviceId,
    socket: UdpSocket,
    led_count: usize,
    active: FrameEncoder,
    candidate: FrameEncoder,
    kelvin: u16,
    color_capable: bool,
    address: StreamAddress,
    duration_ms: u32,
    budget: MessageBudget,
    shared: Arc<SenderShared>,
    settled: watch::Sender<u64>,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl DeviceSender {
    async fn run(mut self, mut frames: watch::Receiver<Option<Arc<FrameRequest>>>) {
        let mut last_settled = 0_u64;
        loop {
            let latest = frames.borrow_and_update().clone();
            if let Some(request) = latest
                && request.sequence > last_settled
            {
                match self.transmit(request, &mut frames).await {
                    Some(sequence) => {
                        last_settled = sequence;
                        self.settled.send_replace(sequence);
                    }
                    None => return,
                }
                continue;
            }
            if frames.changed().await.is_err() {
                return;
            }
        }
    }

    fn converter(&self) -> HsbkEncoder {
        HsbkEncoder {
            kelvin: self.kelvin,
            color_capable: self.color_capable,
            brightness: self.shared.brightness.load(Ordering::Relaxed),
        }
    }

    fn warn_on_size_mismatch(&mut self, colors: &[[u8; 3]]) {
        if colors.len() == self.led_count {
            return;
        }
        let should_warn = self
            .last_size_mismatch_warn_at
            .is_none_or(|warned_at| warned_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
        if should_warn {
            warn!(
                device_id = %self.device_id,
                expected_led_count = self.led_count,
                actual_led_count = colors.len(),
                "LIFX frame size mismatch; padding or truncating to the zone count"
            );
            self.last_size_mismatch_warn_at = Some(Instant::now());
        }
    }

    /// Encode `request` into the candidate encoder. Returns its cost.
    fn prepare(&mut self, request: &FrameRequest) -> Option<usize> {
        self.warn_on_size_mismatch(&request.colors);
        let converter = self.converter();
        let mut address = self.address;
        match self
            .candidate
            .encode(&request.colors, &converter, self.duration_ms, &mut address)
        {
            Ok(count) => Some(count),
            Err(error) => {
                self.record_failure(format!("LIFX frame could not be encoded: {error}"));
                None
            }
        }
    }

    /// Make the candidate the active frame, committing its sequence numbers.
    fn activate(&mut self) {
        std::mem::swap(&mut self.active, &mut self.candidate);
        let count = u8::try_from(self.active.datagram_count() % 256).unwrap_or(0);
        self.address.sequence = self.address.sequence.wrapping_add(count);
    }

    fn record_failure(&self, failure: String) {
        warn!(device_id = %self.device_id, error = %failure, "LIFX output failed");
        *self
            .shared
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(failure);
    }

    /// Send the newest frame to completion. Returns the sequence of the
    /// frame that completed, or `None` when the session closed.
    async fn transmit(
        &mut self,
        request: Arc<FrameRequest>,
        frames: &mut watch::Receiver<Option<Arc<FrameRequest>>>,
    ) -> Option<u64> {
        let mut current = request;
        if self.prepare(&current).is_none() {
            return Some(current.sequence);
        }
        self.activate();
        let mut sent = 0_usize;
        let mut considered = current.sequence;

        loop {
            let total = self.active.datagram_count();
            if sent >= total {
                return Some(current.sequence);
            }

            // A newer frame replaces the tail only when it finishes sooner
            // than the tail would: it repaints every zone and carries the
            // final apply, so nothing the tail would have shown is lost.
            let newest = frames.borrow_and_update().clone();
            if let Some(newer) = newest
                && newer.sequence > considered
            {
                considered = newer.sequence;
                if let Some(cost) = self.prepare(&newer)
                    && (sent == 0 || cost < total - sent)
                {
                    self.activate();
                    current = newer;
                    sent = 0;
                    continue;
                }
            }

            let delay = self.budget.delay_at(Instant::now());
            if !delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    changed = frames.changed() => {
                        if changed.is_err() {
                            return None;
                        }
                        continue;
                    }
                }
            }

            let Some(datagram) = self.active.datagram(sent) else {
                return Some(current.sequence);
            };
            if let Err(error) = self.socket.send(datagram).await {
                self.record_failure(format!("LIFX datagram send failed: {error}"));
                return Some(current.sequence);
            }
            self.budget.record(Instant::now());
            sent += 1;
        }
    }
}

/// LIFX backend implementing [`DeviceBackend`].
pub struct LifxBackend {
    config: LifxConfig,
    request_policy: RequestPolicy,
    adopted: StdRwLock<HashMap<DeviceId, AdoptedDevice>>,
    sessions: StdRwLock<HashMap<DeviceId, Arc<SessionHandle>>>,
}

impl LifxBackend {
    #[must_use]
    pub fn new(config: LifxConfig) -> Self {
        Self {
            config,
            request_policy: RequestPolicy::default(),
            adopted: StdRwLock::new(HashMap::new()),
            sessions: StdRwLock::new(HashMap::new()),
        }
    }

    /// Override the per-request policy used while connecting.
    #[must_use]
    pub const fn with_request_policy(mut self, policy: RequestPolicy) -> Self {
        self.request_policy = policy;
        self
    }

    fn session(&self, id: &DeviceId) -> Option<Arc<SessionHandle>> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn connected(&self, id: &DeviceId) -> Result<Arc<SessionHandle>, DeviceError> {
        self.session(id).ok_or_else(|| DeviceError::Disconnected {
            device: id.to_string(),
        })
    }

    async fn open_session(
        &self,
        device_id: DeviceId,
        route: &AdoptedDevice,
    ) -> Result<SessionHandle> {
        let mut client = LifxClient::bind_for(&route.addr)
            .await
            .with_context(|| format!("failed to bind LIFX socket for {}", route.addr))?;
        let profile = probe_device(&mut client, route.addr, route.serial, self.request_policy)
            .await?
            .ok_or_else(|| anyhow!("LIFX device {} is not a light", route.serial))?;
        client
            .request_ack(
                route.addr,
                route.serial,
                &Message::SetLightPower {
                    level: POWER_ON_LEVEL,
                    duration_ms: 0,
                },
                self.request_policy,
            )
            .await
            .with_context(|| {
                format!("LIFX device {} did not acknowledge power on", route.serial)
            })?;

        let (socket, source, sequence) = client.into_parts();
        socket
            .connect(route.addr)
            .await
            .with_context(|| format!("failed to connect LIFX socket to {}", route.addr))?;

        let plan = OutputPlan::from_topology(&profile.topology);
        // The host spaces offers by the cheapest frame's cost; the sender
        // paces dearer frames message by message.
        let interval = frame_interval(
            self.config.max_messages_per_second,
            plan.best_case_messages(),
        );
        let max_fps = max_fps_for(&profile.topology, self.config.max_messages_per_second);
        let cadence = OutputCadence::from_min_interval(interval, max_fps)
            .with_max_frame_silence(MIN_FRAME_SILENCE.max(interval.saturating_mul(2)));
        let info = profile.device_info(max_fps);

        let shared = Arc::new(SenderShared {
            brightness: AtomicU8::new(u8::MAX),
            failure: StdMutex::new(None),
        });
        let (frames, frames_rx) = watch::channel(None);
        let (settled_tx, settled) = watch::channel(0);
        let sender = DeviceSender {
            device_id,
            socket,
            led_count: plan.led_count(),
            active: FrameEncoder::new(plan.clone()),
            candidate: FrameEncoder::new(plan),
            kelvin: clamp_kelvin(self.config.white_kelvin, profile.kelvin_range()),
            color_capable: profile.color_capable(),
            address: StreamAddress {
                source,
                target: route.serial,
                sequence,
            },
            duration_ms: u32::from(self.config.transition_ms),
            budget: MessageBudget::new(self.config.max_messages_per_second),
            shared: Arc::clone(&shared),
            settled: settled_tx,
            last_size_mismatch_warn_at: None,
        };
        let task = tokio::spawn(sender.run(frames_rx));

        Ok(SessionHandle {
            profile,
            info,
            cadence,
            shared,
            frames,
            settled,
            next_sequence: AtomicU64::new(0),
            task,
        })
    }
}

struct LifxFrameSink {
    device_id: DeviceId,
    handle: Arc<SessionHandle>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for LifxFrameSink {
    fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        self.handle.submit(self.device_id, colors).map(|_| ())
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = self.handle.submit(self.device_id, colors).map(|_| {
            // The frame is handed to the device's sender, which owns the
            // transport from here (DeviceWriteOutcome::Sent).
            observer.transport_started(id);
            DeviceWriteOutcome::Sent
        });
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for LifxBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: LIFX_DRIVER_ID.to_owned(),
            name: "LIFX".to_owned(),
            description: "LIFX lights over the LAN protocol".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let (addr, serial) =
            route_from_metadata(&discovered.metadata).map_err(|_| DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, AdoptedDevice { addr, serial });
        Ok(())
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        Ok(self.session(id).map(|handle| handle.info.clone()))
    }

    async fn connected_device_metadata(
        &self,
        id: &DeviceId,
    ) -> Result<Option<HashMap<String, String>>, DeviceError> {
        Ok(self
            .session(id)
            .map(|handle| profile_metadata(&handle.profile)))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    fn lifecycle_policy(&self, _info: &DeviceInfo) -> DeviceLifecyclePolicy {
        DeviceLifecyclePolicy::default().with_connect_timeout(CONNECT_TIMEOUT)
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if self.session(id).is_some_and(|handle| !handle.is_closed()) {
            return Ok(());
        }
        let route = self
            .adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;

        let handle = self
            .open_session(*id, &route)
            .await
            .map_err(|error| DeviceError::connection(id, error))?;
        info!(
            device_id = %id,
            serial = %route.serial,
            addr = %route.addr,
            class = handle.profile.topology.class_name(),
            leds = handle.info.total_led_count(),
            max_fps = handle.info.capabilities.max_fps,
            "Connected to LIFX device"
        );
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(handle));
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let removed = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        match removed {
            Some(handle) => {
                // Sinks the host still holds see a closed lane from here on.
                handle.task.abort();
                info!(device_id = %id, serial = %handle.profile.serial, "Disconnected from LIFX device");
                Ok(())
            }
            None => Err(DeviceError::Disconnected {
                device: id.to_string(),
            }),
        }
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        let sequence = handle.submit(*id, Arc::new(colors.to_vec()))?;
        handle.settle(*id, sequence).await
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        handle
            .shared
            .brightness
            .store(brightness, Ordering::Relaxed);
        Ok(())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.session(id).map(|handle| handle.cadence.target_fps())
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.session(id).map(|handle| handle.cadence)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.session(id).map(|handle| {
            Arc::new(LifxFrameSink {
                device_id: *id,
                handle,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}
