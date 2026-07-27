//! Cross-platform now-playing input.
//!
//! Linux reads MPRIS over the session bus and Windows reads Global System
//! Media Transport Controls (GSMTC). Metadata polling and artwork enrichment
//! are independent: a slow or hostile image can never delay the next player
//! snapshot.

use std::io::{Cursor, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use tokio::sync::watch;
use tracing::{debug, info};

use hypercolor_types::media::MediaState;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::SourceSessionWriter;
use super::traits::{InputData, InputSource};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

/// Poll cadence for player discovery, status, and position.
pub const MEDIA_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum time allowed for one provider connection or metadata poll.
pub const MEDIA_PROVIDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum time allowed for artwork I/O and decode work.
pub const ART_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum compressed artwork bytes accepted from any source.
pub const MAX_ART_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// Maximum source width or height accepted before decode allocation.
pub const MAX_ART_SOURCE_DIMENSION: u32 = 8_192;

/// Maximum decoded source pixels accepted before decode allocation.
pub const MAX_ART_SOURCE_PIXELS: u64 = 32 * 1024 * 1024;

/// Maximum aggregate decoder allocation.
pub const MAX_ART_DECODE_BYTES: u64 = 128 * 1024 * 1024;

/// Album art is downscaled so its longest edge fits this bound.
pub const MAX_ART_DIMENSION: u32 = 256;

/// Maximum encoded data URL published into frame input.
pub const MAX_ART_DATA_URL_BYTES: usize = 512 * 1024;

/// Maximum redirects followed while fetching remote artwork.
pub const MAX_ART_REDIRECTS: usize = 3;

/// JPEG quality for re-encoded album art.
pub const ART_JPEG_QUALITY: u8 = 80;

#[cfg(any(target_os = "linux", target_os = "windows"))]
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(any(target_os = "linux", target_os = "windows"))]
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// Playback status reported by a platform media provider.
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

    #[cfg(target_os = "linux")]
    fn from_mpris(value: &str) -> Self {
        match value {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// Deferred artwork identity returned with immediate metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkSource {
    /// A bounded `file:`, `http:`, or `https:` resource.
    Url(String),
    /// The current GSMTC thumbnail for a Windows media session.
    WindowsSession(String),
}

impl ArtworkSource {
    fn cache_key(&self) -> &str {
        match self {
            Self::Url(value) | Self::WindowsSession(value) => value,
        }
    }
}

/// One player's state as read from a native provider, in scan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub bus_name: String,
    pub status: PlaybackStatus,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub artwork: Option<ArtworkSource>,
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Pick the player a face should follow.
///
/// Playing beats paused beats stopped. Within a tier the previously active
/// player wins, then scan order. This avoids flapping between paused players.
#[must_use]
pub fn pick_active_player<'a>(
    players: &'a [PlayerSnapshot],
    previous: Option<&str>,
) -> Option<&'a PlayerSnapshot> {
    let best = players.iter().map(|player| player.status.rank()).min()?;
    let mut tier = players.iter().filter(|player| player.status.rank() == best);
    let first = tier.next()?;
    if let Some(previous) = previous
        && first.bus_name != previous
        && let Some(sticky) = players
            .iter()
            .find(|player| player.status.rank() == best && player.bus_name == previous)
    {
        return Some(sticky);
    }
    Some(first)
}

fn artwork_key(player: &PlayerSnapshot) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        player.bus_name,
        player.artist,
        player.album,
        player.track,
        player.duration_ms,
        player.artwork.as_ref().map_or("", ArtworkSource::cache_key)
    )
}

/// Caches resolved album art so a resolver runs once per track identity.
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

    /// Resolve art for a player only when its track or artwork identity changes.
    pub fn resolve(
        &mut self,
        player: &PlayerSnapshot,
        fetch: impl FnOnce(&ArtworkSource) -> Option<String>,
    ) -> Option<String> {
        let key = artwork_key(player);
        if self.key.as_deref() == Some(key.as_str()) {
            return self.data_url.clone();
        }
        self.key = Some(key);
        self.data_url = player.artwork.as_ref().and_then(fetch);
        self.data_url.clone()
    }
}

/// Build the published state from a selected player and optional enrichment.
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

/// Error returned by a native metadata provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct MediaProviderError {
    message: String,
}

impl MediaProviderError {
    /// Wrap a platform error without exposing platform-specific types.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Native metadata provider contract.
///
/// Providers reconnect explicitly. Any failed poll invalidates the current
/// connection so the next call re-enters `connect` rather than remaining
/// attached to a dead bus or manager.
#[async_trait(?Send)]
pub trait MediaMetadataProvider: Send {
    fn backend_name(&self) -> &'static str;
    async fn connect(&mut self) -> std::result::Result<(), MediaProviderError>;
    async fn poll_players(
        &mut self,
    ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError>;
    fn disconnect(&mut self);
}

/// Whether a provider failed while connecting or while polling a connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaProviderFailure {
    #[error("media provider connection failed: {0}")]
    Connect(MediaProviderError),
    #[error("media provider poll failed: {0}")]
    Poll(MediaProviderError),
}

/// Immediate metadata result plus a deferred artwork request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaMetadataPoll {
    pub state: MediaState,
    pub artwork: Option<ArtworkRequest>,
}

