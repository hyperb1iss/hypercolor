//! Cross-platform now-playing input.
//!
//! Linux reads MPRIS over the session bus and Windows reads Global System
//! Media Transport Controls (GSMTC). Metadata polling and artwork enrichment
//! are independent: a slow or hostile image can never delay the next player
//! snapshot.

use std::future::Future;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use hypercolor_types::media::MediaState;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::SourceSessionWriter;
use super::traits::{InputData, InputSource};
use super::worker_retention::{retain_input_worker, spawn_input_worker};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

/// Poll cadence for player discovery, status, and position.
pub const MEDIA_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum time allowed for one provider connection or metadata poll.
pub const MEDIA_PROVIDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum time one native player may consume while producing a snapshot.
pub const MEDIA_PLAYER_TIMEOUT: Duration = Duration::from_secs(2);

/// Deadline after which artwork work is cancelled, reaped, or quarantined.
pub const ART_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Grace period for cooperative blocking artwork cancellation.
const ART_REAP_GRACE: Duration = Duration::from_millis(250);

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
    WindowsSession(WindowsArtworkSource),
}

/// Exact Windows media-session identity retained for deferred artwork reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsArtworkSource {
    source_app_id: String,
    track: String,
    artist: String,
    album: String,
    session: WindowsSessionReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowsSessionReference {
    #[cfg(target_os = "windows")]
    Native(::windows::Media::Control::GlobalSystemMediaTransportControlsSession),
    Fixture(String),
}

impl ArtworkSource {
    /// Build an opaque Windows identity for deterministic provider conformance tests.
    #[doc(hidden)]
    #[must_use]
    pub fn windows_session_fixture(
        session_key: impl Into<String>,
        source_app_id: impl Into<String>,
        track: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
    ) -> Self {
        Self::WindowsSession(WindowsArtworkSource {
            source_app_id: source_app_id.into(),
            track: track.into(),
            artist: artist.into(),
            album: album.into(),
            session: WindowsSessionReference::Fixture(session_key.into()),
        })
    }

    #[cfg(target_os = "windows")]
    fn native_windows_session(
        session: ::windows::Media::Control::GlobalSystemMediaTransportControlsSession,
        source_app_id: String,
        track: String,
        artist: String,
        album: String,
    ) -> Self {
        Self::WindowsSession(WindowsArtworkSource {
            source_app_id,
            track,
            artist,
            album,
            session: WindowsSessionReference::Native(session),
        })
    }

