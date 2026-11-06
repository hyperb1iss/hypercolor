//! Twinkly output backend: login, real-time sessions, the frame sink, and
//! session upkeep (Spec 92 §8 and §10.5).

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::TWINKLY_DRIVER_ID;
use crate::api::{ApiError, AuthToken, DEFAULT_REQUEST_TIMEOUT, TwinklyClient, TwinklyEndpoint};
use crate::config::TwinklyConfig;
use crate::layout::{LayoutSource, segments_for};
use crate::protocol::RtFrameEncoder;
use crate::scanner::{
    TwinklyDeviceProfile, endpoint_from_metadata, metadata_key, profile_metadata,
};

/// LED mode that accepts real-time datagrams.
pub const RT_MODE: &str = "rt";
/// How often a connected session checks its token and mode.
pub const DEFAULT_UPKEEP_INTERVAL: Duration = Duration::from_secs(5);
/// Re-send the last frame after this much silence so the device never
/// times out of real-time mode while Hypercolor holds it.
pub const RT_KEEPALIVE: Duration = Duration::from_secs(1);
/// Log in again this long before the granted token lifetime runs out.
pub const TOKEN_REFRESH_MARGIN: Duration = Duration::from_mins(5);
/// Consecutive upkeep failures before the session reports itself broken.
pub const MAX_UPKEEP_FAILURES: u32 = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Connected device: immutable facts plus the shared stream state.
struct SessionHandle {
    profile: TwinklyDeviceProfile,
    info: DeviceInfo,
    layout_source: LayoutSource,
    cadence: OutputCadence,
    shared: Arc<SessionShared>,
    upkeep: StdMutex<Option<JoinHandle<()>>>,
}

impl SessionHandle {
    fn shut_down(&self) {
        self.shared.closed.store(true, Ordering::Release);
        if let Some(task) = self
            .upkeep
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }

    fn metadata(&self) -> HashMap<String, String> {
        let mut metadata = profile_metadata(&self.profile);
        metadata.insert(
            metadata_key::LAYOUT_SOURCE.to_owned(),
            self.layout_source.as_str().to_owned(),
        );
        metadata
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// State shared between the frame path and the upkeep task.
struct SessionShared {
    stream: Mutex<StreamState>,
    closed: AtomicBool,
    failure: StdMutex<Option<String>>,
}

impl SessionShared {
    fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn mark_failed(&self, reason: String) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
    }
}

struct StreamState {
    socket: UdpSocket,
    encoder: RtFrameEncoder,
    brightness: u8,
    scaled: Vec<[u8; 3]>,
    last_size_mismatch_warn_at: Option<Instant>,
}

/// Twinkly backend implementing [`DeviceBackend`].
pub struct TwinklyBackend {
    config: TwinklyConfig,
    request_timeout: Duration,
    upkeep_interval: Duration,
    adopted: StdRwLock<HashMap<DeviceId, TwinklyEndpoint>>,
    sessions: StdRwLock<HashMap<DeviceId, Arc<SessionHandle>>>,
}

impl TwinklyBackend {
    #[must_use]
    pub fn new(config: TwinklyConfig) -> Self {
        Self {
            config,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            upkeep_interval: DEFAULT_UPKEEP_INTERVAL,
            adopted: StdRwLock::new(HashMap::new()),
            sessions: StdRwLock::new(HashMap::new()),
        }
    }

