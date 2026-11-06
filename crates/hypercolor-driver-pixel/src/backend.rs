//! Pixel controller backend: implements [`DeviceBackend`] over UDP.
//!
//! Each connected device owns a session holding its resolved plan, a
//! reusable channel stream, and per-universe packets whose fixed headers
//! are built once. Every frame encodes colors into the stream, copies
//! stream ranges into universe buffers or DDP spans, and unicasts the
//! packets to the configured receiver.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_pixel_protocols::artnet::{
    ARTNET_PORT, ArtDmxPacket, ArtDmxSequenceTracker, PortAddress,
};
use hypercolor_pixel_protocols::ddp::{DDP_ID_DEFAULT, DdpSequence, DdpSpan, build_ddp_spans};
use hypercolor_pixel_protocols::e131::{E131Packet, E131SequenceTracker, E131SyncPacket};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, info, warn};

use crate::color::encode_pixels;
use crate::config::{PixelDeviceConfig, PixelProtocol, is_unicast_target};
use crate::plan::{DmxPlan, OutputPlan, PixelDevicePlan, resolve_device};

/// Discovery metadata key carrying the device's effective config as JSON.
pub const METADATA_DEVICE_CONFIG: &str = "pixel_device";

/// Longest silence before the last frame is resent.
///
/// E1.31 section 6.6.2 asks for a keep-alive every 800-1000 ms once output
/// is suppressed, and receivers declare data loss after 2.5 s. Art-Net 4
/// recommends the same 800-1000 ms retransmit window. One interval serves
/// all three protocols.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(800);

/// E1.31 section 6.2.6: three packets with `Stream_Terminated` set.
pub const E131_TERMINATION_PACKETS: usize = 3;

/// E1.31 section 6.6.2: three packets of unchanged data before a source
/// suppresses transmission.
pub const E131_IDENTICAL_SENDS: u8 = 3;

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Backend for manually configured sACN, Art-Net, and DDP receivers.
#[derive(Default)]
pub struct PixelBackend {
    /// Plans for adopted devices, keyed by device id.
    adopted: StdRwLock<HashMap<DeviceId, Arc<PixelDevicePlan>>>,
    /// Connected sessions.
    sessions: StdRwLock<HashMap<DeviceId, Arc<SessionSlot>>>,
    /// Shared IPv4 socket on an ephemeral port (E1.31 and DDP).
    socket_v4: OnceCell<Arc<UdpSocket>>,
    /// Shared IPv6 socket on an ephemeral port (E1.31 and DDP).
    socket_v6: OnceCell<Arc<UdpSocket>>,
    /// Art-Net socket bound to UDP 6454. Sessions own it; the backend only
    /// keeps a weak handle, so the port is released when the last Art-Net
    /// session ends and another application can take it.
    artnet_socket: StdMutex<Weak<UdpSocket>>,
    /// Whether the ephemeral-port fallback for Art-Net was already logged.
    artnet_fallback_warned: AtomicBool,
}

/// A connected session plus its closed flag.
///
/// The flag lives outside the mutex so frame sinks can answer `is_closed`
/// without blocking, and disconnect sets it while holding the session lock
/// so no write can slip in after the shutdown frames.
struct SessionSlot {
    closed: AtomicBool,
    session: Mutex<PixelSession>,
}

impl SessionSlot {
    fn open(session: PixelSession) -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            session: Mutex::new(session),
        })
    }

    /// Close the session under its lock and release its transport.
    ///
    /// When `shutdown` approves the session, the black frame and E1.31
    /// termination go out first. Frame sinks may keep this slot alive, so
    /// the socket is taken out of the session here rather than when the last
    /// handle drops; that frees UDP 6454 for other applications as soon as no
    /// open session uses it.
    async fn close(&self, device_id: DeviceId, shutdown: impl FnOnce(&PixelSession) -> bool) {
        let mut session = self.session.lock().await;
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if shutdown(&session)
            && let Err(error) = session.finish().await
        {
            debug!(
                device_id = %device_id,
                target = %session.plan.target,
                error = %error,
                "best-effort pixel controller shutdown frames failed"
            );
        }
        session.socket = None;
    }

    /// Retire a session replaced by a reconnect to `successor_target`.
    ///
    /// The successor carries the stream on, so shutdown frames only go out
    /// when the device moved to a different receiver.
    async fn retire(&self, device_id: DeviceId, successor_target: SocketAddr) {
        self.close(device_id, |session| session.plan.target != successor_target)
            .await;
    }

    /// Lock the session and write a frame unless it was disconnected.
    async fn write(
        &self,
        device_id: DeviceId,
        colors: &[[u8; 3]],
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        let mut session = self.session.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(disconnected(device_id));
        }
        session
            .write_colors(colors)
            .await
            .map_err(|error| DeviceError::write(device_id, error))
    }
}