/// One deduplicated artwork enrichment request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkRequest {
    pub key: String,
    pub source: ArtworkSource,
}

/// Reconnecting provider state machine, exposed for deterministic tests.
pub struct MediaProviderSession {
    provider: Box<dyn MediaMetadataProvider>,
    connected: bool,
    active_player: Option<String>,
}

impl MediaProviderSession {
    #[must_use]
    pub fn new(provider: Box<dyn MediaMetadataProvider>) -> Self {
        Self {
            provider,
            connected: false,
            active_player: None,
        }
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.provider.backend_name()
    }

    /// Connect if needed and obtain one metadata-only snapshot.
    pub async fn poll(&mut self) -> std::result::Result<MediaMetadataPoll, MediaProviderFailure> {
        if !self.connected {
            if let Err(error) = self.provider.connect().await {
                self.provider.disconnect();
                return Err(MediaProviderFailure::Connect(error));
            }
            self.connected = true;
        }

        let players = match self.provider.poll_players().await {
            Ok(players) => players,
            Err(error) => {
                self.provider.disconnect();
                self.connected = false;
                return Err(MediaProviderFailure::Poll(error));
            }
        };
        let picked = pick_active_player(&players, self.active_player.as_deref());
        self.active_player = picked.map(|player| player.bus_name.clone());
        let artwork = picked.and_then(|player| {
            player.artwork.clone().map(|source| ArtworkRequest {
                key: artwork_key(player),
                source,
            })
        });
        Ok(MediaMetadataPoll {
            state: media_state_from_player(picked, None),
            artwork,
        })
    }

    fn disconnect(&mut self) {
        self.provider.disconnect();
        self.connected = false;
    }
}

/// Resource policy enforced before every artwork allocation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtworkPolicy {
    pub fetch_timeout: Duration,
    pub max_source_bytes: usize,
    pub max_source_dimension: u32,
    pub max_source_pixels: u64,
    pub max_decode_bytes: u64,
    pub max_output_dimension: u32,
    pub max_data_url_bytes: usize,
    pub max_redirects: usize,
}

impl Default for ArtworkPolicy {
    fn default() -> Self {
        Self {
            fetch_timeout: ART_FETCH_TIMEOUT,
            max_source_bytes: MAX_ART_SOURCE_BYTES,
            max_source_dimension: MAX_ART_SOURCE_DIMENSION,
            max_source_pixels: MAX_ART_SOURCE_PIXELS,
            max_decode_bytes: MAX_ART_DECODE_BYTES,
            max_output_dimension: MAX_ART_DIMENSION,
            max_data_url_bytes: MAX_ART_DATA_URL_BYTES,
            max_redirects: MAX_ART_REDIRECTS,
        }
    }
}

/// Artwork loading or decoding failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArtworkError {
    #[error("artwork policy exceeds the hard safety envelope")]
    InvalidPolicy,
    #[error("unsupported artwork source")]
    UnsupportedSource,
    #[error("artwork operation timed out")]
    Timeout,
    #[error("artwork source exceeds {limit} bytes")]
    SourceTooLarge { limit: usize },
    #[error("artwork dimensions {width}x{height} exceed policy")]
    DimensionsTooLarge { width: u32, height: u32 },
    #[error("artwork data URL exceeds {limit} bytes")]
    OutputTooLarge { limit: usize },
    #[error("artwork I/O failed: {0}")]
    Io(String),
    #[error("artwork decode failed: {0}")]
    Decode(String),
}

/// Bounded artwork fetcher shared by both native providers.
#[derive(Clone)]
pub struct ArtworkFetcher {
    client: reqwest::Client,
    policy: ArtworkPolicy,
}

impl ArtworkFetcher {
    /// Build a fetcher with explicit redirect, time, and size policy.
    pub fn new(policy: ArtworkPolicy) -> std::result::Result<Self, ArtworkError> {
        if policy.fetch_timeout.is_zero()
            || policy.fetch_timeout > ART_FETCH_TIMEOUT
            || policy.max_source_bytes == 0
            || policy.max_source_bytes > MAX_ART_SOURCE_BYTES
            || policy.max_source_dimension == 0
            || policy.max_source_dimension > MAX_ART_SOURCE_DIMENSION
            || policy.max_source_pixels == 0
            || policy.max_source_pixels > MAX_ART_SOURCE_PIXELS
            || policy.max_decode_bytes == 0
            || policy.max_decode_bytes > MAX_ART_DECODE_BYTES
            || policy.max_output_dimension == 0
            || policy.max_output_dimension > MAX_ART_DIMENSION
            || policy.max_data_url_bytes == 0
            || policy.max_data_url_bytes > MAX_ART_DATA_URL_BYTES
            || policy.max_redirects > MAX_ART_REDIRECTS
        {
            return Err(ArtworkError::InvalidPolicy);
        }
        let client = reqwest::Client::builder()
            .timeout(policy.fetch_timeout)
            .connect_timeout(policy.fetch_timeout)
            .redirect(reqwest::redirect::Policy::limited(policy.max_redirects))
            .build()
            .map_err(|error| ArtworkError::Io(error.to_string()))?;
        Ok(Self { client, policy })
    }