    /// Override the per-request REST timeout.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Override how often connected sessions check their token and mode.
    #[must_use]
    pub const fn with_upkeep_interval(mut self, interval: Duration) -> Self {
        self.upkeep_interval = interval;
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
        id: &DeviceId,
        endpoint: TwinklyEndpoint,
    ) -> Result<SessionHandle> {
        let client = TwinklyClient::new(endpoint, self.request_timeout)?;
        let gestalt = client.gestalt().await?;
        let firmware = client.firmware_version().await?;
        let profile = TwinklyDeviceProfile::from_gestalt(endpoint, &gestalt, &firmware.version)?;
        if profile.device_id() != *id {
            bail!(
                "the Twinkly device at {endpoint} is now {} ({}), not the adopted device",
                profile.display_name(),
                profile.mac
            );
        }

        let token = client.login().await?;
        let layout = match client.layout(&token).await {
            Ok(layout) => Some(layout),
            Err(error) => {
                warn!(device_id = %id, %endpoint, error = %error, "Twinkly layout unavailable; using wiring order");
                None
            }
        };
        let (segments, layout_source) = segments_for(profile.led_count, layout.as_ref());
        let max_fps = self.config.max_fps.max(1);
        let info = profile.device_info_with_segments(segments, max_fps);
        client
            .set_mode(&token, RT_MODE)
            .await
            .context("Twinkly device refused real-time mode")?;

        let socket = bind_rt_socket(endpoint.rt_addr()).await?;
        let led_count = usize::try_from(profile.led_count).unwrap_or(usize::MAX);
        let encoder = RtFrameEncoder::new(
            profile.rt_protocol(),
            profile.led_profile,
            led_count,
            token.bytes,
        )
        .map_err(|error| anyhow!("{error}"))?;
        let cadence = OutputCadence::from_fps(max_fps).with_max_frame_silence(RT_KEEPALIVE);

        let shared = Arc::new(SessionShared {
            stream: Mutex::new(StreamState {
                socket,
                encoder,
                brightness: u8::MAX,
                scaled: Vec::with_capacity(led_count),
                last_size_mismatch_warn_at: None,
            }),
            closed: AtomicBool::new(false),
            failure: StdMutex::new(None),
        });
        let upkeep = tokio::spawn(run_upkeep(
            *id,
            client,
            token,
            Arc::clone(&shared),
            self.upkeep_interval,
        ));
        Ok(SessionHandle {
            profile,
            info,
            layout_source,
            cadence,
            shared,
            upkeep: StdMutex::new(Some(upkeep)),
        })
    }

