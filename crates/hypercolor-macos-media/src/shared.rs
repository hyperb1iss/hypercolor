use std::fmt;
use std::sync::Arc;

/// One supported, explicitly scripted media application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAdapter {
    Music,
    Spotify,
}

impl MediaAdapter {
    #[must_use]
    pub const fn bundle_id(self) -> &'static str {
        match self {
            Self::Music => "com.apple.Music",
            Self::Spotify => "com.spotify.client",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Music => "Music",
            Self::Spotify => "Spotify",
        }
    }
}

/// Whether this process can safely request media Automation access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Available,
    MissingUsageDescription,
    IneligibleResponsibleBundle,
    UnsupportedPlatform,
}

/// Playback state returned by one application adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

/// Artwork supplied by a supported application scripting dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artwork {
    Url(String),
    Bytes { identity: String, data: Arc<[u8]> },
    Deferred(DeferredArtworkSource),
}

/// Artwork payload acquired after its metadata snapshot is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedArtwork {
    Url(String),
    Bytes { identity: String, data: Arc<[u8]> },
}

/// Platform-owned loader for metadata-independent artwork acquisition.
#[doc(hidden)]
pub trait DeferredArtworkLoader: Send + Sync {
    fn load(&self, max_bytes: usize) -> Result<Option<LoadedArtwork>, MediaError>;
}

/// Stable artwork identity plus its platform-owned deferred loader.
#[derive(Clone)]
pub struct DeferredArtworkSource {
    identity: String,
    loader: Arc<dyn DeferredArtworkLoader>,
}

impl DeferredArtworkSource {
    #[doc(hidden)]
    #[must_use]
    pub fn with_loader(
        identity: impl Into<String>,
        loader: Arc<dyn DeferredArtworkLoader>,
    ) -> Self {
        Self {
            identity: identity.into(),
            loader,
        }
    }

    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn load(&self, max_bytes: usize) -> Result<Option<LoadedArtwork>, MediaError> {
        self.loader.load(max_bytes)
    }
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

/// One application snapshot with stable player and track identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPlayerSnapshot {
    pub player_id: String,
    pub track_id: String,
    pub status: PlaybackStatus,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub artwork: Option<Artwork>,
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Failure class retained across the platform boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaErrorKind {
    UnsupportedCapability,
    AuthorizationRequired,
    AuthorizationDenied,
    NoRunningCapablePlayer,
    StaleTarget,
    TimedOut,
    AdapterFailure,
    Disconnected,
}

/// One typed provider or application-adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct MediaError {
    kind: MediaErrorKind,
    adapter: Option<MediaAdapter>,
    message: String,
}

impl MediaError {
    #[must_use]
    pub fn new(
        kind: MediaErrorKind,
        adapter: Option<MediaAdapter>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            adapter,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> MediaErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn adapter(&self) -> Option<MediaAdapter> {
        self.adapter
    }
}

/// One adapter failed while sibling adapters remained usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFailure {
    pub adapter: MediaAdapter,
    pub error: MediaError,
}

/// One successful provider poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaPoll {
    NoRunningCapablePlayer,
    Players {
        players: Vec<MediaPlayerSnapshot>,
        failures: Vec<AdapterFailure>,
    },
}

/// Backend boundary used by native Automation and deterministic fixtures.
#[doc(hidden)]
pub trait AutomationBackend: Send {
    fn capability(&self) -> Capability;
    fn request_authorization(&mut self, adapter: MediaAdapter) -> Result<(), MediaError>;
    fn connect(&mut self) -> Result<(), MediaError>;
    fn poll(&mut self) -> Result<MediaPoll, MediaError>;
    fn disconnect(&mut self);
}

/// Lifecycle wrapper consumed by the shared core provider session.
pub struct MediaProvider {
    backend: Box<dyn AutomationBackend>,
    connected: bool,
}

impl MediaProvider {
    /// Build a provider around an isolated backend for deterministic fixtures.
    #[doc(hidden)]
    #[must_use]
    pub fn with_backend(backend: Box<dyn AutomationBackend>) -> Self {
        Self {
            backend,
            connected: false,
        }
    }

    #[must_use]
    pub fn capability(&self) -> Capability {
        self.backend.capability()
    }

    /// Explicitly request Automation consent for one already-running adapter.
    pub fn request_authorization(&mut self, adapter: MediaAdapter) -> Result<(), MediaError> {
        self.backend.request_authorization(adapter)
    }

    pub fn connect(&mut self) -> Result<(), MediaError> {
        self.backend.connect()?;
        self.connected = true;
        Ok(())
    }

    pub fn poll_players(&mut self) -> Result<MediaPoll, MediaError> {
        if !self.connected {
            return Err(MediaError::new(
                MediaErrorKind::Disconnected,
                None,
                "macOS media provider is disconnected",
            ));
        }
        self.backend.poll()
    }

    pub fn disconnect(&mut self) {
        self.backend.disconnect();
        self.connected = false;
    }

    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}