    fn cache_key(&self) -> String {
        match self {
            Self::Url(value) => value.clone(),
            Self::WindowsSession(WindowsArtworkSource {
                source_app_id,
                track,
                artist,
                album,
                session,
            }) => {
                let session_key = match session {
                    #[cfg(target_os = "windows")]
                    WindowsSessionReference::Native(session) => {
                        use ::windows::core::Interface as _;
                        let identity = session
                            .cast::<::windows::core::IUnknown>()
                            .expect("every WinRT session exposes canonical IUnknown identity");
                        format!("{:p}", identity.as_raw())
                    }
                    WindowsSessionReference::Fixture(session_key) => session_key.clone(),
                };
                format!(
                    "{source_app_id}\u{1f}{session_key}\u{1f}{artist}\u{1f}{album}\u{1f}{track}"
                )
            }
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
    let artwork_key = player
        .artwork
        .as_ref()
        .map_or_else(String::new, ArtworkSource::cache_key);
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        player.bus_name, player.artist, player.album, player.track, player.duration_ms, artwork_key
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

/// Resolve independent player snapshots concurrently while preserving scan order.
///
/// Failed players are skipped when any sibling succeeds. If every attempted
/// player fails, the lowest-indexed failure is returned deterministically.
pub async fn collect_player_snapshots<F>(
    snapshots: impl IntoIterator<Item = F>,
    timeout: Duration,
) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError>
where
    F: Future<Output = std::result::Result<PlayerSnapshot, MediaProviderError>>,
{
    let mut pending: FuturesUnordered<_> =
        snapshots
            .into_iter()
            .enumerate()
            .map(|(index, snapshot)| async move {
                (index, tokio::time::timeout(timeout, snapshot).await)
            })
            .collect();
    let attempted = pending.len();
    let mut ordered = std::iter::repeat_with(|| None)
        .take(attempted)
        .collect::<Vec<_>>();
    let mut failures = std::iter::repeat_with(|| None)
        .take(attempted)
        .collect::<Vec<_>>();

    while let Some((index, result)) = pending.next().await {
        match result {
            Ok(Ok(snapshot)) => ordered[index] = Some(snapshot),
            Ok(Err(error)) => {
                failures[index] = Some(error);
            }
            Err(_) => {
                failures[index] = Some(MediaProviderError::new(format!(
                    "native media player {index} exceeded {timeout:?}"
                )));
            }
        }
    }

    let players = ordered.into_iter().flatten().collect::<Vec<_>>();
    let first_failure = failures.into_iter().flatten().next();
    if players.is_empty() && attempted != 0 {
        return Err(first_failure.expect("every attempted player recorded a failure"));
    }
    if let Some(error) = first_failure {
        debug!(%error, healthy_players = players.len(), "Skipped unhealthy media player");
    }
    Ok(players)
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
    #[error("artwork operation was cancelled")]
    Cancelled,
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
    blocking: Arc<dyn ArtworkBlockingBackend>,
    blocking_gate: Arc<Semaphore>,
}

/// Blocking artwork operations executed under managed cancellation.
pub trait ArtworkBlockingBackend: Send + Sync {
    /// Read one local file while observing cooperative cancellation.
    fn read_file(
        &self,
        path: &Path,
        limit: usize,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError>;

    /// Decode and encode one image while observing cooperative cancellation.
    fn encode(
        &self,
        bytes: &[u8],
        policy: ArtworkPolicy,
        cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError>;
}

struct NativeArtworkBlockingBackend;

impl ArtworkBlockingBackend for NativeArtworkBlockingBackend {
    fn read_file(
        &self,
        path: &Path,
        limit: usize,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        read_bounded_file(path, limit, cancel)
    }

    fn encode(
        &self,
        bytes: &[u8],
        policy: ArtworkPolicy,
        cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        encode_art_jpeg(bytes, policy, cancel)
    }
}

impl ArtworkFetcher {
    /// Build a fetcher with explicit redirect, time, and size policy.
    pub fn new(policy: ArtworkPolicy) -> std::result::Result<Self, ArtworkError> {
        Self::with_blocking_backend(policy, Arc::new(NativeArtworkBlockingBackend))
    }

    /// Build a fetcher with an explicit blocking execution backend.
    pub fn with_blocking_backend(
        policy: ArtworkPolicy,
        blocking: Arc<dyn ArtworkBlockingBackend>,
    ) -> std::result::Result<Self, ArtworkError> {
        Self::build(policy, blocking, Arc::new(Semaphore::new(1)))
    }

    fn build(
        policy: ArtworkPolicy,
        blocking: Arc<dyn ArtworkBlockingBackend>,
        blocking_gate: Arc<Semaphore>,
    ) -> std::result::Result<Self, ArtworkError> {
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
        Ok(Self {
            client,
            policy,
            blocking,
            blocking_gate,
        })
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
        self.fetch_data_url_cancellable(source, &CancellationToken::new())
            .await
    }

    /// Resolve one source and reap its blocking work before cancellation returns.
    pub async fn fetch_data_url_cancellable(
        &self,
        source: &ArtworkSource,
        cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        let operation_cancel = cancel.child_token();
        let work = async {
            let bytes = match source {
                ArtworkSource::Url(url) => self.load_url(url, &operation_cancel).await?,
                ArtworkSource::WindowsSession(source) => {
                    #[cfg(target_os = "windows")]
                    {
                        let WindowsSessionReference::Native(session) = &source.session else {
                            return Err(ArtworkError::UnsupportedSource);
                        };
                        windows::fetch_thumbnail_bytes(
                            session,
                            source,
                            self.policy.max_source_bytes,
                            &operation_cancel,
                        )
                        .await?
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = source;
                        return Err(ArtworkError::UnsupportedSource);
                    }
                }
            };
            let policy = self.policy;
            let blocking = Arc::clone(&self.blocking);
            let blocking_cancel = operation_cancel.clone();
            self.run_blocking("media artwork decoder", &operation_cancel, move || {
                blocking.encode(&bytes, policy, &blocking_cancel)
            })
            .await
        };
        tokio::pin!(work);
        let deadline = tokio::time::sleep(self.policy.fetch_timeout);
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                operation_cancel.cancel();
                let _ = work.await;
                Err(ArtworkError::Cancelled)
            }
            () = &mut deadline => {
                operation_cancel.cancel();
                let _ = work.await;
                Err(ArtworkError::Timeout)
            }
            result = &mut work => result,
        }
    }

    async fn load_url(
        &self,
        value: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        let url = url::Url::parse(value).map_err(|_| ArtworkError::UnsupportedSource)?;
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| ArtworkError::UnsupportedSource)?;
                let limit = self.policy.max_source_bytes;
                let blocking = Arc::clone(&self.blocking);
                let blocking_cancel = cancel.clone();
                self.run_blocking("media artwork file reader", cancel, move || {
                    blocking.read_file(&path, limit, &blocking_cancel)
                })
                .await
            }
            "http" | "https" => self.load_http(url, cancel).await,
            _ => Err(ArtworkError::UnsupportedSource),
        }
    }

