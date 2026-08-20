//! Supported macOS now-playing metadata adapters.
//!
//! macOS has no public system-global now-playing reader. This crate observes
//! only already-running applications with public Apple Event scripting
//! dictionaries. It never binds the private MediaRemote framework and never
//! launches a media application as a side effect of polling.

mod shared;

pub use shared::{
    AdapterFailure, Artwork, AutomationBackend, Capability, DeferredArtworkLoader,
    DeferredArtworkSource, LoadedArtwork, MediaAdapter, MediaError, MediaErrorKind,
    MediaPlayerSnapshot, MediaPoll, MediaProvider, PlaybackStatus,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(target_os = "macos")]
use macos::NativeAutomationBackend;
#[cfg(not(target_os = "macos"))]
use stubs::NativeAutomationBackend;

impl MediaProvider {
    /// Construct the provider for the current platform.
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Box::new(NativeAutomationBackend::new()))
    }
}

impl Default for MediaProvider {
    fn default() -> Self {
        Self::new()
    }
}
