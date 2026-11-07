//! Twinkly output backend: login, real-time sessions, the frame sink, and
//! session upkeep (Spec 92 §8 and §10.5).

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome,
    DiscoveredDevice, OutputCadence,
};
use hypercolor_driver_support::{DeviceSession, FrameDelivery, Session, SessionSink, SessionTable};
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
/// Longest a disconnect waits to put the device back in its prior mode.
pub const RESTORE_TIMEOUT: Duration = Duration::from_secs(2);
/// Metadata key reporting whether Hypercolor holds the lights.
pub const CONTROL_STATE_KEY: &str = "control_state";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);
/// Longest single pause, whatever a policy asks for; keeps deadline
/// arithmetic far from overflow.
const MAX_PAUSE: Duration = Duration::from_hours(24);

/// How the session steps back when another client takes the device's
/// single login (Spec 92 §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentionPolicy {
    /// Pause after the first loss.
    pub initial_backoff: Duration,
    /// Longest pause; each loss soon after a resume doubles the pause up
    /// to this.
    pub max_backoff: Duration,
    /// Holding the login this long without a loss counts as a quiet period
    /// and resets the pause to `initial_backoff`.
    pub quiet_period: Duration,
}

impl Default for ContentionPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_secs(30),
            max_backoff: Duration::from_mins(10),
            quiet_period: Duration::from_mins(10),
        }
    }
}

/// What a connect learned, plus the live control state for metadata.
struct TwinklyFacts {
    profile: TwinklyDeviceProfile,
    info: DeviceInfo,
    layout_source: LayoutSource,
    cadence: OutputCadence,
    shared: Arc<SessionShared>,
}

impl TwinklyFacts {
    fn metadata(&self) -> HashMap<String, String> {
        let mut metadata = profile_metadata(&self.profile);
        metadata.insert(
            metadata_key::LAYOUT_SOURCE.to_owned(),
            self.layout_source.as_str().to_owned(),
        );
        metadata.insert(
            CONTROL_STATE_KEY.to_owned(),
            if self.shared.yielded.load(Ordering::Acquire) {
                "yielded_to_another_app"
            } else {
                "streaming"
            }
            .to_owned(),
        );
        metadata
    }
}

/// State the frame path and the upkeep task both read.
struct SessionShared {
    /// Another client keeps taking the login; frames are declined until
    /// the upkeep takes the lights back.
    yielded: AtomicBool,
    token: StdMutex<AuthToken>,
    failure: StdMutex<Option<String>>,
}

