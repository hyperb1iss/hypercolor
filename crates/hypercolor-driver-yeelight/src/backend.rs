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
use tracing::{debug, info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome,
    DiscoveredDevice, OutputCadence,
};
use hypercolor_driver_support::framing::{BoundedLines, FrameWriteError, FramedWriter, LineError};
use hypercolor_driver_support::{DeviceSession, FrameDelivery, Session, SessionTable};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use crate::YEELIGHT_DRIVER_ID;
use crate::config::YeelightConfig;
use crate::protocol::{
    Capabilities, Command, LightState, MAX_LINE_BYTES, commands_for, decode_message,
    refresh_commands, worst_case_commands,
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
/// Longest wait for a reply, and for any line to finish once its first
/// byte arrives.
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

/// What a connect learned about the device.
struct YeelightFacts {
    profile: YeelightDeviceProfile,
    info: DeviceInfo,
    cadence: OutputCadence,
    mode: StreamMode,
    capabilities: Capabilities,
    music_enabled: bool,
}

#[derive(Default)]
struct SessionShared {
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

/// One open connection to a light.
struct YeelightSession {
    device_id: DeviceId,
    mode: StreamMode,
    capabilities: Capabilities,
    shared: Arc<SessionShared>,
    /// Closed once a failed write dropped the connection: the device allows
    /// only four, and a partial line can never be resumed.
    control: FramedWriter<OwnedWriteHalf>,
    /// `None` in normal mode.
    music: Option<FramedWriter<OwnedWriteHalf>>,
    next_id: u32,
    brightness: u8,
    last_state: Option<LightState>,
    last_sent_at: Option<Instant>,
    next_allowed_at: Option<Instant>,
}

impl YeelightSession {
    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    fn close_connections(&mut self) {
        self.music = None;
        self.control.close();
    }
}

fn write_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + WRITE_TIMEOUT
}

impl DeviceSession for YeelightSession {
    type Facts = YeelightFacts;

    /// A stalled writer holds the session for at most [`WRITE_TIMEOUT`].
    const SETTLE: Duration = WRITE_TIMEOUT;
    const CLEANUP: Duration = WRITE_TIMEOUT.saturating_add(Duration::from_millis(100));

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if let Some(reason) = self.shared.failure() {
            return Err(DeviceError::write(self.device_id, reason));
        }
        let now = Instant::now();
        if self.shared.quota_rejected.swap(false, Ordering::AcqRel) {
            // The rejected command's state never landed; resend in full.
            self.last_state = None;
        }
        if self.mode == StreamMode::Normal {
            if self.shared.quota_blocked(now) {
                return Ok(DeviceWriteOutcome::SuppressedCadence);
            }
            if self.next_allowed_at.is_some_and(|next| now < next) {
                return Ok(DeviceWriteOutcome::SuppressedCadence);
            }
        }

        let color = representative_color(colors);
        let color = if self.brightness == u8::MAX {
            color
        } else {
            let [r, g, b] = color;
            let scaled = Rgb::new(r, g, b).scale(f32::from(self.brightness) / f32::from(u8::MAX));
            [scaled.r, scaled.g, scaled.b]
        };
        let next = LightState::from_color(color, self.capabilities);
        let unchanged = self.last_state == Some(next);
        let refresh_due = self.mode == StreamMode::Music
            && self
                .last_sent_at
                .is_none_or(|sent| now.saturating_duration_since(sent) >= MUSIC_KEEPALIVE);
        let commands = if unchanged && refresh_due {
            refresh_commands(next, self.capabilities)
        } else {
            commands_for(self.last_state, next, self.capabilities)
        };
        if commands.is_empty() {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }

        let mut payload = String::new();
        for command in &commands {
            let id = self.next_id();
            payload.push_str(&command.encode(id));
        }
        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        let writer = match self.mode {
            StreamMode::Music => self.music.as_mut(),
            StreamMode::Normal => Some(&mut self.control),
        };
        let Some(writer) = writer else {
            return Err(DeviceError::Disconnected {
                device: self.device_id.to_string(),
            });
        };
        match writer.write(payload.as_bytes(), write_deadline()).await {
            Ok(()) => {}
            Err(FrameWriteError::Closed) => {
                return Err(DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                });
            }
            Err(error) => {
                // The writer already gave up its connection. Close the
                // other one too and make every later write report this.
                let reason = format!("Yeelight command write failed: {error}");
                self.shared.mark_failed(reason.clone());
                self.close_connections();
                return Err(DeviceError::write(self.device_id, reason));
            }
        }
        self.last_state = Some(next);
        self.last_sent_at = Some(now);
        if self.mode == StreamMode::Normal {
            let commands = u32::try_from(commands.len()).unwrap_or(u32::MAX);
            self.next_allowed_at =
                Some(now + NORMAL_MODE_COMMAND_INTERVAL.saturating_mul(commands));
        }
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Leave music mode. Dropping the session then closes both connections.
    async fn finish(mut self) -> Result<(), DeviceError> {
        if self.mode != StreamMode::Music {
            return Ok(());
        }
        let stop = Command::MusicStop.encode(self.next_id());
        let sent = self.control.write(stop.as_bytes(), write_deadline());
        if sent.await.is_err() {
            debug!(device_id = %self.device_id, "Yeelight set_music stop was not delivered");
        }
        Ok(())
    }
}

