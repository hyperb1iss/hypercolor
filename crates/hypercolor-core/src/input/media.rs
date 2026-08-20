//! Cross-platform now-playing input.
//!
//! Linux reads MPRIS over the session bus and Windows reads Global System
//! Media Transport Controls (GSMTC). Metadata polling and artwork enrichment
//! are independent: a slow or hostile image can never delay the next player
//! snapshot.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
#[cfg(target_os = "macos")]
use std::fmt::Write as _;
use std::future::Future;
use std::io::{Cursor, Read};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use futures_util::{
    FutureExt as _,
    stream::{self, FuturesUnordered, StreamExt as _},
};
#[cfg(target_os = "macos")]
use sha2::{Digest as _, Sha256};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use hypercolor_types::media::MediaState;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use super::SourceSessionWriter;
use super::traits::{
    DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, SourceRoleBinding,
};
use super::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};
use hypercolor_worker_retention::{retain_worker, spawn_worker};

/// Poll cadence for player discovery, status, and position.
pub const MEDIA_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum time allowed for one provider connection or metadata poll.
pub const MEDIA_PROVIDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum time one native player may consume while producing a snapshot.
pub const MEDIA_PLAYER_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum native player metadata operations allowed in flight at once.
pub const MAX_CONCURRENT_MEDIA_PLAYER_POLLS: usize = 8;

/// Maximum age of a successful player snapshot retained across scan ticks.
pub const MEDIA_PLAYER_CACHE_TTL: Duration = Duration::from_secs(30);

/// Initial wait for a newly discovered player before publishing an empty cache.
const MEDIA_PLAYER_INITIAL_WAIT: Duration = Duration::from_millis(100);

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

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
    /// Bounded bytes captured with the metadata snapshot that named them.
    Bytes { identity: String, data: Arc<[u8]> },
    /// Platform-owned work resolved after the metadata snapshot is published.
    Deferred(DeferredArtworkSource),
    /// Platform-owned blocking work resolved under the shared artwork gate.
    DeferredBlocking(BlockingDeferredArtworkSource),
}

type DeferredArtworkFuture<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<DeferredArtworkPayload, ArtworkError>> + 'a>>;

enum DeferredArtworkPayload {
    Url(String),
    Bytes(Vec<u8>),
}

trait DeferredArtworkLoader: Send + Sync {
    fn load<'a>(
        &'a self,
        max_bytes: usize,
        cancel: &'a CancellationToken,
    ) -> DeferredArtworkFuture<'a>;
}

/// Opaque identity and platform loader for deferred artwork.
#[derive(Clone)]
pub struct DeferredArtworkSource {
    identity: String,
    loader: Arc<dyn DeferredArtworkLoader>,
}

impl fmt::Debug for DeferredArtworkSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeferredArtworkSource")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PartialEq for DeferredArtworkSource {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for DeferredArtworkSource {}

trait BlockingDeferredArtworkLoader: Send + Sync {
    fn load(
        &self,
        max_bytes: usize,
        cancel: &CancellationToken,
    ) -> std::result::Result<DeferredArtworkPayload, ArtworkError>;
}

/// Opaque identity and blocking platform loader for deferred artwork.
#[derive(Clone)]
pub struct BlockingDeferredArtworkSource {
    identity: String,
    loader: Arc<dyn BlockingDeferredArtworkLoader>,
}

impl fmt::Debug for BlockingDeferredArtworkSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BlockingDeferredArtworkSource")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PartialEq for BlockingDeferredArtworkSource {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for BlockingDeferredArtworkSource {}

impl ArtworkSource {
    /// Build a platform-neutral embedded artwork fixture.
    #[doc(hidden)]
    #[must_use]
    pub fn embedded(identity: impl Into<String>, data: impl Into<Arc<[u8]>>) -> Self {
        Self::Bytes {
            identity: identity.into(),
            data: data.into(),
        }
    }

    #[cfg(target_os = "windows")]
    fn deferred(identity: impl Into<String>, loader: Arc<dyn DeferredArtworkLoader>) -> Self {
        Self::Deferred(DeferredArtworkSource {
            identity: identity.into(),
            loader,
        })
    }

    #[cfg(target_os = "macos")]
    fn deferred_blocking(
        identity: impl Into<String>,
        loader: Arc<dyn BlockingDeferredArtworkLoader>,
    ) -> Self {
        Self::DeferredBlocking(BlockingDeferredArtworkSource {
            identity: identity.into(),
            loader,
        })
    }

    fn cache_key(&self) -> String {
        match self {
            Self::Url(value) => value.clone(),
            Self::Bytes { identity, .. } => identity.clone(),
            Self::Deferred(source) => source.identity.clone(),
            Self::DeferredBlocking(source) => source.identity.clone(),
        }
    }
}

