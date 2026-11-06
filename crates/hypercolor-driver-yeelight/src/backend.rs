//! Yeelight output backend: control connections, music mode, the quota
//! guard, and the frame sink (Spec 94 §5, §6, and §8.5).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::YEELIGHT_DRIVER_ID;
use crate::config::YeelightConfig;
use crate::framing::{LineReader, write_bounded};
use crate::protocol::{
    Capabilities, Command, LightState, commands_for, decode_message, refresh_commands,
    worst_case_commands,
};
use crate::scanner::{
    AdoptedRoute, YeelightDeviceProfile, metadata_key, profile_metadata, route_from_metadata,
    search_until,
};

/// One command per second: the documented quota of 60 commands per minute
/// on one connection (Spec 94 §6.1).
pub const NORMAL_MODE_COMMAND_INTERVAL: Duration = Duration::from_secs(1);
/// Ask the host to replay the newest frame after this much silence in
/// normal mode, so a frame the quota guard skipped is still delivered.
/// Replays of an unchanged state cost no command.
pub const NORMAL_MODE_REPLAY: Duration = Duration::from_secs(3);
/// Pause after the device reports its quota exhausted.
pub const QUOTA_BACKOFF: Duration = Duration::from_secs(15);
/// Longest a command write may take. A peer that stops reading would
/// otherwise stall the frame path; a timed-out write leaves a partial JSON
/// line, so the connection is dropped and the host reconnects.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// Re-send the last state after this much silence in music mode, so a
/// dropped music connection is noticed by the next write.
pub const MUSIC_KEEPALIVE: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
const MUSIC_ACCEPT_TIMEOUT: Duration = Duration::from_secs(3);
const IDENTITY_SEARCH_WINDOW: Duration = Duration::from_millis(800);

/// How frames reach the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// The device connected back to Hypercolor; no quota, no replies.
    Music,
    /// Commands on the control connection, inside the per-minute quota.
    Normal,
}

impl StreamMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Normal => "normal",
        }
    }
}

struct SessionHandle {
    profile: YeelightDeviceProfile,
    info: DeviceInfo,
    cadence: OutputCadence,
    mode: StreamMode,
    capabilities: Capabilities,
    music_enabled: bool,
    stream: Mutex<StreamState>,
    shared: Arc<SessionShared>,
    reader: StdMutex<Option<JoinHandle<()>>>,
}

impl SessionHandle {
    fn shut_down(&self) {
        self.shared.closed.store(true, Ordering::Release);
        if let Some(task) = self
            .reader
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

#[derive(Default)]
struct SessionShared {
    closed: AtomicBool,
    quota_blocked_until: StdMutex<Option<Instant>>,
    /// The device rejected a command for quota; the state it would have
    /// set is unknown, so the next frame is resent in full.
    quota_rejected: AtomicBool,
    /// Terminal transport failure: the next write reports it so the host
    /// reconnects.
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
        let mut failure = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        if failure.is_none() {
            *failure = Some(reason);
        }
    }

    fn quota_blocked(&self, now: Instant) -> bool {
        self.quota_blocked_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|until| now < until)
    }
}

struct StreamState {
    /// `None` once the connection was dropped after a failure or
    /// disconnect.
    control: Option<OwnedWriteHalf>,
    music: Option<OwnedWriteHalf>,
    next_id: u32,
    brightness: u8,
    last_state: Option<LightState>,
    last_sent_at: Option<Instant>,
    next_allowed_at: Option<Instant>,
}

impl StreamState {
    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }
}

/// Yeelight backend implementing [`DeviceBackend`].
pub struct YeelightBackend {
    config: YeelightConfig,
    quota_backoff: Duration,
    adopted: StdRwLock<HashMap<DeviceId, AdoptedRoute>>,
    sessions: StdRwLock<HashMap<DeviceId, Arc<SessionHandle>>>,
}

