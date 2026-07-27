//! MPRIS now-playing input source.
//!
//! Watches `org.mpris.MediaPlayer2.*` session-bus names, picks the player a
//! face should follow, and publishes [`MediaState`] snapshots through a
//! latest-value channel. Album art is resolved to a bounded JPEG data URL
//! once per track change.
//!
//! The bus interactions live behind [`PlayerSnapshot`] so the pick/gating
//! policy stays pure and testable without a D-Bus session.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::watch;
use tracing::{debug, info};

use hypercolor_types::media::MediaState;

#[cfg(target_os = "linux")]
use super::SourceSessionWriter;
use super::traits::{InputData, InputSource};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

/// Poll cadence for player discovery, status, and position.
pub const MEDIA_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Album art is downscaled so its longest edge fits this bound.
pub const MAX_ART_DIMENSION: u32 = 256;

/// JPEG quality for re-encoded album art.
pub const ART_JPEG_QUALITY: u8 = 80;

// ── Pure policy layer ──────────────────────────────────────────────────────

/// Playback status reported by an MPRIS player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackStatus {
    const fn rank(self) -> u8 {
        match self {
            Self::Playing => 0,
            Self::Paused => 1,
            Self::Stopped => 2,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn parse(value: &str) -> Self {
        match value {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// One player's state as read off the bus, in scan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub bus_name: String,
    pub status: PlaybackStatus,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Pick the player a face should follow: playing beats paused beats
/// stopped; within a tier the previously active player wins, then scan
/// order. Sticking with the previous player keeps faces from flapping
/// between two paused players.
#[must_use]
pub fn pick_active_player<'a>(
    players: &'a [PlayerSnapshot],
    previous: Option<&str>,
) -> Option<&'a PlayerSnapshot> {
    let best = players.iter().map(|p| p.status.rank()).min()?;
    let mut tier = players.iter().filter(|p| p.status.rank() == best);
    let first = tier.next()?;
    if let Some(previous) = previous
        && first.bus_name != previous
        && let Some(sticky) = players
            .iter()
            .find(|p| p.status.rank() == best && p.bus_name == previous)
    {
        return Some(sticky);
    }
    Some(first)
}

/// Caches resolved album art so the fetcher runs once per track change.
#[derive(Debug, Default)]
pub struct ArtCache {
    key: Option<String>,
    data_url: Option<String>,
}

impl ArtCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve art for the picked player, calling `fetch` only when the
    /// (player, artist, track, art URL) identity changes.
    pub fn resolve(
        &mut self,
        player: &PlayerSnapshot,
        fetch: impl FnOnce(&str) -> Option<String>,
    ) -> Option<String> {
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            player.bus_name,
            player.artist,
            player.track,
            player.art_url.as_deref().unwrap_or("")
        );
        if self.key.as_deref() == Some(key.as_str()) {
            return self.data_url.clone();
        }
        self.key = Some(key);
        self.data_url = player.art_url.as_deref().and_then(fetch);
        self.data_url.clone()
    }
}

/// Build the published state from the picked player and resolved art.
#[must_use]
pub fn media_state_from_player(
    player: Option<&PlayerSnapshot>,
    art_data_url: Option<String>,
) -> MediaState {
    let Some(player) = player else {
        return MediaState::unavailable();
    };

    MediaState {
        available: true,
        playing: player.status == PlaybackStatus::Playing,
        track: player.track.clone(),
        artist: player.artist.clone(),
        album: player.album.clone(),
        art_data_url,
        position_ms: player.position_ms,
        duration_ms: player.duration_ms,
        player: player.bus_name.clone(),
    }
}

#[derive(Clone)]
struct CompletedMediaPoll {
    state: Arc<MediaState>,
    completed_at: Instant,
}

/// Publication seam shared by media backends and the status-aware source.
///
/// Every completed poll advances the heartbeat even when its media payload is
/// unchanged. This keeps backend health independent from track-change dedup.
#[derive(Clone)]
pub struct MediaPollPublisher {
    state_tx: watch::Sender<Arc<MediaState>>,
    poll_tx: watch::Sender<Option<Arc<CompletedMediaPoll>>>,
}