    #[must_use]
    pub const fn policy(&self) -> ArtworkPolicy {
        self.policy
    }

    /// Load, decode, resize, and encode one source under the configured limits.
    pub async fn fetch_data_url(
        &self,
        source: &ArtworkSource,
    ) -> std::result::Result<String, ArtworkError> {
        let load = async {
            let bytes = match source {
                ArtworkSource::Url(url) => self.load_url(url).await?,
                ArtworkSource::WindowsSession(session) => {
                    #[cfg(target_os = "windows")]
                    {
                        windows::fetch_thumbnail_bytes(session, self.policy.max_source_bytes)
                            .await?
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = session;
                        return Err(ArtworkError::UnsupportedSource);
                    }
                }
            };
            let policy = self.policy;
            tokio::task::spawn_blocking(move || encode_art_jpeg(&bytes, policy))
                .await
                .map_err(|error| ArtworkError::Decode(error.to_string()))?
        };

        tokio::time::timeout(self.policy.fetch_timeout, load)
            .await
            .map_err(|_| ArtworkError::Timeout)?
    }

    async fn load_url(&self, value: &str) -> std::result::Result<Vec<u8>, ArtworkError> {
        let url = url::Url::parse(value).map_err(|_| ArtworkError::UnsupportedSource)?;
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| ArtworkError::UnsupportedSource)?;
                let limit = self.policy.max_source_bytes;
                tokio::task::spawn_blocking(move || read_bounded_file(&path, limit))
                    .await
                    .map_err(|error| ArtworkError::Io(error.to_string()))?
            }
            "http" | "https" => self.load_http(url).await,
            _ => Err(ArtworkError::UnsupportedSource),
        }
    }

    async fn load_http(&self, url: url::Url) -> std::result::Result<Vec<u8>, ArtworkError> {
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(reqwest_artwork_error)?;
        if response
            .content_length()
            .is_some_and(|length| length > self.policy.max_source_bytes as u64)
        {
            return Err(ArtworkError::SourceTooLarge {
                limit: self.policy.max_source_bytes,
            });
        }

        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(self.policy.max_source_bytes);
        let mut bytes = Vec::with_capacity(capacity);
        while let Some(chunk) = response.chunk().await.map_err(reqwest_artwork_error)? {
            let next_len =
                bytes
                    .len()
                    .checked_add(chunk.len())
                    .ok_or(ArtworkError::SourceTooLarge {
                        limit: self.policy.max_source_bytes,
                    })?;
            if next_len > self.policy.max_source_bytes {
                return Err(ArtworkError::SourceTooLarge {
                    limit: self.policy.max_source_bytes,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(ArtworkError::Io("artwork source was empty".to_owned()));
        }
        Ok(bytes)
    }

    /// Decode bytes directly under the same preallocation and output limits.
    pub fn encode_data_url(&self, bytes: &[u8]) -> std::result::Result<String, ArtworkError> {
        encode_art_jpeg(bytes, self.policy)
    }
}

fn reqwest_artwork_error(error: reqwest::Error) -> ArtworkError {
    if error.is_timeout() {
        ArtworkError::Timeout
    } else {
        ArtworkError::Io(error.to_string())
    }
}

impl Default for ArtworkFetcher {
    fn default() -> Self {
        Self::new(ArtworkPolicy::default()).expect("constant artwork policy builds a client")
    }
}

fn read_bounded_file(
    path: &std::path::Path,
    limit: usize,
) -> std::result::Result<Vec<u8>, ArtworkError> {
    let file = std::fs::File::open(path).map_err(|error| ArtworkError::Io(error.to_string()))?;
    if file
        .metadata()
        .map_err(|error| ArtworkError::Io(error.to_string()))?
        .len()
        > limit as u64
    {
        return Err(ArtworkError::SourceTooLarge { limit });
    }
    read_bounded(file, limit)
}

fn read_bounded(reader: impl Read, limit: usize) -> std::result::Result<Vec<u8>, ArtworkError> {
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut reader = reader.take(take_limit);
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| ArtworkError::Io(error.to_string()))?;
    if bytes.len() > limit {
        return Err(ArtworkError::SourceTooLarge { limit });
    }
    if bytes.is_empty() {
        return Err(ArtworkError::Io("artwork source was empty".to_owned()));
    }
    Ok(bytes)
}

fn encode_art_jpeg(
    bytes: &[u8],
    policy: ArtworkPolicy,
) -> std::result::Result<String, ArtworkError> {
    if bytes.is_empty() || bytes.len() > policy.max_source_bytes {
        return Err(ArtworkError::SourceTooLarge {
            limit: policy.max_source_bytes,
        });
    }

    let probe = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let (width, height) = probe
        .into_dimensions()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if width > policy.max_source_dimension
        || height > policy.max_source_dimension
        || pixels > policy.max_source_pixels
    {
        return Err(ArtworkError::DimensionsTooLarge { width, height });
    }

    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(policy.max_source_dimension);
    limits.max_image_height = Some(policy.max_source_dimension);
    limits.max_alloc = Some(policy.max_decode_bytes);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let image = if image.width() > policy.max_output_dimension
        || image.height() > policy.max_output_dimension
    {
        image.resize(
            policy.max_output_dimension,
            policy.max_output_dimension,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };

    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        Cursor::new(&mut jpeg),
        ART_JPEG_QUALITY,
    );
    image
        .into_rgb8()
        .write_with_encoder(encoder)
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;

    const PREFIX: &str = "data:image/jpeg;base64,";
    let encoded_len = jpeg.len().div_ceil(3).saturating_mul(4);
    let output_len = PREFIX
        .len()
        .checked_add(encoded_len)
        .ok_or(ArtworkError::OutputTooLarge {
            limit: policy.max_data_url_bytes,
        })?;
    if output_len > policy.max_data_url_bytes {
        return Err(ArtworkError::OutputTooLarge {
            limit: policy.max_data_url_bytes,
        });
    }
    let mut data_url = String::with_capacity(output_len);
    data_url.push_str(PREFIX);
    base64::engine::general_purpose::STANDARD.encode_string(jpeg, &mut data_url);
    Ok(data_url)
}

#[derive(Clone)]
struct CompletedMediaPoll {
    state: Arc<MediaState>,
    completed_at: Instant,
    kind: MediaPublicationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaPublicationKind {
    BackendSuccess,
    StateUpdate,
    BackendFailure,
}

/// Publication seam shared by providers, enrichment, and the input source.
#[derive(Clone)]
pub struct MediaPollPublisher {
    state_tx: watch::Sender<Arc<MediaState>>,
    poll_tx: watch::Sender<Option<Arc<CompletedMediaPoll>>>,
    artwork_key: Arc<Mutex<Option<String>>>,
}

impl MediaPollPublisher {
    /// Publish one successfully completed backend poll.
    pub fn publish_completed(&self, state: MediaState, completed_at: Instant) {
        let key = state.available.then(|| state.track_key());
        self.publish_metadata(state, key, completed_at);
    }

    fn publish_metadata(
        &self,
        mut state: MediaState,
        artwork_key: Option<String>,
        completed_at: Instant,
    ) {
        let mut current_key = self
            .artwork_key
            .lock()
            .expect("media artwork-key lock is not poisoned");
        if *current_key == artwork_key {
            state
                .art_data_url
                .clone_from(&self.state_tx.borrow().art_data_url);
        } else {
            *current_key = artwork_key;
        }
        self.publish(state, completed_at, MediaPublicationKind::BackendSuccess);
    }

    fn publish_unavailable(&self, completed_at: Instant) {
        *self
            .artwork_key
            .lock()
            .expect("media artwork-key lock is not poisoned") = None;
        self.publish(
            MediaState::unavailable(),
            completed_at,
            MediaPublicationKind::BackendFailure,
        );
    }

    fn publish_enrichment(&self, key: &str, data_url: String) {
        let current_key = self
            .artwork_key
            .lock()
            .expect("media artwork-key lock is not poisoned");
        if current_key.as_deref() != Some(key) {
            return;
        }
        let mut state = self.state_tx.borrow().as_ref().clone();
        if state.art_data_url.as_deref() == Some(data_url.as_str()) {
            return;
        }
        state.art_data_url = Some(data_url);
        self.publish(state, Instant::now(), MediaPublicationKind::StateUpdate);
    }

    fn publish(&self, state: MediaState, completed_at: Instant, kind: MediaPublicationKind) {
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
            kind,
        })));
    }

    fn reset(&self) {
        *self
            .artwork_key
            .lock()
            .expect("media artwork-key lock is not poisoned") = None;
        self.state_tx
            .send_replace(Arc::new(MediaState::unavailable()));
        self.poll_tx.send_replace(None);
    }
}