fn disconnected(device_id: DeviceId) -> DeviceError {
    DeviceError::Disconnected {
        device: device_id.to_string(),
    }
}

impl PixelBackend {
    /// Create an empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Local address of the socket a connected device sends from.
    #[must_use]
    pub fn connected_socket_local_addr(&self, id: &DeviceId) -> Option<SocketAddr> {
        self.session(id)
            .and_then(|slot| slot.session.try_lock().ok().and_then(|s| s.socket.clone()))
            .and_then(|socket| socket.local_addr().ok())
    }

    /// Resolved plan for an adopted device.
    #[must_use]
    pub fn adopted_plan(&self, id: &DeviceId) -> Option<Arc<PixelDevicePlan>> {
        self.adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn session(&self, id: &DeviceId) -> Option<Arc<SessionSlot>> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    async fn socket_for(&self, plan: &PixelDevicePlan) -> Result<Arc<UdpSocket>> {
        if plan.protocol == PixelProtocol::ArtNet {
            match self.artnet_source_socket() {
                Ok(socket) => return Ok(socket),
                Err(error) => {
                    if !self.artnet_fallback_warned.swap(true, Ordering::Relaxed) {
                        let reason = if error.kind() == std::io::ErrorKind::AddrInUse {
                            "another Art-Net application owns UDP 6454"
                        } else {
                            "UDP 6454 could not be bound"
                        };
                        warn!(
                            error = %error,
                            port = ARTNET_PORT,
                            reason,
                            "sending ArtDmx from an ephemeral source port instead of 6454; \
                             nodes that only accept Art-Net from 6454 will ignore it"
                        );
                    }
                }
            }
        }
        self.ephemeral_socket(plan.target).await
    }

    /// The shared Art-Net source socket, binding UDP 6454 if no session
    /// holds it.
    ///
    /// The bind sets no reuse flags. With `SO_REUSEPORT` the kernel would
    /// load-balance another local Art-Net application's incoming unicast
    /// onto this socket, which never reads, so that application would lose
    /// packets. Without the flags the bind simply fails while another
    /// application owns the port, and the caller falls back.
    fn artnet_source_socket(&self) -> std::io::Result<Arc<UdpSocket>> {
        let mut held = self
            .artnet_socket
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(socket) = held.upgrade() {
            return Ok(socket);
        }
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, ARTNET_PORT))?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        *held = Arc::downgrade(&socket);
        Ok(socket)
    }

    async fn ephemeral_socket(&self, target: SocketAddr) -> Result<Arc<UdpSocket>> {
        let (cell, bind) = if target.is_ipv4() {
            (&self.socket_v4, "0.0.0.0:0")
        } else {
            (&self.socket_v6, "[::]:0")
        };
        cell.get_or_try_init(|| async {
            UdpSocket::bind(bind)
                .await
                .map(Arc::new)
                .with_context(|| format!("failed to bind pixel output socket on {bind}"))
        })
        .await
        .map(Arc::clone)
    }

    fn require_session(&self, id: &DeviceId) -> Result<Arc<SessionSlot>, DeviceError> {
        self.session(id).ok_or_else(|| disconnected(*id))
    }
}

