//! Now-playing media state produced by the MPRIS input source and
//! injected into display faces as `engine.media`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Snapshot of the active media player.
///
/// Album art travels as a bounded JPEG data URL and is refreshed only on
/// track change, so cloning this state is cheap in the steady state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MediaState {
    /// Whether any media player is currently reachable.
    pub available: bool,
    pub playing: bool,
    pub track: String,
    pub artist: String,
    pub album: String,
    /// `data:image/jpeg;base64,...` album art bounded to 256x256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_data_url: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Bus identity of the player this state tracks
    /// (e.g. `org.mpris.MediaPlayer2.spotify`).
    pub player: String,
}

impl MediaState {
    /// State representing "no player available".
    #[must_use]
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// Identity of the playing track, used to gate art refreshes.
    #[must_use]
    pub fn track_key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}", self.player, self.artist, self.track)
    }
}
