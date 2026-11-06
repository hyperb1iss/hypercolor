//! WiZ output backend: sessions, the frame sink, and reply watching
//! (Spec 93 §8 and §9.5).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::WIZ_DRIVER_ID;
use crate::config::WizConfig;
use crate::protocol::{Pilot, SigningKey, decode_reply, encode_set_pilot, error_code};
use crate::scanner::{WizDeviceProfile, addr_from_metadata, probe_device, profile_metadata};
use crate::transport::RequestPolicy;

/// Re-send the last state after this much silence, so one lost datagram
/// does not leave a static scene wrong.
pub const STATE_REFRESH: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const REJECTION_WARN_INTERVAL: Duration = Duration::from_mins(1);
const RECEIVE_BUFFER_LEN: usize = 2048;

/// Connected device: immutable facts plus the shared stream state.
struct SessionHandle {
    profile: WizDeviceProfile,
    info: DeviceInfo,
    cadence: OutputCadence,
    stream: Mutex<StreamState>,
    socket: Arc<UdpSocket>,
    closed: AtomicBool,
    watcher: StdMutex<Option<JoinHandle<()>>>,
}

impl SessionHandle {
    fn shut_down(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(task) = self
            .watcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.shut_down();
    }
}

struct StreamState {
    brightness: u8,
    lit: Option<bool>,
}

/// WiZ backend implementing [`DeviceBackend`].
pub struct WizBackend {
    config: WizConfig,
    signing_key: Option<SigningKey>,
    request_policy: RequestPolicy,
    adopted: StdRwLock<HashMap<DeviceId, SocketAddr>>,
    sessions: StdRwLock<HashMap<DeviceId, Arc<SessionHandle>>>,
}