#[async_trait::async_trait]
impl DeviceBackend for PixelBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: crate::DESCRIPTOR.id.to_owned(),
            name: "Pixel Controllers (sACN / Art-Net / DDP)".to_owned(),
            description: "Manually configured E1.31, Art-Net, and DDP pixel receivers over UDP"
                .to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let device_id = discovered.info.id;
        let Some(raw) = discovered.metadata.get(METADATA_DEVICE_CONFIG) else {
            return Err(DeviceError::NotAdopted { device_id });
        };
        let config: PixelDeviceConfig = serde_json::from_str(raw).map_err(|error| {
            DeviceError::protocol(device_id, format!("invalid pixel device metadata: {error}"))
        })?;
        let plan = resolve_device(&config)
            .map_err(|error| DeviceError::protocol(device_id, error.to_string()))?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device_id, Arc::new(plan));
        Ok(())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let plan = self
            .adopted_plan(id)
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;
        if !is_unicast_target(plan.target.ip()) {
            return Err(DeviceError::connection(
                id,
                format!("{} is not a unicast address", plan.target.ip()),
            ));
        }
        let socket = self
            .socket_for(&plan)
            .await
            .map_err(|error| DeviceError::connection(id, error))?;

        info!(
            device_id = %id,
            pixel_device = %plan.id,
            target = %plan.target,
            protocol = plan.protocol.as_str(),
            pixels = plan.total_pixels(),
            universes = plan.universe_summary().unwrap_or_default(),
            "Connected pixel controller"
        );
        let target = plan.target;
        let session = PixelSession::new(*id, plan, socket)
            .map_err(|error| DeviceError::connection(id, error))?;
        let previous = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, SessionSlot::open(session));
        // A repeated connect replaces the session. Retire the old one so a
        // frame sink still holding it cannot transmit, and so it releases its
        // socket. The map insert is atomic, so concurrent connects for one id
        // each retire whichever slot they displaced and one open slot remains.
        if let Some(previous) = previous {
            previous.retire(*id, target).await;
        }
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let slot = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .ok_or_else(|| disconnected(*id))?;
        // Close under the session lock: frame sinks still hold this slot,
        // and any write that waits on the lock must see the flag.
        slot.close(*id, |_| true).await;
        info!(device_id = %id, "Disconnected pixel controller");
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.require_session(id)?
            .write(*id, colors)
            .await
            .map(|_| ())
    }

    async fn write_colors_shared(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<(), DeviceError> {
        self.write_colors(id, colors.as_slice()).await
    }

    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.require_session(id)?
            .write(*id, colors.as_slice())
            .await
    }

    async fn deliver_colors_shared_observed(
        &self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        match self.require_session(device_id) {
            Ok(session) => {
                deliver_observed(
                    *device_id,
                    &session,
                    delivery_id,
                    &colors,
                    observer.as_ref(),
                )
                .await
            }
            Err(error) => DeviceDeliveryAck::rejected(delivery_id, error),
        }
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.adopted_plan(id)
            .map(|plan| plan.max_fps)
            .filter(|fps| *fps > 0)
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.adopted_plan(id).map(|plan| {
            let identical_sends = if plan.protocol == PixelProtocol::E131 {
                E131_IDENTICAL_SENDS
            } else {
                1
            };
            OutputCadence::from_fps(plan.max_fps)
                .with_max_frame_silence(KEEPALIVE_INTERVAL)
                .with_min_identical_sends(identical_sends)
        })
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.session(id).map(|slot| {
            Arc::new(PixelFrameSink {
                device_id: *id,
                slot,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}

async fn deliver_observed(
    device_id: DeviceId,
    slot: &SessionSlot,
    delivery_id: DeviceDeliveryId,
    colors: &[[u8; 3]],
    observer: &dyn DeviceDeliveryObserver,
) -> DeviceDeliveryAck {
    let payload_bytes = colors.len().saturating_mul(3);
    let started_at = Instant::now();
    let mut session = slot.session.lock().await;
    if slot.closed.load(Ordering::Acquire) {
        return DeviceDeliveryAck::rejected(delivery_id, disconnected(device_id));
    }
    observer.transport_started(delivery_id);
    let result = session
        .write_colors(colors)
        .await
        .map_err(|error| DeviceError::write(device_id, error));
    DeviceDeliveryAck::from_write_result(delivery_id, payload_bytes, started_at.elapsed(), result)
}

struct PixelFrameSink {
    device_id: DeviceId,
    slot: Arc<SessionSlot>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for PixelFrameSink {
    fn is_closed(&self) -> bool {
        self.slot.closed.load(Ordering::Acquire)
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        self.write_colors_shared_outcome(colors).await.map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.slot.write(self.device_id, colors.as_slice()).await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        deliver_observed(self.device_id, &self.slot, id, &colors, observer.as_ref()).await
    }
}

// ── Session ─────────────────────────────────────────────────────────────

/// One E1.31 universe: its packet (fixed header built once) and slot data.
struct E131Universe {
    universe: u16,
    packet: E131Packet,
    data: Vec<u8>,
}

/// One Art-Net universe: its packet and slot data.
struct ArtNetUniverse {
    port_address: PortAddress,
    packet: ArtDmxPacket,
    data: Vec<u8>,
}

/// E1.31 synchronization state: its own sequence space (section 6.3.2).
struct E131Sync {
    universe: u16,
    cid: uuid::Uuid,
    sequences: E131SequenceTracker,
}

enum WireState {
    E131 {
        universes: Vec<E131Universe>,
        sequences: E131SequenceTracker,
        sync: Option<E131Sync>,
    },
    ArtNet {
        universes: Vec<ArtNetUniverse>,
        sequences: ArtDmxSequenceTracker,
    },
    Ddp {
        sequence: DdpSequence,
    },
}

struct PixelSession {
    device_id: DeviceId,
    plan: Arc<PixelDevicePlan>,
    /// Output socket; `None` once the session is closed.
    socket: Option<Arc<UdpSocket>>,
    /// Encoded channel stream, reused every frame.
    stream: Vec<u8>,
    wire: WireState,
    /// Whether any frame has gone out, so shutdown frames are worth sending.
    has_sent: bool,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl PixelSession {
    fn new(
        device_id: DeviceId,
        plan: Arc<PixelDevicePlan>,
        socket: Arc<UdpSocket>,
    ) -> Result<Self> {
        let wire = match &plan.output {
            OutputPlan::E131 { dmx, source } => WireState::E131 {
                universes: dmx
                    .universes
                    .iter()
                    .map(|universe| {
                        let mut packet = E131Packet::new(
                            &source.source_name,
                            source.cid,
                            universe.universe,
                            source.priority,
                        );
                        if let Some(sync) = source.sync_universe {
                            packet.set_sync_address(sync);
                        }
                        E131Universe {
                            universe: universe.universe,
                            packet,
                            data: vec![0; universe.channel_count],
                        }
                    })
                    .collect(),
                sequences: E131SequenceTracker::default(),
                sync: source.sync_universe.map(|universe| E131Sync {
                    universe,
                    cid: source.cid,
                    sequences: E131SequenceTracker::default(),
                }),
            },
            OutputPlan::ArtNet { dmx } => WireState::ArtNet {
                universes: dmx
                    .universes
                    .iter()
                    .map(|universe| {
                        let port_address =
                            PortAddress::new(universe.universe).with_context(|| {
                                format!(
                                    "Art-Net Port-Address {} is out of range",
                                    universe.universe
                                )
                            })?;
                        Ok(ArtNetUniverse {
                            port_address,
                            packet: ArtDmxPacket::new(port_address, 0),
                            data: vec![0; universe.channel_count],
                        })
                    })
                    .collect::<Result<_>>()?,
                sequences: ArtDmxSequenceTracker::default(),
            },
            OutputPlan::Ddp(_) => WireState::Ddp {
                sequence: DdpSequence::default(),
            },
        };
        Ok(Self {
            device_id,
            stream: Vec::with_capacity(plan.total_channels()),
            plan,
            socket: Some(socket),
            wire,
            has_sent: false,
            last_size_mismatch_warn_at: None,
        })
    }

    async fn write_colors(&mut self, colors: &[[u8; 3]]) -> Result<DeviceWriteOutcome> {
        let pixel_count = self.plan.total_pixels() as usize;
        if colors.len() != pixel_count {
            let should_warn = self
                .last_size_mismatch_warn_at
                .is_none_or(|at| at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %self.device_id,
                    expected_led_count = pixel_count,
                    actual_led_count = colors.len(),
                    "pixel controller frame size mismatch; truncating or padding"
                );
                self.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }
        encode_pixels(
            colors,
            self.plan.color_order,
            self.plan.channels_per_pixel,
            pixel_count,
            &mut self.stream,
        );
        self.send_stream(false).await?;
        self.has_sent = true;
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Send a black frame and, for E1.31, terminate every universe stream.
    async fn finish(&mut self) -> Result<()> {
        if !self.has_sent {
            return Ok(());
        }
        self.stream.fill(0);
        self.send_stream(false).await?;
        if matches!(self.wire, WireState::E131 { .. }) {
            for _ in 0..E131_TERMINATION_PACKETS {
                self.send_stream(true).await?;
            }
        }
        Ok(())
    }

    async fn send_stream(&mut self, terminated: bool) -> Result<()> {
        let target = self.plan.target;
        let Some(socket) = self.socket.clone() else {
            bail!("pixel controller session transport was released");
        };
        match (&mut self.wire, &self.plan.output) {
            (
                WireState::E131 {
                    universes,
                    sequences,
                    sync,
                },
                OutputPlan::E131 { dmx, .. },
            ) => {
                fill_universes(dmx, &self.stream, universes, |universe| &mut universe.data);
                for universe in universes.iter_mut() {
                    let sequence = sequences.advance(universe.universe);
                    universe.packet.set_stream_terminated(terminated);
                    universe.packet.set_channels(&universe.data, sequence);
                    send(&socket, universe.packet.as_bytes(), target, "E1.31").await?;
                }
                if let Some(sync) = sync.as_mut().filter(|_| !terminated) {
                    let sequence = sync.sequences.advance(sync.universe);
                    let packet = E131SyncPacket::new(sync.cid, sync.universe, sequence);
                    send(&socket, packet.as_bytes(), target, "E1.31 sync").await?;
                }
            }
            (
                WireState::ArtNet {
                    universes,
                    sequences,
                },
                OutputPlan::ArtNet { dmx },
            ) => {
                fill_universes(dmx, &self.stream, universes, |universe| &mut universe.data);
                for universe in universes.iter_mut() {
                    let sequence = sequences.advance(universe.port_address);
                    universe.packet.set_data(&universe.data, sequence);
                    send(&socket, universe.packet.as_bytes(), target, "Art-Net").await?;
                }
            }
            (WireState::Ddp { sequence }, OutputPlan::Ddp(ddp)) => {
                let spans: Vec<DdpSpan<'_>> = ddp
                    .spans
                    .iter()
                    .map(|span| DdpSpan {
                        offset: span.offset,
                        data: self.stream.get(span.source.clone()).unwrap_or_default(),
                    })
                    .collect();
                let packets = build_ddp_spans(
                    &spans,
                    ddp.data_type,
                    DDP_ID_DEFAULT,
                    ddp.chunk_len,
                    sequence,
                );
                for packet in &packets {
                    send(&socket, packet.as_bytes(), target, "DDP").await?;
                }
            }
            _ => bail!("pixel controller session state does not match its plan"),
        }
        Ok(())
    }
}

/// Copy channel stream ranges into their universe slot buffers.
///
/// Slots no run writes keep the zeros they were allocated with.
fn fill_universes<U>(
    dmx: &DmxPlan,
    stream: &[u8],
    universes: &mut [U],
    data: impl Fn(&mut U) -> &mut Vec<u8>,
) {
    for run in &dmx.runs {
        let (Some(universe), Some(source)) = (
            universes.get_mut(run.universe_index),
            stream.get(run.source.clone()),
        ) else {
            continue;
        };
        if let Some(slots) = data(universe).get_mut(run.first_slot..run.first_slot + source.len()) {
            slots.copy_from_slice(source);
        }
    }
}

async fn send(socket: &UdpSocket, bytes: &[u8], target: SocketAddr, label: &str) -> Result<()> {
    socket
        .send_to(bytes, target)
        .await
        .with_context(|| format!("{label} send to {target} failed"))?;
    Ok(())
}
