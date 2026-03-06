//! Effect engine — manages the active effect and delegates to the renderer.
//!
//! The [`EffectEngine`] is the single orchestrator that owns the current
//! renderer, manages lifecycle transitions, and produces frames on demand.

use std::collections::HashMap;

use tracing::{debug, error, info, warn};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::effect::{ControlValidationError, ControlValue, EffectMetadata, EffectState};

use super::factory::create_renderer_for_metadata;
use super::traits::{EffectRenderer, FrameInput};
use crate::input::InteractionData;

// ── EffectEngine ─────────────────────────────────────────────────────────────

/// Orchestrates the active effect lifecycle and frame production.
///
/// At any given time, the engine holds at most one active renderer. It manages
/// state transitions (`Loading` -> `Initializing` -> `Running` -> `Destroying`),
/// injects per-frame data, and produces canvases for the spatial sampler.
///
/// The engine does **not** own the render loop timer — it is driven externally
/// by the `RenderLoop` which calls [`tick`](Self::tick) at the target framerate.
pub struct EffectEngine {
    /// The currently active renderer, if any.
    renderer: Option<Box<dyn EffectRenderer>>,

    /// Metadata for the active effect, if any.
    metadata: Option<EffectMetadata>,

    /// Current lifecycle state.
    state: EffectState,

    /// Current control values, keyed by control name.
    controls: HashMap<String, ControlValue>,

    /// Cumulative elapsed time since effect activation (seconds).
    elapsed_secs: f32,

    /// Monotonically increasing frame counter.
    frame_number: u64,

    /// Canvas width for frame production.
    canvas_width: u32,

    /// Canvas height for frame production.
    canvas_height: u32,

    /// The currently applied preset ID, if any.
    /// Cleared when controls are tweaked manually or the effect is re-activated.
    active_preset_id: Option<String>,
}

impl EffectEngine {
    /// Create a new engine with no active effect.
    #[must_use]
    pub fn new() -> Self {
        Self {
            renderer: None,
            metadata: None,
            state: EffectState::Loading,
            controls: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
            canvas_width: DEFAULT_CANVAS_WIDTH,
            canvas_height: DEFAULT_CANVAS_HEIGHT,
            active_preset_id: None,
        }
    }

    /// Create an engine with a custom canvas resolution.
    #[must_use]
    pub fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.canvas_width = width;
        self.canvas_height = height;
        self
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub fn state(&self) -> EffectState {
        self.state
    }

    /// Returns the metadata for the active effect, if any.
    #[must_use]
    pub fn active_metadata(&self) -> Option<&EffectMetadata> {
        self.metadata.as_ref()
    }