    async fn load_http(
        &self,
        url: url::Url,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        let response = self.client.get(url).send();
        tokio::pin!(response);
        let mut response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ArtworkError::Cancelled),
            response = &mut response => response,
        }
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
        loop {
            let chunk = response.chunk();
            tokio::pin!(chunk);
            let chunk = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ArtworkError::Cancelled),
                chunk = &mut chunk => chunk,
            }
            .map_err(reqwest_artwork_error)?;
            let Some(chunk) = chunk else {
                break;
            };
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
        self.blocking
            .encode(bytes, self.policy, &CancellationToken::new())
    }

    async fn run_blocking<T, F>(
        &self,
        context: &'static str,
        cancel: &CancellationToken,
        operation: F,
    ) -> std::result::Result<T, ArtworkError>
    where
        T: Send + 'static,
        F: FnOnce() -> std::result::Result<T, ArtworkError> + Send + 'static,
    {
        let gate = Arc::clone(&self.blocking_gate);
        let permit = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ArtworkError::Cancelled),
            permit = gate.acquire_owned() => permit.map_err(|error| ArtworkError::Io(error.to_string()))?,
        };
        let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
        let mut worker = Some(
            spawn_input_worker(
                std::thread::Builder::new().name("hypercolor-artwork".to_owned()),
                move || {
                    let result = operation();
                    drop(permit);
                    let _ = result_tx.send(result);
                },
            )
            .map_err(|error| ArtworkError::Io(error.to_string()))?,
        );

        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                if let Ok(result) = tokio::time::timeout(ART_REAP_GRACE, &mut result_rx).await {
                    result
                } else {
                    warn!(worker = context, "artwork job ignored cancellation; quarantining it");
                    retain_input_worker(
                        worker.take().expect("running artwork worker remains owned"),
                        context,
                    );
                    return Err(ArtworkError::Cancelled);
                }
            }
            result = &mut result_rx => result,
        };
        let worker = worker
            .take()
            .expect("completed artwork worker remains owned");
        if let Err(panic) = worker.join() {
            return Err(ArtworkError::Io(format!("{context} panicked: {panic:?}")));
        }
        result.map_err(|_| ArtworkError::Io(format!("{context} exited without a result")))?
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
        Self::build(
            ArtworkPolicy::default(),
            Arc::new(NativeArtworkBlockingBackend),
            process_artwork_gate(),
        )
        .expect("constant artwork policy builds a client")
    }
}

fn process_artwork_gate() -> Arc<Semaphore> {
    static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(GATE.get_or_init(|| Arc::new(Semaphore::new(1))))
}

fn read_bounded_file(
    path: &std::path::Path,
    limit: usize,
    cancel: &CancellationToken,
) -> std::result::Result<Vec<u8>, ArtworkError> {
    check_artwork_cancelled(cancel)?;
    let metadata = std::fs::metadata(path).map_err(|error| ArtworkError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(ArtworkError::UnsupportedSource);
    }
    if metadata.len() > limit as u64 {
        return Err(ArtworkError::SourceTooLarge { limit });
    }
    check_artwork_cancelled(cancel)?;
    let file = std::fs::File::open(path).map_err(|error| ArtworkError::Io(error.to_string()))?;
    read_bounded(file, limit, cancel)
}

fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    cancel: &CancellationToken,
) -> std::result::Result<Vec<u8>, ArtworkError> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0; 16 * 1024];
    loop {
        check_artwork_cancelled(cancel)?;
        let read = reader
            .read(&mut chunk)
            .map_err(|error| ArtworkError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        let next_len = bytes
            .len()
            .checked_add(read)
            .ok_or(ArtworkError::SourceTooLarge { limit })?;
        if next_len > limit {
            return Err(ArtworkError::SourceTooLarge { limit });
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if bytes.is_empty() {
        return Err(ArtworkError::Io("artwork source was empty".to_owned()));
    }
    Ok(bytes)
}

fn encode_art_jpeg(
    bytes: &[u8],
    policy: ArtworkPolicy,
    cancel: &CancellationToken,
) -> std::result::Result<String, ArtworkError> {
    check_artwork_cancelled(cancel)?;
    if bytes.is_empty() || bytes.len() > policy.max_source_bytes {
        return Err(ArtworkError::SourceTooLarge {
            limit: policy.max_source_bytes,
        });
    }

    let probe = artwork_reader(bytes, policy, false)?;
    let (width, height) = probe
        .into_dimensions()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    check_artwork_cancelled(cancel)?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if width > policy.max_source_dimension
        || height > policy.max_source_dimension
        || pixels > policy.max_source_pixels
    {
        return Err(ArtworkError::DimensionsTooLarge { width, height });
    }

    let reader = artwork_reader(bytes, policy, true)?;
    let image = reader
        .decode()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    check_artwork_cancelled(cancel)?;

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
    check_artwork_cancelled(cancel)?;
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

fn check_artwork_cancelled(cancel: &CancellationToken) -> std::result::Result<(), ArtworkError> {
    if cancel.is_cancelled() {
        Err(ArtworkError::Cancelled)
    } else {
        Ok(())
    }
}

fn artwork_reader(
    bytes: &[u8],
    policy: ArtworkPolicy,
    enforce_dimensions: bool,
) -> std::result::Result<image::ImageReader<Cursor<&[u8]>>, ArtworkError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let mut limits = image::Limits::default();
    if enforce_dimensions {
        limits.max_image_width = Some(policy.max_source_dimension);
        limits.max_image_height = Some(policy.max_source_dimension);
    }
    limits.max_alloc = Some(policy.max_decode_bytes);
    reader.limits(limits);
    Ok(reader)
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

struct MediaPublicationState {
    latest_generation: u64,
    active_generation: Option<u64>,
    artwork_key: Option<String>,
}

impl MediaPublicationState {
    fn initial() -> Self {
        Self {
            latest_generation: 1,
            active_generation: Some(1),
            artwork_key: None,
        }
    }
}

/// Publication seam shared by providers, enrichment, and the input source.
#[derive(Clone)]
pub struct MediaPollPublisher {
    state_tx: watch::Sender<Arc<MediaState>>,
    poll_tx: watch::Sender<Option<Arc<CompletedMediaPoll>>>,
    publication: Arc<Mutex<MediaPublicationState>>,
    generation: u64,
}

impl MediaPollPublisher {
    /// Publish one successfully completed backend poll.
    ///
    /// Returns `false` after this publisher's source generation is retired.
    #[must_use]
    pub fn publish_completed(&self, state: MediaState, completed_at: Instant) -> bool {
        let key = state.available.then(|| state.track_key());
        self.publish_metadata(state, key, completed_at)
    }

    fn publish_metadata(
        &self,
        mut state: MediaState,
        artwork_key: Option<String>,
        completed_at: Instant,
    ) -> bool {
        let mut publication = self
            .publication
            .lock()
            .expect("media publication lock is not poisoned");
        if publication.active_generation != Some(self.generation) {
            return false;
        }
        if publication.artwork_key == artwork_key {
            state
                .art_data_url
                .clone_from(&self.state_tx.borrow().art_data_url);
        } else {
            publication.artwork_key = artwork_key;
        }
        self.publish(state, completed_at, MediaPublicationKind::BackendSuccess);
        drop(publication);
        true
    }

    fn publish_unavailable(&self, completed_at: Instant) -> bool {
        let mut publication = self
            .publication
            .lock()
            .expect("media publication lock is not poisoned");
        if publication.active_generation != Some(self.generation) {
            return false;
        }
        publication.artwork_key = None;
        self.publish(
            MediaState::unavailable(),
            completed_at,
            MediaPublicationKind::BackendFailure,
        );
        drop(publication);
        true
    }

    fn publish_enrichment(&self, key: &str, data_url: String) {
        let publication = self
            .publication
            .lock()
            .expect("media publication lock is not poisoned");
        if publication.active_generation != Some(self.generation)
            || publication.artwork_key.as_deref() != Some(key)
        {
            return;
        }
        let mut state = self.state_tx.borrow().as_ref().clone();
        if state.art_data_url.as_deref() == Some(data_url.as_str()) {
            return;
        }
        state.art_data_url = Some(data_url);
        self.publish(state, Instant::now(), MediaPublicationKind::StateUpdate);
        drop(publication);
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

    fn begin_successor(&self) -> Self {
        let mut publication = self
            .publication
            .lock()
            .expect("media publication lock is not poisoned");
        let generation = publication
            .latest_generation
            .checked_add(1)
            .expect("media publication generation exhausted");
        publication.latest_generation = generation;
        publication.active_generation = Some(generation);
        publication.artwork_key = None;
        self.state_tx
            .send_replace(Arc::new(MediaState::unavailable()));
        self.poll_tx.send_replace(None);
        drop(publication);
        Self {
            state_tx: self.state_tx.clone(),
            poll_tx: self.poll_tx.clone(),
            publication: Arc::clone(&self.publication),
            generation,
        }
    }

    fn retire(&self) {
        let mut publication = self
            .publication
            .lock()
            .expect("media publication lock is not poisoned");
        if publication.active_generation != Some(self.generation) {
            return;
        }
        publication.active_generation = None;
        publication.artwork_key = None;
        self.state_tx
            .send_replace(Arc::new(MediaState::unavailable()));
        self.poll_tx.send_replace(None);
    }

    fn is_active(&self) -> bool {
        self.publication
            .lock()
            .expect("media publication lock is not poisoned")
            .active_generation
            == Some(self.generation)
    }
}

/// Run latest-value artwork enrichment with one reaped attempt at a time.
#[doc(hidden)]
pub async fn run_artwork_loop(
    fetcher: ArtworkFetcher,
    publisher: MediaPollPublisher,
    mut art_rx: tokio::sync::watch::Receiver<Option<Arc<ArtworkRequest>>>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    'artwork: loop {
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

        for attempt in 0..=1 {
            let cancel = CancellationToken::new();
            let fetch = fetcher.fetch_data_url_cancellable(&request.source, &cancel);
            tokio::pin!(fetch);
            let result = tokio::select! {
                result = &mut fetch => result,
                changed = art_rx.changed() => {
                    cancel.cancel();
                    let _ = fetch.await;
                    if changed.is_err() {
                        return;
                    }
                    continue 'artwork;
                }
                changed = stop_rx.changed() => {
                    cancel.cancel();
                    let _ = fetch.await;
                    if changed.is_err() || *stop_rx.borrow() {
                        return;
                    }
                    continue 'artwork;
                }
            };
            match result {
                Ok(data_url) => {
                    publisher.publish_enrichment(&request.key, data_url);
                    break;
                }
                Err(error) => {
                    debug!(%error, attempt, "media artwork enrichment failed");
                    if attempt == 0 {
                        tokio::select! {
                            () = tokio::time::sleep(MEDIA_POLL_INTERVAL) => {}
                            changed = art_rx.changed() => {
                                if changed.is_err() {
                                    return;
                                }
                                continue 'artwork;
                            }
                            changed = stop_rx.changed() => {
                                if changed.is_err() || *stop_rx.borrow() {
                                    return;
                                }
                                continue 'artwork;
                            }
                        }
                    }
                }
            }
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
                publication: Arc::new(Mutex::new(MediaPublicationState::initial())),
                generation: 1,
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

    /// Apply the terminal state transition for an exited poller.
    #[doc(hidden)]
    pub fn report_poller_exit(&mut self, reason: impl Into<String>) {
        self.publisher.retire();
        self.running = false;
        self.last_poll = None;
        self.last_sampled = None;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            self.poller = None;
        }
        if let Some(status) = self.status.session() {
            status.failed(SourceIssue::new("media_poller_exited", reason.into(), true));
        }
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

        let status = self.status.begin_session()?;
        self.publisher = self.publisher.begin_successor();
        self.last_poll = None;
        self.last_sampled = None;

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
                    self.publisher.retire();
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
        self.publisher.retire();
        self.running = false;
        self.last_poll = None;
        self.last_sampled = None;
        self.status.stop();
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(poller) = self.poller.as_mut()
            && poller.stop()
        {
            self.poller = None;
        }
    }

    fn sample(&mut self) -> Result<InputData> {
        if !self.publisher.is_active() {
            return Ok(InputData::None);
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(reason) = self
            .poller
            .as_mut()
            .and_then(worker::MediaPollerThread::observe_exit)
        {
            self.report_poller_exit(reason);
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
        SourceSessionWriter, WORKER_READY_TIMEOUT, WORKER_STOP_TIMEOUT, run_artwork_loop,
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
                            let _ = publisher.publish_metadata(
                                metadata.state,
                                key,
                                std::time::Instant::now(),
                            );
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
                            let _ = publisher.publish_unavailable(std::time::Instant::now());
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
                            let _ = publisher.publish_unavailable(std::time::Instant::now());
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
                            let _ = publisher.publish_unavailable(std::time::Instant::now());
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
        ArtworkSource, MEDIA_PLAYER_TIMEOUT, MediaMetadataProvider, MediaProviderError,
        PlaybackStatus, PlayerSnapshot, collect_player_snapshots,
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

        let snapshots = names
            .into_iter()
            .map(|name| name.to_string())
            .filter(|name| name.starts_with(MPRIS_PREFIX))
            .map(|name| async move { snapshot_player(connection, &name).await });
        collect_player_snapshots(snapshots, MEDIA_PLAYER_TIMEOUT)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn snapshot_player(
        connection: &zbus::Connection,
        bus_name: &str,
    ) -> std::result::Result<PlayerSnapshot, MediaProviderError> {
        let proxy = zbus::Proxy::new(connection, bus_name, MPRIS_PATH, PLAYER_INTERFACE)
            .await
            .map_err(|error| MediaProviderError::new(format!("{bus_name}: {error}")))?;
        let status: String = proxy
            .get_property("PlaybackStatus")
            .await
            .map_err(|error| MediaProviderError::new(format!("{bus_name}: {error}")))?;
        let metadata: std::collections::HashMap<String, OwnedValue> =
            proxy.get_property("Metadata").await.unwrap_or_default();
        let position_us: i64 = proxy.get_property("Position").await.unwrap_or(0);

        Ok(PlayerSnapshot {
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
        GlobalSystemMediaTransportControlsSessionMediaProperties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    use ::windows::Storage::Streams::DataReader;

    use super::{
        ArtworkError, ArtworkSource, MEDIA_PLAYER_TIMEOUT, MediaMetadataProvider,
        MediaProviderError, PlaybackStatus, PlayerSnapshot, WindowsArtworkSource,
        collect_player_snapshots,
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
            let cancellation = operation.clone();
            self.manager = Some(
                await_provider_operation(operation, move || {
                    let _ = cancellation.Cancel();
                })
                .await?,
            );
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
        let snapshots = session_handles
            .into_iter()
            .map(|session| async move { snapshot_session(&session).await });
        collect_player_snapshots(snapshots, MEDIA_PLAYER_TIMEOUT).await
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
        let operation = session
            .TryGetMediaPropertiesAsync()
            .map_err(provider_error)?;
        let cancellation = operation.clone();
        let properties = await_provider_operation(operation, move || {
            let _ = cancellation.Cancel();
        })
        .await?;
        let timeline = session.GetTimelineProperties().map_err(provider_error)?;
        let position_ms = ticks_to_millis(timeline.Position().map_err(provider_error)?.Duration);
        let start_ticks = timeline.StartTime().map_err(provider_error)?.Duration;
        let end_ticks = timeline.EndTime().map_err(provider_error)?.Duration;
        let duration_ms = ticks_to_millis(end_ticks.saturating_sub(start_ticks));
        let track = properties.Title().map_err(provider_error)?.to_string();
        let artist = properties.Artist().map_err(provider_error)?.to_string();
        let album = properties.AlbumTitle().map_err(provider_error)?.to_string();
        let artwork = properties.Thumbnail().is_ok().then(|| {
            ArtworkSource::native_windows_session(
                session.clone(),
                bus_name.clone(),
                track.clone(),
                artist.clone(),
                album.clone(),
            )
        });

        Ok(PlayerSnapshot {
            bus_name,
            status,
            track,
            artist,
            album,
            artwork,
            position_ms,
            duration_ms,
        })
    }

    pub(super) async fn fetch_thumbnail_bytes(
        session: &GlobalSystemMediaTransportControlsSession,
        source: &WindowsArtworkSource,
        max_bytes: usize,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        let operation = session.TryGetMediaPropertiesAsync().map_err(artwork_io)?;
        let cancellation = operation.clone();
        let properties = await_artwork_operation(
            operation,
            move || {
                let _ = cancellation.Cancel();
            },
            cancel,
        )
        .await?;
        if !properties_match(&properties, &source.track, &source.artist, &source.album)? {
            return Err(ArtworkError::Io(format!(
                "GSMTC session {} changed tracks before thumbnail capture",
                source.source_app_id
            )));
        }
        let operation = properties
            .Thumbnail()
            .map_err(artwork_io)?
            .OpenReadAsync()
            .map_err(artwork_io)?;
        let cancellation = operation.clone();
        let stream = await_artwork_operation(
            operation,
            move || {
                let _ = cancellation.Cancel();
            },
            cancel,
        )
        .await?;
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
        let operation = reader.LoadAsync(count).map_err(artwork_io)?;
        let cancellation = operation.clone();
        let loaded = await_artwork_operation(
            operation,
            move || {
                let _ = cancellation.Cancel();
            },
            cancel,
        )
        .await?;
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

    struct CancelWinRtOperation<C: FnOnce()>(Option<C>);

    impl<C: FnOnce()> CancelWinRtOperation<C> {
        const fn new(cancel: C) -> Self {
            Self(Some(cancel))
        }

        fn disarm(&mut self) {
            self.0.take();
        }

        fn cancel(&mut self) {
            if let Some(cancel) = self.0.take() {
                cancel();
            }
        }
    }

    impl<C: FnOnce()> Drop for CancelWinRtOperation<C> {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    async fn await_provider_operation<T, O, C>(
        operation: O,
        cancel: C,
    ) -> std::result::Result<T, MediaProviderError>
    where
        O: std::future::IntoFuture<Output = ::windows::core::Result<T>>,
        C: FnOnce(),
    {
        let mut cancel_on_drop = CancelWinRtOperation::new(cancel);
        let result = std::future::IntoFuture::into_future(operation).await;
        cancel_on_drop.disarm();
        result.map_err(provider_error)
    }

    async fn await_artwork_operation<T, O, C>(
        operation: O,
        cancel_operation: C,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> std::result::Result<T, ArtworkError>
    where
        O: std::future::IntoFuture<Output = ::windows::core::Result<T>>,
        C: FnOnce(),
    {
        let mut cancel_on_drop = CancelWinRtOperation::new(cancel_operation);
        let operation = std::future::IntoFuture::into_future(operation);
        tokio::pin!(operation);
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                cancel_on_drop.cancel();
                Err(ArtworkError::Cancelled)
            },
            result = &mut operation => {
                cancel_on_drop.disarm();
                result.map_err(artwork_io)
            },
        }
    }

    fn properties_match(
        properties: &GlobalSystemMediaTransportControlsSessionMediaProperties,
        track: &str,
        artist: &str,
        album: &str,
    ) -> std::result::Result<bool, ArtworkError> {
        let candidate_track = properties.Title().map_err(artwork_io)?.to_string();
        let candidate_artist = properties.Artist().map_err(artwork_io)?.to_string();
        let candidate_album = properties.AlbumTitle().map_err(artwork_io)?.to_string();
        Ok(candidate_track == track && candidate_artist == artist && candidate_album == album)
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