impl SessionShared {
    fn token(&self) -> AuthToken {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

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

/// One open real-time session.
struct TwinklySession {
    device_id: DeviceId,
    client: TwinklyClient,
    /// Mode the device was in before connect, restored on close.
    previous_mode: Option<String>,
    shared: Arc<SessionShared>,
    socket: UdpSocket,
    encoder: RtFrameEncoder,
    brightness: u8,
    scaled: Vec<[u8; 3]>,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl DeviceSession for TwinklySession {
    type Facts = TwinklyFacts;

    const CLEANUP: Duration = RESTORE_TIMEOUT.saturating_add(Duration::from_millis(100));

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if let Some(reason) = self.shared.failure() {
            return Err(DeviceError::write(self.device_id, reason));
        }
        if self.shared.yielded.load(Ordering::Acquire) {
            // Another app holds the login: frames would carry a dead token.
            return Ok(DeviceWriteOutcome::Declined);
        }

        let led_count = self.encoder.led_count();
        if colors.len() != led_count {
            let should_warn = self
                .last_size_mismatch_warn_at
                .is_none_or(|warned_at| warned_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %self.device_id,
                    expected_led_count = led_count,
                    actual_led_count = colors.len(),
                    "Twinkly frame size mismatch; padding or truncating to the LED count"
                );
                self.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let datagrams = if self.brightness == u8::MAX {
            self.encoder.encode(colors)
        } else {
            let factor = f32::from(self.brightness) / f32::from(u8::MAX);
            self.scaled.clear();
            self.scaled.extend(colors.iter().map(|&[r, g, b]| {
                let scaled = Rgb::new(r, g, b).scale(factor);
                [scaled.r, scaled.g, scaled.b]
            }));
            self.encoder.encode(&self.scaled)
        };

        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        for datagram in datagrams {
            self.socket.send(datagram).await.map_err(|error| {
                DeviceError::write(
                    self.device_id,
                    format!("Twinkly datagram send failed: {error}"),
                )
            })?;
        }
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Put the device back in the mode it had before connect, bounded by
    /// [`RESTORE_TIMEOUT`]. Skipped when another app holds the lights or the
    /// session already failed.
    async fn finish(self) -> Result<(), DeviceError> {
        let Some(mode) = self.previous_mode.as_deref() else {
            return Ok(());
        };
        if self.shared.yielded.load(Ordering::Acquire) || self.shared.failure().is_some() {
            return Ok(());
        }
        let device_id = self.device_id;
        let token = self.shared.token();
        match tokio::time::timeout(RESTORE_TIMEOUT, self.client.set_mode(&token, mode)).await {
            Ok(Ok(())) => debug!(%device_id, mode, "Restored the Twinkly LED mode"),
            Ok(Err(error)) => {
                debug!(%device_id, mode, error = %error, "Twinkly LED mode not restored");
            }
            Err(_) => debug!(%device_id, mode, "Twinkly LED mode restore timed out"),
        }
        Ok(())
    }
}

/// Twinkly backend implementing [`DeviceBackend`].
pub struct TwinklyBackend {
    config: TwinklyConfig,
    request_timeout: Duration,
    upkeep_interval: Duration,
    contention: ContentionPolicy,
    adopted: StdRwLock<HashMap<DeviceId, TwinklyEndpoint>>,
    sessions: SessionTable<TwinklySession>,
}

impl TwinklyBackend {
    #[must_use]
    pub fn new(config: TwinklyConfig) -> Self {
        Self {
            config,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            upkeep_interval: DEFAULT_UPKEEP_INTERVAL,
            contention: ContentionPolicy::default(),
            adopted: StdRwLock::new(HashMap::new()),
            sessions: SessionTable::new(),
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

    /// Override how the session reacts to another client taking the login.
    #[must_use]
    pub const fn with_contention_policy(mut self, policy: ContentionPolicy) -> Self {
        self.contention = policy;
        self
    }

    async fn open_session(
        &self,
        id: &DeviceId,
        endpoint: TwinklyEndpoint,
    ) -> Result<Session<TwinklySession>> {
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
        let previous_mode = match client.mode(&token).await {
            Ok(reply) if reply.mode != RT_MODE => Some(reply.mode),
            Ok(_) => None,
            Err(error) => {
                debug!(device_id = %id, error = %error, "Twinkly LED mode unreadable; nothing to restore");
                None
            }
        };
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
            yielded: AtomicBool::new(false),
            token: StdMutex::new(token.clone()),
            failure: StdMutex::new(None),
        });
        let mut session = Session::new(
            *id,
            TwinklyFacts {
                profile,
                info,
                layout_source,
                cadence,
                shared: Arc::clone(&shared),
            },
            TwinklySession {
                device_id: *id,
                client: client.clone(),
                previous_mode,
                shared: Arc::clone(&shared),
                socket,
                encoder,
                brightness: u8::MAX,
                scaled: Vec::with_capacity(led_count),
                last_size_mismatch_warn_at: None,
            },
        );
        let upkeep = run_upkeep(
            *id,
            client,
            token,
            shared,
            session.sink(),
            self.upkeep_interval,
            self.contention,
        );
        session.spawn(upkeep);
        Ok(session)
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

/// Who holds the device's login, as the upkeep sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    /// Hypercolor holds the login, since `held_since`.
    Owning { held_since: Instant },
    /// Another client took it; try once more at `resume_at`.
    Yielded { resume_at: Instant },
}

/// The ownership state machine (Spec 92 §5.4).
///
/// The first loss of the login to someone else yields at once: no re-login,
/// frames declined, `yielded_to_another_app` reported. After the pause the
/// session tries one resume, a single transition that succeeds only when
/// both the login and the switch to real-time mode do; anything else
/// re-arms the pause. A loss soon after a resume doubles the next pause;
/// holding the login for the quiet period resets it.
struct OwnershipState {
    policy: ContentionPolicy,
    ownership: Ownership,
    backoff: Duration,
}

impl OwnershipState {
    fn new(policy: ContentionPolicy, now: Instant) -> Self {
        Self {
            backoff: policy.initial_backoff.min(MAX_PAUSE),
            policy,
            ownership: Ownership::Owning { held_since: now },
        }
    }

    /// Another client holds the login: pause, and lengthen the next pause.
    fn lose(&mut self, device_id: &DeviceId, shared: &SessionShared, now: Instant) {
        let pause = self.backoff;
        shared.yielded.store(true, Ordering::Release);
        self.ownership = Ownership::Yielded {
            resume_at: now.checked_add(pause).unwrap_or(now),
        };
        self.backoff = self
            .backoff
            .saturating_mul(2)
            .min(self.policy.max_backoff.min(MAX_PAUSE));
        warn!(
            %device_id,
            pause_secs = pause.as_secs(),
            "Another app took control of the Twinkly lights; Hypercolor is pausing instead of fighting for the login"
        );
    }

    /// A resume attempt failed for a reason other than another login: try
    /// again after the same pause.
    fn rearm(&mut self, now: Instant) {
        let pause = self.backoff;
        self.ownership = Ownership::Yielded {
            resume_at: now.checked_add(pause).unwrap_or(now),
        };
    }

    /// The login and real-time mode are ours again.
    fn resumed(&mut self, shared: &SessionShared, now: Instant) {
        shared.yielded.store(false, Ordering::Release);
        self.ownership = Ownership::Owning { held_since: now };
    }
}

/// Keep a session streamable: put the device back in real-time mode when
/// something switched it away, refresh the token before it expires, and
/// step back the moment another client takes the login (Spec 92 §5.3 and
/// §5.4).
async fn run_upkeep(
    device_id: DeviceId,
    client: TwinklyClient,
    mut token: AuthToken,
    shared: Arc<SessionShared>,
    sink: SessionSink<TwinklySession>,
    interval: Duration,
    policy: ContentionPolicy,
) {
    let mut obtained_at = Instant::now();
    let mut failures = 0_u32;
    let mut state = OwnershipState::new(policy, obtained_at);
    loop {
        tokio::time::sleep(interval).await;
        let now = Instant::now();
        let outcome: Result<()> = match state.ownership {
            Ownership::Yielded { resume_at } if now < resume_at => continue,
            Ownership::Yielded { .. } => {
                info!(%device_id, "Taking the Twinkly lights back after a pause");
                match resume(&client, &shared, &sink).await {
                    Ok(fresh) => {
                        token = fresh;
                        obtained_at = Instant::now();
                        state.resumed(&shared, obtained_at);
                        info!(%device_id, "Hypercolor holds the Twinkly lights again");
                        Ok(())
                    }
                    Err(error) if is_unauthorized(&error) => {
                        state.lose(&device_id, &shared, now);
                        Ok(())
                    }
                    Err(error) => {
                        state.rearm(now);
                        Err(error)
                    }
                }
            }
            Ownership::Owning { held_since } => {
                if now.saturating_duration_since(held_since) >= policy.quiet_period {
                    state.backoff = policy.initial_backoff.min(MAX_PAUSE);
                }
                // Check ownership before anything else, including a refresh:
                // a login now would sign out whoever took the device.
                match check_mode(&client, &token).await {
                    Err(error) if is_unauthorized(&error) => {
                        state.lose(&device_id, &shared, now);
                        Ok(())
                    }
                    Err(error) => Err(error),
                    Ok(()) => {
                        let refresh_due = token.expires_in.is_some_and(|lifetime| {
                            obtained_at.elapsed() + TOKEN_REFRESH_MARGIN.min(lifetime / 2)
                                >= lifetime
                        });
                        if refresh_due {
                            debug!(%device_id, "Twinkly token near expiry; logging in again");
                            match resume(&client, &shared, &sink).await {
                                Ok(fresh) => {
                                    token = fresh;
                                    obtained_at = Instant::now();
                                    Ok(())
                                }
                                Err(error) if is_unauthorized(&error) => {
                                    state.lose(&device_id, &shared, now);
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            }
                        } else {
                            Ok(())
                        }
                    }
                }
            }
        };
        match outcome {
            Ok(()) => failures = 0,
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

fn is_unauthorized(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(ApiError::is_unauthorized)
}

async fn check_mode(client: &TwinklyClient, token: &AuthToken) -> Result<()> {
    let mode = client.mode(token).await?;
    if mode.mode != RT_MODE {
        info!(endpoint = %client.endpoint(), mode = %mode.mode, "Twinkly left real-time mode; switching back");
        client.set_mode(token, RT_MODE).await?;
    }
    Ok(())
}

/// Log in, hand the token to the frame encoder, and switch to real-time
/// mode. Both steps must succeed for the token to count as held.
async fn resume(
    client: &TwinklyClient,
    shared: &SessionShared,
    sink: &SessionSink<TwinklySession>,
) -> Result<AuthToken> {
    let token = client.login().await?;
    sink.admit().await?.encoder.set_token(token.bytes);
    token.clone_into(&mut shared.token.lock().unwrap_or_else(PoisonError::into_inner));
    client.set_mode(&token, RT_MODE).await?;
    Ok(token)
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
        Ok(self.sessions.facts(id).map(|facts| facts.info.clone()))
    }

    async fn connected_device_metadata(
        &self,
        id: &DeviceId,
    ) -> Result<Option<HashMap<String, String>>, DeviceError> {
        Ok(self.sessions.facts(id).map(|facts| facts.metadata()))
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
        let endpoint = self
            .adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .copied()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;

        let session = self
            .open_session(id, endpoint)
            .await
            .map_err(|error| DeviceError::connection(id, format!("{error:#}")))?;
        let facts = session.facts();
        self.sessions.install(session).await?;
        info!(
            device_id = %id,
            %endpoint,
            mac = %facts.profile.mac,
            leds = facts.info.total_led_count(),
            profile = %facts.profile.led_profile,
            rt_protocol = %facts.profile.rt_protocol(),
            layout = facts.layout_source.as_str(),
            max_fps = facts.info.capabilities.max_fps,
            "Connected to Twinkly device"
        );
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from Twinkly device");
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
