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
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{info, warn};

use hypercolor_driver_api::{
    Attempt, BackendInfo, DeviceBackend, DeviceFrameSink, DeviceLifecyclePolicy, DiscoveredDevice,
    Frame, MailboxFrame, OutputPolicy, SkipReason, Submission, Transmission,
};
use hypercolor_driver_support::{DeviceSession, Session, SessionTable};
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
struct FrameRequest {
    sequence: u64,
    frame: MailboxFrame,
}

/// State the sender task and its callers share.
#[derive(Debug, Default)]
struct SenderShared {
    brightness: AtomicU8,
}

/// What a connect learned, plus the state shared with the sender.
struct LifxFacts {
    profile: LifxDeviceProfile,
    info: DeviceInfo,
    policy: OutputPolicy,
    shared: Arc<SenderShared>,
}

/// Mailbox to one light's sender task, which owns the socket.
struct LifxSession {
    device_id: DeviceId,
    frames: watch::Sender<Option<Arc<FrameRequest>>>,
}

impl DeviceSession for LifxSession {
    type Facts = LifxFacts;

    /// Hand a frame to the sender, replacing any frame not yet started.
    ///
    /// The attempt goes with it: the sender owns the transport, so it settles
    /// the attempt when the frame finishes or a newer frame replaces it, and
    /// the host stays free to offer that newer frame meanwhile. The sequence
    /// is derived from the slot inside its write lock, so it only grows.
    fn submit(&mut self, frame: Frame) -> impl Future<Output = Submission> + Send {
        if self.frames.is_closed() {
            let (_, attempt) = frame.into_parts();
            let error = DeviceError::Disconnected {
                device: self.device_id.to_string(),
            };
            return std::future::ready(attempt.reject(error).into());
        }
        let frame = MailboxFrame::new(frame);
        let mut replaced = None;
        self.frames.send_modify(|slot| {
            let sequence = slot.as_ref().map_or(0, |frame| frame.sequence) + 1;
            replaced = slot.replace(Arc::new(FrameRequest { sequence, frame }));
        });
        if let Some(replaced) = replaced {
            replaced.frame.supersede();
        }
        std::future::ready(Submission::Pending)
    }
}

/// Remaining messages until the next frame applies, which gates when a
/// newer frame may replace the current one (Spec 88 §9.4).
///
/// Invariant: the bound never grows while a commitment exists. It starts
/// when a frame sends its first datagram, shrinks as that frame's tail goes
/// out and whenever a cheaper frame replaces it, and clears only when a
/// frame finishes. A replacement must cost less than the bound, so the work
/// left before the next apply never increases and frames always finish.
/// Before anything has been sent there is no commitment, and the newest
/// frame simply replaces an unstarted one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameCommitment {
    remaining: Option<usize>,
}

impl FrameCommitment {
    /// Whether a newer frame costing `cost` messages may take over.
    #[must_use]
    pub fn admits(&self, cost: usize) -> bool {
        self.remaining.is_none_or(|remaining| cost < remaining)
    }

    /// A newer frame costing `cost` messages took over.
    pub fn replaced(&mut self, cost: usize) {
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining = (*remaining).min(cost);
        }
    }

    /// A datagram went out and the current frame has `remaining` left.
    pub fn sent(&mut self, remaining: usize) {
        self.remaining = Some(
            self.remaining
                .map_or(remaining, |bound| bound.min(remaining)),
        );
    }

    /// The current frame finished; the next frame starts unconstrained.
    pub fn finished(&mut self) {
        self.remaining = None;
    }

    /// Messages left before the next apply, if a frame has started.
    #[must_use]
    pub const fn remaining(&self) -> Option<usize> {
        self.remaining
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
    /// Whether the last send failed, so a failure and its recovery log once.
    failing: bool,
    last_size_mismatch_warn_at: Option<Instant>,
}

