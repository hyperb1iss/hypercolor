//! Runtime state for actively playing playlists.

use tokio::sync::watch;
use tokio::task::JoinHandle;

use hypercolor_types::library::PlaylistId;

/// In-memory runtime slot for the currently active playlist sequence.
pub struct PlaylistRuntimeState {
    /// Active playlist worker, if any.
    pub active: Option<ActivePlaylistRuntime>,
    next_generation: u64,
}

impl PlaylistRuntimeState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: None,
            next_generation: 1,
        }
    }

    /// Allocate a monotonic generation token for a newly started worker.
    pub fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        generation
    }
}

impl Default for PlaylistRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle and metadata for a running playlist sequence.
pub struct ActivePlaylistRuntime {
    pub generation: u64,
    pub playlist_id: PlaylistId,
    pub playlist_name: String,
    pub loop_enabled: bool,
    pub item_count: usize,
    pub started_at_ms: u64,
    pub stop_tx: watch::Sender<bool>,
    pub task: JoinHandle<()>,
}