impl YeelightBackend {
    #[must_use]
    pub fn new(config: YeelightConfig) -> Self {
        Self {
            config,
            quota_backoff: QUOTA_BACKOFF,
            adopted: StdRwLock::new(HashMap::new()),
            sessions: StdRwLock::new(HashMap::new()),
        }
    }

    /// Override the pause after a quota error.
    #[must_use]
    pub const fn with_quota_backoff(mut self, backoff: Duration) -> Self {
        self.quota_backoff = backoff;
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

    async fn open_session(&self, id: &DeviceId, route: &AdoptedRoute) -> Result<SessionHandle> {
        let profile = confirm_identity(route).await?;
        if profile.device_id() != *id {
            bail!(
                "the Yeelight device at {} is now {}, not the adopted device",
                route.control,
                profile.reply.id
            );
        }
        let capabilities = profile.capabilities();
        if !capabilities.drivable() {
            bail!(
                "the Yeelight device at {} has no drivable light",
                route.control
            );
        }

        let control = tokio::time::timeout(TCP_CONNECT_TIMEOUT, TcpStream::connect(profile.reply.location))
            .await
            .map_err(|_| {
                anyhow!(
                    "Yeelight device at {} refused or ignored the control connection; is LAN Control on?",
                    profile.reply.location
                )
            })?
            .with_context(|| {
                format!(
                    "failed to open the Yeelight control connection to {}; is LAN Control on?",
                    profile.reply.location
                )
            })?;
        control.set_nodelay(true).ok();
        let local_ip = control
            .local_addr()
            .context("Yeelight control connection has no local address")?
            .ip();
        let device_ip = control
            .peer_addr()
            .context("Yeelight control connection has no peer address")?
            .ip();
        let (read_half, mut write_half) = control.into_split();
        let mut lines = LineReader::new(read_half);
        let mut next_id = 0_u32;

        let mut music = None;
        if self.config.music_mode && capabilities.music {
            next_id += 1;
            match start_music(&mut write_half, &mut lines, next_id, local_ip, device_ip).await {
                Ok(stream) => music = Some(stream),
                Err(MusicSetupError::Unavailable(error)) => warn!(
                    device_id = %id,
                    error = %format!("{error:#}"),
                    "Yeelight music mode unavailable; streaming inside the one-per-second quota"
                ),
                // The control connection may hold a partial line or be
                // closed: never reuse it. The host retries the connect.
                Err(MusicSetupError::ControlBroken(error)) => {
                    return Err(
                        error.context("the Yeelight control connection failed during music setup")
                    );
                }
            }
        }
        let mode = if music.is_some() {
            StreamMode::Music
        } else {
            StreamMode::Normal
        };
        let cadence = match mode {
            StreamMode::Music => OutputCadence::from_fps(self.config.max_fps.max(1))
                .with_max_frame_silence(MUSIC_KEEPALIVE),
            StreamMode::Normal => {
                let interval =
                    NORMAL_MODE_COMMAND_INTERVAL.saturating_mul(worst_case_commands(capabilities));
                OutputCadence::from_min_interval(interval, 1)
                    .with_max_frame_silence(NORMAL_MODE_REPLAY)
            }
        };
        let info = profile.device_info(cadence.target_fps());

        let shared = Arc::new(SessionShared::default());
        let reader = tokio::spawn(read_control(
            *id,
            lines,
            Arc::clone(&shared),
            self.quota_backoff,
        ));
        Ok(SessionHandle {
            profile,
            info,
            cadence,
            mode,
            capabilities,
            music_enabled: self.config.music_mode,
            stream: Mutex::new(StreamState {
                control: Some(write_half),
                music,
                next_id,
                brightness: u8::MAX,
                last_state: None,
                last_sent_at: None,
                next_allowed_at: None,
            }),
            shared,
            reader: StdMutex::new(Some(reader)),
        })
    }

    async fn write_session(
        device_id: &DeviceId,
        handle: &SessionHandle,
        colors: &[[u8; 3]],
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if handle.shared.closed.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if let Some(reason) = handle.shared.failure() {
            return Err(DeviceError::write(device_id, reason));
        }
        let now = Instant::now();
        let mut stream = handle.stream.lock().await;
        if handle.shared.closed.load(Ordering::Acquire) {
            // Disconnected while this write waited for the lock.
            stream.music = None;
            stream.control = None;
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        if handle.shared.quota_rejected.swap(false, Ordering::AcqRel) {
            // The rejected command's state never landed; resend in full.
            stream.last_state = None;
        }
        if handle.mode == StreamMode::Normal {
            if handle.shared.quota_blocked(now) {
                return Ok(DeviceWriteOutcome::SuppressedCadence);
            }
            if stream.next_allowed_at.is_some_and(|next| now < next) {
                return Ok(DeviceWriteOutcome::SuppressedCadence);
            }
        }

        let color = representative_color(colors);
        let color = if stream.brightness == u8::MAX {
            color
        } else {
            let [r, g, b] = color;
            let scaled = Rgb::new(r, g, b).scale(f32::from(stream.brightness) / f32::from(u8::MAX));
            [scaled.r, scaled.g, scaled.b]
        };
        let next = LightState::from_color(color, handle.capabilities);
        let unchanged = stream.last_state == Some(next);
        let refresh_due = handle.mode == StreamMode::Music
            && stream
                .last_sent_at
                .is_none_or(|sent| now.saturating_duration_since(sent) >= MUSIC_KEEPALIVE);
        let commands = if unchanged && refresh_due {
            refresh_commands(next, handle.capabilities)
        } else {
            commands_for(stream.last_state, next, handle.capabilities)
        };
        if commands.is_empty() {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }

        let mut payload = String::new();
        for command in &commands {
            let id = stream.next_id();
            payload.push_str(&command.encode(id));
        }
        if let Some((id, observer)) = delivery {
            observer.transport_started(id);
        }
        let writer = match handle.mode {
            StreamMode::Music => stream.music.as_mut(),
            StreamMode::Normal => stream.control.as_mut(),
        };
        let Some(writer) = writer else {
            return Err(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        };
        if let Err(error) = write_bounded(writer, payload.as_bytes(), WRITE_TIMEOUT).await {
            // A partial line cannot be resumed: drop the connection and
            // make every later write report the failure.
            let reason = format!("Yeelight command write failed: {error}");
            handle.shared.mark_failed(reason.clone());
            stream.music = None;
            stream.control = None;
            return Err(DeviceError::write(device_id, reason));
        }
        if handle.shared.closed.load(Ordering::Acquire) {
            // Disconnect began while this write was in flight.
            stream.music = None;
            stream.control = None;
        }
        stream.last_state = Some(next);
        stream.last_sent_at = Some(now);
        if handle.mode == StreamMode::Normal {
            let commands = u32::try_from(commands.len()).unwrap_or(u32::MAX);
            stream.next_allowed_at =
                Some(now + NORMAL_MODE_COMMAND_INTERVAL.saturating_mul(commands));
        }
        Ok(DeviceWriteOutcome::Sent)
    }
}

/// Re-run the search against the device itself to confirm its id and read
/// a fresh `support` list. A device that does not answer keeps its adopted
/// route; the control connection settles reachability.
async fn confirm_identity(route: &AdoptedRoute) -> Result<YeelightDeviceProfile> {
    let fallback = || YeelightDeviceProfile {
        reply: crate::protocol::SearchReply {
            location: route.control,
            id: route.id.clone(),
            model: None,
            fw_ver: None,
            support: route.support.clone(),
            power: None,
            bright: None,
            name: None,
        },
        search_port: route.search.port(),
    };
    match search_until(&[route.search], IDENTITY_SEARCH_WINDOW, Some(1)).await {
        Ok(mut replies) if !replies.is_empty() => Ok(replies.remove(0)),
        Ok(_) => {
            debug!(search = %route.search, "Yeelight device did not answer the identity search");
            Ok(fallback())
        }
        Err(error) => {
            debug!(search = %route.search, error = %error, "Yeelight identity search failed");
            Ok(fallback())
        }
    }
}

/// Why music mode could not start.
enum MusicSetupError {
    /// Music mode is out, but the control connection is intact.
    Unavailable(anyhow::Error),
    /// The control connection failed mid-setup and must not be reused.
    ControlBroken(anyhow::Error),
}

/// Ask the device to connect back for music mode and accept it. Only a
/// connection from the device's own address is accepted, within the
/// original deadline; anything else on the LAN that connects first is
/// dropped.
async fn start_music(
    control: &mut OwnedWriteHalf,
    lines: &mut LineReader<OwnedReadHalf>,
    id: u32,
    local_ip: IpAddr,
    device_ip: IpAddr,
) -> Result<OwnedWriteHalf, MusicSetupError> {
    use MusicSetupError::{ControlBroken, Unavailable};
    if !local_ip.is_ipv4() {
        return Err(Unavailable(anyhow!(
            "music mode needs an IPv4 path to the device"
        )));
    }
    let listener = TcpListener::bind(SocketAddr::new(local_ip, 0))
        .await
        .context("failed to open the music-mode listener")
        .map_err(Unavailable)?;
    let port = listener
        .local_addr()
        .context("music-mode listener has no address")
        .map_err(Unavailable)?
        .port();
    let request = Command::MusicStart {
        host: local_ip,
        port,
    }
    .encode(id);
    write_bounded(control, request.as_bytes(), WRITE_TIMEOUT)
        .await
        .context("failed to send set_music")
        .map_err(ControlBroken)?;
    let reply = tokio::time::timeout(REPLY_TIMEOUT, async {
        loop {
            let Some(line) = lines.next_line().await? else {
                return Err(anyhow!("the device closed the control connection"));
            };
            let Ok(message) = decode_message(&line) else {
                continue;
            };
            if message.id == Some(id) {
                return Ok(message);
            }
        }
    })
    .await
    .map_err(|_| Unavailable(anyhow!("the device did not answer set_music")))?
    .map_err(ControlBroken)?;
    if let Some(error) = reply.error {
        return Err(Unavailable(anyhow!(
            "the device refused set_music: {}",
            error.message
        )));
    }
    let deadline = tokio::time::Instant::now() + MUSIC_ACCEPT_TIMEOUT;
    loop {
        let (stream, peer) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| Unavailable(anyhow!("the device did not connect back for music mode")))?
            .context("music-mode accept failed")
            .map_err(Unavailable)?;
        if peer.ip() != device_ip {
            warn!(%peer, %device_ip, "dropping a music-mode connection from another host");
            continue;
        }
        stream.set_nodelay(true).ok();
        debug!(%peer, "Yeelight music connection accepted");
        let (_, write_half) = stream.into_split();
        return Ok(write_half);
    }
}

/// Read replies and `props` pushes on the control connection. A quota
/// error pauses normal-mode output and invalidates the light's known
/// state; end of stream, a read error, or an oversized line marks the
/// session failed so the next write reports it and the host reconnects.
async fn read_control(
    device_id: DeviceId,
    mut lines: LineReader<OwnedReadHalf>,
    shared: Arc<SessionShared>,
    quota_backoff: Duration,
) {
    let reason = loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break "the device closed the control connection".to_owned(),
            Err(error) => break format!("the control connection failed: {error}"),
        };
        let Ok(message) = decode_message(&line) else {
            continue;
        };
        let Some(error) = message.error else {
            continue;
        };
        if error.is_quota_exceeded() {
            warn!(%device_id, "Yeelight command quota exceeded; pausing output");
            *shared
                .quota_blocked_until
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now() + quota_backoff);
            shared.quota_rejected.store(true, Ordering::Release);
        } else {
            debug!(%device_id, code = error.code, message = %error.message, "Yeelight command rejected");
        }
    };
    if !shared.closed.load(Ordering::Acquire) {
        warn!(%device_id, %reason, "Yeelight control connection lost");
        shared.mark_failed(format!("Yeelight {reason}"));
    }
}