#[cfg(target_os = "macos")]
fn embedded_artwork_identity(namespace: &str, data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut identity = String::with_capacity(namespace.len() + 65);
    identity.push_str(namespace);
    identity.push('\u{1f}');
    for byte in digest {
        write!(&mut identity, "{byte:02x}").expect("writing to String cannot fail");
    }
    identity
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
    kind: MediaProviderErrorKind,
    message: String,
}

/// Neutral failure class retained across native media providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaProviderErrorKind {
    /// Provider-specific failure without a narrower neutral class.
    BackendFailure,
    /// The current process or platform cannot use the provider.
    UnsupportedCapability,
    /// Provider access needs an explicit user authorization action.
    AuthorizationRequired,
    /// The user or operating system denied provider access.
    AuthorizationDenied,
    /// No supported media application is currently running.
    NoRunningPlayer,
    /// The target application changed or exited during the operation.
    StaleTarget,
    /// The provider exceeded its bounded operation deadline.
    TimedOut,
    /// One application adapter failed independently.
    AdapterFailure,
    /// The provider session is disconnected.
    Disconnected,
}

impl MediaProviderError {
    /// Wrap a platform error without exposing platform-specific types.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: MediaProviderErrorKind::BackendFailure,
            message: message.into(),
        }
    }

    /// Preserve a neutral provider failure class across the platform seam.
    #[must_use]
    pub fn classified(kind: MediaProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Return the neutral failure class.
    #[must_use]
    pub const fn kind(&self) -> MediaProviderErrorKind {
        self.kind
    }

    fn issue_code(&self, fallback: &'static str) -> &'static str {
        match self.kind {
            MediaProviderErrorKind::BackendFailure => fallback,
            MediaProviderErrorKind::UnsupportedCapability => "media_provider_unsupported",
            MediaProviderErrorKind::AuthorizationRequired => "authorization_required",
            MediaProviderErrorKind::AuthorizationDenied => "authorization_denied",
            MediaProviderErrorKind::NoRunningPlayer => "media_player_unavailable",
            MediaProviderErrorKind::StaleTarget => "media_target_stale",
            MediaProviderErrorKind::TimedOut => "media_poll_timed_out",
            MediaProviderErrorKind::AdapterFailure => "media_adapter_failed",
            MediaProviderErrorKind::Disconnected => "media_provider_disconnected",
        }
    }

    fn requires_user_action(&self) -> bool {
        matches!(
            self.kind,
            MediaProviderErrorKind::AuthorizationRequired
                | MediaProviderErrorKind::AuthorizationDenied
        )
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
    let snapshots = snapshots.into_iter().enumerate().collect::<Vec<_>>();
    let attempted = snapshots.len();
    let mut ordered = std::iter::repeat_with(|| None)
        .take(attempted)
        .collect::<Vec<_>>();
    let mut failures = std::iter::repeat_with(|| None)
        .take(attempted)
        .collect::<Vec<_>>();

    let mut pending = stream::iter(snapshots.into_iter().map(|(index, snapshot)| async move {
        (index, tokio::time::timeout(timeout, snapshot).await)
    }))
    .buffer_unordered(MAX_CONCURRENT_MEDIA_PLAYER_POLLS);
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PlayerScanToken {
    key: String,
    incarnation: u64,
}

struct DiscoveredPlayer<I> {
    identity: I,
    incarnation: u64,
    seen_generation: u64,
}

struct CachedPlayer {
    snapshot: PlayerSnapshot,
    incarnation: u64,
    refreshed_at: tokio::time::Instant,
}

type PlayerScanFuture = Pin<
    Box<
        dyn Future<
                Output = (
                    PlayerScanToken,
                    std::result::Result<PlayerSnapshot, MediaProviderError>,
                    tokio::time::Instant,
                ),
            > + Send,
    >,
>;

/// Persistent bounded scheduler for native player metadata operations.
#[doc(hidden)]
pub struct PlayerSnapshotScanner<I> {
    discovered: HashMap<String, DiscoveredPlayer<I>>,
    order: Vec<String>,
    queue: VecDeque<PlayerScanToken>,
    queued: HashSet<PlayerScanToken>,
    in_flight: FuturesUnordered<PlayerScanFuture>,
    in_flight_tokens: HashSet<PlayerScanToken>,
    abort_handles: HashMap<PlayerScanToken, tokio::task::AbortHandle>,
    cache: HashMap<String, CachedPlayer>,
    failures: HashMap<PlayerScanToken, MediaProviderError>,
    poll_gate: Arc<Semaphore>,
    discovery_generation: u64,
    next_incarnation: u64,
}

impl<I> Default for PlayerSnapshotScanner<I> {
    fn default() -> Self {
        Self {
            discovered: HashMap::new(),
            order: Vec::new(),
            queue: VecDeque::new(),
            queued: HashSet::new(),
            in_flight: FuturesUnordered::new(),
            in_flight_tokens: HashSet::new(),
            abort_handles: HashMap::new(),
            cache: HashMap::new(),
            failures: HashMap::new(),
            poll_gate: Arc::new(Semaphore::new(MAX_CONCURRENT_MEDIA_PLAYER_POLLS)),
            discovery_generation: 0,
            next_incarnation: 0,
        }
    }
}

impl<I> Drop for PlayerSnapshotScanner<I> {
    fn drop(&mut self) {
        for handle in self.abort_handles.values() {
            handle.abort();
        }
    }
}

impl<I: Clone + PartialEq + 'static> PlayerSnapshotScanner<I> {
    /// Reconcile one completed discovery and advance its fair scan queue.
    pub async fn poll<F, Fut>(
        &mut self,
        discovered: impl IntoIterator<Item = (String, I)>,
        mut snapshot: F,
    ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError>
    where
        F: FnMut(I) -> Fut,
        Fut: Future<Output = std::result::Result<PlayerSnapshot, MediaProviderError>>
            + Send
            + 'static,
    {
        self.reconcile(discovered);
        self.evict_stale(tokio::time::Instant::now());

        self.fill(&mut snapshot);
        let had_fresh_cache = self.has_fresh_cache(tokio::time::Instant::now());
        let mut completions = 0;

        loop {
            let result = if had_fresh_cache || completions != 0 {
                let Some(result) = self.in_flight.next().now_or_never() else {
                    break;
                };
                result
            } else {
                match tokio::time::timeout(MEDIA_PLAYER_INITIAL_WAIT, self.in_flight.next()).await {
                    Ok(result) => result,
                    Err(_) => break,
                }
            };
            let Some((token, result, completed_at)) = result else {
                break;
            };
            completions += 1;
            self.finish(token, result, completed_at);
            if !had_fresh_cache && self.has_fresh_cache(tokio::time::Instant::now()) {
                break;
            }
        }

        self.fill(&mut snapshot);
        self.fresh_snapshots_or_failure(tokio::time::Instant::now())
    }

    /// Cancel all retained operations and cached native state.
    pub fn clear(&mut self) {
        for handle in self.abort_handles.values() {
            handle.abort();
        }
        self.discovered.clear();
        self.order.clear();
        self.queue.clear();
        self.queued.clear();
        self.in_flight = FuturesUnordered::new();
        self.in_flight_tokens.clear();
        self.abort_handles.clear();
        self.cache.clear();
        self.failures.clear();
        self.poll_gate = Arc::new(Semaphore::new(MAX_CONCURRENT_MEDIA_PLAYER_POLLS));
    }

    fn reconcile(&mut self, discovered: impl IntoIterator<Item = (String, I)>) {
        self.discovery_generation = self
            .discovery_generation
            .checked_add(1)
            .expect("media discovery generation exhausted");
        let generation = self.discovery_generation;
        let mut order = Vec::new();
        let mut present = HashSet::new();

        for (key, identity) in discovered {
            if !present.insert(key.clone()) {
                continue;
            }
            order.push(key.clone());
            if self
                .discovered
                .get(&key)
                .is_some_and(|existing| existing.identity == identity)
            {
                let existing = self
                    .discovered
                    .get_mut(&key)
                    .expect("matching media player remains discovered");
                existing.identity = identity;
                existing.seen_generation = generation;
                continue;
            }
            self.next_incarnation = self
                .next_incarnation
                .checked_add(1)
                .expect("media player incarnation exhausted");
            let incarnation = self.next_incarnation;
            self.discovered.insert(
                key.clone(),
                DiscoveredPlayer {
                    identity,
                    incarnation,
                    seen_generation: generation,
                },
            );
            self.enqueue(PlayerScanToken { key, incarnation });
        }

        self.order = order;
        self.discovered
            .retain(|_, player| player.seen_generation == generation);
        self.cache.retain(|key, cached| {
            self.discovered
                .get(key)
                .is_some_and(|player| player.incarnation == cached.incarnation)
        });
        self.failures.retain(|token, _| {
            self.discovered
                .get(&token.key)
                .is_some_and(|player| player.incarnation == token.incarnation)
        });
        self.queue.retain(|token| {
            self.discovered
                .get(&token.key)
                .is_some_and(|player| player.incarnation == token.incarnation)
        });
        self.queued = self.queue.iter().cloned().collect();
        self.abort_handles.retain(|token, handle| {
            let active = self
                .discovered
                .get(&token.key)
                .is_some_and(|player| player.incarnation == token.incarnation);
            if !active {
                handle.abort();
            }
            active
        });
        self.in_flight_tokens.retain(|token| {
            self.discovered
                .get(&token.key)
                .is_some_and(|player| player.incarnation == token.incarnation)
        });
    }

    fn enqueue(&mut self, token: PlayerScanToken) {
        if self.queued.insert(token.clone()) && !self.in_flight_tokens.contains(&token) {
            self.queue.push_back(token);
        }
    }

    fn fill<F, Fut>(&mut self, snapshot: &mut F)
    where
        F: FnMut(I) -> Fut,
        Fut: Future<Output = std::result::Result<PlayerSnapshot, MediaProviderError>>
            + Send
            + 'static,
    {
        while let Some(token) = self.queue.pop_front() {
            self.queued.remove(&token);
            let Some(player) = self.discovered.get(&token.key) else {
                continue;
            };
            if player.incarnation != token.incarnation {
                continue;
            }
            let identity = player.identity.clone();
            let future = snapshot(identity);
            let result_token = token.clone();
            let poll_gate = Arc::clone(&self.poll_gate);
            let task = tokio::spawn(async move {
                let _permit = poll_gate
                    .acquire_owned()
                    .await
                    .expect("media player poll gate remains open");
                let result = tokio::time::timeout(MEDIA_PLAYER_TIMEOUT, future)
                    .await
                    .unwrap_or_else(|_| {
                        Err(MediaProviderError::new(format!(
                            "native media player {} exceeded {:?}",
                            result_token.key, MEDIA_PLAYER_TIMEOUT
                        )))
                    });
                (result_token, result, tokio::time::Instant::now())
            });
            self.abort_handles
                .insert(token.clone(), task.abort_handle());
            let join_token = token.clone();
            self.in_flight.push(Box::pin(async move {
                task.await.unwrap_or_else(|error| {
                    (
                        join_token,
                        Err(MediaProviderError::new(format!(
                            "native media player task failed: {error}"
                        ))),
                        tokio::time::Instant::now(),
                    )
                })
            }));
            self.in_flight_tokens.insert(token);
        }
    }

    fn finish(
        &mut self,
        token: PlayerScanToken,
        result: std::result::Result<PlayerSnapshot, MediaProviderError>,
        completed_at: tokio::time::Instant,
    ) {
        self.in_flight_tokens.remove(&token);
        self.abort_handles.remove(&token);
        let active = self
            .discovered
            .get(&token.key)
            .is_some_and(|player| player.incarnation == token.incarnation);
        if !active {
            return;
        }
        match result {
            Ok(snapshot) => {
                self.failures.remove(&token);
                self.cache.insert(
                    token.key.clone(),
                    CachedPlayer {
                        snapshot,
                        incarnation: token.incarnation,
                        refreshed_at: completed_at,
                    },
                );
            }
            Err(error) => {
                debug!(player = %token.key, %error, "Skipped unhealthy media player");
                self.failures.insert(token.clone(), error);
            }
        }
        self.enqueue(token);
    }

    fn evict_stale(&mut self, now: tokio::time::Instant) {
        self.cache.retain(|_, cached| {
            now.saturating_duration_since(cached.refreshed_at) <= MEDIA_PLAYER_CACHE_TTL
        });
    }

    fn has_fresh_cache(&self, now: tokio::time::Instant) -> bool {
        self.cache.values().any(|cached| {
            now.saturating_duration_since(cached.refreshed_at) <= MEDIA_PLAYER_CACHE_TTL
        })
    }

    fn fresh_snapshots(&self, now: tokio::time::Instant) -> Vec<PlayerSnapshot> {
        self.order
            .iter()
            .filter_map(|key| {
                let player = self.discovered.get(key)?;
                if self.failures.contains_key(&PlayerScanToken {
                    key: key.clone(),
                    incarnation: player.incarnation,
                }) {
                    return None;
                }
                let cached = self.cache.get(key)?;
                (cached.incarnation == player.incarnation
                    && now.saturating_duration_since(cached.refreshed_at) <= MEDIA_PLAYER_CACHE_TTL)
                    .then(|| cached.snapshot.clone())
            })
            .collect()
    }

    fn fresh_snapshots_or_failure(
        &self,
        now: tokio::time::Instant,
    ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
        let snapshots = self.fresh_snapshots(now);
        if self.order.is_empty() {
            return Ok(snapshots);
        }

        let all_failed = self.order.iter().all(|key| {
            self.discovered.get(key).is_some_and(|player| {
                self.failures.contains_key(&PlayerScanToken {
                    key: key.clone(),
                    incarnation: player.incarnation,
                })
            })
        });
        if !all_failed {
            return Ok(snapshots);
        }

        let first_failure = self.order.iter().find_map(|key| {
            let player = self.discovered.get(key)?;
            self.failures
                .get(&PlayerScanToken {
                    key: key.clone(),
                    incarnation: player.incarnation,
                })
                .cloned()
        });
        Err(first_failure.expect("every discovered media player recorded a failure"))
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
    fn take_poll_warning(&mut self) -> Option<MediaProviderError> {
        None
    }
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
    pub warning: Option<MediaProviderError>,
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
        let warning = self.provider.take_poll_warning();
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
            warning,
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
        Self::build(
            policy,
            Arc::new(NativeArtworkBlockingBackend),
            process_artwork_gate(),
        )
    }

    /// Build a fetcher with an isolated blocking backend for deterministic tests.
    #[doc(hidden)]
    pub fn with_isolated_blocking_backend_for_test(
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

    /// Whether two fetchers share the process-wide blocking-work gate.
    #[doc(hidden)]
    #[must_use]
    pub fn shares_process_gate_for_test(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.blocking_gate, &other.blocking_gate)
            && Arc::ptr_eq(&self.blocking_gate, &process_artwork_gate())
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
            let payload = match source {
                ArtworkSource::Url(url) => DeferredArtworkPayload::Url(url.clone()),
                ArtworkSource::Bytes { data, .. } => {
                    if data.len() > self.policy.max_source_bytes {
                        return Err(ArtworkError::SourceTooLarge {
                            limit: self.policy.max_source_bytes,
                        });
                    }
                    DeferredArtworkPayload::Bytes(data.to_vec())
                }
                ArtworkSource::Deferred(source) => {
                    source
                        .loader
                        .load(self.policy.max_source_bytes, &operation_cancel)
                        .await?
                }
                ArtworkSource::DeferredBlocking(source) => {
                    let loader = Arc::clone(&source.loader);
                    let max_bytes = self.policy.max_source_bytes;
                    let blocking_cancel = operation_cancel.clone();
                    self.run_blocking(
                        "platform media artwork loader",
                        &operation_cancel,
                        move || loader.load(max_bytes, &blocking_cancel),
                    )
                    .await?
                }
            };
            let bytes = match payload {
                DeferredArtworkPayload::Url(url) => self.load_url(&url, &operation_cancel).await?,
                DeferredArtworkPayload::Bytes(bytes) => bytes,
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
            spawn_worker(
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
                    retain_worker(
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
        Self::new(ArtworkPolicy::default()).expect("constant artwork policy builds a client")
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
    health_issue: Option<SourceIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaPublicationKind {
    BackendSuccess,
    StateUpdate,
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
        self.publish_metadata(state, key, completed_at, None)
    }

    /// Publish a degraded backend sample through the production status path.
    #[doc(hidden)]
    #[must_use]
    pub fn publish_degraded_completed_for_test(
        &self,
        state: MediaState,
        completed_at: Instant,
        issue: SourceIssue,
    ) -> bool {
        let key = state.available.then(|| state.track_key());
        self.publish_metadata(state, key, completed_at, Some(issue))
    }

    fn publish_metadata(
        &self,
        mut state: MediaState,
        artwork_key: Option<String>,
        completed_at: Instant,
        health_issue: Option<SourceIssue>,
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
        self.publish(
            state,
            completed_at,
            MediaPublicationKind::BackendSuccess,
            health_issue,
        );
        drop(publication);
        true
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
            None,
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
        self.publish(
            state,
            Instant::now(),
            MediaPublicationKind::StateUpdate,
            None,
        );
        drop(publication);
    }

    fn publish(
        &self,
        state: MediaState,
        completed_at: Instant,
        kind: MediaPublicationKind,
        health_issue: Option<SourceIssue>,
    ) {
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
            health_issue,
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

/// Now-playing input source backed by native platform providers.
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
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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

        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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

        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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

        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
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
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
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
            let freshness_deadline = poll.completed_at + MEDIA_POLL_INTERVAL + MEDIA_POLL_INTERVAL;
            if let Some(issue) = &poll.health_issue {
                status.record_degraded_sample(
                    poll.completed_at,
                    freshness_deadline,
                    usize::from(poll.state.available),
                    issue.clone(),
                )?;
            } else {
                status.record_sample(
                    poll.completed_at,
                    freshness_deadline,
                    usize::from(poll.state.available),
                )?;
            }
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

impl SourceRoleBinding for MediaSource {
    type Role = DataSourceRole;
}

impl DataSource for MediaSource {
    fn data_source_kind(&self) -> DataSourceKind {
        DataSourceKind::Media
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
    #[cfg(target_os = "macos")]
    {
        return "macOS Media Automation";
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
    #[cfg(target_os = "macos")]
    {
        return "macos_automation";
    }
    #[allow(unreachable_code)]
    "unsupported"
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
mod worker {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use anyhow::{Context, Result};
    use tokio::time::MissedTickBehavior;
    use tracing::{debug, warn};

    use hypercolor_worker_retention::{retain_worker, spawn_worker};

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
            let join_handle = spawn_worker(
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
            retain_worker(join_handle, "media poller");
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
                            let health_issue = metadata.warning.as_ref().map(|warning| {
                                SourceIssue::new(
                                    warning.issue_code("media_adapter_failed"),
                                    warning.to_string(),
                                    true,
                                )
                            });
                            if let Some(status) = &status {
                                if let Some(issue) = &health_issue {
                                    let warning = metadata
                                        .warning
                                        .as_ref()
                                        .expect("health issue retains its provider warning");
                                    status.set_action_issue(
                                        warning.requires_user_action().then(|| issue.clone()),
                                    );
                                    status.degraded(issue.clone());
                                } else {
                                    status.set_action_issue(None);
                                }
                            }
                            let key = metadata.artwork.as_ref().map(|request| request.key.clone());
                            let _ = publisher.publish_metadata(
                                metadata.state,
                                key,
                                std::time::Instant::now(),
                                health_issue,
                            );
                            if metadata.artwork != last_artwork {
                                last_artwork.clone_from(&metadata.artwork);
                                art_tx.send_replace(metadata.artwork.map(Arc::new));
                            }
                        }
                        Ok(Err(MediaProviderFailure::Connect(error))) => {
                            if let Some(status) = &status {
                                let issue = SourceIssue::new(
                                    error.issue_code("media_provider_unavailable"),
                                    error.to_string(),
                                    true,
                                )
                                .with_remediation(native_remediation());
                                status.set_action_issue(
                                    error.requires_user_action().then(|| issue.clone()),
                                );
                                status.unavailable(issue);
                            }
                            let _ = publisher.publish_unavailable(std::time::Instant::now());
                            if last_artwork.take().is_some() {
                                art_tx.send_replace(None);
                            }
                        }
                        Ok(Err(MediaProviderFailure::Poll(error))) => {
                            if let Some(status) = &status {
                                let issue = SourceIssue::new(
                                    error.issue_code("media_poll_failed"),
                                    error.to_string(),
                                    true,
                                );
                                status.set_action_issue(
                                    error.requires_user_action().then(|| issue.clone()),
                                );
                                status.degraded(issue);
                            }
                            let _ = publisher.publish_unavailable(std::time::Instant::now());
                            if last_artwork.take().is_some() {
                                art_tx.send_replace(None);
                            }
                        }
                        Err(_) => {
                            session.disconnect();
                            if let Some(status) = &status {
                                status.set_action_issue(None);
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
        #[cfg(target_os = "macos")]
        {
            return "run Music or Spotify, then grant Automation from an explicit Hypercolor action";
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

#[cfg(target_os = "macos")]
fn native_provider() -> Box<dyn MediaMetadataProvider> {
    Box::new(macos::AutomationProvider::new())
}

#[cfg(target_os = "macos")]
mod macos {
    use hypercolor_macos_media::{
        AdapterFailure, Artwork, LoadedArtwork, MediaErrorKind, MediaPoll, MediaProvider,
        PlaybackStatus as NativePlaybackStatus,
    };

    use super::{
        ArtworkError, ArtworkSource, BlockingDeferredArtworkLoader, DeferredArtworkPayload,
        MediaMetadataProvider, MediaProviderError, MediaProviderErrorKind, PlaybackStatus,
        PlayerSnapshot, embedded_artwork_identity,
    };

    pub(super) struct AutomationProvider {
        provider: MediaProvider,
        poll_warning: Option<MediaProviderError>,
    }

    impl AutomationProvider {
        pub(super) fn new() -> Self {
            Self {
                provider: MediaProvider::new(),
                poll_warning: None,
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl MediaMetadataProvider for AutomationProvider {
        fn backend_name(&self) -> &'static str {
            "macos_automation"
        }

        async fn connect(&mut self) -> std::result::Result<(), MediaProviderError> {
            self.provider.connect().map_err(provider_error)
        }

        async fn poll_players(
            &mut self,
        ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
            self.poll_warning = None;
            let (players, failures) = match self.provider.poll_players().map_err(provider_error)? {
                MediaPoll::Players { players, failures } => (players, failures),
                MediaPoll::NoRunningCapablePlayer => {
                    return Err(no_running_player_error());
                }
            };
            for failure in &failures {
                tracing::debug!(
                    adapter = failure.adapter.display_name(),
                    error = %failure.error,
                    "macOS media adapter failed"
                );
            }
            if players.is_empty() && !failures.is_empty() {
                let failure = summarize_failures(&failures);
                return Err(MediaProviderError::classified(
                    failure.kind(),
                    format!("all running macOS media adapters failed: {failure}"),
                ));
            }
            if !failures.is_empty() {
                self.poll_warning = Some(summarize_failures(&failures));
            }
            Ok(players.into_iter().map(map_player).collect())
        }

        fn take_poll_warning(&mut self) -> Option<MediaProviderError> {
            self.poll_warning.take()
        }

        fn disconnect(&mut self) {
            self.provider.disconnect();
        }
    }

    fn map_player(player: hypercolor_macos_media::MediaPlayerSnapshot) -> PlayerSnapshot {
        let bus_name = format!("{}\u{1f}{}", player.player_id, player.track_id);
        let artwork = player.artwork.map(|artwork| match artwork {
            Artwork::Url(url) => ArtworkSource::Url(url),
            Artwork::Bytes { identity, data } => ArtworkSource::Bytes {
                identity: embedded_artwork_identity(
                    &format!(
                        "{}\u{1f}{}\u{1f}{identity}",
                        player.player_id, player.track_id
                    ),
                    &data,
                ),
                data,
            },
            Artwork::Deferred(source) => ArtworkSource::deferred_blocking(
                source.identity().to_owned(),
                std::sync::Arc::new(MacosArtworkLoader { source }),
            ),
        });
        PlayerSnapshot {
            bus_name,
            status: match player.status {
                NativePlaybackStatus::Playing => PlaybackStatus::Playing,
                NativePlaybackStatus::Paused => PlaybackStatus::Paused,
                NativePlaybackStatus::Stopped => PlaybackStatus::Stopped,
            },
            track: player.track,
            artist: player.artist,
            album: player.album,
            artwork,
            position_ms: player.position_ms,
            duration_ms: player.duration_ms,
        }
    }

    struct MacosArtworkLoader {
        source: hypercolor_macos_media::DeferredArtworkSource,
    }

    impl BlockingDeferredArtworkLoader for MacosArtworkLoader {
        fn load(
            &self,
            max_bytes: usize,
            cancel: &tokio_util::sync::CancellationToken,
        ) -> std::result::Result<DeferredArtworkPayload, ArtworkError> {
            if cancel.is_cancelled() {
                return Err(ArtworkError::Cancelled);
            }
            let artwork = self
                .source
                .load(max_bytes)
                .map_err(|error| ArtworkError::Io(error.to_string()))?
                .ok_or(ArtworkError::UnsupportedSource)?;
            if cancel.is_cancelled() {
                return Err(ArtworkError::Cancelled);
            }
            Ok(match artwork {
                LoadedArtwork::Url(url) => DeferredArtworkPayload::Url(url),
                LoadedArtwork::Bytes { data, .. } => DeferredArtworkPayload::Bytes(data.to_vec()),
            })
        }
    }

    fn provider_error(error: hypercolor_macos_media::MediaError) -> MediaProviderError {
        MediaProviderError::classified(map_error_kind(error.kind()), error.to_string())
    }

    fn no_running_player_error() -> MediaProviderError {
        MediaProviderError::classified(
            MediaProviderErrorKind::NoRunningPlayer,
            "no supported macOS media application is running",
        )
    }

    fn summarize_failures(failures: &[AdapterFailure]) -> MediaProviderError {
        let first = failures
            .first()
            .expect("adapter failure summary requires one failure");
        let representative = failures
            .iter()
            .find(|failure| {
                matches!(
                    failure.error.kind(),
                    MediaErrorKind::AuthorizationRequired | MediaErrorKind::AuthorizationDenied
                )
            })
            .unwrap_or(first);
        let message = failures
            .iter()
            .map(|failure| format!("{}: {}", failure.adapter.display_name(), failure.error))
            .collect::<Vec<_>>()
            .join("; ");
        MediaProviderError::classified(map_error_kind(representative.error.kind()), message)
    }

    const fn map_error_kind(kind: MediaErrorKind) -> MediaProviderErrorKind {
        match kind {
            MediaErrorKind::UnsupportedCapability => MediaProviderErrorKind::UnsupportedCapability,
            MediaErrorKind::AuthorizationRequired => MediaProviderErrorKind::AuthorizationRequired,
            MediaErrorKind::AuthorizationDenied => MediaProviderErrorKind::AuthorizationDenied,
            MediaErrorKind::NoRunningCapablePlayer => MediaProviderErrorKind::NoRunningPlayer,
            MediaErrorKind::StaleTarget => MediaProviderErrorKind::StaleTarget,
            MediaErrorKind::TimedOut => MediaProviderErrorKind::TimedOut,
            MediaErrorKind::AdapterFailure => MediaProviderErrorKind::AdapterFailure,
            MediaErrorKind::Disconnected => MediaProviderErrorKind::Disconnected,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::no_running_player_error;
        use crate::input::media::MediaProviderErrorKind;

        #[test]
        fn no_running_player_keeps_its_neutral_status_class() {
            let error = no_running_player_error();

            assert_eq!(error.kind(), MediaProviderErrorKind::NoRunningPlayer);
            assert_eq!(error.issue_code("fallback"), "media_player_unavailable");
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::Context;
    use zbus::zvariant::OwnedValue;

    use super::{
        ArtworkSource, MediaMetadataProvider, MediaProviderError, PlaybackStatus, PlayerSnapshot,
        PlayerSnapshotScanner,
    };

    const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
    const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
    const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

    pub(super) struct MprisProvider {
        connection: Option<zbus::Connection>,
        scanner: PlayerSnapshotScanner<String>,
    }

    impl MprisProvider {
        pub(super) fn new() -> Self {
            Self {
                connection: None,
                scanner: PlayerSnapshotScanner::default(),
            }
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
            poll_players(connection, &mut self.scanner)
                .await
                .map_err(|error| MediaProviderError::new(error.to_string()))
        }

        fn disconnect(&mut self) {
            self.connection = None;
            self.scanner.clear();
        }
    }

    async fn poll_players(
        connection: &zbus::Connection,
        scanner: &mut PlayerSnapshotScanner<String>,
    ) -> anyhow::Result<Vec<PlayerSnapshot>> {
        let dbus = zbus::fdo::DBusProxy::new(connection)
            .await
            .context("failed to create D-Bus proxy")?;
        let names = dbus
            .list_names()
            .await
            .context("failed to list D-Bus names")?;

        let players = names
            .into_iter()
            .map(|name| name.to_string())
            .filter(|name| name.starts_with(MPRIS_PREFIX))
            .map(|name| (name.clone(), name));
        let connection = connection.clone();
        scanner
            .poll(players, move |name| {
                let connection = connection.clone();
                async move { snapshot_player(&connection, &name).await }
            })
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
        ArtworkError, ArtworkSource, DeferredArtworkFuture, DeferredArtworkLoader,
        DeferredArtworkPayload, MediaMetadataProvider, MediaProviderError, PlaybackStatus,
        PlayerSnapshot, PlayerSnapshotScanner,
    };

    pub(super) struct GsmtcProvider {
        manager: Option<GlobalSystemMediaTransportControlsSessionManager>,
        scanner: PlayerSnapshotScanner<GlobalSystemMediaTransportControlsSession>,
    }

    impl GsmtcProvider {
        pub(super) fn new() -> Self {
            Self {
                manager: None,
                scanner: PlayerSnapshotScanner::default(),
            }
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
            snapshot_sessions(manager, &mut self.scanner).await
        }

        fn disconnect(&mut self) {
            self.manager = None;
            self.scanner.clear();
        }
    }

    async fn snapshot_sessions(
        manager: &GlobalSystemMediaTransportControlsSessionManager,
        scanner: &mut PlayerSnapshotScanner<GlobalSystemMediaTransportControlsSession>,
    ) -> std::result::Result<Vec<PlayerSnapshot>, MediaProviderError> {
        let sessions = manager.GetSessions().map_err(provider_error)?;
        let session_handles = (0..sessions.Size().map_err(provider_error)?)
            .map(|index| sessions.GetAt(index).map_err(provider_error))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(sessions);
        let mut discovered = Vec::with_capacity(session_handles.len());
        let mut first_key_error = None;
        for session in session_handles {
            match session_key(&session) {
                Ok(key) => discovered.push((key, session)),
                Err(error) => {
                    tracing::debug!(%error, "Skipped media session without a stable identity");
                    first_key_error.get_or_insert(error);
                }
            }
        }
        if discovered.is_empty()
            && let Some(error) = first_key_error
        {
            return Err(error);
        }
        scanner
            .poll(discovered, |session| async move {
                snapshot_session(&session).await
            })
            .await
    }

    fn session_key(
        session: &GlobalSystemMediaTransportControlsSession,
    ) -> std::result::Result<String, MediaProviderError> {
        use ::windows::core::Interface as _;

        let source_app_id = session
            .SourceAppUserModelId()
            .map_err(provider_error)
            .map(|identity| identity.to_string())?;
        let runtime_identity = session
            .cast::<::windows::core::IUnknown>()
            .map_err(provider_error)?;
        // Retaining the session handle keeps this COM identity alive, so its
        // address cannot be recycled into another live scanner incarnation.
        Ok(format!(
            "{source_app_id}\u{1f}{:p}",
            runtime_identity.as_raw()
        ))
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
            let identity = format!(
                "{}\u{1f}{track}\u{1f}{artist}\u{1f}{album}",
                session_key(session).expect("a scanned GSMTC session retains its identity")
            );
            ArtworkSource::deferred(
                identity,
                std::sync::Arc::new(WindowsArtworkLoader {
                    session: session.clone(),
                    source_app_id: bus_name.clone(),
                    track: track.clone(),
                    artist: artist.clone(),
                    album: album.clone(),
                }),
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

    struct WindowsArtworkLoader {
        session: GlobalSystemMediaTransportControlsSession,
        source_app_id: String,
        track: String,
        artist: String,
        album: String,
    }

    impl DeferredArtworkLoader for WindowsArtworkLoader {
        fn load<'a>(
            &'a self,
            max_bytes: usize,
            cancel: &'a tokio_util::sync::CancellationToken,
        ) -> DeferredArtworkFuture<'a> {
            Box::pin(async move {
                fetch_thumbnail_bytes(self, max_bytes, cancel)
                    .await
                    .map(DeferredArtworkPayload::Bytes)
            })
        }
    }

    async fn fetch_thumbnail_bytes(
        source: &WindowsArtworkLoader,
        max_bytes: usize,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        let operation = source
            .session
            .TryGetMediaPropertiesAsync()
            .map_err(artwork_io)?;
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
