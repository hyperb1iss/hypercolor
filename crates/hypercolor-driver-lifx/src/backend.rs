//! LIFX output backend: device sessions, frame sink, and pacing
//! (Spec 88 §9 and §11.5).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{info, warn};

use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::color::{HsbkEncoder, clamp_kelvin};
use crate::config::LifxConfig;
use crate::encoder::{FrameEncoder, OutputPlan, StreamAddress};
use crate::pacer::{MessagePacer, frame_interval};
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

/// Connected device: immutable facts plus the mutable stream session.
struct SessionHandle {
    profile: LifxDeviceProfile,
    info: DeviceInfo,
    cadence: OutputCadence,
    session: Mutex<LifxSession>,
}

struct LifxSession {
    socket: UdpSocket,
    led_count: usize,
    encoder: FrameEncoder,
    converter: HsbkEncoder,
    address: StreamAddress,
    duration_ms: u32,
    pacer: MessagePacer,
    last_size_mismatch_warn_at: Option<Instant>,
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

    async fn open_session(&self, route: &AdoptedDevice) -> Result<SessionHandle> {
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
        let messages = plan.worst_case_messages();
        let interval = frame_interval(self.config.max_messages_per_second, messages);
        let max_fps = max_fps_for(&profile.topology, self.config.max_messages_per_second);
        let cadence = OutputCadence::from_min_interval(interval, max_fps)
            .with_max_frame_silence(MIN_FRAME_SILENCE.max(interval.saturating_mul(2)));
        let info = profile.device_info(max_fps);
        let converter = HsbkEncoder::new(
            clamp_kelvin(self.config.white_kelvin, profile.kelvin_range()),
            profile.color_capable(),
        );

        let session = LifxSession {
            socket,
            led_count: plan.led_count(),
            encoder: FrameEncoder::new(plan),
            converter,
            address: StreamAddress {
                source,
                target: route.serial,
                sequence,
            },
            duration_ms: u32::from(self.config.transition_ms),
            pacer: MessagePacer::new(self.config.max_messages_per_second),
            last_size_mismatch_warn_at: None,
        };
        Ok(SessionHandle {
            profile,
            info,
            cadence,
            session: Mutex::new(session),
        })
    }

    async fn write_session(
        device_id: &DeviceId,
        handle: &SessionHandle,
        colors: &[[u8; 3]],
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<()> {
        let mut guard = handle.session.lock().await;
        let session = &mut *guard;

        if colors.len() != session.led_count {
            let should_warn = session
                .last_size_mismatch_warn_at
                .is_none_or(|warned_at| warned_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %device_id,
                    expected_led_count = session.led_count,
                    actual_led_count = colors.len(),
                    "LIFX frame size mismatch; padding or truncating to the zone count"
                );
                session.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let messages = session.encoder.encode(
            colors,
            &session.converter,
            session.duration_ms,
            &mut session.address,
        )?;
        let delay = session.pacer.delay_at(Instant::now());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        if let Some((id, observer)) = delivery {
            observer.transport_started(id);
        }
        let sent_at = Instant::now();
        for datagram in session.encoder.datagrams() {
            session
                .socket
                .send(datagram)
                .await
                .context("LIFX datagram send failed")?;
        }
        session
            .pacer
            .record(sent_at, u32::try_from(messages).unwrap_or(u32::MAX));
        Ok(())
    }
}

struct LifxFrameSink {
    device_id: DeviceId,
    handle: Arc<SessionHandle>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for LifxFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        LifxBackend::write_session(&self.device_id, &self.handle, colors.as_slice(), None)
            .await
            .map_err(|error| DeviceError::write(self.device_id, error))
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = LifxBackend::write_session(
            &self.device_id,
            &self.handle,
            colors.as_slice(),
            Some((id, observer.as_ref())),
        )
        .await
        .map_err(|error| DeviceError::write(self.device_id, error))
        .map(|()| DeviceWriteOutcome::Sent);
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
        if self.session(id).is_some() {
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
            .open_session(&route)
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
        Self::write_session(id, &handle, colors, None)
            .await
            .map_err(|error| DeviceError::write(id, error))
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        handle.session.lock().await.converter.brightness = brightness;
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