impl WizBackend {
    /// Backend for a validated config. An invalid signing key is ignored
    /// with a warning; config validation rejects it before this point.
    #[must_use]
    pub fn new(config: WizConfig) -> Self {
        let signing_key = config.parsed_signing_key().unwrap_or_else(|error| {
            warn!(%error, "ignoring the configured WiZ signing key");
            None
        });
        Self {
            config,
            signing_key,
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

    async fn open_session(&self, id: &DeviceId, addr: SocketAddr) -> Result<SessionHandle> {
        let profile = probe_device(addr, self.request_policy).await?;
        if profile.device_id() != *id {
            bail!(
                "the WiZ device at {addr} is now {}, not the adopted device",
                profile.mac
            );
        }
        if !profile.class.is_light() {
            bail!("the WiZ device at {addr} is a smart plug, not a light");
        }
        let socket = crate::transport::bind_for(&addr)
            .await
            .with_context(|| format!("failed to bind a WiZ socket for {addr}"))?;
        socket
            .connect(addr)
            .await
            .with_context(|| format!("failed to connect the WiZ socket to {addr}"))?;
        let socket = Arc::new(socket);
        let max_fps = self.config.max_fps.max(1);
        let cadence = OutputCadence::from_fps(max_fps).with_max_frame_silence(STATE_REFRESH);
        let info = profile.device_info(max_fps);
        let watcher = tokio::spawn(watch_replies(
            *id,
            Arc::clone(&socket),
            self.signing_key.is_some(),
        ));
        Ok(SessionHandle {
            profile,
            info,
            cadence,
            stream: Mutex::new(StreamState {
                brightness: u8::MAX,
                lit: None,
            }),
            socket,
            closed: AtomicBool::new(false),
            watcher: StdMutex::new(Some(watcher)),
        })
    }

    async fn write_session(
        device_id: &DeviceId,
        handle: &SessionHandle,
        signing_key: Option<&SigningKey>,
        colors: &[[u8; 3]],
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if handle.closed.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        let mut stream = handle.stream.lock().await;
        let color = representative_color(colors);
        let color = if stream.brightness == u8::MAX {
            color
        } else {
            let [r, g, b] = color;
            let scaled = Rgb::new(r, g, b).scale(f32::from(stream.brightness) / f32::from(u8::MAX));
            [scaled.r, scaled.g, scaled.b]
        };
        let pilot = Pilot::from_color(color, handle.profile.class, handle.profile.min_dimming);
        let lit = !matches!(pilot, Pilot::Off);
        let power_on = lit && stream.lit != Some(true);
        let signing = signing_key.map(|key| (key, unix_seconds()));
        let datagram = encode_set_pilot(pilot, power_on, signing);

        if let Some((id, observer)) = delivery {
            observer.transport_started(id);
        }
        handle
            .socket
            .send(datagram.as_bytes())
            .await
            .map_err(|error| {
                DeviceError::write(device_id, format!("WiZ datagram send failed: {error}"))
            })?;
        stream.lit = Some(lit);
        Ok(DeviceWriteOutcome::Sent)
    }
}

/// One color for a single-zone light. The host samples one LED per light;
/// a longer frame is averaged, the way the Govee driver folds segments.
#[must_use]
pub fn representative_color(colors: &[[u8; 3]]) -> [u8; 3] {
    match colors {
        [] => [0, 0, 0],
        [only] => *only,
        many => {
            let mut sums = [0_u32; 3];
            for color in many {
                for (sum, channel) in sums.iter_mut().zip(color) {
                    *sum += u32::from(*channel);
                }
            }
            let count = u32::try_from(many.len()).unwrap_or(u32::MAX).max(1);
            sums.map(|sum| u8::try_from(sum / count).unwrap_or(u8::MAX))
        }
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Read replies to streamed `setPilot` datagrams and warn when the device
/// rejects them, which on firmware in "Only verified controls" mode means
/// a signing key is needed. Frames never wait on this.
async fn watch_replies(device_id: DeviceId, socket: Arc<UdpSocket>, signed: bool) {
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    let mut last_warned: Option<Instant> = None;
    loop {
        let Ok(len) = socket.recv(&mut buffer).await else {
            // Connected UDP sockets surface ICMP errors here; keep reading.
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        let Ok(reply) = decode_reply(&buffer[..len]) else {
            continue;
        };
        let Some(error) = reply.error else {
            continue;
        };
        if last_warned.is_some_and(|at| at.elapsed() < REJECTION_WARN_INTERVAL) {
            continue;
        }
        last_warned = Some(Instant::now());
        let hint = match (error.code, signed) {
            (error_code::INVALID_PARAMS, false) => {
                "the home may be set to Only verified controls; configure signing_key"
            }
            (error_code::SECURITY_ERROR, _) | (error_code::INVALID_PARAMS, true) => {
                "the configured signing_key does not match this home"
            }
            _ => "the device rejected the command",
        };
        warn!(
            %device_id,
            code = error.code,
            message = %error.message,
            hint,
            "WiZ device rejected a frame"
        );
    }
}

struct WizFrameSink {
    device_id: DeviceId,
    handle: Arc<SessionHandle>,
    signing_key: Option<SigningKey>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for WizFrameSink {
    fn is_closed(&self) -> bool {
        self.handle.closed.load(Ordering::Acquire)
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        WizBackend::write_session(
            &self.device_id,
            &self.handle,
            self.signing_key.as_ref(),
            colors.as_slice(),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = WizBackend::write_session(
            &self.device_id,
            &self.handle,
            self.signing_key.as_ref(),
            colors.as_slice(),
            Some((id, observer.as_ref())),
        )
        .await;
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for WizBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: WIZ_DRIVER_ID.to_owned(),
            name: "WiZ".to_owned(),
            description: "WiZ lights over the local UDP JSON API".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let addr =
            addr_from_metadata(&discovered.metadata).map_err(|_| DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, addr);
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
        let addr = self
            .adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .copied()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;
        let handle = self
            .open_session(id, addr)
            .await
            .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
        info!(
            device_id = %id,
            %addr,
            mac = %handle.profile.mac,
            class = %handle.profile.class,
            signed = self.signing_key.is_some(),
            max_fps = handle.info.capabilities.max_fps,
            "Connected to WiZ device"
        );
        let previous = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(handle));
        if let Some(previous) = previous {
            previous.shut_down();
        }
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
                handle.shut_down();
                info!(device_id = %id, mac = %handle.profile.mac, "Disconnected from WiZ device");
                Ok(())
            }
            None => Err(DeviceError::Disconnected {
                device: id.to_string(),
            }),
        }
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        Self::write_session(id, &handle, self.signing_key.as_ref(), colors, None)
            .await
            .map(|_| ())
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        handle.stream.lock().await.brightness = brightness;
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
            Arc::new(WizFrameSink {
                device_id: *id,
                handle,
                signing_key: self.signing_key.clone(),
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}