impl MediaPollPublisher {
    /// Publish one successfully completed backend poll.
    pub fn publish_completed(&self, state: MediaState, completed_at: Instant) {
        let state = Arc::new(state);
        self.state_tx.send_if_modified(|current| {
            if current.as_ref() == state.as_ref() {
                return false;
            }
            *current = Arc::clone(&state);
            true
        });
        self.poll_tx.send_replace(Some(Arc::new(CompletedMediaPoll {
            state,
            completed_at,
        })));
    }

    fn reset(&self) {
        self.state_tx
            .send_replace(Arc::new(MediaState::unavailable()));
        self.poll_tx.send_replace(None);
    }
}

// ── Input source ───────────────────────────────────────────────────────────

/// Now-playing input source backed by the session-bus MPRIS poller.
///
/// On non-Linux platforms the source starts successfully but reports no
/// player, matching the other Linux-only inputs.
pub struct MediaSource {
    name: String,
    publisher: MediaPollPublisher,
    state_rx: watch::Receiver<Arc<MediaState>>,
    poll_rx: watch::Receiver<Option<Arc<CompletedMediaPoll>>>,
    last_poll: Option<Arc<CompletedMediaPoll>>,
    last_sampled: Option<Arc<MediaState>>,
    last_logged_track_key: Option<String>,
    running: bool,
    status: SourceStatusReporter,
    #[cfg(target_os = "linux")]
    poller: Option<linux::MediaPollerThread>,
}

impl MediaSource {
    #[must_use]
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(Arc::new(MediaState::unavailable()));
        let (poll_tx, poll_rx) = watch::channel(None);
        Self {
            name: "MPRIS Media".to_owned(),
            publisher: MediaPollPublisher { state_tx, poll_tx },
            state_rx,
            poll_rx,
            last_poll: None,
            last_sampled: None,
            last_logged_track_key: None,
            running: false,
            status: SourceStatusReporter::new(
                "media",
                SourceKind::Media,
                "mpris",
                true,
                true,
                true,
            ),
            #[cfg(target_os = "linux")]
            poller: None,
        }
    }

    /// Subscribe to latest-value media snapshots.
    #[must_use]
    pub fn receiver(&self) -> watch::Receiver<Arc<MediaState>> {
        self.state_rx.clone()
    }

    /// Clone the backend publication seam.
    #[must_use]
    pub fn publisher(&self) -> MediaPollPublisher {
        self.publisher.clone()
    }
}

impl Default for MediaSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for MediaSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }

        self.publisher.reset();
        self.last_poll = None;
        self.last_sampled = None;
        let status = self.status.begin_session()?;

        #[cfg(target_os = "linux")]
        {
            self.poller = match linux::MediaPollerThread::spawn(self.publisher.clone(), status) {
                Ok(poller) => Some(poller),
                Err(error) => {
                    if let Some(status) = self.status.session() {
                        status.failed(SourceIssue::new(
                            "media_poller_start_failed",
                            error.to_string(),
                            true,
                        ));
                    }
                    return Err(error);
                }
            };
            self.running = true;
            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = &self.publisher;
            info!("media input source is unavailable on this platform");
            self.running = true;
            if let Some(status) = status {
                status.unavailable(
                    SourceIssue::new(
                        "media_backend_unsupported",
                        "MPRIS media input is unavailable on this platform",
                        false,
                    )
                    .with_remediation("run Hypercolor on Linux to use MPRIS media input"),
                );
            }
            Ok(())
        }
    }

    fn stop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(poller) = self.poller.take() {
            poller.stop();
        }
        self.running = false;
        self.last_poll = None;
        self.last_sampled = None;
        self.status.stop();
    }

    fn sample(&mut self) -> Result<InputData> {
        let Some(poll) = self.poll_rx.borrow().clone() else {
            return Ok(InputData::None);
        };
        if self
            .last_poll
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, &poll))
        {
            return Ok(InputData::None);
        }
        self.last_poll = Some(Arc::clone(&poll));

        if let Some(status) = self.status.session() {
            status.record_sample(
                poll.completed_at,
                poll.completed_at + MEDIA_POLL_INTERVAL + MEDIA_POLL_INTERVAL,
                usize::from(poll.state.available),
            )?;
        }

        let latest = Arc::clone(&poll.state);
        if self.last_sampled.as_ref() == Some(&latest) {
            return Ok(InputData::None);
        }

        let track_key = latest.available.then(|| latest.track_key());
        if track_key != self.last_logged_track_key {
            if track_key.is_some() {
                info!(
                    player = %latest.player,
                    track = %latest.track,
                    artist = %latest.artist,
                    playing = latest.playing,
                    "Media track changed"
                );
            } else {
                debug!("Media player went away");
            }
            self.last_logged_track_key = track_key;
        }

        self.last_sampled = Some(Arc::clone(&latest));
        Ok(InputData::Media(latest))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