/// Now-playing input source backed by MPRIS on Linux and GSMTC on Windows.
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
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    poller: Option<worker::MediaPollerThread>,
}

impl MediaSource {
    #[must_use]
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(Arc::new(MediaState::unavailable()));
        let (poll_tx, poll_rx) = watch::channel(None);
        Self {
            name: native_media_name().to_owned(),
            publisher: MediaPollPublisher {
                state_tx,
                poll_tx,
                artwork_key: Arc::new(Mutex::new(None)),
            },
            state_rx,
            poll_rx,
            last_poll: None,
            last_sampled: None,
            last_logged_track_key: None,
            running: false,
            status: SourceStatusReporter::new(
                "media",
                SourceKind::Media,
                native_media_backend(),
                true,
                true,
                true,
            ),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            poller: None,
        }
    }

    #[must_use]
    pub fn receiver(&self) -> watch::Receiver<Arc<MediaState>> {
        self.state_rx.clone()
    }

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

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(poller) = self.poller.as_mut() {
            if poller.stop() {
                self.poller = None;
            } else {
                anyhow::bail!("previous media poller is still stopping");
            }
        }

        self.publisher.reset();
        self.last_poll = None;
        self.last_sampled = None;
        let status = self.status.begin_session()?;

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            self.poller = match worker::MediaPollerThread::spawn(self.publisher.clone(), status) {
                Ok(poller) => Some(poller),
                Err(error) => {
                    if let Some(status) = self.status.session() {
                        status.failed(SourceIssue::new(
                            "media_poller_start_failed",
                            error.to_string(),
                            true,
                        ));
                    }
                    self.status.stop();
                    self.publisher.reset();
                    return Err(error);
                }
            };
            self.running = true;
            Ok(())
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            info!("media input source is unavailable on this platform");
            self.running = true;
            if let Some(status) = status {
                status.unavailable(
                    SourceIssue::new(
                        "media_backend_unsupported",
                        "native media input is unavailable on this platform",
                        false,
                    )
                    .with_remediation("run Hypercolor on Linux or Windows for media input"),
                );
            }
            Ok(())
        }
    }

    fn stop(&mut self) {
        self.status.stop();
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(poller) = self.poller.as_mut()
            && poller.stop()
        {
            self.poller = None;
        }
        self.running = false;
        self.last_poll = None;
        self.last_sampled = None;
    }

    fn sample(&mut self) -> Result<InputData> {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(reason) = self
            .poller
            .as_mut()
            .and_then(worker::MediaPollerThread::observe_exit)
        {
            self.poller = None;
            self.publisher.reset();
            self.last_poll = None;
            self.last_sampled = None;
            if let Some(status) = self.status.session() {
                status.failed(SourceIssue::new("media_poller_exited", reason, true));
            }
            return Ok(InputData::None);
        }
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

        if poll.kind == MediaPublicationKind::BackendSuccess
            && let Some(status) = self.status.session()
        {
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

const fn native_media_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        return "MPRIS Media";
    }
    #[cfg(target_os = "windows")]
    {
        return "Windows Media";
    }
    #[allow(unreachable_code)]
    "Native Media"
}

