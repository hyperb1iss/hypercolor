//! Input sources — audio, screen capture, and future sensor inputs.
//!
//! This module defines the [`InputSource`] trait for pluggable data sources
//! and the [`InputManager`] that orchestrates them. The render loop calls
//! `sample_all()` each frame to collect fresh data from every active source.

pub mod audio;
pub mod interaction;
pub mod screen;
mod traits;

pub use interaction::InteractionInput;
pub use traits::{InputData, InputSource, InteractionData, KeyboardData, MouseData, ScreenData};

use tracing::{error, info};

// ── InputManager ───────────────────────────────────────────────────────────

/// Orchestrates multiple [`InputSource`] instances.
///
/// Owns a heterogeneous collection of input sources and provides batch
/// lifecycle management. The render loop holds one `InputManager` and
/// calls [`sample_all`] each frame.
///
/// # Example (conceptual)
///
/// ```rust,ignore
/// let mut mgr = InputManager::new();
/// mgr.add_source(Box::new(audio_input));
/// mgr.add_source(Box::new(screen_capture));
/// mgr.start_all()?;
///
/// loop {
///     let samples = mgr.sample_all();
///     // route Audio / Screen data into the pipeline...
/// }
/// ```
pub struct InputManager {
    sources: Vec<Box<dyn InputSource>>,
}

impl InputManager {
    /// Create an empty manager with no sources.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Register a new input source.
    ///
    /// Sources are sampled in registration order. Adding a source does not
    /// start it — call [`start_all`] or start sources individually.
    pub fn add_source(&mut self, source: Box<dyn InputSource>) {
        info!(source = source.name(), "Registered input source");
        self.sources.push(source);
    }

    /// Number of registered input sources.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Snapshot of source names in registration order.
    #[must_use]
    pub fn source_names(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| source.name().to_owned())
            .collect()
    }

    /// Sample every registered source, collecting one [`InputData`] per source.
    ///
    /// Sources that fail to sample emit a warning and produce [`InputData::None`]
    /// for that frame — a single broken source never crashes the render loop.
    pub fn sample_all(&mut self) -> Vec<InputData> {
        self.sources
            .iter_mut()
            .map(|source| {
                source.sample().unwrap_or_else(|err| {
                    error!(source = source.name(), %err, "Input sample failed");
                    InputData::None
                })
            })
            .collect()
    }

    /// Start all registered sources.
    ///
    /// Iterates in registration order. If any source fails to start, previously
    /// started sources are stopped and the first error is returned.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered during startup.
    pub fn start_all(&mut self) -> anyhow::Result<()> {
        for (idx, source) in self.sources.iter_mut().enumerate() {
            if let Err(err) = source.start() {
                error!(source = source.name(), %err, "Failed to start input source");
                // Roll back: stop everything we already started.
                for prev in &mut self.sources[..idx] {
                    prev.stop();
                }
                return Err(err);
            }
            info!(source = source.name(), "Started input source");
        }
        Ok(())
    }

    /// Stop all registered sources. Never fails — errors are logged and swallowed.
    pub fn stop_all(&mut self) {
        for source in &mut self.sources {
            info!(source = source.name(), "Stopping input source");
            source.stop();
        }
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}