// ── Linux poller ───────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod linux {
    use std::io::Cursor;
    use std::sync::Arc;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use base64::Engine as _;
    use tracing::{debug, warn};
    use zbus::zvariant::OwnedValue;

    use hypercolor_types::media::MediaState;

    use super::{
        ART_JPEG_QUALITY, ArtCache, MAX_ART_DIMENSION, MEDIA_POLL_INTERVAL, MediaPollPublisher,
        PlaybackStatus, PlayerSnapshot, SourceIssue, SourceSessionWriter, media_state_from_player,
        pick_active_player,
    };

    const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
    const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
    const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
    const ART_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
    const MAX_ART_SOURCE_BYTES: usize = 8 * 1024 * 1024;

    pub(super) struct MediaPollerThread {
        stop_tx: Sender<()>,
        join_handle: JoinHandle<()>,
    }

    impl MediaPollerThread {
        pub(super) fn spawn(
            publisher: MediaPollPublisher,
            status: Option<SourceSessionWriter>,
        ) -> Result<Self> {
            let (stop_tx, stop_rx) = mpsc::channel();
            let join_handle = std::thread::Builder::new()
                .name("hypercolor-media".to_owned())
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            warn!(%error, "media poller could not build a runtime");
                            if let Some(status) = &status {
                                status.failed(SourceIssue::new(
                                    "media_runtime_start_failed",
                                    error.to_string(),
                                    true,
                                ));
                            }
                            return;
                        }
                    };

                    let connection = runtime.block_on(zbus::Connection::session());
                    let connection = match connection {
                        Ok(connection) => connection,
                        Err(error) => {
                            debug!(%error, "media poller has no session bus; staying idle");
                            if let Some(status) = &status {
                                status.unavailable(
                                    SourceIssue::new(
                                        "media_session_bus_unavailable",
                                        error.to_string(),
                                        true,
                                    )
                                    .with_remediation(
                                        "run Hypercolor inside a desktop login session with D-Bus",
                                    ),
                                );
                            }
                            return;
                        }
                    };

                    let mut art_cache = ArtCache::new();
                    let mut active_player: Option<String> = None;
                    loop {
                        let players = match runtime.block_on(poll_players(&connection)) {
                            Ok(players) => players,
                            Err(error) => {
                                if let Some(status) = &status {
                                    status.degraded(SourceIssue::new(
                                        "media_poll_failed",
                                        error.to_string(),
                                        true,
                                    ));
                                }
                                match stop_rx.recv_timeout(MEDIA_POLL_INTERVAL) {
                                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                                    Err(RecvTimeoutError::Timeout) => continue,
                                }
                            }
                        };
                        let picked = pick_active_player(&players, active_player.as_deref());
                        active_player = picked.map(|player| player.bus_name.clone());
                        let art = picked.and_then(|player| {
                            art_cache
                                .resolve(player, |url| runtime.block_on(fetch_art_data_url(url)))
                        });
                        let state = media_state_from_player(picked, art);
                        publisher.publish_completed(state, std::time::Instant::now());

                        match stop_rx.recv_timeout(MEDIA_POLL_INTERVAL) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                    }
                })
                .context("failed to spawn media poller thread")?;

            Ok(Self {
                stop_tx,
                join_handle,
            })
        }

        pub(super) fn stop(self) {
            let _ = self.stop_tx.send(());
            if let Err(error) = self.join_handle.join() {
                debug!("media poller thread join failed: {error:?}");
            }
        }
    }

    async fn poll_players(connection: &zbus::Connection) -> Result<Vec<PlayerSnapshot>> {
        let dbus = zbus::fdo::DBusProxy::new(connection)
            .await
            .context("failed to create D-Bus proxy")?;
        let names = dbus
            .list_names()
            .await
            .context("failed to list D-Bus names")?;

        let mut players = Vec::new();
        for name in names {
            let name = name.as_str();
            if !name.starts_with(MPRIS_PREFIX) {
                continue;
            }
            if let Some(snapshot) = snapshot_player(connection, name).await {
                players.push(snapshot);
            }
        }
        Ok(players)
    }

    async fn snapshot_player(
        connection: &zbus::Connection,
        bus_name: &str,
    ) -> Option<PlayerSnapshot> {
        let proxy = zbus::Proxy::new(connection, bus_name, MPRIS_PATH, PLAYER_INTERFACE)
            .await
            .ok()?;

        let status: String = proxy.get_property("PlaybackStatus").await.ok()?;
        let metadata: std::collections::HashMap<String, OwnedValue> =
            proxy.get_property("Metadata").await.unwrap_or_default();
        // Position is not signaled by MPRIS; players that don't implement it
        // report zero rather than dropping the whole snapshot.
        let position_us: i64 = proxy.get_property("Position").await.unwrap_or(0);

        Some(PlayerSnapshot {
            bus_name: bus_name.to_owned(),
            status: PlaybackStatus::parse(&status),
            track: metadata_string(&metadata, "xesam:title"),
            artist: metadata_string_list_head(&metadata, "xesam:artist"),
            album: metadata_string(&metadata, "xesam:album"),
            art_url: {
                let url = metadata_string(&metadata, "mpris:artUrl");
                (!url.is_empty()).then_some(url)
            },
            position_ms: position_us.max(0).unsigned_abs() / 1_000,
            duration_ms: metadata_length_us(&metadata) / 1_000,
        })
    }

    fn metadata_string(
        metadata: &std::collections::HashMap<String, OwnedValue>,
        key: &str,
    ) -> String {
        metadata
            .get(key)
            .and_then(|value| <&str>::try_from(value).ok())
            .unwrap_or_default()
            .to_owned()
    }

    fn metadata_string_list_head(
        metadata: &std::collections::HashMap<String, OwnedValue>,
        key: &str,
    ) -> String {
        metadata
            .get(key)
            .and_then(|value| Vec::<String>::try_from(value.try_clone().ok()?).ok())
            .and_then(|artists| artists.into_iter().next())
            .unwrap_or_default()
    }

    fn metadata_length_us(metadata: &std::collections::HashMap<String, OwnedValue>) -> u64 {
        let value = metadata.get("mpris:length");
        let Some(value) = value else { return 0 };
        i64::try_from(value)
            .map(|us| us.max(0).unsigned_abs())
            .or_else(|_| u64::try_from(value))
            .unwrap_or(0)
    }

    async fn fetch_art_data_url(url: &str) -> Option<String> {
        let bytes = if let Some(path) = url.strip_prefix("file://") {
            let decoded = percent_decode(path);
            std::fs::read(decoded).ok()?
        } else if url.starts_with("http://") || url.starts_with("https://") {
            let client = reqwest::Client::builder()
                .timeout(ART_FETCH_TIMEOUT)
                .build()
                .ok()?;
            let response = client.get(url).send().await.ok()?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_ART_SOURCE_BYTES as u64)
            {
                return None;
            }
            let bytes = response.bytes().await.ok()?;
            bytes.to_vec()
        } else {
            return None;
        };

        if bytes.is_empty() || bytes.len() > MAX_ART_SOURCE_BYTES {
            return None;
        }
        encode_art_jpeg(&bytes)
    }

    fn encode_art_jpeg(bytes: &[u8]) -> Option<String> {
        let image = image::load_from_memory(bytes).ok()?;
        let image = if image.width() > MAX_ART_DIMENSION || image.height() > MAX_ART_DIMENSION {
            image.resize(
                MAX_ART_DIMENSION,
                MAX_ART_DIMENSION,
                image::imageops::FilterType::Triangle,
            )
        } else {
            image
        };

        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), {
            ART_JPEG_QUALITY
        });
        image.into_rgb8().write_with_encoder(encoder).ok()?;

        Some(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ))
    }

    /// Minimal percent-decoding for `file://` art paths (spaces and
    /// non-ASCII are common in music libraries).
    fn percent_decode(path: &str) -> std::path::PathBuf {
        let bytes = path.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%'
                && index + 2 < bytes.len()
                && let (Some(high), Some(low)) = (
                    char::from(bytes[index + 1]).to_digit(16),
                    char::from(bytes[index + 2]).to_digit(16),
                )
            {
                decoded.push(u8::try_from(high * 16 + low).unwrap_or(b'%'));
                index += 3;
                continue;
            }
            decoded.push(bytes[index]);
            index += 1;
        }
        std::path::PathBuf::from(String::from_utf8_lossy(&decoded).into_owned())
    }
}
