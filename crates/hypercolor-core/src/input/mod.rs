//! Input sources — audio, screen capture, and future sensor inputs.
//!
//! This module defines the [`InputSource`] trait for pluggable data sources
//! and the [`InputManager`] that orchestrates them. The render loop calls
//! `sample_all()` each frame to collect fresh data from every active source.

pub mod audio;
#[cfg(target_os = "linux")]
pub mod evdev;
pub mod interaction;
pub mod screen;
pub mod sensor;
mod traits;

#[cfg(target_os = "linux")]
pub use evdev::EvdevKeyboardInput;
pub use interaction::InteractionInput;
pub use sensor::SensorPoller;
pub use traits::{InputData, InputSource, InteractionData, KeyboardData, MouseData, ScreenData};

use crate::input::audio::AudioInput;
use crate::types::audio::AudioPipelineConfig;
use crate::types::event::InputEvent;
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::Arc;
use tokio::sync::watch;

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
    sensor_poller: Option<SensorPoller>,
    sensor_snapshot_rx: Option<watch::Receiver<Arc<SystemSnapshot>>>,
}

impl InputManager {
    /// Create an empty manager with no sources.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            sensor_poller: None,
            sensor_snapshot_rx: None,
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

    /// Attach a background system-sensor poller to this input graph.
    pub fn set_sensor_poller(&mut self, poller: SensorPoller) {
        self.set_sensor_snapshot_receiver(poller.receiver());
        self.sensor_poller = Some(poller);
    }

    /// Attach a latest-value sensor stream to this input graph.
    pub fn set_sensor_snapshot_receiver(&mut self, receiver: watch::Receiver<Arc<SystemSnapshot>>) {
        self.sensor_snapshot_rx = Some(receiver);
    }

    /// Clone the configured latest-value sensor receiver, if one exists.
    #[must_use]
    pub fn sensor_snapshot_receiver(&self) -> Option<watch::Receiver<Arc<SystemSnapshot>>> {
        self.sensor_snapshot_rx.as_ref().cloned()
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
        self.sample_all_with_delta_secs(0.0)
    }

    /// Sample every registered source using the current frame delta.
    ///
    /// Sources that ignore cadence can rely on the default trait behavior; the
    /// audio pipeline uses this to keep analysis state aligned with real frame
    /// timing when the render loop shifts tiers or misses budget.
    pub fn sample_all_with_delta_secs(&mut self, delta_secs: f32) -> Vec<InputData> {
        let mut samples = self
            .sources
            .iter_mut()
            .map(|source| {
                source
                    .sample_with_delta_secs(delta_secs)
                    .unwrap_or_else(|err| {
                        error!(source = source.name(), %err, "Input sample failed");
                        InputData::None
                    })
            })
            .collect::<Vec<_>>();

        if let Some(snapshot) = self.latest_sensor_snapshot() {
            samples.push(InputData::Sensors(snapshot));
        }

        samples
    }

    /// Drain discrete input events from every registered source.
    #[must_use]
    pub fn drain_events(&mut self) -> Vec<InputEvent> {
        self.sources
            .iter_mut()
            .flat_map(|source| source.drain_events())
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
        if let Some(sensor_poller) = self.sensor_poller.as_mut() {
            sensor_poller.start()?;
        }

        for (idx, source) in self.sources.iter_mut().enumerate() {
            if let Err(err) = source.start() {
                error!(source = source.name(), %err, "Failed to start input source");
                if let Some(sensor_poller) = self.sensor_poller.as_mut() {
                    sensor_poller.stop();
                }
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
        if let Some(sensor_poller) = self.sensor_poller.as_mut() {
            sensor_poller.stop();
        }
    }

    /// Apply a live audio config change without rebuilding unrelated sources.
    ///
    /// If an audio source already exists, it is reconfigured in place. If audio
    /// is being enabled and no audio source exists yet, one is created and
    /// started. Disabling audio reconfigures the existing source to silence.
    ///
    /// # Errors
    ///
    /// Returns an error if the live audio source switch fails.
    pub fn apply_audio_runtime_config(
        &mut self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        let effective_capture_active = enabled && capture_active;
        let effective_config = if enabled {
            config.clone()
        } else {
            let mut disabled = config.clone();
            disabled.source = crate::types::audio::AudioSourceType::None;
            disabled
        };

        for source in &mut self.sources {
            if source.is_audio_source() {
                source.reconfigure_audio(
                    &effective_config,
                    display_name,
                    effective_capture_active,
                )?;
                info!(
                    source = display_name,
                    enabled,
                    capture_active = effective_capture_active,
                    "Reconfigured live audio input source"
                );
                return Ok(());
            }
        }

        if !enabled {
            return Ok(());
        }

        let mut audio_input = AudioInput::new(&effective_config).with_name(display_name.to_owned());
        audio_input.set_capture_active(effective_capture_active)?;
        audio_input.start()?;
        self.add_source(Box::new(audio_input));
        info!(
            source = display_name,
            capture_active = effective_capture_active,
            "Added live audio input source"
        );
        Ok(())
    }

    /// Toggle live audio capture for any registered audio sources.
    ///
    /// This keeps the input graph intact while allowing the audio backend to
    /// pause or resume hardware capture based on current render demand.
    ///
    /// # Errors
    ///
    /// Returns an error if an audio source cannot update its capture state.
    pub fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        for source in &mut self.sources {
            if source.is_audio_source() {
                source.set_audio_capture_active(active)?;
                info!(
                    source = source.name(),
                    active, "Updated audio capture demand"
                );
            }
        }

        Ok(())
    }

    /// Toggle live screen capture for any registered screen sources.
    ///
    /// This keeps the input graph intact while allowing the capture backend to
    /// pause or resume compositor capture based on current render demand.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot update its capture state.
    pub fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        for source in &mut self.sources {
            if source.is_screen_source() {
                source.set_screen_capture_active(active)?;
                info!(
                    source = source.name(),
                    active, "Updated screen capture demand"
                );
            }
        }

        Ok(())
    }

    /// Return the latest system sensor snapshot, if one is configured.
    #[must_use]
    pub fn latest_sensor_snapshot(&self) -> Option<Arc<SystemSnapshot>> {
        self.sensor_snapshot_rx
            .as_ref()
            .map(|receiver| Arc::clone(&receiver.borrow()))
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}
