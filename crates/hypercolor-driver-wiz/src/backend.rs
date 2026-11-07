//! WiZ output backend: sessions, the frame sink, and reply watching
//! (Spec 93 §8 and §9.5).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::net::UdpSocket;
use tracing::{info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceLifecyclePolicy, DiscoveredDevice, Frame,
    OutputPolicy, Submission,
};
use hypercolor_driver_support::{DeviceSession, Session, SessionTable};
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

/// What a connect learned about the device.
struct WizFacts {
    profile: WizDeviceProfile,
    info: DeviceInfo,
    policy: OutputPolicy,
}

/// One open connection to a light.
struct WizSession {
    device_id: DeviceId,
    profile: WizDeviceProfile,
    signing_key: Option<SigningKey>,
    /// Shared with the reply watcher, which close joins before this drops.
    socket: Arc<UdpSocket>,
    brightness: u8,
    lit: Option<bool>,
}

impl DeviceSession for WizSession {
    type Facts = WizFacts;

    async fn submit(&mut self, frame: Frame) -> Submission {
        let (colors, attempt) = frame.into_parts();
        let color = representative_color(colors.as_slice());
        let color = if self.brightness == u8::MAX {
            color
        } else {
            let [r, g, b] = color;
            let scaled = Rgb::new(r, g, b).scale(f32::from(self.brightness) / f32::from(u8::MAX));
            [scaled.r, scaled.g, scaled.b]
        };
        let pilot = Pilot::from_color(color, self.profile.class, self.profile.min_dimming);
        let lit = !matches!(pilot, Pilot::Off);
        let power_on = lit && self.lit != Some(true);
        let signing = self.signing_key.as_ref().map(|key| (key, unix_seconds()));
        let datagram = encode_set_pilot(pilot, power_on, signing);

        let transmission = attempt.start();
        if let Err(error) = self.socket.send(datagram.as_bytes()).await {
            let error =
                DeviceError::write(self.device_id, format!("WiZ datagram send failed: {error}"));
            return transmission.failed(error).into();
        }
        self.lit = Some(lit);
        transmission.written().into()
    }
}

/// WiZ backend implementing [`DeviceBackend`].
pub struct WizBackend {
    config: WizConfig,
    signing_key: Option<SigningKey>,
    request_policy: RequestPolicy,
    adopted: StdRwLock<HashMap<DeviceId, SocketAddr>>,
    sessions: SessionTable<WizSession>,
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
            sessions: SessionTable::new(),
        }
    }

    /// Override the per-request policy used while connecting.
    #[must_use]
    pub const fn with_request_policy(mut self, policy: RequestPolicy) -> Self {
        self.request_policy = policy;
        self
    }

    fn opener(&self) -> WizOpener {
        WizOpener {
            config: self.config.clone(),
            signing_key: self.signing_key.clone(),
            request_policy: self.request_policy,
        }
    }
}

/// What an open takes from the backend, since it runs on the session table's
/// task and outlives the connect that started it.
struct WizOpener {
    config: WizConfig,
    signing_key: Option<SigningKey>,
    request_policy: RequestPolicy,
}

impl WizOpener {
    async fn open_session(&self, id: &DeviceId, addr: SocketAddr) -> Result<Session<WizSession>> {
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
        let policy = OutputPolicy::from_fps(max_fps).with_replay_after(STATE_REFRESH);
        let info = profile.device_info(max_fps);
        let mut session = Session::new(
            *id,
            WizFacts {
                profile: profile.clone(),
                info,
                policy,
            },
            WizSession {
                device_id: *id,
                profile,
                signing_key: self.signing_key.clone(),
                socket: Arc::clone(&socket),
                brightness: u8::MAX,
                lit: None,
            },
        );
        session.spawn(watch_replies(*id, socket, self.signing_key.is_some()));
        Ok(session)
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
                let addr = self
                    .adopted
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&id)
                    .copied();
                let opener = self.opener();
                async move {
                    let addr = addr.ok_or(DeviceError::NotAdopted { device_id: id })?;
                    let session = opener
                        .open_session(&id, addr)
                        .await
                        .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
                    let facts = session.facts();
                    info!(
                        device_id = %id,
                        %addr,
                        mac = %facts.profile.mac,
                        class = %facts.profile.class,
                        signed = opener.signing_key.is_some(),
                        max_fps = facts.info.capabilities.max_fps,
                        "Connected to WiZ device"
                    );
                    Ok(session)
                }
            })
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from WiZ device");
        Ok(())
    }

    async fn retire(&self) -> Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        self.sessions.require(id)?.admit().await?.brightness = brightness;
        Ok(())
    }

    fn output_policy(&self, id: &DeviceId) -> Option<OutputPolicy> {
        self.sessions.facts(id).map(|facts| facts.policy)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}