    async fn write_session(
        device_id: &DeviceId,
        handle: &SessionHandle,
        colors: &[[u8; 3]],
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<(), DeviceError> {
        if handle.shared.closed.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if let Some(reason) = handle.shared.failure() {
            return Err(DeviceError::write(device_id, reason));
        }

        let mut guard = handle.shared.stream.lock().await;
        let stream = &mut *guard;
        let led_count = stream.encoder.led_count();
        if colors.len() != led_count {
            let should_warn = stream
                .last_size_mismatch_warn_at
                .is_none_or(|warned_at| warned_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %device_id,
                    expected_led_count = led_count,
                    actual_led_count = colors.len(),
                    "Twinkly frame size mismatch; padding or truncating to the LED count"
                );
                stream.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let datagrams = if stream.brightness == u8::MAX {
            stream.encoder.encode(colors)
        } else {
            let factor = f32::from(stream.brightness) / f32::from(u8::MAX);
            stream.scaled.clear();
            stream.scaled.extend(colors.iter().map(|&[r, g, b]| {
                let scaled = Rgb::new(r, g, b).scale(factor);
                [scaled.r, scaled.g, scaled.b]
            }));
            stream.encoder.encode(&stream.scaled)
        };

        if let Some((id, observer)) = delivery {
            observer.transport_started(id);
        }
        for datagram in datagrams {
            stream.socket.send(datagram).await.map_err(|error| {
                DeviceError::write(device_id, format!("Twinkly datagram send failed: {error}"))
            })?;
        }
        Ok(())
    }
}

async fn bind_rt_socket(peer: SocketAddr) -> Result<UdpSocket> {
    let local = match peer {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let socket = UdpSocket::bind(local)
        .await
        .with_context(|| format!("failed to bind a Twinkly real-time socket for {peer}"))?;
    socket
        .connect(peer)
        .await
        .with_context(|| format!("failed to connect the Twinkly real-time socket to {peer}"))?;
    Ok(socket)
}

/// Keep a session streamable: refresh the token before it expires, log in
/// again when the device rejects it (another client logged in), and put
/// the device back in real-time mode when something switched it away.
async fn run_upkeep(
    device_id: DeviceId,
    client: TwinklyClient,
    mut token: AuthToken,
    shared: Arc<SessionShared>,
    interval: Duration,
) {
    let mut obtained_at = Instant::now();
    let mut failures = 0_u32;
    loop {
        tokio::time::sleep(interval).await;
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let refresh_due = token.expires_in.is_some_and(|lifetime| {
            obtained_at.elapsed() + TOKEN_REFRESH_MARGIN.min(lifetime / 2) >= lifetime
        });
        let outcome = if refresh_due {
            debug!(%device_id, "Twinkly token near expiry; logging in again");
            relogin(&client, &shared).await
        } else {
            check_mode(&client, &token).await.map(|()| None)
        };
        let outcome = match outcome {
            Err(error)
                if error
                    .downcast_ref::<ApiError>()
                    .is_some_and(ApiError::is_unauthorized) =>
            {
                info!(%device_id, "Twinkly token rejected, likely another login; logging in again");
                relogin(&client, &shared).await
            }
            other => other,
        };
        match outcome {
            Ok(refreshed) => {
                failures = 0;
                if let Some(fresh) = refreshed {
                    token = fresh;
                    obtained_at = Instant::now();
                }
            }
            Err(error) => {
                failures += 1;
                warn!(%device_id, failures, error = %format!("{error:#}"), "Twinkly session upkeep failed");
                if failures >= MAX_UPKEEP_FAILURES {
                    shared.mark_failed(format!("Twinkly session upkeep failed: {error:#}"));
                    return;
                }
            }
        }
    }
}

async fn check_mode(client: &TwinklyClient, token: &AuthToken) -> Result<()> {
    let mode = client.mode(token).await?;
    if mode.mode != RT_MODE {
        info!(endpoint = %client.endpoint(), mode = %mode.mode, "Twinkly left real-time mode; switching back");
        client.set_mode(token, RT_MODE).await?;
    }
    Ok(())
}

async fn relogin(client: &TwinklyClient, shared: &SessionShared) -> Result<Option<AuthToken>> {
    let token = client.login().await?;
    shared.stream.lock().await.encoder.set_token(token.bytes);
    client.set_mode(&token, RT_MODE).await?;
    Ok(Some(token))
}

struct TwinklyFrameSink {
    device_id: DeviceId,
    handle: Arc<SessionHandle>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for TwinklyFrameSink {
    fn is_closed(&self) -> bool {
        self.handle.shared.closed.load(Ordering::Acquire)
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        TwinklyBackend::write_session(&self.device_id, &self.handle, colors.as_slice(), None).await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = TwinklyBackend::write_session(
            &self.device_id,
            &self.handle,
            colors.as_slice(),
            Some((id, observer.as_ref())),
        )
        .await
        .map(|()| DeviceWriteOutcome::Sent);
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for TwinklyBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: TWINKLY_DRIVER_ID.to_owned(),
            name: "Twinkly".to_owned(),
            description: "Twinkly lights over the local REST API and real-time UDP".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let endpoint =
            endpoint_from_metadata(&discovered.metadata).map_err(|_| DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, endpoint);
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
        Ok(self.session(id).map(|handle| handle.metadata()))
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
        let endpoint = self
            .adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .copied()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;

        let handle = self
            .open_session(id, endpoint)
            .await
            .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
        info!(
            device_id = %id,
            %endpoint,
            mac = %handle.profile.mac,
            leds = handle.info.total_led_count(),
            profile = %handle.profile.led_profile,
            rt_protocol = %handle.profile.rt_protocol(),
            layout = handle.layout_source.as_str(),
            max_fps = handle.info.capabilities.max_fps,
            "Connected to Twinkly device"
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
                info!(device_id = %id, mac = %handle.profile.mac, "Disconnected from Twinkly device");
                Ok(())
            }
            None => Err(DeviceError::Disconnected {
                device: id.to_string(),
            }),
        }
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        Self::write_session(id, &handle, colors, None).await
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        handle.shared.stream.lock().await.brightness = brightness;
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
            Arc::new(TwinklyFrameSink {
                device_id: *id,
                handle,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}