/// How far the frame being sent has got.
enum Progress {
    Waiting(Attempt),
    Sending(Transmission),
}

impl Progress {
    fn started(self) -> Transmission {
        match self {
            Self::Waiting(attempt) => attempt.start(),
            Self::Sending(transmission) => transmission,
        }
    }

    fn superseded(self) {
        match self {
            Self::Waiting(attempt) => {
                attempt.skip(SkipReason::Superseded);
            }
            Self::Sending(transmission) => {
                transmission.superseded();
            }
        }
    }
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
                    Some(sequence) => last_settled = sequence,
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
    fn prepare(&mut self, request: &FrameRequest) -> Result<usize, DeviceError> {
        self.warn_on_size_mismatch(request.frame.colors());
        let converter = self.converter();
        let mut address = self.address;
        self.candidate
            .encode(
                request.frame.colors(),
                &converter,
                self.duration_ms,
                &mut address,
            )
            .map_err(|error| {
                let failure = format!("LIFX frame could not be encoded: {error}");
                self.record_failure(&failure);
                DeviceError::write(self.device_id, failure)
            })
    }

    /// Make the candidate the active frame, committing its sequence numbers.
    fn activate(&mut self) {
        std::mem::swap(&mut self.active, &mut self.candidate);
        let count = u8::try_from(self.active.datagram_count() % 256).unwrap_or(0);
        self.address.sequence = self.address.sequence.wrapping_add(count);
    }

    fn record_failure(&mut self, failure: &str) {
        if !self.failing {
            warn!(device_id = %self.device_id, error = %failure, "LIFX output failed");
        }
        self.failing = true;
    }

    fn record_success(&mut self) {
        if self.failing {
            info!(device_id = %self.device_id, "LIFX output recovered");
        }
        self.failing = false;
    }