/// Yeelight backend implementing [`DeviceBackend`].
pub struct YeelightBackend {
    config: YeelightConfig,
    quota_backoff: Duration,
    adopted: StdRwLock<HashMap<DeviceId, AdoptedRoute>>,
    sessions: SessionTable<YeelightSession>,
}

impl YeelightBackend {
    #[must_use]
    pub fn new(config: YeelightConfig) -> Self {
        Self {
            config,
            quota_backoff: QUOTA_BACKOFF,
            adopted: StdRwLock::new(HashMap::new()),
            sessions: SessionTable::new(),
        }
    }

    /// Override the pause after a quota error.
    #[must_use]
    pub const fn with_quota_backoff(mut self, backoff: Duration) -> Self {
        self.quota_backoff = backoff;
        self
    }

    async fn open_session(
        &self,
        id: &DeviceId,
        route: &AdoptedRoute,
    ) -> Result<Session<YeelightSession>> {
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
        let (read_half, write_half) = control.into_split();
        let mut control = FramedWriter::new(write_half);
        let mut lines = BoundedLines::new(read_half, MAX_LINE_BYTES);
        let mut next_id = 0_u32;

        let mut music = None;
        if self.config.music_mode && capabilities.music {
            next_id += 1;
            match start_music(&mut control, &mut lines, next_id, local_ip, device_ip).await {
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
        let mut session = Session::new(
            *id,
            YeelightFacts {
                profile,
                info,
                cadence,
                mode,
                capabilities,
                music_enabled: self.config.music_mode,
            },
            YeelightSession {
                device_id: *id,
                mode,
                capabilities,
                shared: Arc::clone(&shared),
                control,
                music,
                next_id,
                brightness: u8::MAX,
                last_state: None,
                last_sent_at: None,
                next_allowed_at: None,
            },
        );
        session.spawn(read_control(*id, lines, shared, self.quota_backoff));
        Ok(session)
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
        Ok(mut replies) if !replies.found.is_empty() => Ok(replies.found.remove(0)),
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
    control: &mut FramedWriter<OwnedWriteHalf>,
    lines: &mut BoundedLines<OwnedReadHalf>,
    id: u32,
    local_ip: IpAddr,
    device_ip: IpAddr,
) -> Result<FramedWriter<OwnedWriteHalf>, MusicSetupError> {
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
    control
        .write(request.as_bytes(), write_deadline())
        .await
        .context("failed to send set_music")
        .map_err(ControlBroken)?;
    let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
    let reply = loop {
        let line = match lines.next(deadline).await {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Err(ControlBroken(anyhow!(
                    "the device closed the control connection"
                )));
            }
            Err(LineError::TimedOut) => {
                return Err(Unavailable(anyhow!("the device did not answer set_music")));
            }
            Err(error) => return Err(ControlBroken(error.into())),
        };
        let Ok(message) = decode_message(&line) else {
            continue;
        };
        if message.id == Some(id) {
            break message;
        }
    };
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
        return Ok(FramedWriter::new(write_half));
    }
}

/// Read replies and `props` pushes on the control connection. The device
/// may stay silent indefinitely, but a line it starts must finish. A quota
/// error pauses normal-mode output and invalidates the light's known
/// state; end of stream, a read error, or an oversized or stalled line
/// marks the session failed so the next write reports it and the host
/// reconnects.
async fn read_control(
    device_id: DeviceId,
    mut lines: BoundedLines<OwnedReadHalf>,
    shared: Arc<SessionShared>,
    quota_backoff: Duration,
) {
    let reason = loop {
        let line = match lines.next_after_idle(REPLY_TIMEOUT).await {
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
    warn!(%device_id, %reason, "Yeelight control connection lost");
    shared.mark_failed(format!("Yeelight {reason}"));
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
        Ok(self.sessions.facts(id).map(|facts| facts.info.clone()))
    }

    async fn connected_device_metadata(
        &self,
        id: &DeviceId,
    ) -> Result<Option<HashMap<String, String>>, DeviceError> {
        Ok(self.sessions.facts(id).map(|facts| {
            let mut metadata = profile_metadata(&facts.profile, facts.music_enabled);
            metadata.insert("stream_mode".to_owned(), facts.mode.as_str().to_owned());
            metadata.insert(
                metadata_key::MUSIC_MODE.to_owned(),
                match facts.mode {
                    StreamMode::Music => "on",
                    StreamMode::Normal if facts.capabilities.music && facts.music_enabled => {
                        "failed"
                    }
                    StreamMode::Normal if facts.capabilities.music => "off",
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
        self.sessions.ensure_accepting(id)?;
        if self.sessions.is_open(id) {
            return Ok(());
        }
        let route = self
            .adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;
        let session = self
            .open_session(id, &route)
            .await
            .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
        let facts = session.facts();
        self.sessions.install(session).await?;
        info!(
            device_id = %id,
            control = %facts.profile.reply.location,
            yeelight_id = %facts.profile.reply.id,
            model = ?facts.profile.reply.model,
            mode = facts.mode.as_str(),
            max_fps = facts.info.capabilities.max_fps,
            "Connected to Yeelight device"
        );
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from Yeelight device");
        Ok(())
    }

    async fn retire(&self) -> Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        self.sessions.require(id)?.admit().await?.brightness = brightness;
        Ok(())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.sessions
            .facts(id)
            .map(|facts| facts.cadence.target_fps())
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.sessions.facts(id).map(|facts| facts.cadence)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}