const fn native_media_backend() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        return "mpris";
    }
    #[cfg(target_os = "windows")]
    {
        return "gsmtc";
    }
    #[allow(unreachable_code)]
    "unsupported"
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod worker {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use anyhow::{Context, Result};
    use tokio::time::MissedTickBehavior;
    use tracing::{debug, warn};

    use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};

    use super::{
        ArtworkFetcher, ArtworkRequest, MEDIA_POLL_INTERVAL, MEDIA_PROVIDER_TIMEOUT,
        MediaPollPublisher, MediaProviderFailure, MediaProviderSession, SourceIssue,
        SourceSessionWriter, WORKER_READY_TIMEOUT, WORKER_STOP_TIMEOUT,
    };

    pub(super) struct MediaPollerThread {
        stop_tx: tokio::sync::watch::Sender<bool>,
        exit_rx: mpsc::Receiver<()>,
        join_handle: Option<JoinHandle<()>>,
    }

    impl MediaPollerThread {
        pub(super) fn spawn(
            publisher: MediaPollPublisher,
            status: Option<SourceSessionWriter>,
        ) -> Result<Self> {
            let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (exit_tx, exit_rx) = mpsc::sync_channel(1);
            let join_handle = spawn_input_worker(
                std::thread::Builder::new().name("hypercolor-media".to_owned()),
                move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            if let Some(status) = &status {
                                status.failed(SourceIssue::new(
                                    "media_runtime_start_failed",
                                    error.to_string(),
                                    true,
                                ));
                            }
                            let _ = ready_tx.send(Err(error.to_string()));
                            let _ = exit_tx.send(());
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    tokio::task::LocalSet::new()
                        .block_on(&runtime, run_media_loop(publisher, status, stop_rx));
                    let _ = exit_tx.send(());
                },
            )
            .context("failed to spawn media poller thread")?;

            let mut worker = Self {
                stop_tx,
                exit_rx,
                join_handle: Some(join_handle),
            };
            match ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
                Ok(Ok(())) => Ok(worker),
                Ok(Err(error)) => {
                    worker.stop();
                    anyhow::bail!("media poller failed before readiness: {error}");
                }
                Err(error) => {
                    worker.stop();
                    anyhow::bail!("media poller readiness timed out: {error}");
                }
            }
        }

        pub(super) fn stop(&mut self) -> bool {
            let Some(join_handle) = self.join_handle.as_ref() else {
                return true;
            };
            self.stop_tx.send_replace(true);
            let _ = self.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
            if !join_handle.is_finished() {
                warn!("media poller did not stop before the deadline; retaining its join handle");
                return false;
            }
            let join_handle = self
                .join_handle
                .take()
                .expect("finished media worker remains owned");
            if let Err(error) = join_handle.join() {
                debug!("media poller thread join failed: {error:?}");
            }
            true
        }

        pub(super) fn observe_exit(&mut self) -> Option<String> {
            let join_handle = self.join_handle.as_ref()?;
            if !join_handle.is_finished() {
                return None;
            }
            let join_handle = self
                .join_handle
                .take()
                .expect("finished media worker remains owned");
            Some(join_handle.join().map_or_else(
                |panic| format!("media poller panicked: {panic:?}"),
                |()| "media poller exited unexpectedly".to_owned(),
            ))
        }
    }

    impl Drop for MediaPollerThread {
        fn drop(&mut self) {
            if self.stop() {
                return;
            }
            let Some(join_handle) = self.join_handle.take() else {
                return;
            };
            retain_input_worker(join_handle, "media poller");
        }
    }

    async fn run_media_loop(
        publisher: MediaPollPublisher,
        status: Option<SourceSessionWriter>,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut session = MediaProviderSession::new(super::native_provider());
        let (art_tx, art_rx) = tokio::sync::watch::channel(None);
        let art_task = tokio::task::spawn_local(run_artwork_loop(
            ArtworkFetcher::default(),
            publisher.clone(),
            art_rx,
            stop_rx.clone(),
        ));
        let mut interval = tokio::time::interval(MEDIA_POLL_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_artwork: Option<ArtworkRequest> = None;

        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    let poll = tokio::select! {
                        changed = stop_rx.changed() => {
                            if changed.is_err() || *stop_rx.borrow() {
                                break;
                            }
                            continue;
                        }
                        result = tokio::time::timeout(MEDIA_PROVIDER_TIMEOUT, session.poll()) => result,
                    };
                    match poll {
                        Ok(Ok(metadata)) => {
                            let key = metadata.artwork.as_ref().map(|request| request.key.clone());
                            publisher.publish_metadata(metadata.state, key, std::time::Instant::now());
                            if metadata.artwork != last_artwork {
                                last_artwork.clone_from(&metadata.artwork);
                                art_tx.send_replace(metadata.artwork.map(Arc::new));
                            }
                        }
                        Ok(Err(MediaProviderFailure::Connect(error))) => {
                            if let Some(status) = &status {
                                status.unavailable(
                                    SourceIssue::new(
                                        "media_provider_unavailable",
                                        error.to_string(),
                                        true,
                                    )
                                    .with_remediation(native_remediation()),
                                );
                            }
                            publisher.publish_unavailable(std::time::Instant::now());
                            if last_artwork.take().is_some() {
                                art_tx.send_replace(None);
                            }
                        }
                        Ok(Err(MediaProviderFailure::Poll(error))) => {
                            if let Some(status) = &status {
                                status.degraded(SourceIssue::new(
                                    "media_poll_failed",
                                    error.to_string(),
                                    true,
                                ));
                            }
                            publisher.publish_unavailable(std::time::Instant::now());
                            if last_artwork.take().is_some() {
                                art_tx.send_replace(None);
                            }
                        }
                        Err(_) => {
                            session.disconnect();
                            if let Some(status) = &status {
                                status.degraded(SourceIssue::new(
                                    "media_poll_timed_out",
                                    format!(
                                        "{} metadata poll exceeded {:?}",
                                        session.backend_name(),
                                        MEDIA_PROVIDER_TIMEOUT
                                    ),
                                    true,
                                ));
                            }
                            publisher.publish_unavailable(std::time::Instant::now());
                            if last_artwork.take().is_some() {
                                art_tx.send_replace(None);
                            }
                        }
                    }
                }
            }
        }
        session.disconnect();
        drop(art_tx);
        let _ = art_task.await;
    }

    async fn run_artwork_loop(
        fetcher: ArtworkFetcher,
        publisher: MediaPollPublisher,
        mut art_rx: tokio::sync::watch::Receiver<Option<Arc<ArtworkRequest>>>,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            let Some(request) = art_rx.borrow().clone() else {
                tokio::select! {
                    changed = art_rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                    changed = stop_rx.changed() => {
                        if changed.is_err() || *stop_rx.borrow() {
                            return;
                        }
                    }
                }
                continue;
            };

            tokio::select! {
                result = fetcher.fetch_data_url(&request.source) => {
                    match result {
                        Ok(data_url) => publisher.publish_enrichment(&request.key, data_url),
                        Err(error) => debug!(%error, "media artwork enrichment failed"),
                    }
                    tokio::select! {
                        changed = art_rx.changed() => {
                            if changed.is_err() {
                                return;
                            }
                        }
                        changed = stop_rx.changed() => {
                            if changed.is_err() || *stop_rx.borrow() {
                                return;
                            }
                        }
                    }
                }
                changed = art_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        return;
                    }
                }
            }
        }
    }

    const fn native_remediation() -> &'static str {
        #[cfg(target_os = "linux")]
        {
            return "run Hypercolor inside a desktop login session with D-Bus";
        }
        #[cfg(target_os = "windows")]
        {
            return "allow media-session access and start a GSMTC-capable player";
        }
        #[allow(unreachable_code)]
        "enable a native media provider"
    }
}