    /// Send the newest frame to completion. Returns the sequence of the
    /// frame that completed, or `None` when the session is gone.
    async fn transmit(
        &mut self,
        request: Arc<FrameRequest>,
        frames: &mut watch::Receiver<Option<Arc<FrameRequest>>>,
    ) -> Option<u64> {
        let mut current = request;
        // Nothing to claim means a newer frame already replaced this one.
        let Some(attempt) = current.frame.claim() else {
            return Some(current.sequence);
        };
        if let Err(error) = self.prepare(&current) {
            attempt.reject_transient(error);
            return Some(current.sequence);
        }
        let mut progress = Progress::Waiting(attempt);
        self.activate();
        let mut sent = 0_usize;
        let mut considered = current.sequence;
        let mut commitment = FrameCommitment::default();

        loop {
            let total = self.active.datagram_count();
            if sent >= total {
                progress.started().written();
                return Some(current.sequence);
            }

            // A newer frame takes over only within the commitment bound: it
            // repaints every zone and carries its own apply, so nothing the
            // replaced frame would have shown is lost, and the work left
            // before the next apply never grows.
            let newest = frames.borrow_and_update().clone();
            if let Some(newer) = newest
                && newer.sequence > considered
            {
                considered = newer.sequence;
                match self.prepare(&newer) {
                    Ok(cost) if commitment.admits(cost) => {
                        if let Some(attempt) = newer.frame.claim() {
                            commitment.replaced(cost);
                            self.activate();
                            progress.superseded();
                            progress = Progress::Waiting(attempt);
                            current = newer;
                            sent = 0;
                            continue;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(attempt) = newer.frame.claim() {
                            attempt.reject_transient(error);
                        }
                    }
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
                progress.started().written();
                return Some(current.sequence);
            };
            let transmission = progress.started();
            if let Err(error) = self.socket.send(datagram).await {
                let failure = format!("LIFX datagram send failed: {error}");
                self.record_failure(&failure);
                transmission.failed_transient(DeviceError::write(self.device_id, failure));
                return Some(current.sequence);
            }
            progress = Progress::Sending(transmission);
            self.record_success();
            self.budget.record(Instant::now());
            sent += 1;
            commitment.sent(total - sent);
        }
    }
}

/// LIFX backend implementing [`DeviceBackend`].
pub struct LifxBackend {
    config: LifxConfig,
    request_policy: RequestPolicy,
    adopted: StdRwLock<HashMap<DeviceId, AdoptedDevice>>,
    sessions: SessionTable<LifxSession>,
}

impl LifxBackend {
    #[must_use]
    pub fn new(config: LifxConfig) -> Self {
        Self {
            config,
            request_policy: RequestPolicy::default(),
            adopted: StdRwLock::new(HashMap::new()),
            sessions: SessionTable::new(),
        }
    }

    /// Override the per-request policy used while connecting.
    #[must_use]
    pub const fn with_request_policy(mut self, policy: RequestPolicy) -> Self {
        self.request_policy = policy;
        self
    }

    fn opener(&self) -> LifxOpener {
        LifxOpener {
            config: self.config.clone(),
            request_policy: self.request_policy,
        }
    }
}

/// What an open takes from the backend, since it runs on the session table's
/// task and outlives the connect that started it.
struct LifxOpener {
    config: LifxConfig,
    request_policy: RequestPolicy,
}

impl LifxOpener {
    async fn open_session(
        &self,
        device_id: DeviceId,
        route: &AdoptedDevice,
    ) -> Result<Session<LifxSession>> {
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
        let policy = OutputPolicy::from_min_interval(interval)
            .with_replay_after(MIN_FRAME_SILENCE.max(interval.saturating_mul(2)));
        let info = profile.device_info(max_fps);

        let shared = Arc::new(SenderShared {
            brightness: AtomicU8::new(u8::MAX),
        });
        let (frames, frames_rx) = watch::channel(None);
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
            failing: false,
            last_size_mismatch_warn_at: None,
        };
        let mut session = Session::new(
            device_id,
            LifxFacts {
                profile,
                info,
                policy,
                shared,
            },
            LifxSession { device_id, frames },
        );
        session.spawn(sender.run(frames_rx));
        Ok(session)
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
        Ok(self.sessions.facts(id).map(|facts| facts.info.clone()))
    }

    async fn connected_device_metadata(
        &self,
        id: &DeviceId,
    ) -> Result<Option<HashMap<String, String>>, DeviceError> {
        Ok(self
            .sessions
            .facts(id)
            .map(|facts| profile_metadata(&facts.profile)))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    fn lifecycle_policy(&self, _info: &DeviceInfo) -> DeviceLifecyclePolicy {
        DeviceLifecyclePolicy::default().with_connect_timeout(CONNECT_TIMEOUT)
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let id = *id;
        self.sessions
            .open(&id, || {
                let route = self
                    .adopted
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&id)
                    .cloned();
                let opener = self.opener();
                async move {
                    let route = route.ok_or(DeviceError::NotAdopted { device_id: id })?;
                    let session = opener
                        .open_session(id, &route)
                        .await
                        .map_err(|error| DeviceError::connection(id, error))?;
                    let facts = session.facts();
                    info!(
                        device_id = %id,
                        serial = %route.serial,
                        addr = %route.addr,
                        class = facts.profile.topology.class_name(),
                        leds = facts.info.total_led_count(),
                        max_fps = facts.info.capabilities.max_fps,
                        "Connected to LIFX device"
                    );
                    Ok(session)
                }
            })
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from LIFX device");
        Ok(())
    }

    async fn retire(&self) -> Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let facts = self
            .sessions
            .facts(id)
            .ok_or_else(|| DeviceError::Disconnected {
                device: id.to_string(),
            })?;
        facts.shared.brightness.store(brightness, Ordering::Relaxed);
        Ok(())
    }

    fn output_policy(&self, id: &DeviceId) -> Option<OutputPolicy> {
        self.sessions.facts(id).map(|facts| facts.policy)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}