    /// Returns `true` if an effect is currently loaded and running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state == EffectState::Running && self.renderer.is_some()
    }

    /// Activate a new effect with the given renderer and metadata.
    ///
    /// If an effect is already active, it is destroyed first. The new
    /// renderer is initialized and the engine transitions to `Running`.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's `init` call fails. In that case
    /// the engine returns to an idle state with no active effect.
    pub fn activate(
        &mut self,
        mut renderer: Box<dyn EffectRenderer>,
        metadata: EffectMetadata,
    ) -> anyhow::Result<()> {
        // Tear down any existing effect first
        self.deactivate();

        info!(effect = %metadata.name, "Activating effect");
        self.state = EffectState::Initializing;

        match renderer.init(&metadata) {
            Ok(()) => {
                self.controls = metadata
                    .controls
                    .iter()
                    .map(|control| {
                        (
                            control.control_id().to_owned(),
                            control.default_value.clone(),
                        )
                    })
                    .collect();
                for (name, value) in &self.controls {
                    renderer.set_control(name, value);
                }
                self.renderer = Some(renderer);
                self.metadata = Some(metadata);
                self.elapsed_secs = 0.0;
                self.frame_number = 0;
                self.active_preset_id = None;
                self.state = EffectState::Running;
                debug!("Effect initialized and running");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Effect initialization failed");
                self.state = EffectState::Loading;
                Err(e)
            }
        }
    }

    /// Activate an effect directly from metadata by selecting the correct
    /// renderer implementation (native vs HTML).
    ///
    /// # Errors
    ///
    /// Returns an error if no renderer can be created for the source, or if
    /// the selected renderer fails during initialization.
    pub fn activate_metadata(&mut self, metadata: EffectMetadata) -> anyhow::Result<()> {
        let renderer = create_renderer_for_metadata(&metadata)?;
        self.activate(renderer, metadata)
    }

    /// Deactivate the current effect and release its renderer.
    ///
    /// No-op if no effect is active.
    pub fn deactivate(&mut self) {
        if let Some(ref mut renderer) = self.renderer {
            if let Some(ref meta) = self.metadata {
                info!(effect = %meta.name, "Deactivating effect");
            }
            self.state = EffectState::Destroying;
            renderer.destroy();
        }
        self.renderer = None;
        self.metadata = None;
        self.controls.clear();
        self.elapsed_secs = 0.0;
        self.frame_number = 0;
        self.active_preset_id = None;
        self.state = EffectState::Loading;
    }

    /// Pause the active effect. Renderer stays alive but `tick` returns
    /// an empty canvas.
    pub fn pause(&mut self) {
        if self.state == EffectState::Running {
            debug!("Effect paused");
            self.state = EffectState::Paused;
        } else {
            warn!(state = ?self.state, "Cannot pause — effect is not running");
        }
    }

    /// Resume a paused effect.
    pub fn resume(&mut self) {
        if self.state == EffectState::Paused {
            debug!("Effect resumed");
            self.state = EffectState::Running;
        } else {
            warn!(state = ?self.state, "Cannot resume — effect is not paused");
        }
    }

    /// Update a control parameter value.
    ///
    /// The value is stored and forwarded to the active renderer. If no
    /// renderer is active, the value is stored for when one is activated.
    pub fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
        if let Some(ref mut renderer) = self.renderer {
            renderer.set_control(name, value);
        }
        self.active_preset_id = None;
    }

    /// Validate and update a control value against the active effect schema.
    ///
    /// If no active metadata/control definition exists, the value is forwarded
    /// as-is for backward compatibility.
    pub fn set_control_checked(
        &mut self,
        name: &str,
        value: &ControlValue,
    ) -> Result<ControlValue, ControlValidationError> {
        let (target_name, normalized) = if let Some(definition) = self
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.control_by_id(name))
        {
            (
                definition.control_id().to_owned(),
                definition.validate_value(value)?,
            )
        } else {
            (name.to_owned(), value.clone())
        };
        self.set_control(&target_name, &normalized);
        // Note: active_preset_id already cleared by set_control above
        Ok(normalized)
    }

    /// Snapshot of active control values.
    #[must_use]
    pub fn active_controls(&self) -> &HashMap<String, ControlValue> {
        &self.controls
    }

    /// Returns the currently applied preset ID, or `None` if no preset is active
    /// (either no preset was applied, or controls have been tweaked since).
    #[must_use]
    pub fn active_preset_id(&self) -> Option<&str> {
        self.active_preset_id.as_deref()
    }

    /// Set the active preset ID (called after preset controls are successfully applied).
    pub fn set_active_preset_id(&mut self, id: String) {
        self.active_preset_id = Some(id);
    }

    /// Reset all controls to their metadata-defined defaults.
    ///
    /// Keeps the renderer alive — animation state (elapsed time, frame counter)
    /// is preserved. Clears the active preset ID.
    ///
    /// # Errors
    ///
    /// Returns an error if no effect is currently active.
    pub fn reset_to_defaults(&mut self) -> anyhow::Result<()> {
        let Some(ref metadata) = self.metadata else {
            return Err(anyhow::anyhow!("No active effect to reset"));
        };

        for control in &metadata.controls {
            self.controls.insert(
                control.control_id().to_owned(),
                control.default_value.clone(),
            );
            if let Some(ref mut renderer) = self.renderer {
                renderer.set_control(control.control_id(), &control.default_value);
            }
        }

        self.active_preset_id = None;
        debug!("Controls reset to defaults");
        Ok(())
    }

    /// Produce a single frame.
    ///
    /// Called once per render loop iteration. Returns a black canvas if
    /// no effect is running or the effect is paused.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's `tick` call fails.
    pub fn tick(&mut self, delta_secs: f32, audio: &AudioData) -> anyhow::Result<Canvas> {
        self.tick_with_interaction(delta_secs, audio, &InteractionData::default())
    }

    /// Produce a single frame with host interaction state.
    ///
    /// HTML/Servo effects use `interaction` to populate `engine.keyboard` and
    /// `engine.mouse`, while native effects can ignore it.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's `tick` call fails.
    pub fn tick_with_interaction(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
    ) -> anyhow::Result<Canvas> {
        // If not running, return a blank canvas
        if self.state != EffectState::Running {
            return Ok(Canvas::new(self.canvas_width, self.canvas_height));
        }

        let Some(ref mut renderer) = self.renderer else {
            return Ok(Canvas::new(self.canvas_width, self.canvas_height));
        };

        self.elapsed_secs += delta_secs;

        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio: audio.clone(),
            interaction: interaction.clone(),
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        };

        let canvas = renderer.tick(&input)?;
        self.frame_number += 1;

        Ok(canvas)
    }
}

impl Default for EffectEngine {
    fn default() -> Self {
        Self::new()
    }
}