/// One color for a single-zone light; a longer frame is averaged.
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

struct YeelightFrameSink {
    device_id: DeviceId,
    handle: Arc<SessionHandle>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for YeelightFrameSink {
    fn is_closed(&self) -> bool {
        self.handle.shared.closed.load(Ordering::Acquire)
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        YeelightBackend::write_session(&self.device_id, &self.handle, colors.as_slice(), None)
            .await
            .map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        YeelightBackend::write_session(&self.device_id, &self.handle, colors.as_slice(), None).await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = YeelightBackend::write_session(
            &self.device_id,
            &self.handle,
            colors.as_slice(),
            Some((id, observer.as_ref())),
        )
        .await;
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for YeelightBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: YEELIGHT_DRIVER_ID.to_owned(),
            name: "Yeelight".to_owned(),
            description: "Yeelight lights over LAN Control and music mode".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let route =
            route_from_metadata(&discovered.metadata).map_err(|_| DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, route);
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
        Ok(self.session(id).map(|handle| {
            let mut metadata = profile_metadata(&handle.profile, handle.music_enabled);
            metadata.insert("stream_mode".to_owned(), handle.mode.as_str().to_owned());
            metadata.insert(
                metadata_key::MUSIC_MODE.to_owned(),
                match handle.mode {
                    StreamMode::Music => "on",
                    StreamMode::Normal if handle.capabilities.music && handle.music_enabled => {
                        "failed"
                    }
                    StreamMode::Normal if handle.capabilities.music => "off",
                    StreamMode::Normal => "unsupported",
                }
                .to_owned(),
            );
            metadata
        }))
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
            .open_session(id, &route)
            .await
            .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
        info!(
            device_id = %id,
            control = %handle.profile.reply.location,
            yeelight_id = %handle.profile.reply.id,
            model = ?handle.profile.reply.model,
            mode = handle.mode.as_str(),
            max_fps = handle.info.capabilities.max_fps,
            "Connected to Yeelight device"
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
        let Some(handle) = removed else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        handle.shut_down();
        // A stalled writer holds the stream lock for at most WRITE_TIMEOUT;
        // never wait longer than that plus the stop itself.
        if let Ok(mut stream) = tokio::time::timeout(WRITE_TIMEOUT, handle.stream.lock()).await {
            if handle.mode == StreamMode::Music {
                let stop = Command::MusicStop.encode(stream.next_id());
                let sent = match stream.control.as_mut() {
                    Some(control) => write_bounded(control, stop.as_bytes(), WRITE_TIMEOUT)
                        .await
                        .is_ok(),
                    None => false,
                };
                if !sent {
                    debug!(device_id = %id, "Yeelight set_music stop was not delivered");
                }
            }
            // Close both connections now, even if a frame sink is still
            // held somewhere: the device allows only four.
            stream.music = None;
            stream.control = None;
        } else {
            // The session is already closed, so the stalled writer bails
            // or times out within WRITE_TIMEOUT; drop the sockets as soon
            // as it lets go, even if a frame sink is still held.
            debug!(device_id = %id, "Yeelight writer stalled; closing its connections once it yields");
            let cleanup = Arc::clone(&handle);
            tokio::spawn(async move {
                let mut stream = cleanup.stream.lock().await;
                stream.music = None;
                stream.control = None;
            });
        }
        info!(device_id = %id, yeelight_id = %handle.profile.reply.id, "Disconnected from Yeelight device");
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let handle = self.connected(id)?;
        Self::write_session(id, &handle, colors, None)
            .await
            .map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        let handle = self.connected(id)?;
        Self::write_session(id, &handle, colors.as_slice(), None).await
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
            Arc::new(YeelightFrameSink {
                device_id: *id,
                handle,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}