#[cfg(target_os = "linux")]
fn native_provider() -> Box<dyn MediaMetadataProvider> {
    Box::new(linux::MprisProvider::new())
}

#[cfg(target_os = "windows")]
fn native_provider() -> Box<dyn MediaMetadataProvider> {
    Box::new(windows::GsmtcProvider::new())
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::Context;
    use zbus::zvariant::OwnedValue;

    use super::{
        ArtworkSource, MediaMetadataProvider, MediaProviderError, PlaybackStatus, PlayerSnapshot,
    };

    const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
    const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
    const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

    pub(super) struct MprisProvider {
        connection: Option<zbus::Connection>,
    }

    impl MprisProvider {
        pub(super) const fn new() -> Self {
            Self { connection: None }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl MediaMetadataProvider for MprisProvider {
        fn backend_name(&self) -> &'static str {
            "mpris"
        }

        async fn connect(&mut self) -> std::result::Result<(), MediaProviderError> {
            let connection = zbus::Connection::session()
                .await
                .context("failed to connect to the session bus")
                .map_err(|error| MediaProviderError::new(error.to_string()))?;
            self.connection = Some(connection);
            Ok(())
        }

        async fn poll_players(
            &mut self,
        ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
            let connection = self
                .connection
                .as_ref()
                .ok_or_else(|| MediaProviderError::new("MPRIS provider is disconnected"))?;
            poll_players(connection)
                .await
                .map_err(|error| MediaProviderError::new(error.to_string()))
        }

        fn disconnect(&mut self) {
            self.connection = None;
        }
    }

    async fn poll_players(connection: &zbus::Connection) -> anyhow::Result<Vec<PlayerSnapshot>> {
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
        let position_us: i64 = proxy.get_property("Position").await.unwrap_or(0);

        Some(PlayerSnapshot {
            bus_name: bus_name.to_owned(),
            status: PlaybackStatus::from_mpris(&status),
            track: metadata_string(&metadata, "xesam:title"),
            artist: metadata_string_list_head(&metadata, "xesam:artist"),
            album: metadata_string(&metadata, "xesam:album"),
            artwork: {
                let url = metadata_string(&metadata, "mpris:artUrl");
                (!url.is_empty()).then_some(ArtworkSource::Url(url))
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
        let Some(value) = metadata.get("mpris:length") else {
            return 0;
        };
        i64::try_from(value)
            .map(|microseconds| microseconds.max(0).unsigned_abs())
            .or_else(|_| u64::try_from(value))
            .unwrap_or(0)
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use ::windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    use ::windows::Storage::Streams::DataReader;

    use super::{
        ArtworkError, ArtworkSource, MediaMetadataProvider, MediaProviderError, PlaybackStatus,
        PlayerSnapshot,
    };

    pub(super) struct GsmtcProvider {
        manager: Option<GlobalSystemMediaTransportControlsSessionManager>,
    }

    impl GsmtcProvider {
        pub(super) const fn new() -> Self {
            Self { manager: None }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl MediaMetadataProvider for GsmtcProvider {
        fn backend_name(&self) -> &'static str {
            "gsmtc"
        }

        async fn connect(&mut self) -> std::result::Result<(), MediaProviderError> {
            let operation = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .map_err(provider_error)?;
            self.manager = Some(operation.await.map_err(provider_error)?);
            Ok(())
        }

        async fn poll_players(
            &mut self,
        ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
            let manager = self
                .manager
                .as_ref()
                .ok_or_else(|| MediaProviderError::new("GSMTC provider is disconnected"))?;
            snapshot_sessions(manager).await
        }

        fn disconnect(&mut self) {
            self.manager = None;
        }
    }

    async fn snapshot_sessions(
        manager: &GlobalSystemMediaTransportControlsSessionManager,
    ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
        let sessions = manager.GetSessions().map_err(provider_error)?;
        let session_handles = (0..sessions.Size().map_err(provider_error)?)
            .map(|index| sessions.GetAt(index).map_err(provider_error))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(sessions);
        let session_count = session_handles.len();
        let mut players = Vec::with_capacity(session_count);
        let mut first_error = None;
        for session in session_handles {
            match snapshot_session(&session).await {
                Ok(snapshot) => players.push(snapshot),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if players.is_empty()
            && session_count != 0
            && let Some(error) = first_error
        {
            return Err(error);
        }
        Ok(players)
    }

    async fn snapshot_session(
        session: &GlobalSystemMediaTransportControlsSession,
    ) -> std::result::Result<PlayerSnapshot, MediaProviderError> {
        let bus_name = session
            .SourceAppUserModelId()
            .map_err(provider_error)?
            .to_string();
        let playback = session.GetPlaybackInfo().map_err(provider_error)?;
        let status = match playback.PlaybackStatus().map_err(provider_error)? {
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => {
                PlaybackStatus::Playing
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => {
                PlaybackStatus::Paused
            }
            _ => PlaybackStatus::Stopped,
        };
        let properties = session
            .TryGetMediaPropertiesAsync()
            .map_err(provider_error)?
            .await
            .map_err(provider_error)?;
        let timeline = session.GetTimelineProperties().map_err(provider_error)?;
        let position_ms = ticks_to_millis(timeline.Position().map_err(provider_error)?.Duration);
        let start_ticks = timeline.StartTime().map_err(provider_error)?.Duration;
        let end_ticks = timeline.EndTime().map_err(provider_error)?.Duration;
        let duration_ms = ticks_to_millis(end_ticks.saturating_sub(start_ticks));
        let artwork = properties
            .Thumbnail()
            .is_ok()
            .then(|| ArtworkSource::WindowsSession(bus_name.clone()));

        Ok(PlayerSnapshot {
            bus_name,
            status,
            track: properties.Title().map_err(provider_error)?.to_string(),
            artist: properties.Artist().map_err(provider_error)?.to_string(),
            album: properties.AlbumTitle().map_err(provider_error)?.to_string(),
            artwork,
            position_ms,
            duration_ms,
        })
    }

    pub(super) async fn fetch_thumbnail_bytes(
        source_app_id: &str,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .map_err(artwork_io)?
            .await
            .map_err(artwork_io)?;
        let session = find_session(&manager, source_app_id)?;
        let properties = session
            .TryGetMediaPropertiesAsync()
            .map_err(artwork_io)?
            .await
            .map_err(artwork_io)?;
        let stream = properties
            .Thumbnail()
            .map_err(artwork_io)?
            .OpenReadAsync()
            .map_err(artwork_io)?
            .await
            .map_err(artwork_io)?;
        let size = stream.Size().map_err(artwork_io)?;
        if size > max_bytes as u64 {
            return Err(ArtworkError::SourceTooLarge { limit: max_bytes });
        }
        let count = u32::try_from(size).map_err(|error| ArtworkError::Io(error.to_string()))?;
        if count == 0 {
            return Err(ArtworkError::Io("GSMTC thumbnail was empty".to_owned()));
        }
        let input = stream.GetInputStreamAt(0).map_err(artwork_io)?;
        let reader = DataReader::CreateDataReader(&input).map_err(artwork_io)?;
        let loaded = reader
            .LoadAsync(count)
            .map_err(artwork_io)?
            .await
            .map_err(artwork_io)?;
        if loaded != count {
            return Err(ArtworkError::Io(format!(
                "GSMTC thumbnail ended after {loaded} of {count} bytes"
            )));
        }
        let mut bytes = vec![0; count as usize];
        reader.ReadBytes(&mut bytes).map_err(artwork_io)?;
        let _ = reader.Close();
        Ok(bytes)
    }

    fn find_session(
        manager: &GlobalSystemMediaTransportControlsSessionManager,
        source_app_id: &str,
    ) -> std::result::Result<GlobalSystemMediaTransportControlsSession, ArtworkError> {
        let sessions = manager.GetSessions().map_err(artwork_io)?;
        for index in 0..sessions.Size().map_err(artwork_io)? {
            let session = sessions.GetAt(index).map_err(artwork_io)?;
            let candidate = session.SourceAppUserModelId().map_err(artwork_io)?;
            if candidate == source_app_id {
                return Ok(session);
            }
        }
        Err(ArtworkError::Io(format!(
            "GSMTC session {source_app_id} disappeared"
        )))
    }

    fn ticks_to_millis(ticks: i64) -> u64 {
        ticks.max(0).unsigned_abs() / 10_000
    }

    fn provider_error(error: ::windows::core::Error) -> MediaProviderError {
        MediaProviderError::new(error.to_string())
    }

    fn artwork_io(error: ::windows::core::Error) -> ArtworkError {
        ArtworkError::Io(error.to_string())
    }
}
